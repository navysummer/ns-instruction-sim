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

mod session;
mod sim;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};

use ns_sim::{Arch, Simulator, StepResult};
use session::{FirmwareState, Session};
use sim::{default_mem_base, detect_format, load_firmware_into, parse_num, read_mem_byte, write_mem_byte};

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
    Version,
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
}

#[derive(Subcommand)]
enum ArchSub {
    /// 列出所有支持的架构与设备映射。
    List {
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum DeviceSub {
    /// 列出内置设备目录。
    List {
        #[arg(long)]
        json: bool,
    },
    /// 显示设备信息（内存映射、核心）。
    Info {
        device: String,
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
    #[arg(short, long)]
    session: PathBuf,
    /// 运行到指定地址（临时断点，不持久化）。
    #[arg(long)]
    until: Option<String>,
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
        #[arg(short, long)]
        session: Option<PathBuf>,
        #[arg(short, long)]
        arch: Option<String>,
        /// 按名称子串筛选（大小写不敏感，如 `s` 或 `x1`）。
        #[arg(long)]
        filter: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// 读取单个寄存器。
    Get {
        #[arg(short, long)]
        session: Option<PathBuf>,
        #[arg(short, long)]
        arch: Option<String>,
        /// 寄存器名（如 x1、a0、R3、SP）。位置参数或 --name。
        #[arg(index = 1)]
        name: Option<String>,
        /// 寄存器名（--name 形式）。
        #[arg(long = "name", conflicts_with = "name")]
        name_flag: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// 写单个寄存器。
    Set {
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
        /// 输出原始二进制到文件。
        #[arg(long)]
        out: Option<PathBuf>,
        /// 输出 JSON（地址/宽度/字节数组）。
        #[arg(long)]
        json: bool,
    },
    /// 写内存（字节或从文件）。
    Write {
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
        #[arg(long)]
        json: bool,
    },
    /// 填充内存区域。
    Fill {
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
        #[arg(long)]
        json: bool,
    },
    /// 在内存区域搜索字节模式（找到所有偏移）。
    Search {
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
        /// 搜索模式字节（空格分隔，支持 0xNN）。
        #[arg(long, value_delimiter = ' ', num_args = 1..)]
        pattern: Vec<String>,
        #[arg(long)]
        json: bool,
    },
    /// 加载固件到内存（等价于 run 的加载步骤，不执行）。
    Load {
        #[arg(short, long)]
        session: PathBuf,
        #[arg(short, long)]
        firmware: PathBuf,
        #[arg(short, long)]
        arch: Option<String>,
        #[arg(long, default_value = "0")]
        base: String,
        #[arg(long)]
        json: bool,
    },
    /// 导出内存区域（C 数组或十六进制文本）。
    Dump {
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
        /// 输出格式：`c`（C 数组）或 `hex`（十六进制文本）。
        #[arg(long, default_value = "c")]
        format: String,
        /// 写入文件路径（缺省打印到 stdout）。
        #[arg(long)]
        out: Option<PathBuf>,
        /// 以 JSON 输出地址/长度/格式/名称/字节数组。
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
        #[arg(short, long)]
        session: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// 设置断点。
    Set {
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
        #[arg(long)]
        json: bool,
    },
    /// 清除断点。
    Clear {
        #[arg(short, long)]
        session: PathBuf,
        /// 地址（位置参数或 --addr）。
        #[arg(index = 1)]
        addr: Option<String>,
        /// 地址（--addr 形式）。
        #[arg(long = "addr", conflicts_with = "addr")]
        addr_flag: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// 清除全部断点。
    ClearAll {
        #[arg(short, long)]
        session: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// 启用断点。
    Enable {
        #[arg(short, long)]
        session: PathBuf,
        /// 地址（位置参数或 --addr）。
        #[arg(index = 1)]
        addr: Option<String>,
        /// 地址（--addr 形式）。
        #[arg(long = "addr", conflicts_with = "addr")]
        addr_flag: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// 禁用断点（保留在列表中但不再暂停）。
    Disable {
        #[arg(short, long)]
        session: PathBuf,
        /// 地址（位置参数或 --addr）。
        #[arg(index = 1)]
        addr: Option<String>,
        /// 地址（--addr 形式）。
        #[arg(long = "addr", conflicts_with = "addr")]
        addr_flag: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
struct DisasmArgs {
    #[arg(short, long)]
    session: Option<PathBuf>,
    #[arg(short, long)]
    arch: Option<String>,
    /// 起始地址（默认当前 PC）。
    #[arg(short = 'd', long)]
    addr: Option<String>,
    /// 指令条数（默认 8）。
    #[arg(long, default_value = "8")]
    count: usize,
    /// ARM 状态：`a32` 或 `thumb`（仅 ARM；默认取内核当前状态）。
    #[arg(long, value_parser = ["a32", "thumb"])]
    arm_state: Option<String>,
    /// 标注每行来源（file:line，来自 ELF 行表/符号回退）。
    #[arg(long)]
    source: bool,
    /// 同时显示每条指令的原始字节（十六进制）。
    #[arg(long)]
    raw: bool,
    /// 输出 JSON 数组（地址/反汇编/原始字节/来源）。
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct ResetArgs {
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
    #[arg(short, long)]
    session: Option<PathBuf>,
    #[arg(short, long)]
    arch: Option<String>,
    #[arg(long)]
    firmware: Option<PathBuf>,
    #[arg(long, default_value = "0")]
    base: String,
    /// 输出 JSON（默认）。
    #[arg(long)]
    json: bool,
    /// 逗号分隔的 JSON 字段子集（如 `pc,registers,csr`），仅 `--json` 生效。
    #[arg(long, value_delimiter = ',')]
    fields: Vec<String>,
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
        #[arg(short, long)]
        session: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// 配置虚拟外设基址（UART/GPIO，可选）。
    Config {
        #[arg(short, long)]
        session: PathBuf,
        /// UART 寄存器基址（默认 0x4000_0000）。
        #[arg(long)]
        uart_base: Option<String>,
        /// GPIO 寄存器基址（默认 0x4000_2000）。
        #[arg(long)]
        gpio_base: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// 从 SVD 文件或内置设备加载芯片寄存器（地址 -> 复位值）到仿真器。
    SvdLoad {
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
        #[arg(long)]
        json: bool,
    },
    /// 读取 UART 输出字节数 / 清空。
    Uart {
        #[arg(short, long)]
        session: PathBuf,
        /// 清空输出缓冲。
        #[arg(long)]
        clear: bool,
        #[arg(long)]
        json: bool,
    },
    /// 设置 GPIO 输入引脚值。
    Gpio {
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
        #[arg(long)]
        json: bool,
    },
    /// 查询/配置虚拟定时器。
    Timer {
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
        #[arg(long)]
        json: bool,
    },
    /// 读取外设内存映射寄存器（触发外设读）。
    RegRead {
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
        #[arg(long)]
        json: bool,
    },
    /// 列出会话中已加载（SVD）的外设寄存器。
    RegList {
        #[arg(short, long)]
        session: PathBuf,
        /// 用内置设备键注释寄存器名（如 stm32f103c8，可选）。
        #[arg(long)]
        device: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// 写入外设内存映射寄存器（触发外设写，如 UART 输出、TIMER 控制）。
    RegWrite {
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
        #[arg(long)]
        json: bool,
    },
    /// 读取外设寄存器的命名位域（SVD field）。
    FieldGet {
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
        #[arg(long)]
        json: bool,
    },
    /// 写外设寄存器的命名位域（SVD field），保留其他位。
    FieldSet {
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
        #[arg(short, long)]
        session: PathBuf,
        /// 按名称子串过滤（大小写不敏感）。
        #[arg(long)]
        filter: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// 查询地址处最接近的符号。
    At {
        #[arg(short, long)]
        session: PathBuf,
        /// 地址（位置参数或 --addr）。
        #[arg(index = 1)]
        addr: Option<String>,
        #[arg(long = "addr", conflicts_with = "addr")]
        addr_flag: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// 地址 → (源文件, 行号) 映射。
    Line {
        #[arg(short, long)]
        session: PathBuf,
        /// 地址（位置参数或 --addr）。
        #[arg(index = 1)]
        addr: Option<String>,
        #[arg(long = "addr", conflicts_with = "addr")]
        addr_flag: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// (源文件, 行号) → 地址 映射。
    Addr {
        #[arg(short, long)]
        session: PathBuf,
        /// 源文件索引（0 基）。
        #[arg(long)]
        file: usize,
        /// 源行号（1 基）。
        #[arg(long)]
        line: u32,
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
        #[arg(short, long)]
        session: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// 估算指定地址的指令周期数。
    Estimate {
        #[arg(short, long)]
        session: PathBuf,
        /// 指令地址（位置参数或 --addr）。
        #[arg(index = 1)]
        addr: Option<String>,
        #[arg(long = "addr", conflicts_with = "addr")]
        addr_flag: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// 推进外设 N 个核心周期（不执行指令）。
    Tick {
        #[arg(short, long)]
        session: PathBuf,
        /// 周期数（默认 1）。
        #[arg(long, default_value = "1")]
        cycles: u64,
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
        #[arg(short, long)]
        session: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// 查询当前挂起/使能的中断。
    Pending {
        #[arg(short, long)]
        session: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// 设置/清除外设中断线。
    Set {
        #[arg(short, long)]
        session: PathBuf,
        /// 中断线 0..15（位置参数或 --line）。
        #[arg(index = 1)]
        line: Option<usize>,
        #[arg(long = "line", conflicts_with = "line")]
        line_flag: Option<usize>,
        /// 置位/清除（位置参数或 --value，默认 1）。
        #[arg(index = 2)]
        value: Option<u32>,
        #[arg(long = "value", conflicts_with = "value")]
        value_flag: Option<u32>,
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
        #[arg(short, long)]
        session: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// 喂狗（写 0xAA55 重载超时）。
    Feed {
        #[arg(short, long)]
        session: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// 配置看门狗（启用/禁用、超时周期、基址）。
    Config {
        #[arg(short, long)]
        session: PathBuf,
        #[arg(long)]
        enabled: Option<bool>,
        /// 超时周期数（0 = 禁用）。
        #[arg(long)]
        timeout: Option<u64>,
        /// 寄存器基址（默认 0x4000_0000）。
        #[arg(long)]
        base: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// 强制触发看门狗复位（记录复位事件并复位核心）。
    Reset {
        #[arg(short, long)]
        session: PathBuf,
        /// 复位后 PC（默认当前 PC）。
        #[arg(long)]
        entry: Option<String>,
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
        #[arg(short, long)]
        session: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// 读取单个 CSR。
    Get {
        #[arg(short, long)]
        session: PathBuf,
        /// CSR 名（mstatus/mtvec/mepc/mcause/mie/mhartid）。
        #[arg(index = 1)]
        name: Option<String>,
        #[arg(long = "name", conflicts_with = "name")]
        name_flag: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// 写入单个 CSR。
    Set {
        #[arg(short, long)]
        session: PathBuf,
        #[arg(index = 1)]
        name: Option<String>,
        #[arg(index = 2)]
        value: Option<String>,
        #[arg(long = "name", conflicts_with = "name")]
        name_flag: Option<String>,
        #[arg(long = "value", conflicts_with = "value")]
        value_flag: Option<String>,
        #[arg(long)]
        json: bool,
    },
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
        Command::Version => {
            println!("ns-instruction-sim {}", env!("CARGO_PKG_VERSION"));
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
        Command::Run(args) => cmd_run(args),
        Command::Step(args) => cmd_step(args, StepKind::Step),
        Command::StepOver(args) => cmd_step(args, StepKind::StepOver),
        Command::StepOut(args) => cmd_step(args, StepKind::StepOut),
        Command::Continue(args) => cmd_continue(args),
        Command::Reg(RegArgs { sub }) => cmd_reg(sub),
        Command::Mem(MemArgs { sub }) => cmd_mem(sub),
        Command::Bp(BpArgs { sub }) => cmd_bp(sub),
        Command::Disasm(args) => cmd_disasm(args),
        Command::Reset(args) => cmd_reset(args),
        Command::State(args) => cmd_state(args),
        Command::Periph(PeriphArgs { sub }) => cmd_periph(sub),
        Command::Symbol(SymbolArgs { sub }) => cmd_symbol(sub),
        Command::Cycle(CycleArgs { sub }) => cmd_cycle(sub),
        Command::Interrupt(InterruptArgs { sub }) => cmd_interrupt(sub),
        Command::Watchdog(WatchdogArgs { sub }) => cmd_watchdog(sub),
        Command::Csr(CsrArgs { sub }) => cmd_csr(sub),
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
fn load_or_create(
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
    Ok((sim, fw, session_path.map(|p| p.to_path_buf())))
}

/// Common state output for `run`/`step`/`continue`.
#[derive(serde::Serialize)]
struct StateOutput {
    arch: String,
    pc: u32,
    #[serde(rename = "cycleCount")]
    cycle_count: u64,
    halted: bool,
    #[serde(rename = "lastResult")]
    last_result: String,
    registers: BTreeMap<String, u32>,
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

fn state_output(sim: &Simulator, result: StepResult, firmware: Option<&FirmwareState>) -> StateOutput {
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
    let until_addr = args.until.as_deref().map(parse_num).transpose()?;
    let temp_bp = until_addr.map(|addr| sim.set_breakpoint(addr)).unwrap_or(false);
    // `--steps N` is a bounded "run exactly N instructions" mode; otherwise
    // run to completion with the `--max-steps` upper bound.
    let result = if args.trace {
        trace_run(&mut sim, args.steps, args.max_steps)
    } else {
        match args.steps {
            Some(n) => sim.run(n),
            None => sim.run(args.max_steps),
        }
    };
    if temp_bp {
        if let Some(addr) = until_addr {
            sim.clear_breakpoint(addr);
        }
    }
    let output = state_output(&sim, result, fw_state.as_ref());

    if let Some(session_path) = args.save_session.as_ref().or_else(|| args.session.as_ref()) {
        let session = Session::capture(&sim, fw_state);
        session.save(session_path)?;
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
fn trace_run(sim: &mut Simulator, steps: Option<u64>, max_steps: u64) -> StepResult {
    let limit = steps.unwrap_or(max_steps);
    let mut count = 0u64;
    loop {
        if count >= limit {
            return StepResult::Advanced;
        }
        let pc_before = sim.program_counter();
        let result = sim.step();
        let insn = ns_sim::disassemble_one_state(
            sim.arch,
            pc_before,
            &|addr| sim.read_u8(addr),
            sim.arm_state(),
        );
        let src = source_annotation(sim, pc_before);
        println!(
            "[{count}] 0x{pc_before:08x}: {insn}{src}  -> 0x{:08x} {}",
            sim.program_counter(),
            fmt_step_result(result)
        );
        count += 1;
        if result != StepResult::Advanced {
            return result;
        }
    }
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
    let result = match kind {
        StepKind::Step => sim.step(),
        StepKind::StepOver => sim.step_over(args.max_steps),
        StepKind::StepOut => sim.step_out(args.max_steps),
    };
    let output = state_output(&sim, result, fw.as_ref());
    if let Some(sp) = &session_path {
        let session = Session::capture(&sim, fw);
        session.save(sp)?;
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
    let until_addr = args.until.as_deref().map(parse_num).transpose()?;
    let temp_bp = until_addr.map(|addr| sim.set_breakpoint(addr)).unwrap_or(false);
    let result = if args.trace {
        trace_run(&mut sim, None, args.max_steps)
    } else {
        sim.run(args.max_steps)
    };
    if temp_bp {
        if let Some(addr) = until_addr {
            sim.clear_breakpoint(addr);
        }
    }
    let output = state_output(&sim, result, fw.as_ref());
    if args.save {
        let session = Session::capture(&sim, fw);
        session.save(&args.session)?;
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
        MemSub::Read { session, arch, addr, addr_flag, len, width, out, json } => {
            let addr = addr_flag.or(addr).ok_or_else(|| anyhow::anyhow!("缺少地址"))?;
            let addr_num = parse_num(&addr)?;
            let (sim, _fw, _) = load_or_create(Some(&session), arch.as_deref(), None, "0")?;
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
                    bytes.push(read_mem_byte(&sim, a.wrapping_add(j as u32)));
                }
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
        MemSub::Write { session, arch, addr, addr_flag, bytes, width, values, file, json } => {
            let addr = addr_flag.or(addr).ok_or_else(|| anyhow::anyhow!("缺少地址"))?;
            let addr_num = parse_num(&addr)?;
            let mut data = Vec::new();
            let elem_size = match width.as_str() {
                "byte" => 1usize,
                "half" => 2usize,
                "word" => 4usize,
                other => bail!("不支持的内存宽度: {other}（支持 byte/half/word）"),
            };
            if let Some(f) = &file {
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
            let mut overlay = Vec::new();
            for (i, b) in data.iter().enumerate() {
                let a = addr_num.wrapping_add(i as u32);
                write_mem_byte(&mut sim, a, *b);
                overlay.push((a, *b));
            }
            let mut s = Session::capture(&sim, fw);
            s.memory_overlay.extend(overlay);
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
        MemSub::Fill { session, arch, addr, addr_flag, len, value, value_flag, json } => {
            let addr = addr_flag.or(addr).ok_or_else(|| anyhow::anyhow!("缺少地址"))?;
            let value = value_flag.or(value).ok_or_else(|| anyhow::anyhow!("缺少填充字节"))?;
            let addr_num = parse_num(&addr)?;
            let value_num = parse_num(&value)?;
            if value_num > 0xff {
                bail!("填充字节超出 0..=255: {}", value);
            }
            let value_byte = value_num as u8;
            let (mut sim, fw, _) = load_or_create(Some(&session), arch.as_deref(), None, "0")?;
            let bytes = vec![value_byte; len];
            let mut overlay = Vec::new();
            for (i, b) in bytes.iter().enumerate() {
                let a = addr_num.wrapping_add(i as u32);
                write_mem_byte(&mut sim, a, *b);
                overlay.push((a, *b));
            }
            let mut s = Session::capture(&sim, fw);
            s.memory_overlay.extend(overlay);
            s.save(&session)?;
            if json {
                print_json(&serde_json::json!({
                    "address": addr_num,
                    "len": len,
                    "value": value_byte,
                }))?;
            } else {
                println!("填充 {} 字节 @ 0x{:08x} = 0x{value_byte:02x}", len, addr_num);
            }
            Ok(())
        }
        MemSub::Search { session, arch, addr, addr_flag, len, pattern, json } => {
            let addr_str = addr_flag
                .as_deref()
                .or(addr.as_deref())
                .ok_or_else(|| anyhow::anyhow!("缺少地址"))?;
            let addr_num = parse_num(addr_str)?;
            if pattern.is_empty() {
                bail!("--pattern 至少需要一个字节");
            }
            let mut pat = Vec::new();
            for p in &pattern {
                let v = parse_num(p)?;
                if v > 0xff {
                    bail!("模式字节超出 0..=255: {}", p);
                }
                pat.push(v as u8);
            }
            let (sim, _, _) = load_or_create(Some(&session), arch.as_deref(), None, "0")?;
            let mut matches: Vec<u32> = Vec::new();
            if pat.is_empty() || len < pat.len() {
                // Nothing to search.
            } else {
                let window = pat.len();
                for i in 0..=(len - window) {
                    let mut ok = true;
                    for (j, b) in pat.iter().enumerate() {
                        if sim.read_u8(addr_num.wrapping_add((i + j) as u32)) != *b {
                            ok = false;
                            break;
                        }
                    }
                    if ok {
                        matches.push(addr_num.wrapping_add(i as u32));
                    }
                }
            }
            if json {
                print_json(&serde_json::json!({
                    "address": addr_num,
                    "count": matches.len(),
                    "matches": matches,
                }))?;
            } else {
                if matches.is_empty() {
                    println!("(未找到模式)");
                }
                for m in &matches {
                    println!("0x{m:08x}");
                }
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
                "hex" => hex_dump(addr_num, &bytes),
                other => bail!("不支持的内存导出格式: {other}（支持 c/hex）"),
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
    }
}

fn cmd_bp(sub: BpSub) -> Result<()> {
    match sub {
        BpSub::List { session, json } => {
            let s = Session::load(&session)?;
            let sim = s.apply(None).ok();
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
                    println!("0x{:08x}{extra}  hits={hits}", bp.address);
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
fn source_annotation(sim: &Simulator, address: u32) -> String {
    sim.line_for_address(address)
        .and_then(|(file, line)| sim.source_files.get(file).map(|f| (f, line)))
        .map(|(f, line)| format!("  ; {f}:{line}"))
        .unwrap_or_default()
}

fn cmd_disasm(args: DisasmArgs) -> Result<()> {
    let arch = resolve_arch(args.session.as_deref(), args.arch.as_deref())?;
    let mut pc = match args.addr {
        Some(a) => parse_num(&a)?,
        None => {
            if let Some(sp) = &args.session {
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
    // Build a simulator to read code bytes: load the session's firmware if
    // recorded, otherwise disassemble from a zeroed image (best-effort).
    let sim = if let Some(sp) = &args.session {
        if sp.exists() {
            let s = Session::load(sp)?;
            s.apply(None).unwrap_or_else(|_| Simulator::for_arch(arch).unwrap())
        } else {
            Simulator::for_arch(arch)?
        }
    } else {
        Simulator::for_arch(arch)?
    };
    let arm_state = match args.arm_state.as_deref() {
        Some("a32") => true,
        Some("thumb") => false,
        _ => sim.arm_state(),
    };
    let read = |addr: u32| -> u8 { sim.read_u8(addr) };
    let mut items = Vec::new();
    for _ in 0..args.count {
        let width = ns_sim::instruction_size(arch, pc, &read, arm_state);
        let text = ns_sim::disassemble_one_state(arch, pc, &read, arm_state);
        let raw = if args.raw {
            let mut s = String::new();
            for i in 0..width {
                if i > 0 {
                    s.push(' ');
                }
                s.push_str(&format!("{:02x}", sim.read_u8(pc.wrapping_add(i as u32))));
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
                    .map(|i| sim.read_u8(pc.wrapping_add(i as u32)))
                    .collect::<Vec<_>>(),
                "source": source_annotation(&sim, pc).trim().to_string(),
                "armState": arm_state,
            }));
        } else if args.source {
            let src = source_annotation(&sim, pc);
            println!("0x{pc:08x}: {text}{raw}  {src}");
        } else {
            println!("0x{pc:08x}: {text}{raw}");
        }
        pc = pc.wrapping_add(width);
    }
    if args.json {
        print_json(&items)?;
    }
    Ok(())
}

fn cmd_reset(args: ResetArgs) -> Result<()> {
    let (mut sim, fw, _) = load_or_create(Some(&args.session), None, None, "0")?;
    let entry = match args.entry {
        Some(e) => parse_num(&e)?,
        None => sim.program_counter(),
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
        if args.fields.is_empty() {
            print_json(&output)?;
        } else {
            let mut value = serde_json::to_value(&output)?;
            let obj = value
                .as_object_mut()
                .ok_or_else(|| anyhow::anyhow!("状态不是 JSON 对象"))?;
            obj.retain(|key, _| args.fields.iter().any(|f| f == key));
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
    }
    Ok(())
}

/// Resolve a peripheral-register address from either `--addr` or
/// `--name PERIPH.REG` (with `--device` selecting the built-in SVD).
fn resolve_reg_address(
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
fn resolve_reg_field(
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
                        sim.write_u32(addr, r.reset_value as u32);
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
        PeriphSub::Uart { session, clear, json } => {
            let (mut sim, fw, _) = load_or_create(Some(&session), None, None, "0")?;
            let text = String::from_utf8_lossy(&sim.peripherals.outputs).into_owned();
            let len = sim.peripherals.outputs.len();
            if clear {
                sim.peripherals.outputs.clear();
                let s = Session::capture(&sim, fw);
                s.save(&session)?;
            }
            if json {
                print_json(&serde_json::json!({
                    "len": len,
                    "text": text,
                    "cleared": clear,
                }))?;
            } else {
                println!("UART 输出 ({len} 字节): {text}");
            }
            Ok(())
        }
        PeriphSub::Gpio { session, pin, pin_flag, value, value_flag, json } => {
            let pin = pin_flag.or(pin).ok_or_else(|| anyhow::anyhow!("缺少引脚号"))?;
            let value = value_flag.or(value).ok_or_else(|| anyhow::anyhow!("缺少引脚值"))?;
            if pin > 31 {
                bail!("引脚号必须在 0..=31");
            }
            if value > 1 {
                bail!("引脚值必须是 0 或 1");
            }
            let (mut sim, fw, _) = load_or_create(Some(&session), None, None, "0")?;
            if value == 1 {
                sim.peripherals.gpio_in |= 1 << pin;
            } else {
                sim.peripherals.gpio_in &= !(1 << pin);
            }
            let s = Session::capture(&sim, fw);
            s.save(&session)?;
            if json {
                print_json(&serde_json::json!({
                    "pin": pin,
                    "value": value,
                    "gpioIn": sim.peripherals.gpio_in,
                }))?;
            } else {
                println!("GPIO IN 引脚 {pin} = {value}");
            }
            Ok(())
        }
        PeriphSub::Timer { session, index, enabled, prescaler, auto_reload, json } => {
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
                    bytes.push(sim.read_u8(addr_num.wrapping_add(i as u32)));
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
            sim.write_bytes(addr_num, &data);
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
            let reg_value = sim.peripheral_read_reg(addr).unwrap_or_else(|| sim.read_u32(addr));
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
            let cur = sim.peripheral_read_reg(addr).unwrap_or_else(|| sim.read_u32(addr));
            let new_reg =
                (cur & !(mask << bit_offset)) | ((new_bits << bit_offset) & (mask << bit_offset));
            let bytes = new_reg.to_le_bytes();
            sim.write_bytes(addr, &bytes);
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
        CycleSub::Estimate { session, addr, addr_flag, json } => {
            let addr = addr_flag.or(addr).ok_or_else(|| anyhow::anyhow!("缺少地址"))?;
            let addr_num = parse_num(&addr)?;
            let sim = load_session_sim(&session)?;
            let cycles = sim.cycles_for_instruction(addr_num);
            if json {
                print_json(&serde_json::json!({ "address": addr_num, "cycles": cycles }))?;
            } else {
                println!("0x{:08x}: {} 周期", addr_num, cycles);
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
            if sim.peripherals.watchdog.is_none() {
                sim.peripherals.watchdog = Some(ns_sim::Watchdog::new(base));
            }
            sim.write_u32(base, 0xAA55);
            let s = Session::capture(&sim, fw);
            s.save(&session)?;
            if json {
                print_json(&serde_json::json!({ "fed": true, "base": base }))?;
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
                .unwrap_or(reset_pc);
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
