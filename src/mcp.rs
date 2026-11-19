//! Model Context Protocol (MCP) stdio server.
//!
//! Speaks JSON-RPC 2.0 over stdin/stdout using MCP's `Content-Length` framing
//! and exposes the simulator's capabilities as MCP *tools*, so an LLM agent
//! can drive instruction-level simulation without spawning individual CLI
//! processes. Every tool is session-driven: state persists across tool calls
//! through a JSON session file (supplied per call or configured as the
//! server's default).
//!
//! ```text
//! ns-instruction-sim mcp [--session default.json]
//! ```
//!
//! Supported MCP methods: `initialize`, `ping`, `tools/list`, `tools/call`.
//! Tool results use the same JSON shapes as the CLI's `--json` output where
//! applicable (notably `state`, `step`, `run`, `reg`, `mem`, `bp`, `disasm`).

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};

use ns_sim::{Simulator, StepResult};

use crate::session::{FirmwareState, Session, arch_name, register_index_by_name};
use crate::sim::{
    default_mem_base, detect_format, load_firmware_into, parse_num, read_code_byte,
    read_mem_byte, write_mem_byte,
};

/// Start the MCP stdio server. `default_session` is the session file used by
/// tools that do not pass an explicit `session` argument.
pub fn run(default_session: Option<PathBuf>) -> Result<()> {
    McpServer { default_session }.serve()
}

struct McpServer {
    default_session: Option<PathBuf>,
}

impl McpServer {
    fn resolve_session(&self, args: &Map<String, Value>) -> Option<PathBuf> {
        args.get("session")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .or_else(|| self.default_session.clone())
    }

    fn serve(self) -> Result<()> {
        let stdin = io::stdin();
        let mut reader = stdin.lock();
        let stdout = io::stdout();
        let mut writer = stdout.lock();
        loop {
            let Some(text) = read_message(&mut reader)? else {
                break;
            };
            let Ok(msg) = serde_json::from_str::<Value>(&text) else {
                continue;
            };
            let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
            let id = msg.get("id").cloned();
            let params = msg.get("params").cloned().unwrap_or(Value::Null);
            // Notifications (no id) and MCP lifecycle notifications need no reply.
            if id.is_none() || method.starts_with("notifications/") {
                continue;
            }
            let response = match self.handle(method, &params) {
                Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                Err(e) => json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32000, "message": e.to_string() },
                }),
            };
            let text = serde_json::to_string(&response)?;
            write_message(&mut writer, &text)?;
        }
        Ok(())
    }

    fn handle(&self, method: &str, params: &Value) -> Result<Value> {
        match method {
            "initialize" => {
                let requested = params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or("2024-11-05");
                let version = if matches!(requested, "2024-11-05" | "2025-03-26" | "2025-06-18") {
                    requested
                } else {
                    "2024-11-05"
                };
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": {} },
                    "serverInfo": {
                        "name": "ns-instruction-sim",
                        "version": env!("CARGO_PKG_VERSION"),
                    },
                }))
            }
            "ping" => Ok(Value::Null),
            "tools/list" => {
                let defs: Vec<_> = tools()
                    .iter()
                    .map(|t| {
                        json!({
                            "name": t.name,
                            "description": t.description,
                            "inputSchema": t.schema,
                        })
                    })
                    .collect();
                Ok(json!({ "tools": defs }))
            }
            "tools/call" => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow::anyhow!("tools/call 缺少 name"))?;
                let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
                let args_map = arguments.as_object().cloned().unwrap_or_default();
                let tool = tools()
                    .into_iter()
                    .find(|t| t.name == name)
                    .ok_or_else(|| anyhow::anyhow!("未知工具: {name}"))?;
                match (tool.handler)(self, &args_map) {
                    Ok(result) => Ok(json!({
                        "content": [
                            { "type": "text", "text": serde_json::to_string_pretty(&result)? }
                        ],
                        "structuredContent": result,
                        "isError": false,
                    })),
                    Err(e) => Ok(json!({
                        "content": [ { "type": "text", "text": format!("错误: {e}") } ],
                        "isError": true,
                    })),
                }
            }
            _ => bail!("未知方法: {method}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Framing
// ---------------------------------------------------------------------------

/// Read one `Content-Length` framed message. Returns `None` on EOF.
fn read_message<R: BufRead>(reader: &mut R) -> Result<Option<String>> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return Ok(None);
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some(value) = trimmed.strip_prefix("Content-Length:") {
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .context("无效 Content-Length")?,
            );
        }
    }
    let len = content_length.context("缺少 Content-Length")?;
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    Ok(Some(
        String::from_utf8(buf).context("消息不是有效 UTF-8")?,
    ))
}

fn write_message<W: Write>(writer: &mut W, text: &str) -> Result<()> {
    write!(writer, "Content-Length: {}\r\n\r\n{}", text.len(), text)?;
    writer.flush()?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Argument helpers
// ---------------------------------------------------------------------------

fn s(args: &Map<String, Value>, key: &str) -> Option<String> {
    args.get(key).map(|v| match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => String::new(),
    })
}

fn u(args: &Map<String, Value>, key: &str) -> Option<u32> {
    args.get(key).and_then(|v| match v {
        Value::Number(n) => n.as_u64().map(|x| x as u32),
        Value::String(s) => s.parse().ok(),
        _ => None,
    })
}

fn u64(args: &Map<String, Value>, key: &str) -> Option<u64> {
    args.get(key).and_then(|v| match v {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    })
}

fn b(args: &Map<String, Value>, key: &str) -> Option<bool> {
    args.get(key).and_then(|v| match v {
        Value::Bool(b) => Some(*b),
        Value::String(s) => match s.as_str() {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => None,
        },
        Value::Number(n) => Some(n.as_u64().unwrap_or(0) != 0),
        _ => None,
    })
}

fn arr(args: &Map<String, Value>, key: &str) -> Vec<String> {
    args.get(key)
        .map(|v| match v {
            Value::Array(a) => a
                .iter()
                .filter_map(|x| match x {
                    Value::String(s) => Some(s.clone()),
                    Value::Number(n) => Some(n.to_string()),
                    _ => None,
                })
                .collect(),
            Value::String(s) => s.split_whitespace().map(String::from).collect(),
            _ => Vec::new(),
        })
        .unwrap_or_default()
}

/// Extract a comma-separated (or array) field list, e.g. `"pc,registers"`.
fn csv(args: &Map<String, Value>, key: &str) -> Vec<String> {
    args.get(key)
        .map(|v| match v {
            Value::Array(a) => a
                .iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect(),
            Value::String(s) => s
                .split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect(),
            _ => Vec::new(),
        })
        .unwrap_or_default()
}

/// Load (or create) the session simulator, run `f`, then save it back when
/// `mutate` is true and a session path is known.
fn with_sim<F>(server: &McpServer, args: &Map<String, Value>, mutate: bool, f: F) -> Result<Value>
where
    F: FnOnce(&mut Simulator, Option<&FirmwareState>, Option<&Path>) -> Result<Value>,
{
    let session_path = server.resolve_session(args);
    let arch = s(args, "arch");
    let firmware = s(args, "firmware");
    let base = s(args, "base").unwrap_or_else(|| "0".to_string());
    let (mut sim, fw, path) = crate::load_or_create(
        session_path.as_deref(),
        arch.as_deref(),
        firmware.as_deref().map(Path::new),
        &base,
    )?;
    let result = f(&mut sim, fw.as_ref(), path.as_deref())?;
    if mutate {
        if let Some(p) = &path {
            Session::capture(&sim, fw).save(p)?;
        }
    }
    Ok(result)
}

/// Full state JSON for a simulator (mirrors the CLI `state --json`).
fn full_state(sim: &Simulator, fw: Option<&FirmwareState>) -> Value {
    serde_json::to_value(crate::state_output(sim, StepResult::Advanced, fw)).unwrap_or(Value::Null)
}

// ---------------------------------------------------------------------------
// Tool definitions
// ---------------------------------------------------------------------------

type Handler = fn(&McpServer, &Map<String, Value>) -> Result<Value>;

struct Tool {
    name: &'static str,
    description: &'static str,
    schema: Value,
    handler: Handler,
}

fn obj(props: &[(&str, Value)], required: &[&str]) -> Value {
    let mut properties = Map::new();
    for (k, v) in props {
        properties.insert(k.to_string(), v.clone());
    }
    json!({ "type": "object", "properties": properties, "required": required })
}

fn prop_str(description: &str) -> Value {
    json!({ "type": "string", "description": description })
}

fn prop_num(description: &str) -> Value {
    json!({ "type": "number", "description": description })
}

fn prop_bool(description: &str) -> Value {
    json!({ "type": "boolean", "description": description })
}

fn prop_arr(description: &str) -> Value {
    json!({ "type": "array", "items": { "type": "string" }, "description": description })
}

/// Input schema shared by the `trace` / `coverage` / `profile` tools.
fn analyze_schema() -> Value {
    obj(
        &[
            ("session", prop_str("会话文件路径（JSON）；缺省使用 MCP 服务器默认会话。")),
            ("arch", prop_str("目标架构（如 riscv/arm/mcs51/...）。")),
            ("firmware", prop_str("固件文件（ELF / Intel HEX / BIN）。")),
            ("base", prop_str("加载基址（十六进制或十进制）。")),
            ("steps", prop_num("精确执行 N 条指令。")),
            ("max_steps", prop_num("最大执行步数（默认 100000）。")),
            ("until", prop_str("运行到指定地址（临时断点）。")),
        ],
        &[],
    )
}

fn tools() -> Vec<Tool> {
    let session = prop_str("会话文件路径（JSON）；缺省使用 MCP 服务器默认会话。");
    let arch = prop_str("目标架构（如 riscv/arm/mcs51/...）。");
    let firmware = prop_str("固件文件（ELF / Intel HEX / BIN）。");
    let base = prop_str("加载基址（十六进制或十进制）。");
    let addr = prop_str("地址（十六进制或十进制）。");

    vec![
        Tool {
            name: "arch_list",
            description: "列出全部支持的架构与示例设备。",
            schema: obj(&[], &[]),
            handler: tool_arch_list,
        },
        Tool {
            name: "device_list",
            description: "列出内置设备（芯片）目录。",
            schema: obj(&[], &[]),
            handler: tool_device_list,
        },
        Tool {
            name: "device_info",
            description: "显示一个内置设备的内存映射、核心与外设寄存器。",
            schema: obj(&[("device", prop_str("设备键，如 stm32f103c8。"))], &["device"]),
            handler: tool_device_info,
        },
        Tool {
            name: "device_svd",
            description: "解析并显示一个 CMSIS-SVD 设备描述文件（或内置设备）。",
            schema: obj(
                &[
                    ("file", prop_str("SVD 文件路径（与 device 二选一）。")),
                    ("device", prop_str("内置设备键（与 file 二选一）。")),
                    ("peripheral", prop_str("只看指定外设（按名称）。")),
                    ("register", prop_str("只看指定寄存器（需配合 peripheral）。")),
                ],
                &[],
            ),
            handler: tool_device_svd,
        },
        Tool {
            name: "device_linker",
            description: "生成 GCC 风格链接脚本。",
            schema: obj(&[("device", prop_str("设备键，如 stm32f103c8。"))], &["device"]),
            handler: tool_device_linker,
        },
        Tool {
            name: "device_startup",
            description: "生成最小启动文件（向量表 + 复位处理器）。",
            schema: obj(&[("device", prop_str("设备键，如 stm32f103c8。"))], &["device"]),
            handler: tool_device_startup,
        },
        Tool {
            name: "device_init",
            description: "生成外设初始化 C 骨架（时钟使能 + 复位值注释）。",
            schema: obj(
                &[
                    ("device", prop_str("设备键，如 stm32f103c8。")),
                    ("peripheral", prop_arr("要初始化的外设（可重复）。")),
                    ("register", prop_arr("寄存器写入 PERIPH.REG=VALUE（可重复）。")),
                ],
                &["device"],
            ),
            handler: tool_device_init,
        },
        Tool {
            name: "session_create",
            description: "创建（或重建）一个会话，可加载固件；返回完整状态。",
            schema: obj(
                &[
                    ("session", session.clone()),
                    ("arch", arch.clone()),
                    ("firmware", firmware.clone()),
                    ("base", base.clone()),
                ],
                &["session"],
            ),
            handler: tool_session_create,
        },
        Tool {
            name: "session_load",
            description: "加载既有会话并返回完整状态。",
            schema: obj(&[("session", session.clone())], &["session"]),
            handler: tool_session_load,
        },
        Tool {
            name: "session_save",
            description: "将当前会话状态保存到磁盘（幂等；返回完整状态）。",
            schema: obj(&[("session", session.clone())], &["session"]),
            handler: tool_session_save,
        },
        Tool {
            name: "state",
            description: "读取当前完整状态（寄存器/内存/外设/CSR/断点/固件/符号），可用 fields 选取字段子集。",
            schema: obj(
                &[
                    ("session", session.clone()),
                    ("arch", arch.clone()),
                    ("firmware", firmware.clone()),
                    ("base", base.clone()),
                    ("fields", prop_str("逗号分隔的 JSON 字段子集（如 pc,registers,csr）。")),
                ],
                &[],
            ),
            handler: tool_state,
        },
        Tool {
            name: "step",
            description: "单步执行一条指令，返回执行后的状态。",
            schema: obj(
                &[
                    ("session", session.clone()),
                    ("arch", arch.clone()),
                    ("firmware", firmware.clone()),
                    ("base", base.clone()),
                    ("max_steps", prop_num("步过/步出最大执行步数（默认 100000）。")),
                ],
                &[],
            ),
            handler: tool_step,
        },
        Tool {
            name: "step_over",
            description: "步过：执行并停在调用者下一条指令。",
            schema: obj(
                &[
                    ("session", session.clone()),
                    ("arch", arch.clone()),
                    ("firmware", firmware.clone()),
                    ("base", base.clone()),
                    ("max_steps", prop_num("最大执行步数（默认 100000）。")),
                ],
                &[],
            ),
            handler: tool_step_over,
        },
        Tool {
            name: "step_out",
            description: "步出：运行至当前函数返回。",
            schema: obj(
                &[
                    ("session", session.clone()),
                    ("arch", arch.clone()),
                    ("firmware", firmware.clone()),
                    ("base", base.clone()),
                    ("max_steps", prop_num("最大执行步数（默认 100000）。")),
                ],
                &[],
            ),
            handler: tool_step_out,
        },
        Tool {
            name: "run",
            description: "加载固件并运行到结束/步数上限/指定地址，返回状态。",
            schema: obj(
                &[
                    ("session", session.clone()),
                    ("arch", arch.clone()),
                    ("firmware", firmware.clone()),
                    ("base", base.clone()),
                    ("steps", prop_num("只运行 N 条指令。")),
                    ("max_steps", prop_num("最大执行步数（默认 1000000）。")),
                    ("until", addr.clone()),
                    ("cycle_accurate", prop_bool("周期精确模式（按指令预估周期推进）。")),
                ],
                &["firmware"],
            ),
            handler: tool_run,
        },
        Tool {
            name: "continue",
            description: "从当前 PC 连续运行至断点/停止/步数上限/指定地址，返回状态。",
            schema: obj(
                &[
                    ("session", session.clone()),
                    ("until", addr.clone()),
                    ("max_steps", prop_num("最大执行步数（默认 1000000）。")),
                ],
                &["session"],
            ),
            handler: tool_continue,
        },
        Tool {
            name: "reg_list",
            description: "列出全部寄存器（名称 -> 值）。",
            schema: obj(&[("session", session.clone()), ("arch", arch.clone())], &[]),
            handler: tool_reg_list,
        },
        Tool {
            name: "reg_get",
            description: "读取单个寄存器。",
            schema: obj(
                &[
                    ("session", session.clone()),
                    ("arch", arch.clone()),
                    ("name", prop_str("寄存器名，如 x1、a0、R3、SP。")),
                ],
                &["name"],
            ),
            handler: tool_reg_get,
        },
        Tool {
            name: "reg_set",
            description: "写入单个寄存器并持久化。",
            schema: obj(
                &[
                    ("session", session.clone()),
                    ("name", prop_str("寄存器名，如 x1、a0、R3、SP。")),
                    ("value", prop_str("值（十六进制或十进制）。")),
                ],
                &["name", "value"],
            ),
            handler: tool_reg_set,
        },
        Tool {
            name: "mem_read",
            description: "读取内存（字节/半字/字/双字）。",
            schema: obj(
                &[
                    ("session", session.clone()),
                    ("arch", arch.clone()),
                    ("addr", addr.clone()),
                    ("len", prop_num("元素个数（默认 64）。")),
                    ("width", prop_str("数据宽度：byte/half/word/dword。")),
                ],
                &["addr"],
            ),
            handler: tool_mem_read,
        },
        Tool {
            name: "mem_write",
            description: "写入内存（字节序列或按宽度写入小端值）。",
            schema: obj(
                &[
                    ("session", session.clone()),
                    ("arch", arch.clone()),
                    ("addr", addr.clone()),
                    ("bytes", prop_arr("字节序列（支持 0xNN）。")),
                    ("values", prop_arr("值序列（配合 width，十进制/十六进制）。")),
                    ("width", prop_str("数据宽度：byte/half/word。")),
                    ("file", prop_str("从二进制文件读取字节写入。")),
                ],
                &["addr"],
            ),
            handler: tool_mem_write,
        },
        Tool {
            name: "mem_fill",
            description: "填充内存区域为指定字节。",
            schema: obj(
                &[
                    ("session", session.clone()),
                    ("arch", arch.clone()),
                    ("addr", addr.clone()),
                    ("len", prop_num("长度（字节）。")),
                    ("value", prop_str("填充字节（0-255）。")),
                ],
                &["addr", "len", "value"],
            ),
            handler: tool_mem_fill,
        },
        Tool {
            name: "mem_search",
            description: "在内存区域搜索字节模式，返回所有偏移。",
            schema: obj(
                &[
                    ("session", session.clone()),
                    ("arch", arch.clone()),
                    ("addr", addr.clone()),
                    ("len", prop_num("搜索长度（默认 256）。")),
                    ("pattern", prop_arr("搜索模式字节。")),
                ],
                &["addr", "pattern"],
            ),
            handler: tool_mem_search,
        },
        Tool {
            name: "mem_load",
            description: "加载固件到内存（不执行），返回状态。",
            schema: obj(
                &[
                    ("session", session.clone()),
                    ("arch", arch.clone()),
                    ("firmware", firmware.clone()),
                    ("base", base.clone()),
                ],
                &["firmware"],
            ),
            handler: tool_mem_load,
        },
        Tool {
            name: "mem_dump",
            description: "导出内存区域（C 数组 / Intel HEX / hex+ASCII 文本）。",
            schema: obj(
                &[
                    ("session", session.clone()),
                    ("arch", arch.clone()),
                    ("addr", addr.clone()),
                    ("len", prop_num("长度（字节，默认 64）。")),
                    ("name", prop_str("C 数组名（仅 format=c）。")),
                    ("format", prop_str("输出格式：c/hex/dump。")),
                ],
                &["addr"],
            ),
            handler: tool_mem_dump,
        },
        Tool {
            name: "bp_set",
            description: "设置断点（按地址/符号/源码行号，支持条件/命中次数/一次性）。",
            schema: obj(
                &[
                    ("session", session.clone()),
                    ("addr", addr.clone()),
                    ("symbol", prop_str("按符号名设置断点。")),
                    ("line", prop_str("按源码行号设置断点（file:line）。")),
                    ("condition", prop_str("条件表达式，如 x1 == 10。")),
                    ("hit_count", prop_num("命中次数（第 N 次命中时暂停）。")),
                    ("once", prop_bool("一次性断点：命中后自动清除。")),
                ],
                &[],
            ),
            handler: tool_bp_set,
        },
        Tool {
            name: "bp_clear",
            description: "清除指定地址的断点。",
            schema: obj(&[("session", session.clone()), ("addr", addr.clone())], &["addr"]),
            handler: tool_bp_clear,
        },
        Tool {
            name: "bp_list",
            description: "列出全部断点（地址/条件/命中次数/一次性/启用/实际命中）。",
            schema: obj(&[("session", session.clone())], &["session"]),
            handler: tool_bp_list,
        },
        Tool {
            name: "bp_enable",
            description: "启用指定地址的断点。",
            schema: obj(&[("session", session.clone()), ("addr", addr.clone())], &["addr"]),
            handler: tool_bp_enable,
        },
        Tool {
            name: "bp_disable",
            description: "禁用指定地址的断点（保留但不再暂停）。",
            schema: obj(&[("session", session.clone()), ("addr", addr.clone())], &["addr"]),
            handler: tool_bp_disable,
        },
        Tool {
            name: "bp_clear_all",
            description: "清除全部断点。",
            schema: obj(&[("session", session.clone())], &["session"]),
            handler: tool_bp_clear_all,
        },
        Tool {
            name: "disasm",
            description: "反汇编 N 条指令（地址/反汇编/字节/来源/ARM 状态）。",
            schema: obj(
                &[
                    ("session", session.clone()),
                    ("arch", arch.clone()),
                    ("addr", addr.clone()),
                    ("count", prop_num("指令条数（默认 8）。")),
                    ("arm_state", prop_str("ARM 状态：a32 或 thumb。")),
                    ("raw", prop_bool("是否包含原始字节。")),
                    ("source", prop_bool("是否标注来源 file:line。")),
                ],
                &[],
            ),
            handler: tool_disasm,
        },
        Tool {
            name: "reset",
            description: "复位仿真器（PC 回入口/指定地址）。",
            schema: obj(&[("session", session.clone()), ("entry", addr.clone())], &["session"]),
            handler: tool_reset,
        },
        Tool {
            name: "cycle_get",
            description: "读取当前周期计数。",
            schema: obj(&[("session", session.clone())], &["session"]),
            handler: tool_cycle_get,
        },
        Tool {
            name: "cycle_estimate",
            description: "估算指定地址的指令周期数。",
            schema: obj(&[("session", session.clone()), ("addr", addr.clone())], &["addr"]),
            handler: tool_cycle_estimate,
        },
        Tool {
            name: "cycle_tick",
            description: "推进外设 N 个核心周期（不执行指令）。",
            schema: obj(
                &[
                    ("session", session.clone()),
                    ("cycles", prop_num("周期数（默认 1）。")),
                ],
                &["session"],
            ),
            handler: tool_cycle_tick,
        },
        Tool {
            name: "interrupt_take",
            description: "触发一次中断（需要 mstatus.MIE 且 mie 对应位使能）。",
            schema: obj(&[("session", session.clone())], &["session"]),
            handler: tool_interrupt_take,
        },
        Tool {
            name: "interrupt_pending",
            description: "查询当前挂起/使能的中断。",
            schema: obj(&[("session", session.clone())], &["session"]),
            handler: tool_interrupt_pending,
        },
        Tool {
            name: "interrupt_set",
            description: "设置/清除外设中断线。",
            schema: obj(
                &[
                    ("session", session.clone()),
                    ("line", prop_num("中断线 0..15。")),
                    ("value", prop_num("置位/清除（默认 1）。")),
                ],
                &["line"],
            ),
            handler: tool_interrupt_set,
        },
        Tool {
            name: "periph_status",
            description: "显示虚拟外设完整状态（UART/GPIO/定时器/看门狗/寄存器）。",
            schema: obj(&[("session", session.clone())], &["session"]),
            handler: tool_periph_status,
        },
        Tool {
            name: "periph_config",
            description: "配置虚拟外设基址（UART/GPIO 重定位并持久化）。",
            schema: obj(
                &[
                    ("session", session.clone()),
                    ("uart_base", prop_str("UART 寄存器基址（默认 0x40000000）。")),
                    ("gpio_base", prop_str("GPIO 寄存器基址（默认 0x40002000）。")),
                ],
                &[],
            ),
            handler: tool_periph_config,
        },
        Tool {
            name: "periph_uart",
            description: "读取 UART 输出（可清空缓冲）。",
            schema: obj(
                &[
                    ("session", session.clone()),
                    ("clear", prop_bool("清空输出缓冲。")),
                ],
                &["session"],
            ),
            handler: tool_periph_uart,
        },
        Tool {
            name: "periph_gpio",
            description: "设置 GPIO 输入引脚值。",
            schema: obj(
                &[
                    ("session", session.clone()),
                    ("pin", prop_num("引脚号 0-31。")),
                    ("value", prop_num("引脚值 0/1。")),
                ],
                &["pin", "value"],
            ),
            handler: tool_periph_gpio,
        },
        Tool {
            name: "periph_timer",
            description: "查询/配置虚拟定时器。",
            schema: obj(
                &[
                    ("session", session.clone()),
                    ("index", prop_num("定时器索引（0..8）。")),
                    ("enabled", prop_bool("启用/禁用。")),
                    ("prescaler", prop_num("预分频。")),
                    ("auto_reload", prop_num("自动重载值。")),
                ],
                &["session"],
            ),
            handler: tool_periph_timer,
        },
        Tool {
            name: "periph_reg_read",
            description: "读取外设内存映射寄存器（可触发外设读）。",
            schema: obj(
                &[
                    ("session", session.clone()),
                    ("addr", addr.clone()),
                    ("name", prop_str("寄存器名 PERIPH.REG（配合 device）。")),
                    ("device", prop_str("内置设备键（配合 name）。")),
                    ("len", prop_num("读取字节数（默认 4）。")),
                ],
                &[],
            ),
            handler: tool_periph_reg_read,
        },
        Tool {
            name: "periph_reg_write",
            description: "写入外设内存映射寄存器（可触发外设写，如 UART 输出）。",
            schema: obj(
                &[
                    ("session", session.clone()),
                    ("addr", addr.clone()),
                    ("name", prop_str("寄存器名 PERIPH.REG（配合 device）。")),
                    ("device", prop_str("内置设备键（配合 name）。")),
                    ("bytes", prop_arr("要写入的字节序列。")),
                ],
                &[],
            ),
            handler: tool_periph_reg_write,
        },
        Tool {
            name: "periph_reg_list",
            description: "列出会话中已加载（SVD）的外设寄存器。",
            schema: obj(
                &[
                    ("session", session.clone()),
                    ("device", prop_str("用内置设备键注释寄存器名。")),
                ],
                &["session"],
            ),
            handler: tool_periph_reg_list,
        },
        Tool {
            name: "periph_field_get",
            description: "读取外设寄存器的命名位域（SVD field）。",
            schema: obj(
                &[
                    ("session", session.clone()),
                    ("device", prop_str("内置设备键。")),
                    ("peripheral", prop_str("外设名。")),
                    ("register", prop_str("寄存器名。")),
                    ("field", prop_str("位域名。")),
                ],
                &["device", "peripheral", "register", "field"],
            ),
            handler: tool_periph_field_get,
        },
        Tool {
            name: "periph_field_set",
            description: "写外设寄存器的命名位域（保留其他位）。",
            schema: obj(
                &[
                    ("session", session.clone()),
                    ("device", prop_str("内置设备键。")),
                    ("peripheral", prop_str("外设名。")),
                    ("register", prop_str("寄存器名。")),
                    ("field", prop_str("位域名。")),
                    ("value", prop_str("字段值（十进制/十六进制）。")),
                ],
                &["device", "peripheral", "register", "field", "value"],
            ),
            handler: tool_periph_field_set,
        },
        Tool {
            name: "periph_svd_load",
            description: "从 SVD 文件或内置设备加载芯片寄存器到仿真器。",
            schema: obj(
                &[
                    ("session", session.clone()),
                    ("file", prop_str("SVD 文件路径。")),
                    ("device", prop_str("内置设备键。")),
                    ("peripheral", prop_str("只加载指定外设。")),
                    ("force", prop_bool("覆盖既有寄存器值（默认 false）。")),
                ],
                &[],
            ),
            handler: tool_periph_svd_load,
        },
        Tool {
            name: "symbol_list",
            description: "列出 ELF 符号表。",
            schema: obj(
                &[
                    ("session", session.clone()),
                    ("filter", prop_str("按名称子串过滤。")),
                ],
                &["session"],
            ),
            handler: tool_symbol_list,
        },
        Tool {
            name: "symbol_at",
            description: "查询地址处最接近的符号。",
            schema: obj(&[("session", session.clone()), ("addr", addr.clone())], &["addr"]),
            handler: tool_symbol_at,
        },
        Tool {
            name: "symbol_line",
            description: "地址 -> (源文件, 行号) 映射。",
            schema: obj(&[("session", session.clone()), ("addr", addr.clone())], &["addr"]),
            handler: tool_symbol_line,
        },
        Tool {
            name: "symbol_addr",
            description: "(源文件索引, 行号) -> 地址 映射。",
            schema: obj(
                &[
                    ("session", session.clone()),
                    ("file", prop_num("源文件索引（0 基）。")),
                    ("line", prop_num("源行号（1 基）。")),
                ],
                &["file", "line"],
            ),
            handler: tool_symbol_addr,
        },
        Tool {
            name: "watchdog_status",
            description: "查询看门狗状态。",
            schema: obj(&[("session", session.clone())], &["session"]),
            handler: tool_watchdog_status,
        },
        Tool {
            name: "watchdog_feed",
            description: "喂狗（写 0xAA55 重载超时）。",
            schema: obj(&[("session", session.clone())], &["session"]),
            handler: tool_watchdog_feed,
        },
        Tool {
            name: "watchdog_config",
            description: "配置看门狗（启用/禁用、超时周期、基址）。",
            schema: obj(
                &[
                    ("session", session.clone()),
                    ("enabled", prop_bool("启用/禁用。")),
                    ("timeout", prop_num("超时周期数（0 = 禁用）。")),
                    ("base", addr.clone()),
                ],
                &["session"],
            ),
            handler: tool_watchdog_config,
        },
        Tool {
            name: "watchdog_reset",
            description: "强制触发看门狗复位（记录复位事件并复位核心）。",
            schema: obj(
                &[("session", session.clone()), ("entry", addr.clone())],
                &["session"],
            ),
            handler: tool_watchdog_reset,
        },
        Tool {
            name: "csr_list",
            description: "列出全部 CSR（RISC-V）。",
            schema: obj(&[("session", session.clone())], &["session"]),
            handler: tool_csr_list,
        },
        Tool {
            name: "csr_get",
            description: "读取单个 CSR。",
            schema: obj(
                &[
                    ("session", session.clone()),
                    ("name", prop_str("CSR 名（mstatus/mtvec/mepc/mcause/mie/mhartid）。")),
                ],
                &["name"],
            ),
            handler: tool_csr_get,
        },
        Tool {
            name: "csr_set",
            description: "写入单个 CSR。",
            schema: obj(
                &[
                    ("session", session.clone()),
                    ("name", prop_str("CSR 名（mstatus/mtvec/mepc/mcause/mie/mhartid）。")),
                    ("value", prop_str("值（十六进制或十进制）。")),
                ],
                &["name", "value"],
            ),
            handler: tool_csr_set,
        },
        Tool {
            name: "trace",
            description: "运行并输出结构化指令轨迹（每条指令的 PC/反汇编/字节/来源/周期）。",
            schema: analyze_schema(),
            handler: tool_trace,
        },
        Tool {
            name: "coverage",
            description: "运行并统计执行覆盖（各 PC 执行次数与唯一 PC 数）。",
            schema: analyze_schema(),
            handler: tool_coverage,
        },
        Tool {
            name: "profile",
            description: "运行并统计指令助记符直方图。",
            schema: analyze_schema(),
            handler: tool_profile,
        },
    ]
}

// ---------------------------------------------------------------------------
// Tool handlers
// ---------------------------------------------------------------------------

fn tool_arch_list(_: &McpServer, _: &Map<String, Value>) -> Result<Value> {
    let archs = [
        ("riscv", "RISC-V (RV32I)", "esp32c3"),
        ("arm", "ARM Cortex-M", "stm32f103c8"),
        ("mcs51", "Intel 8051 (MCS-51)", "mcs51"),
        ("mcs251", "Intel 80251 (MCS-251)", "80251"),
        ("c166", "Infineon C166 (16-bit)", "c166"),
        ("xtensa", "ESP32 Xtensa", "esp32"),
        ("avr", "Atmel AVR (8-bit)", "atmega328p"),
        ("msp430", "TI MSP430 (16-bit)", "msp430g2553"),
        ("v850", "Renesas V850 (32-bit)", "v850es"),
        ("stm8", "STMicroelectronics STM8", "stm8s003f3"),
        ("rx", "Renesas RX (32-bit)", "rx62n"),
        ("rl78", "Renesas RL78 (16-bit)", "rl78g14"),
    ];
    let items: Vec<_> = archs
        .iter()
        .map(|(name, label, device)| {
            json!({ "name": name, "label": label, "exampleDevice": device })
        })
        .collect();
    Ok(json!({ "architectures": items }))
}

fn tool_device_list(_: &McpServer, _: &Map<String, Value>) -> Result<Value> {
    let items: Vec<_> = ns_device::CATALOG
        .iter()
        .map(|e| {
            json!({
                "key": e.key,
                "label": e.label,
                "core": e.core,
                "flashSize": e.flash_size,
                "ramSize": e.ram_size,
            })
        })
        .collect();
    Ok(json!({ "devices": items }))
}

fn tool_device_info(_: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let device = s(args, "device").ok_or_else(|| anyhow::anyhow!("缺少 device"))?;
    let entry = ns_device::catalog_entry(&device)
        .ok_or_else(|| anyhow::anyhow!("未知设备: {device}"))?;
    let regions: Vec<_> = ns_device::memory_map(&device)
        .iter()
        .map(|r| json!({ "name": r.name, "start": r.start, "size": r.size }))
        .collect();
    let peripherals = ns_device::device_for_target(&device)
        .map(|dev| {
            dev.sorted_peripherals()
                .iter()
                .map(|p| {
                    json!({
                        "name": p.name,
                        "baseAddress": p.base_address,
                        "description": p.description,
                        "registers": p.registers
                            .iter()
                            .map(|r| json!({
                                "name": r.name,
                                "address": r.address,
                                "resetValue": r.reset_value,
                                "description": r.description,
                            }))
                            .collect::<Vec<_>>(),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(json!({
        "key": entry.key,
        "label": entry.label,
        "core": entry.core,
        "flashStart": entry.flash_start,
        "flashSize": entry.flash_size,
        "ramStart": entry.ram_start,
        "ramSize": entry.ram_size,
        "memoryMap": regions,
        "peripherals": peripherals,
    }))
}

fn mcp_field_json(f: &ns_device::Field) -> Value {
    json!({
        "name": f.name,
        "bitOffset": f.bit_offset,
        "bitWidth": f.bit_width,
        "description": f.description,
    })
}

fn mcp_register_json(r: &ns_device::Register) -> Value {
    json!({
        "name": r.name,
        "address": r.address,
        "size": r.size,
        "resetValue": r.reset_value,
        "description": r.description,
        "fields": r.fields.iter().map(mcp_field_json).collect::<Vec<_>>(),
    })
}

fn mcp_peripheral_json(p: &ns_device::Peripheral) -> Value {
    json!({
        "name": p.name,
        "baseAddress": p.base_address,
        "description": p.description,
        "registers": p.registers.iter().map(mcp_register_json).collect::<Vec<_>>(),
    })
}

fn mcp_device_json(d: &ns_device::Device, peripherals: &[&ns_device::Peripheral]) -> Value {
    json!({
        "name": d.name,
        "vendor": d.vendor,
        "core": d.core,
        "memory": d.memory.iter().map(|m| json!({
            "name": m.name,
            "start": m.start,
            "size": m.size,
            "access": m.access,
        })).collect::<Vec<_>>(),
        "peripherals": peripherals.iter().map(|p| mcp_peripheral_json(p)).collect::<Vec<_>>(),
    })
}

fn tool_device_svd(_: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let file = s(args, "file");
    let device_key = s(args, "device");
    let peripheral = s(args, "peripheral");
    let register = s(args, "register");
    let device = match (file, device_key) {
        (Some(f), None) => ns_device::load_svd_file(Path::new(&f))?,
        (None, Some(k)) => ns_device::device_for_target(&k)
            .ok_or_else(|| anyhow::anyhow!("未知设备或缺少内置 SVD: {k}"))?,
        (Some(_), Some(_)) => bail!("file 与 device 只能二选一"),
        (None, None) => bail!("请提供 file 或 device"),
    };
    let peripherals: Vec<&ns_device::Peripheral> = match &peripheral {
        Some(name) => vec![device
            .peripheral(name)
            .ok_or_else(|| anyhow::anyhow!("SVD 中无外设: {name}"))?],
        None => device.sorted_peripherals(),
    };
    if let Some(reg_name) = &register {
        let p = peripherals
            .first()
            .ok_or_else(|| anyhow::anyhow!("register 需要先指定 peripheral"))?;
        let reg = p
            .registers
            .iter()
            .find(|r| &r.name == reg_name)
            .ok_or_else(|| anyhow::anyhow!("外设 {} 中无寄存器: {reg_name}", p.name))?;
        return Ok(mcp_register_json(reg));
    }
    Ok(mcp_device_json(&device, &peripherals))
}

fn tool_device_linker(_: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let device = s(args, "device").ok_or_else(|| anyhow::anyhow!("缺少 device"))?;
    if ns_device::catalog_entry(&device).is_none() {
        bail!("未知设备: {device}");
    }
    let content = ns_device::linker_script(&device)
        .ok_or_else(|| anyhow::anyhow!("无法生成链接脚本：设备 {device} 不受支持"))?;
    Ok(json!({ "device": device, "kind": "linker", "content": content }))
}

fn tool_device_startup(_: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let device = s(args, "device").ok_or_else(|| anyhow::anyhow!("缺少 device"))?;
    if ns_device::catalog_entry(&device).is_none() {
        bail!("未知设备: {device}");
    }
    let content = ns_device::startup_file(&device)
        .ok_or_else(|| anyhow::anyhow!("无法生成启动文件：设备 {device} 不受支持"))?;
    Ok(json!({ "device": device, "kind": "startup", "content": content }))
}

fn tool_device_init(_: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let device = s(args, "device").ok_or_else(|| anyhow::anyhow!("缺少 device"))?;
    if ns_device::catalog_entry(&device).is_none() {
        bail!("未知设备: {device}");
    }
    let peripheral_names = arr(args, "peripheral");
    let register_overrides = arr(args, "register");
    let names: Vec<String> = if peripheral_names.is_empty() {
        let dev = ns_device::device_for_target(&device)
            .ok_or_else(|| anyhow::anyhow!("设备 {device} 无内置 SVD，请用 peripheral 指定"))?;
        dev.sorted_peripherals().iter().map(|p| p.name.clone()).collect()
    } else {
        peripheral_names
    };
    let mut register_values: std::collections::BTreeMap<String, std::collections::BTreeMap<String, u64>> =
        std::collections::BTreeMap::new();
    for spec in &register_overrides {
        let (left, value) = spec
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("register 格式应为 PERIPH.REG=VALUE: {spec}"))?;
        let (periph, reg) = left
            .split_once('.')
            .ok_or_else(|| anyhow::anyhow!("register 格式应为 PERIPH.REG=VALUE: {spec}"))?;
        let value = parse_num(value)?;
        register_values
            .entry(periph.to_string())
            .or_default()
            .insert(reg.to_string(), value as u64);
    }
    let configs: Vec<ns_device::PeripheralConfig> = names
        .iter()
        .map(|name| ns_device::PeripheralConfig {
            name: name.clone(),
            purpose: String::new(),
            options: Vec::new(),
            register_values: register_values.get(name).cloned().unwrap_or_default(),
        })
        .collect();
    let content = ns_device::generate_peripheral_init(&device, &configs)
        .ok_or_else(|| anyhow::anyhow!("无法生成外设初始化：设备 {device} 不受支持"))?;
    Ok(json!({ "device": device, "kind": "init", "content": content }))
}

fn tool_session_create(_server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let session = s(args, "session").ok_or_else(|| anyhow::anyhow!("缺少 session"))?;
    let path = PathBuf::from(&session);
    let arch = s(args, "arch").unwrap_or_else(|| "riscv".to_string());
    let firmware = s(args, "firmware");
    let base = parse_num(s(args, "base").as_deref().unwrap_or("0"))?;
    let mut sim = Simulator::for_arch(crate::session::parse_arch(&arch)?)?;
    let mut fw = None;
    if let Some(f) = &firmware {
        load_firmware_into(&mut sim, Path::new(f), base)?;
        fw = Some(FirmwareState {
            path: f.clone(),
            base,
            format: detect_format(Path::new(f)).label().to_string(),
        });
    }
    Session::capture(&sim, fw.clone()).save(&path)?;
    Ok(full_state(&sim, fw.as_ref()))
}

fn tool_session_load(_server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let session = s(args, "session").ok_or_else(|| anyhow::anyhow!("缺少 session"))?;
    let path = Path::new(&session);
    if !path.exists() {
        bail!("会话不存在: {session}");
    }
    let loaded = Session::load(path)?;
    let sim = loaded.apply(None)?;
    Ok(full_state(&sim, loaded.firmware.as_ref()))
}

fn tool_session_save(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    with_sim(server, args, true, |sim, fw, _| Ok(full_state(sim, fw)))
}

fn tool_state(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let fields = csv(args, "fields");
    with_sim(server, args, false, |sim, fw, _| {
        let mut value = full_state(sim, fw);
        if !fields.is_empty() {
            value = crate::select_json_fields(&value, &fields)?;
        }
        Ok(value)
    })
}

fn tool_step(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    with_sim(server, args, true, |sim, fw, _| {
        let result = sim.step();
        Ok(serde_json::to_value(crate::state_output(sim, result, fw))?)
    })
}

fn tool_step_over(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let max = u64(args, "max_steps").unwrap_or(100_000);
    with_sim(server, args, true, |sim, fw, _| {
        let result = sim.step_over(max);
        Ok(serde_json::to_value(crate::state_output(sim, result, fw))?)
    })
}

fn tool_step_out(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let max = u64(args, "max_steps").unwrap_or(100_000);
    with_sim(server, args, true, |sim, fw, _| {
        let result = sim.step_out(max);
        Ok(serde_json::to_value(crate::state_output(sim, result, fw))?)
    })
}

fn tool_run(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let firmware = s(args, "firmware").ok_or_else(|| anyhow::anyhow!("缺少 firmware"))?;
    let base = parse_num(s(args, "base").as_deref().unwrap_or("0"))?;
    let (mut sim, _, _) = crate::load_or_create(
        server.resolve_session(args).as_deref(),
        s(args, "arch").as_deref(),
        None,
        "0",
    )?;
    let format = detect_format(Path::new(&firmware));
    let fw_state = FirmwareState {
        path: firmware.clone(),
        base,
        format: format.label().to_string(),
    };
    load_firmware_into(&mut sim, Path::new(&firmware), base)?;
    sim.cycle_accurate = b(args, "cycle_accurate").unwrap_or(false);
    let until_addr = s(args, "until").map(|x| parse_num(&x)).transpose()?;
    if let Some(addr) = until_addr {
        if addr > sim.pc_limit() {
            bail!("until 地址 0x{addr:08x} 超出 {} 的 PC 可寻址范围", sim.arch.label());
        }
    }
    let temp_bp = until_addr.map(|a| sim.set_breakpoint(a)).unwrap_or(false);
    let result = match u64(args, "steps") {
        Some(n) => sim.run(n),
        None => sim.run(u64(args, "max_steps").unwrap_or(1_000_000)),
    };
    if temp_bp {
        if let Some(addr) = until_addr {
            sim.clear_breakpoint(addr);
        }
    }
    let output = serde_json::to_value(crate::state_output(&sim, result, Some(&fw_state)))?;
    if let Some(p) = server.resolve_session(args) {
        Session::capture(&sim, Some(fw_state)).save(&p)?;
    }
    Ok(output)
}

fn tool_continue(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    with_sim(server, args, true, |sim, fw, _| {
        let until_addr = s(args, "until").map(|x| parse_num(&x)).transpose()?;
        if let Some(addr) = until_addr {
            if addr > sim.pc_limit() {
                bail!("until 地址 0x{addr:08x} 超出 {} 的 PC 可寻址范围", sim.arch.label());
            }
        }
        let temp_bp = until_addr.map(|a| sim.set_breakpoint(a)).unwrap_or(false);
        let result = sim.run(u64(args, "max_steps").unwrap_or(1_000_000));
        if temp_bp {
            if let Some(addr) = until_addr {
                sim.clear_breakpoint(addr);
            }
        }
        Ok(serde_json::to_value(crate::state_output(sim, result, fw))?)
    })
}

fn tool_reg_list(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    with_sim(server, args, false, |sim, _, _| {
        let mut regs = Map::new();
        let count = ns_sim::reg_count_for(sim.arch);
        for i in 0..count {
            regs.insert(ns_sim::reg_label_for(sim.arch, i), json!(sim.read_reg(i)));
        }
        Ok(json!({ "arch": arch_name(sim.arch), "registers": regs }))
    })
}

fn tool_reg_get(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let name = s(args, "name").ok_or_else(|| anyhow::anyhow!("缺少 name"))?;
    with_sim(server, args, false, |sim, _, _| {
        let map = register_index_by_name(sim.arch);
        let idx = *map
            .get(&name.to_ascii_lowercase())
            .ok_or_else(|| anyhow::anyhow!("未知寄存器: {name}"))?;
        Ok(json!({ "name": name, "index": idx, "value": sim.read_reg(idx) }))
    })
}

fn tool_reg_set(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let name = s(args, "name").ok_or_else(|| anyhow::anyhow!("缺少 name"))?;
    let value = parse_num(&s(args, "value").ok_or_else(|| anyhow::anyhow!("缺少 value"))?)?;
    with_sim(server, args, true, |sim, _, _| {
        let map = register_index_by_name(sim.arch);
        let idx = *map
            .get(&name.to_ascii_lowercase())
            .ok_or_else(|| anyhow::anyhow!("未知寄存器: {name}"))?;
        sim.write_reg(idx, value);
        Ok(json!({ "name": name, "index": idx, "value": value }))
    })
}

fn tool_mem_read(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let addr_num = parse_num(&s(args, "addr").ok_or_else(|| anyhow::anyhow!("缺少 addr"))?)?;
    let len = u(args, "len").unwrap_or(64) as usize;
    let width = s(args, "width").unwrap_or_else(|| "byte".to_string());
    with_sim(server, args, false, |sim, _, _| {
        let elem_size = match width.as_str() {
            "byte" => 1usize,
            "half" => 2usize,
            "word" => 4usize,
            "dword" => 8usize,
            other => bail!("不支持的内存宽度: {other}（支持 byte/half/word/dword）"),
        };
        let mut bytes = Vec::with_capacity(len * elem_size);
        for i in 0..len {
            let a = addr_num.wrapping_add((i * elem_size) as u32);
            for j in 0..elem_size {
                bytes.push(read_mem_byte(sim, a.wrapping_add(j as u32)));
            }
        }
        Ok(json!({ "address": addr_num, "len": len, "width": width, "bytes": bytes }))
    })
}

fn tool_mem_write(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let addr_num = parse_num(&s(args, "addr").ok_or_else(|| anyhow::anyhow!("缺少 addr"))?)?;
    let width = s(args, "width").unwrap_or_else(|| "byte".to_string());
    let mut data = Vec::new();
    if let Some(file) = s(args, "file") {
        data = std::fs::read(&file).with_context(|| format!("读取 {file} 失败"))?;
    } else {
        let elem_size = match width.as_str() {
            "byte" => 1usize,
            "half" => 2usize,
            "word" => 4usize,
            other => bail!("不支持的内存宽度: {other}（支持 byte/half/word）"),
        };
        if elem_size > 1 {
            for v in arr(args, "values") {
                let n = parse_num(&v)? as u64;
                let max = (1u64 << (elem_size * 8)) - 1;
                if n > max {
                    bail!("值超出 {} 位: {v}", elem_size * 8);
                }
                for j in 0..elem_size {
                    data.push(((n >> (8 * j)) & 0xff) as u8);
                }
            }
        } else {
            for v in arr(args, "bytes") {
                let n = parse_num(&v)?;
                if n > 0xff {
                    bail!("字节超出 0..=255: {v}");
                }
                data.push(n as u8);
            }
        }
    }
    let data_len = data.len();
    with_sim(server, args, true, |sim, _, _| {
        for (i, byte) in data.iter().enumerate() {
            write_mem_byte(sim, addr_num.wrapping_add(i as u32), *byte);
        }
        Ok(json!({ "address": addr_num, "len": data_len, "width": width }))
    })
}

fn tool_mem_fill(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let addr_num = parse_num(&s(args, "addr").ok_or_else(|| anyhow::anyhow!("缺少 addr"))?)?;
    let len = u(args, "len").ok_or_else(|| anyhow::anyhow!("缺少 len"))? as usize;
    let value = parse_num(&s(args, "value").ok_or_else(|| anyhow::anyhow!("缺少 value"))?)?;
    if value > 0xff {
        bail!("填充字节超出 0..=255: {value}");
    }
    let value_byte = value as u8;
    with_sim(server, args, true, |sim, _, _| {
        for i in 0..len {
            write_mem_byte(sim, addr_num.wrapping_add(i as u32), value_byte);
        }
        Ok(json!({ "address": addr_num, "len": len, "value": value_byte }))
    })
}

fn tool_mem_search(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let addr_num = parse_num(&s(args, "addr").ok_or_else(|| anyhow::anyhow!("缺少 addr"))?)?;
    let len = u(args, "len").unwrap_or(256) as usize;
    let pattern = arr(args, "pattern");
    let mut pat = Vec::new();
    for p in &pattern {
        let v = parse_num(p)?;
        if v > 0xff {
            bail!("模式字节超出 0..=255: {p}");
        }
        pat.push(v as u8);
    }
    if pat.is_empty() {
        bail!("pattern 至少需要一个字节");
    }
    with_sim(server, args, false, |sim, _, _| {
        let mut matches = Vec::new();
        if len >= pat.len() {
            for off in 0..=(len - pat.len()) {
                let mut ok = true;
                for (j, p) in pat.iter().enumerate() {
                    if read_mem_byte(sim, addr_num.wrapping_add((off + j) as u32)) != *p {
                        ok = false;
                        break;
                    }
                }
                if ok {
                    matches.push(addr_num.wrapping_add(off as u32));
                }
            }
        }
        Ok(json!({ "address": addr_num, "matches": matches }))
    })
}

fn tool_mem_load(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let firmware = s(args, "firmware").ok_or_else(|| anyhow::anyhow!("缺少 firmware"))?;
    let base = parse_num(s(args, "base").as_deref().unwrap_or("0"))?;
    with_sim(server, args, true, |sim, fw, _| {
        load_firmware_into(sim, Path::new(&firmware), base)?;
        Ok(full_state(sim, fw))
    })
}

fn tool_mem_dump(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let addr_num = parse_num(&s(args, "addr").ok_or_else(|| anyhow::anyhow!("缺少 addr"))?)?;
    let len = u(args, "len").unwrap_or(64) as usize;
    let format = s(args, "format").unwrap_or_else(|| "c".to_string());
    let name = s(args, "name").unwrap_or_else(|| "data".to_string());
    with_sim(server, args, false, |sim, _, _| {
        let mut bytes = Vec::with_capacity(len);
        for i in 0..len {
            bytes.push(read_mem_byte(sim, addr_num.wrapping_add(i as u32)));
        }
        let rendered = match format.as_str() {
            "c" => {
                let mut out = String::from("const uint8_t ");
                out.push_str(&name);
                out.push_str("[");
                out.push_str(&len.to_string());
                out.push_str("] = {");
                for (i, b) in bytes.iter().enumerate() {
                    if i % 16 == 0 {
                        out.push('\n');
                    }
                    out.push_str(&format!("0x{b:02x},"));
                }
                out.push_str("\n};\n");
                out
            }
            "hex" => crate::sim::encode_intel_hex(addr_num, &bytes),
            "dump" => {
                let mut out = String::new();
                for (i, chunk) in bytes.chunks(16).enumerate() {
                    let base = addr_num as usize + i * 16;
                    out.push_str(&format!("{base:08x}  "));
                    for b in chunk {
                        out.push_str(&format!("{b:02x} "));
                    }
                    out.push('\n');
                }
                out
            }
            other => bail!("不支持的内存导出格式: {other}（支持 c/hex/dump）"),
        };
        Ok(json!({
            "address": addr_num,
            "len": len,
            "format": format,
            "name": name,
            "bytes": bytes,
            "text": rendered,
        }))
    })
}

fn tool_bp_set(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let addr_arg = s(args, "addr");
    let symbol = s(args, "symbol");
    let line = s(args, "line");
    let condition = s(args, "condition").unwrap_or_default();
    let hit_count = u(args, "hit_count").unwrap_or(0);
    let once = b(args, "once").unwrap_or(false);
    with_sim(server, args, true, |sim, _, _| {
        let addr = if let Some(a) = &addr_arg {
            parse_num(a)?
        } else if let Some(sym) = &symbol {
            sim.symbols
                .iter()
                .find(|s| s.name == *sym)
                .ok_or_else(|| anyhow::anyhow!("未找到符号: {sym}"))?
                .address
        } else if let Some(l) = &line {
            let (file, lineno) = l
                .split_once(':')
                .ok_or_else(|| anyhow::anyhow!("line 格式应为 file:line: {l}"))?;
            let file_idx = sim
                .source_files
                .iter()
                .position(|f| f == file)
                .ok_or_else(|| anyhow::anyhow!("未知源文件: {file}"))?;
            let lineno = lineno
                .parse::<u32>()
                .map_err(|_| anyhow::anyhow!("无效行号: {lineno}"))?;
            sim.address_for_line(file_idx, lineno)
                .ok_or_else(|| anyhow::anyhow!("文件 {file} 无行号 {lineno}"))?
        } else {
            bail!("请提供 addr、symbol 或 line");
        };
        if addr > sim.pc_limit() {
            bail!("断点地址 0x{addr:08x} 超出 {} 的 PC 可寻址范围", sim.arch.label());
        }
        let set = if once {
            sim.set_breakpoint_ex_once(addr, condition.clone(), hit_count)
        } else {
            sim.set_breakpoint_ex(addr, condition.clone(), hit_count)
        };
        Ok(json!({
            "address": addr,
            "condition": condition,
            "hitCount": hit_count,
            "once": once,
            "set": set,
        }))
    })
}

fn tool_bp_clear(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let addr = parse_num(&s(args, "addr").ok_or_else(|| anyhow::anyhow!("缺少 addr"))?)?;
    with_sim(server, args, true, |sim, _, _| {
        let removed = sim.clear_breakpoint(addr);
        Ok(json!({ "address": addr, "removed": removed }))
    })
}

fn tool_bp_list(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    with_sim(server, args, false, |sim, _, _| {
        let items: Vec<_> = sim
            .breakpoints
            .iter()
            .map(|bp| {
                json!({
                    "address": bp.address,
                    "condition": bp.condition,
                    "hitCount": bp.hit_count,
                    "once": bp.once,
                    "enabled": bp.enabled,
                    "hits": bp.hits(),
                })
            })
            .collect();
        Ok(json!({ "breakpoints": items }))
    })
}

fn tool_bp_enable(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let addr = parse_num(&s(args, "addr").ok_or_else(|| anyhow::anyhow!("缺少 addr"))?)?;
    with_sim(server, args, true, |sim, _, _| {
        let ok = sim.set_breakpoint_enabled(addr, true);
        Ok(json!({ "address": addr, "enabled": ok }))
    })
}

fn tool_bp_disable(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let addr = parse_num(&s(args, "addr").ok_or_else(|| anyhow::anyhow!("缺少 addr"))?)?;
    with_sim(server, args, true, |sim, _, _| {
        let ok = sim.set_breakpoint_enabled(addr, false);
        Ok(json!({ "address": addr, "enabled": ok }))
    })
}

fn tool_bp_clear_all(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    with_sim(server, args, true, |sim, _, _| {
        let count = sim.breakpoints.len();
        sim.breakpoints.clear();
        Ok(json!({ "cleared": count }))
    })
}

fn tool_disasm(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let arch = {
        let arch_arg = s(args, "arch");
        let (sim, _, _) = crate::load_or_create(
            server.resolve_session(args).as_deref(),
            arch_arg.as_deref(),
            None,
            "0",
        )?;
        sim.arch
    };
    let mut pc = match s(args, "addr") {
        Some(a) => parse_num(&a)?,
        None => {
            let session = server.resolve_session(args);
            if let Some(sp) = &session {
                if sp.exists() {
                    Session::load(sp)?.pc
                } else {
                    default_mem_base(arch)
                }
            } else {
                default_mem_base(arch)
            }
        }
    };
    let sim = if let Some(sp) = server.resolve_session(args) {
        if sp.exists() {
            Session::load(&sp)?
                .apply(None)
                .unwrap_or_else(|_| Simulator::for_arch(arch).unwrap())
        } else {
            Simulator::for_arch(arch)?
        }
    } else {
        Simulator::for_arch(arch)?
    };
    let count = u(args, "count").unwrap_or(8) as usize;
    let raw = b(args, "raw").unwrap_or(false);
    let source = b(args, "source").unwrap_or(false);
    let arm_state = match s(args, "arm_state").as_deref() {
        Some("a32") => true,
        Some("thumb") => false,
        _ => sim.arm_state(),
    };
    let read = |addr: u32| -> u8 { read_code_byte(&sim, addr) };
    let mut items = Vec::new();
    for _ in 0..count {
        let width = ns_sim::instruction_size(arch, pc, &read, arm_state);
        let text = ns_sim::disassemble_one_state(arch, pc, &read, arm_state);
        let bytes: Vec<u8> = (0..width)
            .map(|i| read_code_byte(&sim, pc.wrapping_add(i as u32)))
            .collect();
        let src = if source {
            crate::source_annotation(&sim, pc).trim().to_string()
        } else {
            String::new()
        };
        if raw {
            items.push(json!({
                "address": pc,
                "text": text,
                "bytes": bytes,
                "source": src,
                "armState": arm_state,
                "raw": bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "),
            }));
        } else {
            items.push(json!({
                "address": pc,
                "text": text,
                "bytes": bytes,
                "source": src,
                "armState": arm_state,
            }));
        }
        pc = pc.wrapping_add(width);
    }
    Ok(json!({ "arch": arch_name(arch), "instructions": items }))
}

fn tool_reset(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let entry = s(args, "entry");
    with_sim(server, args, true, |sim, fw, _| {
        let entry_num = match entry {
            Some(e) => parse_num(&e)?,
            None => {
                if let Some(fw) = fw {
                    let mut fresh = Simulator::for_arch(sim.arch)?;
                    load_firmware_into(&mut fresh, Path::new(&fw.path), fw.base)?;
                    fresh.program_counter()
                } else {
                    sim.entry()
                }
            }
        };
        sim.reset(entry_num);
        Ok(json!({ "pc": sim.program_counter(), "cycleCount": sim.cycle_count, "halted": sim.halted }))
    })
}

fn tool_cycle_get(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    with_sim(server, args, false, |sim, _, _| {
        Ok(json!({ "cycleCount": sim.cycle_count }))
    })
}

fn tool_cycle_estimate(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let addr = parse_num(&s(args, "addr").ok_or_else(|| anyhow::anyhow!("缺少 addr"))?)?;
    with_sim(server, args, false, |sim, _, _| {
        Ok(json!({ "address": addr, "cycles": sim.cycles_for_instruction(addr) }))
    })
}

fn tool_cycle_tick(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let cycles = u64(args, "cycles").unwrap_or(1);
    with_sim(server, args, true, |sim, _, _| {
        sim.tick_peripherals_for(cycles);
        Ok(json!({ "cycleCount": sim.cycle_count, "ticked": cycles }))
    })
}

fn tool_interrupt_take(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    with_sim(server, args, true, |sim, _, _| {
        let taken = sim.take_interrupt();
        Ok(json!({ "taken": taken, "pc": sim.program_counter() }))
    })
}

fn tool_interrupt_pending(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    with_sim(server, args, false, |sim, _, _| {
        let pending: Vec<usize> = sim
            .peripherals
            .irq_pending
            .iter()
            .enumerate()
            .filter(|(_, p)| **p)
            .map(|(i, _)| i)
            .collect();
        Ok(json!({
            "pending": pending,
            "anyPending": sim.any_pending(),
            "mie": sim.csr.mie,
            "mstatus": sim.csr.mstatus,
        }))
    })
}

fn tool_interrupt_set(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let line = u(args, "line").ok_or_else(|| anyhow::anyhow!("缺少 line"))? as usize;
    let value = u(args, "value").unwrap_or(1);
    if line >= 16 {
        bail!("中断线必须在 0..16");
    }
    if value > 1 {
        bail!("中断线值必须是 0 或 1");
    }
    with_sim(server, args, true, |sim, _, _| {
        sim.peripherals.irq_pending[line] = value == 1;
        Ok(json!({ "line": line, "value": value, "pending": sim.peripherals.irq_pending[line] }))
    })
}

fn tool_periph_status(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    with_sim(server, args, false, |sim, fw, _| {
        let state = full_state(sim, fw);
        Ok(state.get("peripherals").cloned().unwrap_or(Value::Null))
    })
}

fn tool_periph_config(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let uart_base = s(args, "uart_base").map(|x| parse_num(&x)).transpose()?;
    let gpio_base = s(args, "gpio_base").map(|x| parse_num(&x)).transpose()?;
    if uart_base.is_none() && gpio_base.is_none() {
        bail!("至少提供 uart_base 或 gpio_base");
    }
    with_sim(server, args, true, |sim, _, _| {
        if let Some(v) = uart_base {
            sim.peripherals.uart_base = v;
        }
        if let Some(v) = gpio_base {
            sim.peripherals.gpio_base = v;
        }
        Ok(json!({
            "uartBase": sim.peripherals.uart_base,
            "gpioBase": sim.peripherals.gpio_base,
        }))
    })
}

fn tool_periph_uart(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let clear = b(args, "clear").unwrap_or(false);
    with_sim(server, args, true, |sim, _, _| {
        let text = String::from_utf8_lossy(&sim.peripherals.outputs).into_owned();
        let len = sim.peripherals.outputs.len();
        if clear {
            sim.peripherals.outputs.clear();
        }
        Ok(json!({ "len": len, "text": text, "cleared": clear }))
    })
}

fn tool_periph_gpio(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let pin = u(args, "pin").ok_or_else(|| anyhow::anyhow!("缺少 pin"))?;
    let value = u(args, "value").ok_or_else(|| anyhow::anyhow!("缺少 value"))?;
    if pin > 31 {
        bail!("引脚号必须在 0..=31");
    }
    if value > 1 {
        bail!("引脚值必须是 0 或 1");
    }
    with_sim(server, args, true, |sim, _, _| {
        if value == 1 {
            sim.peripherals.gpio_in |= 1 << pin;
        } else {
            sim.peripherals.gpio_in &= !(1 << pin);
        }
        Ok(json!({ "pin": pin, "value": value, "gpioIn": sim.peripherals.gpio_in }))
    })
}

fn tool_periph_timer(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let index = u(args, "index").unwrap_or(0) as usize;
    if index >= 8 {
        bail!("定时器索引必须在 0..8");
    }
    let enabled = b(args, "enabled");
    let prescaler = u(args, "prescaler");
    let auto_reload = u(args, "auto_reload");
    with_sim(server, args, true, |sim, _, _| {
        while sim.peripherals.timers.len() <= index {
            sim.peripherals
                .timers
                .push(ns_sim::Timer::new(0x4000_1000, sim.peripherals.timers.len()));
        }
        {
            let t = &mut sim.peripherals.timers[index];
            if let Some(v) = enabled {
                t.enabled = v;
            }
            if let Some(v) = prescaler {
                t.prescaler = v;
            }
            if let Some(v) = auto_reload {
                t.auto_reload = v;
            }
        }
        let t = &sim.peripherals.timers[index];
        Ok(json!({
            "index": index,
            "base": t.base,
            "enabled": t.enabled,
            "counter": t.counter,
            "autoReload": t.auto_reload,
            "prescaler": t.prescaler,
            "irqLine": t.irq_line,
            "updateCount": t.update_count,
        }))
    })
}

fn tool_periph_reg_read(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let addr_num = crate::resolve_reg_address(
        s(args, "addr").as_deref(),
        s(args, "name").as_deref(),
        s(args, "device").as_deref(),
    )?;
    let len = u(args, "len").unwrap_or(4) as usize;
    with_sim(server, args, false, |sim, _, _| {
        let mut bytes = Vec::with_capacity(len);
        if let Some(value) = sim.peripheral_read_reg(addr_num) {
            let value_bytes = value.to_le_bytes();
            for i in 0..len {
                bytes.push(if i < 4 { value_bytes[i] } else { 0 });
            }
        } else {
            for i in 0..len {
                bytes.push(read_mem_byte(sim, addr_num.wrapping_add(i as u32)));
            }
        }
        Ok(json!({ "address": addr_num, "bytes": bytes }))
    })
}

fn tool_periph_reg_write(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let addr_num = crate::resolve_reg_address(
        s(args, "addr").as_deref(),
        s(args, "name").as_deref(),
        s(args, "device").as_deref(),
    )?;
    let mut data = Vec::new();
    for v in arr(args, "bytes") {
        let n = parse_num(&v)?;
        if n > 0xff {
            bail!("字节超出 0..=255: {v}");
        }
        data.push(n as u8);
    }
    let data_len = data.len();
    with_sim(server, args, true, |sim, _, _| {
        for (i, byte) in data.iter().enumerate() {
            sim.peripheral_write_reg(addr_num.wrapping_add(i as u32), *byte);
        }
        Ok(json!({ "address": addr_num, "len": data_len }))
    })
}

fn tool_periph_reg_list(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let device = s(args, "device");
    with_sim(server, args, false, |sim, _, _| {
        let mut names: std::collections::BTreeMap<u32, String> = std::collections::BTreeMap::new();
        if let Some(key) = &device {
            let dev = ns_device::device_for_target(key)
                .ok_or_else(|| anyhow::anyhow!("未知设备或缺少内置 SVD: {key}"))?;
            for p in &dev.sorted_peripherals() {
                for r in &p.registers {
                    names.insert(r.address as u32, format!("{}.{}", p.name, r.name));
                }
            }
        }
        let mut entries: Vec<(u32, u32)> = sim
            .peripherals
            .external_regs
            .iter()
            .map(|(a, v)| (*a, *v))
            .collect();
        entries.sort_by_key(|(a, _)| *a);
        let items: Vec<_> = entries
            .iter()
            .map(|(a, v)| {
                json!({
                    "address": a,
                    "value": v,
                    "name": names.get(a).cloned().unwrap_or_default(),
                })
            })
            .collect();
        Ok(json!({ "registers": items }))
    })
}

fn tool_periph_field_get(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let device = s(args, "device").ok_or_else(|| anyhow::anyhow!("缺少 device"))?;
    let peripheral = s(args, "peripheral").ok_or_else(|| anyhow::anyhow!("缺少 peripheral"))?;
    let register = s(args, "register").ok_or_else(|| anyhow::anyhow!("缺少 register"))?;
    let field = s(args, "field").ok_or_else(|| anyhow::anyhow!("缺少 field"))?;
    let (addr, bit_offset, bit_width) =
        crate::resolve_reg_field(&device, &peripheral, &register, &field)?;
    with_sim(server, args, false, |sim, _, _| {
        let value = sim.peripheral_read_reg(addr).unwrap_or(0);
        let field_value = (value >> bit_offset) & ((1u32 << bit_width) - 1);
        Ok(json!({
            "address": addr,
            "bitOffset": bit_offset,
            "bitWidth": bit_width,
            "fieldValue": field_value,
            "registerValue": value,
        }))
    })
}

fn tool_periph_field_set(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let device = s(args, "device").ok_or_else(|| anyhow::anyhow!("缺少 device"))?;
    let peripheral = s(args, "peripheral").ok_or_else(|| anyhow::anyhow!("缺少 peripheral"))?;
    let register = s(args, "register").ok_or_else(|| anyhow::anyhow!("缺少 register"))?;
    let field = s(args, "field").ok_or_else(|| anyhow::anyhow!("缺少 field"))?;
    let value = s(args, "value").ok_or_else(|| anyhow::anyhow!("缺少 value"))?;
    let (addr, bit_offset, bit_width) =
        crate::resolve_reg_field(&device, &peripheral, &register, &field)?;
    let field_value = parse_num(&value)?;
    let mask = (1u32 << bit_width) - 1;
    if field_value > mask {
        bail!("字段值超出 {} 位: {value}", bit_width);
    }
    with_sim(server, args, true, |sim, _, _| {
        let current = sim.peripheral_read_reg(addr).unwrap_or(0);
        let new_value = (current & !(mask << bit_offset)) | ((field_value & mask) << bit_offset);
        let bytes = new_value.to_le_bytes();
        for (i, byte) in bytes.iter().enumerate() {
            sim.peripheral_write_reg(addr.wrapping_add(i as u32), *byte);
        }
        Ok(json!({
            "address": addr,
            "bitOffset": bit_offset,
            "bitWidth": bit_width,
            "fieldValue": field_value,
            "registerValue": new_value,
        }))
    })
}

fn tool_periph_svd_load(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let file = s(args, "file");
    let device = s(args, "device");
    let peripheral = s(args, "peripheral");
    let force = b(args, "force").unwrap_or(false);
    let (dev, source) = match (file, device) {
        (Some(f), None) => (ns_device::load_svd_file(Path::new(&f))?, f),
        (None, Some(d)) => (
            ns_device::device_for_target(&d)
                .ok_or_else(|| anyhow::anyhow!("未知设备或缺少内置 SVD: {d}"))?,
            d,
        ),
        (Some(_), Some(_)) => bail!("file 与 device 只能二选一"),
        (None, None) => bail!("请提供 file 或 device"),
    };
    let periphs: Vec<&ns_device::Peripheral> = match &peripheral {
        Some(name) => vec![dev
            .peripheral(name)
            .ok_or_else(|| anyhow::anyhow!("SVD 中无外设: {name}"))?],
        None => dev.sorted_peripherals(),
    };
    with_sim(server, args, true, |sim, _, _| {
        let mut loaded = 0usize;
        for p in &periphs {
            for r in &p.registers {
                let addr = r.address as u32;
                if force || !sim.peripherals.external_regs.contains_key(&addr) {
                    let v = r.reset_value as u32;
                    sim.peripherals.external_regs.insert(addr, v);
                    let bytes = v.to_le_bytes();
                    for (i, byte) in bytes.iter().enumerate() {
                        write_mem_byte(sim, addr.wrapping_add(i as u32), *byte);
                    }
                    loaded += 1;
                }
            }
        }
        Ok(json!({
            "source": source,
            "loaded": loaded,
            "peripherals": periphs.len(),
            "force": force,
        }))
    })
}

fn tool_symbol_list(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let filter = s(args, "filter").map(|f| f.to_ascii_lowercase());
    with_sim(server, args, false, |sim, _, _| {
        let items: Vec<_> = sim
            .symbols
            .iter()
            .filter(|sym| {
                filter
                    .as_ref()
                    .map(|f| sym.name.to_ascii_lowercase().contains(f))
                    .unwrap_or(true)
            })
            .map(|sym| json!({ "name": sym.name, "address": sym.address, "size": sym.size }))
            .collect();
        Ok(json!({ "symbols": items }))
    })
}

fn tool_symbol_at(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let addr = parse_num(&s(args, "addr").ok_or_else(|| anyhow::anyhow!("缺少 addr"))?)?;
    with_sim(server, args, false, |sim, _, _| {
        Ok(sim
            .symbol_at(addr)
            .map(|sym| json!({ "name": sym.name, "address": sym.address, "size": sym.size }))
            .unwrap_or(Value::Null))
    })
}

fn tool_symbol_line(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let addr = parse_num(&s(args, "addr").ok_or_else(|| anyhow::anyhow!("缺少 addr"))?)?;
    with_sim(server, args, false, |sim, _, _| {
        Ok(sim
            .line_for_address(addr)
            .and_then(|(file, line)| sim.source_files.get(file).map(|f| (f.clone(), line)))
            .map(|(file, line)| json!({ "file": file, "line": line }))
            .unwrap_or(Value::Null))
    })
}

fn tool_symbol_addr(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let file = u(args, "file").ok_or_else(|| anyhow::anyhow!("缺少 file"))? as usize;
    let line = u(args, "line").ok_or_else(|| anyhow::anyhow!("缺少 line"))?;
    with_sim(server, args, false, |sim, _, _| {
        Ok(sim
            .address_for_line(file, line)
            .map(|addr| json!({ "address": addr }))
            .unwrap_or(Value::Null))
    })
}

fn tool_watchdog_status(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    with_sim(server, args, false, |sim, _, _| {
        Ok(sim
            .peripherals
            .watchdog
            .as_ref()
            .map(|w| {
                json!({
                    "base": w.base,
                    "enabled": w.enabled,
                    "timeoutCycles": w.timeout_cycles,
                    "elapsed": w.elapsed,
                    "resetCount": w.reset_count,
                    "lastResetPc": w.last_reset_pc,
                })
            })
            .unwrap_or(Value::Null))
    })
}

fn tool_watchdog_feed(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    with_sim(server, args, true, |sim, _, _| {
        let fed = if let Some(w) = sim.peripherals.watchdog.as_mut() {
            w.elapsed = 0;
            true
        } else {
            false
        };
        Ok(json!({ "fed": fed }))
    })
}

fn tool_watchdog_config(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let enabled = b(args, "enabled");
    let timeout = u64(args, "timeout");
    let base = s(args, "base").map(|x| parse_num(&x)).transpose()?;
    with_sim(server, args, true, |sim, _, _| {
        let w = sim
            .peripherals
            .watchdog
            .get_or_insert_with(|| ns_sim::Watchdog::new(base.unwrap_or(0x4000_0000)));
        if let Some(v) = enabled {
            w.enabled = v;
        }
        if let Some(v) = timeout {
            w.timeout_cycles = v;
        }
        if let Some(v) = base {
            w.base = v;
        }
        Ok(json!({
            "base": w.base,
            "enabled": w.enabled,
            "timeoutCycles": w.timeout_cycles,
            "elapsed": w.elapsed,
            "resetCount": w.reset_count,
            "lastResetPc": w.last_reset_pc,
        }))
    })
}

fn tool_watchdog_reset(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let entry = s(args, "entry");
    with_sim(server, args, true, |sim, fw, _| {
        let reset_pc = sim.program_counter();
        let entry_num = match entry {
            Some(e) => parse_num(&e)?,
            None => {
                if let Some(fw) = fw {
                    let mut fresh = Simulator::for_arch(sim.arch)?;
                    load_firmware_into(&mut fresh, Path::new(&fw.path), fw.base)?;
                    fresh.program_counter()
                } else {
                    sim.entry()
                }
            }
        };
        if let Some(w) = sim.peripherals.watchdog.as_mut() {
            w.reset_count += 1;
            w.last_reset_pc = reset_pc;
            w.elapsed = 0;
        }
        sim.reset(entry_num);
        Ok(json!({
            "reset": true,
            "pc": sim.program_counter(),
            "lastResetPc": reset_pc,
            "cycleCount": sim.cycle_count,
        }))
    })
}

fn tool_csr_list(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    with_sim(server, args, false, |sim, _, _| {
        Ok(json!({
            "mstatus": sim.csr.mstatus,
            "mtvec": sim.csr.mtvec,
            "mepc": sim.csr.mepc,
            "mcause": sim.csr.mcause,
            "mie": sim.csr.mie,
            "mhartid": sim.csr.mhartid,
        }))
    })
}

fn tool_csr_get(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let name = s(args, "name").ok_or_else(|| anyhow::anyhow!("缺少 name"))?;
    with_sim(server, args, false, |sim, _, _| {
        let value = csr_value(sim, &name)?;
        Ok(json!({ "name": name, "value": value }))
    })
}

fn tool_csr_set(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    let name = s(args, "name").ok_or_else(|| anyhow::anyhow!("缺少 name"))?;
    let value = parse_num(&s(args, "value").ok_or_else(|| anyhow::anyhow!("缺少 value"))?)?;
    with_sim(server, args, true, |sim, _, _| {
        set_csr_value(sim, &name, value)?;
        Ok(json!({ "name": name, "value": value }))
    })
}

fn csr_value(sim: &Simulator, name: &str) -> Result<u32> {
    match name {
        "mstatus" => Ok(sim.csr.mstatus),
        "mtvec" => Ok(sim.csr.mtvec),
        "mepc" => Ok(sim.csr.mepc),
        "mcause" => Ok(sim.csr.mcause),
        "mie" => Ok(sim.csr.mie),
        "mhartid" => Ok(sim.csr.mhartid),
        _ => bail!("未知 CSR: {name}（支持 mstatus/mtvec/mepc/mcause/mie/mhartid）"),
    }
}

fn set_csr_value(sim: &mut Simulator, name: &str, value: u32) -> Result<()> {
    match name {
        "mstatus" => sim.csr.mstatus = value,
        "mtvec" => sim.csr.mtvec = value,
        "mepc" => sim.csr.mepc = value,
        "mcause" => sim.csr.mcause = value,
        "mie" => sim.csr.mie = value,
        "mhartid" => sim.csr.mhartid = value,
        _ => bail!("未知 CSR: {name}（支持 mstatus/mtvec/mepc/mcause/mie/mhartid）"),
    }
    Ok(())
}

/// The requested analysis output mode for the `trace`/`coverage`/`profile` tools.
enum AnalyzeTool {
    Trace,
    Coverage,
    Profile,
}

fn analyze_tool(server: &McpServer, args: &Map<String, Value>, kind: AnalyzeTool) -> Result<Value> {
    let until = s(args, "until").map(|x| parse_num(&x)).transpose()?;
    let steps = u64(args, "steps");
    let max_steps = u64(args, "max_steps").unwrap_or(100_000);
    let collect_trace = matches!(kind, AnalyzeTool::Trace);
    with_sim(server, args, true, |sim, _, _| {
        if let Some(addr) = until {
            if addr > sim.pc_limit() {
                bail!("until 地址 0x{addr:08x} 超出 {} 的 PC 可寻址范围", sim.arch.label());
            }
        }
        let temp_bp = until.map(|a| sim.set_breakpoint(a)).unwrap_or(false);
        let analysis = crate::sim::analyze_run(sim, steps.unwrap_or(max_steps), collect_trace);
        if temp_bp {
            if let Some(a) = until {
                sim.clear_breakpoint(a);
            }
        }
        let arch_name = arch_name(sim.arch);
        Ok(match kind {
            AnalyzeTool::Trace => json!({
                "arch": arch_name,
                "steps": analysis.steps,
                "cycleCount": analysis.cycle_count,
                "result": analysis.result,
                "trace": analysis.trace,
            }),
            AnalyzeTool::Coverage => {
                let items: Vec<Value> = analysis
                    .coverage
                    .iter()
                    .map(|(pc, hits)| json!({ "pc": pc, "hits": hits }))
                    .collect();
                let unique = items.len();
                json!({
                    "arch": arch_name,
                    "steps": analysis.steps,
                    "cycleCount": analysis.cycle_count,
                    "result": analysis.result,
                    "uniquePcs": unique,
                    "coverage": items,
                })
            }
            AnalyzeTool::Profile => {
                let items: Vec<Value> = analysis
                    .histogram
                    .iter()
                    .map(|(mnemonic, count)| json!({ "mnemonic": mnemonic, "count": count }))
                    .collect();
                let unique = items.len();
                json!({
                    "arch": arch_name,
                    "steps": analysis.steps,
                    "cycleCount": analysis.cycle_count,
                    "result": analysis.result,
                    "uniqueMnemonics": unique,
                    "histogram": items,
                })
            }
        })
    })
}

fn tool_trace(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    analyze_tool(server, args, AnalyzeTool::Trace)
}

fn tool_coverage(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    analyze_tool(server, args, AnalyzeTool::Coverage)
}

fn tool_profile(server: &McpServer, args: &Map<String, Value>) -> Result<Value> {
    analyze_tool(server, args, AnalyzeTool::Profile)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn server() -> McpServer {
        McpServer { default_session: None }
    }

    #[test]
    fn read_message_parses_content_length_frame() {
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let input = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        let mut cursor = Cursor::new(input.as_bytes());
        let msg = read_message(&mut cursor).unwrap().unwrap();
        assert_eq!(msg, body);
        // EOF after the message.
        assert!(read_message(&mut cursor).unwrap().is_none());
    }

    #[test]
    fn read_message_requires_content_length() {
        let input = "X-Header: 1\r\n\r\n{}";
        let mut cursor = Cursor::new(input.as_bytes());
        let err = read_message(&mut cursor).unwrap_err();
        assert!(err.to_string().contains("Content-Length"));
    }

    #[test]
    fn read_message_eof_returns_none() {
        let mut cursor = Cursor::new(b"".as_slice());
        assert!(read_message(&mut cursor).unwrap().is_none());
    }

    #[test]
    fn write_message_frames_with_content_length() {
        let mut out = Vec::new();
        write_message(&mut out, "{\"a\":1}").unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("Content-Length: 7\r\n\r\n"));
        assert!(text.ends_with("{\"a\":1}"));
    }

    #[test]
    fn handle_ping_returns_null() {
        assert_eq!(server().handle("ping", &Value::Null).unwrap(), Value::Null);
    }

    #[test]
    fn handle_initialize_advertises_tools() {
        let result = server()
            .handle("initialize", &json!({ "protocolVersion": "2024-11-05" }))
            .unwrap();
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert_eq!(result["serverInfo"]["name"], "ns-instruction-sim");
        assert_eq!(result["capabilities"]["tools"], json!({}));
    }

    #[test]
    fn handle_unknown_method_errors() {
        let err = server().handle("bogus", &Value::Null).unwrap_err();
        assert!(err.to_string().contains("未知方法"));
    }

    #[test]
    fn handle_tools_list_lists_all_tools_with_schemas() {
        let result = server().handle("tools/list", &Value::Null).unwrap();
        let tools = result["tools"].as_array().unwrap();
        assert!(tools.len() >= 60, "expected >= 60 tools, got {}", tools.len());
        for t in tools {
            assert!(t["name"].as_str().is_some());
            assert!(t["description"].as_str().is_some());
            assert!(t["inputSchema"]["type"] == "object");
        }
    }

    #[test]
    fn handle_tools_call_unknown_tool_errors() {
        let err = server()
            .handle("tools/call", &json!({ "name": "nope", "arguments": {} }))
            .unwrap_err();
        assert!(err.to_string().contains("未知工具"));
    }

    #[test]
    fn handle_tools_call_missing_name_errors() {
        let err = server().handle("tools/call", &json!({ "arguments": {} })).unwrap_err();
        assert!(err.to_string().contains("缺少 name"));
    }

    #[test]
    fn handle_tools_call_success_returns_structured_content() {
        let result = server()
            .handle("tools/call", &json!({ "name": "arch_list", "arguments": {} }))
            .unwrap();
        assert_eq!(result["isError"], false);
        let archs = result["structuredContent"]["architectures"].as_array().unwrap();
        assert!(archs.len() >= 12);
        assert_eq!(result["content"][0]["type"], "text");
    }

    #[test]
    fn handle_tools_call_tool_error_returns_is_error() {
        // `reg_set` with no `name` must surface isError=true (not a JSON-RPC error).
        let result = server()
            .handle("tools/call", &json!({ "name": "reg_set", "arguments": {} }))
            .unwrap();
        assert_eq!(result["isError"], true);
        assert!(result["content"][0]["text"].as_str().unwrap().contains("错误"));
    }

    #[test]
    fn arg_helpers_extract_typed_values() {
        let args = json!({
            "name": "x1",
            "count": 4,
            "enabled": true,
            "bytes": ["0x01", 2],
        });
        let map = args.as_object().unwrap();
        assert_eq!(s(map, "name").as_deref(), Some("x1"));
        assert_eq!(u(map, "count"), Some(4));
        assert_eq!(u64(map, "count"), Some(4));
        assert_eq!(b(map, "enabled"), Some(true));
        assert_eq!(arr(map, "bytes"), vec!["0x01".to_string(), "2".to_string()]);
    }
}
