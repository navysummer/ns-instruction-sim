# skill — ns-instruction-sim 开发操作指南

> 面向编码代理（Codex）的项目级技能文档。配合根目录 `AGENTS.md` 使用：`AGENTS.md` 定义**规范与
> 范围**，本文件定义**在此仓库工作时必须遵循的操作步骤与约定**。

## 1. 技能目标

在 `ns-instruction-sim` 仓库中安全、一致地完成以下任务：

- 构建/测试/运行仿真 CLI，保证零警告零错误、测试全绿。
- 新增或扩展 CLI 子命令，使外部调用方通过命令行完整驱动指令级仿真。
- 修改 `crates/ns-sim`（12 架构内核/外设）或 `crates/ns-device`（SVD 设备目录）时保持内部 API 稳定。
- 编写/维护端到端测试（`tests/cli.rs`）与文档同步（`AGENTS.md`/`README.md`/`skill.md`）。

## 2. 工作区前置检查

开始任何修改前，先确认当前状态：

```bash
cd /Users/navysummer/Desktop/other/ns-instruction-sim
git status
cargo build
cargo test
```

若 `cargo build` 因网络失败（依赖下载受限），使用 `--offline`：

```bash
cargo build --offline
cargo test --offline
```

## 3. 代码组织约定

### 3.1 目录职责

- `src/main.rs`：`clap` 命令定义 + 子命令实现。新增命令在此添加 `Command` 变体、`Args` 结构与
  `cmd_*` 函数，并在 `real_main()` 分发。
- `src/session.rs`：`Session`（JSON 会话）、`arch_name`/`parse_arch`/`register_index_by_name`。
- `src/sim.rs`：`load_firmware_into`（ELF/HEX/BIN 检测与加载）、`default_mem_base`、`parse_num`、
  `read_mem_byte`/`write_mem_byte`。
- `crates/ns-sim/src/`：仿真核心。**只读参考**，除非任务明确要求修改内核；修改须保持既有 `pub fn`
  签名与行为。
- `crates/ns-device/src/`：设备目录与 SVD。同上。
- `tests/cli.rs`：以外部进程方式驱动 `CARGO_BIN_EXE_ns-instruction-sim` 的端到端测试。

### 3.2 命名与参数

- 子命令：kebab-case（`step-over`、`clear-all`、`reg set`、`mem read`）。
- 参数：`--flag` kebab-case；可提供位置参数时用 `#[arg(index = N)]` 与 `#[arg(long)]` 双形式，
  用 `conflicts_with` 防冲突。
- 布尔开关：`--json`、`--cycle-accurate`、`--save`、`--clear` 等。

### 3.3 错误与输出

- 错误统一返回 `anyhow::Result`，由 `main` 打印 `错误: <msg>` 到 stderr 并 `exit(1)`；命令内部
  禁止 `panic!`/`unwrap()` 处理用户输入。
- 列表/状态命令必须支持 `--json`：用 `serde_json::to_string_pretty` 输出稳定字段。
- 人类可读输出用 `println!` 表格；机器输出统一走 `print_json`。

## 4. 命令速查（外部调用）

```bash
# 版本
ns-instruction-sim version

# 列出架构 / 设备
ns-instruction-sim arch list
ns-instruction-sim arch list --json
ns-instruction-sim device info stm32f103c8 --json
ns-instruction-sim device list
ns-instruction-sim device info stm32f103c8
ns-instruction-sim device svd --file chip.svd --json
ns-instruction-sim device svd --file chip.svd --peripheral USART1 --register DR --json
ns-instruction-sim device svd --device stm32f103c8 --json
ns-instruction-sim device linker stm32f103c8 --out chip.ld
ns-instruction-sim device startup stm32f103c8 --out startup.s
ns-instruction-sim device init stm32f103c8 --peripheral GPIOA --register GPIOA.CRL=0x44444444 --out periph_init.c

# 加载并运行固件（ELF 自动识别架构）
ns-instruction-sim run --firmware app.elf --save-session s.json --json
ns-instruction-sim run --firmware app.bin --arch riscv --base 0x80000000 --max-steps 1000
ns-instruction-sim run --firmware app.elf --steps 1000 --json
ns-instruction-sim run --firmware app.elf --session s.json --json
ns-instruction-sim run --firmware app.elf --until 0x80000008 --json
ns-instruction-sim run --firmware app.elf --steps 1000 --trace

# 单步 / 步过 / 步出 / 连续运行
ns-instruction-sim step --session s.json
ns-instruction-sim step --session s.json --source
ns-instruction-sim step-over --session s.json
ns-instruction-sim step-out --session s.json
ns-instruction-sim continue --session s.json --max-steps 100000
ns-instruction-sim continue --session s.json --until 0x80000008 --trace
ns-instruction-sim continue --session s.json --until 0x80000008 --source
ns-instruction-sim continue --session s.json --until 0x80000008

# 寄存器
ns-instruction-sim reg list --session s.json
ns-instruction-sim reg list --session s.json --filter ra --json
ns-instruction-sim reg get --session s.json x1
ns-instruction-sim reg set --session s.json x5 0x2a

# 内存
ns-instruction-sim mem read --session s.json --addr 0x20000000 --len 64
ns-instruction-sim mem read --session s.json --addr 0x20000000 --len 16 --width word
ns-instruction-sim mem read --session s.json --addr 0x20000000 --len 64 --json
ns-instruction-sim mem write --session s.json --addr 0x20000000 --bytes 0x01 0x02 0x03
ns-instruction-sim mem write --session arm.json --arch arm --addr 0x20000000 --bytes 0xaa 0xbb
ns-instruction-sim mem write --session s.json --addr 0x20000000 --width word --values 0xefbeadde 0x12345678
ns-instruction-sim mem fill --session s.json --addr 0x20000000 --len 16 --value 0 --json
ns-instruction-sim mem fill --session s.json --addr 0x20000000 --len 16 --value 0
ns-instruction-sim mem search --session s.json --addr 0x20000000 --len 4096 --pattern 0xde 0xad 0xbe 0xef
ns-instruction-sim mem dump --session s.json --addr 0x20000000 --len 64 --name bootloader --format c
ns-instruction-sim mem dump --session s.json --addr 0x20000000 --len 64 --format c --json

# 断点
ns-instruction-sim bp set --session s.json --addr 0x80000004
ns-instruction-sim bp set --session s.json --addr 0x80000008 --condition "x1 == 3" --hit-count 2
ns-instruction-sim bp set --session s.json --addr 0x8000000c --once
ns-instruction-sim bp set --session s.json --symbol main
ns-instruction-sim bp set --session s.json --line main.c:10
ns-instruction-sim bp set --session s.json --addr 0x80000004 --json
ns-instruction-sim bp list --session s.json
ns-instruction-sim bp clear --session s.json --addr 0x80000004
ns-instruction-sim bp disable --session s.json --addr 0x80000004
ns-instruction-sim bp enable --session s.json --addr 0x80000004

# 反汇编 / 复位 / 状态 / 外设
ns-instruction-sim disasm --session s.json --addr 0x80000000 --count 8
ns-instruction-sim disasm --session arm.json --addr 0x0 --count 4 --arm-state thumb
ns-instruction-sim disasm --session s.json --addr 0x80000000 --count 8 --raw
ns-instruction-sim disasm --session s.json --addr 0x80000000 --count 8 --json
ns-instruction-sim disasm --session s.json --addr 0x80000000 --count 8 --source
ns-instruction-sim reset --session s.json --entry 0x80000000
ns-instruction-sim reset --session s.json --entry 0x80000000 --json
ns-instruction-sim state --session s.json --json
ns-instruction-sim state --session s.json --json --fields pc,cycleCount,registers
ns-instruction-sim state --session s.json --summary
# state --json 另含 firmware{path,base,format} 与 pcSymbol{name,address,size}（无则 null）
ns-instruction-sim periph status --session s.json
ns-instruction-sim periph gpio --session s.json --pin 0 --value 1
ns-instruction-sim periph config --session s.json --uart-base 0x50000000 --gpio-base 0x50002000

# 符号 / 行号
ns-instruction-sim symbol list --session s.json
ns-instruction-sim symbol at --session s.json 0x80000000
ns-instruction-sim symbol line --session s.json 0x80000000
ns-instruction-sim symbol addr --session s.json --file 0 --line 1

# 周期计数
ns-instruction-sim cycle get --session s.json
ns-instruction-sim cycle estimate --session s.json 0x80000000
ns-instruction-sim cycle tick --session s.json --cycles 100

# 中断控制器
ns-instruction-sim interrupt pending --session s.json
ns-instruction-sim interrupt set --session s.json --line 3 --value 1
ns-instruction-sim interrupt take --session s.json

# 看门狗
ns-instruction-sim watchdog status --session s.json
ns-instruction-sim watchdog config --session s.json --enabled true --timeout 1000
ns-instruction-sim watchdog feed --session s.json
ns-instruction-sim watchdog reset --session s.json --entry 0x80000000

# CSR（RISC-V machine-mode 寄存器）
ns-instruction-sim csr list --session s.json
ns-instruction-sim csr get --session s.json mtvec
ns-instruction-sim csr set --session s.json mtvec 0x80000010

# 外设寄存器直读写（UART/GPIO/TIMER/WATCHDOG 内存映射）
ns-instruction-sim periph reg-read --session s.json --addr 0x40002000 --len 4
ns-instruction-sim periph reg-write --session s.json --addr 0x40000000 --bytes 0x48 0x69
ns-instruction-sim periph reg-read --session s.json --name TIM2.CR1 --device stm32f103c8
ns-instruction-sim periph reg-write --session s.json --name TIM2.CR1 --device stm32f103c8 --bytes 0x01 0x00 0x00 0x00
ns-instruction-sim periph field-get --session s.json --device stm32f103c8 --peripheral RCC --register CR --field HSION
ns-instruction-sim periph field-set --session s.json --device stm32f103c8 --peripheral RCC --register CR --field HSION --value 1
ns-instruction-sim periph timer --session s.json --index 1 --enabled true --prescaler 8 --auto-reload 1000

# 从 SVD 文件加载芯片寄存器（地址 -> 复位值）到仿真器，支持直读写与持久化
ns-instruction-sim periph svd-load --session s.json --file crates/ns-device/src/svd/stm32f103c8.svd --json
ns-instruction-sim periph svd-load --session s.json --file chip.svd --peripheral TIM2 --force
ns-instruction-sim periph svd-load --session s.json --device stm32f103c8 --json
ns-instruction-sim periph reg-list --session s.json --device stm32f103c8
```

## 5. 新增子命令流程（必须遵循）

1. **确认能力来源**：目标能力是否在 `crates/ns-sim`/`crates/ns-device` 已有 API。若无，先在
   `ns-sim` 实现（保持 API 稳定）再接入 CLI；若已有，直接适配。
2. **定义命令**：在 `src/main.rs` 添加 `Command`/`Args`/`Subcommand` 变体，提供位置+flag 参数。
3. **实现 `cmd_*`**：复用 `load_or_create`/`Session`/`state_output`；错误用 `bail!`/`Context`。
4. **接入分发**：在 `real_main()` 的 `match` 添加分支。
5. **加端到端测试**：在 `tests/cli.rs` 新增 `#[test]`，断言退出码与输出关键字段。
6. **同步文档**：更新 `AGENTS.md` §3.1 命令总表、§4 待完善清单、`README.md`。
7. **验证**：`cargo build` 零警告零错误 + `cargo test` 全绿 + 实测新命令。

## 6. 禁止事项

- 禁止在 `Cargo.toml` 添加 `path = "../..."` 仓库外依赖；仿真核心必须来自本仓库 `crates/`。
- 禁止用 `todo!()`、`unimplemented!()`、空函数体或假数据冒充实现。
- 禁止用 `#[allow(dead_code)]` 等掩盖警告。
- 禁止破坏 `ns-sim`/`ns-device` 既有 `pub fn` 签名（CLI 层调用依赖它们）。
- 禁止提交未实测的修改；`cargo test` 不全绿不得结束任务。

## 7. 完成定义

一个任务“完成”当且仅当：

1. `cargo build` 0 警告 0 错误；
2. `cargo test` 全绿；
3. 新增命令/功能有 `tests/cli.rs` 覆盖；
4. `AGENTS.md`、`README.md`、`skill.md` 与代码同步；
5. 用真实二进制实测关键路径并确认输出符合契约。
