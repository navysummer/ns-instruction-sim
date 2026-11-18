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
   `lastResult`、`registers`、`armState`（仅 ARM，`a32`/`thumb`，其余架构 null）、`uartText`、`uart`、
   `armXpsr`（仅 ARM：N/Z/C/V 条件标志、Thumb 位、当前异常号的 xPSR 原值，其余架构 null）、
   `peripherals`（UART/GPIO/定时器/看门狗/中断/`externalRegs`）、
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
- `watchdog_reset_default_goes_to_entry`：`watchdog reset` 未指定 `--entry` 时回到固件/架构入口
  （而非看门狗触发时的 PC），仍记录 `lastResetPc`。
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
- `state_reports_arm_state`：`state --json` 报告 ARM 指令集状态（`armState`=`a32`/`thumb`，
  其余架构为 null），供外部选择正确的反汇编模式。
- `state_reports_arm_xpsr`：`state --json` 报告 ARM xPSR（`armXpsr`，含 N/Z/C/V 条件标志与
  Thumb 位；其余架构为 null），供外部无需执行指令即可读取条件标志。
- `version_command_and_flag`：`version` 子命令（含 `--json`）与 `--version` 均输出版本号。
- `mem_write_read_arm_roundtrip`：非 RISC-V（ARM）内存写跨会话持久化。
- `periph_config_relocates_bases`：外设 UART/GPIO 基址重定位并持久化。
- `mem_write_with_arch_creates_session`：`mem write --arch` 直接创建指定架构会话。
- `disasm_arm_state_override`：`disasm --arm-state a32|thumb` 切换 ARM 解码并校验步进宽度。
- `disasm_thumb_full_families`：Thumb-1/Thumb-2 全家族反汇编镜像执行内核（LSL/ADD/MOV/CMP/SUB
  imm、PC-rel LDR、寄存器偏移 LDRH、字/字节/半字立即、SP 相对、PUSH/POP、LDMIA、SVC、条件/无条件分支、
  BL、MOVW、ADDW、LDR/STR/LDRB/STRB 立即），并校验按编码实际宽度推进。
- `disasm_thumb2_ldrh_strh_barriers`：Thumb-2 `LDRH/STRH` 立即与 `DMB/DSB/ISB` 屏障反汇编，
  校验宽度与助记符。
- `disasm_a32_operand2_shifts`：A32 数据传送/数据处理操作数 2 的移位（LSL/LSR/ASR/ROR、
  RRX、寄存器指定位移）反汇编镜像执行内核。
- `run_arm_thumb1_field_extraction`：端到端执行 Thumb-1 `LSL`/`SUBS reg`/`SUBS imm3`，
  校验内核按编码提取 Rn/Rm/Rd（`lsls r0,r1,#5`→r0=0x400、`subs r0,r2,r1`→r0=5-32、
  `subs r0,r2,#3`→r0=2），锁定格式 0/1 字段提取修复。
- `run_arm_thumb1_pc_relative`：端到端执行 Thumb-1 `LDR r,[pc,#imm]` 与 `ADD rd,pc,#imm`，
  校验 PC 相对基址按架构 PC（当前 +4）取字对齐（`ldr r0,[pc,#0]` 读取 4 字对齐字面量
  0xCAFEBABE、`add r2,pc,#4` 得 0x08000018），锁定 16 位 PC 相对寻址修复。
- `disasm_thumb2_bl_branch_target`：Thumb-2 `BL`/`B.W` 分支目标按架构 PC（当前 +4）计算
  （`bl 0x8` 自 0x0、`b.w 0x10` 自 0x4），锁定分支目标基址与 `B.W` 解码修复。
- `run_arm_thumb1_asr_immediate`：端到端执行 Thumb-1 `ASRS r0,r1,#5`（0x1148）→
  r0=0xFC000000，锁定 `ASR` 立即分类（top=1 且 bit11 清 0）与移位量语义。
- `run_arm_thumb1_branch_target`：端到端分两次 `step --session`（跨会话保存/恢复）执行
  `movs r0,#0`（置 Z）后 `beq +2`（0xD001）命中，PC 停在 0x08000018（架构 PC = 当前 +4 基址），
  锁定 Thumb-1 分支目标基址与 ARM 条件标志（xPSR）跨会话持久化。
- `run_arm_thumb1_high_cmp_v_flag`：端到端分三次 `step --session` 执行 `cmp r0,r1`（0x4508，
  0x80000000 - 1 溢出）后 `bvs +2`（0xD601）命中，PC 停在 0x0800001a 且 r2=2，
  锁定高寄存器 CMP 更新 V 标志（原不更新，使 `VS/VC` 有符号条件分支失效）。
- `run_arm_thumb1_cbz_branch`：端到端分三次 `step --session` 执行 `movs r0,#0` 后 `cbz r0,+4`
  （0xB110）命中，PC 停在 0x0800001c 且 r2=1（r1 未被触碰），锁定 CBZ/CBNZ 分支执行
  （原被误当作 `ADD/SUB SP`，导致栈与 PC 损坏）。
- `run_arm_thumb1_extend_and_mul`：端到端分三次 `step --session` 执行 `sxtb r3,r4`（0xB263，
  r4=0x80→r3=0xFFFFFF80）、`uxth r5,r6`（0xB2B5，r6=0xFFFF8000→r5=0x8000）、
  `mul r0,r1,r2`（0xFB01F002，3×5=15），锁定符号/零扩展与 Thumb-2 MUL 执行。
- `run_arm_thumb2_mrs_msr`：端到端分三次 `step --session` 执行 `movs r0,#0`（置 Z）、
  `mrs r1,xpsr`（0xF3EF8103→r1=0x41000000，读 Z+Thumb）、`msr xpsr_nzcvq,r2`
  （0xF3828803，r2=0x80000000→置 N 清 Z），`state --json` 校验 `armXpsr`=0x81000000，
  锁定 Thumb-2 MRS/MSR 读写状态寄存器。
- `run_arm_thumb1_rev_and_clz`：端到端分三次 `step --session` 执行 `cpsie i`（0xB662，清
  PRIMASK）、`rev r1,r0`（0xBA08，0x12345678→0x78563412）、`clz r2,r3`（0xFAB3F287，
  r3=0x00008000→r2=16），锁定 Thumb-1 CPSID/CPSIE 对 PRIMASK 的写与 REV/REV16/REVSH、
  Thumb-2 CLZ 的执行。
- `run_arm_thumb2_div_load_literal_and_ldm`：端到端分四次 `step --session` 执行
  `ldr.w r0,[r1,r2]`（0xF8510002，r1=0x20000000/r2=4→读 0x20000004 得 0xCAFEBABE）、
  `sdiv r3,r4,r5`（0xFB94F5F3，-7/2=-3）、`ldmia.w r6!,{r7,r8}`（0xE8B68001，
  r6 写回 0x20000018）、`udiv r9,r10,r11`（0xFBBAFBF9，0x10000/3=0x5555），
  锁定 Thumb-2 寄存器偏移存取、有/无符号除法与 LDM/STM 宽格式写回。
- `run_arm_thumb2_data_processing_register`：端到端分四次 `step --session` 执行
  `add.w r3,r1,r2`（0xEB010203，5+7=12）、`and.w r4,r3,r5`（0xEA030504，12&14=12）、
  `cmp.w r0,r1`（0xEBB00F01，9-5 置 C 清 N/Z）、`mov.w r6,r7`（0xEA4F0607，r6=0x1234），
  `state --json` 校验寄存器与 `armXpsr` 标志，锁定 Thumb-2 数据处理（寄存器）执行。
- `run_arm_thumb2_ldrd_strd_tbb`：端到端分三次 `step --session` 执行 `ldrd r0,r1,[r2]`
  （0xE9D20100，读 0x20000000/0x20000004 得 0x11111111/0x22222222）、
  `strd r4,r5,[r6,#16]`（0xE9C64504，写 0x20000020/0x20000024 得 0xCAFE/0xBEEF）、
  `tbb [r7,r8]`（0xE8D7F008，r7=0x20000030/r8=2，索引字节 0x20 → 目标 0x0800005C），
  校验寄存器、PC 与 STRD 内存写跨会话持久化（`mem read`），锁定 Thumb-2 双字传送
  与表分支执行；同时校验 ARM 运行时内存写进入会话 `memory_overlay`（跨进程 step 可续跑）。
- `run_arm_thumb2_modified_immediate`：端到端分四次 `step --session` 执行
  `mov.w r0,#0xff`（0xF04F00FF，r0=0xFF）、`add.w r3,r4,#0x40`（0xF1040340，5+0x40=0x45）、
  `sub.w r5,r6,#0x200`（0xF5A67500，0x300−0x200=0x100）、`cmp.w r0,#0x40`（0xF1B00F40，
  0xff−0x40 置 C 清 N/Z/V），`state --json` 校验寄存器与 `armXpsr` 标志，锁定
  Thumb-2 修正立即数据处理执行（含 `thumb_expand_imm12` 扩展）。
- `disasm_thumb2_modified_immediate`：`disasm --arm-state thumb` 将
  `mov r0,#0xff`/`add r3,r4,#0x40`/`sub r5,r6,#0x200`/`cmp r0,#0x40`/`tst r2,#0xff`
  反汇编为真实助记符并按扩展立即数显示。
- `run_arm_thumb1_it_bkpt`：端到端分八次 `step --session` 执行
  `movs r0,#0`（0x2000，置 Z）、`it eq`（0xBF08，`moveq r0,r1` 执行）、
  `movs r2,#1`（0x2201，清 Z）、`ite eq`（0xBF0C，`moveq r3,r4` 跳过、
  `movne r5,r6` 执行）、`bkpt #3`（0xBE03，停机），校验寄存器/标志/PC 并验证
  IT 状态跨会话持久化（跨进程 `step --session` 保留 IT 块与索引）。
- `disasm_thumb1_it_bkpt`：`disasm --arm-state thumb` 将
  `it eq`/`ite ge`/`itt eq`/`itete eq`/`bkpt #3` 反汇编为真实助记符。
- `run_arm_thumb2_bfx`：端到端分四次 `step --session` 执行 `ubfx r1,r2,#4,#8`
  （0xF3C21107，r2=0xABCDEF12 → r1=0xF1）、`sbfx r3,r4,#2,#6`（0xF3440385，
  r4=0x80 → r3=0xFFFFFFE0）、`bfc r5,#4,#8`（0xF36F150B，清 [11:4] →
  0xFFFFF00F）、`bfi r6,r7,#3,#5`（0xF36706C7，插入 r7 低 5 位到 [7:3] →
  0xF8），`state --json` 校验，锁定 Thumb-2 位域提取/插入/清除执行。
- `disasm_thumb2_bfx`：`disasm --arm-state thumb` 将 `ubfx`/`sbfx`/`bfc`/`bfi`
  反汇编为真实助记符与字段参数。
- `run_arm_thumb2_rev_extend`：端到端分八次 `step --session` 执行
  `rev.w r7,r8`（0xFA98F788，0x12345678 → 0x78563412）、`rev16.w r9,r10`
  （0xFA9AF99A，→ 0x34127856）、`revsh.w r11,r12`（0xFA9CFBBC，0x12340080 →
  0xFFFF8000）、`rbit r0,r1`（0xFA91F0A1，0x12345678 → 0x1E6A2C48）、
  `uxtb.w r8,r9`（0xFA5FF889，→ 0x80）、`sxtb.w r10,r11`（0xFA4FFA8B，→
  0xFFFFFF80）、`uxth.w r0,r1`（0xFA1FF081，→ 0x8000）、`sxth.w r2,r3`
  （0xFA0FF283，→ 0xFFFF8000），`state --json` 校验，锁定 Thumb-2
  REV/REV16/REVSH/RBIT 与符号/零扩展执行。
- `disasm_thumb2_rev_extend`：`disasm --arm-state thumb` 将
  `rev.w`/`rev16.w`/`revsh.w`/`rbit`/`uxtb.w`/`sxtb.w`/`uxth.w`/`sxth.w`
  反汇编为真实助记符。
- `run_arm_thumb2_ldrex_strex`：端到端分六次 `step --session` 执行
  `ldrex r0,[r1]`（0xE8510F00，读 0x20000000 得 0xDEADBEEF）、
  `strex r2,r3,[r4]`（0xE8443200，写 0x20000004 得 0xCAFEBABE，r2=0 表示成功）、
  `ldrexb r5,[r6]`（0xE8D65F4F，读 0x7F）、`strexb r7,r8,[r9]`（0xE8C98F47，
  写 0xAB，r7=0）、`ldrexh r10,[r11]`（0xE8DBAF5F，读 0xBEEF）、
  `strexh r12,r0,[r1]`（0xE8C10F5C，写 0x1234，r12=0），`state --json` 与
  `mem read` 校验寄存器与存储结果，锁定 Thumb-2 独占访问执行（单核 Cortex-M
  独占监视恒授予，STREX 写并记录成功）。
- `disasm_thumb2_ldrex_strex`：`disasm --arm-state thumb` 将
  `ldrex`/`strex`/`ldrexb`/`strexb`/`ldrexh`/`strexh` 反汇编为真实助记符。
- `disasm_source_annotates_lines`：`disasm --source` 标注每行 file:line。
- `disasm_raw_shows_bytes`：`disasm --raw` 显示每条指令的原始字节。
- `disasm_json`：`disasm --json` 输出地址/反汇编/字节/来源 JSON 数组。
- `disasm_mcs51_code_space_width`：mcs51 独立代码 ROM 被 `disasm` 读取，且按实际指令宽度推进（NOP=1、LJMP addr16=3）。
- `disasm_mcs51_core_forms`：mcs51 反汇编镜像执行内核的旋转（RR/RLC/RL/RRC）、
  ALU 立即/直接/寄存器（ADD/ADDC/SUBB/ANL/ORL/XRL，含新增 `ORL/ANL/XRL A,Rn`）、
  MOV/MOVX/MOVC、XCH/XCHD、位操作、相对分支（JB/JNB/JBC/CJNE/DJNZ）等家族；内核同时修复 `RR A`/`RLC A` 缺失与
  `ANL/ORL/XRL A,direct` 编码（`0x55`/`0x45`/`0x65`，原 `0x54`/`0x44`/`0x64` 误用）。
- `disasm_mcs51_alu_acc_register`：mcs51 `ORL/ANL/XRL A,Rn`（`0x48-0x4F`/`0x58-0x5F`/`0x68-0x6F`）
  按低半字节取源寄存器并各推进 1 字节。
- `disasm_mcs51_ajmp_acall_addr11`：mcs51 `AJMP/ACALL addr11` 按 opcode bits 7..5
  取页位 A10..A8、后随低地址字节，按 2 字节推进；执行内核同步补齐全部 8 个页组合
  （此前只覆盖 `0x01`/`0x11` 且用 opcode 低半字节当作地址，属错误）。
- `step_mcs51_inc_dec_register`：mcs51 执行内核补齐 `INC Rn`（0x08-0x0F）与
  `DEC Rn`（0x18-0x1F），寄存器写入/读回跨会话一致（此前反汇编已显示但核心会 halt）。
- `disasm_mcs251_code_space_width`：80251 变长指令按实际宽度推进（NOP=1、MOV Rn,#imm16=3）。
- `disasm_mcs251_core_forms`：80251 反汇编镜像执行内核的 RET/INC/DEC/JC/JNC/MOV A,direct/
  MOVX @Ri/CLR/CPL/RL/RR/JMP @A+DPTR 家族，按实际宽度推进（相对跳转=2、MOV A,direct=2）。
- `step_mcs251_movx_r1_roundtrip`：80251 `MOVX @R1,A`（0xF3）/`MOVX A,@R1`（0xE3）
  按 opcode 最低位选择寄存器索引，单进程运行 2 条指令完成外部 RAM 写读回环。
- `disasm_avr_32bit_width`：AVR LDS/STS/JMP/CALL 32 位指令按 4 字节推进并正确解码（此前守卫隐藏了这些分支）。
- `disasm_avr_muls_lpm_spm_push`：AVR 反汇编补齐 `MULS`（0x0200 有符号乘法）、
  `LPM`（0x95C8）、`SPM`（0x95E8）与 `PUSH Rr`（`0x920F | d<<4`），
  并将 LDS/STS/LD/ST/POP/PUSH/LPM 按 `word & 0xFE0F` 掩码解码以支持任意目的寄存器。
- `disasm_c166_imm_width`：C166 `MOV Rn,#imm16`/`CALL` 按 3 字节推进并解码小端立即数（与执行内核一致）。
- `disasm_c166_logic_register_forms`：C166 `0x50-0x5F` 按 `bits 3:2` 解码
  `AND`/`OR`/`XOR Rn, Rm`（00=AND、01=OR、1x=XOR；低半字节为目的寄存器，
  故 `0x54` 目标 R4），与执行内核一致并按 2 字节推进。
- `disasm_xtensa_24bit_width`：本仿真器的 Xtensa 内核按 24 位取指，反汇编按 3 字节推进并解码 NOP。
- `disasm_v850_variable_width`：V850 变长指令（2/4/6 字节）按解码宽度推进（NOP=2、DISPOSE=4）。
- `disasm_stm8_variable_width`：STM8 变长指令按实际宽度推进并解码内核主流操作码族（NOP=1、JRA=2、
  CALL=3、CALLF=4、`0x90` 前缀 Y 索引=3），替换原先仅解码 7 个操作码的极小反汇编器。
- `disasm_stm8_standalone_90`：STM8 `0x90` 既是 Y 索引前缀又是单字节 `LDW Y,X`；当其后紧跟
  无 Y 形式的操作码（如 `NOP`/`SLA addr8`）时按独立 `LDW Y,X`（1 字节）解码，并继续解码下一条。
- `step_stm8_sp_relative_ld`：STM8 `0x17` 修正为 `LD (off8,SP),A`（写回栈相对字节）、
  `0x16` 为 `LD A,(off8,SP)`（读入 A）；此前 `0x17` 误把操作数读入 A 而不写内存。
- `disasm_rl78_decodes_core_families`：RL78 反汇编扩展为镜像内核 `execute_single`/`execute_61`
  的实际分发——`0x61` 前缀的寄存器-寄存器（add/addc/sub/subc/cmp/and/or/xor/xch）、
  `ES:[HL+B/C]`、`ES:[HL+imm8]`、`ES:[imm16]`、PSW（push/pop）、控制流（skc/sknc/skz/sknz/
  br/reti/retb/rol/ror/rolwc/halt/stop/brk）及单字节 add/addw/mov/inc/dec/push/pop/ret 家族，
  替代 `.byte`/`prefix 0x61:` 占位符。
- `stm8_step_over_and_step_out_use_call_width`：STM8 `call addr16` 为 3 字节，`step-over` 运行调用并停在
  调用者下一条指令（pc+3），`ret` 被 `step-out` 识别。
- `disasm_msp430_extension_words`：MSP430 双操作数指令的立即数/绝对/索引操作数携带扩展字，
  反汇编按实际宽度推进（操作码字 + 扩展字），替换固定 2 字节（`MOV #imm16,R5`=4、`MOV Rn,Rn`=2）。
- `msp430_step_over_and_step_out_use_return`：MSP430 反汇编使用大写助记符（`CALL`/`RETI`），
  返回/调用识别必须大小写无关；`step-over` 在 4 字节 `CALL #imm` 上于 pc+4 恢复，`step-out`
  识别 `MOV @SP+, PC`（`RET` 伪指令）并在返回地址停下。
- `step_source_annotates_lines`：`step --source` 按 DWARF 行表标注 file:line。
- `stm8_step_over_and_step_out_use_call_width`：`step-over` 按实际指令宽度计算返回地址
  （STM8 `call addr16`=3 字节），`step-out` 识别 `ret` 并在返回地址停下（修复哈佛内核代码空间读取）。
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
- `disasm_rx_fc_fd_long_forms`：RX `0xFC`/`0xFD` 长操作码族反汇编文本与实际宽度推进。
- `step_rx_fc_fd_long_forms`：RX `0xFC`/`0xFD` 长操作码族执行（ADC/SHLR/SBB）与 PC/寄存器变化。
- `disasm_rx_bit_and_register_ops`：RX `0x78-0x7D`（BSET/BCLR/BTST #imm5, rd）与
  `0x7E`/`0x7F`（NOT/NEG/ABS/RORC/ROLC/PUSH/POP/PUSHC/POPC 与 JMP/JSR/BRA.l/BSR.l）
  按 2 字节解码为真实助记符（此前 0x7A-0x7D 落到占位符、0x7E/0x7F 误配到 NOT/JMP 族）。
- `byte_opcode_disasm_matches_core`/`word_opcode_disasm_matches_core`：逐操作码枚举校验
  “反汇编占位符 ⟺ 核心停机”，确保执行内核支持的操作码族都有真实反汇编（MCS-51/80251/
  C166/STM8/RL78/AVR/MSP430/RX）。

### 5.2 反汇编宽度

- `disasm` 通过 `ns_sim::instruction_size` 按编码实际宽度推进 PC：ARM A32 恒 4 字节，Thumb-1
  16 位为 2 字节、Thumb-2 32 位（首半字 `0xE800-0xFFFF`，即 `half & 0xF800 >= 0xE800`）为 4 字节；
  注意 Thumb-1 无条件分支 `b`（`0xE000-0xE7FF`）仍是 16 位，不得误判为 32 位；
  C166/AVR/MSP430/Xtensa 为 2 字节，
  MCS-51 按操作码变长（NOP=1、立即/相对/直接=2、LJMP/LCALL/DPTR=3），80251 按操作码变长
  （寄存器/累加器=1、相对/直接=2、`MOV Rn,#imm16`=3、24 位绝对跳转/DPTR=4），AVR 的 LDS/STS/JMP/CALL
  为 4 字节其余 2 字节，C166 的 `MOV Rn,#imm16`/`CALL` 为 3 字节、NOP/RET 为 1 字节其余 2 字节，
  Xtensa 按 24 位取指为 3 字节、MSP430 为 2 字节、V850 按解码变长（2/4/6 字节），其余固定 4 字节。
  STM8 按操作码与前缀变长（NOP/PUSH/RET=1、短跳=2、`CALL`/直接16/`(off8,X)`=3、`CALLF`/`INT`=4、
  `0x90`/`0x91`/`0x92`/`0x72` 前缀按所修饰操作码累计），MSP430 为 2 字节操作码字加每个立即数/
  绝对/索引操作数扩展字（`MOV Rn,Rn`=2、`MOV #imm16,R5`=4、`PUSH #imm16`=4）；RL78 按操作码
  与前缀变长（无操作数=1、`#imm8`/`saddr`/`[HL+imm8]`=2、`#imm16`/`ES:[imm16]`/`saddr,#imm8`=3、
  `0x61` 前缀寄存器-寄存器=2、`0x61`+`#imm8` 型=3、`0x61`+`ES:[imm16]` 型=4，且与核心的
  `op & 0xF8` 分发行为一致——`0x61 0x59` 等按寄存器-寄存器家族解码为 2 字节）；RX 按操作码与
  寻址方式变长（`BRA.s`=1、`BCnd.s`/通用 2 字节、带 8 位位移/立即数=3、带 16 位位移/立即数=4、
  `BRA.a`/`BSR.a`=6），其反汇编与核心一致采用大端取指字并按 `op & 0xF8`/`op & 0xFC`/`op & 0xE0`
  家族分发；核心的 `MOV dsp[lds], dsp[ldd]` 家族收窄为 `op 0xC0-0xEF`，使位操作（0xF0/0xF4）、
  寄存器索引（0xF8/0xFA）与长操作码（0xFC）家族可正常执行与解码。
  该行为由 ns-sim 单测 `instruction_size_matches_encoding_width` 与 CLI 端到端
  `disasm_mcs51_code_space_width`/`disasm_mcs251_code_space_width`/`disasm_avr_32bit_width`/
  `disasm_c166_imm_width`/`disasm_xtensa_24bit_width`/`disasm_v850_variable_width`/
  `disasm_stm8_variable_width`/`disasm_msp430_extension_words`/`disasm_rl78_variable_width`/
  `disasm_rx_variable_width`/`disasm_rx_decodes_families`/`disasm_rl78_decodes_core_families`/
  `disasm_mcs51_ajmp_acall_addr11`/`disasm_avr_muls_lpm_spm_push` 锁定。
  ARM Thumb 全家族（Thumb-1/Thumb-2）另由 ns-sim 单测 `disassembles_thumb_full_families`/
  `disassembles_thumb2_wide_forms` 锁定：Thumb-1 覆盖执行内核全部 16 位家族，Thumb-2 覆盖
  BL/BLX、MOVW/MOVT、ADDW/SUBW、LDR/STR/LDRB/STRB/LDRH/STRH 12 位立即与 DMB/DSB/ISB
  内存屏障、数据处理（寄存器/修正立即）、位域 SBFX/UBFX/BFC/BFI 与 TBB/TBH/LDRD/STRD；`b`（0xE001）与
  `bl 0x28`（0xF000D000）分别按 2/4 字节推进。屏障（0xF3BF
  0x8F5F/8F6F/8F7F）在执行内核中为 no-op（原仅 DMB 被识别，DSB/ISB 会误 halt），
  由 ns-sim 单测 `execute_wide_ldrh_strh_and_barriers` 与 CLI `disasm_thumb2_ldrh_strh_barriers` 锁定。
  Thumb-2 分支目标现以架构 PC（当前 +4）为基址（原 `BL/BLX` 与 `B.W` 反汇编按当前地址算，
  比执行内核偏 4 字节）；`B.W` 解码修复为可到达（原 `first & 0x0C00 == 0x0800` 与
  `first & 0xF800 == 0xF000` 矛盾，使 `B.W` 永不匹配），现按 `second & 0xD000 == 0x9000`
  识别。由 ns-sim 单测 `disassembles_thumb2_wide_forms`/`bw_unconditional_branches` 与 CLI
  `disasm_thumb2_bl_branch_target` 锁定。
  Thumb-1 执行内核的字段提取同步修复为与编码一致：格式 0 取 Rm=bits5-3（bit6 属移位量）、
  格式 1 以 bit9 选 SUB/bit10 选立即且 Rn=bits5-3/Rm=bits8-6、格式 4 ALU 取 Rm=bits5-3、
  格式 5 取 Rn=bits5-3/Rm=bits8-6、格式 9/0xA 取 Rd=bits10-8、格式 0xC 取 Rn=bits10-8
  （原实现把 rn/rm 字段弄错或把 `sub`/`immediate` 两比特弄反）。由 ns-sim 单测
  `thumb1_field_extraction_matches_encoding`/`thumb1_high_register_and_sp_relative_fields`/
  `thumb1_register_offset_addressing` 与 CLI `run_arm_thumb1_field_extraction` 锁定。
  Thumb-1 格式 4 ALU 指令（`LSL/LSR/ASR/ADC/SBC/ROR/RSB/MUL`）现更新 N/Z 标志
  （原只更新 C 或完全不更新），与反汇编 `*s` 后缀一致；由 ns-sim 单测
  `thumb1_alu_sets_nz_flags` 锁定。
  Thumb-1 加减运算（`ADD/SUB` 立即/寄存器、`CMP/CMN/ADC/SBC/RSB`）现计算溢出标志 V
  （原恒为 0，使 `VS/VC/GE/LT/GT/LE` 有符号条件分支失效）；由 ns-sim 单测
  `thumb1_add_sub_sets_v_flag` 锁定。
  Thumb-1 移位（`LSL/LSR/ASR` 立即与寄存器）现对 `ASR #0`/`LSR #0`（即移位 32）与
  寄存器移位量 ≥32 按 ARM 语义处理（原 `asrs` 立即与寄存器移位量 ≥32 会因 Rust 移位
  溢出 panic，且 `ASR` 立即被误判为 `ADD/SUB` 组——`top=1` 且 bit11 清 0）。由 ns-sim
  单测 `thumb1_shift_zero_and_wide_amounts`/`disassembles_thumb_full_families` 与 CLI
  `run_arm_thumb1_asr_immediate` 锁定。
  Thumb-1 `LDMIA` 现写回基址寄存器（原只写回 `STMIA`）；当基址寄存器在加载列表中时不写回
  （加载值优先），与 A32 `arm_block_transfer` 一致；由 ns-sim 单测
  `thumb1_ldm_stm_writeback` 锁定。
  Thumb-1 的 PC 相对寻址（`LDR r,[pc,#imm]` 与 `ADD rd,pc,#imm`）现按架构 PC（当前地址
  +4，两条半字）取字对齐作为基址（原把下一指令地址按半字对齐，在 4 字节对齐/2 字节对齐
  两种边界各偏 4 字节），由 ns-sim 单测 `thumb1_pc_relative_uses_architectural_pc` 与
  CLI `run_arm_thumb1_pc_relative` 锁定。
  Thumb-1 条件分支（`0xD000-0xDFFF`）与无条件分支（`0xE000-0xE7FF`）目标现以架构 PC
  （当前地址 +4）为基址（原用下一指令地址按半字对齐，使分支目标偏低 2 字节），与
  PC 相对寻址及 Thumb-2 分支基址一致；由 ns-sim 单测 `unconditional_branch`/
  `conditional_branch_target_base` 与 CLI `run_arm_thumb1_branch_target` 锁定。
  ARM 条件标志（xPSR 的 N/Z/C/V 与 Thumb 位）与指令集状态（A32/Thumb）现随会话
  保存/恢复（`Session::capture`/`apply` 新增 `arm_xpsr`/`arm_arm_state` 字段），使
  `step --session` 分次调用仍能正确评估条件分支；由 CLI `run_arm_thumb1_branch_target`
  锁定。
  Thumb-1 高寄存器 `CMP rD,rM`（`0x4500-0x45FF`）现更新溢出标志 V（原只更新 N/Z/C），
  与 A32/Thumb `CMP` 的 N/Z/C/V 语义一致；由 ns-sim 单测 `thumb1_high_cmp_sets_v_flag`
  与 CLI `run_arm_thumb1_high_cmp_v_flag` 锁定。
  Thumb-1 `CBZ`/`CBNZ`（`0xB1xx`/`0xB9xx`，编码 `1011 i 0 0 1 imm5 Rn`）现按 Rn 为零/
  非零条件分支（目标 = 当前地址 + 4 + imm5*2），原被误当作 `ADD/SUB SP`（`insn & 0x0C00
  == 0` 判据过宽），会错误改写 SP；`ADD/SUB SP` 立即数现正确屏蔽 bit7（子/加标志），
  使 `sub sp,#imm` 不再把 0x200 混入偏移；`NOP`（0xBF00）现为空操作（原被当作 `POP`，
  会错误读取并改写 SP/PC）。由 ns-sim 单测 `thumb1_cbz_cbnz_branch`/
  `thumb1_add_sub_sp_immediate`/`thumb1_nop_no_effect` 与 CLI `run_arm_thumb1_cbz_branch`
  锁定。
  Thumb-1 符号/零扩展（`SXTH/SXTB/UXTH/UXTB`，`0xB200-0xB2FF`，编码 `1011 0010 0 op Rm Rd`，
  op=bits7-6，Rm=bits5-3，Rd=bits2-0）现正确执行（原被当作 `PUSH/POP`/`ADD/SUB SP`），
  Thumb-2 `MUL Rd,Rn,Rm`（`0xFB00` 前缀，second bits15-12=1111、bits7-4=0）现执行低 32 位
  乘积；由 ns-sim 单测 `thumb1_extend_sign_zero`/`thumb2_mul` 与 CLI
  `run_arm_thumb1_extend_and_mul` 锁定。
  Thumb-2 `MRS Rd,<apsr|ipsr|epsr|xpsr>`（0xF3EF 0x8Rd0SYSm）与
  `MSR <apsr|xpsr>_<mask>,Rm`（0xF380-0xF38F 0x8maskSYSm）现读写特殊寄存器：
  MRS 的 Rd 取 second bits11-8、SYSm 取 second bits3-0（xPSR=3/IPSR=5/EPSR=6/APSR=0），
  MSR 的源寄存器 Rm 取 first bits3-0、mask 取 second bits11-8（flags=mask8、GE=mask4）；
  由 ns-sim 单测 `thumb2_mrs_msr` 与 CLI `run_arm_thumb2_mrs_msr` 锁定。
  Thumb-1 `CPSIE/CPSID`（`0xB660-0xB67F`，编码 `1011 0110 0 i 0 1 0 0`，bit4=0 使能/1 禁用、
  bit1=`i` 选 PRIMASK）现读写 PRIMASK（CPSIE 清、CPSID 置，其余位保留），原被当作
  `REV` 家族执行；`REV/REV16/REVSH`（`0xBA00-0xBAFF`，编码 `1011 1010 0 op Rm Rd`，
  op=bits7-6：0=REV、1=REV16、2=REVSH、Rm=bits5-3、Rd=bits2-0）现执行字节/半字反序
  （REVSH 对反转后的半字符号扩展）；Thumb-2 `CLZ Rd,Rm`（0xFAB0-0xFABF 且
  second & 0x00FF == 0x0087，Rd=second bits11-8、Rm=first bits3-0）现返回前导零计数
  （0 输入返回 32）。由 ns-sim 单测 `thumb1_cps_primask`/`thumb1_rev_family`/
  `thumb2_clz` 与 CLI `run_arm_thumb1_rev_and_clz` 锁定。
  Thumb-2 寄存器偏移存取（`0xF800-0xF85F`，宽 bits7-5 选字/半字/字节、bit4 选载入，
  second 的 bits5-4 为 `LSL #imm2`）与字面量载入（`LDR` 0xF8DF / `LDRB` 0xF89F /
  `LDRH` 0xF8BF，基址 = Align(PC,4)+imm12）现正确执行（原把字面量误当作
  `[r15,#imm]` 立即数存取、寄存器偏移误判为 LDM/STM 宽格式）；`SDIV`（0xFB90 前缀）/
  `UDIV`（0xFBB0 前缀，second bits15-12=1111、bits7-4=1111，除零返回 0）现执行有/无
  符号除法；LDM/STM 宽格式（bits15-12=1110、bits11-9=100、bit6=0，P=bit8/U=bit7/W=bit5/
  L=bit4）现按 IA/IB/DA/DB 全部寻址方式执行并写回基址（载入时基址在列表中则写回被
  载入值覆盖）。由 ns-sim 单测 `thumb2_ldr_str_register_offset`/`thumb2_ldr_literal`/
  `thumb2_sdiv_udiv`/`thumb2_ldm_stm_multiple` 与 CLI `run_arm_thumb2_div_load_literal_and_ldm`
  锁定。
  Thumb-2 数据处理（寄存器，bits15-9 = `1110101`）现按 `op = bit8<<3 | bits7-5` 分发执行
  AND/BIC/ORR/ORN/EOR/ADD/ADC/SBC/SUB/RSB，S=bit4 更新 N/Z/C/V（逻辑运算 C 取自移位器、
  V 不变；加减运算按 ARM 进位/溢出语义）；`Rn=PC` 的 ORR/ORN 即 MOV/MVN（其 Rd 在
  bits3-0、Rm 在 bits11-8，与一般布局相反）；`S=1 且 Rd=PC` 的 AND/EOR/ADD/SUB 即
  TST/TEQ/CMN/CMP（只置标志不写回）；移位器支持 LSL/LSR/ASR #imm2（imm2=0 时 LSR/ASR
  取 #32）与 ROR #imm2（imm2=0 即 RRX）。由 ns-sim 单测
  `thumb2_data_processing_register`/`thumb2_mov_mvn_compare`/
  `thumb2_data_processing_shifted_register` 与 CLI `run_arm_thumb2_data_processing_register`
  锁定。
  Thumb-2 表分支 `TBB`/`TBH`（`0xE8D0-0xE8DF` 前缀、second bits15-12=1111，bit4 选
  TBH；Rn=first bits3-0、Rm=second bits3-0；表基址 = Align(PC,4)，PC 为当前地址 +4；
  TBB 读 `[Rn+Rm]` 字节索引、TBH 读 `[Rn+Rm,LSL #1]` 半字索引，目标 = 基址 + 2×索引）
  现正确执行（TBB 编码 `0xE8D7F008` = `tbb [r7,r8]`，基址按已前进的 PC 取字对齐，
  与 BL/LDR 字面量/PC 相对寻址的架构 PC 约定一致）；`LDRD`/`STRD`（bits15-9 =
  `1110100`、bit6=1、second bits15-12 ≠ 1111；Rn=first bits3-0、Rt=second bits15-12、
  Rt2=second bits11-8、偏移 = imm8×4、P=bit8/U=bit7/W=bit5/L=bit4）现执行双字传送
  （载入到 Rt/Rt2 或自 Rt/Rt2 存储，前/后索引与写回，载入时若 Rn 在寄存器对中则不写回），
  编码 `0xE9D20100` = `ldrd r0,r1,[r2]`。ARM 运行时数据空间写现记入会话
  `memory_overlay`（跨进程 `step --session` 可续跑并读回）。由 ns-sim 单测
  `thumb2_tbb_tbh`/`thumb2_ldrd_strd` 与 CLI `run_arm_thumb2_ldrd_strd_tbb` 锁定。
  Thumb-2 修正立即数据处理（bits15-11=`11110`、bit9=0；op=first bits8-5，与寄存器形式同
  表：AND=0/BIC=1/ORR=2/ORN=3/EOR=4/ADD=8/SUB=13/RSB=14，S=bit4、Rn=first bits3-0、
  i=bit10、imm3=second bits14-12、Rd=second bits11-8、imm8=second bits7-0，
  imm12=(i<<11)|(imm3<<8)|imm8）现执行；`Rn=PC` 的 ORR/ORN 即 MOV/MVN，`S=1 且 Rd=PC`
  的 AND/EOR/ADD/SUB 即 TST/TEQ/CMN/CMP（只置标志不写回）。立即按
  `thumb_expand_imm12` 扩展：imm12 bits11-10=00 时为字节重复模式（由 bits11-8 选
  00=0x000000XX、01=0x00XX00XX、10=0xXX00XX00、11=0xXXXXXXXX，进位沿用输入），
  否则为 `(byte|0x80)` 循环右移 `2×rot+(byte>>7)`（byte=imm12 bits7-0、
  rot=imm12 bits11-8，进位=结果 bit31）。逻辑运算 C 取自扩展进位、V 不变；加减按
  ARM 进位/溢出语义。反汇编按扩展值显示立即（`mov r0,#0xff`/`add r3,r4,#0x40` 等）。
  由 ns-sim 单测 `thumb2_data_processing_modified_immediate`/`thumb_expand_imm12_matches_encodings`
  与 CLI `run_arm_thumb2_modified_immediate`/`disasm_thumb2_modified_immediate` 锁定。
  Thumb-1 `IT`（If-Then，`0xBF00-0xBFFF`，cond=bits7-4、mask=bits3-0，mask=0 即 NOP）现
  正确执行条件块：块长度 = `4 - trailing_zeros(mask)`，第 0 条指令恒为 then（cond），
  第 i 条（i≥1）为 then 当 `(mask >> (4-i)) & 1 == 0`，否则 else（cond^1）；后续指令按
  该条件执行或跳过（跳过仅推进 PC），块结束自动清除。此前 IT 被误当作 POP 破坏 SP。
  `BKPT`（`0xBE00-0xBEFF`）现停机（原误当作 POP）。IT/BKPT 状态（mask/cond/index）加入
  会话持久化，跨进程 `step --session` 可续跑条件块。由 ns-sim 单测 `thumb1_it_block` 与
  CLI `run_arm_thumb1_it_bkpt`/`disasm_thumb1_it_bkpt` 锁定。
  Thumb-2 位域 `SBFX`/`UBFX`/`BFC`/`BFI`（首半字 `0xF340|Rn`、`0xF360|Rn`（Rn=PC 即
  BFC `0xF36F`）、`0xF3C0|Rn`；第二半字 `0 imm3(3) Rd(4) imm2(2) 0 lsb(5)`，
  `lsb=(imm3<<2)|imm2`）现执行与反汇编：`SBFX`/`UBFX` 提取 `[lsb:lsb+width-1]` 并按
  `width=lsb5+1` 有/零符号扩展；`BFC` 清除 `[msb:lsb]`（`msb=lsb5`）、`BFI` 将 Rn 低
  `width=msb-lsb+1` 位插入 `[msb:lsb]` 并保留 Rd 其余位。由 ns-sim 单测 `thumb2_bfx` 与
  CLI `run_arm_thumb2_bfx`/`disasm_thumb2_bfx` 锁定。
  Thumb-2 宽 `REV`/`REV16`/`REVSH`/`RBIT`（首半字 `1111 1010 1001 Rm`，第二半字
  `1111 Rd 10op2 Rm`，bits7-4 选择操作 8=REV/9=REV16/A=RBIT/B=REVSH，Rm 在首半字与
  第二半字低半字节重复）现执行与反汇编；宽 `SXTH`/`UXTH`/`SXTB`/`UXTB`（首半字
  `1111 1010 0op1 1111`，op=bits7-4：0=SXTH/1=UXTH/4=SXTB/5=UXTB；第二半字
  `1111 Rd 10RR Rm`，RR=bits5-4 旋转 `ROR #8RR`）现按旋转后结果有/零扩展。由 ns-sim
  单测 `thumb2_rev_family_and_extend` 与 CLI `run_arm_thumb2_rev_extend`/
  `disasm_thumb2_rev_extend` 锁定。
  Thumb-2 独占访问 `LDREX`/`STREX`/`LDREXB`/`STREXB`/`LDREXH`/`STREXH`（首半字
  bits11-8=`1000`、bit5=0、bits7-6=01（字）或 11（字节/半字）；字形式第二半字
  bits7-0=0、Rt=bits15-12、状态寄存器=bits11-8（存）/不用（载）；字节/半字形式
  第二半字 bits7-4=`0100`/`0101`、bits11-8=`1111`、Rt=bits15-12、状态寄存器=bits3-0
  （存）/不用（载））现执行与反汇编；单核 Cortex-M 独占监视恒授予，故 STREX 无条件
  写并在状态寄存器记录 0（成功）。由 ns-sim 单测 `thumb2_ldrex_strex` 与 CLI
  `run_arm_thumb2_ldrex_strex`/`disasm_thumb2_ldrex_strex` 锁定。
  Thumb-1 高寄存器 `BLX rX`（`0x47xx` bit7 置位）现链接返回地址到 LR（原与 `BX` 一样不写 LR），
  由 ns-sim 单测 `thumb1_bx_blx_register` 锁定。
  A32 操作数 2 的移位由 ns-sim 单测 `disassembles_a32_operand2_shifts` 与 CLI
  `disasm_a32_operand2_shifts` 锁定：数据传送与数据处理操作数 2 现按
  `ArmCore::arm_operand2` 解码 LSL/LSR/ASR/ROR 立即、RRX 与寄存器指定位移（原仅显示
  `lsl #imm` 且忽略移位类型与寄存器移位）；半字/有符号传送（LDRH/LDRSB/LDRSH/STRH）
  现区分立即与寄存器偏移（原把寄存器偏移当作立即）。
  A32 `MUL/MLA` 的目标寄存器现取 bits19-16（原误用 bits15-12，把 `mul r3,...`
  显示成 `mul r0,...`），由 `disassembles_a32_operand2_shifts`/`disasm_a32_operand2_shifts`
  用例锁定。
  A32 半字传送寄存器偏移由 ns-sim 单测 `arm_a32_halfword_transfer_register_offset`
  锁定执行内核语义（LDRH/STRH `[rn, rm]`）。
  A32 `LDRD`/`STRD`（双字传送，bit20=0 且 sh=2/3，寄存器对 Rd/Rd+1，偏移为 imm8 字节或
  Rm）现正确执行（原 `LDRD` 被当作 `LDRSB` 加载、`STRD` 被当作 `STRH` 半字存储），
  反汇编同时输出寄存器对；由 ns-sim 单测 `arm_a32_ldrd_strd` 锁定。
  RL78 反汇编同时镜像核心 `execute_61` 的 `op & 0xF8` 低组优先分发（`0x61 0x59` 等被寄存器-
  寄存器家族捕获为 `and a, A`，`0x61 0xAA`/`0xAB` 经 `0xA8` 家族路由为 `mov a, ES:[imm16]`），
  与核心执行语义逐字节一致。

- RX 长操作码族（`0xFC`/`0xFD`，寄存器-寄存器与立即数长格式）已按 binutils 权威编码表
  （`rx-decode.c`/`rx.h`）完整补齐。核心 `execute_fc`/`execute_fd` 的 `hi` 选择子改为从**第二
  字节**（操作选择子）取值、寄存器字段从**第三字节**（`op[2]`）读取，不再恒为 `0xC`/`0xD`。
  已实现的寄存器-寄存器整数家族：`SBB/NEG/ADC/ABS`、`MAX/MIN/EMUL/EMULU/DIV/DIVU/TST/XOR/NOT`、
  `XCHG/STZ/STNZ`、`BSET/BCLR/BTST/BNOT`（寄存器位操作）、`SHLR/SHAR/SHLL/ROTR/ROTL/REVW/REVL`、
  `MVTC/MVFC`、`MOV`（寄存器-寄存器，带 b/w/l 符号扩展）、`SC`（条件存寄存器）与立即数家族：`ROTR/ROTL #imm`、
  `SHLR/SHAR/SHLL #imm,rs,rd`、`ADC/MAX/MIN/EMULU/DIVU/TST/XOR/STZ #imm`、`MVTC #imm,cr`、
  `BM/BNOT #imm,rd`。`BM` 按 `cond` 将目标位写为条件值、`BNOT` 翻转目标位；`MVTC/MVFC` 按
  `c*16+rdst`/`s*16+rsrc` 解析控制寄存器，CR0 即 `psw`。内存寻址形式（`ss/sd != 3`）、浮点
  （`FSUB/FCMP/FADD/FMUL/FDIV/FTOI/ROUND/FSQRT/FTOU`）、64 位累加器（`MULHI/MULLO/MACHI/MACLO`
  等）、`MOVU/MOVCO/MOVLI` 与 DSP（`DMOV/SAVE/RSTR/MVFAC*`）因仿真器无 FPU/64 位累加器/
  原子内存模型而如实停机，不产生错误语义。反汇编器镜像同一家族：`0xFC`/`0xFD` 各分支输出
  对应助记符与操作数，`instruction_len`
  对 `0xFC` 返回 3 字节（操作码字+操作数），对 `0xFD` 返回 3 字节、立即数 ALU/MVTC 形式按 `im`
  字段返回 4/5/6/7 字节（`imm8`/`imm16`/`imm24`/`imm32`）。核心修正 `set_pc`/`reset`/
  `load_image`/RTS/中断返回/寄存器跳转的 `pc & !1` Thumb 对齐掩码（RX 的 `BRA.s` 为 1 字节，
  合法 PC 可为奇地址）。该行为由 ns-sim 单测 `fc_alu_register_forms`/`fc_bit_register_forms`/
  `fc_bm_sc_immediate`/`fd_shift_rotate_forms`/`fd_imm_shifts_and_rotates`/`fd_mvtc_mvfc`/
  `fd_imm_alu`/`fd_bm_immediate`/`disassembles_fc_fd_long_forms`/`instruction_len_fc_fd` 与 CLI
  端到端 `disasm_rx_fc_fd_long_forms`/`step_rx_fc_fd_long_forms` 锁定。

- `step-over`/`step-out` 通过 `is_call_mnemonic`/`is_return_mnemonic` 识别调用与返回，并用
  `instruction_size` 计算返回地址。RX 的 `bsr.w`/`bsr.a` 已纳入调用助记符，`step-over` 在
  `bsr.w`（4 字节）后回到 `pc+4`；核心修正 `BRA.a`/`BSR.a`、`BRA.w`/`BSR.w` 的 link 标志
  （`BSR` 入栈返回地址、`BRA` 不入栈）。由 ns-sim 单测 `branch_link_flags`/
  `step_over_and_step_out_use_rx_call_width` 与 CLI 端到端 `rx_step_over_uses_call_width` 锁定。

---

## 6. 参考实现原则（复制自 ns-embed-ide）

1. **只复制能力，不复制工程**：`crates/ns-sim`、`crates/ns-device` 源码与独立 `Cargo.toml`；
   删除对上级 workspace 的依赖声明（`version.workspace`/`edition.workspace`），改为本仓库固定值。
2. **保持内部 API 稳定**：复制时保留 `pub fn` 签名，避免 IDE 侧既有语义漂移；仅在 CLI 层做适配。
3. **禁止仓库外引用**：`Cargo.toml` 不得出现 `../` 的 path 依赖；一切依赖走 crates.io 或本仓库内。
4. **版本独立**：`ns-sim`/`ns-device` 版本随本仓库统一 `0.1.0`，不继承外部版本号。
