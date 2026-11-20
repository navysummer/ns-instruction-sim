//! ns-instruction-sim — a command-line instruction-level simulator.
//!
//! A thin, scriptable front-end over the `ns-sim` instruction-level cores
//! used by ns-embed-ide. External tools (CI, scripts, IDEs) can drive every
//! supported architecture (RISC-V, ARM Cortex-M, MCS-51, MCS-251, C166,
//! Xtensa, AVR, MSP430, V850, STM8, RX, RL78) through subcommands:
//!
//! ```text
//! ns-instruction-sim arch list
//! ns-instruction-sim run --firmware app.elf
//! ns-instruction-sim step --session sim.json
//! ns-instruction-sim reg set --session sim.json --name x1 --value 0x10
//! ns-instruction-sim mem dump --session sim.json --addr 0x20000000 --len 64
//! ns-instruction-sim bp set --session sim.json --addr 0x80000000
//! ns-instruction-sim disasm --arch riscv --addr 0x80000000 --count 8
//! ```

mod mcp;
mod session;
mod sim;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};

use ns_sim::{Arch, Simulator, StepResult};
use session::{FirmwareState, Session};
use sim::{FirmwareFormat, analyze_run, default_mem_base, detect_format, encode_intel_hex, load_firmware_into, parse_num, read_code_byte, read_mem_byte, read_mem_u32, write_mem_byte};

#[derive(Parser)]
#[command(
    name = "ns-instruction-sim",
    version,
    about = "跨平台嵌入式指令级仿真命令行工具",
    long_about = "跨平台嵌入式指令级仿真命令行工具。支持 RISC-V、ARM Cortex-M、MCS-51、MCS-251、C166、Xtensa、AVR、MSP430、V850、STM8、RX、RL78 全部架构的固件加载、单步/连续运行、断点、寄存器/内存读写、反汇编与周期计数，所有能力均可被外部软件通过命令行调用。",
    version,
    disable_version_flag = false
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 显示版本号。
    Version {
        /// 以 JSON 输出名称与版本号。
        #[arg(long)]
        json: bool,
    },
    /// 列出支持的架构。
    Arch {
        #[command(subcommand)]
        sub: ArchSub,
    },
    /// 列出支持的设备（芯片）目录。
    Device {
        #[command(subcommand)]
        sub: DeviceSub,
    },
    /// 加载固件并运行（一条命令完成 加载→运行→输出状态）。
    Run(RunArgs),
    /// 单步执行一条指令。
    Step(StepArgs),
    /// 步过（step over）。
    StepOver(StepArgs),
    /// 步出（step out）。
    StepOut(StepArgs),
    /// 连续运行直到断点 / 停止 / 步数上限。
    Continue(ContinueArgs),
    /// 读取/修改寄存器。
    Reg(RegArgs),
    /// 读取/修改内存。
    Mem(MemArgs),
    /// 管理断点。
    Bp(BpArgs),
    /// 管理内存观察点（监视地址范围被写入时暂停）。
    Watch(WatchArgs),
    /// 反汇编指令。
    Disasm(DisasmArgs),
    /// 重置仿真器。
    Reset(ResetArgs),
    /// 显示当前状态（JSON 或 人类可读）。
    State(StateArgs),
    /// 查询/控制虚拟外设。
    Periph(PeriphArgs),
    /// 查询符号/行号映射（来自 ELF 符号表）。
    Symbol(SymbolArgs),
    /// 查询/推进周期与指令周期估算。
    Cycle(CycleArgs),
    /// 触发/查询中断。
    Interrupt(InterruptArgs),
    /// 查询/控制虚拟看门狗。
    Watchdog(WatchdogArgs),
    /// 读写 RISC-V 控制状态寄存器（CSR）。
    Csr(CsrArgs),
    /// 逐步追踪执行并输出结构化指令轨迹（每条指令的 PC/反汇编/字节/来源/周期）。
    Trace(AnalyzeArgs),
    /// 运行并统计执行覆盖（哪些 PC 被执行、各执行多少次）。
    Coverage(AnalyzeArgs),
    /// 运行并统计指令助记符直方图。
    Profile(AnalyzeArgs),
    /// 查询持久化会话的元信息或比较两个会话的差异。
    Session(SessionArgs),
    /// 查询固件元信息（格式/架构/入口/符号表/首条指令）。
    Firmware {
        #[command(subcommand)]
        sub: FirmwareSub,
    },
    /// 启动 MCP（Model Context Protocol）stdio 服务器，供大模型通过工具调用操作仿真器。
    Mcp {
        /// 默认会话文件路径（JSON）；MCP 工具未显式传入 `session` 时使用。
        #[arg(short, long)]
        session: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum ArchSub {
    /// 列出所有支持的架构与设备映射。
    List {
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum DeviceSub {
    /// 列出内置设备目录。
    List {
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 显示设备信息（内存映射、核心）。
    Info {
        device: String,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 生成 GCC 风格链接脚本（stdout 或 --out）。
    Linker {
        /// 目标设备键（如 stm32f103c8）。
        device: String,
        /// 写入文件路径（缺省打印到 stdout）。
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 生成最小启动文件（向量表 + 复位处理器，stdout 或 --out）。
    Startup {
        /// 目标设备键（如 stm32f103c8）。
        device: String,
        /// 写入文件路径（缺省打印到 stdout）。
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 生成外设初始化 C 骨架（时钟使能 + 复位值注释，可指定寄存器值）。
    Init {
        /// 目标设备键（如 stm32f103c8）。
        device: String,
        /// 要初始化的外设（可重复；缺省为设备全部外设）。
        #[arg(short, long)]
        peripheral: Vec<String>,
        /// 具体寄存器写入 PERIPH.REG=VALUE（可重复；十六进制或十进制）。
        #[arg(long)]
        register: Vec<String>,
        /// 写入文件路径（缺省打印到 stdout）。
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 解析并显示一个 CMSIS-SVD 设备描述文件。
    Svd {
        /// SVD 文件路径（与 --device 二选一）。
        #[arg(short, long)]
        file: Option<PathBuf>,
        /// 内置设备键（如 stm32f103c8；与 --file 二选一）。
        #[arg(long)]
        device: Option<String>,
        /// 只看指定外设（按名称，可选）。
        #[arg(long)]
        peripheral: Option<String>,
        /// 只看指定寄存器（按名称，需配合 --peripheral）。
        #[arg(long)]
        register: Option<String>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 列出设备内存映射（FLASH / RAM / PERIPHERALS 等区域）。
    MemoryMap {
        /// 目标设备键（如 stm32f103c8）。
        device: String,
        /// 查询地址落在哪个区域（可选；十六进制或十进制）。
        #[arg(short, long)]
        addr: Option<String>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
struct SessionArgs {
    #[command(subcommand)]
    sub: SessionSub,
}

#[derive(Subcommand)]
enum SessionSub {
    /// 显示会话元信息（架构/PC/周期/寄存器数/断点数/内存/外设/固件）。
    Info(SessionInfoArgs),
    /// 比较两个会话文件，列出全部差异（标量/寄存器/断点/内存）。
    Diff(SessionDiffArgs),
}

#[derive(Subcommand)]
enum FirmwareSub {
    /// 显示固件元信息（格式/大小/架构/入口/符号表/源文件/首条指令反汇编）。
    Info {
        /// 固件文件（ELF / Intel HEX / BIN）。
        #[arg(long)]
        firmware: PathBuf,
        /// 目标架构（非 ELF 固件需要；缺省 ELF 自动检测、其余默认 riscv）。
        #[arg(short, long)]
        arch: Option<String>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
struct RunArgs {
    /// 目标会话（存在则保留其外设/内存/断点并运行固件，缺失则新建）。
    #[arg(short, long)]
    session: Option<PathBuf>,
    /// 固件文件（ELF / Intel HEX / BIN）。
    #[arg(short, long)]
    firmware: PathBuf,
    /// 目标架构（可自动从 ELF 推导）。如 riscv/arm/mcs51/...
    #[arg(short, long)]
    arch: Option<String>,
    /// 原始 BIN/HEX 的加载基址（十六进制或十进制）。
    #[arg(long, default_value = "0")]
    base: String,
    /// 最大执行步数（默认 1,000,000）。
    #[arg(long, default_value = "1000000")]
    max_steps: u64,
    /// 只运行 N 条指令（优先级高于 --max-steps；到断点/停机则提前停止）。
    #[arg(long)]
    steps: Option<u64>,
    /// 运行到指定地址（临时断点，不持久化）。
    #[arg(long)]
    until: Option<String>,
    /// 运行到指定符号地址（临时断点，不持久化；与 --until 互斥）。
    #[arg(long, conflicts_with = "until")]
    until_symbol: Option<String>,
    /// 运行到寄存器/表达式条件成立（每步后求值，如 `x1 == 5`；到断点/停机/步数上限提前停止）。
    #[arg(long)]
    until_condition: Option<String>,
    /// 运行到周期计数达到该值即停止（含 cycle-accurate 模式的指令周期预估；`lastResult="cycles"`）。
    #[arg(long)]
    max_cycles: Option<u64>,
    /// 保存会话到指定文件（JSON）。
    #[arg(long)]
    save_session: Option<PathBuf>,
    /// 周期精确模式（按指令预估周期推进）。
    #[arg(long)]
    cycle_accurate: bool,
    /// 输出 JSON 状态（默认文本）。
    #[arg(long)]
    json: bool,
    /// 打印每条执行指令的 PC/反汇编/来源（逐步追踪）。
    #[arg(long)]
    trace: bool,
    /// 与 --trace 搭配：打印每条执行指令后发生变化的寄存器。
    #[arg(long)]
    trace_regs: bool,
    /// 在停止点追加 file:line 来源标注。
    #[arg(long)]
    source: bool,
}

#[derive(Args)]
struct StepArgs {
    /// 会话文件（不提供则新建临时会话并立即结束）。
    #[arg(short, long)]
    session: Option<PathBuf>,
    /// 目标架构（无会话时使用）。
    #[arg(short, long)]
    arch: Option<String>,
    /// 固件文件（无会话时使用）。
    #[arg(long)]
    firmware: Option<PathBuf>,
    /// 固件加载基址（仅 BIN/HEX）。
    #[arg(long, default_value = "0")]
    base: String,
    /// 步过/步出的最大执行步数（默认 100,000）。
    #[arg(long, default_value = "100000")]
    max_steps: u64,
    /// 输出 JSON。
    #[arg(long)]
    json: bool,
    /// 追加源代码位置标注（file:line，来自 DWARF 行表）。
    #[arg(long)]
    source: bool,
}

#[derive(Args)]
struct ContinueArgs {
    /// 会话文件路径（JSON）。
    #[arg(short, long)]
    session: PathBuf,
    /// 运行到指定地址（临时断点，不持久化）。
    #[arg(long)]
    until: Option<String>,
    /// 运行到指定符号地址（临时断点，不持久化；与 --until 互斥）。
    #[arg(long, conflicts_with = "until")]
    until_symbol: Option<String>,
    /// 运行到寄存器/表达式条件成立（每步后求值，如 `x1 == 5`；到断点/停机/步数上限提前停止）。
    #[arg(long)]
    until_condition: Option<String>,
    /// 运行到周期计数达到该值即停止（含 cycle-accurate 模式的指令周期预估；`lastResult="cycles"`）。
    #[arg(long)]
    max_cycles: Option<u64>,
    /// 最大执行步数（默认 1,000,000）。
    #[arg(long, default_value = "1000000")]
    max_steps: u64,
    /// 是否保存回会话。
    #[arg(long, default_value = "true")]
    save: bool,
    /// 输出 JSON。
    #[arg(long)]
    json: bool,
    /// 打印每条执行指令的 PC/反汇编/来源（逐步追踪）。
    #[arg(long)]
    trace: bool,
    /// 与 --trace 搭配：打印每条执行指令后发生变化的寄存器。
    #[arg(long)]
    trace_regs: bool,
    /// 在停止点追加 file:line 来源标注。
    #[arg(long)]
    source: bool,
}

#[derive(Args)]
struct RegArgs {
    #[command(subcommand)]
    sub: RegSub,
}

#[derive(Subcommand)]
enum RegSub {
    /// 列出所有寄存器。
    List {
        /// 会话文件路径（JSON；不存在则新建临时会话）。
        #[arg(short, long)]
        session: Option<PathBuf>,
        /// 目标架构（如 riscv/arm/mcs51/...；无会话时使用）。
        #[arg(short, long)]
        arch: Option<String>,
        /// 按名称子串筛选（大小写不敏感，如 `s` 或 `x1`）。
        #[arg(long)]
        filter: Option<String>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 读取单个寄存器。
    Get {
        /// 会话文件路径（JSON；不存在则新建临时会话）。
        #[arg(short, long)]
        session: Option<PathBuf>,
        /// 目标架构（如 riscv/arm/mcs51/...；无会话时使用）。
        #[arg(short, long)]
        arch: Option<String>,
        /// 寄存器名（如 x1、a0、R3、SP）。位置参数或 --name。
        #[arg(index = 1)]
        name: Option<String>,
        /// 寄存器名（--name 形式）。
        #[arg(long = "name", conflicts_with = "name")]
        name_flag: Option<String>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 写单个寄存器。
    Set {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 寄存器名（位置参数或 --name）。
        #[arg(index = 1)]
        name: Option<String>,
        /// 值（十六进制或十进制，位置参数或 --value）。
        #[arg(index = 2)]
        value: Option<String>,
        /// 寄存器名（--name 形式）。
        #[arg(long = "name", conflicts_with = "name")]
        name_flag: Option<String>,
        /// 值（--value 形式）。
        #[arg(long = "value", conflicts_with = "value")]
        value_flag: Option<String>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
struct MemArgs {
    #[command(subcommand)]
    sub: MemSub,
}

#[derive(Subcommand)]
enum MemSub {
    /// 读取内存（hex dump 或原始字节）。
    Read {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 目标架构（无会话时使用，默认 riscv）。
        #[arg(short, long)]
        arch: Option<String>,
        /// 起始地址（十六进制或十进制，位置参数或 --addr）。
        #[arg(index = 1)]
        addr: Option<String>,
        /// 起始地址（--addr 形式）。
        #[arg(long = "addr", conflicts_with = "addr")]
        addr_flag: Option<String>,
        /// 长度（字节，默认 64）。
        #[arg(long, default_value = "64")]
        len: usize,
        /// 数据宽度：`byte`/`half`/`word`/`dword`（默认 byte；此时 `--len` 为元素个数）。
        #[arg(long, default_value = "byte")]
        width: String,
        /// 以 ASCII 文本解读（可打印字符原样、其余显示为 `.`，忽略末尾 NUL）。
        #[arg(long, conflicts_with = "string")]
        ascii: bool,
        /// 读取以 NUL 结尾的字符串（最多 `--len` 字节；与 `--ascii` 互斥）。
        #[arg(long, conflicts_with = "ascii")]
        string: bool,
        /// 输出原始二进制到文件。
        #[arg(long)]
        out: Option<PathBuf>,
        /// 输出 JSON（地址/宽度/字节数组）。
        #[arg(long)]
        json: bool,
    },
    /// 写内存（字节或从文件）。
    Write {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 目标架构（无会话时使用，默认 riscv）。
        #[arg(short, long)]
        arch: Option<String>,
        /// 起始地址（位置参数或 --addr）。
        #[arg(index = 1)]
        addr: Option<String>,
        /// 起始地址（--addr 形式）。
        #[arg(long = "addr", conflicts_with = "addr")]
        addr_flag: Option<String>,
        /// 要写的字节（空格分隔，支持 0xNN）。
        #[arg(long, value_delimiter = ' ', num_args = 1..)]
        bytes: Vec<String>,
        /// 数据宽度：`byte`/`half`/`word`（默认 byte；配合 `--values` 写入小端值）。
        #[arg(long, default_value = "byte")]
        width: String,
        /// 要写的值（配合 `--width`；空格分隔，十进制/十六进制）。
        #[arg(long, value_delimiter = ' ', num_args = 1..)]
        values: Vec<String>,
        /// 从二进制文件读取字节写入。
        #[arg(long)]
        file: Option<PathBuf>,
        /// 写入字符串的 ASCII 字节（与 --file/--bytes/--values 互斥）。
        #[arg(long, conflicts_with_all = ["bytes", "values", "file"])]
        string: Option<String>,
        /// 配合 --string：追加 NUL 终止符。
        #[arg(long, requires = "string")]
        null: bool,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 填充内存区域。
    Fill {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 目标架构（无会话时使用，默认 riscv）。
        #[arg(short, long)]
        arch: Option<String>,
        /// 起始地址（位置参数或 --addr）。
        #[arg(index = 1)]
        addr: Option<String>,
        /// 起始地址（--addr 形式）。
        #[arg(long = "addr", conflicts_with = "addr")]
        addr_flag: Option<String>,
        /// 长度（字节）。
        #[arg(long, default_value = "1")]
        len: usize,
        /// 填充字节（0-255，位置参数或 --value）。
        #[arg(index = 2)]
        value: Option<String>,
        /// 填充字节（--value 形式）。
        #[arg(long = "value", conflicts_with = "value")]
        value_flag: Option<String>,
        /// 使用确定性伪随机字节填充（与 --value 互斥）。
        #[arg(long, conflicts_with_all = ["value", "value_flag"])]
        random: bool,
        /// 伪随机种子（十进制/十六进制；默认固定常量，可复现）。
        #[arg(long)]
        seed: Option<String>,
        /// 重复填充的字节模式（空格分隔，如 `0xDE 0xAD`；与 --value/--random 互斥）。
        #[arg(long, value_delimiter = ' ', num_args = 1.., conflicts_with_all = ["value", "value_flag", "random"])]
        pattern: Vec<String>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 在内存区域搜索模式（按 --width 解释 --pattern 的整数值，找到所有偏移）。
    Search {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 目标架构（无会话时使用，默认 riscv）。
        #[arg(short, long)]
        arch: Option<String>,
        /// 起始地址（位置参数或 --addr）。
        #[arg(index = 1)]
        addr: Option<String>,
        /// 起始地址（--addr 形式）。
        #[arg(long = "addr", conflicts_with = "addr")]
        addr_flag: Option<String>,
        /// 搜索长度（字节，默认 256）。
        #[arg(long, default_value = "256")]
        len: usize,
        /// 搜索模式（空格分隔的整数；宽度由 --width 决定，支持 0xNN）。
        #[arg(long, value_delimiter = ' ', num_args = 1..)]
        pattern: Vec<String>,
        /// 模式宽度：`byte`/`half`/`word`/`dword`（默认 byte；每个模式值按小端展开为字节）。
        #[arg(long, default_value = "byte")]
        width: String,
        /// 将搜索结果写入文件（缺省打印到 stdout）。
        #[arg(long)]
        out: Option<PathBuf>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 加载固件到内存（等价于 run 的加载步骤，不执行）。
    Load {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 固件文件（ELF / Intel HEX / BIN）。
        #[arg(short, long)]
        firmware: PathBuf,
        /// 目标架构（如 riscv/arm/mcs51/...；无会话时使用）。
        #[arg(short, long)]
        arch: Option<String>,
        /// 加载基址（十六进制或十进制）。
        #[arg(long, default_value = "0")]
        base: String,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 导出内存区域（C 数组或十六进制文本）。
    Dump {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 目标架构（无会话时使用，默认 riscv）。
        #[arg(short, long)]
        arch: Option<String>,
        /// 起始地址（十六进制或十进制，位置参数或 --addr）。
        #[arg(index = 1)]
        addr: Option<String>,
        /// 起始地址（--addr 形式）。
        #[arg(long = "addr", conflicts_with = "addr")]
        addr_flag: Option<String>,
        /// 长度（字节，默认 64）。
        #[arg(long, default_value = "64")]
        len: usize,
        /// C 数组名（仅 --format c）。
        #[arg(long, default_value = "data")]
        name: String,
        /// 输出格式：`c`（C 数组）、`hex`（Intel HEX，可回载）或 `dump`（十六进制+ASCII 文本）。
        #[arg(long, default_value = "c")]
        format: String,
        /// 写入文件路径（缺省打印到 stdout）。
        #[arg(long)]
        out: Option<PathBuf>,
        /// 以 JSON 输出地址/长度/格式/名称/字节数组。
        #[arg(long)]
        json: bool,
    },
    /// 转储设备命名内存区域（FLASH/RAM/PERIPHERALS），起始地址从目录解析。
    Region {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 目标架构（无会话时使用）。
        #[arg(short, long)]
        arch: Option<String>,
        /// 内置设备键（如 stm32f103c8）。
        #[arg(long)]
        device: String,
        /// 区域名（FLASH/RAM/PERIPHERALS，大小写不敏感）。
        #[arg(long)]
        region: String,
        /// 区域起始偏移（字节）。
        #[arg(long, default_value = "0")]
        offset: usize,
        /// 读取长度（字节；默认整个区域，上限 64 KiB）。
        #[arg(long)]
        len: Option<usize>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 复制内存区域（源地址 → 目标地址，重叠范围按 memmove 语义处理）。
    Copy {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 目标架构（无会话时使用，默认 riscv）。
        #[arg(short, long)]
        arch: Option<String>,
        /// 源起始地址（十六进制或十进制，位置参数或 --src）。
        #[arg(index = 1)]
        src: Option<String>,
        /// 源起始地址（--src 形式）。
        #[arg(long = "src", conflicts_with = "src")]
        src_flag: Option<String>,
        /// 目标起始地址（十六进制或十进制，位置参数或 --dst）。
        #[arg(index = 2)]
        dst: Option<String>,
        /// 目标起始地址（--dst 形式）。
        #[arg(long = "dst", conflicts_with = "dst")]
        dst_flag: Option<String>,
        /// 复制长度（字节）。
        #[arg(long, default_value = "1")]
        len: usize,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
struct BpArgs {
    #[command(subcommand)]
    sub: BpSub,
}

#[derive(Subcommand)]
enum BpSub {
    /// 列出断点。
    List {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 用 ELF 符号表注释断点地址（找到时追加符号名）。
        #[arg(long)]
        symbols: bool,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 设置断点。
    Set {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 地址（位置参数或 --addr）。
        #[arg(index = 1)]
        addr: Option<String>,
        /// 地址（--addr 形式）。
        #[arg(long = "addr", conflicts_with = "addr")]
        addr_flag: Option<String>,
        /// 按符号名设置断点（与地址互斥）。
        #[arg(long, conflicts_with_all = ["addr", "addr_flag"])]
        symbol: Option<String>,
        /// 按源码行号设置断点（`file:line`，如 `main.c:10`；与地址/符号互斥）。
        #[arg(long, conflicts_with_all = ["addr", "addr_flag", "symbol"])]
        line: Option<String>,
        /// 条件表达式（如 `x1 == 10`）。
        #[arg(long)]
        condition: Option<String>,
        /// 命中次数（第 N 次命中时暂停）。
        #[arg(long, default_value = "0")]
        hit_count: u32,
        /// 一次性断点：命中后自动清除。
        #[arg(long)]
        once: bool,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 修改已有断点的条件与命中次数（保留运行时命中计数，不重建断点）。
    Modify {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 地址（位置参数或 --addr）。
        #[arg(index = 1)]
        addr: Option<String>,
        /// 地址（--addr 形式）。
        #[arg(long = "addr", conflicts_with = "addr")]
        addr_flag: Option<String>,
        /// 按符号名定位断点（与地址互斥）。
        #[arg(long, conflicts_with_all = ["addr", "addr_flag"])]
        symbol: Option<String>,
        /// 新的条件表达式（如 `x1 == 10`）。
        #[arg(long)]
        condition: Option<String>,
        /// 新的命中次数（第 N 次命中时暂停）。
        #[arg(long)]
        hit_count: Option<u32>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 清除断点。
    Clear {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 地址（位置参数或 --addr）。
        #[arg(index = 1)]
        addr: Option<String>,
        /// 地址（--addr 形式）。
        #[arg(long = "addr", conflicts_with = "addr")]
        addr_flag: Option<String>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 清除全部断点。
    ClearAll {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 启用断点。
    Enable {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 地址（位置参数或 --addr）。
        #[arg(index = 1)]
        addr: Option<String>,
        /// 地址（--addr 形式）。
        #[arg(long = "addr", conflicts_with = "addr")]
        addr_flag: Option<String>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 禁用断点（保留在列表中但不再暂停）。
    Disable {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 地址（位置参数或 --addr）。
        #[arg(index = 1)]
        addr: Option<String>,
        /// 地址（--addr 形式）。
        #[arg(long = "addr", conflicts_with = "addr")]
        addr_flag: Option<String>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
struct WatchArgs {
    #[command(subcommand)]
    sub: WatchSub,
}

/// 内存观察点（watchpoint）管理。`step`/`run`/`continue` 在每条指令前后比较被监视
/// 的字节范围，被写入时暂停（`lastResult="watchpoint"`，`watchHit` 报告地址）。
#[derive(Subcommand)]
enum WatchSub {
    /// 设置内存观察点（地址 + 长度）。
    Set {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 监视起始地址（位置参数或 --addr）。
        #[arg(index = 1)]
        addr: Option<String>,
        /// 监视起始地址（--addr 形式）。
        #[arg(long = "addr", conflicts_with = "addr")]
        addr_flag: Option<String>,
        /// 监视字节数（1..64，默认 1）。
        #[arg(long, default_value = "1")]
        len: u32,
        /// 第 N 次写入时暂停（0 = 每次写入）。
        #[arg(long, default_value = "0")]
        hit_count: u32,
        /// 一次性观察点：命中后自动禁用。
        #[arg(long)]
        once: bool,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 列出内存观察点。
    List {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 清除观察点。
    Clear {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 地址（位置参数或 --addr）。
        #[arg(index = 1)]
        addr: Option<String>,
        /// 地址（--addr 形式）。
        #[arg(long = "addr", conflicts_with = "addr")]
        addr_flag: Option<String>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 清除全部观察点。
    ClearAll {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 启用观察点。
    Enable {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 地址（位置参数或 --addr）。
        #[arg(index = 1)]
        addr: Option<String>,
        /// 地址（--addr 形式）。
        #[arg(long = "addr", conflicts_with = "addr")]
        addr_flag: Option<String>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 禁用观察点（保留在列表中但不再暂停）。
    Disable {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 地址（位置参数或 --addr）。
        #[arg(index = 1)]
        addr: Option<String>,
        /// 地址（--addr 形式）。
        #[arg(long = "addr", conflicts_with = "addr")]
        addr_flag: Option<String>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
struct DisasmArgs {
    /// 会话文件路径（JSON；不存在则新建临时会话）。
    #[arg(short, long)]
    session: Option<PathBuf>,
    /// 固件文件（ELF / Intel HEX / BIN；直接加载后反汇编，无需先建会话）。
    #[arg(long)]
    firmware: Option<PathBuf>,
    /// BIN/HEX 加载基址（十六进制或十进制；ELF 忽略）。
    #[arg(long, default_value = "0")]
    base: String,
    /// 目标架构（如 riscv/arm/mcs51/...；无会话时使用）。
    #[arg(short, long)]
    arch: Option<String>,
    /// 起始地址（默认当前 PC）。
    #[arg(short = 'd', long)]
    addr: Option<String>,
    /// 按符号名起始（如 main；与 --addr 互斥）。
    #[arg(long, conflicts_with = "addr")]
    symbol: Option<String>,
    /// 指令条数（默认 8）。
    #[arg(long, default_value = "8")]
    count: usize,
    /// ARM 状态：`a32` 或 `thumb`（仅 ARM；默认取内核当前状态）。
    #[arg(long, value_parser = ["a32", "thumb"])]
    arm_state: Option<String>,
    /// 标注每行来源（file:line，来自 ELF 行表/符号回退）。
    #[arg(long)]
    source: bool,
    /// 在符号起始地址处插入 `名称:` 标签行（来自 ELF 符号表）。
    #[arg(long)]
    labels: bool,
    /// 同时显示每条指令的原始字节（十六进制）。
    #[arg(long)]
    raw: bool,
    /// 写出文件路径（缺省打印到 stdout；文本或 JSON 均可）。
    #[arg(short, long)]
    out: Option<PathBuf>,
    /// 输出 JSON 数组（地址/反汇编/原始字节/来源）。
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct ResetArgs {
    /// 会话文件路径（JSON）。
    #[arg(short, long)]
    session: PathBuf,
    /// 重置 PC 到入口/基址。
    #[arg(long)]
    entry: Option<String>,
    /// 输出 JSON。
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct StateArgs {
    /// 会话文件路径（JSON；不存在则新建临时会话）。
    #[arg(short, long)]
    session: Option<PathBuf>,
    /// 目标架构（如 riscv/arm/mcs51/...；无会话时使用）。
    #[arg(short, long)]
    arch: Option<String>,
    /// 固件文件（ELF / Intel HEX / BIN）。
    #[arg(long)]
    firmware: Option<PathBuf>,
    /// 加载基址（十六进制或十进制）。
    #[arg(long, default_value = "0")]
    base: String,
    /// 输出 JSON（默认）。
    #[arg(long)]
    json: bool,
    /// 逗号分隔的 JSON 字段子集（如 `pc,registers,csr`），仅 `--json` 生效。
    #[arg(long, value_delimiter = ',')]
    fields: Vec<String>,
    /// 附带内存快照的起始地址（如 0x20000000；读取 `--mem-len` 字节）。
    #[arg(long)]
    mem: Option<String>,
    /// 内存快照长度（字节，默认 64；与 `--mem` 搭配）。
    #[arg(long, default_value = "64")]
    mem_len: usize,
    /// 单行摘要（PC/周期/停止），忽略字段子集。
    #[arg(long)]
    summary: bool,
}

#[derive(Args)]
struct PeriphArgs {
    #[command(subcommand)]
    sub: PeriphSub,
}

#[derive(Subcommand)]
enum PeriphSub {
    /// 显示虚拟外设状态（UART 输出、GPIO、定时器、看门狗）。
    Status {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 配置虚拟外设基址（UART/GPIO，可选）。
    Config {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// UART 寄存器基址（默认 0x4000_0000）。
        #[arg(long)]
        uart_base: Option<String>,
        /// GPIO 寄存器基址（默认 0x4000_2000）。
        #[arg(long)]
        gpio_base: Option<String>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 从 SVD 文件或内置设备加载芯片寄存器（地址 -> 复位值）到仿真器。
    SvdLoad {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// SVD 文件路径（与 --device 二选一）。
        #[arg(short, long)]
        file: Option<PathBuf>,
        /// 内置设备键（如 stm32f103c8；与 --file 二选一）。
        #[arg(long)]
        device: Option<String>,
        /// 只加载指定外设（按名称，可选）。
        #[arg(long)]
        peripheral: Option<String>,
        /// 覆盖既有寄存器值（默认 false：只写入缺失的）。
        #[arg(long)]
        force: bool,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 读取 UART 输出字节数 / 清空 / 注入输入字节（供固件从数据寄存器读取）。
    Uart {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 清空输出缓冲。
        #[arg(long)]
        clear: bool,
        /// 注入 UART 输入文本（UTF-8 字节，FIFO）。
        #[arg(long)]
        input: Option<String>,
        /// 注入 UART 输入字节（空格分隔，支持 0xNN；与 --input 互斥）。
        #[arg(long, value_delimiter = ' ', num_args = 1.., conflicts_with = "input")]
        bytes: Vec<String>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 设置 GPIO 输入引脚值。
    Gpio {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 引脚号 0-31（位置参数或 --pin）。
        #[arg(index = 1)]
        pin: Option<u32>,
        /// 引脚号（--pin 形式）。
        #[arg(long = "pin", conflicts_with = "pin")]
        pin_flag: Option<u32>,
        /// 引脚值 0/1（位置参数或 --value）。
        #[arg(index = 2)]
        value: Option<u32>,
        /// 引脚值（--value 形式）。
        #[arg(long = "value", conflicts_with = "value")]
        value_flag: Option<u32>,
        /// 设置该引脚方向（0 = 输入，1 = 输出）。
        #[arg(long)]
        direction: Option<u32>,
        /// 设置该引脚输出电平（0/1；写入 gpio_out 位）。
        #[arg(long)]
        output: Option<u32>,
        /// 仅读取当前引脚状态（不修改）。
        #[arg(long)]
        read: bool,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 查询/配置虚拟定时器。
    Timer {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 定时器索引（0..8）。
        #[arg(long, default_value = "0")]
        index: usize,
        /// 启用/禁用（`--enabled true|false`）。
        #[arg(long)]
        enabled: Option<bool>,
        /// 预分频（可选，仅配置时使用）。
        #[arg(long)]
        prescaler: Option<u32>,
        /// 自动重载值（可选，仅配置时使用）。
        #[arg(long)]
        auto_reload: Option<u32>,
        /// 直接设置运行时计数器值（可选）。
        #[arg(long)]
        counter: Option<u32>,
        /// 直接设置更新计数（可选）。
        #[arg(long)]
        update_count: Option<u64>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 读取外设内存映射寄存器（触发外设读）。
    RegRead {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 外设寄存器地址（与 --name 二选一）。
        #[arg(long = "addr")]
        addr: Option<String>,
        /// 内置设备键（配合 --name，如 stm32f103c8）。
        #[arg(long)]
        device: Option<String>,
        /// 寄存器名 PERIPH.REG（如 TIM2.CR1，与 --addr 二选一）。
        #[arg(long)]
        name: Option<String>,
        /// 读取字节数（默认 4）。
        #[arg(long, default_value = "4")]
        len: usize,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 列出会话中已加载（SVD）的外设寄存器。
    RegList {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 用内置设备键注释寄存器名（如 stm32f103c8，可选）。
        #[arg(long)]
        device: Option<String>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 转储设备全部外设寄存器的当前值（命名地址到值；读取 SVD 加载或内存）。
    RegDump {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 内置设备键（如 stm32f103c8）。
        #[arg(long)]
        device: String,
        /// 只转储指定外设（按名称，可选）。
        #[arg(long)]
        peripheral: Option<String>,
        /// 附带 SVD 位域解码（每个字段的位偏移/宽度/当前值/描述）。
        #[arg(long)]
        fields: bool,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 写入外设内存映射寄存器（触发外设写，如 UART 输出、TIMER 控制）。
    RegWrite {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 外设寄存器地址（与 --name 二选一）。
        #[arg(long = "addr")]
        addr: Option<String>,
        /// 内置设备键（配合 --name，如 stm32f103c8）。
        #[arg(long)]
        device: Option<String>,
        /// 寄存器名 PERIPH.REG（如 TIM2.CR1，与 --addr 二选一）。
        #[arg(long)]
        name: Option<String>,
        /// 要写入的字节（空格分隔，支持 0xNN）。
        #[arg(long, value_delimiter = ' ', num_args = 1..)]
        bytes: Vec<String>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 读取外设寄存器的命名位域（SVD field）。
    FieldGet {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 内置设备键（如 stm32f103c8）。
        #[arg(long)]
        device: String,
        /// 外设名（如 TIM2）。
        #[arg(long)]
        peripheral: String,
        /// 寄存器名（如 CR1）。
        #[arg(long)]
        register: String,
        /// 位域名（如 CEN）。
        #[arg(long)]
        field: String,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 写外设寄存器的命名位域（SVD field），保留其他位。
    FieldSet {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 内置设备键（如 stm32f103c8）。
        #[arg(long)]
        device: String,
        /// 外设名（如 TIM2）。
        #[arg(long)]
        peripheral: String,
        /// 寄存器名（如 CR1）。
        #[arg(long)]
        register: String,
        /// 位域名（如 CEN）。
        #[arg(long)]
        field: String,
        /// 字段值（十进制/十六进制）。
        #[arg(long)]
        value: String,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
struct SymbolArgs {
    #[command(subcommand)]
    sub: SymbolSub,
}

#[derive(Subcommand)]
enum SymbolSub {
    /// 列出 ELF 符号表（名称/地址/大小）。
    List {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 按名称子串过滤（大小写不敏感）。
        #[arg(long)]
        filter: Option<String>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 查询地址处最接近的符号。
    At {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 地址（位置参数或 --addr）。
        #[arg(index = 1)]
        addr: Option<String>,
        /// 地址（--addr 形式，十六进制或十进制）。
        #[arg(long = "addr", conflicts_with = "addr")]
        addr_flag: Option<String>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 查询最接近且地址不大于给定地址的符号（含偏移）。
    Nearest {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 地址（位置参数或 --addr）。
        #[arg(index = 1)]
        addr: Option<String>,
        /// 地址（--addr 形式，十六进制或十进制）。
        #[arg(long = "addr", conflicts_with = "addr")]
        addr_flag: Option<String>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 地址 → (源文件, 行号) 映射。
    Line {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 地址（位置参数或 --addr）。
        #[arg(index = 1)]
        addr: Option<String>,
        /// 地址（--addr 形式，十六进制或十进制）。
        #[arg(long = "addr", conflicts_with = "addr")]
        addr_flag: Option<String>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// (源文件, 行号) → 地址 映射。
    Addr {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 源文件索引（0 基）。
        #[arg(long)]
        file: usize,
        /// 源行号（1 基）。
        #[arg(long)]
        line: u32,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
struct CycleArgs {
    #[command(subcommand)]
    sub: CycleSub,
}

#[derive(Subcommand)]
enum CycleSub {
    /// 读取当前周期计数。
    Get {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 估算指定地址的指令周期数。
    Estimate {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 指令地址（位置参数或 --addr）。
        #[arg(index = 1)]
        addr: Option<String>,
        /// 地址（--addr 形式，十六进制或十进制）。
        #[arg(long = "addr", conflicts_with = "addr")]
        addr_flag: Option<String>,
        /// ELF 符号名（与 --addr 二选一；从会话固件符号表解析地址）。
        #[arg(long = "symbol")]
        symbol: Option<String>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 推进外设 N 个核心周期（不执行指令）。
    Tick {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 周期数（默认 1）。
        #[arg(long, default_value = "1")]
        cycles: u64,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 设置周期计数（不执行指令；用于模拟时间流逝/超时）。
    Set {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 目标周期数（位置参数或 --count）。
        #[arg(index = 1)]
        count: Option<String>,
        /// 目标周期数（--count 形式，十六进制或十进制）。
        #[arg(long = "count", conflicts_with = "count")]
        count_flag: Option<String>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
struct InterruptArgs {
    #[command(subcommand)]
    sub: InterruptSub,
}

#[derive(Subcommand)]
enum InterruptSub {
    /// 触发一次中断（需要 mstatus.MIE 且 mie 对应位使能）。
    Take {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 查询当前挂起/使能的中断。
    Pending {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 设置/清除外设中断线。
    Set {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 中断线 0..15（位置参数或 --line）。
        #[arg(index = 1)]
        line: Option<usize>,
        /// 中断线（--line 形式）。
        #[arg(long = "line", conflicts_with = "line")]
        line_flag: Option<usize>,
        /// 置位/清除（位置参数或 --value，默认 1）。
        #[arg(index = 2)]
        value: Option<u32>,
        /// 值（--value 形式，十六进制或十进制）。
        #[arg(long = "value", conflicts_with = "value")]
        value_flag: Option<u32>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
struct WatchdogArgs {
    #[command(subcommand)]
    sub: WatchdogSub,
}

#[derive(Subcommand)]
enum WatchdogSub {
    /// 查询看门狗状态。
    Status {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 喂狗（写 0xAA55 重载超时）。
    Feed {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 配置看门狗（启用/禁用、超时周期、基址）。
    Config {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 启用/禁用（true/false）。
        #[arg(long)]
        enabled: Option<bool>,
        /// 超时周期数（0 = 禁用）。
        #[arg(long)]
        timeout: Option<u64>,
        /// 寄存器基址（默认 0x4000_0000）。
        #[arg(long)]
        base: Option<String>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 强制触发看门狗复位（记录复位事件并复位核心）。
    Reset {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 复位后 PC（默认当前 PC）。
        #[arg(long)]
        entry: Option<String>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
struct CsrArgs {
    #[command(subcommand)]
    sub: CsrSub,
}

#[derive(Subcommand)]
enum CsrSub {
    /// 列出全部 CSR（RISC-V）。
    List {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 读取单个 CSR。
    Get {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// CSR 名（mstatus/mtvec/mepc/mcause/mie/mhartid）。
        #[arg(index = 1)]
        name: Option<String>,
        /// 名称（--name 形式）。
        #[arg(long = "name", conflicts_with = "name")]
        name_flag: Option<String>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
    /// 写入单个 CSR。
    Set {
        /// 会话文件路径（JSON）。
        #[arg(short, long)]
        session: PathBuf,
        /// 名称（位置参数或 --name）。
        #[arg(index = 1)]
        name: Option<String>,
        /// 值（位置参数或 --value，十六进制或十进制）。
        #[arg(index = 2)]
        value: Option<String>,
        /// 名称（--name 形式）。
        #[arg(long = "name", conflicts_with = "name")]
        name_flag: Option<String>,
        /// 值（--value 形式，十六进制或十进制）。
        #[arg(long = "value", conflicts_with = "value")]
        value_flag: Option<String>,
        /// 以 JSON 格式输出（机器可读）。
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
struct AnalyzeArgs {
    /// 会话文件（不提供则新建临时会话）。
    #[arg(short, long)]
    session: Option<PathBuf>,
    /// 目标架构（无会话时使用）。
    #[arg(short, long)]
    arch: Option<String>,
    /// 固件文件（无会话时使用）。
    #[arg(long)]
    firmware: Option<PathBuf>,
    /// 固件加载基址（仅 BIN/HEX）。
    #[arg(long, default_value = "0")]
    base: String,
    /// 精确追踪 N 条指令（优先级高于 --max-steps；到断点/停机则提前停止）。
    #[arg(long)]
    steps: Option<u64>,
    /// 最大执行步数（默认 100,000）。
    #[arg(long, default_value = "100000")]
    max_steps: u64,
    /// 运行到周期计数达到该值即停止（含 cycle-accurate 模式的指令周期预估；`result="cycles"`）。
    #[arg(long)]
    max_cycles: Option<u64>,
    /// 运行到指定地址（临时断点，不持久化）。
    #[arg(long)]
    until: Option<String>,
    /// 运行到指定符号地址（临时断点，不持久化；与 --until 互斥）。
    #[arg(long, conflicts_with = "until")]
    until_symbol: Option<String>,
    /// trace 时附带每条指令执行后的寄存器快照（数据流追踪）。
    #[arg(long)]
    regs: bool,
    /// 写出文件路径（缺省打印到 stdout；文本或 JSON 均可）。
    #[arg(short, long)]
    out: Option<PathBuf>,
    /// 输出 JSON。
    #[arg(long)]
    json: bool,
}

/// Session arguments for the `session info` subcommand.
#[derive(Args)]
struct SessionInfoArgs {
    /// 会话文件（JSON）。
    #[arg(short = 'f', long)]
    file: PathBuf,
    /// 输出 JSON。
    #[arg(long)]
    json: bool,
}

/// Session arguments for the `session diff` subcommand.
#[derive(Args)]
struct SessionDiffArgs {
    /// 会话文件 A（基准）。
    #[arg(short, long)]
    a: PathBuf,
    /// 会话文件 B（比较对象）。
    #[arg(short, long)]
    b: PathBuf,
    /// 最多列出的差异条数（默认 400）。
    #[arg(long, default_value = "400")]
    limit: usize,
    /// 输出 JSON。
    #[arg(long)]
    json: bool,
}

fn main() {
    if let Err(e) = real_main() {
        eprintln!("错误: {e}");
        std::process::exit(1);
    }
}

fn real_main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Version { json } => {
            if json {
                print_json(&serde_json::json!({
                    "name": "ns-instruction-sim",
                    "version": env!("CARGO_PKG_VERSION"),
                }))?;
            } else {
                println!("ns-instruction-sim {}", env!("CARGO_PKG_VERSION"));
            }
            Ok(())
        }
        Command::Arch { sub: ArchSub::List { json } } => cmd_arch_list(json),
        Command::Device { sub: DeviceSub::List { json } } => cmd_device_list(json),
        Command::Device { sub: DeviceSub::Info { device, json } } => cmd_device_info(&device, json),
        Command::Device { sub: DeviceSub::Linker { device, out, json } } => {
            cmd_device_codegen(&device, out.as_deref(), CodegenKind::Linker, json)
        }
        Command::Device { sub: DeviceSub::Startup { device, out, json } } => {
            cmd_device_codegen(&device, out.as_deref(), CodegenKind::Startup, json)
        }
        Command::Device { sub: DeviceSub::Init { device, peripheral, register, out, json } } => {
            cmd_device_init(&device, &peripheral, &register, out.as_deref(), json)
        }
        Command::Device { sub: DeviceSub::Svd { file, device, peripheral, register, json } } => {
            cmd_device_svd(
                file.as_deref(),
                device.as_deref(),
                peripheral.as_deref(),
                register.as_deref(),
                json,
            )
        }
        Command::Device { sub: DeviceSub::MemoryMap { device, addr, json } } => {
            cmd_device_memory_map(&device, addr.as_deref(), json)
        }
        Command::Run(args) => cmd_run(args),
        Command::Step(args) => cmd_step(args, StepKind::Step),
        Command::StepOver(args) => cmd_step(args, StepKind::StepOver),
        Command::StepOut(args) => cmd_step(args, StepKind::StepOut),
        Command::Continue(args) => cmd_continue(args),
        Command::Reg(RegArgs { sub }) => cmd_reg(sub),
        Command::Mem(MemArgs { sub }) => cmd_mem(sub),
        Command::Bp(BpArgs { sub }) => cmd_bp(sub),
        Command::Watch(WatchArgs { sub }) => cmd_watch(sub),
        Command::Disasm(args) => cmd_disasm(args),
        Command::Reset(args) => cmd_reset(args),
        Command::State(args) => cmd_state(args),
        Command::Periph(PeriphArgs { sub }) => cmd_periph(sub),
        Command::Symbol(SymbolArgs { sub }) => cmd_symbol(sub),
        Command::Cycle(CycleArgs { sub }) => cmd_cycle(sub),
        Command::Interrupt(InterruptArgs { sub }) => cmd_interrupt(sub),
        Command::Watchdog(WatchdogArgs { sub }) => cmd_watchdog(sub),
        Command::Csr(CsrArgs { sub }) => cmd_csr(sub),
        Command::Session(SessionArgs { sub: SessionSub::Info(args) }) => cmd_session_info(args),
        Command::Session(SessionArgs { sub: SessionSub::Diff(args) }) => cmd_session_diff(args),
        Command::Firmware { sub: FirmwareSub::Info { firmware, arch, json } } => {
            cmd_firmware_info(&firmware, arch.as_deref(), json)
        }
        Command::Trace(args) => cmd_analyze(args, AnalyzeKind::Trace),
        Command::Coverage(args) => cmd_analyze(args, AnalyzeKind::Coverage),
        Command::Profile(args) => cmd_analyze(args, AnalyzeKind::Profile),
        Command::Mcp { session } => mcp::run(session),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn arch_from_arg(arch: Option<&str>) -> Result<Arch> {
    match arch {
        Some(name) => session::parse_arch(name),
        None => Ok(Arch::RiscV),
    }
}

/// Resolve the architecture for a session or a fresh sim.
fn resolve_arch(session: Option<&Path>, arch_arg: Option<&str>) -> Result<Arch> {
    if let Some(sp) = session {
        if sp.exists() {
            let s = Session::load(sp)?;
            return session::parse_arch(&s.arch);
        }
    }
    arch_from_arg(arch_arg)
}

/// Print JSON to stdout with pretty formatting.
fn print_json<T: serde::Serialize>(value: &T) -> Result<()> {
    let text = serde_json::to_string_pretty(value)?;
    println!("{text}");
    Ok(())
}

/// Hex dump of a memory slice.
fn hex_dump(addr: u32, bytes: &[u8]) -> String {
    let mut out = String::new();
    for (i, chunk) in bytes.chunks(16).enumerate() {
        let base = addr as usize + i * 16;
        out.push_str(&format!("{base:08x}  "));
        for (j, b) in chunk.iter().enumerate() {
            out.push_str(&format!("{b:02x} "));
            if j == 7 {
                out.push(' ');
            }
        }
        for _ in chunk.len()..16 {
            out.push_str("   ");
            if chunk.len() <= 8 {
                out.push(' ');
            }
        }
        out.push_str(" |");
        for b in chunk {
            let c = if b.is_ascii_graphic() || *b == b' ' { *b as char } else { '.' };
            out.push(c);
        }
        out.push('|');
        out.push('\n');
    }
    out
}

/// Format a StepResult for display.
fn fmt_step_result(result: StepResult) -> &'static str {
    match result {
        StepResult::Advanced => "advanced",
        StepResult::Breakpoint => "breakpoint",
        StepResult::Halted => "halted",
    }
}

/// Ensure a session exists, creating it from the given simulator.
/// Load an existing session into a simulator, or create a fresh one.
pub(crate) fn load_or_create(
    session_path: Option<&Path>,
    arch_arg: Option<&str>,
    firmware: Option<&Path>,
    base: &str,
) -> Result<(Simulator, Option<FirmwareState>, Option<PathBuf>)> {
    if let Some(sp) = session_path {
        if sp.exists() {
            let s = Session::load(sp)?;
            let fw = s.firmware.clone();
            let sim = s.apply(None)?;
            return Ok((sim, fw, Some(sp.to_path_buf())));
        }
    }
    let arch = arch_from_arg(arch_arg)?;
    let mut sim = Simulator::for_arch(arch)?;
    let base_num = parse_num(base)?;
    let mut fw = None;
    if let Some(path) = firmware {
        load_firmware_into(&mut sim, path, base_num)?;
        fw = Some(FirmwareState {
            path: path.to_string_lossy().into_owned(),
            base: base_num,
            format: detect_format(path).label().to_string(),
        });
    }
    // Persist a freshly-created session so subsequent commands can operate on
    // it (e.g. `state --arch riscv --session s.json` then `run --session s.json`).
    if let Some(sp) = session_path {
        let s = Session::capture(&sim, fw.clone());
        s.save(sp)?;
    }
    Ok((sim, fw, session_path.map(|p| p.to_path_buf())))
}

/// Common state output for `run`/`step`/`continue`.
#[derive(serde::Serialize)]
pub(crate) struct StateOutput {
    arch: String,
    pc: u32,
    #[serde(rename = "cycleCount")]
    cycle_count: u64,
    halted: bool,
    #[serde(rename = "lastResult")]
    last_result: String,
    registers: BTreeMap<String, u32>,
    /// ARM 指令集状态（仅 ARM；`a32`/`thumb`，其余架构为 null）。
    #[serde(rename = "armState")]
    arm_state: Option<String>,
    /// ARM xPSR（仅 ARM：N/Z/C/V 条件标志、Thumb 位、当前异常号；其余架构为
    /// null）。让外部调用方无需执行指令即可读取条件标志（例如评估条件分支）。
    #[serde(rename = "armXpsr")]
    arm_xpsr: Option<u32>,
    /// ARM If-Then (IT) 块状态（仅 ARM：`{mask,cond,index}`，`mask==0` 表示无
    /// 活动 IT 块；其余架构为 null）。让外部调用方在 Thumb-2 IT 块中途观察
    /// 剩余条件指令（评估条件执行是否命中）。
    #[serde(rename = "armItState")]
    arm_it_state: Option<serde_json::Value>,
    /// 本架构可执行的最大 PC 地址（16 位 PC 架构为 0xFFFF，其余为 u32::MAX）。
    #[serde(rename = "pcLimit")]
    pc_limit: u32,
    uart: Vec<u8>,
    #[serde(rename = "uartText")]
    uart_text: String,
    /// Virtual-peripheral snapshot (UART/GPIO/timers/watchdog/interrupts).
    peripherals: serde_json::Value,
    /// RISC-V machine-mode CSR snapshot.
    csr: serde_json::Value,
    /// Active breakpoints (address, condition, configured hit count, runtime hits).
    breakpoints: Vec<BreakpointOutput>,
    /// Loaded firmware info (path/base/format), if any.
    firmware: Option<FirmwareOutput>,
    /// Nearest ELF symbol at PC (name/address/size), if any.
    #[serde(rename = "pcSymbol")]
    pc_symbol: Option<PcSymbolOutput>,
    /// Address of the memory write that tripped a watchpoint (null otherwise),
    /// so an external caller can tell why the run stopped.
    #[serde(rename = "watchHit")]
    watch_hit: Option<u32>,
}

#[derive(serde::Serialize)]
struct BreakpointOutput {
    address: u32,
    condition: String,
    #[serde(rename = "hitCount")]
    hit_count: u32,
    once: bool,
    enabled: bool,
    hits: u64,
}

#[derive(serde::Serialize)]
struct FirmwareOutput {
    path: String,
    base: u32,
    format: String,
}

#[derive(serde::Serialize)]
struct PcSymbolOutput {
    name: String,
    address: u32,
    size: u32,
}

fn peripherals_json(sim: &Simulator) -> serde_json::Value {
    let timers: Vec<_> = sim
        .peripherals
        .timers
        .iter()
        .map(|t| {
            serde_json::json!({
                "base": t.base,
                "enabled": t.enabled,
                "counter": t.counter,
                "autoReload": t.auto_reload,
                "prescaler": t.prescaler,
                "irqLine": t.irq_line,
                "updateCount": t.update_count,
            })
        })
        .collect();
    let watchdog = sim.peripherals.watchdog.as_ref().map(|w| {
        serde_json::json!({
            "base": w.base,
            "enabled": w.enabled,
            "timeoutCycles": w.timeout_cycles,
            "elapsed": w.elapsed,
            "resetCount": w.reset_count,
            "lastResetPc": w.last_reset_pc,
        })
    });
    let irq_pending: Vec<usize> = sim
        .peripherals
        .irq_pending
        .iter()
        .enumerate()
        .filter(|(_, p)| **p)
        .map(|(i, _)| i)
        .collect();
    serde_json::json!({
        "uartBase": sim.peripherals.uart_base,
        "uartInput": sim.peripherals.uart_rx.borrow().iter().copied().collect::<Vec<_>>(),
        "gpioBase": sim.peripherals.gpio_base,
        "uart": sim.peripherals.outputs,
        "uartText": String::from_utf8_lossy(&sim.peripherals.outputs),
        "gpioOut": sim.peripherals.gpio_out,
        "gpioDir": sim.peripherals.gpio_dir,
        "gpioIn": sim.peripherals.gpio_in,
        "timers": timers,
        "watchdog": watchdog,
        "irqPending": irq_pending,
        "externalRegs": sim.peripherals.external_regs,
    })
}

fn csr_json(sim: &Simulator) -> serde_json::Value {
    serde_json::json!({
        "mstatus": sim.csr.mstatus,
        "mtvec": sim.csr.mtvec,
        "mepc": sim.csr.mepc,
        "mcause": sim.csr.mcause,
        "mie": sim.csr.mie,
        "mhartid": sim.csr.mhartid,
    })
}

pub(crate) fn state_output(sim: &Simulator, result: StepResult, firmware: Option<&FirmwareState>) -> StateOutput {
    let mut registers = BTreeMap::new();
    let count = ns_sim::reg_count_for(sim.arch);
    for i in 0..count {
        registers.insert(ns_sim::reg_label_for(sim.arch, i), sim.read_reg(i));
    }
    StateOutput {
        arch: session::arch_name(sim.arch).to_string(),
        pc: sim.program_counter(),
        cycle_count: sim.cycle_count,
        halted: sim.halted,
        last_result: fmt_step_result(result).to_string(),
        registers,
        arm_state: if sim.arch == ns_sim::Arch::Arm {
            Some(if sim.arm_state() { "a32" } else { "thumb" }.to_string())
        } else {
            None
        },
        arm_xpsr: if sim.arch == ns_sim::Arch::Arm {
            sim.arm_xpsr()
        } else {
            None
        },
        arm_it_state: if sim.arch == ns_sim::Arch::Arm {
            sim.arm_it_state().map(|(mask, cond, index)| {
                serde_json::json!({ "mask": mask, "cond": cond, "index": index })
            })
        } else {
            None
        },
        pc_limit: sim.pc_limit(),
        uart: sim.peripherals.outputs.clone(),
        uart_text: String::from_utf8_lossy(&sim.peripherals.outputs).into_owned(),
        peripherals: peripherals_json(sim),
        csr: csr_json(sim),
        breakpoints: sim
            .breakpoints
            .iter()
            .map(|bp| BreakpointOutput {
                address: bp.address,
                condition: bp.condition.clone(),
                hit_count: bp.hit_count,
                once: bp.once,
                enabled: bp.enabled,
                hits: bp.hits(),
            })
            .collect(),
        firmware: firmware.map(|f| FirmwareOutput {
            path: f.path.clone(),
            base: f.base,
            format: f.format.clone(),
        }),
        pc_symbol: sim.symbol_at(sim.program_counter()).map(|s| PcSymbolOutput {
            name: s.name.clone(),
            address: s.address,
            size: s.size,
        }),
        watch_hit: None,
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

fn cmd_arch_list(json: bool) -> Result<()> {
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
    if json {
        let items: Vec<_> = archs
            .iter()
            .map(|(name, label, device)| serde_json::json!({
                "name": name,
                "label": label,
                "exampleDevice": device,
            }))
            .collect();
        print_json(&items)?;
        return Ok(());
    }
    println!("{:<10} {:<34} {}", "ARCH", "LABEL", "EXAMPLE_DEVICE");
    for (name, label, device) in archs {
        println!("{name:<10} {label:<34} {device}");
    }
    Ok(())
}

fn cmd_device_list(json: bool) -> Result<()> {
    if json {
        let items: Vec<_> = ns_device::CATALOG
            .iter()
            .map(|e| serde_json::json!({
                "key": e.key,
                "label": e.label,
                "core": e.core,
                "flashSize": e.flash_size,
                "ramSize": e.ram_size,
            }))
            .collect();
        print_json(&items)?;
        return Ok(());
    }
    println!("{:<20} {:<34} {:<16} {:>10} {:>10}", "KEY", "LABEL", "CORE", "FLASH", "RAM");
    for entry in ns_device::CATALOG {
        println!(
            "{:<20} {:<34} {:<16} {:>10} {:>10}",
            entry.key, entry.label, entry.core, entry.flash_size, entry.ram_size
        );
    }
    Ok(())
}

fn cmd_device_info(device: &str, json: bool) -> Result<()> {
    let entry = ns_device::catalog_entry(device)
        .ok_or_else(|| anyhow::anyhow!("未知设备: {device}"))?;
    if json {
        let regions: Vec<_> = ns_device::memory_map(device)
            .iter()
            .map(|r| serde_json::json!({ "name": r.name, "start": r.start, "size": r.size }))
            .collect();
        let peripherals = ns_device::device_for_target(device)
            .map(|dev| {
                dev.sorted_peripherals()
                    .iter()
                    .map(|p| serde_json::json!({
                        "name": p.name,
                        "baseAddress": p.base_address,
                        "description": p.description,
                        "registers": p.registers.iter().map(register_json).collect::<Vec<_>>(),
                    }))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        print_json(&serde_json::json!({
            "key": entry.key,
            "label": entry.label,
            "core": entry.core,
            "flashStart": entry.flash_start,
            "flashSize": entry.flash_size,
            "ramStart": entry.ram_start,
            "ramSize": entry.ram_size,
            "memoryMap": regions,
            "peripherals": peripherals,
        }))?;
        return Ok(());
    }
    println!("设备: {}", entry.label);
    println!("  键: {}", entry.key);
    println!("  核心: {}", entry.core);
    println!("  Flash: 0x{:08x} ({} B)", entry.flash_start, entry.flash_size);
    println!("  RAM:   0x{:08x} ({} B)", entry.ram_start, entry.ram_size);
    println!();
    println!("内存映射:");
    for region in ns_device::memory_map(device) {
        println!("  {:<16} 0x{:012x}  size={}", region.name, region.start, region.size);
    }
    // Show SVD peripherals if present.
    if let Some(dev) = ns_device::device_for_target(device) {
        if !dev.peripherals.is_empty() {
            println!();
            println!("外设 ({}):", dev.peripherals.len());
            for p in dev.sorted_peripherals() {
                println!("  {:<20} 0x{:08x}  {}", p.name, p.base_address, p.description);
                for r in &p.registers {
                    println!(
                        "    {:<24} 0x{:08x}  {} 位  reset=0x{:08x}  {}",
                        r.name, r.address, r.size, r.reset_value, r.description
                    );
                }
            }
        }
    }
    Ok(())
}

fn field_json(f: &ns_device::Field) -> serde_json::Value {
    serde_json::json!({
        "name": f.name,
        "bitOffset": f.bit_offset,
        "bitWidth": f.bit_width,
        "description": f.description,
    })
}

fn register_json(r: &ns_device::Register) -> serde_json::Value {
    serde_json::json!({
        "name": r.name,
        "address": r.address,
        "size": r.size,
        "resetValue": r.reset_value,
        "description": r.description,
        "fields": r.fields.iter().map(field_json).collect::<Vec<_>>(),
    })
}

fn peripheral_json(p: &ns_device::Peripheral) -> serde_json::Value {
    serde_json::json!({
        "name": p.name,
        "baseAddress": p.base_address,
        "description": p.description,
        "registers": p.registers.iter().map(register_json).collect::<Vec<_>>(),
    })
}

fn device_json(d: &ns_device::Device, peripherals: &[&ns_device::Peripheral]) -> serde_json::Value {
    serde_json::json!({
        "name": d.name,
        "vendor": d.vendor,
        "core": d.core,
        "memory": d.memory.iter().map(|m| serde_json::json!({
            "name": m.name,
            "start": m.start,
            "size": m.size,
            "access": m.access,
        })).collect::<Vec<_>>(),
        "peripherals": peripherals.iter().map(|p| peripheral_json(p)).collect::<Vec<_>>(),
    })
}

/// The device code-generation artifact kind.
#[derive(Clone, Copy)]
enum CodegenKind {
    Linker,
    Startup,
}

fn cmd_device_codegen(device: &str, out: Option<&Path>, kind: CodegenKind, json: bool) -> Result<()> {
    if ns_device::catalog_entry(device).is_none() {
        bail!("未知设备: {device}");
    }
    let label = match kind {
        CodegenKind::Linker => "链接脚本",
        CodegenKind::Startup => "启动文件",
    };
    let text = match kind {
        CodegenKind::Linker => ns_device::linker_script(device),
        CodegenKind::Startup => ns_device::startup_file(device),
    }
    .ok_or_else(|| anyhow::anyhow!("无法生成{label}：设备 {device} 不受支持"))?;
    match out {
        Some(path) => {
            std::fs::write(path, &text).with_context(|| format!("写入 {} 失败", path.display()))?;
            if json {
                print_json(&serde_json::json!({
                    "device": device,
                    "kind": match kind { CodegenKind::Linker => "linker", CodegenKind::Startup => "startup" },
                    "file": path.to_string_lossy(),
                    "bytes": text.len(),
                }))?;
            } else {
                println!("已生成{label} -> {}", path.display());
            }
        }
        None => {
            if json {
                print_json(&serde_json::json!({
                    "device": device,
                    "kind": match kind { CodegenKind::Linker => "linker", CodegenKind::Startup => "startup" },
                    "content": text,
                }))?;
            } else {
                print!("{text}");
            }
        }
    }
    Ok(())
}

fn cmd_device_init(
    device: &str,
    peripheral_names: &[String],
    register_overrides: &[String],
    out: Option<&Path>,
    json: bool,
) -> Result<()> {
    if ns_device::catalog_entry(device).is_none() {
        bail!("未知设备: {device}");
    }
    // Resolve which peripherals to configure (default: every SVD peripheral).
    let names: Vec<String> = if peripheral_names.is_empty() {
        let dev = ns_device::device_for_target(device)
            .ok_or_else(|| anyhow::anyhow!("设备 {device} 无内置 SVD，请用 --peripheral 指定"))?;
        dev.sorted_peripherals().iter().map(|p| p.name.clone()).collect()
    } else {
        peripheral_names.to_vec()
    };

    // Parse `PERIPH.REG=VALUE` overrides into per-peripheral register values.
    let mut register_values: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
    for spec in register_overrides {
        let (left, value) = spec
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("--register 格式应为 PERIPH.REG=VALUE: {spec}"))?;
        let (periph, reg) = left
            .split_once('.')
            .ok_or_else(|| anyhow::anyhow!("--register 格式应为 PERIPH.REG=VALUE: {spec}"))?;
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
    let text = ns_device::generate_peripheral_init(device, &configs)
        .ok_or_else(|| anyhow::anyhow!("无法生成外设初始化：设备 {device} 不受支持"))?;
    match out {
        Some(path) => {
            std::fs::write(path, &text).with_context(|| format!("写入 {} 失败", path.display()))?;
            if json {
                print_json(&serde_json::json!({
                    "device": device,
                    "kind": "init",
                    "file": path.to_string_lossy(),
                    "bytes": text.len(),
                }))?;
            } else {
                println!("已生成外设初始化 -> {}", path.display());
            }
        }
        None => {
            if json {
                print_json(&serde_json::json!({
                    "device": device,
                    "kind": "init",
                    "content": text,
                }))?;
            } else {
                print!("{text}");
            }
        }
    }
    Ok(())
}

fn cmd_device_svd(
    file: Option<&Path>,
    device_key: Option<&str>,
    peripheral: Option<&str>,
    register: Option<&str>,
    json: bool,
) -> Result<()> {
    let device = match (file, device_key) {
        (Some(f), None) => ns_device::load_svd_file(f)?,
        (None, Some(k)) => ns_device::device_for_target(k)
            .ok_or_else(|| anyhow::anyhow!("未知设备或缺少内置 SVD: {k}"))?,
        (Some(_), Some(_)) => bail!("--file 与 --device 只能二选一"),
        (None, None) => bail!("请提供 --file 或 --device"),
    };
    let peripherals: Vec<&ns_device::Peripheral> = match peripheral {
        Some(name) => {
            let p = device
                .peripheral(name)
                .ok_or_else(|| anyhow::anyhow!("SVD 中无外设: {name}"))?;
            vec![p]
        }
        None => device.sorted_peripherals(),
    };

    // A register filter requires a peripheral and shows its fields.
    if let Some(reg_name) = register {
        let p = peripherals
            .first()
            .ok_or_else(|| anyhow::anyhow!("--register 需要先指定 --peripheral"))?;
        let reg = p
            .registers
            .iter()
            .find(|r| r.name == reg_name)
            .ok_or_else(|| anyhow::anyhow!("外设 {} 中无寄存器: {reg_name}", p.name))?;
        if json {
            print_json(&register_json(reg))?;
        } else {
            println!("{}:{} @ 0x{:08x} ({} 位)", p.name, reg.name, reg.address, reg.size);
            if !reg.description.is_empty() {
                println!("  描述: {}", reg.description);
            }
            println!("  复位值: 0x{:08x}", reg.reset_value);
            println!("  字段:");
            if reg.fields.is_empty() {
                println!("    (无)");
            }
            for f in &reg.fields {
                println!(
                    "    {:<24} bit {}:{}  {}",
                    f.name,
                    f.bit_offset,
                    f.bit_offset + f.bit_width.saturating_sub(1),
                    f.description
                );
            }
        }
        return Ok(());
    }

    if json {
        print_json(&device_json(&device, &peripherals))?;
    } else {
        println!("设备: {}", device.name);
        println!("厂商: {}", device.vendor);
        println!("核心: {}", device.core);
        if !device.memory.is_empty() {
            println!("内存映射:");
            for m in &device.memory {
                println!("  {:<16} 0x{:012x} size=0x{:x} {}", m.name, m.start, m.size, m.access);
            }
        }
        println!("外设 ({}):", peripherals.len());
        for p in &peripherals {
            println!(
                "  {:<24} 0x{:08x}  {} ({} 寄存器)",
                p.name,
                p.base_address,
                p.description,
                p.registers.len()
            );
            // When a specific peripheral is selected, expand its registers.
            if peripheral.is_some() {
                for r in &p.registers {
                    println!(
                        "    {:<24} 0x{:08x}  {} 位  reset=0x{:08x}  {}",
                        r.name, r.address, r.size, r.reset_value, r.description
                    );
                }
            }
        }
    }
    Ok(())
}

/// Print the memory map of a device (`device memory-map`), optionally
/// locating `--addr` inside one of the regions.
fn cmd_device_memory_map(device: &str, addr: Option<&str>, json: bool) -> Result<()> {
    if ns_device::catalog_entry(device).is_none() {
        bail!("未知设备: {device}");
    }
    let regions = ns_device::memory_map(device);
    let target = addr.map(parse_num).transpose()?;
    if let Some(a) = target {
        if !regions.iter().any(|r| {
            (a as u64) >= r.start && (a as u64) < r.start.saturating_add(r.size)
        }) {
            bail!("地址 0x{a:08x} 不在 {device} 的任何内存区域中");
        }
    }
    if json {
        let items: Vec<_> = regions
            .iter()
            .map(|r| {
                serde_json::json!({
                    "name": r.name,
                    "start": r.start,
                    "size": r.size,
                    "end": r.start.saturating_add(r.size) - 1,
                    "access": r.access,
                })
            })
            .collect();
        print_json(&serde_json::json!({
            "device": device,
            "regions": items,
            "lookup": target,
            "lookupRegion": target.and_then(|a| regions
                .iter()
                .find(|r| (a as u64) >= r.start && (a as u64) < r.start.saturating_add(r.size))
                .map(|r| r.name.clone())),
        }))?;
        return Ok(());
    }
    println!("设备 {device} 内存映射:");
    println!("  {:<14} {:>12} {:>12} {:>12}  {}", "区域", "起始", "结束", "大小", "访问");
    for r in &regions {
        let end = r.start.saturating_add(r.size) - 1;
        println!(
            "  {:<14} 0x{:08x} 0x{:08x} {:>10}  {}",
            r.name, r.start, end, r.size, r.access
        );
    }
    if let Some(a) = target {
        let name = regions
            .iter()
            .find(|r| (a as u64) >= r.start && (a as u64) < r.start.saturating_add(r.size))
            .map(|r| r.name.clone())
            .unwrap_or_default();
        println!("地址 0x{a:08x} → 区域 {name}");
    }
    Ok(())
}

/// Why a bounded run stopped early (beyond a breakpoint/halt/max-steps).
#[derive(Clone, Copy, PartialEq, Eq)]
enum StopReason {
    None,
    Condition,
    Cycles,
    Watchpoint,
}

impl StopReason {
    /// The stable `lastResult` label for a bounded stop, or `None` when the
    /// run ended for an instruction-level reason (advance/breakpoint/halt).
    fn label(self) -> Option<&'static str> {
        match self {
            StopReason::Condition => Some("condition"),
            StopReason::Cycles => Some("cycles"),
            StopReason::Watchpoint => Some("watchpoint"),
            StopReason::None => None,
        }
    }
}

fn at_cycle_bound(sim: &Simulator, max_cycles: Option<u64>) -> bool {
    max_cycles.map_or(false, |mc| sim.cycle_count >= mc)
}

/// Run up to `max_steps` instructions, stopping early when `expr` evaluates
/// true (checked at entry and after each step), when the cycle counter reaches
/// `max_cycles`, or when a memory watchpoint is written.
/// Returns `(result, reason)`.
pub(crate) fn run_until_condition(
    sim: &mut Simulator,
    max_steps: u64,
    expr: &str,
    max_cycles: Option<u64>,
    mut watch: Option<&mut WatchTracker>,
) -> (StepResult, StopReason) {
    if ns_sim::eval_condition(expr, &sim.reg_view(), sim.program_counter(), sim.cycle_count) {
        return (StepResult::Advanced, StopReason::Condition);
    }
    if at_cycle_bound(sim, max_cycles) {
        return (StepResult::Advanced, StopReason::Cycles);
    }
    let mut steps = 0u64;
    while steps < max_steps {
        if let Some(w) = watch.as_deref_mut() {
            w.snapshot(sim);
        }
        let result = sim.step();
        if result != StepResult::Advanced {
            return (result, StopReason::None);
        }
        steps += 1;
        if ns_sim::eval_condition(expr, &sim.reg_view(), sim.program_counter(), sim.cycle_count) {
            return (StepResult::Advanced, StopReason::Condition);
        }
        if at_cycle_bound(sim, max_cycles) {
            return (StepResult::Advanced, StopReason::Cycles);
        }
        if let Some(w) = watch.as_deref_mut() {
            if w.check(sim).is_some() {
                return (StepResult::Advanced, StopReason::Watchpoint);
            }
        }
    }
    (StepResult::Advanced, StopReason::None)
}

/// Run up to `max_steps` instructions, stopping early when the cycle counter
/// reaches `max_cycles` (checked at entry and after each step; meaningful in
/// cycle-accurate mode where one instruction may consume several cycles) or
/// when a memory watchpoint is written.
/// Returns `(result, reason)`.
pub(crate) fn run_until_cycles(
    sim: &mut Simulator,
    max_steps: u64,
    max_cycles: Option<u64>,
    mut watch: Option<&mut WatchTracker>,
) -> (StepResult, StopReason) {
    if at_cycle_bound(sim, max_cycles) {
        return (StepResult::Advanced, StopReason::Cycles);
    }
    let mut steps = 0u64;
    while steps < max_steps {
        if let Some(w) = watch.as_deref_mut() {
            w.snapshot(sim);
        }
        let result = sim.step();
        if result != StepResult::Advanced {
            return (result, StopReason::None);
        }
        steps += 1;
        if at_cycle_bound(sim, max_cycles) {
            return (StepResult::Advanced, StopReason::Cycles);
        }
        if let Some(w) = watch.as_deref_mut() {
            if w.check(sim).is_some() {
                return (StepResult::Advanced, StopReason::Watchpoint);
            }
        }
    }
    (StepResult::Advanced, StopReason::None)
}

/// The session's memory watchpoints plus the byte ranges snapshotted before
/// the instruction currently being executed, so each step can be checked for
/// a write. `None` watchpoints means "no session to watch against", in which
/// case nothing changes and the saved session is left untouched.
struct WatchTracker {
    wps: Option<Vec<session::WatchState>>,
    snap: Vec<Option<Vec<u8>>>,
    /// Address of the write that tripped the most recent step (if any).
    last_hit: Option<u32>,
}

impl WatchTracker {
    /// A tracker with no watchpoints (a freshly created session).
    fn empty() -> Self {
        Self {
            wps: Some(Vec::new()),
            snap: Vec::new(),
            last_hit: None,
        }
    }

    /// Load the watchpoints persisted in `path` (an absent file yields an empty tracker).
    fn from_session(path: Option<&Path>) -> Result<Self> {
        let Some(path) = path else {
            return Ok(Self::empty());
        };
        if !path.exists() {
            return Ok(Self::empty());
        }
        let s = Session::load(path)?;
        Ok(Self {
            wps: Some(s.watchpoints),
            snap: Vec::new(),
            last_hit: None,
        })
    }

    /// Read the watched byte ranges (the state before the next instruction).
    fn snapshot(&mut self, sim: &Simulator) {
        let Some(wps) = &self.wps else {
            return;
        };
        self.snap.clear();
        for wp in wps {
            if !wp.enabled {
                self.snap.push(None);
                continue;
            }
            let mut bytes = Vec::with_capacity(wp.len as usize);
            for i in 0..wp.len {
                bytes.push(read_mem_byte(sim, wp.address.wrapping_add(i)));
            }
            self.snap.push(Some(bytes));
        }
    }

    /// Compare the watched ranges with the snapshot taken before the step.
    /// Returns the address of the first written byte and updates hit counters.
    fn check(&mut self, sim: &Simulator) -> Option<u32> {
        let Some(wps) = self.wps.as_mut() else {
            return None;
        };
        let snap = std::mem::take(&mut self.snap);
        let mut hit_addr = None;
        for (wp, before) in wps.iter_mut().zip(snap) {
            let Some(before) = before else {
                continue;
            };
            if before.is_empty() {
                continue;
            }
            let changed = before
                .iter()
                .enumerate()
                .any(|(i, b)| *b != read_mem_byte(sim, wp.address.wrapping_add(i as u32)));
            if !changed {
                continue;
            }
            wp.hits += 1;
            let hit = wp.hit_count == 0 || wp.hits >= wp.hit_count as u64;
            if hit && hit_addr.is_none() {
                hit_addr = Some(wp.address);
            }
            if hit && wp.once {
                wp.enabled = false;
            }
        }
        self.last_hit = hit_addr;
        hit_addr
    }

    /// The address written by the instruction that tripped the last step.
    fn hit(&self) -> Option<u32> {
        self.last_hit
    }

    /// The current watchpoints, if this tracker came from a session.
    fn state(&self) -> Option<Vec<session::WatchState>> {
        self.wps.clone()
    }
}

/// Persist the post-step session, carrying the watchpoints (and their runtime
/// hit counters) that [`Session::capture`] does not model. A tracker without
/// watchpoints leaves the session exactly as `capture` produced it.
fn save_session_with_watchpoints(
    path: &Path,
    sim: &Simulator,
    fw: Option<FirmwareState>,
    watch: &WatchTracker,
) -> Result<()> {
    let mut session = Session::capture(sim, fw);
    if let Some(wps) = watch.state() {
        session.watchpoints = wps;
    }
    session.save(path)
}

/// Resolve a `--until ADDR` or `--until-symbol NAME` stop target into an
/// absolute address (symbols are matched case-insensitively after firmware
/// load, so the simulator must already hold the symbol table).
fn resolve_until(
    sim: &Simulator,
    until: &Option<String>,
    until_symbol: &Option<String>,
) -> Result<Option<u32>> {
    if let Some(sym) = until_symbol {
        let found = sim
            .symbols
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(sym))
            .ok_or_else(|| anyhow::anyhow!("符号未找到: {sym}"))?;
        Ok(Some(found.address))
    } else {
        until.as_deref().map(parse_num).transpose()
    }
}

fn cmd_run(args: RunArgs) -> Result<()> {
    let base = parse_num(&args.base)?;
    // If a session is given, load it (preserving peripherals, memory,
    // breakpoints and SVD registers); otherwise create a fresh simulator.
    let (mut sim, _existing_fw, _) =
        load_or_create(args.session.as_deref(), args.arch.as_deref(), None, "0")?;
    let firmware = detect_format(&args.firmware);
    let fw_state = Some(FirmwareState {
        path: args.firmware.to_string_lossy().into_owned(),
        base,
        format: firmware.label().to_string(),
    });
    load_firmware_into(&mut sim, &args.firmware, base)?;
    sim.cycle_accurate = args.cycle_accurate;

    // `--until ADDR` installs a temporary breakpoint removed before saving.
    let until_addr = resolve_until(&sim, &args.until, &args.until_symbol)?;
    if let Some(addr) = until_addr {
        if addr > sim.pc_limit() {
            bail!("--until 地址 0x{addr:08x} 超出 {} 的 PC 可寻址范围", sim.arch.label());
        }
    }
    let temp_bp = until_addr.map(|addr| sim.set_breakpoint(addr)).unwrap_or(false);
    // `--steps N` is a bounded "run exactly N instructions" mode; otherwise
    // run to completion with the `--max-steps` upper bound.
    let tracing = args.trace || args.trace_regs;
    let mut watch = WatchTracker::from_session(args.session.as_deref())?;
    let reason;
    let result = if let Some(expr) = &args.until_condition {
        if tracing {
            let (r, rsn) = trace_run(&mut sim, args.steps, args.max_steps, Some(expr), args.trace_regs, args.max_cycles, Some(&mut watch));
            reason = rsn;
            r
        } else {
            let (r, rsn) = run_until_condition(&mut sim, args.max_steps, expr, args.max_cycles, Some(&mut watch));
            reason = rsn;
            r
        }
    } else if tracing {
        let (r, rsn) = trace_run(&mut sim, args.steps, args.max_steps, None, args.trace_regs, args.max_cycles, Some(&mut watch));
        reason = rsn;
        r
    } else {
        let (r, rsn) = run_until_cycles(&mut sim, args.steps.unwrap_or(args.max_steps), args.max_cycles, Some(&mut watch));
        reason = rsn;
        r
    };
    if temp_bp {
        if let Some(addr) = until_addr {
            sim.clear_breakpoint(addr);
        }
    }
    let mut output = state_output(&sim, result, fw_state.as_ref());
    if let Some(label) = reason.label() {
        output.last_result = label.to_string();
    }
    output.watch_hit = if reason == StopReason::Watchpoint { watch.hit() } else { None };

    if let Some(session_path) = args.save_session.as_ref().or_else(|| args.session.as_ref()) {
        save_session_with_watchpoints(session_path, &sim, fw_state, &watch)?;
    }

    if args.json {
        print_json(&output)?;
    } else {
        println!("架构: {}", sim.arch.label());
        println!("PC:   0x{:08x}", output.pc);
        println!("周期: {}", output.cycle_count);
        let src = if args.source {
            source_annotation(&sim, sim.program_counter())
        } else {
            String::new()
        };
        println!("停止: {}{src}", output.last_result);
        if !output.uart.is_empty() {
            println!("UART: {:?}", output.uart_text);
        }
        println!("寄存器:");
        for (name, value) in &output.registers {
            println!("  {name:<8} 0x{value:08x}");
        }
    }
    Ok(())
}

/// Run with per-instruction tracing: print each executed instruction's
/// address, disassembly and source line, then the resulting PC/state. Stops
/// at a breakpoint or halt (or after the step/halt bound).
fn reg_map(sim: &Simulator) -> BTreeMap<String, u32> {
    let count = ns_sim::reg_count_for(sim.arch);
    let mut m = BTreeMap::new();
    for i in 0..count {
        m.insert(ns_sim::reg_label_for(sim.arch, i), sim.read_reg(i));
    }
    m
}

/// A single traced instruction, with the registers that changed as a result.
#[derive(serde::Serialize)]
pub(crate) struct TraceLine {
    pc: u32,
    /// Program counter after executing this instruction.
    next_pc: u32,
    /// Step result label for this instruction (`advanced`/`breakpoint`/`halted`).
    result: String,
    text: String,
    source: String,
    /// Registers whose value changed after executing this instruction.
    regs: BTreeMap<String, u32>,
}

/// Run with per-instruction collection, returning the structured trace plus
/// the reason a `--until-condition`/`--max-cycles` bound stopped the run early.
pub(crate) fn collect_trace_run(
    sim: &mut Simulator,
    steps: Option<u64>,
    max_steps: u64,
    cond: Option<&str>,
    trace_regs: bool,
    max_cycles: Option<u64>,
    mut watch: Option<&mut WatchTracker>,
) -> (StepResult, StopReason, Vec<TraceLine>) {
    let limit = steps.unwrap_or(max_steps);
    let mut count = 0u64;
    let check = |sim: &Simulator| -> bool {
        cond.map(|c| ns_sim::eval_condition(c, &sim.reg_view(), sim.program_counter(), sim.cycle_count))
            .unwrap_or(false)
    };
    let mut lines = Vec::new();
    if check(sim) {
        return (StepResult::Advanced, StopReason::Condition, lines);
    }
    if at_cycle_bound(sim, max_cycles) {
        return (StepResult::Advanced, StopReason::Cycles, lines);
    }
    loop {
        if count >= limit {
            return (StepResult::Advanced, StopReason::None, lines);
        }
        if let Some(w) = watch.as_deref_mut() {
            w.snapshot(sim);
        }
        let pc_before = sim.program_counter();
        let regs_before = if trace_regs { reg_map(sim) } else { BTreeMap::new() };
        let result = sim.step();
        let insn = ns_sim::disassemble_one_state(
            sim.arch,
            pc_before,
            &|addr| read_code_byte(sim, addr),
            sim.arm_state(),
        );
        let src = source_annotation(sim, pc_before);
        let mut changed = BTreeMap::new();
        if trace_regs {
            let after = reg_map(sim);
            for (k, v) in &after {
                if regs_before.get(k) != Some(v) {
                    changed.insert(k.clone(), *v);
                }
            }
        }
        lines.push(TraceLine {
            pc: pc_before,
            next_pc: sim.program_counter(),
            result: fmt_step_result(result).to_string(),
            text: insn,
            source: src,
            regs: changed,
        });
        count += 1;
        if result != StepResult::Advanced {
            return (result, StopReason::None, lines);
        }
        if check(sim) {
            return (StepResult::Advanced, StopReason::Condition, lines);
        }
        if at_cycle_bound(sim, max_cycles) {
            return (StepResult::Advanced, StopReason::Cycles, lines);
        }
        if let Some(w) = watch.as_deref_mut() {
            if w.check(sim).is_some() {
                return (StepResult::Advanced, StopReason::Watchpoint, lines);
            }
        }
    }
}

fn trace_run(
    sim: &mut Simulator,
    steps: Option<u64>,
    max_steps: u64,
    cond: Option<&str>,
    trace_regs: bool,
    max_cycles: Option<u64>,
    watch: Option<&mut WatchTracker>,
) -> (StepResult, StopReason) {
    let (result, reason, lines) =
        collect_trace_run(sim, steps, max_steps, cond, trace_regs, max_cycles, watch);
    for (i, line) in lines.iter().enumerate() {
        let regs_changed = line
            .regs
            .iter()
            .map(|(k, v)| format!(" {k}=0x{v:08x}"))
            .collect::<String>();
        println!(
            "[{i}] 0x{:08x}: {}{}  -> 0x{:08x} {}{}",
            line.pc,
            line.text,
            if line.source.is_empty() { String::new() } else { line.source.clone() },
            line.next_pc,
            line.result,
            regs_changed,
        );
    }
    (result, reason)
}

enum StepKind {
    Step,
    StepOver,
    StepOut,
}

fn cmd_step(args: StepArgs, kind: StepKind) -> Result<()> {
    let base = parse_num(&args.base)?;
    let (mut sim, fw, session_path) = load_or_create(
        args.session.as_deref(),
        args.arch.as_deref(),
        args.firmware.as_deref(),
        &args.base,
    )?;
    let mut watch = WatchTracker::from_session(session_path.as_deref())?;
    // Snapshot the watched ranges before executing so the step can be checked
    // for a write (covers step-over/step-out, which step internally).
    watch.snapshot(&sim);
    let result = match kind {
        StepKind::Step => sim.step(),
        StepKind::StepOver => sim.step_over(args.max_steps),
        StepKind::StepOut => sim.step_out(args.max_steps),
    };
    let watch_hit = watch.check(&sim);
    let mut output = state_output(&sim, result, fw.as_ref());
    if watch_hit.is_some() {
        output.last_result = "watchpoint".to_string();
    }
    output.watch_hit = watch_hit;
    if let Some(sp) = &session_path {
        save_session_with_watchpoints(sp, &sim, fw, &watch)?;
    }
    if args.json {
        print_json(&output)?;
    } else {
        let src = if args.source {
            source_annotation(&sim, sim.program_counter())
        } else {
            String::new()
        };
        println!(
            "PC=0x{:08x} cycles={} halted={} result={}{src}",
            output.pc, output.cycle_count, output.halted, output.last_result
        );
    }
    let _ = base;
    Ok(())
}

fn cmd_continue(args: ContinueArgs) -> Result<()> {
    let (mut sim, fw, _) = load_or_create(Some(&args.session), None, None, "0")?;
    // `--until ADDR` installs a temporary breakpoint that is removed before
    // the session is saved (unless the address was already a breakpoint).
    let until_addr = resolve_until(&sim, &args.until, &args.until_symbol)?;
    if let Some(addr) = until_addr {
        if addr > sim.pc_limit() {
            bail!("--until 地址 0x{addr:08x} 超出 {} 的 PC 可寻址范围", sim.arch.label());
        }
    }
    let temp_bp = until_addr.map(|addr| sim.set_breakpoint(addr)).unwrap_or(false);
    let tracing = args.trace || args.trace_regs;
    let mut watch = WatchTracker::from_session(Some(&args.session))?;
    let reason;
    let result = if let Some(expr) = &args.until_condition {
        if tracing {
            let (r, rsn) = trace_run(&mut sim, None, args.max_steps, Some(expr), args.trace_regs, args.max_cycles, Some(&mut watch));
            reason = rsn;
            r
        } else {
            let (r, rsn) = run_until_condition(&mut sim, args.max_steps, expr, args.max_cycles, Some(&mut watch));
            reason = rsn;
            r
        }
    } else if tracing {
        let (r, rsn) = trace_run(&mut sim, None, args.max_steps, None, args.trace_regs, args.max_cycles, Some(&mut watch));
        reason = rsn;
        r
    } else {
        let (r, rsn) = run_until_cycles(&mut sim, args.max_steps, args.max_cycles, Some(&mut watch));
        reason = rsn;
        r
    };
    if temp_bp {
        if let Some(addr) = until_addr {
            sim.clear_breakpoint(addr);
        }
    }
    let mut output = state_output(&sim, result, fw.as_ref());
    if let Some(label) = reason.label() {
        output.last_result = label.to_string();
    }
    output.watch_hit = if reason == StopReason::Watchpoint { watch.hit() } else { None };
    if args.save {
        save_session_with_watchpoints(&args.session, &sim, fw, &watch)?;
    }
    if args.json {
        print_json(&output)?;
    } else {
        let src = if args.source {
            source_annotation(&sim, sim.program_counter())
        } else {
            String::new()
        };
        println!("PC=0x{:08x} cycles={} halted={} result={}{src}", output.pc, output.cycle_count, output.halted, output.last_result);
    }
    Ok(())
}

fn cmd_reg(sub: RegSub) -> Result<()> {
    match sub {
        RegSub::List { session, arch, filter, json } => {
            let arch = resolve_arch(session.as_deref(), arch.as_deref())?;
            let mut regs = BTreeMap::new();
            let count = ns_sim::reg_count_for(arch);
            for i in 0..count {
                regs.insert(ns_sim::reg_label_for(arch, i), 0u32);
            }
            if let Some(sp) = session {
                if sp.exists() {
                    let s = Session::load(&sp)?;
                    for (i, v) in &s.registers {
                        regs.insert(ns_sim::reg_label_for(arch, *i), *v);
                    }
                }
            }
            let filter = filter.map(|f| f.to_ascii_lowercase());
            if let Some(f) = &filter {
                regs.retain(|name, _| name.to_ascii_lowercase().contains(f));
            }
            if json {
                print_json(&regs)?;
            } else {
                for (name, value) in &regs {
                    println!("{name:<8} 0x{value:08x}");
                }
            }
            Ok(())
        }
        RegSub::Get { session, arch, name, name_flag, json } => {
            let name = name_flag.or(name).unwrap_or_else(|| "".to_string());
            let arch = resolve_arch(session.as_deref(), arch.as_deref())?;
            let map = session::register_index_by_name(arch);
            let idx = *map.get(&name.to_ascii_lowercase())
                .ok_or_else(|| anyhow::anyhow!("未知寄存器: {name}"))?;
            let value = if let Some(sp) = session {
                if sp.exists() {
                    let s = Session::load(&sp)?;
                    s.registers.iter().find(|(i, _)| *i == idx).map(|(_, v)| *v).unwrap_or(0)
                } else {
                    0
                }
            } else {
                0
            };
            if json {
                print_json(&serde_json::json!({ "name": name, "index": idx, "value": value }))?;
            } else {
                println!("{} = 0x{value:08x}", name);
            }
            Ok(())
        }
        RegSub::Set { session, name, value, name_flag, value_flag, json } => {
            let name = name_flag.or(name).ok_or_else(|| anyhow::anyhow!("缺少寄存器名"))?;
            let value = value_flag.or(value).ok_or_else(|| anyhow::anyhow!("缺少寄存器值"))?;
            let (mut sim, fw, _) = load_or_create(Some(&session), None, None, "0")?;
            let map = session::register_index_by_name(sim.arch);
            let idx = *map.get(&name.to_ascii_lowercase())
                .ok_or_else(|| anyhow::anyhow!("未知寄存器: {name}"))?;
            let val = parse_num(&value)?;
            sim.write_reg(idx, val);
            let session_out = Session::capture(&sim, fw);
            session_out.save(&session)?;
            if json {
                print_json(&serde_json::json!({ "name": name, "index": idx, "value": val }))?;
            } else {
                println!("{} = 0x{val:08x}", name);
            }
            Ok(())
        }
    }
}

fn cmd_mem(sub: MemSub) -> Result<()> {
    match sub {
        MemSub::Read { session, arch, addr, addr_flag, len, width, ascii, string, out, json } => {
            let addr = addr_flag.or(addr).ok_or_else(|| anyhow::anyhow!("缺少地址"))?;
            let addr_num = parse_num(&addr)?;
            let (sim, _fw, _) = load_or_create(Some(&session), arch.as_deref(), None, "0")?;
            if (ascii || string) && out.is_some() {
                bail!("--ascii/--string 与 --out 互斥");
            }
            let elem_size = match width.as_str() {
                "byte" => 1usize,
                "half" => 2usize,
                "word" => 4usize,
                "dword" => 8usize,
                other => bail!("不支持的内存宽度: {other}（支持 byte/half/word/dword）"),
            };
            if (ascii || string) && elem_size != 1 {
                bail!("--ascii/--string 要求 --width byte");
            }
            let mut bytes = Vec::with_capacity(len * elem_size);
            for i in 0..len {
                let a = addr_num.wrapping_add((i * elem_size) as u32);
                for j in 0..elem_size {
                    bytes.push(read_mem_byte(&sim, a.wrapping_add(j as u32)));
                }
            }
            if ascii || string {
                let text: String = bytes
                    .iter()
                    .map(|&b| {
                        if b == 0 {
                            '\0'
                        } else if b.is_ascii_graphic() || b == b' ' {
                            b as char
                        } else {
                            '.'
                        }
                    })
                    .collect();
                let text = if string {
                    text.split('\0').next().unwrap_or("").to_string()
                } else {
                    text.trim_end_matches('\0').to_string()
                };
                if json {
                    print_json(&serde_json::json!({
                        "address": addr_num,
                        "len": len,
                        "width": width,
                        "bytes": bytes,
                        "text": text,
                        "ascii": ascii,
                        "string": string,
                    }))?;
                } else {
                    println!("{text}");
                }
                return Ok(());
            }
            if let Some(out_path) = out {
                std::fs::write(&out_path, &bytes)
                    .with_context(|| format!("写入 {} 失败", out_path.display()))?;
                println!("已写入 {} 字节到 {}", bytes.len(), out_path.display());
            } else if json {
                print_json(&serde_json::json!({
                    "address": addr_num,
                    "len": len,
                    "width": width,
                    "bytes": bytes,
                }))?;
            } else if elem_size > 1 {
                // Print one value per element as a hex integer (little-endian).
                for i in 0..len {
                    let mut v: u64 = 0;
                    for j in 0..elem_size {
                        v |= (bytes[i * elem_size + j] as u64) << (8 * j);
                    }
                    let a = addr_num.wrapping_add((i * elem_size) as u32);
                    let digits = elem_size * 2;
                    println!("0x{a:08x}: 0x{v:0digits$x}");
                }
            } else {
                print!("{}", hex_dump(addr_num, &bytes));
            }
            Ok(())
        }
        MemSub::Write { session, arch, addr, addr_flag, bytes, width, values, file, string, null, json } => {
            let addr = addr_flag.or(addr).ok_or_else(|| anyhow::anyhow!("缺少地址"))?;
            let addr_num = parse_num(&addr)?;
            let mut data = Vec::new();
            let elem_size = match width.as_str() {
                "byte" => 1usize,
                "half" => 2usize,
                "word" => 4usize,
                other => bail!("不支持的内存宽度: {other}（支持 byte/half/word）"),
            };
            if let Some(s) = &string {
                if elem_size != 1 {
                    bail!("--string 要求 --width byte");
                }
                data.extend_from_slice(s.as_bytes());
                if null {
                    data.push(0);
                }
            } else if let Some(f) = &file {
                data = std::fs::read(&f).with_context(|| format!("读取 {} 失败", f.display()))?;
            } else if elem_size > 1 {
                for v in &values {
                    let n = parse_num(v)? as u64;
                    let max = (1u64 << (elem_size * 8)) - 1;
                    if n > max {
                        bail!("值超出 {} 位: {v}", elem_size * 8);
                    }
                    for j in 0..elem_size {
                        data.push(((n >> (8 * j)) & 0xff) as u8);
                    }
                }
            } else {
                for b in &bytes {
                    let v = parse_num(b)?;
                    if v > 0xff {
                        bail!("字节超出 0..=255: {}", b);
                    }
                    data.push(v as u8);
                }
            }
            let (mut sim, fw, _) = load_or_create(Some(&session), arch.as_deref(), None, "0")?;
            for (i, b) in data.iter().enumerate() {
                let a = addr_num.wrapping_add(i as u32);
                write_mem_byte(&mut sim, a, *b);
            }
            let s = Session::capture(&sim, fw);
            s.save(&session)?;
            if json {
                print_json(&serde_json::json!({
                    "address": addr_num,
                    "len": data.len(),
                    "width": width,
                }))?;
            } else {
                println!("写入 {} 字节 @ 0x{:08x}", data.len(), addr_num);
            }
            Ok(())
        }
        MemSub::Fill { session, arch, addr, addr_flag, len, value, value_flag, random, seed, pattern, json } => {
            let addr = addr_flag.or(addr).ok_or_else(|| anyhow::anyhow!("缺少地址"))?;
            let addr_num = parse_num(&addr)?;
            let (mut sim, fw, _) = load_or_create(Some(&session), arch.as_deref(), None, "0")?;
            let parsed_pat: Option<Vec<u8>> = if pattern.is_empty() {
                None
            } else {
                let mut pat = Vec::new();
                for p in &pattern {
                    let n = parse_num(p)?;
                    if n > 0xff {
                        bail!("模式字节超出 0..=255: {p}");
                    }
                    pat.push(n as u8);
                }
                Some(pat)
            };
            let (bytes, value_byte, seed_used): (Vec<u8>, u8, Option<u64>) = if let Some(pat) = &parsed_pat {
                let mut bytes = Vec::with_capacity(len);
                for i in 0..len {
                    bytes.push(pat[i % pat.len()]);
                }
                (bytes, 0, None)
            } else if random {
                let seed_input = seed.as_deref().map(parse_num).transpose()?.unwrap_or(0x9E37_79B9) as u64;
                let mut st = seed_input;
                let bytes = (0..len)
                    .map(|_| {
                        st ^= st << 13;
                        st ^= st >> 7;
                        st ^= st << 17;
                        (st >> 24) as u8
                    })
                    .collect();
                (bytes, 0, Some(seed_input))
            } else {
                let value = value_flag.or(value).ok_or_else(|| anyhow::anyhow!("缺少填充字节"))?;
                let value_num = parse_num(&value)?;
                if value_num > 0xff {
                    bail!("填充字节超出 0..=255: {}", value);
                }
                (vec![value_num as u8; len], value_num as u8, None)
            };
            for (i, b) in bytes.iter().enumerate() {
                let a = addr_num.wrapping_add(i as u32);
                write_mem_byte(&mut sim, a, *b);
            }
            let s = Session::capture(&sim, fw);
            s.save(&session)?;
            if json {
                print_json(&serde_json::json!({
                    "address": addr_num,
                    "len": len,
                    "value": value_byte,
                    "random": random,
                    "seed": seed_used,
                    "pattern": pattern,
                }))?;
            } else {
                if random {
                    println!(
                        "填充 {} 字节 @ 0x{:08x}（伪随机，seed {}）",
                        len,
                        addr_num,
                        seed_used.map(|s| format!("{s:#x}")).unwrap_or_default()
                    );
                } else if !pattern.is_empty() {
                    let pat_hex: Vec<String> = parsed_pat
                        .as_ref()
                        .unwrap()
                        .iter()
                        .map(|b| format!("0x{b:02x}"))
                        .collect();
                    println!(
                        "填充 {} 字节 @ 0x{:08x} = 模式 {}",
                        len,
                        addr_num,
                        pat_hex.join(" ")
                    );
                } else {
                    println!("填充 {} 字节 @ 0x{:08x} = 0x{value_byte:02x}", len, addr_num);
                }
            }
            Ok(())
        }
        MemSub::Search { session, arch, addr, addr_flag, len, pattern, width, out, json } => {
            let addr_str = addr_flag
                .as_deref()
                .or(addr.as_deref())
                .ok_or_else(|| anyhow::anyhow!("缺少地址"))?;
            let addr_num = parse_num(addr_str)?;
            if pattern.is_empty() {
                bail!("--pattern 至少需要一个字节");
            }
            let pat = expand_search_pattern(&pattern, &width)?;
            let (sim, _, _) = load_or_create(Some(&session), arch.as_deref(), None, "0")?;
            let mut matches: Vec<u32> = Vec::new();
            if pat.is_empty() || len < pat.len() {
                // Nothing to search.
            } else {
                let window = pat.len();
                for i in 0..=(len - window) {
                    let mut ok = true;
                    for (j, b) in pat.iter().enumerate() {
                        if read_mem_byte(&sim, addr_num.wrapping_add((i + j) as u32)) != *b {
                            ok = false;
                            break;
                        }
                    }
                    if ok {
                        matches.push(addr_num.wrapping_add(i as u32));
                    }
                }
            }
            let text = if json {
                let value = serde_json::json!({
                    "address": addr_num,
                    "width": width,
                    "patternTokens": pattern.len(),
                    "count": matches.len(),
                    "matches": matches,
                });
                serde_json::to_string_pretty(&value)?
            } else {
                let mut s = String::new();
                if matches.is_empty() {
                    s.push_str("(未找到模式)\n");
                }
                for m in &matches {
                    s.push_str(&format!("0x{m:08x}\n"));
                }
                s
            };
            if let Some(out_path) = out {
                std::fs::write(&out_path, text.as_bytes())
                    .with_context(|| format!("写入 {} 失败", out_path.display()))?;
                println!("已搜索 {} 字节，找到 {} 处匹配，结果写入 {}", len, matches.len(), out_path.display());
            } else {
                print!("{text}");
            }
            Ok(())
        }
        MemSub::Load { session, firmware, arch, base, json } => {
            let base_num = parse_num(&base)?;
            let (mut sim, _fw, _) = load_or_create(Some(&session), arch.as_deref(), None, "0")?;
            load_firmware_into(&mut sim, &firmware, base_num)?;
            let fw = Some(FirmwareState {
                path: firmware.to_string_lossy().into_owned(),
                base: base_num,
                format: detect_format(&firmware).label().to_string(),
            });
            let s = Session::capture(&sim, fw);
            s.save(&session)?;
            if json {
                print_json(&serde_json::json!({
                    "firmware": firmware.to_string_lossy(),
                    "base": base_num,
                    "format": detect_format(&firmware).label(),
                }))?;
            } else {
                println!("已加载 {} 到仿真器", firmware.display());
            }
            Ok(())
        }
        MemSub::Dump { session, arch, addr, addr_flag, len, name, format, out, json } => {
            let addr = addr_flag.or(addr).ok_or_else(|| anyhow::anyhow!("缺少地址"))?;
            let addr_num = parse_num(&addr)?;
            let (sim, _fw, _) = load_or_create(Some(&session), arch.as_deref(), None, "0")?;
            let mut bytes = Vec::with_capacity(len);
            for i in 0..len {
                bytes.push(read_mem_byte(&sim, addr_num.wrapping_add(i as u32)));
            }
            if json {
                print_json(&serde_json::json!({
                    "address": addr_num,
                    "len": len,
                    "format": format,
                    "name": name,
                    "bytes": bytes,
                }))?;
                return Ok(());
            }
            let text = match format.as_str() {
                "c" => {
                    let mut s = format!("const unsigned char {name}[{len}] = {{\n");
                    for chunk in bytes.chunks(16) {
                        s.push_str("    ");
                        for (i, b) in chunk.iter().enumerate() {
                            if i > 0 {
                                s.push_str(", ");
                            }
                            s.push_str(&format!("0x{b:02x}"));
                        }
                        s.push_str(",\n");
                    }
                    s.push_str("};\n");
                    s
                }
                "hex" => encode_intel_hex(addr_num, &bytes),
                "dump" => hex_dump(addr_num, &bytes),
                other => bail!("不支持的内存导出格式: {other}（支持 c/hex/dump）"),
            };
            if let Some(out_path) = out {
                std::fs::write(&out_path, text.as_bytes())
                    .with_context(|| format!("写入 {} 失败", out_path.display()))?;
                println!("已导出 {} 字节到 {}", bytes.len(), out_path.display());
            } else {
                print!("{text}");
            }
            Ok(())
        }
        MemSub::Region { session, arch, device, region, offset, len, json } => {
            let regions = ns_device::memory_map(&device);
            let reg = regions
                .iter()
                .find(|r| r.name.eq_ignore_ascii_case(&region))
                .ok_or_else(|| {
                    let names: Vec<&str> = regions.iter().map(|r| r.name.as_str()).collect();
                    anyhow::anyhow!("未知区域 {region}（可用: {}）", names.join("/"))
                })?;
            if offset as u64 >= reg.size {
                bail!("偏移 {offset} 超出区域 {} 大小 {}", reg.name, reg.size);
            }
            let max = (reg.size - offset as u64) as usize;
            let count = len.unwrap_or(max).min(max).min(64 * 1024);
            let (sim, _, _) = load_or_create(Some(&session), arch.as_deref(), None, "0")?;
            let start = (reg.start as u32).wrapping_add(offset as u32);
            let mut bytes = Vec::with_capacity(count);
            for i in 0..count {
                bytes.push(read_mem_byte(&sim, start.wrapping_add(i as u32)));
            }
            if json {
                print_json(&serde_json::json!({
                    "device": device,
                    "region": reg.name,
                    "start": start,
                    "size": reg.size,
                    "offset": offset,
                    "len": count,
                    "bytes": bytes,
                }))?;
            } else {
                println!(
                    "{} @ 0x{:08x}（区域 {}，大小 {} 字节，偏移 {}）",
                    reg.name, start, reg.name, reg.size, offset
                );
                print!("{}", hex_dump(start, &bytes));
            }
            Ok(())
        }
        MemSub::Copy { session, arch, src, src_flag, dst, dst_flag, len, json } => {
            let src = src_flag.or(src).ok_or_else(|| anyhow::anyhow!("缺少源地址"))?;
            let dst = dst_flag.or(dst).ok_or_else(|| anyhow::anyhow!("缺少目标地址"))?;
            let src = parse_num(&src)?;
            let dst = parse_num(&dst)?;
            let (mut sim, fw, _) = load_or_create(Some(&session), arch.as_deref(), None, "0")?;
            // Read the whole source range first so overlapping src/dst ranges
            // behave like `memmove` (no clobber before read).
            let mut bytes = Vec::with_capacity(len);
            for i in 0..len {
                bytes.push(read_mem_byte(&sim, src.wrapping_add(i as u32)));
            }
            for (i, b) in bytes.iter().enumerate() {
                write_mem_byte(&mut sim, dst.wrapping_add(i as u32), *b);
            }
            let s = Session::capture(&sim, fw);
            s.save(&session)?;
            if json {
                print_json(&serde_json::json!({ "src": src, "dst": dst, "len": len }))?;
            } else {
                println!("复制 {} 字节 0x{:08x} -> 0x{:08x}", len, src, dst);
            }
            Ok(())
        }

    }
}

fn cmd_bp(sub: BpSub) -> Result<()> {
    match sub {
        BpSub::List { session, symbols, json } => {
            let s = Session::load(&session)?;
            let sim = s.apply(None).ok();
            let sym_for = |addr: u32| -> Option<String> {
                if !symbols {
                    return None;
                }
                sim.as_ref()?
                    .symbols
                    .iter()
                    .find(|x| x.address == addr)
                    .map(|x| x.name.clone())
            };
            if json {
                let items: Vec<_> = s
                    .breakpoints
                    .iter()
                    .map(|bp| {
                        serde_json::json!({
                            "address": bp.address,
                            "condition": bp.condition,
                            "hitCount": bp.hit_count,
                            "once": bp.once,
                            "enabled": bp.enabled,
                            "hits": sim.as_ref().map(|sim| sim.breakpoint_hits(bp.address)).unwrap_or(0),
                            "symbol": sym_for(bp.address),
                        })
                    })
                    .collect();
                print_json(&items)?;
            } else {
                if s.breakpoints.is_empty() {
                    println!("(无断点)");
                }
                for bp in &s.breakpoints {
                    let mut parts = Vec::new();
                    if bp.once {
                        parts.push("once".to_string());
                    }
                    if !bp.condition.is_empty() {
                        parts.push(format!("cond='{}'", bp.condition));
                    }
                    if bp.hit_count > 0 {
                        parts.push(format!("hit={}", bp.hit_count));
                    }
                    if !bp.enabled {
                        parts.push("disabled".to_string());
                    }
                    let extra = if parts.is_empty() {
                        String::new()
                    } else {
                        format!("  {}", parts.join(" "))
                    };
                    let hits = sim
                        .as_ref()
                        .map(|sim| sim.breakpoint_hits(bp.address))
                        .unwrap_or(0);
                    let sym = sym_for(bp.address)
                        .map(|n| format!("  {n}"))
                        .unwrap_or_default();
                    println!("0x{:08x}{extra}  hits={hits}{sym}", bp.address);
                }
            }
            Ok(())
        }
        BpSub::Set { session, addr, addr_flag, symbol, line, condition, hit_count, once, json } => {
            let (mut sim, fw, _) = load_or_create(Some(&session), None, None, "0")?;
            let addr_num = if let Some(sym) = &symbol {
                let found = sim
                    .symbols
                    .iter()
                    .find(|s| s.name.eq_ignore_ascii_case(sym))
                    .ok_or_else(|| anyhow::anyhow!("符号未找到: {sym}"))?;
                found.address
            } else if let Some(line_spec) = &line {
                let (file_part, line_part) = line_spec
                    .rsplit_once(':')
                    .ok_or_else(|| anyhow::anyhow!("行号格式应为 file:line，如 main.c:10"))?;
                let line_num: u32 = line_part
                    .parse()
                    .map_err(|_| anyhow::anyhow!("非法行号: {line_part}"))?;
                let file_idx = sim
                    .source_files
                    .iter()
                    .position(|f| {
                        f == file_part
                            || std::path::Path::new(f)
                                .file_name()
                                .and_then(|n| n.to_str())
                                .map(|n| n == file_part)
                                .unwrap_or(false)
                    })
                    .ok_or_else(|| anyhow::anyhow!("源文件未找到: {file_part}"))?;
                sim.address_for_line(file_idx, line_num)
                    .ok_or_else(|| anyhow::anyhow!("行 {line_part} 无对应指令地址"))?
            } else {
                let addr = addr_flag
                    .or(addr)
                    .ok_or_else(|| anyhow::anyhow!("缺少地址"))?;
                parse_num(&addr)?
            };
            if addr_num > sim.pc_limit() {
                bail!(
                    "断点地址 0x{addr_num:08x} 超出 {} 的 PC 可寻址范围（0x{:08x}）",
                    sim.arch.label(),
                    sim.pc_limit()
                );
            }
            let cond = condition.unwrap_or_default();
            if once {
                sim.set_breakpoint_ex_once(addr_num, cond.clone(), hit_count);
            } else {
                sim.set_breakpoint_ex(addr_num, cond.clone(), hit_count);
            }
            let s = Session::capture(&sim, fw);
            s.save(&session)?;
            let once = once;
            if json {
                print_json(&serde_json::json!({
                    "address": addr_num,
                    "condition": cond,
                    "hitCount": hit_count,
                    "once": once,
                    "enabled": true,
                }))?;
            } else if once {
                println!("一次性断点已设置 @ 0x{:08x}", addr_num);
            } else {
                println!("断点已设置 @ 0x{:08x}", addr_num);
            }
            Ok(())
        }
        BpSub::Modify { session, addr, addr_flag, symbol, condition, hit_count, json } => {
            if condition.is_none() && hit_count.is_none() {
                bail!("至少需要 --condition 或 --hit-count 之一");
            }
            let (mut sim, fw, _) = load_or_create(Some(&session), None, None, "0")?;
            let addr_num = if let Some(sym) = &symbol {
                sim.symbols
                    .iter()
                    .find(|s| s.name.eq_ignore_ascii_case(sym))
                    .ok_or_else(|| anyhow::anyhow!("符号未找到: {sym}"))?
                    .address
            } else {
                let a = addr_flag
                    .or(addr)
                    .ok_or_else(|| anyhow::anyhow!("缺少地址"))?;
                parse_num(&a)?
            };
            let (cond, hits) = sim
                .breakpoints
                .iter()
                .find(|bp| bp.address == addr_num)
                .map(|bp| (bp.condition.clone(), bp.hit_count))
                .ok_or_else(|| anyhow::anyhow!("断点不存在 @ 0x{addr_num:08x}"))?;
            let cond = condition.unwrap_or(cond);
            let hits = hit_count.unwrap_or(hits);
            sim.update_breakpoint(addr_num, cond.clone(), hits);
            let s = Session::capture(&sim, fw);
            s.save(&session)?;
            let live = sim
                .breakpoints
                .iter()
                .find(|bp| bp.address == addr_num)
                .unwrap();
            if json {
                print_json(&serde_json::json!({
                    "address": addr_num,
                    "condition": live.condition,
                    "hitCount": live.hit_count,
                    "once": live.once,
                    "enabled": live.enabled,
                    "hits": sim.breakpoint_hits(addr_num),
                }))?;
            } else {
                println!(
                    "断点已更新 @ 0x{addr_num:08x}（条件 '{}'，命中次数 {}，历史命中 {}）",
                    live.condition,
                    live.hit_count,
                    sim.breakpoint_hits(addr_num)
                );
            }
            Ok(())
        }
        BpSub::Clear { session, addr, addr_flag, json } => {
            let addr = addr_flag.or(addr).ok_or_else(|| anyhow::anyhow!("缺少地址"))?;
            let addr_num = parse_num(&addr)?;
            let (mut sim, fw, _) = load_or_create(Some(&session), None, None, "0")?;
            let removed = sim.clear_breakpoint(addr_num);
            let s = Session::capture(&sim, fw);
            s.save(&session)?;
            if json {
                print_json(&serde_json::json!({
                    "address": addr_num,
                    "removed": removed,
                }))?;
            } else {
                println!("断点{}已清除 @ 0x{:08x}", if removed { "" } else { " 未找到，" }, addr_num);
            }
            Ok(())
        }
        BpSub::ClearAll { session, json } => {
            let (mut sim, fw, _) = load_or_create(Some(&session), None, None, "0")?;
            let count = sim.breakpoints.len();
            sim.breakpoints.clear();
            let s = Session::capture(&sim, fw);
            s.save(&session)?;
            if json {
                print_json(&serde_json::json!({ "cleared": count }))?;
            } else {
                println!("已清除全部断点");
            }
            Ok(())
        }
        BpSub::Enable { session, addr, addr_flag, json } => {
            let addr = addr_flag.or(addr).ok_or_else(|| anyhow::anyhow!("缺少地址"))?;
            let addr_num = parse_num(&addr)?;
            let (mut sim, fw, _) = load_or_create(Some(&session), None, None, "0")?;
            let found = sim.set_breakpoint_enabled(addr_num, true);
            if !found {
                bail!("断点不存在 @ 0x{:08x}", addr_num);
            }
            let s = Session::capture(&sim, fw);
            s.save(&session)?;
            if json {
                print_json(&serde_json::json!({ "address": addr_num, "enabled": true }))?;
            } else {
                println!("已启用断点 @ 0x{:08x}", addr_num);
            }
            Ok(())
        }
        BpSub::Disable { session, addr, addr_flag, json } => {
            let addr = addr_flag.or(addr).ok_or_else(|| anyhow::anyhow!("缺少地址"))?;
            let addr_num = parse_num(&addr)?;
            let (mut sim, fw, _) = load_or_create(Some(&session), None, None, "0")?;
            let found = sim.set_breakpoint_enabled(addr_num, false);
            if !found {
                bail!("断点不存在 @ 0x{:08x}", addr_num);
            }
            let s = Session::capture(&sim, fw);
            s.save(&session)?;
            if json {
                print_json(&serde_json::json!({ "address": addr_num, "enabled": false }))?;
            } else {
                println!("已禁用断点 @ 0x{:08x}", addr_num);
            }
            Ok(())
        }
    }
}

/// Format a `; file:line` source annotation for `address`, using the DWARF
/// line table and source files attached to the simulator (empty when none).
fn watch_json(wp: &session::WatchState) -> serde_json::Value {
    serde_json::json!({
        "address": wp.address,
        "len": wp.len,
        "hitCount": wp.hit_count,
        "once": wp.once,
        "enabled": wp.enabled,
        "hits": wp.hits,
    })
}

/// Find a watchpoint by address, reporting a stable error when absent.
fn watch_find(s: &mut Session, addr: u32) -> Result<&mut session::WatchState> {
    s.watchpoints
        .iter_mut()
        .find(|w| w.address == addr)
        .ok_or_else(|| anyhow::anyhow!("观察点不存在 @ 0x{addr:08x}"))
}

fn cmd_watch(sub: WatchSub) -> Result<()> {
    match sub {
        WatchSub::Set {
            session,
            addr,
            addr_flag,
            len,
            hit_count,
            once,
            json,
        } => {
            let addr_num = parse_num(
                &addr_flag.or(addr).ok_or_else(|| anyhow::anyhow!("缺少地址"))?,
            )?;
            if len == 0 || len > 64 {
                bail!("--len 必须在 1..64");
            }
            let mut s = Session::load(&session)?;
            // Replacing a watchpoint at the same address starts its hit count over.
            s.watchpoints.retain(|w| w.address != addr_num);
            let wp = session::WatchState {
                address: addr_num,
                len,
                hit_count,
                once,
                enabled: true,
                hits: 0,
            };
            s.watchpoints.push(wp.clone());
            s.save(&session)?;
            let mut out = watch_json(&wp);
            out["count"] = serde_json::json!(s.watchpoints.len());
            if json {
                print_json(&out)?;
            } else {
                println!(
                    "已设置观察点 @ 0x{addr_num:08x}（{len} 字节，共 {} 个）",
                    s.watchpoints.len()
                );
            }
            Ok(())
        }
        WatchSub::List { session, json } => {
            let wps = if session.exists() {
                Session::load(&session)?.watchpoints
            } else {
                Vec::new()
            };
            if json {
                let items: Vec<_> = wps.iter().map(watch_json).collect();
                print_json(&items)?;
            } else if wps.is_empty() {
                println!("(无观察点)");
            } else {
                for w in &wps {
                    let state = if w.enabled { "enabled" } else { "disabled" };
                    println!(
                        "0x{a:08x}（{len} 字节）  {state}  hits={hits}{}",
                        if w.once { "  once" } else { "" },
                        a = w.address,
                        len = w.len,
                        hits = w.hits,
                    );
                }
            }
            Ok(())
        }
        WatchSub::Clear { session, addr, addr_flag, json } => {
            let addr_num = parse_num(
                &addr_flag.or(addr).ok_or_else(|| anyhow::anyhow!("缺少地址"))?,
            )?;
            let mut s = Session::load(&session)?;
            let before = s.watchpoints.len();
            s.watchpoints.retain(|w| w.address != addr_num);
            if s.watchpoints.len() == before {
                bail!("观察点不存在 @ 0x{addr_num:08x}");
            }
            s.save(&session)?;
            if json {
                print_json(&serde_json::json!({ "address": addr_num, "removed": true }))?;
            } else {
                println!("已清除观察点 @ 0x{addr_num:08x}");
            }
            Ok(())
        }
        WatchSub::ClearAll { session, json } => {
            let mut s = Session::load(&session)?;
            let removed = s.watchpoints.len();
            s.watchpoints.clear();
            s.save(&session)?;
            if json {
                print_json(&serde_json::json!({ "removed": removed, "count": 0 }))?;
            } else {
                println!("已清除 {removed} 个观察点");
            }
            Ok(())
        }
        WatchSub::Enable {
            session,
            addr,
            addr_flag,
            json,
        } => set_watch_enabled(&session, addr, addr_flag, true, json),
        WatchSub::Disable {
            session,
            addr,
            addr_flag,
            json,
        } => set_watch_enabled(&session, addr, addr_flag, false, json),
    }
}

/// Enable/disable a watchpoint by address (shared by `watch enable`/`disable`).
fn set_watch_enabled(
    session: &Path,
    addr: Option<String>,
    addr_flag: Option<String>,
    enable: bool,
    json: bool,
) -> Result<()> {
    let addr = addr_flag.or(addr).ok_or_else(|| anyhow::anyhow!("缺少地址"))?;
    let addr_num = parse_num(&addr)?;
    let mut s = Session::load(session)?;
    let wp = watch_find(&mut s, addr_num)?;
    wp.enabled = enable;
    s.save(session)?;
    if json {
        print_json(&serde_json::json!({ "address": addr_num, "enabled": enable }))?;
    } else {
        println!(
            "已{}观察点 @ 0x{addr_num:08x}",
            if enable { "启用" } else { "禁用" }
        );
    }
    Ok(())
}

/// Format a `; file:line` source annotation for `address`, using the DWARF
/// line table and source files attached to the simulator (empty when none).
pub(crate) fn source_annotation(sim: &Simulator, address: u32) -> String {
    sim.line_for_address(address)
        .and_then(|(file, line)| sim.source_files.get(file).map(|f| (f, line)))
        .map(|(f, line)| format!("  ; {f}:{line}"))
        .unwrap_or_default()
}

fn cmd_disasm(args: DisasmArgs) -> Result<()> {
    let mut arch = resolve_arch(args.session.as_deref(), args.arch.as_deref())?;
    // Build a simulator to read code bytes: load the session's firmware if
    // recorded, otherwise disassemble from a zeroed image (best-effort).
    let mut sim = if let Some(sp) = &args.session {
        if sp.exists() {
            let s = Session::load(sp)?;
            s.apply(None).unwrap_or_else(|_| Simulator::for_arch(arch).unwrap())
        } else {
            Simulator::for_arch(arch)?
        }
    } else {
        Simulator::for_arch(arch)?
    };
    // `--firmware` loads the image directly (architecture inferred from an ELF).
    if let Some(fw) = &args.firmware {
        load_firmware_into(&mut sim, fw, parse_num(&args.base)?)?;
        arch = sim.arch;
    }
    let mut pc = if args.symbol.is_some() {
        // Resolved after the simulator is built (symbols come from the loaded
        // firmware).
        default_mem_base(arch)
    } else {
        match args.addr {
            Some(a) => parse_num(&a)?,
            None => {
                if args.firmware.is_some() {
                    sim.program_counter()
                } else if let Some(sp) = &args.session {
                    if sp.exists() {
                        Session::load(sp)?.pc
                    } else {
                        default_mem_base(arch)
                    }
                } else {
                    default_mem_base(arch)
                }
            }
        }
    };
    if let Some(sym) = &args.symbol {
        let found = sim
            .symbols
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(sym))
            .ok_or_else(|| anyhow::anyhow!("符号未找到: {sym}"))?;
        pc = found.address;
    }
    let arm_state = match args.arm_state.as_deref() {
        Some("a32") => true,
        Some("thumb") => false,
        _ => sim.arm_state(),
    };
    let read = |addr: u32| -> u8 { read_code_byte(&sim, addr) };
    let mut items = Vec::new();
    let mut lines: Vec<String> = Vec::new();
    // Symbol address -> comma-separated label names, for `--labels`.
    let mut labels: BTreeMap<u32, String> = BTreeMap::new();
    for s in &sim.symbols {
        let entry = labels.entry(s.address).or_default();
        if !entry.is_empty() {
            entry.push_str(", ");
        }
        entry.push_str(&s.name);
    }
    for _ in 0..args.count {
        let width = ns_sim::instruction_size(arch, pc, &read, arm_state);
        let text = ns_sim::disassemble_one_state(arch, pc, &read, arm_state);
        let label = labels.get(&pc).cloned();
        let raw = if args.raw {
            let mut s = String::new();
            for i in 0..width {
                if i > 0 {
                    s.push(' ');
                }
                s.push_str(&format!("{:02x}", read_code_byte(&sim, pc.wrapping_add(i as u32))));
            }
            format!("  ; raw: {s}")
        } else {
            String::new()
        };
        if args.json {
            items.push(serde_json::json!({
                "address": pc,
                "text": text,
                "bytes": (0..width)
                    .map(|i| read_code_byte(&sim, pc.wrapping_add(i as u32)))
                    .collect::<Vec<_>>(),
                "source": source_annotation(&sim, pc).trim().to_string(),
                "label": label,
                "armState": arm_state,
            }));
        } else if args.out.is_some() {
            if args.labels {
                if let Some(l) = &label {
                    lines.push(format!("{l}:"));
                }
            }
            let line = if args.source {
                format!("0x{pc:08x}: {text}{raw}  {}", source_annotation(&sim, pc))
            } else {
                format!("0x{pc:08x}: {text}{raw}")
            };
            lines.push(line);
        } else if args.source {
            if args.labels {
                if let Some(l) = &label {
                    println!("{l}:");
                }
            }
            let src = source_annotation(&sim, pc);
            println!("0x{pc:08x}: {text}{raw}  {src}");
        } else {
            if args.labels {
                if let Some(l) = &label {
                    println!("{l}:");
                }
            }
            println!("0x{pc:08x}: {text}{raw}");
        }
        pc = pc.wrapping_add(width);
    }
    if args.json {
        emit_out(args.out.as_deref(), &serde_json::to_string_pretty(&items)?)?;
    } else if let Some(path) = &args.out {
        std::fs::write(path, format!("{}\n", lines.join("\n")))
            .with_context(|| format!("写入 {} 失败", path.display()))?;
        println!("已写出反汇编 -> {}（{} 条指令）", path.display(), lines.len());
    }
    Ok(())
}

fn cmd_reset(args: ResetArgs) -> Result<()> {
    let (mut sim, fw, _) = load_or_create(Some(&args.session), None, None, "0")?;
    let entry = match args.entry {
        Some(e) => parse_num(&e)?,
        None => default_reset_entry(&sim, fw.as_ref())?,
    };
    sim.reset(entry);
    let s = Session::capture(&sim, fw);
    s.save(&args.session)?;
    if args.json {
        print_json(&serde_json::json!({
            "pc": sim.program_counter(),
            "cycleCount": sim.cycle_count,
            "halted": sim.halted,
        }))?;
    } else {
        println!("已重置 @ 0x{:08x}", sim.program_counter());
    }
    Ok(())
}

/// Derive the reset entry for a session: the loaded firmware's entry (ELF
/// entry / BIN base) when present, otherwise the architecture's reset entry.
fn default_reset_entry(sim: &Simulator, fw: Option<&FirmwareState>) -> Result<u32> {
    if let Some(fw) = fw {
        let mut fresh = Simulator::for_arch(sim.arch)?;
        load_firmware_into(&mut fresh, Path::new(&fw.path), fw.base)?;
        Ok(fresh.program_counter())
    } else {
        Ok(sim.entry())
    }
}

fn cmd_state(args: StateArgs) -> Result<()> {
    let (sim, fw, _) = load_or_create(
        args.session.as_deref(),
        args.arch.as_deref(),
        args.firmware.as_deref(),
        &args.base,
    )?;
    let output = state_output(&sim, StepResult::Advanced, fw.as_ref());
    if args.summary {
        println!(
            "PC=0x{:08x} cycles={} halted={} result={}",
            output.pc, output.cycle_count, output.halted, output.last_result
        );
        return Ok(());
    }
    if args.json {
        let mut value = serde_json::to_value(&output)?;
        // Watchpoints live in the session file, not the simulator core; report
        // them so an external caller gets the complete state snapshot.
        if let Some(sp) = &args.session {
            if sp.exists() {
                if let Ok(s) = Session::load(sp) {
                    let wps: Vec<serde_json::Value> =
                        s.watchpoints.iter().map(watch_json).collect();
                    if let Some(obj) = value.as_object_mut() {
                        obj.insert("watchpoints".to_string(), serde_json::json!(wps));
                    }
                }
            }
        }
        if let Some(ma) = &args.mem {
            let addr = parse_num(ma)?;
            let mut bytes = Vec::with_capacity(args.mem_len);
            for i in 0..args.mem_len {
                bytes.push(read_mem_byte(&sim, addr.wrapping_add(i as u32)));
            }
            if let Some(obj) = value.as_object_mut() {
                obj.insert(
                    "memory".to_string(),
                    serde_json::json!({
                        "address": addr,
                        "len": args.mem_len,
                        "bytes": bytes,
                    }),
                );
            }
        }
        if args.fields.is_empty() {
            print_json(&value)?;
        } else {
            value = select_json_fields(&value, &args.fields)?;
            print_json(&value)?;
        }
    } else {
        println!("架构: {}", sim.arch.label());
        println!("PC:   0x{:08x}", output.pc);
        println!("周期: {}", output.cycle_count);
        println!("停止: {}", output.halted);
        println!("寄存器:");
        for (name, value) in &output.registers {
            println!("  {name:<8} 0x{value:08x}");
        }
        if !output.uart.is_empty() {
            println!("UART: {:?}", output.uart_text);
        }
        if let Some(ma) = &args.mem {
            let addr = parse_num(ma)?;
            let mut bytes = Vec::with_capacity(args.mem_len);
            for i in 0..args.mem_len {
                bytes.push(read_mem_byte(&sim, addr.wrapping_add(i as u32)));
            }
            print!("{}", hex_dump(addr, &bytes));
        }
    }
    Ok(())
}

/// Keep only the requested (possibly dot-nested) fields of a JSON value.
/// An unknown field path is reported as an error instead of being dropped.
pub(crate) fn select_json_fields(
    value: &serde_json::Value,
    paths: &[String],
) -> Result<serde_json::Value> {
    fn insert_path(
        map: &mut serde_json::Map<String, serde_json::Value>,
        parts: &[&str],
        leaf: serde_json::Value,
    ) {
        if parts.len() == 1 {
            map.insert(parts[0].to_string(), leaf);
        } else if let Some(obj) = map
            .entry(parts[0].to_string())
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
        {
            insert_path(obj, &parts[1..], leaf);
        }
    }
    let mut result = serde_json::Map::new();
    for path in paths {
        let parts: Vec<&str> = path.split('.').collect();
        let mut cur = value;
        for part in &parts {
            cur = cur
                .get(*part)
                .ok_or_else(|| anyhow::anyhow!("未知状态字段: {path}"))?;
        }
        insert_path(&mut result, &parts, cur.clone());
    }
    Ok(serde_json::Value::Object(result))
}

/// Resolve a peripheral-register address from either `--addr` or
/// `--name PERIPH.REG` (with `--device` selecting the built-in SVD).
pub(crate) fn resolve_reg_address(
    addr: Option<&str>,
    name: Option<&str>,
    device: Option<&str>,
) -> Result<u32> {
    match (addr, name) {
        (Some(a), None) => parse_num(a),
        (None, Some(n)) => {
            let dev_key =
                device.ok_or_else(|| anyhow::anyhow!("--name 需要配合 --device"))?;
            let (periph, reg) = n
                .split_once('.')
                .ok_or_else(|| anyhow::anyhow!("--name 格式应为 PERIPH.REG: {n}"))?;
            let dev = ns_device::device_for_target(dev_key)
                .ok_or_else(|| anyhow::anyhow!("未知设备或缺少内置 SVD: {dev_key}"))?;
            let p = dev
                .peripheral(periph)
                .ok_or_else(|| anyhow::anyhow!("设备 {dev_key} 无外设: {periph}"))?;
            let r = p
                .registers
                .iter()
                .find(|r| r.name == reg)
                .ok_or_else(|| anyhow::anyhow!("外设 {periph} 无寄存器: {reg}"))?;
            Ok(r.address as u32)
        }
        (Some(_), Some(_)) => bail!("--addr 与 --name 只能二选一"),
        (None, None) => bail!("请提供 --addr 或 --name"),
    }
}

/// Resolve a named SVD field to its register address, bit offset and width.
pub(crate) fn resolve_reg_field(
    device: &str,
    peripheral: &str,
    register: &str,
    field: &str,
) -> Result<(u32, u32, u32)> {
    let dev = ns_device::device_for_target(device)
        .ok_or_else(|| anyhow::anyhow!("未知设备或缺少内置 SVD: {device}"))?;
    let p = dev
        .peripheral(peripheral)
        .ok_or_else(|| anyhow::anyhow!("设备 {device} 无外设: {peripheral}"))?;
    let r = p
        .registers
        .iter()
        .find(|r| r.name == register)
        .ok_or_else(|| anyhow::anyhow!("外设 {peripheral} 无寄存器: {register}"))?;
    let f = r
        .fields
        .iter()
        .find(|f| f.name == field)
        .ok_or_else(|| anyhow::anyhow!("寄存器 {register} 无位域: {field}"))?;
    if f.bit_offset >= 32 || f.bit_width > 32 || f.bit_offset + f.bit_width > 32 {
        bail!(
            "位域 {field} 超出 32 位范围 (bit {} width {})",
            f.bit_offset,
            f.bit_width
        );
    }
    Ok((r.address as u32, f.bit_offset, f.bit_width))
}

/// Decode a register value into its SVD bit fields, extracting each field's
/// current value from the register (a zero-width field decodes to 0).
pub(crate) fn decode_fields(value: u32, fields: &[ns_device::Field]) -> Vec<serde_json::Value> {
    fields
        .iter()
        .map(|f| {
            let mask = if f.bit_width == 0 || f.bit_width >= 32 {
                u32::MAX
            } else {
                (1u32 << f.bit_width) - 1
            };
            let bits = if f.bit_offset < 32 && f.bit_width > 0 {
                (value >> f.bit_offset) & mask
            } else {
                0
            };
            serde_json::json!({
                "name": f.name,
                "bitOffset": f.bit_offset,
                "bitWidth": f.bit_width,
                "value": bits,
                "description": f.description,
            })
        })
        .collect()
}

/// Dump every named register of a device (optionally filtered to one
/// peripheral) with its current value, reset value and decoded bit fields.
/// Shared by the `periph reg-dump` CLI and the `periph_reg_dump` MCP tool.
pub(crate) fn device_reg_dump(
    sim: &Simulator,
    dev: &ns_device::Device,
    filter: Option<&str>,
    with_fields: bool,
) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for p in dev.sorted_peripherals() {
        if let Some(f) = filter {
            if !p.name.eq_ignore_ascii_case(f) {
                continue;
            }
        }
        for r in &p.registers {
            let addr = r.address as u32;
            let value = sim.peripheral_read_reg(addr).unwrap_or_else(|| {
                let mut v = 0u32;
                let size = (r.size as usize).max(1);
                for i in 0..size.min(4) {
                    v |= (read_mem_byte(sim, addr.wrapping_add(i as u32)) as u32) << (i * 8);
                }
                v
            });
            let mut entry = serde_json::json!({
                "peripheral": p.name,
                "register": r.name,
                "address": addr,
                "value": value,
                "size": r.size,
                "resetValue": r.reset_value,
                "description": r.description,
            });
            if with_fields {
                entry["fields"] = serde_json::Value::Array(decode_fields(value, &r.fields));
            }
            out.push(entry);
        }
    }
    out
}

/// Parse a numeric value that may be hex (`0x…`), binary (`0b…`) or decimal,
/// up to 64 bits (needed for `dword` search patterns wider than a `u32`).
fn parse_num_u64(text: &str) -> Result<u64> {
    let text = text.trim();
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16)
            .map_err(|e| anyhow::anyhow!("无效十六进制数 '{text}': {e}"))
    } else if let Some(bin) = text.strip_prefix("0b").or_else(|| text.strip_prefix("0B")) {
        u64::from_str_radix(bin, 2)
            .map_err(|e| anyhow::anyhow!("无效二进制数 '{text}': {e}"))
    } else {
        text.parse::<u64>()
            .map_err(|e| anyhow::anyhow!("无效数值 '{text}': {e}"))
    }
}

/// Expand a `mem search` pattern into the raw little-endian bytes to look for.
/// Each token is an integer of the requested width (`byte`/`half`/`word`/`dword`),
/// so callers can search for a word or double-word constant directly instead of
/// spelling its bytes. Shared by the CLI and the `mem_search` MCP tool.
pub(crate) fn expand_search_pattern(
    pattern: &[String],
    width: &str,
) -> Result<Vec<u8>> {
    let size = match width {
        "" | "byte" => 1usize,
        "half" => 2,
        "word" => 4,
        "dword" => 8,
        other => bail!("无效宽度 '{other}'（支持 byte/half/word/dword）"),
    };
    let max = if size * 8 == 64 {
        u64::MAX
    } else {
        (1u64 << (size * 8)) - 1
    };
    let mut bytes = Vec::new();
    for token in pattern {
        let value = parse_num_u64(token)?;
        if value > max {
            bail!("模式值超出 {} 位: {token}", size * 8);
        }
        for i in 0..size {
            bytes.push((value >> (8 * i)) as u8);
        }
    }
    Ok(bytes)
}

fn cmd_periph(sub: PeriphSub) -> Result<()> {
    match sub {
        PeriphSub::Status { session, json } => {
            let (sim, _, _) = load_or_create(Some(&session), None, None, "0")?;
            let uart_text = String::from_utf8_lossy(&sim.peripherals.outputs).into_owned();
            let gpio_events: Vec<_> = sim
                .peripherals
                .gpio_events
                .iter()
                .map(|e| serde_json::json!({"cycle": e.cycle, "pin": e.pin, "value": e.value, "pc": e.pc}))
                .collect();
            let timers: Vec<_> = sim
                .peripherals
                .timers
                .iter()
                .map(|t| serde_json::json!({"base": t.base, "enabled": t.enabled, "counter": t.counter, "autoReload": t.auto_reload, "prescaler": t.prescaler, "updateCount": t.update_count}))
                .collect();
            let watchdog = sim.peripherals.watchdog.as_ref().map(|w| serde_json::json!({
                "base": w.base, "enabled": w.enabled, "timeoutCycles": w.timeout_cycles,
                "elapsed": w.elapsed, "resetCount": w.reset_count, "lastResetPc": w.last_reset_pc,
            }));
            let out = serde_json::json!({
                "uartBase": sim.peripherals.uart_base,
                "gpioBase": sim.peripherals.gpio_base,
                "uart": sim.peripherals.outputs,
                "uartText": uart_text,
                "gpioOut": sim.peripherals.gpio_out,
                "gpioDir": sim.peripherals.gpio_dir,
                "gpioIn": sim.peripherals.gpio_in,
                "gpioEvents": gpio_events,
                "timers": timers,
                "watchdog": watchdog,
                "externalRegs": sim.peripherals.external_regs,
            });
            if json {
                print_json(&out)?;
            } else {
                println!(
                    "基址: UART=0x{:08x} GPIO=0x{:08x}",
                    sim.peripherals.uart_base, sim.peripherals.gpio_base
                );
                println!("UART 输出: {} 字节 {:?}", sim.peripherals.outputs.len(), uart_text);
                println!("GPIO OUT=0x{:08x} DIR=0x{:08x} IN=0x{:08x}", sim.peripherals.gpio_out, sim.peripherals.gpio_dir, sim.peripherals.gpio_in);
                println!("GPIO 事件数: {}", sim.peripherals.gpio_events.len());
                println!("定时器数: {}", sim.peripherals.timers.len());
                println!(
                    "SVD 寄存器数: {}",
                    sim.peripherals.external_regs.len()
                );
            }
            Ok(())
        }
        PeriphSub::Config { session, uart_base, gpio_base, json } => {
            if uart_base.is_none() && gpio_base.is_none() {
                bail!("至少提供 --uart-base 或 --gpio-base");
            }
            let (mut sim, fw, _) = load_or_create(Some(&session), None, None, "0")?;
            if let Some(v) = uart_base.as_deref() {
                sim.peripherals.uart_base = parse_num(v)?;
            }
            if let Some(v) = gpio_base.as_deref() {
                sim.peripherals.gpio_base = parse_num(v)?;
            }
            let out = serde_json::json!({
                "uartBase": sim.peripherals.uart_base,
                "gpioBase": sim.peripherals.gpio_base,
            });
            let s = Session::capture(&sim, fw);
            s.save(&session)?;
            if json {
                print_json(&out)?;
            } else {
                println!(
                    "外设基址: UART=0x{:08x} GPIO=0x{:08x}",
                    sim.peripherals.uart_base, sim.peripherals.gpio_base
                );
            }
            Ok(())
        }
        PeriphSub::SvdLoad { session, file, device, peripheral, force, json } => {
            let (device, source) = match (file, device) {
                (Some(f), None) => {
                    let dev = ns_device::load_svd_file(&f)?;
                    (dev, f.display().to_string())
                }
                (None, Some(d)) => {
                    let dev = ns_device::device_for_target(&d)
                        .ok_or_else(|| anyhow::anyhow!("未知设备或缺少内置 SVD: {d}"))?;
                    (dev, d)
                }
                (Some(_), Some(_)) => bail!("--file 与 --device 只能二选一"),
                (None, None) => bail!("请提供 --file 或 --device"),
            };
            let periphs: Vec<&ns_device::Peripheral> = match peripheral.as_deref() {
                Some(name) => {
                    let p = device.peripheral(name).ok_or_else(|| {
                        anyhow::anyhow!("SVD 中无外设: {name}")
                    })?;
                    vec![p]
                }
                None => device.sorted_peripherals(),
            };
            let (mut sim, fw, _) = load_or_create(Some(&session), None, None, "0")?;
            let mut loaded = 0usize;
            for p in &periphs {
                for r in &p.registers {
                    let addr = r.address as u32;
                    if force || !sim.peripherals.external_regs.contains_key(&addr) {
                        sim.peripherals.external_regs.insert(addr, r.reset_value as u32);
                        // 写入后备内存，便于直接 mem read 也能看到值。
                        let v = r.reset_value as u32;
                        for i in 0..4 {
                            write_mem_byte(&mut sim, addr.wrapping_add(i as u32), (v >> (8 * i)) as u8);
                        }
                        loaded += 1;
                    }
                }
            }
            let out = serde_json::json!({
                "source": source,
                "loaded": loaded,
                "peripherals": periphs.len(),
                "force": force,
            });
            let s = Session::capture(&sim, fw);
            s.save(&session)?;
            if json {
                print_json(&out)?;
            } else {
                println!(
                    "已从 {} 加载 {} 个寄存器（{} 个外设）", source, loaded, periphs.len()
                );
            }
            Ok(())
        }
        PeriphSub::Uart { session, clear, input, bytes, json } => {
            let (mut sim, fw, _) = load_or_create(Some(&session), None, None, "0")?;
            let mut injected = 0usize;
            if let Some(text) = &input {
                injected = text.len();
                sim.push_uart_input(text.as_bytes());
            } else if !bytes.is_empty() {
                let mut data = Vec::with_capacity(bytes.len());
                for b in &bytes {
                    let v = parse_num(b)?;
                    if v > 0xff {
                        bail!("字节超出 0..=255: {}", b);
                    }
                    data.push(v as u8);
                }
                injected = data.len();
                sim.push_uart_input(&data);
            }
            if injected > 0 || clear {
                if clear {
                    sim.peripherals.outputs.clear();
                }
                let s = Session::capture(&sim, fw);
                s.save(&session)?;
            }
            let text = String::from_utf8_lossy(&sim.peripherals.outputs).into_owned();
            let len = sim.peripherals.outputs.len();
            let rx_len = sim.peripherals.uart_rx.borrow().len();
            if json {
                print_json(&serde_json::json!({
                    "len": len,
                    "text": text,
                    "cleared": clear,
                    "injected": injected,
                    "inputLen": rx_len,
                }))?;
            } else {
                let mut msg = format!("UART 输出 ({len} 字节): {text}");
                if injected > 0 {
                    msg.push_str(&format!("\nUART 注入 {injected} 字节（剩余接收队列 {rx_len}）"));
                }
                println!("{msg}");
            }
            Ok(())
        }
        PeriphSub::Gpio { session, pin, pin_flag, value, value_flag, direction, output, read, json } => {
            let pin = pin_flag.or(pin).ok_or_else(|| anyhow::anyhow!("缺少引脚号"))?;
            if pin > 31 {
                bail!("引脚号必须在 0..=31");
            }
            let value = value_flag.or(value);
            if let Some(v) = value {
                if v > 1 {
                    bail!("引脚值必须是 0 或 1");
                }
            }
            if let Some(v) = direction {
                if v > 1 {
                    bail!("方向必须是 0 或 1");
                }
            }
            if let Some(v) = output {
                if v > 1 {
                    bail!("输出值必须是 0 或 1");
                }
            }
            let (mut sim, fw, _) = load_or_create(Some(&session), None, None, "0")?;
            if let Some(v) = value {
                if v == 1 {
                    sim.peripherals.gpio_in |= 1 << pin;
                } else {
                    sim.peripherals.gpio_in &= !(1 << pin);
                }
            }
            if let Some(v) = direction {
                if v == 1 {
                    sim.peripherals.gpio_dir |= 1 << pin;
                } else {
                    sim.peripherals.gpio_dir &= !(1 << pin);
                }
            }
            if let Some(v) = output {
                if v == 1 {
                    sim.peripherals.gpio_out |= 1 << pin;
                } else {
                    sim.peripherals.gpio_out &= !(1 << pin);
                }
            }
            let s = Session::capture(&sim, fw);
            s.save(&session)?;
            let in_val = (sim.peripherals.gpio_in >> pin) & 1;
            let out_val = (sim.peripherals.gpio_out >> pin) & 1;
            let dir_val = (sim.peripherals.gpio_dir >> pin) & 1;
            if json {
                print_json(&serde_json::json!({
                    "pin": pin,
                    "value": in_val,
                    "input": in_val,
                    "output": out_val,
                    "direction": dir_val,
                    "read": read,
                    "gpioIn": sim.peripherals.gpio_in,
                    "gpioOut": sim.peripherals.gpio_out,
                    "gpioDir": sim.peripherals.gpio_dir,
                }))?;
            } else {
                println!("GPIO 引脚 {pin}: IN={in_val} OUT={out_val} DIR={dir_val}");
            }
            Ok(())
        }
        PeriphSub::Timer { session, index, enabled, prescaler, auto_reload, counter, update_count, json } => {
            if index >= 8 {
                bail!("定时器索引必须在 0..8");
            }
            let (mut sim, fw, _) = load_or_create(Some(&session), None, None, "0")?;
            // Ensure the timer exists (extend the vector up to index+1).
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
                if let Some(v) = counter {
                    t.counter = v;
                }
                if let Some(v) = update_count {
                    t.update_count = v;
                }
            }
            let t = &sim.peripherals.timers[index];
            let out = serde_json::json!({
                "index": index,
                "base": t.base,
                "enabled": t.enabled,
                "counter": t.counter,
                "autoReload": t.auto_reload,
                "prescaler": t.prescaler,
                "irqLine": t.irq_line,
                "updateCount": t.update_count,
            });
            let s = Session::capture(&sim, fw);
            s.save(&session)?;
            if json {
                print_json(&out)?;
            } else {
                println!(
                    "定时器[{index}]: base=0x{:08x} enabled={} counter={} autoReload={} prescaler={} irqLine={} updates={}",
                    t.base, t.enabled, t.counter, t.auto_reload, t.prescaler, t.irq_line, t.update_count
                );
            }
            Ok(())
        }
        PeriphSub::RegRead { session, addr, device, name, len, json } => {
            let addr_num = resolve_reg_address(addr.as_deref(), name.as_deref(), device.as_deref())?;
            let (sim, _, _) = load_or_create(Some(&session), None, None, "0")?;
            let mut bytes = Vec::with_capacity(len);
            // If the address maps to a peripheral register, report the 32-bit
            // register value (little-endian); otherwise fall back to raw bytes.
            if let Some(value) = sim.peripheral_read_reg(addr_num) {
                let value_bytes = value.to_le_bytes();
                for i in 0..len {
                    bytes.push(if i < 4 { value_bytes[i] } else { 0 });
                }
            } else {
                for i in 0..len {
                    bytes.push(read_mem_byte(&sim, addr_num.wrapping_add(i as u32)));
                }
            }
            if json {
                print_json(&serde_json::json!({
                    "address": addr_num, "bytes": bytes,
                }))?;
            } else {
                print!("{}", hex_dump(addr_num, &bytes));
            }
            Ok(())
        }
        PeriphSub::RegList { session, device, json } => {
            let (sim, _, _) = load_or_create(Some(&session), None, None, "0")?;
            // Optional address -> name annotation from a built-in device SVD.
            let mut names: BTreeMap<u32, String> = BTreeMap::new();
            if let Some(key) = device.as_deref() {
                let dev = ns_device::device_for_target(key)
                    .ok_or_else(|| anyhow::anyhow!("未知设备或缺少内置 SVD: {key}"))?;
                for p in &dev.sorted_peripherals() {
                    for r in &p.registers {
                        names.insert(r.address as u32, format!("{}.{}", p.name, r.name));
                    }
                }
            }
            let mut entries: Vec<(u32, u32)> =
                sim.peripherals.external_regs.iter().map(|(a, v)| (*a, *v)).collect();
            entries.sort_by_key(|(a, _)| *a);
            if json {
                let items: Vec<_> = entries
                    .iter()
                    .map(|(a, v)| {
                        serde_json::json!({
                            "address": a,
                            "value": v,
                            "name": names.get(a).cloned().unwrap_or_default(),
                        })
                    })
                    .collect();
                print_json(&items)?;
            } else {
                if entries.is_empty() {
                    println!("(未加载任何外设寄存器)");
                }
                for (a, v) in &entries {
                    if let Some(name) = names.get(a) {
                        println!("0x{a:08x}  0x{v:08x}  {name}");
                    } else {
                        println!("0x{a:08x}  0x{v:08x}");
                    }
                }
            }
            Ok(())
        }
        PeriphSub::RegDump { session, device, peripheral, fields, json } => {
            let dev = ns_device::device_for_target(&device)
                .ok_or_else(|| anyhow::anyhow!("未知设备或缺少内置 SVD: {device}"))?;
            let (sim, _, _) = load_or_create(Some(&session), None, None, "0")?;
            let out = device_reg_dump(&sim, &dev, peripheral.as_deref(), fields);
            if json {
                print_json(&serde_json::json!({
                    "device": device,
                    "count": out.len(),
                    "registers": out,
                }))?;
            } else {
                if out.is_empty() {
                    println!("(设备 {device} 无寄存器)");
                }
                for r in &out {
                    let name = format!(
                        "{}.{}",
                        r["peripheral"].as_str().unwrap_or(""),
                        r["register"].as_str().unwrap_or("")
                    );
                    println!(
                        "{:<24} 0x{:08x}  0x{:08x}  {}",
                        name,
                        r["address"].as_u64().unwrap_or(0) as u32,
                        r["value"].as_u64().unwrap_or(0) as u32,
                        r["description"].as_str().unwrap_or("")
                    );
                    if fields {
                        let decoded: Vec<&serde_json::Value> = r
                            .get("fields")
                            .and_then(|v| v.as_array())
                            .map(|a| a.iter().collect())
                            .unwrap_or_default();
                        for f in decoded {
                            let off = f["bitOffset"].as_u64().unwrap_or(0);
                            let wid = f["bitWidth"].as_u64().unwrap_or(0);
                            let end = if wid == 0 { off } else { off + wid - 1 };
                            let bits = f["value"].as_u64().unwrap_or(0) as u32;
                            let name = f["name"].as_str().unwrap_or("").to_string();
                            let desc = f["description"].as_str().unwrap_or("").to_string();
                            println!(
                                "    {:<24} bit {}:{}  0x{bits:02x}  {desc}",
                                name,
                                off,
                                end
                            );
                        }
                    }
                }
            }
            Ok(())
        }
        PeriphSub::RegWrite { session, addr, device, name, bytes, json } => {
            let addr_num = resolve_reg_address(addr.as_deref(), name.as_deref(), device.as_deref())?;
            let mut data = Vec::new();
            for b in &bytes {
                let v = parse_num(b)?;
                if v > 0xff {
                    bail!("字节超出 0..=255: {}", b);
                }
                data.push(v as u8);
            }
            let (mut sim, fw, _) = load_or_create(Some(&session), None, None, "0")?;
            // Write the byte sequence to backing memory, then route each byte
            // through the peripheral handler. GPIO/timer/watchdog only act on
            // the base offset (0), while UART emits every byte.
            for (i, b) in data.iter().enumerate() {
                write_mem_byte(&mut sim, addr_num.wrapping_add(i as u32), *b);
            }
            for (i, b) in data.iter().enumerate() {
                sim.peripheral_write_reg(addr_num.wrapping_add(i as u32), *b);
            }
            let s = Session::capture(&sim, fw);
            s.save(&session)?;
            if json {
                print_json(&serde_json::json!({ "address": addr_num, "written": data.len() }))?;
            } else {
                println!("已写入 {} 字节 @ 0x{:08x}", data.len(), addr_num);
            }
            Ok(())
        }
        PeriphSub::FieldGet { session, device, peripheral, register, field, json } => {
            let (addr, bit_offset, bit_width) =
                resolve_reg_field(&device, &peripheral, &register, &field)?;
            let (sim, _, _) = load_or_create(Some(&session), None, None, "0")?;
            let mask = if bit_width >= 32 { u32::MAX } else { (1u32 << bit_width) - 1 };
            let reg_value = sim.peripheral_read_reg(addr).unwrap_or_else(|| read_mem_u32(&sim, addr));
            let field_value = (reg_value >> bit_offset) & mask;
            if json {
                print_json(&serde_json::json!({
                    "address": addr,
                    "field": format!("{}.{}.{}", peripheral, register, field),
                    "bitOffset": bit_offset,
                    "bitWidth": bit_width,
                    "value": field_value,
                    "register": reg_value,
                }))?;
            } else {
                println!(
                    "{}.{}.{} = 0x{:x} (bit {}:{} reg=0x{:08x})",
                    peripheral, register, field, field_value, bit_offset,
                    bit_offset + bit_width.saturating_sub(1), reg_value
                );
            }
            Ok(())
        }
        PeriphSub::FieldSet { session, device, peripheral, register, field, value, json } => {
            let (addr, bit_offset, bit_width) =
                resolve_reg_field(&device, &peripheral, &register, &field)?;
            let new_bits = parse_num(&value)?;
            let mask = if bit_width >= 32 { u32::MAX } else { (1u32 << bit_width) - 1 };
            if new_bits > mask {
                bail!("字段值超出 {} 位: {}", bit_width, value);
            }
            let (mut sim, fw, _) = load_or_create(Some(&session), None, None, "0")?;
            let cur = sim.peripheral_read_reg(addr).unwrap_or_else(|| read_mem_u32(&sim, addr));
            let new_reg =
                (cur & !(mask << bit_offset)) | ((new_bits << bit_offset) & (mask << bit_offset));
            let bytes = new_reg.to_le_bytes();
            for (i, b) in bytes.iter().enumerate() {
                write_mem_byte(&mut sim, addr.wrapping_add(i as u32), *b);
            }
            for (i, b) in bytes.iter().enumerate() {
                sim.peripheral_write_reg(addr.wrapping_add(i as u32), *b);
            }
            let s = Session::capture(&sim, fw);
            s.save(&session)?;
            if json {
                print_json(&serde_json::json!({
                    "address": addr,
                    "field": format!("{}.{}.{}", peripheral, register, field),
                    "bitOffset": bit_offset,
                    "bitWidth": bit_width,
                    "value": new_bits,
                    "register": new_reg,
                }))?;
            } else {
                println!(
                    "{}.{}.{} = 0x{:x} (reg=0x{:08x})",
                    peripheral, register, field, new_bits, new_reg
                );
            }
            Ok(())
        }
    }
}


// ---------------------------------------------------------------------------
// Symbol / cycle / interrupt / watchdog / CSR commands
// ---------------------------------------------------------------------------

/// Load a simulator from an existing session (firmware re-loaded if recorded).
fn load_session_sim(session: &Path) -> Result<Simulator> {
    let s = Session::load(session)?;
    s.apply(None)
        .map_err(|e| anyhow::anyhow!("恢复会话 {} 失败: {e}", session.display()))
}

fn cmd_symbol(sub: SymbolSub) -> Result<()> {
    match sub {
        SymbolSub::List { session, filter, json } => {
            let sim = load_session_sim(&session)?;
            let filter = filter.map(|f| f.to_ascii_lowercase());
            let symbols: Vec<_> = sim
                .symbols
                .iter()
                .filter(|s| {
                    filter
                        .as_ref()
                        .map(|f| s.name.to_ascii_lowercase().contains(f))
                        .unwrap_or(true)
                })
                .map(|s| serde_json::json!({ "name": s.name, "address": s.address, "size": s.size }))
                .collect();
            if json {
                print_json(&symbols)?;
            } else if symbols.is_empty() {
                println!("(无符号)");
            } else {
                println!("{:<40} {:>12} {:>8}", "NAME", "ADDRESS", "SIZE");
                for s in &symbols {
                    println!(
                        "{:<40} 0x{:08x} {:>8}",
                        s["name"].as_str().unwrap_or(""),
                        s["address"].as_u64().unwrap_or(0),
                        s["size"].as_u64().unwrap_or(0)
                    );
                }
            }
            Ok(())
        }
        SymbolSub::At { session, addr, addr_flag, json } => {
            let addr = addr_flag.or(addr).ok_or_else(|| anyhow::anyhow!("缺少地址"))?;
            let addr_num = parse_num(&addr)?;
            let sim = load_session_sim(&session)?;
            let found = sim.symbol_at(addr_num).map(|s| {
                serde_json::json!({ "name": s.name, "address": s.address, "size": s.size })
            });
            if json {
                print_json(&found)?;
            } else if let Some(s) = &found {
                println!(
                    "0x{:08x}: {} (size {})",
                    s["address"].as_u64().unwrap_or(0),
                    s["name"].as_str().unwrap_or(""),
                    s["size"].as_u64().unwrap_or(0)
                );
            } else {
                println!("0x{:08x}: (无符号)", addr_num);
            }
            Ok(())
        }
        SymbolSub::Nearest { session, addr, addr_flag, json } => {
            let addr = addr_flag.or(addr).ok_or_else(|| anyhow::anyhow!("缺少地址"))?;
            let addr_num = parse_num(&addr)?;
            let sim = load_session_sim(&session)?;
            let found = sim
                .symbols
                .iter()
                .filter(|s| s.address <= addr_num)
                .max_by_key(|s| s.address)
                .map(|s| {
                    serde_json::json!({
                        "name": s.name,
                        "address": s.address,
                        "size": s.size,
                        "offset": addr_num - s.address,
                    })
                });
            if json {
                print_json(&found)?;
            } else if let Some(s) = &found {
                println!(
                    "0x{:08x}: {} + {:#x} (base 0x{:08x}, size {})",
                    addr_num,
                    s["name"].as_str().unwrap_or(""),
                    s["offset"].as_u64().unwrap_or(0),
                    s["address"].as_u64().unwrap_or(0),
                    s["size"].as_u64().unwrap_or(0)
                );
            } else {
                println!("0x{:08x}: (无前置符号)", addr_num);
            }
            Ok(())
        }
        SymbolSub::Line { session, addr, addr_flag, json } => {
            let addr = addr_flag.or(addr).ok_or_else(|| anyhow::anyhow!("缺少地址"))?;
            let addr_num = parse_num(&addr)?;
            let sim = load_session_sim(&session)?;
            let found = sim
                .line_for_address(addr_num)
                .map(|(file, line)| serde_json::json!({ "file": file, "line": line }));
            if json {
                print_json(&found)?;
            } else if let Some(v) = &found {
                println!("0x{:08x}: file={} line={}", addr_num, v["file"], v["line"]);
            } else {
                println!("0x{:08x}: (无行号映射)", addr_num);
            }
            Ok(())
        }
        SymbolSub::Addr { session, file, line, json } => {
            let sim = load_session_sim(&session)?;
            let found = sim.address_for_line(file, line);
            if json {
                print_json(&found)?;
            } else if let Some(a) = found {
                println!("file={} line={} -> 0x{a:08x}", file, line);
            } else {
                println!("file={} line={}: (无地址映射)", file, line);
            }
            Ok(())
        }
    }
}

fn cmd_cycle(sub: CycleSub) -> Result<()> {
    match sub {
        CycleSub::Get { session, json } => {
            let sim = load_session_sim(&session)?;
            if json {
                print_json(&serde_json::json!({ "cycleCount": sim.cycle_count }))?;
            } else {
                println!("周期: {}", sim.cycle_count);
            }
            Ok(())
        }
        CycleSub::Estimate { session, addr, addr_flag, symbol, json } => {
            if symbol.is_some() && (addr.is_some() || addr_flag.is_some()) {
                bail!("--addr 与 --symbol 只能二选一");
            }
            let sim = load_session_sim(&session)?;
            let (addr_num, resolved_symbol) = if let Some(name) = symbol.as_deref() {
                let sym = sim
                    .symbols
                    .iter()
                    .find(|s| s.name == name)
                    .ok_or_else(|| anyhow::anyhow!("符号未找到: {name}"))?;
                (sym.address, Some(sym.name.clone()))
            } else {
                let addr = addr_flag.or(addr).ok_or_else(|| anyhow::anyhow!("缺少地址或符号"))?;
                (parse_num(&addr)?, None)
            };
            let cycles = sim.cycles_for_instruction(addr_num);
            if json {
                print_json(&serde_json::json!({
                    "address": addr_num,
                    "symbol": resolved_symbol,
                    "cycles": cycles,
                }))?;
            } else {
                if let Some(name) = resolved_symbol {
                    println!("{name} (0x{:08x}): {} 周期", addr_num, cycles);
                } else {
                    println!("0x{:08x}: {} 周期", addr_num, cycles);
                }
            }
            Ok(())
        }
        CycleSub::Tick { session, cycles, json } => {
            let (mut sim, fw, _) = load_or_create(Some(&session), None, None, "0")?;
            sim.tick_peripherals_for(cycles);
            sim.cycle_count = sim.cycle_count.saturating_add(cycles);
            let s = Session::capture(&sim, fw);
            s.save(&session)?;
            if json {
                print_json(&serde_json::json!({ "cycles": cycles, "cycleCount": sim.cycle_count }))?;
            } else {
                println!("已推进 {cycles} 周期，累计 {}", sim.cycle_count);
            }
            Ok(())
        }
        CycleSub::Set { session, count, count_flag, json } => {
            let count = count_flag.or(count).ok_or_else(|| anyhow::anyhow!("缺少周期数"))?;
            let count = parse_num(&count)?;
            let (mut sim, fw, _) = load_or_create(Some(&session), None, None, "0")?;
            sim.cycle_count = count as u64;
            let s = Session::capture(&sim, fw);
            s.save(&session)?;
            if json {
                print_json(&serde_json::json!({ "cycleCount": sim.cycle_count }))?;
            } else {
                println!("周期计数已设为 {}", sim.cycle_count);
            }
            Ok(())
        }
    }
}

fn cmd_interrupt(sub: InterruptSub) -> Result<()> {
    match sub {
        InterruptSub::Take { session, json } => {
            let (mut sim, fw, _) = load_or_create(Some(&session), None, None, "0")?;
            let taken = sim.take_interrupt();
            let s = Session::capture(&sim, fw);
            s.save(&session)?;
            if json {
                print_json(&serde_json::json!({ "taken": taken, "pc": sim.program_counter(), "mcause": sim.csr.mcause }))?;
            } else if taken {
                println!("已取中断，PC=0x{:08x} mcause=0x{:08x}", sim.program_counter(), sim.csr.mcause);
            } else {
                println!("无中断可取（检查 mstatus.MIE 与 mie 使能位）");
            }
            Ok(())
        }
        InterruptSub::Pending { session, json } => {
            let sim = load_session_sim(&session)?;
            let lines: Vec<usize> = sim
                .peripherals
                .irq_pending
                .iter()
                .enumerate()
                .filter(|(_, p)| **p)
                .map(|(i, _)| i)
                .collect();
            if json {
                print_json(&serde_json::json!({
                    "anyPending": sim.any_pending(),
                    "pendingLines": lines,
                    "mstatus": sim.csr.mstatus,
                    "mie": sim.csr.mie,
                }))?;
            } else {
                println!("anyPending={} 挂起行={:?} mstatus=0x{:08x} mie=0x{:08x}",
                    sim.any_pending(), lines, sim.csr.mstatus, sim.csr.mie);
            }
            Ok(())
        }
        InterruptSub::Set { session, line, line_flag, value, value_flag, json } => {
            let line = line_flag.or(line).ok_or_else(|| anyhow::anyhow!("缺少中断线"))?;
            let value = value_flag.or(value).unwrap_or(1);
            if line >= 16 {
                bail!("中断线必须在 0..16");
            }
            if value > 1 {
                bail!("中断线值必须是 0 或 1");
            }
            let (mut sim, fw, _) = load_or_create(Some(&session), None, None, "0")?;
            sim.peripherals.irq_pending[line] = value == 1;
            let s = Session::capture(&sim, fw);
            s.save(&session)?;
            if json {
                print_json(&serde_json::json!({ "line": line, "value": value }))?;
            } else {
                println!("IRQ {line} = {value}");
            }
            Ok(())
        }
    }
}

fn cmd_watchdog(sub: WatchdogSub) -> Result<()> {
    match sub {
        WatchdogSub::Status { session, json } => {
            let sim = load_session_sim(&session)?;
            let w = sim.peripherals.watchdog.as_ref();
            if json {
                print_json(&w.map(|w| serde_json::json!({
                    "base": w.base, "enabled": w.enabled, "timeoutCycles": w.timeout_cycles,
                    "elapsed": w.elapsed, "resetCount": w.reset_count, "lastResetPc": w.last_reset_pc,
                })))?;
            } else if let Some(w) = w {
                println!(
                    "看门狗: base=0x{:08x} enabled={} timeout={} elapsed={} resets={} lastResetPc=0x{:08x}",
                    w.base, w.enabled, w.timeout_cycles, w.elapsed, w.reset_count, w.last_reset_pc
                );
            } else {
                println!("(未启用看门狗)");
            }
            Ok(())
        }
        WatchdogSub::Feed { session, json } => {
            let (mut sim, fw, _) = load_or_create(Some(&session), None, None, "0")?;
            let base = sim
                .peripherals
                .watchdog
                .as_ref()
                .map(|w| w.base)
                .unwrap_or(0x4000_0000);
            let elapsed_before = sim.peripherals.watchdog.as_ref().map(|w| w.elapsed).unwrap_or(0);
            {
                let w = sim.peripherals.watchdog.get_or_insert_with(|| ns_sim::Watchdog::new(base));
                w.base = base;
                // Feeding the watchdog resets its elapsed counter.
                w.elapsed = 0;
            }
            sim.write_u32(base, 0xAA55);
            let s = Session::capture(&sim, fw);
            s.save(&session)?;
            if json {
                print_json(&serde_json::json!({
                    "fed": true,
                    "base": base,
                    "elapsedBefore": elapsed_before,
                    "elapsed": 0u64,
                }))?;
            } else {
                println!("已喂狗 @ 0x{:08x}", base);
            }
            Ok(())
        }
        WatchdogSub::Config { session, enabled, timeout, base, json } => {
            let base_num = base.as_deref().map(parse_num).transpose()?.unwrap_or(0x4000_0000);
            let (mut sim, fw, _) = load_or_create(Some(&session), None, None, "0")?;
            let (out, base_out, enabled_out, timeout_out) = {
                let w = sim.peripherals.watchdog.get_or_insert_with(|| ns_sim::Watchdog::new(base_num));
                w.base = base_num;
                if let Some(v) = enabled {
                    w.enabled = v;
                }
                if let Some(v) = timeout {
                    w.timeout_cycles = v;
                }
                let out = serde_json::json!({
                    "base": w.base, "enabled": w.enabled, "timeoutCycles": w.timeout_cycles,
                    "elapsed": w.elapsed, "resetCount": w.reset_count, "lastResetPc": w.last_reset_pc,
                });
                (out, w.base, w.enabled, w.timeout_cycles)
            };
            let s = Session::capture(&sim, fw);
            s.save(&session)?;
            if json {
                print_json(&out)?;
            } else {
                println!(
                    "看门狗配置: base=0x{:08x} enabled={} timeout={}",
                    base_out, enabled_out, timeout_out
                );
            }
            Ok(())
        }
        WatchdogSub::Reset { session, entry, json } => {
            let (mut sim, fw, _) = load_or_create(Some(&session), None, None, "0")?;
            let base = sim
                .peripherals
                .watchdog
                .as_ref()
                .map(|w| w.base)
                .unwrap_or(0x4000_0000);
            if sim.peripherals.watchdog.is_none() {
                sim.peripherals.watchdog = Some(ns_sim::Watchdog::new(base));
            }
            let reset_pc = sim.program_counter();
            // Record the watchdog-triggered reset event.
            {
                let w = sim.peripherals.watchdog.as_mut().unwrap();
                w.reset_count = w.reset_count.saturating_add(1);
                w.last_reset_pc = reset_pc;
                w.elapsed = 0;
            }
            let entry_num = entry
                .as_deref()
                .map(parse_num)
                .transpose()?
                .unwrap_or(default_reset_entry(&sim, fw.as_ref())?);
            sim.reset(entry_num);
            let s = Session::capture(&sim, fw);
            s.save(&session)?;
            let w = sim.peripherals.watchdog.as_ref().unwrap();
            if json {
                print_json(&serde_json::json!({
                    "resetCount": w.reset_count,
                    "lastResetPc": w.last_reset_pc,
                    "pc": sim.program_counter(),
                }))?;
            } else {
                println!(
                    "看门狗复位 @ 0x{:08x}（来自 0x{:08x}，累计 {} 次）",
                    sim.program_counter(),
                    reset_pc,
                    w.reset_count
                );
            }
            Ok(())
        }
    }
}

/// CSR field accessor helpers (RISC-V machine-mode registers).
fn csr_field(name: &str, sim: &Simulator) -> Option<u32> {
    match name.to_ascii_lowercase().as_str() {
        "mstatus" => Some(sim.csr.mstatus),
        "mtvec" => Some(sim.csr.mtvec),
        "mepc" => Some(sim.csr.mepc),
        "mcause" => Some(sim.csr.mcause),
        "mie" => Some(sim.csr.mie),
        "mhartid" => Some(sim.csr.mhartid),
        _ => None,
    }
}

fn csr_field_mut(name: &str, sim: &mut Simulator, value: u32) -> bool {
    match name.to_ascii_lowercase().as_str() {
        "mstatus" => sim.csr.mstatus = value,
        "mtvec" => sim.csr.mtvec = value,
        "mepc" => sim.csr.mepc = value,
        "mcause" => sim.csr.mcause = value,
        "mie" => sim.csr.mie = value,
        "mhartid" => sim.csr.mhartid = value,
        _ => return false,
    }
    true
}

fn cmd_csr(sub: CsrSub) -> Result<()> {
    match sub {
        CsrSub::List { session, json } => {
            let sim = load_session_sim(&session)?;
            let names = ["mstatus", "mtvec", "mepc", "mcause", "mie", "mhartid"];
            let mut out = serde_json::Map::new();
            for name in names {
                out.insert(name.to_string(), serde_json::json!(csr_field(name, &sim)));
            }
            if json {
                print_json(&out)?;
            } else {
                for (name, value) in &out {
                    println!("{name:<8} 0x{:08x}", value.as_u64().unwrap_or(0));
                }
            }
            Ok(())
        }
        CsrSub::Get { session, name, name_flag, json } => {
            let name = name_flag.or(name).ok_or_else(|| anyhow::anyhow!("缺少 CSR 名"))?;
            let sim = load_session_sim(&session)?;
            let value = csr_field(&name, &sim).ok_or_else(|| anyhow::anyhow!("未知 CSR: {name}"))?;
            if json {
                print_json(&serde_json::json!({ "name": name, "value": value }))?;
            } else {
                println!("{name} = 0x{value:08x}");
            }
            Ok(())
        }
        CsrSub::Set { session, name, value, name_flag, value_flag, json } => {
            let name = name_flag.or(name).ok_or_else(|| anyhow::anyhow!("缺少 CSR 名"))?;
            let value = value_flag.or(value).ok_or_else(|| anyhow::anyhow!("缺少 CSR 值"))?;
            let val = parse_num(&value)?;
            let (mut sim, fw, _) = load_or_create(Some(&session), None, None, "0")?;
            if !csr_field_mut(&name, &mut sim, val) {
                bail!("未知 CSR: {name}");
            }
            let s = Session::capture(&sim, fw);
            s.save(&session)?;
            if json {
                print_json(&serde_json::json!({ "name": name, "value": val }))?;
            } else {
                println!("{name} = 0x{val:08x}");
            }
            Ok(())
        }
    }
}
// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Read the ELF `e_machine` field (ELF32 at offset 18, ELF64 at offset 20).
fn elf_machine(bytes: &[u8]) -> Option<u16> {
    if bytes.len() < 20 || !bytes.starts_with(b"\x7fELF") {
        return None;
    }
    let is64 = bytes[4] == 2;
    let off = if is64 { 20 } else { 18 };
    if bytes.len() < off + 2 {
        return None;
    }
    Some(u16::from_le_bytes([bytes[off], bytes[off + 1]]))
}

/// `firmware info`: report the format, detected architecture, entry point and
/// the symbol table of a firmware image without needing a session.
fn cmd_firmware_info(firmware: &Path, arch_arg: Option<&str>, json: bool) -> Result<()> {
    let value = firmware_info_value(firmware, arch_arg)?;
    if json {
        print_json(&value)?;
        return Ok(());
    }
    println!("固件: {}", value["path"].as_str().unwrap_or(""));
    println!(
        "  格式 {}  大小 {} 字节  架构 {}  入口 0x{:08x}  {}",
        value["format"].as_str().unwrap_or(""),
        value["bytes"].as_u64().unwrap_or(0),
        value["arch"].as_str().unwrap_or(""),
        value["entry"].as_u64().unwrap_or(0) as u32,
        value["entryText"].as_str().unwrap_or("")
    );
    println!(
        "  符号 {} 个  源文件 {} 个",
        value["symbolCount"].as_u64().unwrap_or(0),
        value["sourceFiles"].as_u64().unwrap_or(0)
    );
    for s in value["symbols"].as_array().unwrap().iter().take(12) {
        println!(
            "    {:<32} 0x{:08x}",
            s["name"].as_str().unwrap_or(""),
            s["address"].as_u64().unwrap_or(0)
        );
    }
    if value["symbolCount"].as_u64().unwrap_or(0) > 12 {
        println!("    ...（共 {} 个）", value["symbolCount"].as_u64().unwrap_or(0));
    }
    Ok(())
}

/// Machine-readable firmware metadata shared by the CLI and the MCP server.
pub(crate) fn firmware_info_value(
    firmware: &Path,
    arch_arg: Option<&str>,
) -> Result<serde_json::Value> {
    let format = detect_format(firmware);
    let bytes = std::fs::read(firmware)
        .with_context(|| format!("读取固件 {} 失败", firmware.display()))?;
    let arch = if let Some(a) = arch_arg {
        arch_from_arg(Some(a))?
    } else if format == FirmwareFormat::Elf {
        let machine = elf_machine(&bytes)
            .ok_or_else(|| anyhow::anyhow!("无法读取 ELF e_machine"))?;
        Arch::from_elf_machine(machine)
            .ok_or_else(|| anyhow::anyhow!("不支持的 ELF 机器码 {machine}（无法确定架构）"))?
    } else {
        Arch::RiscV
    };
    let mut sim = Simulator::for_arch(arch)?;
    load_firmware_into(&mut sim, firmware, 0)
        .with_context(|| format!("加载固件 {} 失败", firmware.display()))?;
    let entry = sim.program_counter();
    let first_text = {
        let read = |a: u32| -> u8 { read_code_byte(&sim, a) };
        ns_sim::disassemble_one_state(arch, entry, &read, sim.arm_state())
    };
    let symbols: Vec<_> = sim
        .symbols
        .iter()
        .take(200)
        .map(|s| {
            serde_json::json!({
                "name": s.name,
                "address": s.address,
                "size": s.size,
            })
        })
        .collect();
    let machine = elf_machine(&bytes);
    Ok(serde_json::json!({
        "path": firmware.display().to_string(),
        "format": format.label(),
        "bytes": bytes.len(),
        "arch": session::arch_name(arch),
        "entry": entry,
        "entryText": first_text,
        "machine": machine,
        "symbolCount": sim.symbols.len(),
        "sourceFiles": sim.source_files.len(),
        "symbols": symbols,
    }))
}

/// `session info`: show persisted-session metadata in a compact table.
fn cmd_session_info(args: SessionInfoArgs) -> Result<()> {
    let value = Session::load(&args.file)?.info_value(Some(&args.file));
    if args.json {
        print_json(&value)?;
        return Ok(());
    }
    let periph = &value["peripherals"];
    let fw = value["firmware"].as_object();
    println!("会话: {}", args.file.display());
    println!(
        "  架构 {}  PC 0x{:08x}  周期 {}  停机 {}",
        value["arch"],
        value["pc"].as_u64().unwrap_or(0) as u32,
        value["cycleCount"].as_u64().unwrap_or(0),
        value["halted"].as_bool().unwrap_or(false)
    );
    println!(
        "  寄存器 {} 个  断点 {} 个（启用 {}）  内存字节 {}  覆盖写字节 {}",
        value["registers"].as_u64().unwrap_or(0),
        value["breakpoints"].as_u64().unwrap_or(0),
        value["enabledBreakpoints"].as_u64().unwrap_or(0),
        value["memoryBytes"].as_u64().unwrap_or(0),
        value["memoryOverlayBytes"].as_u64().unwrap_or(0)
    );
    println!(
        "  UART 基址 0x{:08x}  GPIO 基址 0x{:08x}  定时器 {}  外部寄存器 {}  看门狗 {}",
        periph["uartBase"].as_u64().unwrap_or(0) as u32,
        periph["gpioBase"].as_u64().unwrap_or(0) as u32,
        periph["timers"].as_u64().unwrap_or(0),
        periph["externalRegs"].as_u64().unwrap_or(0),
        if periph["watchdog"].as_bool().unwrap_or(false) {
            "已启用"
        } else {
            "未启用"
        }
    );
    if let Some(f) = fw {
        println!(
            "  固件 {} @ 0x{:08x} ({})",
            f.get("path").and_then(|v| v.as_str()).unwrap_or(""),
            f.get("base").and_then(|v| v.as_u64()).unwrap_or(0) as u32,
            f.get("format").and_then(|v| v.as_str()).unwrap_or("")
        );
    }
    Ok(())
}

/// `session diff`: list every difference between two session files.
fn cmd_session_diff(args: SessionDiffArgs) -> Result<()> {
    let value = session::session_diff(&args.a, &args.b, args.limit)?;
    if args.json {
        print_json(&value)?;
        return Ok(());
    }
    let summary = &value["summary"];
    println!(
        "A = {} ({})",
        args.a.display(),
        value["a"]["arch"].as_str().unwrap_or("?")
    );
    println!(
        "B = {} ({})",
        args.b.display(),
        value["b"]["arch"].as_str().unwrap_or("?")
    );
    println!(
        "差异: 标量 {}  寄存器 {}  断点 {}  内存 {}{}",
        summary["scalar"].as_u64().unwrap_or(0),
        summary["register"].as_u64().unwrap_or(0),
        summary["breakpoint"].as_u64().unwrap_or(0),
        summary["memory"].as_u64().unwrap_or(0),
        if summary["truncated"].as_u64().unwrap_or(0) > 0 {
            format!("（截断 {} 条，仅显示前 {} 条）", summary["truncated"].as_u64().unwrap_or(0), args.limit)
        } else {
            String::new()
        }
    );
    if value["same"].as_bool().unwrap_or(false) {
        println!("两个会话完全一致。");
        return Ok(());
    }
    for d in value["differences"].as_array().unwrap() {
        println!(
            "  [{:<10}] {:<20} A={}  B={}",
            d["kind"].as_str().unwrap_or(""),
            d["name"].as_str().unwrap_or(""),
            d["a"],
            d["b"]
        );
    }
    Ok(())
}

/// The requested analysis output mode.
enum AnalyzeKind {
    Trace,
    Coverage,
    Profile,
}

/// Run the simulator while collecting per-instruction trace, coverage and a
/// mnemonic histogram, then print or serialize the requested view.
fn cmd_analyze(args: AnalyzeArgs, kind: AnalyzeKind) -> Result<()> {
    let (mut sim, fw, _) = load_or_create(
        args.session.as_deref(),
        args.arch.as_deref(),
        args.firmware.as_deref(),
        &args.base,
    )?;
    // `--until ADDR` installs a temporary breakpoint removed before saving.
    let until_addr = resolve_until(&sim, &args.until, &args.until_symbol)?;
    if let Some(addr) = until_addr {
        if addr > sim.pc_limit() {
            bail!("until 地址 0x{addr:08x} 超出 {} 的 PC 可寻址范围", sim.arch.label());
        }
    }
    let temp_bp = until_addr.map(|addr| sim.set_breakpoint(addr)).unwrap_or(false);
    let limit = args.steps.unwrap_or(args.max_steps);
    let collect_trace = matches!(kind, AnalyzeKind::Trace);
    let mut watch = WatchTracker::from_session(args.session.as_deref())?;
    let mut hook = |before: bool, s: &Simulator| -> Option<u32> {
        if before {
            watch.snapshot(s);
            None
        } else {
            watch.check(s)
        }
    };
    let analysis = analyze_run(
        &mut sim,
        limit,
        collect_trace,
        args.regs,
        args.max_cycles,
        Some(&mut hook),
    );
    if temp_bp {
        if let Some(addr) = until_addr {
            sim.clear_breakpoint(addr);
        }
    }
    if let Some(sp) = args.session.as_ref() {
        save_session_with_watchpoints(sp, &sim, fw, &watch)?;
    }
    let arch_name = session::arch_name(sim.arch);
    let payload: serde_json::Value = match kind {
        AnalyzeKind::Trace => serde_json::json!({
            "arch": arch_name,
            "steps": analysis.steps,
            "cycleCount": analysis.cycle_count,
            "result": analysis.result,
            "watchHit": analysis.watch_hit,
            "trace": &analysis.trace,
        }),
        AnalyzeKind::Coverage => {
            let items: Vec<_> = analysis
                .coverage
                .iter()
                .map(|(pc, hits)| serde_json::json!({ "pc": pc, "hits": hits }))
                .collect();
            serde_json::json!({
                "arch": arch_name,
                "steps": analysis.steps,
                "cycleCount": analysis.cycle_count,
                "result": analysis.result,
                "watchHit": analysis.watch_hit,
                "uniquePcs": items.len(),
                "coverage": items,
            })
        }
        AnalyzeKind::Profile => {
            let items: Vec<_> = analysis
                .histogram
                .iter()
                .map(|(mnemonic, count)| serde_json::json!({ "mnemonic": mnemonic, "count": count }))
                .collect();
            serde_json::json!({
                "arch": arch_name,
                "steps": analysis.steps,
                "cycleCount": analysis.cycle_count,
                "result": analysis.result,
                "watchHit": analysis.watch_hit,
                "uniqueMnemonics": items.len(),
                "histogram": items,
            })
        }
    };
    if args.json {
        emit_out(args.out.as_deref(), &serde_json::to_string_pretty(&payload)?)?;
        return Ok(());
    }
    let mut text = String::new();
    match kind {
        AnalyzeKind::Trace => {
            for e in &analysis.trace {
                let bytes: String =
                    e.bytes.iter().map(|b| format!("{b:02x} ")).collect();
                let src = if e.source.is_empty() {
                    String::new()
                } else {
                    format!(" ; {}", e.source)
                };
                text.push_str(&format!("0x{:08x}: {:<40} ; {}{src}\n", e.pc, e.text, bytes));
            }
            text.push_str(&format!(
                "停止: {}（{} 条指令，周期 {}）\n",
                analysis.result, analysis.steps, analysis.cycle_count
            ));
        }
        AnalyzeKind::Coverage => {
            text.push_str(&format!(
                "已执行 {} 条指令，覆盖 {} 个唯一 PC，停止: {}，周期 {}:\n",
                analysis.steps,
                analysis.coverage.len(),
                analysis.result,
                analysis.cycle_count
            ));
            for (pc, hits) in &analysis.coverage {
                text.push_str(&format!("  0x{pc:08x} x{hits}\n"));
            }
        }
        AnalyzeKind::Profile => {
            text.push_str(&format!(
                "已执行 {} 条指令，{} 种助记符，停止: {}，周期 {}:\n",
                analysis.steps,
                analysis.histogram.len(),
                analysis.result,
                analysis.cycle_count
            ));
            for (mnemonic, count) in &analysis.histogram {
                text.push_str(&format!("  {mnemonic:<40} x{count}\n"));
            }
        }
    }
    emit_out(args.out.as_deref(), &text)?;
    Ok(())
}

/// Write `text` to `out` (printing a confirmation line) or to stdout.
fn emit_out(out: Option<&Path>, text: &str) -> Result<()> {
    match out {
        Some(path) => {
            std::fs::write(path, text)
                .with_context(|| format!("写入 {} 失败", path.display()))?;
            println!("已写出 -> {}（{} 字节）", path.display(), text.len());
        }
        None => print!("{text}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_num_hex_and_dec() {
        assert_eq!(parse_num("0x10").unwrap(), 16);
        assert_eq!(parse_num("16").unwrap(), 16);
        assert_eq!(parse_num("0b1010").unwrap(), 10);
        assert!(parse_num("zz").is_err());
    }

    #[test]
    fn arch_roundtrip() {
        for (name, arch) in [
            ("riscv", Arch::RiscV),
            ("arm", Arch::Arm),
            ("mcs51", Arch::Mcs51),
            ("mcs251", Arch::Mcs251),
            ("c166", Arch::C166),
            ("xtensa", Arch::Xtensa),
            ("avr", Arch::Avr),
            ("msp430", Arch::Msp430),
            ("v850", Arch::V850),
            ("stm8", Arch::Stm8),
            ("rx", Arch::Rx),
            ("rl78", Arch::Rl78),
        ] {
            assert_eq!(session::parse_arch(name).unwrap(), arch);
            assert_eq!(session::parse_arch(session::arch_name(arch)).unwrap(), arch);
        }
    }
}
