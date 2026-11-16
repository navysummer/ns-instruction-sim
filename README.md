# ns-instruction-sim

跨平台嵌入式**指令级仿真命令行工具**。仿真核心（`crates/ns-sim`、`crates/ns-device`）**内嵌于本仓库**，
不依赖任何外部 IDE 工程。外部软件（CI、脚本、其他 IDE、测试框架）通过命令行完整驱动仿真：

```bash
# 加载并运行固件
ns-instruction-sim run --firmware app.elf --save-session s.json --json
ns-instruction-sim run --firmware app.elf --steps 1000 --json
ns-instruction-sim run --firmware app.elf --session s.json --json
ns-instruction-sim version

# 单步 / 连续运行 / 寄存器 / 内存 / 断点 / 反汇编 / 状态 / 外设
ns-instruction-sim step --session s.json
ns-instruction-sim continue --session s.json --max-steps 100000
ns-instruction-sim reg set --session s.json x5 0x2a
ns-instruction-sim mem read --session s.json --addr 0x20000000 --len 64
ns-instruction-sim mem write --session arm.json --arch arm --addr 0x20000000 --bytes 0xaa 0xbb
ns-instruction-sim bp set --session s.json --addr 0x80000004
ns-instruction-sim disasm --session s.json --addr 0x80000000 --count 8
ns-instruction-sim disasm --session arm.json --addr 0x0 --count 4 --arm-state thumb
ns-instruction-sim state --session s.json --json
ns-instruction-sim periph status --session s.json

# 符号 / 行号 / 周期 / 中断 / 看门狗 / CSR / 外设寄存器
ns-instruction-sim symbol list --session s.json
ns-instruction-sim cycle get --session s.json
ns-instruction-sim interrupt pending --session s.json
ns-instruction-sim watchdog config --session s.json --enabled true --timeout 1000
ns-instruction-sim csr set --session s.json mtvec 0x80000010
ns-instruction-sim periph reg-write --session s.json --addr 0x40000000 --bytes 0x48 0x69
ns-instruction-sim periph timer --session s.json --index 1 --enabled true
ns-instruction-sim periph config --session s.json --uart-base 0x50000000

# 设备 / SVD 文件解析
ns-instruction-sim device list
ns-instruction-sim device svd --file chip.svd --json
ns-instruction-sim device svd --file chip.svd --peripheral USART1 --register DR
ns-instruction-sim device linker stm32f103c8 --out chip.ld
ns-instruction-sim device startup stm32f103c8 --out startup.s
ns-instruction-sim device init stm32f103c8 --peripheral GPIOA --register GPIOA.CRL=0x44444444 --out periph_init.c

# 从 SVD 文件加载芯片寄存器到仿真器（可直读写/持久化）
ns-instruction-sim periph svd-load --session s.json --file crates/ns-device/src/svd/stm32f103c8.svd --json
ns-instruction-sim periph svd-load --session s.json --file chip.svd --peripheral TIM2 --force
ns-instruction-sim periph svd-load --session s.json --device stm32f103c8 --json
```

支持 12 种架构：RISC-V、ARM Cortex-M、MCS-51、MCS-251、C166、Xtensa、AVR、MSP430、V850、STM8、
RX、RL78。详见 [AGENTS.md](AGENTS.md)（开发规范）与 [skill.md](skill.md)（操作指南）。

## 构建与测试

```bash
cargo build
cargo test
```
