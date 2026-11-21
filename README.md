# ns-instruction-sim

跨平台嵌入式**指令级仿真命令行工具**。仿真核心（`crates/ns-sim`、`crates/ns-device`）**内嵌于本仓库**，
不依赖任何外部 IDE 工程。外部软件（CI、脚本、其他 IDE、测试框架）通过命令行完整驱动仿真：

```bash
# 加载并运行固件
ns-instruction-sim run --firmware app.elf --save-session s.json --json
ns-instruction-sim arch list --json
ns-instruction-sim device info stm32f103c8 --json
ns-instruction-sim run --firmware app.elf --steps 1000 --json
ns-instruction-sim run --firmware app.elf --session s.json --json
ns-instruction-sim run --firmware app.elf --until 0x80000008 --json
ns-instruction-sim run --firmware app.elf --until-symbol main --json
ns-instruction-sim run --firmware app.elf --until-condition "x1 == 5" --json
ns-instruction-sim run --firmware app.elf --cycle-accurate --max-cycles 1000 --json
ns-instruction-sim run --firmware app.elf --steps 1000 --trace
ns-instruction-sim run --firmware app.elf --steps 100 --trace --trace-regs
ns-instruction-sim version
ns-instruction-sim version --json

# 单步 / 连续运行 / 寄存器 / 内存 / 断点 / 反汇编 / 状态 / 外设
ns-instruction-sim step --session s.json
ns-instruction-sim continue --session s.json --max-steps 100000
ns-instruction-sim continue --session s.json --until 0x80000008
ns-instruction-sim continue --session s.json --max-cycles 1000 --json
ns-instruction-sim continue --session s.json --until 0x80000008 --trace
ns-instruction-sim continue --session s.json --until 0x80000008 --source
ns-instruction-sim continue --session s.json --until-condition "x1 == 5"
ns-instruction-sim reg set --session s.json x5 0x2a
ns-instruction-sim reg list --session s.json --filter ra --json
ns-instruction-sim mem read --session s.json --addr 0x20000000 --len 64
ns-instruction-sim mem read --session s.json --addr 0x20000000 --len 16 --width word
ns-instruction-sim mem read --session s.json --addr 0x20000000 --len 64 --json
ns-instruction-sim mem read --session s.json --addr 0x20000000 --len 64 --ascii
ns-instruction-sim mem read --session s.json --addr 0x20000000 --len 64 --string
ns-instruction-sim mem read --session s.json --addr 0x20000000 --len 64 --string --json
ns-instruction-sim mem write --session arm.json --arch arm --addr 0x20000000 --bytes 0xaa 0xbb
ns-instruction-sim mem write --session s.json --device stm32f103c8 --region RAM --offset 0 --bytes 0x01 0x02
ns-instruction-sim mem write --session s.json --addr 0x20000000 --width word --values 0xefbeadde
ns-instruction-sim mem write --session s.json --addr 0x20000000 --string "hello" --null
ns-instruction-sim mem read --session s.json --symbol g_counter --len 4 --json
ns-instruction-sim mem write --session s.json --symbol g_counter --bytes 0xde 0xad 0xbe 0xef
ns-instruction-sim mem fill --session s.json --addr 0x20000000 --len 16 --value 0 --json
ns-instruction-sim mem fill --session s.json --addr 0x20000000 --len 4096 --random --seed 0x1234 --json
ns-instruction-sim mem fill --session s.json --addr 0x20000000 --len 4096 --pattern 0xDE 0xAD 0xBE 0xEF
ns-instruction-sim mem search --session s.json --addr 0x20000000 --len 4096 --pattern 0xde 0xad 0xbe 0xef
ns-instruction-sim mem search --session s.json --addr 0x20000000 --len 4096 --pattern 0xdeadbeef --width word --json
ns-instruction-sim mem search --session s.json --addr 0x20000000 --len 4096 --pattern 0xde 0xad --json --out search.json
ns-instruction-sim mem dump --session s.json --addr 0x20000000 --len 64 --name bootloader --format c
ns-instruction-sim mem dump --session s.json --addr 0x20000000 --len 64 --format c --json
ns-instruction-sim mem region --session s.json --device stm32f103c8 --region FLASH --len 16
ns-instruction-sim mem region --session s.json --device stm32f103c8 --region RAM --offset 0x100 --len 32 --json
ns-instruction-sim mem copy --session s.json --src 0x08000000 --dst 0x20000000 --len 256
ns-instruction-sim mem copy --session s.json --src 0x08000000 --dst 0x20000000 --len 256 --json
ns-instruction-sim bp set --session s.json --addr 0x80000004
ns-instruction-sim bp set --session s.json --addr 0x80000008 --once
ns-instruction-sim bp set --session s.json --symbol main
ns-instruction-sim bp set --session s.json --line main.c:10
ns-instruction-sim bp set --session s.json --addr 0x80000004 --json
ns-instruction-sim bp modify --session s.json --addr 0x80000004 --condition "x1 == 5" --hit-count 3
ns-instruction-sim bp disable --session s.json --addr 0x80000004
ns-instruction-sim bp list --session s.json --symbols
ns-instruction-sim bp list --session s.json --symbols --json

# 内存观察点（被写入时暂停，lastResult="watchpoint"、watchHit=<地址>）
ns-instruction-sim watch set --session s.json --addr 0x20000000 --len 4
ns-instruction-sim watch set --session s.json --symbol gv --offset 0 --len 4 --json
ns-instruction-sim watch set --session s.json --addr 0x20000000 --len 4 --hit-count 2 --once --json
ns-instruction-sim watch list --session s.json --json
ns-instruction-sim watch list --session s.json --symbols --json
ns-instruction-sim watch modify --session s.json --addr 0x20000000 --len 8 --hit-count 5 --once true --json
ns-instruction-sim watch set --session s.json --device stm32f103c8 --region FLASH --offset 0 --len 4 --json
ns-instruction-sim watch disable --session s.json --addr 0x20000000
ns-instruction-sim watch enable --session s.json --addr 0x20000000
ns-instruction-sim watch clear --session s.json --addr 0x20000000
ns-instruction-sim watch clear-all --session s.json

ns-instruction-sim disasm --session s.json --addr 0x80000000 --count 8
ns-instruction-sim disasm --session arm.json --addr 0x0 --count 4 --arm-state thumb
ns-instruction-sim disasm --session s.json --addr 0x80000000 --count 8 --raw
ns-instruction-sim disasm --session s.json --addr 0x80000000 --count 8 --json
ns-instruction-sim disasm --session s.json --addr 0x80000000 --count 8 --source
ns-instruction-sim disasm --firmware app.elf --addr 0x80000000 --count 8
ns-instruction-sim disasm --firmware app.elf --symbol main --count 16 --json
ns-instruction-sim state --session s.json --json
ns-instruction-sim state --session arm.json --json --fields armItState,pcLimit
ns-instruction-sim state --session s.json --mem 0x20000000 --mem-len 16 --json
ns-instruction-sim state --session s.json --summary
ns-instruction-sim reset --session s.json --entry 0x80000000 --json
ns-instruction-sim periph status --session s.json
ns-instruction-sim periph uart --session s.json --input "AT\r\n"
ns-instruction-sim periph uart --session s.json --bytes 0x41 0x42 0x43
ns-instruction-sim periph uart --session s.json --peek --json
ns-instruction-sim periph uart --session s.json --rx-drain --json
ns-instruction-sim periph gpio --session s.json --pin 0 --direction 1 --output 1 --read --json
ns-instruction-sim periph gpio --session s.json --events --json
ns-instruction-sim periph gpio --session s.json --clear-events

# 符号 / 行号 / 周期 / 中断 / 看门狗 / CSR / 外设寄存器
ns-instruction-sim symbol list --session s.json
ns-instruction-sim symbol nearest --session s.json --addr 0x80000004 --json
ns-instruction-sim cycle get --session s.json
ns-instruction-sim cycle estimate --session s.json --symbol main --json
ns-instruction-sim cycle set --session s.json --count 1000
ns-instruction-sim interrupt pending --session s.json
ns-instruction-sim interrupt vector --session s.json --json
ns-instruction-sim interrupt clear --session s.json --json
ns-instruction-sim watchdog config --session s.json --enabled true --timeout 1000
ns-instruction-sim watchdog reset --session s.json --entry 0x80000000
ns-instruction-sim csr set --session s.json mtvec 0x80000010
ns-instruction-sim periph reg-write --session s.json --addr 0x40000000 --bytes 0x48 0x69
ns-instruction-sim periph reg-read --session s.json --name TIM2.CR1 --device stm32f103c8
ns-instruction-sim periph field-get --session s.json --device stm32f103c8 --peripheral RCC --register CR --field HSION
ns-instruction-sim periph timer --session s.json --index 1 --enabled true --prescaler 8 --auto-reload 1000 --counter 123 --update-count 5
ns-instruction-sim periph timer --session s.json --list --json
ns-instruction-sim periph config --session s.json --uart-base 0x50000000

# 执行轨迹 / 覆盖 / 助记符直方图 / 会话对比
ns-instruction-sim trace --session s.json --steps 200 --json
ns-instruction-sim trace --session s.json --steps 200 --regs --json
ns-instruction-sim firmware info --firmware app.elf --json
ns-instruction-sim trace --session s.json --steps 200 --out trace.json
ns-instruction-sim coverage --session s.json --steps 2000 --json
ns-instruction-sim profile --session s.json --steps 2000
ns-instruction-sim disasm --session s.json --addr 0x80000000 --count 64 --out listing.lst
ns-instruction-sim disasm --session s.json --addr 0x80000000 --count 64 --labels --json
ns-instruction-sim disasm --session s.json --symbol main --count 16 --json
ns-instruction-sim session info --file s.json --json
ns-instruction-sim session diff --a s.json --b s2.json --json
ns-instruction-sim session patch --file s.json --pc 0x80000010 --cycle 42 --reg x1=0x55 --csr mtvec=0x80000010 --json
ns-instruction-sim session patch --file arm.json --arm-state thumb --xpsr 0x01000000 --it 8,0,0 --json

# 设备 / SVD 文件解析
ns-instruction-sim device list
ns-instruction-sim device memory-map stm32f103c8 --addr 0x40010000 --json
ns-instruction-sim device lookup stm32f103c8 --addr 0x40010800 --json
ns-instruction-sim device lookup stm32f103c8 --name GPIOA.CRL --json
ns-instruction-sim device lookup stm32f103c8 --name GPIOA
ns-instruction-sim device svd --file chip.svd --json
ns-instruction-sim device svd --file chip.svd --peripheral USART1 --register DR
ns-instruction-sim device svd --device stm32f103c8 --json
ns-instruction-sim device linker stm32f103c8 --out chip.ld
ns-instruction-sim device startup stm32f103c8 --out startup.s
ns-instruction-sim device init stm32f103c8 --peripheral GPIOA --register GPIOA.CRL=0x44444444 --out periph_init.c

# 从 SVD 文件加载芯片寄存器到仿真器（可直读写/持久化）
ns-instruction-sim periph svd-load --session s.json --file crates/ns-device/src/svd/stm32f103c8.svd --json
ns-instruction-sim periph svd-load --session s.json --file chip.svd --peripheral TIM2 --force
ns-instruction-sim periph svd-load --session s.json --device stm32f103c8 --json
ns-instruction-sim periph reg-dump --session s.json --device stm32f103c8 --json
ns-instruction-sim periph reg-dump --session s.json --device stm32f103c8 --peripheral RCC --fields
ns-instruction-sim periph reg-list --session s.json --device stm32f103c8
```

支持 12 种架构：RISC-V、ARM Cortex-M、MCS-51、MCS-251、C166、Xtensa、AVR、MSP430、V850、STM8、
RX、RL78。详见 [AGENTS.md](AGENTS.md)（开发规范）与 [skill.md](skill.md)（操作指南）。

## MCP（大模型操作仿真）

`ns-instruction-sim mcp [--session s.json]` 启动 MCP stdio 服务器（JSON-RPC 2.0 +
`Content-Length` 帧）。大模型或任何 MCP 客户端可 `tools/list` 获得 ≥50 个工具（架构/设备/会话/
状态/步进/寄存器/内存/断点/反汇编/周期/中断/外设/符号/看门狗/CSR/执行轨迹/覆盖/直方图/
会话对比/设备内存映射），再 `tools/call` 驱动
指令级仿真；状态通过会话文件跨工具调用持久化。详见 `AGENTS.md` §3.4 与 `skill.md` §4.1。

```bash
# 启动（默认会话文件可选）
ns-instruction-sim mcp --session s.json
```

## 帮助

每个命令与子命令、每个参数都有 `--help` / `-h` 帮助文本，任何层级均可查看用法：

```bash
ns-instruction-sim --help
ns-instruction-sim run --help
ns-instruction-sim bp set --help
ns-instruction-sim mcp --help
```

## 构建与测试

```bash
cargo build
cargo test
```
