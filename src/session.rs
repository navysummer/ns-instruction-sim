//! Persistent simulator session state.
//!
//! A session is a JSON file that captures enough of the simulator's state
//! (architecture, PC, registers, breakpoints, cycle count, UART output and
//! the loaded firmware) for an external tool to drive simulation across
//! multiple command-line invocations. The backing memory is persisted as a
//! sparse list of non-zero bytes (the firmware is re-loaded from its path on
//! resume first), plus a replay overlay for explicit `mem write`/`fill` so
//! runtime memory writes survive a session round-trip for every architecture,
//! keeping the session file small and portable.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use ns_sim::{Arch, Simulator};

/// A breakpoint as persisted in a session file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BpState {
    pub address: u32,
    pub condition: String,
    pub hit_count: u32,
    /// Runtime hit counter (number of times the PC reached this address).
    #[serde(default)]
    pub hits: u64,
}

/// Firmware source recorded so the simulator can be rebuilt on resume.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FirmwareState {
    pub path: String,
    pub base: u32,
    /// `"elf"`, `"hex"` or `"bin"`.
    pub format: String,
}

/// Persisted virtual-peripheral state.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PeriphState {
    /// UART register base address.
    #[serde(default)]
    pub uart_base: u32,
    /// GPIO register base address.
    #[serde(default)]
    pub gpio_base: u32,
    pub gpio_out: u32,
    pub gpio_dir: u32,
    pub gpio_in: u32,
    /// (base, enabled, prescaler, counter, auto_reload, irq_line, update_count).
    pub timers: Vec<(u32, bool, u32, u32, u32, usize, u64)>,
    /// (base, enabled, timeout_cycles, elapsed, reset_count, last_reset_pc).
    pub watchdog: Option<(u32, bool, u64, u64, u64, u32)>,
    /// Peripheral interrupt lines that are currently asserted (0..16).
    #[serde(default)]
    pub irq_pending: Vec<usize>,
}

/// Persisted machine-mode CSR values (RISC-V). Kept across all architectures
/// for a uniform session format; non-RISC-V cores leave these at default 0.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CsrState {
    #[serde(default)]
    pub mstatus: u32,
    #[serde(default)]
    pub mtvec: u32,
    #[serde(default)]
    pub mepc: u32,
    #[serde(default)]
    pub mcause: u32,
    #[serde(default)]
    pub mie: u32,
    #[serde(default)]
    pub mhartid: u32,
}

/// Serializable session snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub arch: String,
    pub pc: u32,
    /// (register index, value) pairs, in display order.
    pub registers: Vec<(usize, u32)>,
    pub breakpoints: Vec<BpState>,
    pub cycle_count: u64,
    pub halted: bool,
    pub uart_output: Vec<u8>,
    pub firmware: Option<FirmwareState>,
    #[serde(default)]
    pub peripherals: PeriphState,
    #[serde(default)]
    pub csr: CsrState,
    /// Non-zero bytes of the simulator's backing memory (offset, value). Only
    /// non-zero bytes are stored to keep the session small; a byte explicitly
    /// cleared to 0 is not distinguished from one never written.
    #[serde(default)]
    pub memory: Vec<(u32, u8)>,
    /// Explicit memory writes recorded by `mem write`/`fill`, replayed through
    /// the architecture-aware writer on resume. This keeps runtime memory
    /// writes visible across sessions for every architecture (not just the
    /// RISC-V backing store captured in `memory`).
    #[serde(default)]
    pub memory_overlay: Vec<(u32, u8)>,
}

impl Session {
    /// Load a session file.
    pub fn load(path: &Path) -> Result<Session> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("读取会话 {} 失败", path.display()))?;
        serde_json::from_str(&text)
            .with_context(|| format!("解析会话 {} 失败", path.display()))
    }

    /// Save a session file.
    pub fn save(&self, path: &Path) -> Result<()> {
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(path, text)
            .with_context(|| format!("写入会话 {} 失败", path.display()))
    }

    /// Take a snapshot of the current simulator.
    pub fn capture(sim: &Simulator, firmware: Option<FirmwareState>) -> Session {
        let count = ns_sim::reg_count_for(sim.arch);
        let mut registers = Vec::with_capacity(count);
        for i in 0..count {
            registers.push((i, sim.read_reg(i)));
        }
        Session {
            arch: arch_name(sim.arch).to_string(),
            pc: sim.program_counter(),
            registers,
            breakpoints: sim
                .breakpoints
                .iter()
                .map(|bp| BpState {
                    address: bp.address,
                    condition: bp.condition.clone(),
                    hit_count: bp.hit_count,
                    hits: bp.hits(),
                })
                .collect(),
            cycle_count: sim.cycle_count,
            halted: sim.halted,
            uart_output: sim.peripherals.outputs.clone(),
            firmware,
            peripherals: PeriphState {
                uart_base: sim.peripherals.uart_base,
                gpio_base: sim.peripherals.gpio_base,
                gpio_out: sim.peripherals.gpio_out,
                gpio_dir: sim.peripherals.gpio_dir,
                gpio_in: sim.peripherals.gpio_in,
                timers: sim
                    .peripherals
                    .timers
                    .iter()
                    .map(|t| (t.base, t.enabled, t.prescaler, t.counter, t.auto_reload, t.irq_line, t.update_count))
                    .collect(),
                watchdog: sim.peripherals.watchdog.as_ref().map(|w| {
                    (
                        w.base,
                        w.enabled,
                        w.timeout_cycles,
                        w.elapsed,
                        w.reset_count,
                        w.last_reset_pc,
                    )
                }),
                irq_pending: sim
                    .peripherals
                    .irq_pending
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| **p)
                    .map(|(i, _)| i)
                    .collect(),
            },
            csr: CsrState {
                mstatus: sim.csr.mstatus,
                mtvec: sim.csr.mtvec,
                mepc: sim.csr.mepc,
                mcause: sim.csr.mcause,
                mie: sim.csr.mie,
                mhartid: sim.csr.mhartid,
            },
            memory: sim
                .memory
                .iter()
                .enumerate()
                .filter(|(_, b)| **b != 0)
                .map(|(i, b)| (i as u32, *b))
                .collect(),
            memory_overlay: Vec::new(),
        }
    }

    /// Restore a snapshot into a fresh simulator (with firmware re-loaded).
    pub fn apply(&self, firmware: Option<&FirmwareState>) -> Result<Simulator> {
        let arch = parse_arch(&self.arch)?;
        let mut sim = Simulator::for_arch(arch)
            .map_err(|e| anyhow::anyhow!("创建 {} 仿真器失败: {e}", arch.label()))?;

        // Re-load firmware if recorded.
        if let Some(fw) = firmware.or(self.firmware.as_ref()) {
            let path = Path::new(&fw.path);
            if !path.exists() {
                bail!("固件文件不存在: {}", path.display());
            }
            crate::sim::load_firmware_into(&mut sim, path, fw.base)?;
        }

        // Restore register file (all architectures route through write_reg).
        for (index, value) in &self.registers {
            sim.write_reg(*index, *value);
        }
        // PC is a register in some cores (ARM/XTENSA), so apply after regs.
        sim.set_program_counter(self.pc);
        sim.cycle_count = self.cycle_count;
        sim.halted = self.halted;
        if !sim.peripherals.outputs.is_empty() {
            sim.peripherals.outputs.clear();
        }
        sim.peripherals.outputs.extend_from_slice(&self.uart_output);
        for bp in &self.breakpoints {
            sim.set_breakpoint_ex(bp.address, bp.condition.clone(), bp.hit_count);
            sim.set_breakpoint_hits(bp.address, bp.hits);
        }
        // Restore virtual-peripheral state.
        sim.peripherals.uart_base = self.peripherals.uart_base;
        sim.peripherals.gpio_base = self.peripherals.gpio_base;
        sim.peripherals.gpio_out = self.peripherals.gpio_out;
        sim.peripherals.gpio_dir = self.peripherals.gpio_dir;
        sim.peripherals.gpio_in = self.peripherals.gpio_in;
        for (base, enabled, prescaler, counter, auto_reload, irq_line, update_count) in
            &self.peripherals.timers
        {
            let mut t = ns_sim::Timer::new(*base, *irq_line);
            t.enabled = *enabled;
            t.prescaler = *prescaler;
            t.counter = *counter;
            t.auto_reload = *auto_reload;
            t.update_count = *update_count;
            sim.peripherals.timers.push(t);
        }
        if let Some((base, enabled, timeout_cycles, elapsed, reset_count, last_reset_pc)) =
            &self.peripherals.watchdog
        {
            let mut w = ns_sim::Watchdog::new(*base);
            w.enabled = *enabled;
            w.timeout_cycles = *timeout_cycles;
            w.elapsed = *elapsed;
            w.reset_count = *reset_count;
            w.last_reset_pc = *last_reset_pc;
            sim.peripherals.watchdog = Some(w);
        }
        for line in &self.peripherals.irq_pending {
            if *line < 16 {
                sim.peripherals.irq_pending[*line] = true;
            }
        }
        sim.csr.mstatus = self.csr.mstatus;
        sim.csr.mtvec = self.csr.mtvec;
        sim.csr.mepc = self.csr.mepc;
        sim.csr.mcause = self.csr.mcause;
        sim.csr.mie = self.csr.mie;
        sim.csr.mhartid = self.csr.mhartid;
        for (offset, value) in &self.memory {
            let idx = (*offset as usize) % ns_sim::MEMORY_SIZE;
            sim.memory[idx] = *value;
        }
        for (addr, value) in &self.memory_overlay {
            crate::sim::write_mem_byte(&mut sim, *addr, *value);
        }
        Ok(sim)
    }
}

/// Machine-readable name for an architecture (used in JSON / state files).
pub fn arch_name(arch: Arch) -> &'static str {
    match arch {
        Arch::RiscV => "riscv",
        Arch::Arm => "arm",
        Arch::Mcs51 => "mcs51",
        Arch::Mcs251 => "mcs251",
        Arch::C166 => "c166",
        Arch::Xtensa => "xtensa",
        Arch::Avr => "avr",
        Arch::Msp430 => "msp430",
        Arch::V850 => "v850",
        Arch::Stm8 => "stm8",
        Arch::Rx => "rx",
        Arch::Rl78 => "rl78",
    }
}

/// Parse a machine-readable architecture name (case-insensitive).
pub fn parse_arch(name: &str) -> Result<Arch> {
    let n = name.to_ascii_lowercase();
    let arch = match n.as_str() {
        "riscv" | "rv32" | "rv32i" | "risc-v" => Arch::RiscV,
        "arm" | "cortex" | "cortex-m" => Arch::Arm,
        "mcs51" | "8051" | "c51" => Arch::Mcs51,
        "mcs251" | "80251" | "c251" => Arch::Mcs251,
        "c166" | "xc166" | "xc2000" | "st10" => Arch::C166,
        "xtensa" | "lx6" | "lx7" | "esp32" => Arch::Xtensa,
        "avr" | "atmega" | "attiny" => Arch::Avr,
        "msp430" => Arch::Msp430,
        "v850" => Arch::V850,
        "stm8" => Arch::Stm8,
        "rx" | "rx600" | "r5f" => Arch::Rx,
        "rl78" => Arch::Rl78,
        _ => bail!("未知架构: {name}（支持: riscv/arm/mcs51/mcs251/c166/xtensa/avr/msp430/v850/stm8/rx/rl78）"),
    };
    Ok(arch)
}

/// A map of register name -> index for the current architecture, used by the
/// `reg` subcommand to resolve names case-insensitively.
pub fn register_index_by_name(arch: Arch) -> BTreeMap<String, usize> {
    let mut map = BTreeMap::new();
    let count = ns_sim::reg_count_for(arch);
    for i in 0..count {
        let label = ns_sim::reg_label_for(arch, i);
        map.insert(label.to_ascii_lowercase(), i);
        // Numeric aliases: xN (RISC-V), rN (ARM/C166/RX), aN (Xtensa).
        let alias = match arch {
            Arch::RiscV => format!("x{i}"),
            Arch::Arm | Arch::C166 | Arch::Rx => format!("r{i}"),
            Arch::Xtensa => format!("a{i}"),
            _ => String::new(),
        };
        if !alias.is_empty() {
            map.insert(alias, i);
        }
    }
    map
}
