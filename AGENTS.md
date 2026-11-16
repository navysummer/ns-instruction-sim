# ns-instruction-sim — 开发规范

> 跨平台嵌入式**指令级仿真命令行工具**。本文件是项目的**权威开发规范**，为后续开发提供范围、
> 架构与验收依据。技术栈：**Rust 2024 edition**。仿真核心源码**内嵌在本仓库**，不依赖任何外部
> IDE/工程仓库（尤其不依赖 `ns-embed-ide` 作为 path 依赖）。

---

## 1. 项目概述

| 项 | 内容 |
|---|---|
| 名称 | `ns-instruction-sim` |
| 定位 | 可被外部软件（CI、脚本、其他 IDE、测试框架）通过命令行完整驱动的**指令级仿真器** |
| 核心能力 | 12 种架构的固件加载、单步/步过/步出/连续运行、断点（条件/命中次数）、寄存器/内存读写、反汇编、周期计数、虚拟外设（UART/GPIO/TIMER/看门狗/中断控制器）、会话持久化 |
| 平台 | Windows / macOS / Linux |
| 交付形态 | 单个可执行文件 `ns-instruction-sim`，纯命令行，无 GUI 依赖 |
| 参考 | `ns-embed-ide` 的内建仿真核心（`ns-sim` / `ns-device`）——只复用能力模型，**不复用其工程结构** |

### 1.1 核心设计原则

1. **仿真代码自包含**：`ns-sim`（12 架构指令级内核）与 `ns-device`（SVD 设备目录）源码必须位于
   本仓库 `crates/` 下。禁止通过 `path = "../..."` 引用仓库外 crate；仿真能力由本仓库独立交付。
2. **完全开放式命令调用**：所有仿真操作（加载、执行、状态、寄存器、内存、断点、外设、反汇编、
   周期、中断、符号）都必须有对应子命令；外部调用方**不得**依赖内部实现细节，只能通过 CLI 契约。
3. **指令级仿真**：仿真以指令为粒度推进，单步可观察 PC、寄存器、内存、周期与外设状态变化；
   支持周期精确模式（外设按指令预估周期推进）。
4. **状态即会话**：仿真器状态（寄存器/内存/PC/断点/外设/固件引用）可序列化为 JSON 会话文件，
   任何时刻可保存、恢复、续跑——这是跨进程驱动仿真的基础。
5. **输出可编程**：默认人类可读，`--json` 输出结构化 JSON（字段稳定、可机器解析）；错误走 stderr
   且以非零退出码结束，不静默。
6. **参数尽可能全面**：每个命令暴露该操作的全部可控参数（地址、长度、索引、值、条件、命中次数、
   架构、基址、步数上限、周期模式、会话路径等），让外部能完成“完整调用”，不遗漏仿真能力。

### 1.2 工程纪律（所有修改必须遵守）

1. **零警告、零错误**：修改完成后 `cargo build` 必须 0 警告 0 错误、`cargo test` 全绿。禁止用
   `#[allow(dead_code)]`、`#[allow(unused)]`、`#[allow(clippy::*)]` 等掩盖本项目的警告；未使用的
   代码必须真正接入命令逻辑，否则删除。
2. **完整实现**：规格列出的功能必须真实、完整实现并验证。禁止：
   - 用 `todo!()`、`unimplemented!()`、空函数体、硬编码返回值冒充完成；
   - 命令“存在但报错/只打印帮助/无实际效果”；
   - 以“预留”“后续再做”“先占位”为由交付核心路径；
   - 只用假数据、只跑通演示路径却声称功能已完成。
3. **交付即验证**：提交前在本机实际运行 `cargo build` + `cargo test`，并用 `target/debug/ns-instruction-sim`
   实测关键路径（`arch list`、`device list`、`run`、`step`、`reg set/get`、`mem read/write`、`bp set`、
   `continue`、`disasm`、`state --json`、`periph status`）。
4. **文档同步**：`AGENTS.md` / `README.md` 的完成状态必须与代码一致；新增命令须同步更新本规范中的
   命令表与验收标准。

---

## 2. 技术架构

```
┌──────────────────────────────────────────────────────────┐
│             ns-instruction-sim (CLI 入口)                │
│  clap 子命令解析 → 会话(load/save) → 仿真驱动 → 输出     │
│  src/main.rs · src/session.rs · src/sim.rs               │
├──────────────────────────────────────────────────────────┤
│                 crates/ns-sim (指令级内核)               │
│  Arch 分发 · Simulator · 12 架构内核 · 虚拟外设          │
│  断点/条件 · 反汇编 · 周期/中断 · 符号/行号               │
├──────────────────────────────────────────────────────────┤
│                crates/ns-device (设备目录)               │
│  CatalogDevice · SVD 解析 · 外设/寄存器/字段 · 内存映射  │
└──────────────────────────────────────────────────────────┘
```

### 2.1 Workspace 布局

```
ns-instruction-sim/
├── Cargo.toml            # workspace: 根 crate + crates/ns-sim + crates/ns-device
├── src/
│   ├── main.rs           # clap 定义 + 全部子命令实现
│   ├── session.rs        # Session 序列化/反序列化、arch 映射
│   └── sim.rs            # 固件加载（ELF/HEX/BIN）、内存助手
├── crates/
│   ├── ns-sim/           # 指令级仿真核心（复制自 ns-embed-ide 能力模型）
│   └── ns-device/        # 设备/SVD 目录（复制自 ns-embed-ide 能力模型）
└── tests/
    └── cli.rs            # 端到端：以外部进程方式驱动二进制
```

### 2.2 支持的架构

| 架构 | `arch` 值 | 设备示例 | 寄存器视图 | 反汇编 |
|---|---|---|---|---|
| RISC-V RV32I | `riscv` | esp32c3 | x0–x31 | ✔ |
| ARM Cortex-M | `arm` | stm32f103c8 | r0–r15 | ✔ (Thumb-1) |
| Intel 8051 | `mcs51` | mcs51 | ACC/B/PSW/SP/DPTR+R0–R7 | ✔ |
| Intel 80251 | `mcs251` | 80251 | ACC/B/PSW/SP/DPTR+R0–R15 | ✔ |
| Infineon C166 | `c166` | c166 | R0–R15 | ✔ |
| ESP32 Xtensa | `xtensa` | esp32 | a0–a15 | ✔ |
| AVR 8-bit | `avr` | atmega328p | 架构专用 | ✔ |
| TI MSP430 | `msp430` | msp430g2553 | 架构专用 | ✔ |
| Renesas V850 | `v850` | v850es | 架构专用 | ✔ |
| STM8 | `stm8` | stm8s003f3 | 架构专用 | ✔ |
| Renesas RX | `rx` | rx62n | 架构专用 | ✔ |
| Renesas RL78 | `rl78` | rl78g14 | 架构专用 | ✔ |

---

## 3. CLI 命令契约

### 3.1 命令总表

| 命令 | 子命令 | 功能 | 关键参数 | 输出 |
|---|---|---|---|---|
| `version` | — | 显示版本号 | — | 文本 |
| `arch` | `list` | 列出支持的架构与示例设备 | — | 表 |
| `device` | `list` / `info` / `svd` / `linker` / `startup` / `init` | 设备目录/详情/SVD 解析（文件或内置）/链接脚本/启动文件/外设初始化 C 骨架 | `--file --device --peripheral --register --out --json` | 表/文本/JSON |
| `run` | — | 加载固件并运行到结束/步数上限/指定步数（可指定会话保留外设/SVD 寄存器） | `--firmware --session --arch --base --max-steps --steps --cycle-accurate --save-session --json` | 状态 |
| `step` | — | 单步一条指令 | `--session --arch --firmware --base --json` | 状态 |
| `step-over` | — | 步过（跳过调用） | 同上 | 状态 |
| `step-out` | — | 步出（运行至返回） | 同上 | 状态 |
| `continue` | — | 连续运行至断点/停止/上限 | `--session --max-steps --save --json` | 状态 |
| `reg` | `list` / `get` / `set` | 寄存器列出/读/写 | `--name --value --arch --session --json` | 表/值/JSON |
| `mem` | `read` / `write` / `fill` / `search` / `load` | 内存读/写/填充/搜索模式/加载固件 | `--addr --len --bytes --file --value --pattern --firmware --base --arch` | hex/JSON |
| `bp` | `list` / `set` / `clear` / `clear-all` | 断点管理 | `--addr --condition --hit-count --session` | 表/JSON |
| `disasm` | — | 反汇编 N 条（可指定 ARM 状态） | `--arch --addr --count --arm-state --session` | 文本 |
| `reset` | — | 复位仿真器（PC 回入口） | `--session --entry` | 文本 |
| `state` | — | 完整状态（JSON/文本） | `--session --arch --firmware --base --json` | 状态/JSON |
| `periph` | `status` / `config` / `uart` / `gpio` / `timer` / `reg-read` / `reg-write` / `reg-list` / `field-get` / `field-set` / `svd-load` | 外设状态/基址配置/控制/寄存器直读写/列出已加载寄存器/位域读写/SVD 加载 | `--uart-base --gpio-base --pin --value --index --enabled --prescaler --auto-reload --addr --name --len --bytes --file --device --peripheral --register --field --force --session --json` | 表/JSON |
| `symbol` | `list` / `at` / `line` / `addr` | ELF 符号与源行号映射 | `--filter --addr --file --line --session --json` | 表/JSON |
| `cycle` | `get` / `estimate` / `tick` | 周期计数/指令周期估算/外设推进 | `--addr --cycles --session --json` | 值/JSON |
| `interrupt` | `take` / `pending` / `set` | 中断触发/查询/置线 | `--line --value --session --json` | 值/JSON |
| `watchdog` | `status` / `feed` / `config` | 看门狗查询/喂狗/配置 | `--enabled --timeout --base --session --json` | 表/JSON |
| `csr` | `list` / `get` / `set` | RISC-V machine-mode CSR 读写 | `--name --value --session --json` | 值/JSON |

### 3.2 输出格式约定

1. **人类可读**（默认）：表格/文本，适合终端交互。
2. **机器可读**（`--json`）：稳定字段的 JSON 对象（`arch`、`pc`、`cycleCount`、`halted`、
   `lastResult`、`registers`、`uartText`、`uart`、`peripherals`（UART/GPIO/定时器/看门狗/中断/`externalRegs`）、
   `csr`（mstatus/mtvec/mepc/mcause/mie/mhartid）、`breakpoints`（地址/条件/命中次数/实际命中）、
   `firmware`（path/base/format，无则 null）），
   字段名不随版本随意变动。
3. **错误**：一律写 stderr，格式 `错误: <消息>`，进程退出码 `1`。成功的 `--json` 退出码 `0`。
4. **会话文件**：JSON，含 `arch`、`pc`、`cycle_count`、`registers`、`memory`、`breakpoints`、
   `memory_overlay`（显式内存写，跨架构重放）、`peripherals`（含 `external_regs`）、`csr`、`firmware`（路径/格式/基址）。
   缺失路径时自动新建临时会话。

### 3.3 新增命令必须满足

- 用 `clap` derive 声明参数，参数名用 kebab-case；提供位置参数与 `--flag` 双形式（便于脚本与交互）。
- 每个命令必须同时给出**人类可读**与 `--json` 两种输出（涉及状态/列表的命令），机器字段稳定。
- 无会话时可接受的命令必须提供 `--arch` 或 `--firmware` 以自举；需要既有会话的命令必须校验会话存在。
- 错误路径必须如实返回错误（不 panic、不静默、不打印成功假象）。
- 新功能必须在 `tests/cli.rs` 增加端到端用例，驱动真实二进制验证输出。

---

## 4. 待完善清单

> 依据 ns-sim 公开 API 与当前 CLI 覆盖度对比，列出未在 CLI 暴露的仿真能力。每项完成后须更新
> 本节状态与 §3.1 命令总表。

| # | ns-sim 能力 | 对应 API | CLI 缺口 | 状态 |
|---|---|---|---|---|
| T1 | 符号/行号查询 | `symbol_at` / `address_for_line` / `line_for_address` | `symbol list/at/line/addr` | ✅ 已完成 |
| T2 | 指令周期估算 | `cycles_for_instruction` | `cycle get/estimate/tick` | ✅ 已完成 |
| T3 | 中断触发/查询 | `take_interrupt` / `any_pending` | `interrupt take/pending/set` | ✅ 已完成 |
| T4 | 外设寄存器直读写 | `peripheral_read` / `peripheral_write` | `periph reg-read/reg-write` | ✅ 已完成 |
| T5 | 看门狗控制 | `Watchdog` / `tick_peripherals_for` | `watchdog status/feed/config` | ✅ 已完成 |
| T6 | CSR 读写 | `Csr` + CSRRW/CSRRS/CSRRC | `csr list/get/set` | ✅ 已完成 |
| T7 | 断点命中次数 | `breakpoint_hits` | `bp list` 含 `hits` | ✅ 已完成 |
| T8 | 条件断点高级属性 | `set_breakpoint_ex` / `update_breakpoint` | `bp set --condition --hit-count` | ✅ 已完成 |
| T9 | 架构寄存器元数据 | `reg_count_for` / `reg_label_for` | 已用于 `state`/`reg list` | ✅ 已完成 |
| T10 | 全架构反汇编 | `disassemble_one` / `disassemble_one_state` | 已用于 `disasm` | ✅ 已完成 |

---

## 5. 验证矩阵

每次变更后运行：

```bash
cd /Users/navysummer/Desktop/other/ns-instruction-sim
cargo build
cargo test
```

并实测（12 架构至少各跑一次 `state --json`）：

```bash
./target/debug/ns-instruction-sim arch list
./target/debug/ns-instruction-sim device list
for a in riscv arm mcs51 mcs251 c166 xtensa avr msp430 v850 stm8 rx rl78; do
  ./target/debug/ns-instruction-sim state --arch "$a" --json
done
```

### 5.1 端到端覆盖（tests/cli.rs）

- `arch_list_shows_all`：12 架构全部列出。
- `device_list_and_info`：设备目录与详情。
- `run_riscv_elf_and_inspect`：ELF 加载→运行→PC/寄存器/内存校验。
- `breakpoint_flow`：断点命中→`continue` 停在断点。
- `reg_set_and_get`：寄存器写读往返。
- `session_roundtrip_preserves_firmware_and_peripherals`：会话保存/恢复。
- `all_arch_state_works`：12 架构 `state --json` 均成功。
- `cycle_get_and_tick`：周期计数读取/推进/指令周期估算。
- `csr_set_get_roundtrip`：CSR 写读往返与会话持久化。
- `watchdog_config_feed_status`：看门狗配置/喂狗/状态。
- `interrupt_set_pending_and_take`：中断置线/挂起查询/触发跳转（mtvec/mcause）。
- `bp_list_shows_hits`：断点命中次数持久化并展示。
- `periph_timer_config_and_uart_write`：定时器配置与 UART 寄存器写输出。
- `device_svd_parse_and_filter`：SVD 文件解析、外设/寄存器过滤与错误路径。
- `run_steps_runs_exactly_n_instructions`：`--steps N` 精确运行 N 条指令并停住。
- `state_includes_peripherals_and_csr`：`state --json` 含外设与 CSR 完整快照。
- `version_command_and_flag`：`version` 子命令与 `--version` 均输出版本号。
- `mem_write_read_arm_roundtrip`：非 RISC-V（ARM）内存写跨会话持久化。
- `periph_config_relocates_bases`：外设 UART/GPIO 基址重定位并持久化。
- `mem_write_with_arch_creates_session`：`mem write --arch` 直接创建指定架构会话。
- `disasm_arm_state_override`：`disasm --arm-state a32|thumb` 切换 ARM 解码并校验步进宽度。
- `periph_svd_load_populates_registers`：`periph svd-load` 从 SVD 载入复位值并支持直读写/持久化。
- `periph_svd_load_instruction_write_couples`：`run --session` 在 SVD 加载的会话上运行，指令级 `sw` 直写寄存器并读回。
- `periph_svd_load_by_device_key`：`periph svd-load --device` 从内置 SVD 加载；缺源报错。
- `periph_reg_list_shows_loaded_registers`：`periph reg-list` 列出已加载寄存器（--device 注释名/JSON/空会话）。
- `periph_reg_name_access`：`periph reg-read/reg-write --name PERIPH.REG` 按名读写（未知寄存器/缺源报错）。
- `periph_field_access`：`periph field-get/field-set` 按 SVD 位域读写并保留其他位。
- `device_codegen_linker_and_startup`：`device linker/startup` 生成链接脚本与启动文件（stdout/--out/未知设备错误）。
- `device_init_codegen`：`device init` 生成外设初始化 C 骨架（含 --register 覆盖与格式校验）。
- `mem_search_finds_pattern`：`mem search` 在内存区域定位字节模式（JSON 匹配数/地址、未找到提示）。
- `state_reports_loaded_firmware`：`state --json` 报告已加载固件（path/base/format）。
- `device_svd_by_device_key`：`device svd --device` 从内置 SVD 解析；缺源报错。

### 5.2 反汇编宽度

- `disasm` 通过 `ns_sim::instruction_size` 按编码实际宽度推进 PC：ARM A32 恒 4 字节，Thumb-1
  16 位为 2 字节、Thumb-2 32 位（`0xE000` 前缀半字）为 4 字节；C166/AVR/MSP430/Xtensa 为 2 字节，
  其余固定 4 字节。该行为由 ns-sim 单测 `instruction_size_matches_encoding_width` 锁定。

---

## 6. 参考实现原则（复制自 ns-embed-ide）

1. **只复制能力，不复制工程**：`crates/ns-sim`、`crates/ns-device` 源码与独立 `Cargo.toml`；
   删除对上级 workspace 的依赖声明（`version.workspace`/`edition.workspace`），改为本仓库固定值。
2. **保持内部 API 稳定**：复制时保留 `pub fn` 签名，避免 IDE 侧既有语义漂移；仅在 CLI 层做适配。
3. **禁止仓库外引用**：`Cargo.toml` 不得出现 `../` 的 path 依赖；一切依赖走 crates.io 或本仓库内。
4. **版本独立**：`ns-sim`/`ns-device` 版本随本仓库统一 `0.1.0`，不继承外部版本号。
