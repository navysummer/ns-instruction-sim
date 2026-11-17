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
| `version` | — | 显示版本号 | `--json` | 文本/JSON |
| `arch` | `list` | 列出支持的架构与示例设备 | `--json` | 表/JSON |
| `device` | `list` / `info` / `svd` / `linker` / `startup` / `init` | 设备目录/详情/SVD 解析（文件或内置）/链接脚本/启动文件/外设初始化 C 骨架 | `--file --device --peripheral --register --out --json` | 表/文本/JSON |
| `run` | — | 加载固件并运行到结束/步数上限/指定步数/指定地址（可指定会话保留外设/SVD 寄存器） | `--firmware --session --arch --base --max-steps --steps --until --cycle-accurate --save-session --json --trace --source` | 状态 |
| `step` | — | 单步一条指令 | `--session --arch --firmware --base --json --source` | 状态 |
| `step-over` | — | 步过（跳过调用） | 同上 | 状态 |
| `step-out` | — | 步出（运行至返回） | 同上 | 状态 |
| `continue` | — | 连续运行至断点/停止/上限/指定地址（临时断点） | `--session --until --max-steps --save --json --trace --source` | 状态 |
| `reg` | `list` / `get` / `set` | 寄存器列出/读/写 | `--name --value --arch --filter --session --json` | 表/值/JSON |
| `mem` | `read` / `write` / `fill` / `search` / `load` / `dump` | 内存读/写/填充/搜索模式/加载固件/导出（C 数组 / Intel HEX 可回载 / hex+ASCII 视图） | `--addr --len --bytes --values --file --value --pattern --firmware --base --arch --name --format --width --out --json` | hex/C/JSON |
| `bp` | `list` / `set` / `clear` / `clear-all` / `enable` / `disable` | 断点管理（条件/命中次数/一次性/启用禁用/按符号或行号设置） | `--addr --symbol --line --condition --hit-count --once --session --json` | 表/JSON |
| `disasm` | — | 反汇编 N 条（可指定 ARM 状态/来源标注/原始字节/JSON） | `--arch --addr --count --arm-state --source --raw --json --session` | 文本/JSON |
| `reset` | — | 复位仿真器（PC 回入口） | `--session --entry --json` | 文本/JSON |
| `state` | — | 完整状态（JSON/文本/字段子集/单行摘要） | `--session --arch --firmware --base --json --fields --summary` | 状态/JSON/摘要 |
| `periph` | `status` / `config` / `uart` / `gpio` / `timer` / `reg-read` / `reg-write` / `reg-list` / `field-get` / `field-set` / `svd-load` | 外设状态/基址配置/控制/寄存器直读写/列出已加载寄存器/位域读写/SVD 加载 | `--uart-base --gpio-base --pin --value --index --enabled --prescaler --auto-reload --addr --name --len --bytes --file --device --peripheral --register --field --force --session --json` | 表/JSON |
| `symbol` | `list` / `at` / `line` / `addr` | ELF 符号与源行号映射 | `--filter --addr --file --line --session --json` | 表/JSON |
| `cycle` | `get` / `estimate` / `tick` | 周期计数/指令周期估算/外设推进 | `--addr --cycles --session --json` | 值/JSON |
| `interrupt` | `take` / `pending` / `set` | 中断触发/查询/置线 | `--line --value --session --json` | 值/JSON |
| `watchdog` | `status` / `feed` / `config` / `reset` | 看门狗查询/喂狗/配置/强制复位 | `--enabled --timeout --base --entry --session --json` | 表/JSON |
| `csr` | `list` / `get` / `set` | RISC-V machine-mode CSR 读写 | `--name --value --session --json` | 值/JSON |

### 3.2 输出格式约定

1. **人类可读**（默认）：表格/文本，适合终端交互。
2. **机器可读**（`--json`）：稳定字段的 JSON 对象（`arch`、`pc`、`cycleCount`、`halted`、
   `lastResult`、`registers`、`uartText`、`uart`、`peripherals`（UART/GPIO/定时器/看门狗/中断/`externalRegs`）、
   `csr`（mstatus/mtvec/mepc/mcause/mie/mhartid）、`breakpoints`（地址/条件/命中次数/一次性/启用状态/实际命中）、
   `firmware`（path/base/format，无则 null）、`pcSymbol`（最近 ELF 符号 name/address/size，无则 null）），
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
| T5 | 看门狗控制 | `Watchdog` / `tick_peripherals_for` / `reset` | `watchdog status/feed/config/reset` | ✅ 已完成 |
| T6 | CSR 读写 | `Csr` + CSRRW/CSRRS/CSRRC | `csr list/get/set` | ✅ 已完成 |
| T7 | 断点命中次数 | `breakpoint_hits` | `bp list` 含 `hits` | ✅ 已完成 |
| T8 | 条件断点高级属性 | `set_breakpoint_ex` / `update_breakpoint` / `set_breakpoint_enabled` | `bp set --condition --hit-count`、`bp enable/disable` | ✅ 已完成 |
| T9 | 架构寄存器元数据 | `reg_count_for` / `reg_label_for` | 已用于 `state`/`reg list` | ✅ 已完成 |
| T10 | 全架构反汇编 | `disassemble_one` / `disassemble_one_state` | 已用于 `disasm` | ✅ 已完成 |
| T11 | 一次性断点 | `set_breakpoint_ex_once` / `breakpoint_is_once` | `bp set --once`（命中后自动清除） | ✅ 已完成 |

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
- `continue_until_stops_at_address`：`continue --until` 临时断点运行到指定地址且不持久化。
- `reg_set_and_get`：寄存器写读往返。
- `session_roundtrip_preserves_firmware_and_peripherals`：会话保存/恢复。
- `all_arch_state_works`：12 架构 `state --json` 均成功。
- `all_arch_core_operations`：12 架构寄存器列出/单步/内存写读往返/反汇编均成功。
- `mem_overlay_persists_across_step`：ARM（私有内存模型核心）显式内存写在单步后仍持久化（memory_overlay 跨调用重放）。
- `cycle_get_and_tick`：周期计数读取/推进/指令周期估算。
- `csr_set_get_roundtrip`：CSR 写读往返与会话持久化。
- `watchdog_config_feed_status`：看门狗配置/喂狗（复位 elapsed 计数并持久化）/状态。
- `watchdog_force_reset`：`watchdog reset` 记录复位事件并复位核心（持久化）。
- `interrupt_set_pending_and_take`：中断置线/挂起查询/触发跳转（mtvec/mcause）。
- `bp_list_shows_hits`：断点命中次数持久化并展示。
- `bp_once_autoclears`：`bp set --once` 命中后自动清除，再次 `continue` 不再停靠。
- `bp_enable_disable_roundtrip`：`bp disable` 不再暂停，`bp enable` 恢复暂停。
- `bp_set_by_symbol`：`bp set --symbol` 按符号名解析地址设置断点（含未找到错误路径）。
- `bp_set_by_line`：`bp set --line file:line` 按源码行号设置断点（含非法行号错误路径）。
- `bp_management_json`：`bp set/clear/clear-all/enable/disable --json` 机器可读确认。
- `bp_set_out_of_range_rejected`：16 位 PC 架构（mcs51/c166/avr）越界断点地址被拒绝，不静默回绕；`--until` 同校验。
- `periph_timer_config_and_uart_write`：定时器配置与 UART 寄存器写输出。
- `device_svd_parse_and_filter`：SVD 文件解析、外设/寄存器过滤与错误路径。
- `run_steps_runs_exactly_n_instructions`：`--steps N` 精确运行 N 条指令并停住。
- `run_until_stops_at_address`：`run --until` 临时断点运行到指定地址且不持久化。
- `run_trace_prints_each_instruction`：`run --trace` 打印每条执行指令的 PC/反汇编/来源。
- `continue_trace_prints_each_instruction`：`continue --trace` 打印每条执行指令并停靠临时断点。
- `continue_source_annotates_stop`：`continue --source` 在停止点标注 file:line。
- `mem_dump_c_array`：`mem dump --format c` 生成 C 数组（含非法格式错误路径）。
- `mem_dump_json`：`mem dump --json` 输出地址/长度/格式/名称/字节数组（机器可读导出）。
- `mem_dump_hex_roundtrip`：`mem dump --format hex` 生成合法 Intel HEX，可被 `mem load` 回载（含 64 KiB 银行边界与 `--format dump`/非法格式路径）。
- `mem_write_read_roundtrip_persists`：内存写读跨进程往返并持久化。
- `mem_fill_hex_value`：`mem fill --value` 十六进制值填充并可读回。
- `mem_read_word_width`：`mem read --width` 按字节/半字/字/双字读取（含非法宽度错误路径）。
- `mem_read_json`：`mem read --json` 输出地址/宽度/字节数组。
- `mem_write_word_width`：`mem write --width --values` 按字/半字小端写入（含越界错误路径）。
- `mem_write_fill_json`：`mem write/fill --json` 机器可读确认。
- `arch_device_periph_json`：`arch/device/periph uart/gpio --json` 机器可读输出。
- `device_codegen_json`：`device linker/startup/init --json` 包裹生成内容。
- `state_fields_and_summary`：`state --fields` JSON 字段子集（含点号嵌套路径与未知字段报错）、`state --summary` 单行摘要。
- `reg_list_filter`：`reg list --filter` 按名称子串筛选寄存器。
- `reset_json`：`reset --json` 输出 PC/周期/停止 JSON。
- `reset_default_goes_to_entry`：`reset` 不带 `--entry` 回固件/架构入口（非当前 PC）。
- `state_includes_peripherals_and_csr`：`state --json` 含外设与 CSR 完整快照。
- `version_command_and_flag`：`version` 子命令（含 `--json`）与 `--version` 均输出版本号。
- `mem_write_read_arm_roundtrip`：非 RISC-V（ARM）内存写跨会话持久化。
- `periph_config_relocates_bases`：外设 UART/GPIO 基址重定位并持久化。
- `mem_write_with_arch_creates_session`：`mem write --arch` 直接创建指定架构会话。
- `disasm_arm_state_override`：`disasm --arm-state a32|thumb` 切换 ARM 解码并校验步进宽度。
- `disasm_source_annotates_lines`：`disasm --source` 标注每行 file:line。
- `disasm_raw_shows_bytes`：`disasm --raw` 显示每条指令的原始字节。
- `disasm_json`：`disasm --json` 输出地址/反汇编/字节/来源 JSON 数组。
- `disasm_mcs51_code_space_width`：mcs51 独立代码 ROM 被 `disasm` 读取，且按实际指令宽度推进（NOP=1、LJMP addr16=3）。
- `disasm_mcs251_code_space_width`：80251 变长指令按实际宽度推进（NOP=1、MOV Rn,#imm16=3）。
- `disasm_avr_32bit_width`：AVR LDS/STS/JMP/CALL 32 位指令按 4 字节推进并正确解码（此前守卫隐藏了这些分支）。
- `disasm_c166_imm_width`：C166 `MOV Rn,#imm16`/`CALL` 按 3 字节推进并解码小端立即数（与执行内核一致）。
- `disasm_xtensa_24bit_width`：本仿真器的 Xtensa 内核按 24 位取指，反汇编按 3 字节推进并解码 NOP。
- `disasm_v850_variable_width`：V850 变长指令（2/4/6 字节）按解码宽度推进（NOP=2、DISPOSE=4）。
- `step_source_annotates_lines`：`step --source` 按 DWARF 行表标注 file:line。
- `step_over_and_step_out_use_code_space`：`step-over` 按实际指令宽度计算返回地址（mcs51 LCALL=3 字节），
  `step-out` 从独立代码 ROM 读取 `RET` 并在返回地址停下（修复哈佛内核代码空间读取）。
- `state_bootstraps_persistent_session`：`state --arch --session` 在缺失路径时物化会话文件，后续 `cycle estimate`/`run` 可直接操作。
- `periph_svd_load_populates_registers`：`periph svd-load` 从 SVD 载入复位值并支持直读写/持久化。
- `periph_svd_load_instruction_write_couples`：`run --session` 在 SVD 加载的会话上运行，指令级 `sw` 直写寄存器并读回。
- `periph_svd_load_by_device_key`：`periph svd-load --device` 从内置 SVD 加载；缺源报错。
- `periph_reg_list_shows_loaded_registers`：`periph reg-list` 列出已加载寄存器（--device 注释名/JSON/空会话）。
- `periph_status_reports_svd_registers`：`periph status --json` 含 `externalRegs` 快照。
- `periph_reg_name_access`：`periph reg-read/reg-write --name PERIPH.REG` 按名读写（未知寄存器/缺源报错）。
- `periph_field_access`：`periph field-get/field-set` 按 SVD 位域读写并保留其他位。
- `periph_reg_write_data_space_avr`：AVR I/O 空间外设寄存器写入落在数据空间（`periph reg-read`/`mem read` 均可见）。
- `device_codegen_linker_and_startup`：`device linker/startup` 生成链接脚本与启动文件（stdout/--out/未知设备错误）。
- `device_init_codegen`：`device init` 生成外设初始化 C 骨架（含 --register 覆盖与格式校验）。
- `mem_search_finds_pattern`：`mem search` 在内存区域定位字节模式（JSON 匹配数/地址、未找到提示）。
- `mem_search_harvard_data_space`：Harvard 架构（8051）数据空间搜索使用统一内存助手，与 `mem write` 所见一致。
- `mem_search_out_writes_file`：`mem search --out` 将 JSON/文本结果写入文件。
- `state_reports_loaded_firmware`：`state --json` 报告已加载固件（path/base/format）。
- `state_reports_pc_symbol`：`state --json` 报告 PC 处最近符号（pcSymbol）。
- `symbol_list_shows_symbols`：`symbol list --json` 列出 ELF 符号（名称/地址）。
- `device_svd_by_device_key`：`device svd --device` 从内置 SVD 解析；缺源报错。

### 5.2 反汇编宽度

- `disasm` 通过 `ns_sim::instruction_size` 按编码实际宽度推进 PC：ARM A32 恒 4 字节，Thumb-1
  16 位为 2 字节、Thumb-2 32 位（`0xE000` 前缀半字）为 4 字节；C166/AVR/MSP430/Xtensa 为 2 字节，
  MCS-51 按操作码变长（NOP=1、立即/相对/直接=2、LJMP/LCALL/DPTR=3），80251 按操作码变长
  （寄存器/累加器=1、相对/直接=2、`MOV Rn,#imm16`=3、24 位绝对跳转/DPTR=4），AVR 的 LDS/STS/JMP/CALL
  为 4 字节其余 2 字节，C166 的 `MOV Rn,#imm16`/`CALL` 为 3 字节、NOP/RET 为 1 字节其余 2 字节，
  Xtensa 按 24 位取指为 3 字节、MSP430 为 2 字节、V850 按解码变长（2/4/6 字节），其余固定 4 字节。
  该行为由 ns-sim 单测 `instruction_size_matches_encoding_width` 与 CLI 端到端
  `disasm_mcs51_code_space_width`/`disasm_mcs251_code_space_width`/`disasm_avr_32bit_width`/
  `disasm_c166_imm_width`/`disasm_xtensa_24bit_width`/`disasm_v850_variable_width` 锁定。

---

## 6. 参考实现原则（复制自 ns-embed-ide）

1. **只复制能力，不复制工程**：`crates/ns-sim`、`crates/ns-device` 源码与独立 `Cargo.toml`；
   删除对上级 workspace 的依赖声明（`version.workspace`/`edition.workspace`），改为本仓库固定值。
2. **保持内部 API 稳定**：复制时保留 `pub fn` 签名，避免 IDE 侧既有语义漂移；仅在 CLI 层做适配。
3. **禁止仓库外引用**：`Cargo.toml` 不得出现 `../` 的 path 依赖；一切依赖走 crates.io 或本仓库内。
4. **版本独立**：`ns-sim`/`ns-device` 版本随本仓库统一 `0.1.0`，不继承外部版本号。
