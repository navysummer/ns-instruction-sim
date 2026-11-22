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
| `device` | `memory-map` | 列出设备内存区域（FLASH/RAM/PERIPHERALS），可用 `--addr` 查询地址所属区域 | `device [--addr] [--json]` | 表/JSON |
| `device` | `lookup` | 按地址或 `PERIPH[.REG]` 名反查设备内存区域/外设/寄存器（内置 SVD），含寄存器内偏移与位域字段 | `device --addr | --name [--json]` | 表/JSON |
| `device` | `peripherals` | 列出设备全部外设概览（名称/基址/寄存器数/描述），可用 `--filter` 按名称子串筛选（大小写不敏感） | `device <DEVICE> [--filter] [--json]` | 表/JSON |
| `device` | `peripheral` | 显示单个外设的寄存器明细（名称/偏移/尺寸/复位值/位域字段），`--register` 只看单个寄存器 | `device <DEVICE> <PERIPH> [--register] [--json]` | 表/JSON |
| `run` | — | 加载固件并运行到结束/步数上限/指定步数/指定地址/指定符号/条件成立/周期上限（可指定会话保留外设/SVD 寄存器；`--trace-regs` 打印每条指令后变化的寄存器；`--max-cycles` 到达即停并报 `lastResult="cycles"`） | `--firmware --session --arch --base --max-steps --steps --until --until-symbol --until-condition --max-cycles --cycle-accurate --save-session --json --trace --trace-regs --source` | 状态 |
| `step` | — | 单步一条指令 | `--session --arch --firmware --base --json --source` | 状态 |
| `step-over` | — | 步过（跳过调用） | 同上 | 状态 |
| `step-out` | — | 步出（运行至返回） | 同上 | 状态 |
| `continue` | — | 连续运行至断点/停止/上限/指定地址/指定符号/条件成立/周期上限（临时断点/条件每步后求值；`--trace-regs` 打印每条指令后变化的寄存器；`--max-cycles` 到达即停并报 `lastResult="cycles"`） | `--session --until --until-symbol --until-condition --max-steps --max-cycles --save --json --trace --trace-regs --source` | 状态 |
| `reg` | `list` / `get` / `set` | 寄存器列出/读/写 | `--name --value --arch --filter --session --json` | 表/值/JSON |
| `mem` | `read` / `write` / `fill` / `search` / `load` / `dump` / `region` / `copy` | 内存读/写/填充（含确定性伪随机；`fill` 的 `--pattern` 重复填充字节模式）/搜索模式（`search` 的 `--pattern` 按 `--width` `byte`/`half`/`word`/`dword` 解释为小端整数值，可直接按字/双字常量搜索；`--mask` 按相同宽度展开为逐字节掩码，某字节掩码为 0 表示通配）/加载固件/导出（C 数组 / Intel HEX 可回载 / hex+ASCII 视图）/按设备命名区域转储（FLASH/RAM/PERIPHERALS，起始地址从目录解析，偏移/长度校验）/复制内存区域（源→目标，重叠范围按 memmove 语义）；`read`/`write`/`fill`/`search` 均可用 `--device`+`--region`+`--offset` 按设备命名区域寻址（与 `--addr` 互斥）或 `--symbol` 按 ELF 数据符号起始地址寻址（与 `--addr`/`--device` 互斥）；`read` 可用 `--ascii`（按 ASCII 文本解读，非可打印显示为 `.`）或 `--string`（读取 NUL 结尾字符串，最多 `--len` 字节），`write` 可用 `--string`（写入字符串 ASCII 字节，`--null` 追加 NUL 终止符） | `--addr --len --bytes --values --file --value --random --seed --pattern --firmware --base --arch --device --region --offset --symbol --src --dst --name --format --width --ascii --string --null --mask --out --json` | hex/C/JSON |
| `bp` | `list` / `set` / `modify` / `clear` / `clear-all` / `enable` / `disable` / `hits` | 断点管理（条件/命中次数/一次性/启用禁用/按符号或行号设置；`set` 可用 `--device`+`--region`+`--offset` 按设备命名内存区域解析地址；`modify` 更新既有断点的条件与命中次数而不重建，保留运行时命中计数；`hits` 预置断点运行时命中计数，使命中次数断点按新计数继续；`list --symbols` 用 ELF 符号注释断点地址） | `--addr --symbol --line --device --region --offset --condition --hit-count --once --count --symbols --session --json` | 表/JSON |
| `watch` | `set` / `list` / `modify` / `clear` / `clear-all` / `enable` / `disable` / `hits` | 内存观察点管理（`step`/`run`/`continue`/`trace`/`coverage`/`profile` 在每条指令前后比较被监视字节范围，被写入时暂停并报 `lastResult`/`result="watchpoint"`、`watchHit=<地址>`；支持长度/命中次数/一次性/启用禁用；命中计数持久化；`modify` 更新既有观察点的长度/命中次数/一次性而不重建，保留运行时命中计数；`hits` 预置观察点运行时命中计数，使命中次数观察点按新计数继续；`set` 检测与既有观察点的地址范围重叠并返回 `overlaps`/`overlapCount`；`set` 可用 `--symbol`+`--offset` 按 ELF 符号解析监视地址或 `--device`+`--region`+`--offset` 按设备命名区域解析监视地址；`list --symbols` 用 ELF 符号注释观察点地址） | `--addr --symbol --device --region --offset --len --hit-count --once --count --symbols --session --json` | 表/JSON |
| `disasm` | — | 反汇编 N 条（可指定 ARM 状态/来源标注/符号标签/符号起始/原始字节/JSON；`--firmware` 直接加载固件反汇编，无需先建会话，架构从 ELF 推导） | `--firmware --base --arch --addr --symbol --count --arm-state --source --labels --raw --out --json --session` | 文本/JSON |
| `reset` | — | 复位仿真器（PC 回入口） | `--session --entry --json` | 文本/JSON |
| `state` | — | 完整状态（JSON/文本/字段子集/单行摘要；`--mem` 附带指定起始地址的内存快照，人读模式输出 hex dump；JSON 含 `watchpoints` 完整观察点快照） | `--session --arch --firmware --base --mem --mem-len --json --fields --summary` | 状态/JSON/摘要 |
| `periph` | `status` / `config` / `uart` / `gpio` / `timer` / `reg-read` / `reg-write` / `reg-list` / `reg-dump` / `field-get` / `field-set` / `svd-load` | 外设状态/基址配置/控制/寄存器直读写/列出已加载寄存器/位域读写/SVD 加载；`config` 可重定位 UART/GPIO/定时器基址（`--timer-base` 同时更新已有定时器并用于后续创建）；`uart` 可注入输入字节（`--input`/`--bytes`，固件从 UART 数据寄存器读取，FIFO 消费），`--peek` 查看下一条待读字节而不消费、`--rx-drain` 清空接收 FIFO；`reg-dump --fields` 附带每个寄存器的 SVD 位域解码（位偏移/宽度/当前值/描述）；`timer` 可直接设置/读取运行时计数器（`--counter`）与更新计数（`--update-count`），`--list` 列出已配置的全部定时器（只读，不创建，与配置参数互斥）；`gpio` 可设置引脚方向/输出电平（`--direction`/`--output`）并读取输入/输出/方向（`--read`），`--events` 查询输出变化事件日志（cycle/pin/value/pc）、`--clear-events` 清空该日志（两者均持久化） | `--uart-base --gpio-base --timer-base --pin --value --direction --output --read --events --clear-events --index --enabled --prescaler --auto-reload --counter --update-count --list --addr --name --len --bytes --input --peek --rx-drain --file --device --peripheral --register --field --fields --force --session --json` | 表/JSON |
| `symbol` | `list` / `at` / `nearest` / `line` / `addr` | ELF 符号与源行号映射（`nearest` 查询地址不大于给定地址的最近符号，含 `offset`） | `--filter --addr --file --line --session --json` | 表/JSON |
| `cycle` | `get` / `estimate` / `tick` / `set` | 周期计数/指令周期估算/外设推进/设置周期计数（模拟时间流逝/超时）；`estimate` 可用 `--symbol` 按 ELF 符号解析地址 | `--addr --symbol --cycles --count --session --json` | 值/JSON |
| `interrupt` | `take` / `pending` / `vector` / `set` / `clear` | 中断触发/查询/向量表/置线/清除全部挂起；`vector` 返回 mtvec 基址与模式（`direct`/`vectored`）及每行目标地址；`clear` 清空全部挂起线并持久化（保留使能状态） | `--line --value --session --json` | 值/JSON |
| `watchdog` | `status` / `feed` / `config` / `reset` | 看门狗查询/喂狗/配置/强制复位 | `--enabled --timeout --base --entry --session --json` | 表/JSON |
| `csr` | `list` / `get` / `set` | RISC-V machine-mode CSR 读写 | `--name --value --session --json` | 值/JSON |
| `session` | `info` / `diff` / `patch` | 会话元信息（架构/PC/周期/寄存器数/断点/内存/外设/固件/CSR）或比较两个会话的全部差异；`patch` 直接修改 PC/周期/停机/寄存器/CSR 并保存，ARM 会话还可设置指令集状态（`--arm-state`）、xPSR（`--xpsr`）与 If-Then 块（`--it mask,cond,index`） | `--file` / `--a --b --limit` / `--pc --cycle --halted --reg --csr --arm-state --xpsr --it [--json]` | 文本/JSON |
| `session` | `merge` | 合并两个会话（A 为基准、B 的运行时状态优先；寄存器/内存/断点/观察点/外部寄存器/定时器按地址并集 B 胜出，UART 输入输出与看门狗取 B 非空值，GPIO 事件拼接、IRQ 行并集；`--regs-keep` 保留 A 的寄存器文件） | `--a --b --out [--regs-keep] [--json]` | 文本/JSON |
| `firmware` | `info` | 固件元信息（格式/大小/架构/入口/符号表/源文件/首条指令反汇编），ELF 自动检测架构 | `--firmware --arch [--json]` | 文本/JSON |
| `firmware` | `verify` | 计算/校验固件镜像完整性校验和（CRC32/CRC16-CCITT/Adler32/FNV-1a/加和），基于原始文件字节；`--expected` 按 `--algo`（缺省 crc32）比对，不匹配非零退出 | `--firmware --arch --algo --expected [--json]` | 文本/JSON |
| `firmware` | `symbols` | 列出 ELF 符号表与源文件（无需会话；非 ELF 固件无符号），`--filter` 按名称子串筛选 | `--firmware --arch --filter [--json]` | 文本/JSON |
| `trace` | — | 运行并输出结构化指令轨迹（每条指令的 PC/反汇编/原始字节/来源/周期/寄存器快照；`--max-cycles` 到达即停并报 `result="cycles"`；内存观察点被写入时停并报 `result="watchpoint"`、`watchHit=<地址>`；`--top N` 仅返回前 N 条轨迹） | `--session --arch --firmware --base --steps --max-steps --max-cycles --until --until-symbol --top --regs --out --json` | 轨迹/JSON |
| `coverage` | — | 运行并统计执行覆盖（各 PC 执行次数、唯一 PC 数；`--max-cycles` 到达即停并报 `result="cycles"`；内存观察点被写入时停并报 `result="watchpoint"`、`watchHit=<地址>`；`--top N` 按命中次数降序仅返回前 N 个 PC，`uniquePcs` 仍为全量） | `--session --arch --firmware --base --steps --max-steps --max-cycles --until --until-symbol --top --json` | 覆盖表/JSON |
| `profile` | — | 运行并统计指令助记符直方图（`--max-cycles` 到达即停并报 `result="cycles"`；内存观察点被写入时停并报 `result="watchpoint"`、`watchHit=<地址>`；`--top N` 按计数降序仅返回前 N 种助记符，`uniqueMnemonics` 仍为全量） | `--session --arch --firmware --base --steps --max-steps --max-cycles --until --until-symbol --top --json` | 直方图/JSON |
| `mcp` | — | 启动 MCP（Model Context Protocol）stdio 服务器，供大模型通过工具调用完整驱动仿真 | `--session`（默认会话文件） | JSON-RPC 2.0 / 工具结果 |

> `trace` / `coverage` / `profile` / `disasm` 均支持 `--out FILE`，把 JSON 或文本结果直接写入文件
> （stdout 仅打印一行确认），便于把仿真产物落到磁盘供后续工具消费。

### 3.2 输出格式约定

1. **人类可读**（默认）：表格/文本，适合终端交互。
2. **机器可读**（`--json`）：稳定字段的 JSON 对象（`arch`、`pc`、`cycleCount`、`halted`、
   `lastResult`（`advanced`/`breakpoint`/`halted`/`condition`——`condition` 表示
   `--until-condition` 表达式成立而停）、`registers`、`armState`（仅 ARM，`a32`/`thumb`，其余架构 null）、`uartText`、`uart`、
   `armXpsr`（仅 ARM：N/Z/C/V 条件标志、Thumb 位、当前异常号的 xPSR 原值，其余架构 null）、
   `armItState`（仅 ARM：`{mask,cond,index}`，`mask==0` 表示无活动 IT 块；其余架构 null，
   用于在 Thumb-2 IT 块中途观察剩余条件指令）、`pcLimit`（本架构可执行的最大 PC 地址，
   16 位 PC 架构为 0xFFFF，其余为 u32::MAX）、
   `peripherals`（UART 输出/`uartInput`（未消费的注入输入字节）/GPIO/定时器/看门狗/中断/`externalRegs`）、
   `gpioEvents`（GPIO 输出变化事件：cycle/pin/value/pc）、
   `csr`（mstatus/mtvec/mepc/mcause/mie/mhartid）、`breakpoints`（地址/条件/命中次数/一次性/启用状态/实际命中）、
   `watchpoints`（内存观察点：地址/长度/命中次数/一次性/启用状态/实际命中；`state --json`）、
   `watchHit`（本步触发观察点的地址，未触发为 null）、
   `firmware`（path/base/format，无则 null）、`pcSymbol`（最近 ELF 符号 name/address/size，无则 null）），
   字段名不随版本随意变动。
3. **错误**：一律写 stderr，格式 `错误: <消息>`，进程退出码 `1`。成功的 `--json` 退出码 `0`。
4. **会话文件**：JSON，含 `arch`、`pc`、`cycle_count`、`registers`、`memory`、`breakpoints`、
   `watchpoints`（地址/长度/命中次数/一次性/启用状态/实际命中）、
   `memory_overlay`（显式内存写，跨架构重放）、`peripherals`（含 `external_regs`、`gpio_events`（cycle/pin/value/pc）与
   `uart_input` 未消费输入字节）、`csr`、`firmware`（路径/格式/基址）。
   缺失路径时自动新建临时会话。

### 3.3 新增命令必须满足

- 用 `clap` derive 声明参数，参数名用 kebab-case；提供位置参数与 `--flag` 双形式（便于脚本与交互）。
- 每个命令必须同时给出**人类可读**与 `--json` 两种输出（涉及状态/列表的命令），机器字段稳定。
- 无会话时可接受的命令必须提供 `--arch` 或 `--firmware` 以自举；需要既有会话的命令必须校验会话存在。
- 错误路径必须如实返回错误（不 panic、不静默、不打印成功假象）。
- 新功能必须在 `tests/cli.rs` 增加端到端用例，驱动真实二进制验证输出。

### 3.4 MCP（Model Context Protocol）服务器

`ns-instruction-sim mcp [--session default.json]` 在 stdin/stdout 上实现 JSON-RPC 2.0 的
MCP **stdio** 传输（`Content-Length` 帧），使大模型（或任何 MCP 客户端）能通过工具调用
完整驱动指令级仿真，无需逐个生成 CLI 进程。工具参数与结果复用 CLI 的 `--json` 字段约定。

1. **握手**：客户端发送 `initialize`（`protocolVersion`），服务器返回
   `protocolVersion`（`2024-11-05`）、`capabilities.tools` 与 `serverInfo`；随后客户端发送
   `notifications/initialized`（无响应）。`ping` 返回空。
2. **`tools/list`**：返回全部工具（≥50），每个工具含 `name`、`description`、`inputSchema`
   （JSON Schema），`inputSchema` 为参数提供机器可读的帮助。
3. **`tools/call`**：`{name, arguments}` → 成功返回
   `{content:[{type:"text",text}], structuredContent, isError:false}`；工具内部错误返回
   `{content:[{type:"text",text:"错误: ..."}], isError:true}`（不是 JSON-RPC 错误）。未知方法
   返回 JSON-RPC `-32000` 错误。通知（无 `id`）不返回响应。
4. **会话模型**：大多数工具以 `session` 参数指向 JSON 会话文件；未传时使用
   `mcp --session` 的默认会话。每个修改状态的工具（step/run/mem/bp/periph/watchdog/csr/...）
   在返回前将会话保存回磁盘，因此跨工具调用状态持续。`session_create`、`session_load`、
   `session_save` 显式管理会话；`state` 读取完整快照。
5. **工具覆盖**：`arch_list`、`device_list`、`device_info`、`device_svd/linker/startup/init`、
   `session_create/load/save`、
   `state`（支持 `mem`/`mem_len` 附带内存快照）、`step`、`step_over`、`step_out`、
   `run`/`continue`/`trace`/`coverage`/`profile`（均支持 `max_cycles` 周期上限，`lastResult`/`result="cycles"`；
   均响应内存观察点，被写入时 `lastResult`/`result="watchpoint"` 且返回 `watchHit`；
   均支持 `until`/`until_symbol` 停止目标）、`reg_list/get/set`、
   `mem_read/write/fill/search/load/dump/region/copy`
   （`mem_search` 支持 `width` 参数，`pattern` 值按 `byte`/`half`/`word`/`dword` 小端展开，
   返回 `count`/`patternTokens`，`mask` 按相同宽度展开为逐字节掩码（0 为通配）；
   `mem_search`/`mem_dump` 支持 `out` 把结果写入文件并返回 `file`/`bytes`）、
   `mem_read/write/fill/search` 支持 `device`/`region`/`offset` 按设备命名区域寻址（与 `addr` 互斥）、
   `mem_read/write/fill/search` 支持 `symbol` 按 ELF 数据符号起始地址寻址（与 `addr`/`device` 互斥）、
   `mem_read` 支持 `ascii`/`string` 返回 `text`（NUL 结尾字符串或 ASCII 视图）、
   `mem_write` 支持 `string`/`null`（写字符串 ASCII 字节并可追加 NUL）、
   `mem_fill` 支持 `pattern`（重复填充字节模式）、
   `bp_set/modify/clear/list/enable/disable/clear_all`
   （`bp_list` 支持 `symbols` 参数；`bp_hits` 预置断点运行时命中计数；
   `bp_set` 支持 `device`/`region`/`offset` 按设备命名区域寻址）、
   `watch_set/list/modify/clear/enable/disable/clear_all`（内存观察点；`watch_set` 支持
   `len`/`hit_count`/`once`/`symbol`/`offset`（按 ELF 符号解析监视地址）或
   `device`/`region`/`offset`（按设备命名区域解析监视地址），
   `watch_set` 返回 `overlaps`/`overlapCount` 报告既有范围重叠，
   `watch_modify` 更新长度/命中次数/一次性并保留运行时命中计数，
   `watch_list` 支持 `symbols` 参数用 ELF 符号注释观察点地址，
   `watch_hits` 预置观察点运行时命中计数，
   `state` 返回 `watchpoints` 数组），
   `disasm`、`reset`、`cycle_get/estimate/tick/set`（`cycle_estimate` 支持 `symbol` 参数，
   从会话固件符号表解析地址）、`interrupt_take/pending/vector/set/clear`（`interrupt_vector`
   返回 `base`/`mode`/`vectors`；`interrupt_clear` 清空全部挂起线并持久化）、
   `periph_status/config/uart/gpio/timer/reg_read/reg_write/reg_list/reg_dump/field_get/field_set/svd_load`
   （`periph_config` 支持 `uart_base`/`gpio_base`/`timer_base` 重定位并持久化；
   `periph_reg_dump` 默认返回 `fields` 位域解码，`fields: false` 省略；
   `periph_uart` 支持 `peek` 查看下一条待读字节与 `rx_drain` 清空接收 FIFO；
   `periph_timer` 支持 `counter`/`update_count` 直接设置运行时计数器与更新计数，
   `list` 列出已配置的全部定时器（只读，与配置参数互斥）；
   `periph_gpio` 支持 `direction`/`output`/`read` 设置方向、输出并读取引脚状态，以及
   `events`/`clear_events` 查询/清空输出变化事件日志（均持久化））、
   `symbol_list/at/nearest/line/addr`、`watchdog_status/feed/config/reset`、`csr_list/get/set`。
   `trace`、`coverage`、`profile` 与 CLI 同名子命令一一对应（`analyze_schema` 共享参数），
   返回结构化轨迹/覆盖/直方图 JSON，并支持 `top` 参数（trace 按顺序、coverage/profile 按次数降序）
   与 `out` 参数把结果写入文件；
   `device_memory_map`（设备内存区域与地址所属区域）、`device_lookup`（按地址或 `PERIPH[.REG]` 名反查区域/外设/寄存器）、
   `device_peripherals`（按 `filter` 名称子串列出设备外设概览：名称/基址/寄存器数/描述）、
   `device_peripheral`（显示单个外设的寄存器明细：名称/偏移/尺寸/复位值/位域字段，`register` 只选一个）、
   `session_info`（会话元信息）、
   `session_diff`（两个会话文件的完整差异：标量/寄存器/断点/内存）、
   `session_merge`（合并两个会话：A 为基准、B 的运行时状态优先；`regs_keep` 保留 A 的寄存器文件）、
   `session_patch`（直接修改会话的 PC/周期/停机/寄存器/CSR 并保存；ARM 会话支持
   `arm_state`/`xpsr`/`it_mask`/`it_cond`/`it_index` 设置执行状态）、
   `firmware_info`（固件元信息：格式/架构/入口/符号表/首条指令）补齐设备与会话维度的工具。
   `firmware_verify`（固件镜像校验和：CRC32/CRC16-CCITT/Adler32/FNV-1a/加和，`expected` 按
   `algo` 缺省 crc32 比对并返回 `verified`）补齐固件完整性维度。
   `firmware_symbols`（ELF 符号表与源文件列表，`filter` 按名称子串筛选）补齐符号查询维度。

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

MCP 服务器的协议层另有 13 个单元测试（`src/mcp.rs` `#[cfg(test)]`），覆盖 `Content-Length`
帧解析/写出、`initialize`/`ping`、未知方法/未知工具/缺参错误、`tools/list` 完整 schema、
`tools/call` 成功与工具错误（`isError:true`）路径以及参数类型提取。

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
- `cycle_estimate_by_symbol`：`cycle estimate --symbol` 按 ELF 符号解析地址并估算周期
  （含未知符号与 `--addr`/`--symbol` 互斥错误路径）。
- `mcp_cycle_estimate_symbol`：MCP `cycle_estimate` 的 `symbol` 参数解析并返回地址/周期，
  未知符号与 `addr`+`symbol` 冲突以 `isError:true` 返回。
- `cycle_set_roundtrip`：`cycle set --count` 直接设置周期计数（位置参数/`--count` 双形式），
  `state --json` 可见持久化；缺周期数报错。
- `mcp_cycle_set`：MCP `cycle_set` 设置后 `cycle_get` 读回一致。
- `csr_set_get_roundtrip`：CSR 写读往返与会话持久化。
- `watchdog_config_feed_status`：看门狗配置/喂狗（复位 elapsed 计数并持久化）/状态。
- `watchdog_force_reset`：`watchdog reset` 记录复位事件并复位核心（持久化）。
- `watchdog_reset_default_goes_to_entry`：`watchdog reset` 未指定 `--entry` 时回到固件/架构入口
  （而非看门狗触发时的 PC），仍记录 `lastResetPc`。
- `interrupt_set_pending_and_take`：中断置线/挂起查询/触发跳转（mtvec/mcause）。
- `interrupt_vector_query`：`interrupt vector` 返回 mtvec 基址/模式（`direct`/`vectored`）与
  每行目标地址。
- `mcp_interrupt_vector`：MCP `interrupt_vector` 返回 `base`/`mode`/`vectors`，支持向量模式地址计算。
- `interrupt_clear_pending`：`interrupt clear` 清空全部挂起线（`cleared` 回报数量、`pendingLines` 为空、
  人读 `已清除 N 条`），跨调用持久化。
- `mcp_interrupt_clear`：MCP `interrupt_clear` 清空全部挂起线并持久化，`interrupt_pending` 读回 `pending: []`。
- `bp_list_shows_hits`：断点命中次数持久化并展示。
- `bp_once_autoclears`：`bp set --once` 命中后自动清除，再次 `continue` 不再停靠。
- `bp_enable_disable_roundtrip`：`bp disable` 不再暂停，`bp enable` 恢复暂停。
- `bp_modify_updates_existing_breakpoint`：`bp modify` 更新条件与命中次数但保留运行时命中计数
  （先走到断点一次 `hits=1`，再 `--condition`/`--hit-count` 改属性后 `hits` 仍为 1；
  仅改一个属性时另一属性保留；无属性/断点不存在/16 位 PC 越界均报错）。
- `mcp_bp_modify_tool`：MCP `bp_modify` 修改 `condition`+`hit_count` 并在 `bp_list` 读回持久化，
  缺属性与断点不存在以 `isError:true` 返回。
- `bp_hits_sets_runtime_counter`：`bp hits` 预置断点运行时命中计数（`--hit-count 2` 首轮不暂停，
  `bp hits --count 1` 后下一轮命中即暂停；位置参数/`--count` 双形式；缺计数/断点不存在/缺地址均报错）。
- `mcp_bp_hits_tool`：MCP `bp_hits` 设置命中计数并在 `bp_list` 读回持久化，
  缺 `count` 与断点不存在以 `isError:true` 返回。
- `bp_set_by_symbol`：`bp set --symbol` 按符号名解析地址设置断点（含未找到错误路径）。
- `bp_set_by_line`：`bp set --line file:line` 按源码行号设置断点（含非法行号错误路径）。
- `bp_set_device_region`：`bp set --device --region --offset` 按设备命名内存区域解析断点地址
  （ARM FLASH @ 0x08000000，`bp list` 读回一致；未知区域与缺 region 均报错）。
- `mcp_bp_set_device_region`：MCP `bp_set` 的 `device`/`region`/`offset` 参数按命名区域寻址，
  未知区域以 `isError:true` 返回。
- `bp_management_json`：`bp set/clear/clear-all/enable/disable --json` 机器可读确认。
- `bp_list_symbols`：`bp list --symbols` 用 ELF 符号注释断点地址（JSON `symbol` 字段 + 人读后缀），
  未加 `--symbols` 时 `symbol` 为 null。
- `mcp_bp_list_symbols`：MCP `bp_list {"symbols":true}` 返回符号注释，缺省为 null。
- `bp_set_out_of_range_rejected`：16 位 PC 架构（mcs51/c166/avr）越界断点地址被拒绝，不静默回绕；`--until` 同校验。
- `watch_set_step_and_continue`：`watch set` 后 `step` 在写入被监视范围时暂停
  （`lastResult="watchpoint"`、`watchHit=<地址>`），命中计数持久化，`state --json` 含
  `watchpoints`，`continue` 在无新写入时正常走到停机。
- `watch_hit_count_and_once`：`--hit-count N` 只在第 N 次写入时暂停（前面的写入被观测但不暂停）、
  `--once` 命中后自动禁用（后续写入不再暂停）。
- `watch_enable_disable_clear`：`disable` 使观察点惰性（不再暂停）、`enable` 恢复、`clear` 移除；
  清除不存在的观察点与 `--len` 越界（>64）均报错；`clear-all` 在空列表时报告 0 个移除。
- `mcp_watch_tools`：MCP `watch_set/list/clear/enable/disable/clear_all` 与 `state` 的
  `watchpoints` 数组；`continue` 停在观察点（`lastResult`/`watchHit`），`watch_set` 的 `len>64`
 以 `isError:true` 返回。
- `watch_modify_updates_existing`：`watch modify` 更新长度/命中次数/一次性但不重建观察点，
  保留运行时命中计数（先走一次写入 `hits=1`，再改属性后 `hits` 仍为 1）；
  无属性/观察点不存在/`--len` 越界均报错。
- `mcp_watch_modify`：MCP `watch_modify` 修改 `len`/`hit_count`/`once` 并在 `watch_list`
  读回持久化；缺属性、观察点不存在与 `len>64` 以 `isError:true` 返回。
- `watch_hits_sets_runtime_counter`：`watch hits` 预置观察点运行时命中计数
  （`--hit-count 2` 首轮写入不暂停，`watch hits --count 1` 后下一轮写入即暂停并报
  `lastResult="watchpoint"`/`watchHit`；位置参数/`--count` 双形式；
  缺计数/观察点不存在/缺地址均报错）。
- `mcp_watch_hits_tool`：MCP `watch_hits` 设置命中计数并在 `watch_list` 读回持久化，
  缺 `count` 与观察点不存在以 `isError:true` 返回。
- `mem_write_preserves_watchpoints`：`mem write`/`bp set`/`reg set` 等会话改写命令
  保留已持久化的内存观察点（`watch list` 仍可见，命中计数不变）。
- `mcp_mem_write_preserves_watchpoints`：MCP `mem_write`（经 `with_sim` 保存）保留观察点，
  `watch_list` 仍返回该观察点。
- `watch_overlap_reported`：`watch set` 在地址范围与既有观察点重叠时返回
  `overlaps`/`overlapCount`（JSON），不重叠时为 0。
- `mcp_watch_overlap`：MCP `watch_set` 返回 `overlaps`/`overlapCount`，报告与既有观察点的重叠。
- `watch_set_by_symbol`：`watch set --symbol`（可配 `--offset`）按 ELF 符号解析监视地址；
  未知符号与 `--addr`+`--symbol` 冲突报错。
- `mcp_watch_set_by_symbol`：MCP `watch_set` 的 `symbol`/`offset` 参数解析地址，未知符号以
  `isError:true` 返回。
- `watch_list_symbols`：`watch list --symbols` 用 ELF 符号注释观察点地址（JSON `symbol`，
  未命中符号为 null；人读模式追加符号名），不带 `--symbols` 时 `symbol` 为 null。
- `mcp_watch_list_symbols`：MCP `watch_list {"symbols":true}` 返回符号注释，缺省为 null。
- `watch_set_by_region`：`watch set --device/--region`（可配 `--offset`）按设备命名区域解析
  监视地址（FLASH@0 → 0x08000000、RAM@8 → 0x20000008）；未知区域、`--device` 缺 `--region`
  与 `--offset` 无 `--symbol`/区域均报错。
- `mcp_watch_set_by_region`：MCP `watch_set` 的 `device`/`region`/`offset` 参数解析地址并持久化；
  未知区域与 `device` 缺 `region` 以 `isError:true` 返回。
- `analyze_stops_on_watchpoint`：`trace`/`coverage`/`profile` 在内存观察点被写入时停止
  （`result="watchpoint"`、`watchHit=<地址>`），命中计数持久化。
- `mcp_analyze_stops_on_watchpoint`：MCP `trace` 同样停在观察点并返回 `watchHit`，`state` 反映
  持久化命中计数。
- `periph_timer_config_and_uart_write`：定时器配置与 UART 寄存器写输出。
- `periph_timer_counter_set_readback`：`periph timer --counter`/`--update-count` 直接设置定时器
  运行时计数器与更新计数，并在独立调用中读回持久化值。
- `mcp_periph_timer_counter`：MCP `periph_timer` 的 `counter`/`update_count` 参数设置后读回一致。
- `periph_timer_list`：`periph timer --list` 列出已配置的全部定时器（JSON `count`/`timers` 数组、
  人读 `定时器数量`），只读且与配置参数互斥。
- `mcp_periph_timer_list`：MCP `periph_timer {"list":true}` 返回 `count`/`timers`（含已配置计数器），
  `list` 与配置参数同用以 `isError:true` 返回。
- `periph_gpio_direction_output_readback`：`periph gpio --direction/--output/--read` 设置并读取
  引脚方向/输出/输入电平（含非法方向错误路径）。
- `mcp_periph_gpio_status`：MCP `periph_gpio` 的 `direction`/`output`/`read` 往返一致，
  缺修改参数且未 `read` 以 `isError:true` 返回。
- `periph_gpio_events_query_and_clear`：`periph gpio --events` 查询固件 `sw` 到 GPIO 输出寄存器产生的
  输出变化事件（cycle/pin/value/pc，跨会话持久化），`--clear-events` 清空事件日志
  （`eventCount=0`、`clearedEvents=true`）；人读模式打印 `GPIO 事件数`；`--value` 未配合 `--pin`
  返回明确错误而非 panic。
- `mcp_periph_gpio_events`：MCP `periph_gpio {"events":true}` 返回事件数组、
  `{"clear_events":true}` 清空并持久化，`value` 缺 `pin` 以 `isError:true` 返回。
- `periph_uart_input_injects_and_firmware_reads`：`periph uart --bytes/--input` 注入输入字节，
  固件从 UART 数据寄存器读取（FIFO 消费、跨会话持久化），`state --json` 的
  `peripherals.uartInput` 显示未消费字节。
- `mcp_periph_uart_input`：MCP `periph_uart {"bytes":[...]}` 注入后 `run` 读到首字节。
- `periph_uart_peek_and_drain`：`periph uart --peek` 查看下一条待读字节而不消费（多次 peek
  返回同一字节、`inputLen` 不变），`--rx-drain` 清空接收 FIFO（`drained` 回报丢弃数），
  peek 后 `peek` 为 null；人读模式标注 `待读首字节: 0xNN`。
- `mcp_periph_uart_peek_drain`：MCP `periph_uart` 的 `peek`/`rx_drain` 参数跨调用持久化，
  peek 不消费、drain 清空后 peek 为 null。
- `device_svd_parse_and_filter`：SVD 文件解析、外设/寄存器过滤与错误路径。
- `run_steps_runs_exactly_n_instructions`：`--steps N` 精确运行 N 条指令并停住。
- `run_until_stops_at_address`：`run --until` 临时断点运行到指定地址且不持久化。
- `run_until_symbol_stops`：`run/continue --until-symbol NAME` 解析符号地址并停靠（临时断点不持久化；
  未知符号报错；`--until` 与 `--until-symbol` 互斥）。
- `continue_until_symbol_stops`：`continue --until-symbol` 从会话续跑停靠符号地址。
- `analyze_until_symbol_stops`：`trace/coverage/profile --until-symbol` 停靠符号地址
  （断点指令不计数为已执行，与 `run` 一致）。
- `mcp_run_until_symbol`：MCP `run` 的 `until_symbol` 停靠符号地址；未知符号以 `isError:true` 返回。
- `run_max_cycles_stops_at_cycle_bound`：`run --max-cycles N` 到达周期计数即停
  （`lastResult="cycles"`，PC/周期/寄存器持久化）；`continue --max-cycles` 从会话续跑并停靠下一周期边界。
- `mcp_run_max_cycles`：MCP `run`/`trace` 的 `max_cycles` 参数返回 `lastResult`/`result="cycles"`。
- `run_until_condition_stops`：`run/continue --until-condition "x1 == 1"` 每步后求值，
  条件成立即停（`lastResult="condition"`，PC/寄存器持久化）；永不成立的条件运行到停机
  （`lastResult="halted"`）；`--trace` 与条件结合打印追踪并在停止点报 `result=condition`。
- `mcp_run_until_condition`：MCP `continue {"until_condition":"x1 == 1"}` 停于条件成立。
- `run_trace_prints_each_instruction`：`run --trace` 打印每条执行指令的 PC/反汇编/来源。
- `run_trace_regs_prints_changed_registers`：`run/continue --trace-regs` 打印每条指令后
  发生变化的寄存器（如 `ra=0x00000001`、`sp=0x00000003`），并可结合 `--until-condition`。
- `mcp_run_trace_regs`：MCP `run {"trace":true,"trace_regs":true}` 返回 `trace` 数组，
  每条含 `pc/regs`（变化寄存器快照）；仅 `trace_regs` 也启用收集。
- `continue_trace_prints_each_instruction`：`continue --trace` 打印每条执行指令并停靠临时断点。
- `continue_source_annotates_stop`：`continue --source` 在停止点标注 file:line。
- `mem_dump_c_array`：`mem dump --format c` 生成 C 数组（含非法格式错误路径）。
- `mem_dump_json`：`mem dump --json` 输出地址/长度/格式/名称/字节数组（机器可读导出）。
- `mem_dump_hex_roundtrip`：`mem dump --format hex` 生成合法 Intel HEX，可被 `mem load` 回载（含 64 KiB 银行边界与 `--format dump`/非法格式路径）。
- `mem_region_dump`：`mem region --device --region` 按目录内存映射解析命名区域（FLASH/RAM/PERIPHERALS），
  偏移/长度校验、区域基址/字节数组 JSON 输出、写后读回一致；未知区域与越界偏移均报错。
- `mcp_mem_region`：MCP `mem_region` 返回区域基址/长度/字节，未知区域以 `isError:true` 返回。
- `mcp_mem_search_out_and_count`：MCP `mem_search` 支持 `out` 把 JSON 写入文件并返回
  `count`/`file`/`bytes`；缺省内联返回 `count`/`matches`。
- `mcp_mem_dump_out`：MCP `mem_dump` 支持 `out` 把渲染结果（C 数组）写入文件并返回
  `file`/`bytes`。
- `mem_region_addressing_read_write_fill_search`：`mem read/write/fill/search --device+--region+--offset`
  按设备命名区域寻址（RAM @ 0x20000000），写/填后读回一致、搜索命中；缺 region、未知区域、
  `--addr` 与区域并存均报错。
- `mcp_mem_region_addressing`：MCP `mem_read/write/fill/search` 的 `device`/`region`/`offset`
  参数按命名区域寻址，未知区域与缺 region 以 `isError:true` 返回。
- `mem_copy_roundtrip`：`mem copy --src --dst --len` 复制字节并读回（`--src/--dst` 与位置参数
  双形式），重叠范围按 memmove 语义（dst 位于 src 之后不破坏源），缺源地址报错。
- `mcp_mem_copy`：MCP `mem_copy` 复制后 `mem_read` 读回一致。
- `mem_write_read_roundtrip_persists`：内存写读跨进程往返并持久化。
- `mem_symbol_relative_addressing`：`mem read/write/fill/search --symbol` 按 ELF 数据符号
  起始地址寻址（写 0xDEADBEEF 到 `g_counter`@0x80000100 后读回一致、`fill` 后 `search` 命中
  偏移、未知符号与 `--symbol` 同 `--addr` 冲突均报错）。
- `mcp_mem_symbol`：MCP `mem_read`/`mem_write` 的 `symbol` 参数解析数据符号地址并往返一致，
  未知符号与 `symbol` 同 `addr` 冲突以 `isError:true` 返回。
- `mem_fill_hex_value`：`mem fill --value` 十六进制值填充并可读回。
- `mem_read_word_width`：`mem read --width` 按字节/半字/字/双字读取（含非法宽度错误路径）。
- `mem_read_json`：`mem read --json` 输出地址/宽度/字节数组。
- `mem_read_ascii_and_string`：`mem read --ascii` 按 ASCII 文本解读（非可打印显示为 `.`），
  `--string` 读取 NUL 结尾字符串（`--ascii`/`--string` 互斥、非 byte 宽度被拒绝）。
- `mcp_mem_read_ascii_string`：MCP `mem_read` 的 `ascii`/`string` 参数返回 `text`。
- `mem_write_string_roundtrip`：`mem write --string --null` 写入字符串 ASCII 字节并读回；
  `--null` 缺 `--string` 报错、非 byte 宽度被拒绝。
- `mcp_mem_write_string`：MCP `mem_write` 的 `string`/`null` 写入后 `mem_read --string` 读回。
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
- `state_reports_arm_it_and_pc_limit`：`state --json` 报告 ARM Thumb-2 IT 块
  （`armItState` 的 `mask/cond/index`，`it eq` 后 `mask=8`，IT 指令执行后归零）与
  `pcLimit`（ARM 为 0xFFFFFFFF，mcs51 等 16 位 PC 架构为 0xFFFF；非 ARM 的 `armItState` 为 null）。
- `mcp_state_reports_arm_it`：MCP `state` 同样返回 `armItState`/`pcLimit`。
- `state_mem_snapshot`：`state --mem ADDR [--mem-len N]` 在 JSON 输出附带
  `memory {address,len,bytes}`（默认 64 字节，越界截断），人读模式输出 hex dump；
  `--fields memory` 可单独选择该快照字段。
- `mcp_state_mem`：MCP `state` 的 `mem`/`mem_len` 参数返回内存快照。
- `version_command_and_flag`：`version` 子命令（含 `--json`）与 `--version` 均输出版本号。
- `mem_write_read_arm_roundtrip`：非 RISC-V（ARM）内存写跨会话持久化。
- `periph_config_relocates_bases`：外设 UART/GPIO 基址重定位并持久化。
- `periph_config_timer_base`：`periph config --timer-base` 重定位定时器基址并用于后续创建，
  再次重定位更新已有定时器；`state --json` 的 `peripherals.timerBase` 反映持久化值。
- `mcp_periph_config_timer_base`：MCP `periph_config {"timer_base":...}` 重定位定时器基址，
  `periph_timer` 随后创建在该基址，`periph_status` 读回 `timerBase`。
- `mem_write_with_arch_creates_session`：`mem write --arch` 直接创建指定架构会话。
- `disasm_arm_state_override`：`disasm --arm-state a32|thumb` 切换 ARM 解码并校验步进宽度。
- `disasm_count_zero_returns_empty`：`disasm --count 0 --json` 返回空指令数组，
  人读模式无输出且不报错。
- `mcp_disasm_count_zero_returns_empty`：MCP `disasm {"count":0}` 返回空 `instructions` 数组。
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
- `run_arm_thumb2_dp_shifted`：端到端分五次 `step --session` 执行
  `add.w r0,r1,r2,lsl #3`（0xEB0100C2，0x10+0x08<<3=0x50）、
  `sub.w r3,r4,r5,lsr #2`（0xEBA40395，0x100−0x80>>2=0xE0）、
  `and.w r6,r7,r8,ror #1`（0xEA070678，0xFF & (0x81 ror #1)=0x40）、
  `mov.w r9,r10,asr #4`（0xEA4F192A，0x80>>4=0x08）、
  `orr.w r0,r1,r2,lsl #1`（0xEA410042，0x10 | 0x08<<1=0x10），
  `state --json` 校验寄存器，锁定 Thumb-2 数据处理（寄存器移位）执行——移位量由
  `imm3:imm2`（bits 14-12 : 7-6）合成 5 位，MOV/MVN 采用通用 `rd=bits 11-8`/`rm=bits 3-0`
  布局（此前移位仅取 `imm2` 且 MOV/MVN 交换了目的/源寄存器）。
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
- `run_arm_thumb2_cond_branch_wide`：端到端分三次 `step --session` 执行
  `movs r0,#0`（0x2000，置 Z）后 `beq.w`（0xF000 0x801A，目标 0x0800004A，
  PC=当前+4+偏移）命中，跳过回退路径的 `movs r3,#3`（r3 保持 0）并停在目标
  `movs r2,#1`（r2=1）；另建会话用 `movs r1,#1`（清 Z）使 `beq.w` 不命中，
  回退到 0x08000016 执行 `movs r3,#3`（r3=3），锁定 Thumb-2 条件分支宽格式
  执行（条件失败为 no-op，PC 已推进）。
- `disasm_thumb2_cond_branch_wide`：`disasm --arm-state thumb` 将
  `beq.w 0x24`/`bne.w 0x38`/`bne.w 0xfffffb58`/`bne.w 0x9c50`/`bgt.w 0x18`
  反汇编为真实助记符，按 20 位 S:J1:imm6:J2:imm10:0 符号扩展偏移计算目标。
- `run_arm_thumb2_movw_movt_addw_subw`：端到端分四次 `step --session` 执行
  `movw r0,#0x200`（0xF2402000，r0=0x200）、`movt r1,#0x1234`（0xF2C12134，
  r1=0x12340000）、`addw r2,r3,#0x567`（0xF2035267，0+0x567）、
  `subw r4,r5,#0x89a`（0xF6A5049A，0−0x89A），`state --json` 校验寄存器，
  锁定 Thumb-2 MOVW/MOVT/ADDW/SUBW 宽格式执行（opcode bits7-4：MOVW=0100、
  MOVT=1100、ADDW=0000、SUBW=1010；立即按 imm4:i:imm3:imm8 组合）。
- `disasm_thumb2_movw_movt_addw_subw`：`disasm --arm-state thumb` 将
  `movw r0,#0x200`/`movt r1,#0x1234`/`movw r2,#0xabcd`/`addw r2,r3,#0x567`/
  `subw r8,r9,#0xfff` 反汇编为真实助记符。
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
- `run_arm_thumb2_ldrsb_ldrsh`：端到端分四次 `step --session` 执行
  `ldrsb.w r6,[r7,r8,lsl #1]`（0xF9176018，r7=0x20000000/r8=1，地址
  0x20000002 读 0x80 → r6=0xFFFFFF80）、`ldrsh.w r9,[r10,r11,lsl #2]`
  （0xF93A902B，地址 0x20000014 读 0x8000 → r9=0xFFFF8000）、
  `ldrsb.w r0,[r1,#4]`（0xF9910004，地址 0x20000024 读 0x7F → r0=0x7F）、
  `ldrsh.w r2,[r3,#8]`（0xF9B32008，地址 0x20000038 读 0x1234 → r2=0x1234），
  `state --json` 校验，锁定 Thumb-2 有符号字节/半字加载（寄存器偏移、立即
  偏移与 PC 相对字面量）。
- `disasm_thumb2_ldrsb_ldrsh`：`disasm --arm-state thumb` 将
  `ldrsb`/`ldrsh` 的寄存器偏移、立即偏移与 `[pc, #imm]` 字面量形式反汇编为
  真实助记符。
- `run_arm_thumb2_hints`：端到端分五次 `step --session` 执行
  `nop.w`（0xF3AF8000）/`yield.w`（0xF3AF8001）/`wfe.w`（0xF3AF8002）/
  `wfi.w`（0xF3AF8003）/`sev.w`（0xF3AF8004），校验均为无操作（不 halt、
  PC 按 4 字节推进到 0x08000024），锁定 Thumb-2 提示指令执行。
- `disasm_thumb2_hints`：`disasm --arm-state thumb` 将
  `nop.w`/`yield.w`/`wfe.w`/`wfi.w`/`sev.w` 与 Thumb-1 `yield`/`wfe`/`wfi`/`sev`
  反汇编为真实助记符。
- `run_arm_thumb2_ldr_str_imm8`：端到端分五次 `step --session` 执行
  `str r1,[r0,#-4]`（0xF8401C04，写 0x2000003C=0xDEADBEEF）、
  `ldr r11,[sp],#4`（0xF85DBB04，读 0x20000000=0x11223344、sp=0x20000004）、
  `strb r3,[r4,#-1]`（0xF8043C01，写 0x2000000F=0xAB）、
  `ldrh r5,[r6,#-2]`（0xF8365C02，读 0x2000001E=0xCDEF）、
  `str r11,[sp,#-4]!`（0xF84DBD04，写 0x20000000、sp 回 0x20000000），
  `state --json` 与 `mem read` 校验寄存器与存储跨会话持久化，锁定
  Thumb-2 8 位立即 LDR/STR（含前/后索引写回）。
- `run_arm_thumb2_mla_mls`：端到端分三次 `step --session` 执行
  `mul r1,r2,r3`（0xFB02F103，3×5=15）、`mla r4,r5,r6,r7`（0xFB057406，
  7×11+13=90）、`mls r8,r9,r10,r11`（0xFB09B81A，23−17×19=0xFFFFFED4），
  `state --json` 校验，锁定 Thumb-2 MUL/MLA/MLS 执行。
- `run_arm_thumb2_umull_smull`：端到端分三次 `step --session` 执行
  `umull r0,r1,r2,r3`（0xFBA20103，0xFFFFFFFF²=0xFFFFFFFE00000001）、
  `smull r4,r5,r6,r7`（0xFBC64507，−7×2=−14=0xFFFFFFFFFFFFFFF2）、
  `umlal r8,r9,r10,r11`（0xFBAA891B，0+3×5=15），
  `state --json` 校验，锁定 Thumb-2 UMULL/SMULL/UMLAL/SMLAL 64 位乘加执行。
- `run_arm_thumb2_sat_q_sel_extend`：端到端分九次 `step --session` 执行
  `qadd r0,r1,r2`（0xFA8281F0，0x40000000+0x40000000 饱和 → 0x7FFFFFFF）、
  `qdadd r3,r4,r5`（0xFA8594F3，r5+2·r4=0x4000）、
  `qsub r6,r7,r8`（0xFA88A7F6，r8−r7=0x1000）、
  `qdsub r9,r10,r11`（0xFA8BBAF9，r11−2·r10=0）、
  `sel r0,r1,r2`（0xFAA182F0，GE=0 → Rm=0x22222222）、
  `sxtah r3,r4,r5`（0xFA0485F3，0x10+0xFFFF8000=0xFFFF8010）、
  `uxtah r6,r7,r8`（0xFA1788F6，0x20+0x8000=0x8020）、
  `sxtab r9,r10,r11`（0xFA4A8BF9，0x30+0xFFFFFF80=0xFFFFFFB0）、
  `uxtab r12,r0,r1`（0xFA5081FC，r0+0x11=0x22222233），`state --json` 校验，
  锁定 Thumb-2 饱和加减/选择/扩展加执行（QADD 家族操作数序 Rd,Rm,Rn；
  扩展加的操作码位于首半字 bits 7-4，Rn≠PC）。
- `disasm_thumb2_sat_q_sel_extend`：`disasm --arm-state thumb` 将
  `qadd`/`qdadd`/`qsub`/`qdsub`/`sel`/`sxtah`/`uxtah`/`sxtab`/`uxtab`
  反汇编为真实助记符（共享 0xFAxx 空间，须按 bits 7-4 与 Rn≠PC 区分扩展加）。
- `run_arm_thumb2_ssat_usat`：端到端分四次 `step --session` 执行
  `ssat r0,#8,r1`（0xF3010007，128→127=0x7F）、
  `ssat r2,#16,r3,lsl #4`（0xF3030F12，0x2000<<4=0x20000→0x7FFF）、
  `usat r4,#8,r5`（0xF3850804，−1→0）、
  `usat r6,#16,r7,asr #4`（0xF3A71016，0x10000 asr 4→0x1000），
  `state --json` 校验，锁定 Thumb-2 SSAT/USAT 饱和立即执行（第二半字 bit15=0，
  与 B&lt;cond&gt;.W 区分）。
- `disasm_thumb2_ssat_usat`：`disasm --arm-state thumb` 将
  `ssat`/`usat`（含 `lsl #n`/`asr #n` 移位形式）反汇编为真实助记符。
- `run_arm_thumb2_mul_wide`：端到端分八次 `step --session` 执行
  `smulbb r0,r1,r2`（0xFB1102F0，−1×5=−5）、
  `smulbt r3,r4,r5`（0xFB1415F3，6×−1=−6）、
  `smlabb r6,r7,r8,r9`（0xFB170896，7×8+9=0x41）、
  `smmul r10,r11,r12`（0xFB5B0CFA，0x10000000×0x10&gt;&gt;32=1）、
  `smmla r0,r1,r2,r3`（0xFB510230，0+4=4）、
  `smmls r4,r5,r6,r7`（0xFB650674，7−0=7）、
  `smlald r8,r9,r10,r11`（0xFBCACB89，0+10×11=110）、
  `smlsld r12,r0,r1,r2`（0xFBD1C2C0，0−12×13=−156=0xFFFFFF64:0xFFFFFFFF），
  `state --json` 校验，锁定 Thumb-2 有符号 16×16 与最高有效字乘加、
  SMLALD/SMLSLD 64 位乘减执行。
- `disasm_thumb2_mul_wide`：`disasm --arm-state thumb` 将
  `smulbb`/`smulbt`/`smlabb`/`smmul`/`smmla`/`smmls`/`smlald`/`smlsld`
  反汇编为真实助记符（UMULL/SMULL 家族按第二半字 bits 7-5=000 与
  SMLALD/SMLSLD bits 7-5=110 区分）。
- `run_arm_thumb2_mul_word`：端到端分十一次 `step --session` 执行
  `smulwb r0,r1,r2`（0xFB3102F0，0x10000×3&gt;&gt;16=3）、
  `smulwt r3,r4,r5`（0xFB3415F3，0x20000×1&gt;&gt;16=2）、
  `smlawb r6,r7,r8,r9`（0xFB370896，0xC+0xA=0x16）、
  `smlawt r10,r11,r12,r0`（0xFB3B1C0A，2+r0=5）、
  `smlalbb r1,r2,r3,r4`（0xFBC38412，5×6=30）、
  `smlalbt r5,r6,r7,r8`（0xFBC79856，7×8=56）、
  `smlaltb r9,r10,r11,r12`（0xFBCBAC9A，9×10=90）、
  `smlaltt r0,r1,r2,r3`（0xFBC2B301，11×12=132）、
  `umaal r4,r5,r6,r7`（0xFBE66745，3×4+1+2=15）、
  `usad8 r8,r9,r10`（0xFB790AF8，Σ|Δbyte|=16）、
  `usada8 r11,r12,r0,r1`（0xFB7C001B，16+5=21），`state --json` 校验，
  锁定 Thumb-2 字×半字乘加（SMULWB/SMULWT/SMLAWB/SMLAWT）、长半字乘加
  （SMLALBB/BT/TB/TT）、UMAAL 与 USAD8/USADA8 执行。
- `disasm_thumb2_mul_word`：`disasm --arm-state thumb` 将
  `smulwb`/`smulwt`/`smlawb`/`smlawt`/`smlalbb`/`smlalbt`/`smlaltb`/`smlaltt`/
  `umaal`/`usad8`/`usada8` 反汇编为真实助记符（SMLALxy 按第二半字 bits 7-6=10
  与 SMLALD bits 7-6=11 区分）。
- `disasm_source_annotates_lines`：`disasm --source` 标注每行 file:line。
- `disasm_raw_shows_bytes`：`disasm --raw` 显示每条指令的原始字节。
- `disasm_json`：`disasm --json` 输出地址/反汇编/字节/来源 JSON 数组。
- `disasm_from_firmware`：`disasm --firmware` 直接加载固件反汇编（`--addr`/`--symbol`
  起始，无地址时从固件入口开始；架构从 ELF 推导；未知符号报错）。
- `mcp_disasm_firmware`：MCP `disasm {"firmware":...}` 支持 `addr`/`symbol` 起始。
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
- `mem_search_width_pattern`：`mem search --width byte|half|word|dword` 将 `--pattern` 解释为
  小端整数值并逐宽展开（word 常量 `0xDEADBEEF`、half `0xBEEF`、dword 64 位常量 `0x0123456789ABCDEF`
  均可按地址定位）；非法宽度与超宽数值报错误。
- `mcp_mem_search_width`：MCP `mem_search` 的 `width` 参数返回 `width`/`matches`；缺省按字节解释，
  数值超出所请求宽度时报错。
- `mem_search_mask`：`mem search --mask` 按 `--width` 展开逐字节掩码（`0x00` 为通配字节，
  `0xFF` 精确匹配），可匹配低位通配的字（`0xDEADBEEF`/`0xDEADBEFF` 均命中）；掩码长度与展开后
  模式不一致时报错。
- `mcp_mem_search_mask`：MCP `mem_search` 的 `mask` 参数按相同宽度展开并返回 `mask`/`matches`，
  长度不一致以 `isError:true` 返回。
- `mem_search_out_writes_file`：`mem search --out` 将 JSON/文本结果写入文件。
- `state_reports_loaded_firmware`：`state --json` 报告已加载固件（path/base/format）。
- `state_reports_pc_symbol`：`state --json` 报告 PC 处最近符号（pcSymbol）。
- `symbol_list_shows_symbols`：`symbol list --json` 列出 ELF 符号（名称/地址）。
- `symbol_nearest`：`symbol nearest` 查询地址不大于给定地址的最近符号（含 `offset`；
  位置参数/`--addr` 双形式；无前置符号为 null；缺地址报错）。
- `mcp_symbol_nearest`：MCP `symbol_nearest` 返回最近符号与偏移，无符号为 null。
- `device_svd_by_device_key`：`device svd --device` 从内置 SVD 解析；缺源报错。
- `disasm_rx_fc_fd_long_forms`：RX `0xFC`/`0xFD` 长操作码族反汇编文本与实际宽度推进。
- `step_rx_fc_fd_long_forms`：RX `0xFC`/`0xFD` 长操作码族执行（ADC/SHLR/SBB）与 PC/寄存器变化。
- `disasm_rx_variable_width`：RX 变长指令（1/2/4/6 字节）按实际解码宽度推进并解码
  短分支/立即族（`nop`=2、`bra.s`=1、`beq.s`=2、`add #imm16`=4、`bra.a`=6），
  与内核大端取指一致。
- `disasm_rx_decodes_families`：RX 寄存器/dsp/立即族反汇编镜像执行内核并读取大端操作数
  （`add r0,r0`、`sub +16[r2],r0`、`movu.b +0x3412[r3],r0`、`mov.l +16[r0],+32[r12]`）。
- `rx_step_over_uses_call_width`：RX `BSR.w` 为 4 字节，`step-over` 在返回地址 pc+4 恢复
  （且 `BSR.w` 压栈返回地址），`rts` 被 `step-out` 识别。
- `disasm_rx_bit_and_register_ops`：RX `0x78-0x7D`（BSET/BCLR/BTST #imm5, rd）与
  `0x7E`/`0x7F`（NOT/NEG/ABS/RORC/ROLC/PUSH/POP/PUSHC/POPC 与 JMP/JSR/BRA.l/BSR.l）
  按 2 字节解码为真实助记符（此前 0x7A-0x7D 落到占位符、0x7E/0x7F 误配到 NOT/JMP 族）。
- `disasm_rl78_variable_width`：RL78 变长指令（1/2/3/4 字节）按实际解码宽度推进，
  含 `0x61` 前缀携带 16 位/8 位操作数形式（`nop`=1、`add A,#imm8`=2、`add saddr,#imm8`=3、
  `addw AX,#imm16`=3、`add A,ES:[imm16]`=3、`0x61 mov A,ES:[imm16]`=4、`0x61 and A,rba`=2）。
- `byte_opcode_disasm_matches_core`/`word_opcode_disasm_matches_core`：逐操作码枚举校验
  “反汇编占位符 ⟺ 核心停机”，确保执行内核支持的操作码族都有真实反汇编（MCS-51/80251/
  C166/STM8/RL78/AVR/MSP430/RX）。
- `mcp_handshake_and_list_tools`：MCP stdio 握手（initialize）+ `tools/list` 返回 ≥60 个工具，
  每个工具含 `name`/`description`/`inputSchema`，覆盖架构/设备（含 SVD/代码生成）/会话/状态/步进/寄存器/内存/断点/反汇编/周期/中断/外设/符号/看门狗/CSR。
- `mcp_call_step_and_state`：MCP `session_create` → `mem_write` → `step`（PC+4、周期+1）→
  `state`（含 `fields` 字段子集）/`reg_set`/`reg_get`/`bp_set`/`bp_list`/`disasm`/`periph_config`
  完整往返，工具错误以 `isError:true` 返回。
- `mcp_device_codegen_and_svd`：MCP `device_svd`（内置设备/按外设寄存器/缺源错误）、
  `device_linker`/`device_startup`/`device_init` 代码生成（含未知设备错误路径）。
- `mcp_all_arch_step`：12 架构通过 MCP `session_create`→`step`/`reg_list`/`mem_read`/`disasm`
  均成功，验证 MCP 工具在全部架构上可用。
- `analyze_trace_coverage_profile`：`trace` 输出结构化指令轨迹（`pc`/`text`/`bytes`/`source`/
  `cycle`，RISC-V `addi zero, zero, 0` 精确 2 条并推进 0x80000004）；`coverage` 统计唯一 PC 与
  各 PC 命中次数；`profile` 统计助记符直方图（同一 NOP 计 2 次）；`--top 1` 截断 trace、
  按次数保留 coverage/profile 前 1 条（`uniquePcs`/`uniqueMnemonics` 仍为全量）；人读轨迹含 PC 列。
- `mcp_trace_coverage_profile`：MCP `trace`/`coverage`/`profile` 工具返回同等结构化结果，
  `top` 参数同样截断结果，且 `until` 越界（16 位 PC 架构 mcs51 的 `0x10000`）以 `isError:true` 返回。
- `session_info_and_diff`：`session info`（人读 + `--json`：arch/pc/cycleCount/breakpoints/
  memoryOverlayBytes/peripherals.uartBase）与 `session diff --a/--b`（`same`、`summary` 各类计数、
  含 scalar/register/breakpoint/memory 四类差异、`--limit 1` 截断计数、缺失会话报错）。
- `session_merge_command`：`session merge` 以 A 为基准、B 的运行时状态（PC/周期）优先，
  寄存器并集 B 胜出、断点/内存覆盖并集（2 断点、8 字节）、`--regs-keep` 保留 A 的寄存器文件、
  架构不一致与缺失会话报错。
- `mcp_session_merge`：MCP `session_merge` 的 `a`/`b`/`out` 合并后 `reg_set` 读回一致，
  `regs_keep` 保留 A 寄存器、缺参/架构不一致以 `isError:true` 返回。
- `session_patch_fields`：`session patch --pc/--cycle/--halted/--reg/--csr` 一次修改标量、寄存器
  与 CSR 并保存，随后 `reg get`/`csr get` 读回一致；未知寄存器报错。
- `mcp_session_patch`：MCP `session_patch` 的 `pc`/`cycle_count`/`halted`/`registers`/`csr`
  参数修改后 `reg_get`/`csr_get` 读回一致，未知寄存器以 `isError:true` 返回。
- `session_patch_arm_state`：`session patch --arm-state/--xpsr/--it` 在 ARM 会话设置指令集
  状态/xPSR/If-Then 块并持久化（`state --json` 读回 `armState`/`armXpsr`/`armItState`），
  `--arm-state a32` 清除 Thumb 位；`--it` 格式错误与非 ARM 会话均报错。
- `mcp_session_patch_arm_state`：MCP `session_patch` 的 `arm_state`/`xpsr`/`it_mask`/`it_cond`/
  `it_index` 设置后 `state` 读回一致；非 ARM 会话的 `arm_state`/`it_mask` 以 `isError:true` 返回。
- `device_memory_map_and_address_lookup`：`device memory-map` 列出 FLASH/RAM/PERIPHERALS 区域
  （`access`/`start`/`end`/`size`），`--addr` 解析所属区域；越界地址与未知设备报错。
- `device_lookup_resolves_register`：`device lookup` 按地址反查区域/外设/寄存器
  （GPIOA.CRL@0x40010800 含 `registerSize`/`registerOffset`/`fields`，CRH 为独立寄存器不被前一个
  的 SVD 位宽吞并；`0x08000000` 仅命中 FLASH 区域；未知设备报错）。
- `device_lookup_by_name`：`device lookup --name PERIPH.REG` 按名解析寄存器（含位域字段），
  `--name PERIPH` 解析到外设（`register`/`fields` 为 null）；未知外设/寄存器、缺 `--addr`/`--name`
  均报错。
- `device_peripherals_list`：`device peripherals` 列出外设概览（名称/基址/寄存器数/描述），
  `--filter` 名称子串筛选（大小写不敏感），未知设备报错。
- `mcp_device_peripherals`：MCP `device_peripherals` 列出/筛选，未知设备以 `isError:true` 返回。
- `device_peripheral_command`：`device peripheral` 显示外设寄存器明细（GPIOA 的 CRL
  @0x40010800、offset 0、size 32、复位 0x44444444、位域 MODE0），`--register` 只选单个寄存器，
  未知设备/外设/寄存器均报错。
- `mcp_device_peripheral`：MCP `device_peripheral` 返回寄存器明细与位域，`register` 只选一个，
  缺参/未知外设以 `isError:true` 返回。
- `disasm_and_trace_write_files`：`disasm --out` 写出汇编清单、`trace --json --out` 写出结构化
  轨迹、`profile --out` 写出文本直方图，文件内容均可被重新解析校验。
- `firmware_info_command`：`firmware info` 对 ELF 自动检测架构与入口（`entryText` 反汇编首条指令、
  `machine=243`），BIN 可 `--arch` 显式指定，缺失文件报错。
- `firmware_verify_command`：`firmware verify` 对原始文件字节计算全部校验和（8 字节
  `00..07` 的 CRC32=`0x88aa689f`、Adler32=`0x005c001d`、CRC16-CCITT=`0x178d`、FNV-1a 等），
  `--algo` 只选单个算法、`--expected` 匹配退出 0 且 `verified=true`、不匹配非零退出并报
  `校验失败`、未知算法/缺失文件报错。
- `mcp_firmware_verify`：MCP `firmware_verify` 返回全部校验和并支持 `algo`+`expected` 比对
  （`verified` 真假），缺失 `firmware`/未知算法以 `isError:true` 返回。
- `firmware_symbols_command`：`firmware symbols` 列出 ELF 符号（`reset_handler`@0x80000000、
  size=4）与源文件，`--filter` 名称子串筛选（大小写不敏感）、BIN 无符号、缺失文件报错。
- `mcp_firmware_symbols`：MCP `firmware_symbols` 返回符号表并支持 `filter`，缺失固件以
  `isError:true` 返回。
- `trace_regs_snapshot`：`trace --regs` 每条指令附带执行后寄存器快照（`addi ra,zero,1`→ra=1、
  `addi sp,ra,2`→sp=3），不带 `--regs` 时 `regs` 为空（向后兼容）。
- `mcp_firmware_info_and_trace_regs`：MCP `firmware_info` 返回同等元信息（含缺失文件错误路径），
  `trace` 的 `regs: true` 参数返回每条指令后的寄存器快照。
- `periph_reg_dump_command`：`periph reg-dump` 转储设备全部命名寄存器的当前值（SVD 复位值
  0x44444444 读回 GPIOA.CRL、`--peripheral` 过滤、未知设备报错）。
- `periph_reg_dump_decodes_fields`：`periph reg-dump --fields` 解码每个寄存器的 SVD 位域
  （`fields[{name,bitOffset,bitWidth,value,description}]`，`field-set RCC.CR.HSION=1` 后解码值随之变化；
  不带 `--fields` 时省略该数组；人读模式打印 `bit 起始:结束` 与当前值）。
- `mcp_periph_reg_dump_tool`：MCP `periph_reg_dump` 返回 `count`/`registers`（含 `GPIOA.CRL`），
  `peripheral` 过滤与未知设备错误路径；默认附带 `fields` 位域解码（`GPIOA.CRL.MODE0`=0x4），
  `fields: false` 返回紧凑转储。
- `disasm_labels_annotate_symbols`：`disasm --labels` 在符号起始地址插入 `名称:` 标签行，
  JSON 项携带 `label`（仅符号首条指令非空）、`--out` 落盘含标签。
- `mcp_disasm_labels`：MCP `disasm` 的 `labels: true` 在 `instructions[i].label` 标注符号地址，
  缺省为 null。
- `mem_fill_random_deterministic`：`mem fill --random --seed` 生成确定性伪随机字节（同 seed 复现、
  `seed` 字段回报输入种子、`--random` 与 `--value` 互斥）。
- `mcp_mem_fill_random`：MCP `mem_fill` 的 `random`/`seed` 参数确定性复现与 `value` 冲突错误路径。
- `mem_fill_pattern`：`mem fill --pattern` 重复填充字节模式并可读回（与 `--value` 互斥）。
- `mcp_mem_fill_pattern`：MCP `mem_fill` 的 `pattern` 参数重复填充后 `mem_read` 读回一致。
- `disasm_from_symbol`：`disasm --symbol main` 从符号地址开始反汇编（JSON 首项地址=符号地址、
  未知符号报错）。
- `mcp_disasm_from_symbol`：MCP `disasm` 的 `symbol` 参数从符号地址开始（含未知符号错误路径）。
- `mcp_session_and_memory_map_tools`：MCP `session_info` / `session_diff` / `device_memory_map` /
  `device_lookup` 工具端到端（`device_lookup` 解析 GPIOA.CRL 寄存器与位域，含缺参/未知设备
  错误路径）与 `trace` 的 `out` 参数把轨迹写入文件并回报 `file`/`bytes`。
- `help_every_command`：每个命令与子命令（含 `mcp`、`session info/diff/patch/merge`、`device memory-map`、
  `device lookup`、`device peripherals`、`device peripheral`、`firmware info/verify/symbols`、
  `watch set/list/modify/clear/clear-all/enable/disable`）的
  `--help` 与 `-h` 均退出 0 且含 `Usage:`。
- `help_every_param`：`run`/`bp set`/`mem read`/`reg set`/`periph reg-write`/`watchdog config`/
  `watch set`/`watch list`/`watch modify`/`trace`/`coverage`/`profile`/`mcp` 的 `--help` 包含每个参数的帮助文本
  （`--session`、`--firmware`、`--arch`、`--addr`、`--condition`、`--once`、`--steps`、`--until`、
  `--len`、`--hit-count` 等）。

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
  内存屏障、8 位立即（含前/后索引写回）、数据处理（寄存器/修正立即）、
  MUL/MLA/MLS/UMULL/SMULL/UMLAL/SMLAL、位域 SBFX/UBFX/BFC/BFI 与 TBB/TBH/LDRD/STRD；`b`（0xE001）与
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
  second & 0x00F8 == 0x0080，即第二半字 `0xF0Rd 1000 0Rm`，Rd=second bits11-8、
  Rm=first bits3-0）现返回前导零计数（0 输入返回 32），低位 Rm 位随源寄存器变化
  （原误判为固定 0x87，导致 `clz r2,r2` 等真实编码未被识别）。由 ns-sim 单测
  `thumb1_cps_primask`/`thumb1_rev_family`/`thumb2_clz` 与 CLI
  `run_arm_thumb1_rev_and_clz` 锁定。
  Thumb-2 `MOVW`/`MOVT`/`ADDW`/`SUBW`（`1111 0 i 10 0op ...`，opcode bits7-4：
  MOVW=0100、MOVT=1100、ADDW=0000、SUBW=1010；第二半字 `0 imm3 Rd imm8`，bit15=0；
  立即数 `imm16=(imm4<<12)|(i<<11)|(imm3<<8)|imm8`，`imm12=(i<<11)|(imm3<<8)|imm8`，
  i=first bit10、imm4=first bits3-0、imm3=second bits14-12、imm8=second bits7-0）现执行
  与反汇编：MOVW 写低半字、MOVT 写高半字（top 由 first bit7 选，非 second bit11），
  ADDW/SUBW 加/减 12 位立即。此前门控误用 `second & 0xF000`（MOVW 需 0x4000、
  ADDW 需 0x2000），且 imm16/imm12 从 first bits11-8 取位，导致真实编码
  `0xF240 0x2000`（movw r0,#0x200）、`0xF2C1 0x2134`（movt r1,#0x1234）等被误判或
  显示为 `.word`。由 ns-sim 单测 `thumb2_movw_movt_addw_subw_wide` 与 CLI
  `run_arm_thumb2_movw_movt_addw_subw`/`disasm_thumb2_movw_movt_addw_subw` 锁定。
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
  Thumb-2 8 位立即 LDR/STR（`0xF800-0xF87F`，first 为 `1111 1000 0 size L Rn`、second
  为 `Rt 1 P U W imm8`，P=bit10/U=bit9/W=bit8）现执行字节/半字/字存取，支持前索引
  （含写回 `!`）与后索引（P=0 恒写回）寻址，负偏移按 U=0 取补；该类编码与寄存器偏移
  形式共享 `0xF800-0xF85F` 区间，由 second bit11 区分（寄存器偏移为 0，立即为 1）。
  由 ns-sim 单测 `thumb2_ldr_str_imm8_writeback` 与 CLI
  `run_arm_thumb2_ldr_str_imm8` 锁定。
  Thumb-2 乘加/乘长：`MLA/MLS`（`0xFB00` 前缀、second bits7-5=0、Ra≠15，bit4 选
  MLA/MLS；Rd=second bits11-8、Rm=second bits3-0、Ra=second bits15-12、Rn=first bits3-0）
  现执行 `Rd=Rn*Rm+Ra`/`Rd=Ra-Rn*Rm`（低 32 位回绕）；`UMULL/SMULL/UMLAL/SMLAL`
  （`0xFBA0`/`0xFBC0` 前缀，bit6 选有符号，second bit4 选累加；RdLo=second bits15-12、
  RdHi=second bits11-8、Rm=second bits3-0、Rn=first bits3-0）现执行 64 位乘与 64 位累加
  （UMLAL/SMLAL 将既有 `RdHi:RdLo` 作为加数）。由 ns-sim 单测
  `thumb2_mla_mls`/`thumb2_umull_smull` 与 CLI `run_arm_thumb2_mla_mls`/
  `run_arm_thumb2_umull_smull` 锁定。
  Thumb-2 数据处理（寄存器，bits15-9 = `1110101`）现按 `op = bit8<<3 | bits7-5` 分发执行
  AND/BIC/ORR/ORN/EOR/ADD/ADC/SBC/SUB/RSB，S=bit4 更新 N/Z/C/V（逻辑运算 C 取自移位器、
  V 不变；加减运算按 ARM 进位/溢出语义）；`Rn=PC` 的 ORR/ORN 即 MOV/MVN，与一般布局
  一致（`Rd = second bits11-8`、`Rm = second bits3-0`，不再交换）；`S=1 且 Rd=PC` 的
  AND/EOR/ADD/SUB 即 TST/TEQ/CMN/CMP（只置标志不写回）；移位器支持 LSL/LSR/ASR/ROR，
  移位量由 `imm3:imm2`（bits14-12 : bits7-6）合成完整 5 位（imm5=0 时 LSR/ASR 取 #32、
  ROR 即 RRX），不再只取 `imm2`。由 ns-sim 单测
  `thumb2_data_processing_register`/`thumb2_mov_mvn_compare`/
  `thumb2_data_processing_shifted_register` 与 CLI `run_arm_thumb2_data_processing_register`/
  `run_arm_thumb2_dp_shifted` 锁定。
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
  Thumb-2 条件分支宽格式 `B<cond>.W`（首半字 `1111 0 S cond imm6`，bit10=S、bits9-6=cond、
  bits5-0=imm6；第二半字 `10 J1 0 J2 imm10`，bit13=J1、bit11=J2、bits10-0=imm10）现执行
  与反汇编：20 位偏移 `S:J1:imm6:J2:imm10:0` 按符号扩展，目标 = 架构 PC（当前地址 +4）+
  偏移；条件由当前标志评估，失败即 no-op（PC 已推进）。该编码与 0xF3xx 专用指令
  （DMB/DSB/ISB、提示、MRS/MSR、位域）共用 `11110` 首半字高 5 位，故在那些专用处理器
  之后检查；位域指令（第二半字 bit15=0）与 `B<cond>.W`（第二半字 bit15=1）由 bit15 区分，
  而前向 `B<cond>.W`（cond 0-7、S=0）须先于修正立即数据处理处理器。由 ns-sim 单测
  `thumb2_cond_branch_wide` 与 CLI `run_arm_thumb2_cond_branch_wide`/
  `disasm_thumb2_cond_branch_wide` 锁定。
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
  Thumb-2 有符号 `LDRSB`/`LDRSH` 宽形式现执行与反汇编：寄存器偏移（首半字
  `0xF910|Rn`/`0xF930|Rn`，bit7=0；第二半字 `Rt 0000 00shift Rm`）、立即偏移与
  PC 相对字面量（首半字 `0xF990|Rn`/`0xF9B0|Rn`，bit7=1，Rn=PC 即字面量；第二半字
  `Rt imm12`，字面量基址为 `Align(PC,4)`）。`LDRSB` 有符号扩展字节、`LDRSH`
  有符号扩展半字。由 ns-sim 单测 `thumb2_ldrsb_ldrsh` 与 CLI
  `run_arm_thumb2_ldrsb_ldrsh`/`disasm_thumb2_ldrsb_ldrsh` 锁定。
  Thumb-2 提示指令 `NOP.W`/`YIELD.W`/`WFE.W`/`WFI.W`/`SEV.W`（`0xF3AF 0x8000-8004`）
  与 Thumb-1 `YIELD`/`WFE`/`WFI`/`SEV`（`0xBF10`/`0xBF20`/`0xBF30`/`0xBF40`，低半字节
  为 0 时按 bits7-4 区分提示与 NOP）现执行（均为无操作）并反汇编为真实助记符。由
  ns-sim 单测 `thumb2_hints` 与 CLI `run_arm_thumb2_hints`/`disasm_thumb2_hints` 锁定。
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
