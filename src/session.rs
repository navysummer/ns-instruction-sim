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
use serde_json::Value;

use ns_sim::{Arch, Simulator};

/// A breakpoint as persisted in a session file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BpState {
    pub address: u32,
    pub condition: String,
    pub hit_count: u32,
    /// One-shot breakpoint: auto-cleared after the first hit.
    #[serde(default)]
    pub once: bool,
    /// Whether the breakpoint is active (disabled ones are kept but inert).
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Runtime hit counter (number of times the PC reached this address).
    #[serde(default)]
    pub hits: u64,
}

fn default_true() -> bool {
    true
}

/// A persisted memory watchpoint: `step` / `run` / `continue` compare the
/// watched byte range before and after every instruction and stop when it
/// changes. `hit_count` stops on the N-th write (0 = every write), `once`
/// auto-clears after the first stop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchState {
    pub address: u32,
    /// Number of watched bytes (1..64).
    pub len: u32,
    /// Stop on the N-th hit (0 = every hit).
    #[serde(default)]
    pub hit_count: u32,
    #[serde(default)]
    pub once: bool,
    /// Whether the watchpoint is active (disabled ones are kept but inert).
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Runtime hit counter (number of writes observed so far).
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
    /// Bytes injected into the UART receive queue (FIFO, unconsumed).
    #[serde(default)]
    pub uart_input: Vec<u8>,
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
    /// SVD-defined chip registers (address, value).
    #[serde(default)]
    pub external_regs: Vec<(u32, u32)>,
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
    /// ARM xPSR (condition flags / Thumb bit / exception number), when the
    /// session belongs to an ARM core. Kept separate from `registers` so the
    /// 16-register view stays stable while condition flags survive a session
    /// round-trip.
    #[serde(default)]
    pub arm_xpsr: Option<u32>,
    /// ARM instruction-set state (true = A32, false = Thumb), when ARM.
    #[serde(default)]
    pub arm_arm_state: Option<bool>,
    /// ARM If-Then block mask (bits 3-0 of the IT encoding; 0 = inactive).
    #[serde(default)]
    pub arm_it_mask: Option<u8>,
    /// ARM If-Then block base condition (bits 3-0 of the IT encoding).
    #[serde(default)]
    pub arm_it_cond: Option<u32>,
    /// ARM If-Then block instruction index (0-based within the block).
    #[serde(default)]
    pub arm_it_index: Option<u8>,
    pub breakpoints: Vec<BpState>,
    /// Memory watchpoints (address, len, hit_count, hits, once, enabled).
    /// Persisted like breakpoints so a debug session can keep watching a
    /// variable across `step` / `continue` invocations.
    #[serde(default)]
    pub watchpoints: Vec<WatchState>,
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
            arm_xpsr: if sim.arch == Arch::Arm {
                sim.arm_xpsr()
            } else {
                None
            },
            arm_arm_state: if sim.arch == Arch::Arm {
                Some(sim.arm_state())
            } else {
                None
            },
            arm_it_mask: if sim.arch == Arch::Arm {
                sim.arm_it_state().map(|(mask, _, _)| mask)
            } else {
                None
            },
            arm_it_cond: if sim.arch == Arch::Arm {
                sim.arm_it_state().map(|(_, cond, _)| cond)
            } else {
                None
            },
            arm_it_index: if sim.arch == Arch::Arm {
                sim.arm_it_state().map(|(_, _, index)| index)
            } else {
                None
            },
            breakpoints: sim
                .breakpoints
                .iter()
                .map(|bp| BpState {
                    address: bp.address,
                    condition: bp.condition.clone(),
                    hit_count: bp.hit_count,
                    once: bp.once,
                    enabled: bp.enabled,
                    hits: bp.hits(),
                })
                .collect(),
            cycle_count: sim.cycle_count,
            halted: sim.halted,
            // Watchpoints live in the session file only; they carry their own
            // hit counters, so `capture` never overwrites them.
            watchpoints: Vec::new(),
            uart_output: sim.peripherals.outputs.clone(),
            firmware,
            peripherals: PeriphState {
                uart_base: sim.peripherals.uart_base,
                uart_input: sim.peripherals.uart_rx.borrow().iter().copied().collect(),
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
                external_regs: sim
                    .peripherals
                    .external_regs
                    .iter()
                    .map(|(a, v)| (*a, *v))
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
            memory_overlay: sim.memory_overlay.clone(),
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
        // Restore ARM instruction-set state and condition flags. The state is
        // applied first because it re-derives the xPSR Thumb bit; the saved
        // xPSR is then written back verbatim so flags are exact.
        if let Some(arm_state) = self.arm_arm_state {
            sim.set_arm_state(arm_state);
        }
        if let Some(xpsr) = self.arm_xpsr {
            sim.set_arm_xpsr(xpsr);
        }
        if let Some(mask) = self.arm_it_mask {
            sim.set_arm_it_state(mask, self.arm_it_cond.unwrap_or(0), self.arm_it_index.unwrap_or(0));
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
            if bp.once {
                sim.set_breakpoint_ex_once(bp.address, bp.condition.clone(), bp.hit_count);
            } else {
                sim.set_breakpoint_ex(bp.address, bp.condition.clone(), bp.hit_count);
            }
            if !bp.enabled {
                sim.set_breakpoint_enabled(bp.address, false);
            }
            sim.set_breakpoint_hits(bp.address, bp.hits);
        }
        // Restore virtual-peripheral state.
        sim.peripherals.uart_base = self.peripherals.uart_base;
        sim.peripherals.uart_rx.borrow_mut().extend(self.peripherals.uart_input.iter().copied());
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
        for (addr, value) in &self.peripherals.external_regs {
            sim.peripherals.external_regs.insert(*addr, *value);
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
        // Replay re-records each entry into the fresh simulator; overwrite with
        // the canonical loaded overlay so the list never duplicates on reload.
    sim.memory_overlay = self.memory_overlay.clone();
    Ok(sim)
    }

    /// Machine-readable session summary (path, sizes, counts and key fields).
    pub fn info_value(&self, path: Option<&Path>) -> serde_json::Value {
        let peripherals = &self.peripherals;
        let watchdog = peripherals.watchdog.as_ref();
        let firmware = self.firmware.as_ref();
        serde_json::json!({
            "path": path.map(|p| p.display().to_string()),
            "arch": self.arch,
            "pc": self.pc,
            "cycleCount": self.cycle_count,
            "halted": self.halted,
            "registers": self.registers.len(),
            "armXpsr": self.arm_xpsr,
            "armState": self.arm_arm_state.map(|arm| if arm { "a32" } else { "thumb" }),
            "breakpoints": self.breakpoints.len(),
            "enabledBreakpoints": self.breakpoints.iter().filter(|b| b.enabled).count(),
            "memoryBytes": self.memory.len(),
            "memoryOverlayBytes": self.memory_overlay.len(),
            "uartBytes": self.uart_output.len(),
            "firmware": firmware.map(|f| serde_json::json!({
                "path": f.path,
                "base": f.base,
                "format": f.format,
            })),
            "peripherals": serde_json::json!({
                "uartBase": peripherals.uart_base,
                "gpioBase": peripherals.gpio_base,
                "timers": peripherals.timers.len(),
                "watchdog": watchdog.is_some(),
                "watchdogBase": watchdog.map(|w| w.0),
                "watchdogEnabled": watchdog.map(|w| w.1),
                "irqPending": peripherals.irq_pending.clone(),
                "externalRegs": peripherals.external_regs.len(),
                "csr": serde_json::json!({
                    "mstatus": self.csr.mstatus,
                    "mtvec": self.csr.mtvec,
                    "mepc": self.csr.mepc,
                    "mcause": self.csr.mcause,
                    "mie": self.csr.mie,
                    "mhartid": self.csr.mhartid,
                }),
            }),
        })
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

/// Compare two session files and describe every difference between them.
///
/// Returns `{a, b, same, summary, differences}`. Each difference carries a
/// `kind` (`scalar` / `register` / `breakpoint` / `memory`) plus the two
/// values; the list is ordered (scalars, registers, breakpoints, memory) and
/// capped at `limit` entries so huge sessions stay printable.
pub fn session_diff(a_path: &Path, b_path: &Path, limit: usize) -> Result<serde_json::Value> {
    let a = Session::load(a_path)?;
    let b = Session::load(b_path)?;
    let a_arch = parse_arch(&a.arch).unwrap_or(Arch::RiscV);
    let b_arch = parse_arch(&b.arch).unwrap_or(Arch::RiscV);
    let mut ordered: Vec<serde_json::Value> = Vec::new();
    let mut counts = serde_json::json!({
        "scalar": 0, "register": 0, "breakpoint": 0, "memory": 0, "truncated": 0
    });
    let mut truncated: u64 = 0;
    let mut push = |kind: &str, name: String, av: serde_json::Value, bv: serde_json::Value| {
        if ordered.len() >= limit {
            truncated += 1;
            return;
        }
        counts[kind] = Value::from(
            counts.get(kind).and_then(|v| v.as_u64()).unwrap_or(0) + 1,
        );
        ordered.push(serde_json::json!({ "kind": kind, "name": name, "a": av, "b": bv }));
    };

    // Scalar fields (architecture, PC, cycle count, halt flag, ARM state).
    for (field, av, bv) in [
        ("arch", a.arch.clone(), b.arch.clone()),
        ("pc", format!("{:#010x}", a.pc), format!("{:#010x}", b.pc)),
        ("cycleCount", a.cycle_count.to_string(), b.cycle_count.to_string()),
        ("halted", a.halted.to_string(), b.halted.to_string()),
        ("armXpsr", a.arm_xpsr.map(|v| format!("{v:#010x}")).unwrap_or_default(),
         b.arm_xpsr.map(|v| format!("{v:#010x}")).unwrap_or_default()),
        ("armState", a.arm_arm_state.map(|s| if s { "a32" } else { "thumb" }).unwrap_or_default().to_string(),
         b.arm_arm_state.map(|s| if s { "a32" } else { "thumb" }).unwrap_or_default().to_string()),
    ] {
        if av != bv {
            push("scalar", field.to_string(), av.into(), bv.into());
        }
    }

    // Registers, aligned by (index, name) so differing architectures still
    // diff field-wise instead of one opaque block.
    let a_regs: BTreeMap<usize, (String, u32)> = a
        .registers
        .iter()
        .map(|(i, v)| (*i, (ns_sim::reg_label_for(a_arch, *i), *v)))
        .collect();
    let b_regs: BTreeMap<usize, (String, u32)> = b
        .registers
        .iter()
        .map(|(i, v)| (*i, (ns_sim::reg_label_for(b_arch, *i), *v)))
        .collect();
    let mut keys: Vec<usize> = a_regs.keys().chain(b_regs.keys()).copied().collect();
    keys.sort();
    keys.dedup();
    for key in keys {
        let av = a_regs.get(&key).map(|(_, v)| *v);
        let bv = b_regs.get(&key).map(|(_, v)| *v);
        if av != bv {
            let name = b_regs
                .get(&key)
                .or_else(|| a_regs.get(&key))
                .map(|(n, _)| n.clone())
                .unwrap_or_else(|| format!("r{key}"));
            push(
                "register",
                name,
                av.map(|v| format!("{v:#010x}")).map(Value::from).unwrap_or(Value::Null),
                bv.map(|v| format!("{v:#010x}")).map(Value::from).unwrap_or(Value::Null),
            );
        }
    }

    // Breakpoints, keyed by address with every persisted attribute compared.
    let a_bp: BTreeMap<u32, serde_json::Value> =
        a.breakpoints.iter().map(|x| (x.address, bp_json(x))).collect();
    let b_bp: BTreeMap<u32, serde_json::Value> =
        b.breakpoints.iter().map(|x| (x.address, bp_json(x))).collect();
    let mut bp_keys: Vec<u32> = a_bp.keys().chain(b_bp.keys()).copied().collect();
    bp_keys.sort();
    bp_keys.dedup();
    for addr in bp_keys {
        if a_bp.get(&addr) != b_bp.get(&addr) {
            push(
                "breakpoint",
                format!("0x{addr:08x}"),
                a_bp.get(&addr).cloned().unwrap_or(serde_json::Value::Null),
                b_bp.get(&addr).cloned().unwrap_or(serde_json::Value::Null),
            );
        }
    }

    // Memory overlay, keyed by address (byte value).
    let a_mem: BTreeMap<u32, u8> = a.memory_overlay.iter().map(|(k, v)| (*k, *v)).collect();
    let b_mem: BTreeMap<u32, u8> = b.memory_overlay.iter().map(|(k, v)| (*k, *v)).collect();
    let mut mem_keys: Vec<u32> = a_mem.keys().chain(b_mem.keys()).copied().collect();
    mem_keys.sort();
    mem_keys.dedup();
    for addr in mem_keys {
        if a_mem.get(&addr) != b_mem.get(&addr) {
            push(
                "memory",
                format!("0x{addr:08x}"),
                a_mem.get(&addr).map(|v| Value::from(format!("{v:02x}"))).unwrap_or(Value::Null),
                b_mem.get(&addr).map(|v| Value::from(format!("{v:02x}"))).unwrap_or(Value::Null),
            );
        }
    }

    let same = ordered.is_empty() && truncated == 0;
    counts["truncated"] = Value::from(truncated);
    Ok(serde_json::json!({
        "a": a.info_value(Some(a_path)),
        "b": b.info_value(Some(b_path)),
        "same": same,
        "summary": counts,
        "differences": ordered,
    }))
}

/// Serialize one persisted breakpoint for the diff view.
fn bp_json(bp: &BpState) -> serde_json::Value {
    serde_json::json!({
        "condition": bp.condition,
        "hitCount": bp.hit_count,
        "once": bp.once,
        "enabled": bp.enabled,
        "hits": bp.hits,
    })
}
