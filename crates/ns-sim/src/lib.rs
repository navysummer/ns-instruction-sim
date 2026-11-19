//! `ns-sim` — a built-in instruction-level simulator for ns-embed-ide.
//!
//! Implements a clean, verifiable RV32I (RISC-V 32-bit integer) core that can
//! load a program image (raw bytes or an ELF32), step/run instructions, and
//! expose registers and memory. A small virtual-peripheral model provides a
//! memory-mapped UART whose output the IDE can surface in the console.

use serde::Serialize;

pub mod mcs51;

pub mod arm;

pub mod x51;

pub mod c166;

pub mod xtensa;

pub mod avr;

pub mod msp430;

pub mod v850;

pub mod stm8;

pub mod rx;

pub mod rl78;

use std::collections::HashMap;
pub const REG_COUNT: usize = 32;
pub const MEMORY_SIZE: usize = 64 * 1024; // 64 KiB simulated RAM
pub const ENTRY_POINT: u32 = 0x8000_0000;

/// The target instruction-set architecture a simulator instance runs.
///
/// Each variant maps to a concrete core implementation (`for_arch` constructs
/// the matching core): RV32I, ARM (Thumb/Thumb-2 + A32), MCS-51, MCS-251,
/// C166, Xtensa, AVR, MSP430, V850, STM8, RX and RL78. Every core has its own
/// register/PC/memory model and single-step decoder, selected by the target
/// MCU in `project.toml` or by the ELF `e_machine`.
///
/// [`Arch::label`] and [`Arch::from_target`] are used by the IDE to report
/// which core a loaded artifact belongs to; `from_elf_machine` maps the ELF
/// header so a mismatched artifact is rejected rather than silently mis-decoded.
///
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    /// RISC-V 32-bit (RV32I); used by ESP32-C3 and generic RISC-V targets.
    RiscV,
    /// ARM Cortex-M (Thumb/Thumb-2); used by STM32, nRF52, RP2040.
    Arm,
    /// Intel 8051 / MCS-51 8-bit microcontroller.
    Mcs51,
    /// Intel 80251 / MCS-251 16-bit (8051 superset).
    Mcs251,
    /// Infineon C166 16-bit (and its XC166/XC2000 derivatives).
    C166,
    /// ESP32 Xtensa (LX6/LX7, e.g. ESP32/ESP32-S3).
    Xtensa,
    /// Atmel/Microchip AVR 8-bit (ATmega/ATtiny; IAR AVR).
    Avr,
    /// TI MSP430 16-bit (IAR MSP430).
    Msp430,
    /// Renesas V850E 32-bit (IAR V850).
    V850,
    /// ST Microelectronics STM8 (ST8 core, IAR STM8).
    Stm8,
    /// Renesas RX (RX600 family, IAR RX).
    Rx,
    /// Renesas RL78 (16-bit CISC, IAR RL78).
    Rl78,

}

impl Arch {
    /// Human-readable label for status messages.
    pub fn label(self) -> &'static str {
        match self {
            Arch::RiscV => "RISC-V (RV32I)",
            Arch::Arm => "ARM Cortex-M",
            Arch::Mcs51 => "Intel 8051 (MCS-51)",
            Arch::Mcs251 => "Intel 80251 (MCS-251)",
            Arch::C166 => "Infineon C166 (16-bit)",
            Arch::Xtensa => "ESP32 Xtensa",
            Arch::Avr => "Atmel AVR (8-bit)",
            Arch::Msp430 => "TI MSP430 (16-bit)",
            Arch::V850 => "Renesas V850 (32-bit)",
            Arch::Stm8 => "STMicroelectronics STM8",
            Arch::Rx => "Renesas RX (32-bit)",
            Arch::Rl78 => "Renesas RL78 (16-bit)",
        }
    }

    /// Map an ELF `e_machine` field to an architecture.
    pub fn from_elf_machine(machine: u16) -> Option<Arch> {
        match machine {
            243 => Some(Arch::RiscV),  // EM_RISCV
            40 => Some(Arch::Arm),     // EM_ARM
            94 => Some(Arch::Xtensa),  // EM_XTENSA
            83 => Some(Arch::Avr),     // EM_AVR
            105 | 167 => Some(Arch::Msp430), // EM_MSP430 / EM_MSP430_2
            87 => Some(Arch::V850),    // EM_V850
            186 => Some(Arch::Stm8),   // EM_STM8
            173 => Some(Arch::Rx),     // EM_RX (Renesas RX)
            197 => Some(Arch::Rl78),   // EM_RL78 (Renesas RL78)
            _ => None,
        }
    }

    /// Map a `project.toml` target key (e.g. `stm32f103c8`, `esp32c3`) to an
    /// architecture. `catalog_entry`-independent: uses the same core naming the
    /// device catalog uses. Unknown targets default to [`Arch::RiscV`] since
    /// that is the only implemented core and the historical default.
    pub fn from_target(target: &str) -> Arch {
        let t = target.to_ascii_lowercase();
        if t.contains("riscv") || t.starts_with("esp32c") {
            Arch::RiscV
        } else if t.contains("esp32") {
            Arch::Xtensa
        } else if t.contains("80251") || t.contains("mcs251") || t.contains("c251") {
            Arch::Mcs251
        } else if t.contains("c166")
            || t.contains("xc166")
            || t.contains("xc2000")
            || t.contains("c16x")
            || t.contains("st10")
        {
            Arch::C166
        } else if t.contains("8051") || t.contains("mcs51") || t.contains("c51") {
            Arch::Mcs51
        } else if t.contains("atmega")
            || t.contains("attiny")
            || t.contains("at90")
            || t.contains("avr")
        {
            Arch::Avr
        } else if t.contains("msp430") {
            Arch::Msp430
        } else if t.contains("v850") {
            Arch::V850
        } else if t.contains("stm8")
            || t.contains("stm8s")
            || t.contains("stm8l")
            || t.starts_with("stm8")
        {
            Arch::Stm8
        } else if t.contains("rx") || t.starts_with("r5f") {
            Arch::Rx
        } else if t.contains("rl78") {
            Arch::Rl78
        } else if t.contains("arm7")
            || t.contains("arm9")
            || t.contains("arm926")
            || t.contains("arm920")
            || t.contains("lpc21")
            || t.contains("lpc22")
            || t.contains("s3c24")
            || t.contains("s3c2440")
            || t.contains("at91")
            || t.contains("imx")
            || t.contains("a32")
            || t.contains("arm-state")
        {
            Arch::Arm
        } else if t.starts_with("stm32")
            || t.starts_with("nrf")
            || t.starts_with("rp2040")
            || t.contains("cortex")
        {
            Arch::Arm
        } else {
            Arch::RiscV
        }
    }
}

/// A single GPIO output-change event. `cycle` is the cycle count when the
/// write happened, and `pc` is the program counter of the instruction that
/// caused it — so the IDE can align the event with the source location of the
/// store, not just a wall-clock tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpioEvent {
    /// Cycle count when the change was committed.
    pub cycle: u64,
    /// Pin index (0..32).
    pub pin: u32,
    /// New pin value (0 or 1).
    pub value: u32,
    /// Program counter of the store instruction that caused the change.
    pub pc: u32,
}

/// A virtual peripheral: a memory-mapped UART at `base`, whose `putchar`
/// callback the simulator invokes on writes to the data register.
pub struct VirtualPeripherals {
    pub uart_base: u32,
    pub putchar: Box<dyn FnMut(u8) + Send>,
    pub outputs: Vec<u8>,
    /// GPIO register base address.
    pub gpio_base: u32,
    /// GPIO output value (bit per pin).
    pub gpio_out: u32,
    /// GPIO direction mask (1 = output).
    pub gpio_dir: u32,
    /// GPIO input value (bit per pin).
    pub gpio_in: u32,
    /// Log of GPIO output changes: (cycle, pin, value).
    pub gpio_events: Vec<GpioEvent>,
    /// Up to 8 general-purpose timers.
    pub timers: Vec<Timer>,
    /// The watchdog timer (WDT), if enabled by the firmware.
    pub watchdog: Option<Watchdog>,
    /// Pending interrupt lines asserted by peripherals.
    pub irq_pending: [bool; 16],
    /// Generic chip registers loaded from an SVD file (address -> value).
    /// Consulted as a fallback by `peripheral_read`/`peripheral_write` after
    /// the fixed UART/GPIO/timer/watchdog model, letting external callers read
    /// and write arbitrary memory-mapped registers.
    pub external_regs: HashMap<u32, u32>,
}

/// A virtual watchdog timer. When enabled, it counts core cycles; if the
/// firmware fails to feed it (write 0xAA55 to the reload register) before
/// `timeout_cycles` elapse, the watchdog fires a reset (`reset_count`
/// increments and `last_reset_pc` records the PC that overflowed). The IDE
/// can surface the reset count so runaway firmware is visible rather than
/// silently stalling.
#[derive(Debug, Clone)]
pub struct Watchdog {
    pub base: u32,
    pub enabled: bool,
    /// Cycles before the watchdog fires (0 = never while disabled).
    pub timeout_cycles: u64,
    /// Cycles elapsed since the last feed or enable.
    pub elapsed: u64,
    /// Number of times the watchdog has fired (reset events).
    pub reset_count: u64,
    /// PC at which the watchdog fired (for the IDE to report).
    pub last_reset_pc: u32,
}

impl Watchdog {
    pub fn new(base: u32) -> Self {
        Self {
            base,
            enabled: false,
            timeout_cycles: 0,
            elapsed: 0,
            reset_count: 0,
            last_reset_pc: 0,
        }
    }
}

/// A virtual general-purpose timer: prescaler + up-counter with auto-reload.
#[derive(Debug, Clone)]
pub struct Timer {
    pub base: u32,
    pub enabled: bool,
    pub prescaler: u32,
    pub counter: u32,
    pub auto_reload: u32,
    /// Cycles until the next prescaler wrap.
    prescaler_clock: u32,
    /// Interrupt line this timer raises on update.
    pub irq_line: usize,
    /// Number of update events (overflows) fired.
    pub update_count: u64,
}

impl Timer {
    pub fn new(base: u32, irq_line: usize) -> Self {
        Self {
            base,
            enabled: false,
            prescaler: 0,
            counter: 0,
            auto_reload: 0,
            prescaler_clock: 0,
            irq_line,
            update_count: 0,
        }
    }

    /// Advance the timer by one core cycle. Returns the number of update events
    /// (overflows) fired by this cycle: 0 for a normal prescaler tick, and 1+
    /// when the up-counter crosses its auto-reload in one cycle (the counter may
    /// be several reloads ahead when `run()` loops at high speed, so the exact
    /// count is computed rather than treated as a single event).
    pub fn tick(&mut self) -> u64 {
        if !self.enabled {
            return 0;
        }
        let prescaler = (self.prescaler & 0xffff).max(1);
        self.prescaler_clock = self.prescaler_clock.wrapping_add(1);
        if self.prescaler_clock >= prescaler {
            self.prescaler_clock = 0;
            let reload = self.auto_reload.max(1);
            if self.counter >= reload {
                let overflows = self.counter / reload;
                self.counter = self.counter % reload;
                self.update_count = self.update_count.saturating_add(overflows as u64);
                overflows as u64
            } else {
                self.counter = self.counter.saturating_add(1);
                0
            }
        } else {
            0
        }
    }
}

impl VirtualPeripherals {
    pub fn new(uart_base: u32) -> Self {
        Self {
            uart_base,
            putchar: Box::new(|_| {}),
            outputs: Vec::new(),
            gpio_base: 0x4000_2000,
            gpio_out: 0,
            gpio_dir: 0,
            gpio_in: 0,
            gpio_events: Vec::new(),
            timers: Vec::new(),
            watchdog: None,
            irq_pending: [false; 16],
            external_regs: HashMap::new(),
        }
    }
}

/// Machine-mode control and status registers for the RV32I core. The simulator
/// implements the minimal set needed for interrupt handling: the trap vector,
/// the saved program counter/cause, global interrupt enable, and pending/mask
/// bits that gate which peripheral lines are taken.
#[derive(Debug, Clone)]
pub struct Csr {
    /// `mstatus`: machine status. Bit 3 = MIE (global interrupt enable).
    pub mstatus: u32,
    /// `mtvec`: trap vector base. Bits 0..1 mode: 0 = direct, 1 = vectored.
    pub mtvec: u32,
    /// `mepc`: program counter saved on trap entry.
    pub mepc: u32,
    /// `mcause`: cause of the last trap (interrupt = bit 31 set).
    pub mcause: u32,
    /// `mie`: machine interrupt-enable bits (one per peripheral line).
    pub mie: u32,
    /// `mhartid`: hardware thread id (always 0 for this single-core model).
    pub mhartid: u32,
}

impl Default for Csr {
    fn default() -> Self {
        Self {
            mstatus: 0,
            mtvec: 0,
            mepc: 0,
            mcause: 0,
            mie: 0,
            mhartid: 0,
        }
    }
}

/// The RV32I simulator core.
pub struct Simulator {
    pub pc: u32,
    pub registers: [u32; REG_COUNT],
    pub memory: Vec<u8>,
    /// Explicit data-space writes that must be replayed when the session is
    /// reloaded (architecture cores with their own memory model do not mirror
    /// into `memory`, so the CLI records them here for cross-invocation replay).
    pub memory_overlay: Vec<(u32, u8)>,
    /// Whether execution should stop (a breakpoint or halt instruction).
    pub halted: bool,
    /// Simulated breakpoints (with optional condition / hit count).
    pub breakpoints: Vec<SimBreakpoint>,

    /// Steps executed since the last reset.
    pub cycle_count: u64,
    pub peripherals: VirtualPeripherals,
    /// Function symbols parsed from the ELF (name, address, size).
    pub symbols: Vec<Symbol>,
    /// A flattened source line table: (address, line number, file index).
    pub line_table: Vec<LineEntry>,
    /// Source file names referenced by the line table.
    pub source_files: Vec<String>,
    /// Machine-mode control and status registers (interrupt support).
    pub csr: Csr,
    /// Target architecture this simulator was created for. Controls which
    /// instruction decoder is used; only [`Arch::RiscV`] is implemented so far.
    pub arch: Arch,
    /// The 8051 core, present only when `arch == Arch::Mcs51`. The RV32I
    /// fields (`pc`, `registers`, `memory`) are unused in that mode.
    pub mcs51: Option<mcs51::Mc51Core>,
    /// The ARM Cortex-M core, present only when `arch == Arch::Arm`. The
    /// RV32I/8051 fields are unused in that mode.
    pub arm: Option<arm::ArmCore>,
    /// The 80251 core, present only when `arch == Arch::Mcs251`.
    pub x51: Option<x51::X51Core>,
    /// The C166 core, present only when `arch == Arch::C166`.
    pub c166: Option<c166::C166Core>,
    /// The Xtensa LX6 core, present only when `arch == Arch::Xtensa`.
    pub xtensa: Option<xtensa::Lx6Core>,
    /// The AVR core, present only when `arch == Arch::Avr`.
    pub avr: Option<avr::AvrCore>,
    /// The MSP430 core, present only when `arch == Arch::Msp430`.
    pub msp430: Option<msp430::Msp430Core>,
    /// The V850 core, present only when `arch == Arch::V850`.
    pub v850: Option<v850::V850Core>,
    /// The STM8 core, present only when `arch == Arch::Stm8`.
    pub stm8: Option<stm8::Stm8Core>,
    /// The RX core, present only when `arch == Arch::Rx`.
    pub rx: Option<rx::RxCore>,
    /// The RL78 core, present only when `arch == Arch::Rl78`.
    pub rl78: Option<rl78::Rl78Core>,
    /// Cycle-accurate modeling: when enabled, each `step` advances
    /// `cycle_count` by the instruction's estimated latency instead of 1.
    pub cycle_accurate: bool,
    /// Cycles accounted for by the most recently stepped instruction (0 when
    /// cycle-accurate is off). Lets the UI show per-instruction timing.
    pub last_instruction_cycles: u32,
}


/// A symbol (function/global) with its address and size.
#[derive(Debug, Clone)]
pub struct Symbol {
    pub name: String,
    pub address: u32,
    pub size: u32,
}

/// A single source-line mapping: instruction address to file/line.
#[derive(Debug, Clone, Copy)]
pub struct LineEntry {
    pub address: u32,
    pub file: usize,
    pub line: u32,
}

/// A simulated breakpoint with an optional condition expression and hit count.
/// The condition is a C-like expression over register names (`x0`-`x31` for
/// RISC-V, `r0`-`r15` for ARM, `a0`-`a15` for Xtensa, etc.) and the synthetic
/// `pc`/`cycle` values; it is evaluated with the simulator's register view at
/// the moment the PC reaches `address`. A hit-count breakpoint pauses only on
/// the N-th hit (`hit_count`, 1-based).
#[derive(Debug, Clone)]
pub struct SimBreakpoint {
    pub address: u32,
    /// Optional condition expression; empty = unconditional.
    pub condition: String,
    /// 1-based hit count; 0 = stop on first hit.
    pub hit_count: u32,
    /// One-shot breakpoint: automatically removed after the first hit that
    /// pauses the simulator.
    pub once: bool,
    /// Whether the breakpoint is active. Disabled breakpoints are kept in
    /// the set but never pause execution.
    pub enabled: bool,
    /// Number of times the PC has reached this address since set.
    hits: u64,
}

impl SimBreakpoint {
    pub fn new(address: u32) -> Self {
        Self {
            address,
            condition: String::new(),
            hit_count: 0,
            once: false,
            enabled: true,
            hits: 0,
        }
    }

    /// Number of times the PC has reached this address since set.
    pub fn hits(&self) -> u64 {
        self.hits
    }

    /// Set the runtime hit counter (used when restoring a session).
    pub fn set_hits(&mut self, hits: u64) {
        self.hits = hits;
    }

    /// Record a hit at this breakpoint. Returns `true` when the simulator
    /// should pause: the hit count has reached the threshold (or it is
    /// unconditional) and the condition (if any) evaluates true.
    fn record_hit(&mut self, regs: &SimRegs, pc: u32, cycle: u64) -> bool {
        self.hits += 1;
        if self.hit_count > 0 && self.hits < self.hit_count as u64 {
            return false;
        }
        if self.condition.is_empty() {
            return true;
        }
        eval_condition(&self.condition, regs, pc, cycle)
    }
}

/// Register values the condition evaluator can read by name.
#[derive(Debug, Clone, Default)]
pub struct SimRegs {
    /// (name, value) pairs for every visible register, in ABI order.
    pub named: Vec<(String, u32)>,
}

impl SimRegs {
    /// Value of the register named `name` (case-insensitive), if present.
    pub fn get(&self, name: &str) -> Option<u32> {
        let name = name.to_ascii_lowercase();
        self.named
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(&name))
            .map(|(_, v)| *v)
    }
}

/// Evaluate a breakpoint condition expression. Supports:
/// - Integer literals: `42`, `0x2A`, `0b1010`, `0o17`.
/// - Register names (`x0`..`x31`, `r0`..`r15`, `a0`..`a15`, `pc`, `cycle`).
/// - Comparison: `==` `!=` `<` `<=` `>` `>=`.
/// - Arithmetic: `+` `-` `*` `/` `%` (with C precedence).
/// - Logical: `&&` `||` `!` (short-circuit, non-zero = true).
/// - Parentheses and unary minus.
/// An empty condition is true (no condition). Any parse/name error is false
/// (a malformed condition must never silently pass).
pub fn eval_condition(expr: &str, regs: &SimRegs, pc: u32, cycle: u64) -> bool {
    let trimmed = expr.trim();
    if trimmed.is_empty() {
        return true;
    }
    match CondParser::new(trimmed, regs, pc, cycle).parse_or() {

        Some(value) => value != 0,
        None => false,
    }
}

/// Recursive-descent parser for the condition grammar. Returns `None` on any
/// lexing or semantic error so a bad condition is reported as not-matching.
struct CondParser<'a> {
    s: &'a [u8],
    pos: usize,
    regs: &'a SimRegs,
    pc: u32,
    cycle: u64,
}

impl<'a> CondParser<'a> {
    fn new(s: &'a str, regs: &'a SimRegs, pc: u32, cycle: u64) -> Self {
        Self {
            s: s.as_bytes(),
            pos: 0,
            regs,
            pc,
            cycle,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    /// Parse `||` (lowest precedence).
    fn parse_or(&mut self) -> Option<i64> {
        let mut left = self.parse_and()?;
        loop {
            self.skip_ws();
            if self.peek() == Some(b'|') && self.s.get(self.pos + 1) == Some(&b'|') {
                self.pos += 2;
                let right = self.parse_and()?;
                left = (left != 0 || right != 0) as i64;
            } else {
                break;
            }
        }
        Some(left)
    }

    fn parse_and(&mut self) -> Option<i64> {
        let mut left = self.parse_cmp()?;
        loop {
            self.skip_ws();
            if self.peek() == Some(b'&') && self.s.get(self.pos + 1) == Some(&b'&') {
                self.pos += 2;
                let right = self.parse_cmp()?;
                left = (left != 0 && right != 0) as i64;
            } else {
                break;
            }
        }
        Some(left)
    }

    fn parse_cmp(&mut self) -> Option<i64> {
        let left = self.parse_add()?;
        self.skip_ws();
        let op = match (self.peek(), self.s.get(self.pos + 1).copied()) {
            (Some(b'='), Some(b'=')) => {
                self.pos += 2;
                Some("==")
            }
            (Some(b'!'), Some(b'=')) => {
                self.pos += 2;
                Some("!=")
            }
            (Some(b'<'), Some(b'=')) => {
                self.pos += 2;
                Some("<=")
            }
            (Some(b'>'), Some(b'=')) => {
                self.pos += 2;
                Some(">=")
            }
            (Some(b'<'), _) => {
                self.pos += 1;
                Some("<")
            }
            (Some(b'>'), _) => {
                self.pos += 1;
                Some(">")
            }
            _ => None,
        };
        let Some(op) = op else {
            return Some(left);
        };
        let right = self.parse_add()?;
        Some(match op {
            "==" => (left == right) as i64,
            "!=" => (left != right) as i64,
            "<" => (left < right) as i64,
            "<=" => (left <= right) as i64,
            ">" => (left > right) as i64,
            ">=" => (left >= right) as i64,
            _ => 0,
        })
    }

    fn parse_add(&mut self) -> Option<i64> {
        let mut left = self.parse_mul()?;
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b'+') => {
                    self.pos += 1;
                    left = left.wrapping_add(self.parse_mul()?);
                }
                Some(b'-') => {
                    self.pos += 1;
                    left = left.wrapping_sub(self.parse_mul()?);
                }
                _ => break,
            }
        }
        Some(left)
    }

    fn parse_mul(&mut self) -> Option<i64> {
        let mut left = self.parse_unary()?;
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b'*') => {
                    self.pos += 1;
                    left = left.wrapping_mul(self.parse_unary()?);
                }
                Some(b'/') => {
                    self.pos += 1;
                    let right = self.parse_unary()?;
                    left = if right == 0 { 0 } else { left / right };
                }
                Some(b'%') => {
                    self.pos += 1;
                    let right = self.parse_unary()?;
                    left = if right == 0 { 0 } else { left % right };
                }
                _ => break,
            }
        }
        Some(left)
    }

    fn parse_unary(&mut self) -> Option<i64> {
        self.skip_ws();
        match self.peek() {
            Some(b'!') => {
                self.pos += 1;
                let v = self.parse_unary()?;
                Some((v == 0) as i64)
            }
            Some(b'-') => {
                self.pos += 1;
                let v = self.parse_unary()?;
                Some(v.wrapping_neg())
            }
            Some(b'+') => {
                self.pos += 1;
                self.parse_unary()
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Option<i64> {
        self.skip_ws();
        match self.peek() {
            Some(b'(') => {
                self.pos += 1;
                let v = self.parse_or()?;
                self.skip_ws();
                if self.peek() == Some(b')') {
                    self.pos += 1;
                    Some(v)
                } else {
                    None
                }
            }
            Some(c) if c.is_ascii_digit() => self.parse_number(),
            Some(c) if c.is_ascii_alphabetic() || c == b'_' => self.parse_ident(),
            _ => None,
        }
    }

    fn parse_number(&mut self) -> Option<i64> {
        let start = self.pos;
        if self.s.get(self.pos) == Some(&b'0') {
            match self.s.get(self.pos + 1).copied() {
                Some(b'x') | Some(b'X') => {
                    self.pos += 2;
                    let dstart = self.pos;
                    while self.peek().is_some_and(|c| c.is_ascii_hexdigit()) {
                        self.pos += 1;
                    }
                    if self.pos == dstart {
                        return None;
                    }
                    let text = std::str::from_utf8(&self.s[dstart..self.pos]).ok()?;
                    return i64::from_str_radix(text, 16).ok();
                }
                Some(b'b') | Some(b'B') => {
                    self.pos += 2;
                    let dstart = self.pos;
                    while self.peek().is_some_and(|c| c == b'0' || c == b'1') {
                        self.pos += 1;
                    }
                    if self.pos == dstart {
                        return None;
                    }
                    let text = std::str::from_utf8(&self.s[dstart..self.pos]).ok()?;
                    return i64::from_str_radix(text, 2).ok();
                }
                Some(b'o') | Some(b'O') => {
                    self.pos += 2;
                    let dstart = self.pos;
                    while self.peek().is_some_and(|c| matches!(c, b'0'..=b'7')) {
                        self.pos += 1;
                    }
                    if self.pos == dstart {
                        return None;
                    }
                    let text = std::str::from_utf8(&self.s[dstart..self.pos]).ok()?;
                    return i64::from_str_radix(text, 8).ok();
                }
                _ => {}
            }
        }
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.pos += 1;
        }
        let text = std::str::from_utf8(&self.s[start..self.pos]).ok()?;
        text.parse::<i64>().ok()
    }

    fn parse_ident(&mut self) -> Option<i64> {
        let start = self.pos;
        while self.peek().is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_') {
            self.pos += 1;
        }
        let name = std::str::from_utf8(&self.s[start..self.pos]).ok()?;
        let lower = name.to_ascii_lowercase();
        if lower == "pc" {
            Some(self.pc as i64)
        } else if lower == "cycle" || lower == "cycles" {
            Some(self.cycle as i64)
        } else {
            self.regs.get(name).map(|v| v as i64)
        }
    }
}



/// A single-step outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepResult {
    Advanced,
    Breakpoint,
    Halted,
}

impl Simulator {
    pub fn new() -> Self {
        Self {
            pc: ENTRY_POINT,
            registers: [0; REG_COUNT],
            memory: vec![0; MEMORY_SIZE],
            memory_overlay: Vec::new(),
            halted: false,
            breakpoints: Vec::new(),
            cycle_count: 0,
            peripherals: VirtualPeripherals::new(0x4000_0000),
            symbols: Vec::new(),
            line_table: Vec::new(),
            source_files: Vec::new(),
            csr: Csr::default(),
            arch: Arch::RiscV,
            mcs51: None,
            arm: None,
            x51: None,
            c166: None,
            xtensa: None,
            avr: None,
            msp430: None,
            v850: None,
            stm8: None,
            rx: None,
            rl78: None,
            cycle_accurate: false,
            last_instruction_cycles: 0,
        }
    }
    /// Largest program-counter address this architecture can execute at.
    /// 16-bit-PC cores (MCS-51 / C166) and the AVR byte-addressed program
    /// space top out at `0xFFFF`; all others use the full 32-bit range.
    pub fn pc_limit(&self) -> u32 {
        match self.arch {
            Arch::Mcs51 | Arch::C166 | Arch::Avr => 0xFFFF,
            _ => u32::MAX,
        }
    }
    /// Default reset entry point for the architecture (a fresh core starts
    /// here). Loaded firmware may override this with its own entry / base.
    pub fn entry(&self) -> u32 {
        match self.arch {
            Arch::RiscV | Arch::Arm | Arch::Xtensa => 0x8000_0000,
            _ => 0,
        }
    }
    /// Create a simulator for a target architecture. RISC-V, 8051, ARM,
    /// 80251, C166, Xtensa, AVR, MSP430 and V850 are implemented.
    pub fn for_arch(arch: Arch) -> anyhow::Result<Self> {
        let mut sim = Self::new();
        sim.arch = arch;
        match arch {
            Arch::RiscV
            | Arch::Mcs51
            | Arch::Arm
            | Arch::Mcs251
            | Arch::C166
            | Arch::Xtensa
            | Arch::Avr
            | Arch::Msp430
            | Arch::V850
            | Arch::Stm8
            | Arch::Rx
            | Arch::Rl78 => {
                if arch == Arch::Mcs51 {
                    sim.mcs51 = Some(mcs51::Mc51Core::new());
                } else if arch == Arch::Arm {
                    sim.arm = Some(arm::ArmCore::new());
                } else if arch == Arch::Mcs251 {
                    sim.x51 = Some(x51::X51Core::new());
                } else if arch == Arch::C166 {
                    sim.c166 = Some(c166::C166Core::new());
                } else if arch == Arch::Xtensa {
                    sim.xtensa = Some(xtensa::Lx6Core::new());
                } else if arch == Arch::Avr {
                    sim.avr = Some(avr::AvrCore::new());
                } else if arch == Arch::Msp430 {
                    sim.msp430 = Some(msp430::Msp430Core::new());
                } else if arch == Arch::V850 {
                    sim.v850 = Some(v850::V850Core::new());
                } else if arch == Arch::Stm8 {
                    sim.stm8 = Some(stm8::Stm8Core::new());
                } else if arch == Arch::Rx {
                    sim.rx = Some(rx::RxCore::new());
                } else if arch == Arch::Rl78 {
                    sim.rl78 = Some(rl78::Rl78Core::new());
                }
                Ok(sim)
            }
        }
    }
    /// Reset the core (PC to entry, registers zeroed, memory preserved).
    /// For ARM, the vector table at the entry base is used when present.
    pub fn reset(&mut self, entry: u32) {
        if let Some(core) = self.arm.as_mut() {
            core.reset_from_vector_table(entry & !3, entry);
            self.pc = core.regs[arm::PC];
            self.halted = core.halted;
            self.cycle_count = 0;
            return;
        }
        if let Some(core) = self.xtensa.as_mut() {
            core.reset(entry);
            self.pc = core.pc;
            self.halted = core.halted;
            self.cycle_count = 0;
            return;
        }
        if let Some(core) = self.avr.as_mut() {
            core.reset(entry);
            self.pc = core.pc;
            self.halted = core.halted;
            self.cycle_count = 0;
            return;
        }
        if let Some(core) = self.msp430.as_mut() {
            core.reset(entry);
            self.pc = core.pc;
            self.halted = core.halted;
            self.cycle_count = 0;
            return;
        }
        if let Some(core) = self.v850.as_mut() {
            core.reset(entry);
            self.pc = core.pc;
            self.halted = core.halted;
            self.cycle_count = 0;
            return;
        }
        if let Some(core) = self.stm8.as_mut() {
            core.reset(entry);
            self.pc = core.pc;
            self.halted = core.halted;
            self.cycle_count = 0;
            return;
        }
        if let Some(core) = self.rx.as_mut() {
            core.reset(entry);
            self.pc = core.pc;
            self.halted = core.halted;
            self.cycle_count = 0;
            return;
        }
        if let Some(core) = self.rl78.as_mut() {
            core.reset(entry);
            self.pc = core.pc;
            self.halted = core.halted;
            self.cycle_count = 0;
            return;
        }
        self.pc = entry;
        self.registers = [0; REG_COUNT];
        self.halted = false;
        self.cycle_count = 0;
    }

    /// Load a raw program image at `base` (used for `.bin` artifacts).
    pub fn load_image(&mut self, base: u32, image: &[u8]) {
        self.reset(base);
        self.write_bytes(base, image);
    }

    /// Load a decoded program image into the 8051 code space. `base` is the
    /// lowest load address and `image` the contiguous bytes from `ns_flash`'s
    /// Intel HEX decoder. Only valid when `arch == Arch::Mcs51`.
    pub fn load_mcs51(&mut self, base: u16, image: &[u8]) -> anyhow::Result<()> {
        let core = self.mcs51.as_mut().ok_or_else(|| {
            anyhow::anyhow!("8051 内核未创建（目标架构非 8051）")
        })?;
        for (i, byte) in image.iter().enumerate() {
            let addr = (base as usize).wrapping_add(i);
            if addr >= mcs51::ROM_SIZE {
                anyhow::bail!("8051 代码地址 0x{addr:x} 超出 64 KiB 代码空间");
            }
            core.rom[addr] = *byte;
        }
        core.pc = base;
        core.halted = false;
        Ok(())
    }

    /// Load a decoded program image into the 80251 code space. `base` is the
    /// lowest load address. Only valid when `arch == Arch::Mcs251`.
    pub fn load_x51(&mut self, base: u32, image: &[u8]) -> anyhow::Result<()> {
        let core = self.x51.as_mut().ok_or_else(|| {
            anyhow::anyhow!("80251 内核未创建（目标架构非 80251）")
        })?;
        for (i, byte) in image.iter().enumerate() {
            let addr = (base as usize).wrapping_add(i);
            if addr >= x51::ROM_SIZE {
                anyhow::bail!("80251 代码地址 0x{addr:x} 超出代码空间");
            }
            core.rom[addr] = *byte;
        }
        core.pc = base;
        core.halted = false;
        Ok(())
    }

    /// Load a decoded program image into the C166 code space. `base` is the
    /// lowest load address. Only valid when `arch == Arch::C166`.
    pub fn load_c166(&mut self, base: u16, image: &[u8]) -> anyhow::Result<()> {
        let core = self.c166.as_mut().ok_or_else(|| {
            anyhow::anyhow!("C166 内核未创建（目标架构非 C166）")
        })?;
        for (i, byte) in image.iter().enumerate() {
            let addr = (base as usize).wrapping_add(i);
            if addr >= c166::ROM_SIZE {
                anyhow::bail!("C166 代码地址 0x{addr:x} 超出 64 KiB 代码空间");
            }
            core.rom[addr] = *byte;
        }
        core.pc = base;
        core.halted = false;
        Ok(())
    }
    /// Load a program image into the AVR core's program memory. `base` is the
    /// byte address. Only valid when `arch == Arch::Avr`.
    pub fn load_avr(&mut self, base: u32, image: &[u8]) -> anyhow::Result<()> {
        let core = self.avr.as_mut().ok_or_else(|| {
            anyhow::anyhow!("AVR 内核未创建（目标架构非 AVR）")
        })?;
        for (i, byte) in image.iter().enumerate() {
            let addr = base as usize + i;
            if addr >= avr::ROM_SIZE {
                anyhow::bail!("AVR 代码地址 0x{addr:x} 超出 64 KiB 代码空间");
            }
            core.rom[addr] = *byte;
        }
        core.pc = base;
        core.halted = false;
        Ok(())
    }

    /// Load a program image into the MSP430 core (flat 64 KiB memory).
    pub fn load_msp430(&mut self, base: u32, image: &[u8]) -> anyhow::Result<()> {
        let core = self.msp430.as_mut().ok_or_else(|| {
            anyhow::anyhow!("MSP430 内核未创建（目标架构非 MSP430）")
        })?;
        core.write_bytes(base, image);
        core.set_pc(base);
        core.halted = false;
        Ok(())
    }

    /// Load a program image into the V850 core (flat 1 MiB memory).
    pub fn load_v850(&mut self, base: u32, image: &[u8]) -> anyhow::Result<()> {
        let core = self.v850.as_mut().ok_or_else(|| {
            anyhow::anyhow!("V850 内核未创建（目标架构非 V850）")
        })?;
        core.write_bytes(base, image);
        core.set_pc(base);
        core.halted = false;
        Ok(())
    }
    /// Load a program image into the STM8 core (flat 16 MiB address space).
    /// `base` is the 24-bit load address; the core starts at `base`.
    pub fn load_stm8(&mut self, base: u32, image: &[u8]) -> anyhow::Result<()> {
        let core = self.stm8.as_mut().ok_or_else(|| {
            anyhow::anyhow!("STM8 内核未创建（目标架构非 STM8）")
        })?;
        core.load_image(base, image);
        Ok(())
    }
    /// Load a program image into the RX core (flat 64 MiB memory window).
    pub fn load_rx(&mut self, base: u32, image: &[u8]) -> anyhow::Result<()> {
        let core = self.rx.as_mut().ok_or_else(|| {
            anyhow::anyhow!("RX 内核未创建（目标架构非 RX）")
        })?;
        core.load_image(base, image);
        Ok(())
    }

    /// Load a program image into the RL78 core (1 MiB flat memory window).
    pub fn load_rl78(&mut self, base: u32, image: &[u8]) -> anyhow::Result<()> {
        let core = self.rl78.as_mut().ok_or_else(|| {
            anyhow::anyhow!("RL78 内核未创建（目标架构非 RL78）")
        })?;
        core.load_image(base, image);
        Ok(())
    }

    /// Load an ELF32 binary. Returns the entry point.
    pub fn load_elf(&mut self, elf: &[u8]) -> anyhow::Result<u32> {
        if elf.len() < 52 || &elf[0..4] != b"\x7fELF" {
            anyhow::bail!("不是有效的 ELF 文件");
        }
        let class = elf[4];
        let data = elf[5];
        if class != 1 {
            anyhow::bail!("仅支持 ELF32（class=1），当前 class={class}");
        }
        let little = data == 1;
        // Verify the ELF's machine type matches the simulator's architecture,
        // so a Thumb/RISC-V artifact isn't silently decoded with the wrong ISA.
        let machine = read_u16(elf, 18, little);
        let Some(elf_arch) = Arch::from_elf_machine(machine) else {
            anyhow::bail!("不支持的 ELF 机器类型 e_machine={machine}");
        };
        if elf_arch != self.arch {
            anyhow::bail!(
                "ELF 架构 {} 与仿真器架构 {} 不匹配",
                elf_arch.label(),
                self.arch.label()
            );
        }
        let entry = read_u32(elf, 24, little);
        let phoff = read_u32(elf, 28, little);
        let phentsize = read_u16(elf, 42, little) as usize;
        let phnum = read_u16(elf, 44, little) as usize;
        // For ARM Cortex-M, the vector table lives at the lowest PT_LOAD
        // address (the flash base); record it so reset() reads SP/PC from it.
        let mut flash_base: Option<u32> = None;
        for i in 0..phnum {
            let off = phoff as usize + i * phentsize;
            if off + 56 > elf.len() {
                break;
            }
            let p_type = read_u32(elf, off, little);
            if p_type != 1 {
                continue; // PT_LOAD
            }
            let p_offset = read_u32(elf, off + 4, little) as usize;
            let p_vaddr = read_u32(elf, off + 8, little);
            let p_filesz = read_u32(elf, off + 16, little) as usize;
            let p_memsz = read_u32(elf, off + 20, little) as usize;
            if self.arm.is_some() {
                flash_base = Some(flash_base.map_or(p_vaddr, |b| b.min(p_vaddr)));
            }
            let end = p_offset.saturating_add(p_filesz).min(elf.len());
            let data_slice = &elf[p_offset..end];
            self.write_bytes(p_vaddr, data_slice);
            // Zero-fill the .bss portion.
            if p_memsz > p_filesz {
                let bss_start = p_vaddr.wrapping_add(p_filesz as u32);
                self.zero_bytes(bss_start, p_memsz - p_filesz);
            }
        }
        if let Some(core) = self.arm.as_mut() {
            if let Some(base) = flash_base {
                core.vtor = base & !3;
                core.reset_from_vector_table(base & !3, entry);
            } else {
                core.reset_from_vector_table(entry & !3, entry);
            }
            self.pc = core.regs[arm::PC];
            self.halted = core.halted;
        } else {
            self.reset(entry);
        }
        self.parse_symbols(elf, little);
        Ok(entry)
    }

    /// Parse ELF `.symtab`/`.strtab` sections into [`Symbol`]s and a coarse
    /// line table (function start → sequential line numbers). The line table is
    /// a best-effort fallback when DWARF line info is unavailable.
    fn parse_symbols(&mut self, elf: &[u8], little: bool) {
        if elf.len() < 52 {
            return;
        }
        let shoff = read_u32(elf, 32, little) as usize;
        let shentsize = read_u16(elf, 46, little) as usize;
        let shnum = read_u16(elf, 48, little) as usize;
        let shstrndx = read_u16(elf, 50, little) as usize;
        if shoff == 0 || shentsize == 0 {
            return;
        }
        let section_name = |index: usize| -> Option<&str> {
            let off = shoff + index * shentsize;
            if off + 40 > elf.len() {
                return None;
            }
            let name_off = read_u32(elf, off, little) as usize;
            let str_off = shoff + shstrndx * shentsize;
            if str_off + 40 > elf.len() {
                return None;
            }
            let str_off_data = read_u32(elf, str_off + 16, little) as usize;
            let str_size = read_u32(elf, str_off + 20, little) as usize;
            let start = str_off_data.saturating_add(name_off);
            if start >= elf.len() || start >= str_off_data.saturating_add(str_size) {
                return None;
            }
            let end = elf[start..]
                .iter()
                .position(|b| *b == 0)
                .map(|p| start + p)
                .unwrap_or(start);
            std::str::from_utf8(&elf[start..end]).ok()
        };

        let mut symtab: Option<(usize, usize, usize)> = None; // offset, entsize, count
        let mut strtab: Option<(usize, usize)> = None; // offset, size
        for index in 0..shnum {
            let off = shoff + index * shentsize;
            if off + 40 > elf.len() {
                break;
            }
            let name = section_name(index).unwrap_or("");
            let sh_offset = read_u32(elf, off + 16, little) as usize;
            let sh_size = read_u32(elf, off + 20, little) as usize;
            let sh_entsize = read_u32(elf, off + 36, little) as usize;
            match name {
                ".symtab" => symtab = Some((sh_offset, sh_entsize, sh_size / sh_entsize.max(1))),
                ".strtab" => strtab = Some((sh_offset, sh_size)),
                _ => {}
            }
        }
        let Some((sym_off, sym_entsize, sym_count)) = symtab else {
            return;
        };
        let Some((str_off, str_size)) = strtab else {
            return;
        };
        let read_str = |offset: usize| -> String {
            let start = str_off.saturating_add(offset);
            if start >= str_off.saturating_add(str_size) || start >= elf.len() {
                return String::new();
            }
            let end = elf[start..]
                .iter()
                .position(|b| *b == 0)
                .map(|p| start + p)
                .unwrap_or(start);
            String::from_utf8_lossy(&elf[start..end]).into_owned()
        };

        let mut symbols = Vec::new();
        for index in 0..sym_count {
            let off = sym_off + index * sym_entsize.max(16);
            if off + 16 > elf.len() {
                break;
            }
            let name_off = read_u32(elf, off, little) as usize;
            let value = read_u32(elf, off + 4, little);
            let size = read_u32(elf, off + 8, little);
            let info = elf[off + 12];
            let kind = info & 0x0f;
            if kind == 2 && size > 0 {
                // STT_FUNC
                let name = read_str(name_off);
                if !name.is_empty() {
                    symbols.push(Symbol {
                        name,
                        address: value,
                        size,
                    });
                }
            }
        }
        symbols.sort_by_key(|symbol| symbol.address);
        // Build a coarse line table: each function start maps to an incrementing
        // line within the file. This is a fallback; DWARF would be more precise.
        let mut line_table = Vec::new();
        for (i, symbol) in symbols.iter().enumerate() {
            let line = (i as u32) + 1;
            line_table.push(LineEntry {
                address: symbol.address,
                file: 0,
                line,
            });
        }
        self.source_files = vec!["<elf>".to_string()];
        self.symbols = symbols;
        self.line_table = line_table;
    }

    /// Set a breakpoint at `address`. Returns whether a new one was added.
    /// An existing breakpoint at the address is left untouched (use
    /// [`Self::update_breakpoint`] to change its condition / hit count).
    pub fn set_breakpoint(&mut self, address: u32) -> bool {
        if self.breakpoints.iter().any(|bp| bp.address == address) {
            false
        } else {
            self.breakpoints.push(SimBreakpoint::new(address));
            true
        }
    }

    /// Set a breakpoint at `address` with a condition and hit count.
    pub fn set_breakpoint_ex(
        &mut self,
        address: u32,
        condition: impl Into<String>,
        hit_count: u32,
    ) -> bool {
        if let Some(bp) = self.breakpoints.iter_mut().find(|bp| bp.address == address) {
            bp.condition = condition.into();
            bp.hit_count = hit_count;
            false
        } else {
            let mut bp = SimBreakpoint::new(address);
            bp.condition = condition.into();
            bp.hit_count = hit_count;
            self.breakpoints.push(bp);
            true
        }
    }

    /// Set a one-shot breakpoint at `address`: it pauses on the first hit
    /// (subject to the optional condition / hit count) and is then removed
    /// automatically, so a subsequent `run`/`continue` will not stop there
    /// again.
    pub fn set_breakpoint_ex_once(
        &mut self,
        address: u32,
        condition: impl Into<String>,
        hit_count: u32,
    ) -> bool {
        if let Some(bp) = self.breakpoints.iter_mut().find(|bp| bp.address == address) {
            bp.condition = condition.into();
            bp.hit_count = hit_count;
            bp.once = true;
            false
        } else {
            let mut bp = SimBreakpoint::new(address);
            bp.condition = condition.into();
            bp.hit_count = hit_count;
            bp.once = true;
            self.breakpoints.push(bp);
            true
        }
    }

    /// Update an existing breakpoint's condition / hit count in place.
    pub fn update_breakpoint(
        &mut self,
        address: u32,
        condition: impl Into<String>,
        hit_count: u32,
    ) -> bool {
        if let Some(bp) = self.breakpoints.iter_mut().find(|bp| bp.address == address) {
            bp.condition = condition.into();
            bp.hit_count = hit_count;
            true
        } else {
            false
        }
    }

    pub fn clear_breakpoint(&mut self, address: u32) -> bool {
        let before = self.breakpoints.len();
        self.breakpoints.retain(|bp| bp.address != address);
        before != self.breakpoints.len()
    }

    /// The current hit count for the breakpoint at `address` (0 if none).
    pub fn breakpoint_hits(&self, address: u32) -> u64 {
        self.breakpoints
            .iter()
            .find(|bp| bp.address == address)
            .map(|bp| bp.hits)
            .unwrap_or(0)
    }

    /// Restore the runtime hit counter for a breakpoint (session restore).
    pub fn set_breakpoint_hits(&mut self, address: u32, hits: u64) {
        if let Some(bp) = self.breakpoints.iter_mut().find(|bp| bp.address == address) {
            bp.hits = hits;
        }
    }

    /// Addresses of enabled (active) breakpoints, in set order. Disabled
    /// breakpoints are not passed to the core and never pause execution.
    pub fn enabled_breakpoint_addresses(&self) -> Vec<u32> {
        self.breakpoints
            .iter()
            .filter(|bp| bp.enabled)
            .map(|bp| bp.address)
            .collect()
    }

    /// Enable or disable a breakpoint without removing it from the set.
    /// Returns whether the breakpoint existed.
    pub fn set_breakpoint_enabled(&mut self, address: u32, enabled: bool) -> bool {
        if let Some(bp) = self.breakpoints.iter_mut().find(|bp| bp.address == address) {
            bp.enabled = enabled;
            true
        } else {
            false
        }
    }

    /// Whether the breakpoint at `address` is enabled (true if it exists and
    /// is enabled; false when absent).
    pub fn breakpoint_is_enabled(&self, address: u32) -> bool {
        self.breakpoints
            .iter()
            .find(|bp| bp.address == address)
            .map(|bp| bp.enabled)
            .unwrap_or(false)
    }

    /// Whether the breakpoint at `address` is one-shot (auto-cleared on hit).
    pub fn breakpoint_is_once(&self, address: u32) -> bool {
        self.breakpoints
            .iter()
            .find(|bp| bp.address == address)
            .map(|bp| bp.once)
            .unwrap_or(false)
    }

    /// Remove one-shot breakpoints currently sitting at the program counter.
    /// Called after a `Breakpoint` step result so a one-shot breakpoint is
    /// consumed exactly once and disappears from the breakpoint set.
    fn clear_once_breakpoints_at_pc(&mut self) {
        let pc = self.program_counter();
        self.breakpoints.retain(|bp| !(bp.once && bp.address == pc));
    }

    /// Build the register-name view used by condition evaluation, from the
    /// architecture's register labels and values.
    pub fn reg_view(&self) -> SimRegs {
        let mut named = Vec::new();
        let count = reg_count_for(self.arch);
        for i in 0..count {
            let label = reg_label_for(self.arch, i);
            let value = self.read_reg(i);
            named.push((label.clone(), value));
            // Numeric aliases so conditions can use `x1` (RISC-V), `r1` (ARM),
            // `a1` (Xtensa) alongside the ABI names.
            let alias = match self.arch {
                Arch::RiscV => format!("x{i}"),
                Arch::Arm | Arch::C166 | Arch::Rx => format!("r{i}"),
                _ => String::new(),
            };
            if !alias.is_empty() {
                named.push((alias, value));
            }
        }
        SimRegs { named }
    }




    /// Find the symbol containing `address` (function name + start), if any.
    pub fn symbol_at(&self, address: u32) -> Option<&Symbol> {
        self.symbols
            .iter()
            .find(|symbol| address >= symbol.address && address < symbol.address + symbol.size)
    }

    /// Map a source line (1-based within a file index) to its first address.
    pub fn address_for_line(&self, file: usize, line: u32) -> Option<u32> {
        self.line_table
            .iter()
            .find(|entry| entry.file == file && entry.line == line)
            .map(|entry| entry.address)
    }

    /// Map an address to its best-guess source line (file index, 1-based line).
    pub fn line_for_address(&self, address: u32) -> Option<(usize, u32)> {
        self.line_table
            .iter()
            .filter(|entry| entry.address <= address)
            .max_by_key(|entry| entry.address)
            .map(|entry| (entry.file, entry.line))
    }

    /// Estimate the clock cycles the instruction at `pc` takes, for
    /// cycle-accurate modeling. Returns 1 for the common case and a higher
    /// value for multi-cycle ops (loads/stores, branches, multiplies, etc.).
    /// This is a conservative pipeline model, not a cycle-exact datasheet
    /// count; it exists so cycle-accurate simulation shows realistic timing
    /// rather than a flat 1-cycle-per-instruction assumption.
    pub fn cycles_for_instruction(&self, pc: u32) -> u32 {
        match self.arch {
            Arch::RiscV => {
                let insn = self.read_u32(pc);
                let opcode = insn & 0x7F;
                match opcode {
                    // Loads and stores: 2 cycles (address + data phase).
                    0x03 | 0x23 => 2,
                    // JAL/JALR and taken branches: 2 cycles.
                    0x6F | 0x67 | 0x63 => 2,
                    // M extension multiply/divide: 4 cycles.
                    0x33 => {
                        let funct3 = (insn >> 12) & 0x7;
                        let funct7 = insn >> 25;
                        if funct7 == 0x01 || (funct3 == 0x04 || funct3 == 0x05) && funct7 == 0x00 {
                            4
                        } else {
                            1
                        }
                    }
                    _ => 1,
                }
            }
            Arch::Arm => {
                let half = self.read_u8(pc) as u16 | (self.read_u8(pc + 1) as u16) << 8;
                match half & 0xF800 {
                    // LDR/STR (register/imm offset): 2 cycles.
                    0x6000 | 0x6800 | 0x7000 | 0x7800 => 2,
                    // PUSH/POP/STM/LDM: 2 + N (one per register, approx).
                    0xB400 | 0xBC00 | 0xC000 | 0xC800 => 2,
                    // B/BL: 2 cycles.
                    0xE000 => 2,
                    _ => 1,
                }
            }
            // Most 8/16-bit CISC cores use 1 cycle per instruction in the
            // model; multi-byte or memory ops are approximated as 1.
            _ => 1,
        }
    }


    /// Execute one instruction. Returns what happened.
    /// Execute one instruction (or stop at a breakpoint / halt). One-shot
    /// breakpoints are consumed automatically: once a `Breakpoint` result is
    /// returned for a one-shot address, the breakpoint is removed so a later
    /// step will not stop there again.
    pub fn step(&mut self) -> StepResult {
        let result = self.step_inner();
        if result == StepResult::Breakpoint {
            self.clear_once_breakpoints_at_pc();
        }
        result
    }

    fn step_inner(&mut self) -> StepResult {
        let pc_before = self.program_counter();
        let est = self.cycles_for_instruction(pc_before);
        if let Some(core) = self.mcs51.as_mut() {
            let bps: Vec<u16> = self
                .breakpoints
                .iter()
                .map(|bp| (bp.address & 0xFFFF) as u16)
                .collect();

            let result = core.step(&bps);
            self.last_instruction_cycles = if self.cycle_accurate { est.max(1) } else { 1 };
            self.cycle_count += self.last_instruction_cycles as u64;
            let result = match result {
                mcs51::Mc51Step::Advanced => StepResult::Advanced,
                mcs51::Mc51Step::Breakpoint => StepResult::Breakpoint,
                mcs51::Mc51Step::Halted => StepResult::Halted,
            };
            self.tick_peripherals_for(self.last_instruction_cycles as u64);
            return result;
        }
         if let Some(core) = self.arm.as_mut() {
             let bps: Vec<u32> = self.breakpoints.iter().filter(|bp| bp.enabled).map(|bp| bp.address).collect();

             let result = core.step(&bps);
             self.pc = core.regs[arm::PC];
             self.last_instruction_cycles = if self.cycle_accurate { est.max(1) } else { 1 };
             self.cycle_count += self.last_instruction_cycles as u64;
             self.halted = core.halted;
            let store_events = std::mem::take(&mut core.store_events);
            let result = match result {
                arm::ArmStep::Advanced => StepResult::Advanced,
                arm::ArmStep::Breakpoint => StepResult::Breakpoint,
                arm::ArmStep::Halted => StepResult::Halted,
            };
            for (address, byte) in store_events {
                self.peripheral_write(address, byte);
                // Record runtime data-space writes so they survive session
                // reloads (the ARM core keeps its own memory image, so it is
                // not mirrored into `self.memory`; only this overlay is
                // replayed when a session is restored).
                if let Some(entry) = self.memory_overlay.iter_mut().find(|(a, _)| *a == address) {
                    entry.1 = byte;
                } else {
                    self.memory_overlay.push((address, byte));
                }
            }
            self.tick_peripherals_for(self.last_instruction_cycles as u64);
            return result;
        }
        if let Some(core) = self.x51.as_mut() {
            let bps: Vec<u32> = self.breakpoints.iter().filter(|bp| bp.enabled).map(|bp| bp.address).collect();
            let result = core.step(&bps);
            self.pc = core.pc;
            self.last_instruction_cycles = if self.cycle_accurate { est.max(1) } else { 1 };
            self.cycle_count += self.last_instruction_cycles as u64;
            self.halted = core.halted;
            let result = match result {
                x51::X51Step::Advanced => StepResult::Advanced,
                x51::X51Step::Breakpoint => StepResult::Breakpoint,
                x51::X51Step::Halted => StepResult::Halted,
            };
            self.tick_peripherals_for(self.last_instruction_cycles as u64);
            return result;
        }
        if let Some(core) = self.c166.as_mut() {
            let bps: Vec<u16> = self
                .breakpoints
                .iter()
                .map(|bp| (bp.address & 0xFFFF) as u16)
                .collect();

            let result = core.step(&bps);
            self.pc = core.pc as u32;
            self.last_instruction_cycles = if self.cycle_accurate { est.max(1) } else { 1 };
            self.cycle_count += self.last_instruction_cycles as u64;
            self.halted = core.halted;
            let result = match result {
                c166::C166Step::Advanced => StepResult::Advanced,
                c166::C166Step::Breakpoint => StepResult::Breakpoint,
                c166::C166Step::Halted => StepResult::Halted,
            };
            self.tick_peripherals_for(self.last_instruction_cycles as u64);
            return result;
        }
        if let Some(core) = self.xtensa.as_mut() {
            let bps: Vec<u32> = self.breakpoints.iter().filter(|bp| bp.enabled).map(|bp| bp.address).collect();
            let result = core.step(&bps);
            self.pc = core.pc;
            self.last_instruction_cycles = if self.cycle_accurate { est.max(1) } else { 1 };
            self.cycle_count += self.last_instruction_cycles as u64;
            self.halted = core.halted;
            let result = match result {
                xtensa::Lx6Step::Advanced => StepResult::Advanced,
                xtensa::Lx6Step::Breakpoint => StepResult::Breakpoint,
                xtensa::Lx6Step::Halted => StepResult::Halted,
            };
            self.tick_peripherals_for(self.last_instruction_cycles as u64);
            return result;
        }
        if let Some(core) = self.avr.as_mut() {
            let bps: Vec<u32> = self.breakpoints.iter().filter(|bp| bp.enabled).map(|bp| bp.address).collect();
            let result = core.step(&bps);
            self.pc = core.pc;
            self.last_instruction_cycles = if self.cycle_accurate { est.max(1) } else { 1 };
            self.cycle_count += self.last_instruction_cycles as u64;
            self.halted = core.halted;
            let result = match result {
                avr::AvrStep::Advanced => StepResult::Advanced,
                avr::AvrStep::Breakpoint => StepResult::Breakpoint,
                avr::AvrStep::Halted => StepResult::Halted,
            };
            self.tick_peripherals_for(self.last_instruction_cycles as u64);
            return result;
        }
        if let Some(core) = self.msp430.as_mut() {
            let bps: Vec<u32> = self.breakpoints.iter().filter(|bp| bp.enabled).map(|bp| bp.address).collect();
            let result = core.step(&bps);
            self.pc = core.pc;
            self.last_instruction_cycles = if self.cycle_accurate { est.max(1) } else { 1 };
            self.cycle_count += self.last_instruction_cycles as u64;
            self.halted = core.halted;
            let result = match result {
                msp430::Msp430Step::Advanced => StepResult::Advanced,
                msp430::Msp430Step::Breakpoint => StepResult::Breakpoint,
                msp430::Msp430Step::Halted => StepResult::Halted,
            };
            self.tick_peripherals_for(self.last_instruction_cycles as u64);
            return result;
        }
        if let Some(core) = self.v850.as_mut() {
            let bps: Vec<u32> = self.breakpoints.iter().filter(|bp| bp.enabled).map(|bp| bp.address).collect();
            let result = core.step(&bps);
            self.pc = core.pc;
            self.last_instruction_cycles = if self.cycle_accurate { est.max(1) } else { 1 };
            self.cycle_count += self.last_instruction_cycles as u64;
            self.halted = core.halted;
            let result = match result {
                v850::V850Step::Advanced => StepResult::Advanced,
                v850::V850Step::Breakpoint => StepResult::Breakpoint,
                v850::V850Step::Halted => StepResult::Halted,
            };
            self.tick_peripherals_for(self.last_instruction_cycles as u64);
            return result;
        }
        if let Some(core) = self.stm8.as_mut() {
            let bps: Vec<u32> = self.breakpoints.iter().filter(|bp| bp.enabled).map(|bp| bp.address).collect();
            let result = core.step(&bps);
            self.pc = core.pc;
            self.last_instruction_cycles = if self.cycle_accurate { est.max(1) } else { 1 };
            self.cycle_count += self.last_instruction_cycles as u64;
            self.halted = core.halted;
            let result = match result {
                stm8::Stm8Step::Advanced => StepResult::Advanced,
                stm8::Stm8Step::Breakpoint => StepResult::Breakpoint,
                stm8::Stm8Step::Halted => StepResult::Halted,
            };
            self.tick_peripherals_for(self.last_instruction_cycles as u64);
            return result;
        }
        if let Some(core) = self.rx.as_mut() {
            let bps: Vec<u32> = self.breakpoints.iter().filter(|bp| bp.enabled).map(|bp| bp.address).collect();
            let result = core.step(&bps);
            self.pc = core.pc;
            self.last_instruction_cycles = if self.cycle_accurate { est.max(1) } else { 1 };
            self.cycle_count += self.last_instruction_cycles as u64;
            self.halted = core.halted;
            let result = match result {
                rx::RxStep::Advanced => StepResult::Advanced,
                rx::RxStep::Breakpoint => StepResult::Breakpoint,
                rx::RxStep::Halted => StepResult::Halted,
            };
            self.tick_peripherals_for(self.last_instruction_cycles as u64);
            return result;
        }
        if let Some(core) = self.rl78.as_mut() {
            let bps: Vec<u32> = self.breakpoints.iter().filter(|bp| bp.enabled).map(|bp| bp.address).collect();
            let result = core.step(&bps);
            self.pc = core.pc;
            self.last_instruction_cycles = if self.cycle_accurate { est.max(1) } else { 1 };
            self.cycle_count += self.last_instruction_cycles as u64;
            self.halted = core.halted;
            return match result {
                rl78::Rl78Step::Advanced => StepResult::Advanced,
                rl78::Rl78Step::Breakpoint => StepResult::Breakpoint,
                rl78::Rl78Step::Halted => StepResult::Halted,
            };
        }
        if self.halted {
            return StepResult::Halted;
        }
        if let Some(index) = self
            .breakpoints
            .iter()
            .filter(|bp| bp.enabled)
            .position(|bp| bp.address == self.pc)
        {
            let regs = self.reg_view();
            let should_stop = self.breakpoints[index]
                .record_hit(&regs, self.pc, self.cycle_count);
            if should_stop {
                return StepResult::Breakpoint;
            }
        }

        // Take a pending, enabled interrupt before the next instruction: this
        // mirrors real hardware, which traps at an instruction boundary. The
        // vector entry then runs the ISR via the trap vector (mtvec).
        if self.take_interrupt() {
            self.last_instruction_cycles = if self.cycle_accurate { est.max(1) } else { 1 };
            self.cycle_count += self.last_instruction_cycles as u64;
            return StepResult::Advanced;
        }
        let insn = self.read_u32(self.pc);
        self.execute(insn);
            self.last_instruction_cycles = if self.cycle_accurate { est.max(1) } else { 1 };
            self.cycle_count += self.last_instruction_cycles as u64;
        self.tick_peripherals_for(self.last_instruction_cycles as u64);
        if self.halted {
            StepResult::Halted
        } else {
            StepResult::Advanced
        }
    }

    /// The current program counter, unified across architectures (8051 is a
    /// 16-bit PC; the RV32I core is 32-bit).
    pub fn program_counter(&self) -> u32 {
        if let Some(core) = &self.mcs51 {
            core.pc as u32
        } else if let Some(core) = &self.x51 {
            core.pc
        } else if let Some(core) = &self.c166 {
            core.pc as u32
        } else if let Some(core) = &self.avr {
            core.pc
        } else if let Some(core) = &self.msp430 {
            core.pc
        } else if let Some(core) = &self.v850 {
            core.pc
        } else if let Some(core) = &self.stm8 {
            core.pc
        } else if let Some(core) = &self.rx {
            core.pc
        } else if let Some(core) = &self.rl78 {
            core.pc
        } else {
            self.pc
        }
    }

    /// Whether the ARM-family core is in ARM (A32) state. Only meaningful when
    /// `arch == Arch::Arm`; returns `false` for Cortex-M (Thumb) and other
    /// architectures.
    pub fn arm_state(&self) -> bool {
        self.arm.as_ref().map(|core| core.arm_state).unwrap_or(false)
    }

    /// Set the ARM-family core instruction-set state (A32 vs Thumb). Only
    /// meaningful when `arch == Arch::Arm`; ignored otherwise.
    pub fn set_arm_state(&mut self, arm: bool) {
        if let Some(core) = self.arm.as_mut() {
            core.set_arm_state(arm);
        }
    }

    /// Read the ARM xPSR (condition flags, Thumb bit, exception number), or
    /// `None` when the core is not ARM.
    pub fn arm_xpsr(&self) -> Option<u32> {
        self.arm.as_ref().map(|core| core.xpsr)
    }

    /// Write the ARM xPSR directly (condition flags, Thumb bit, exception
    /// number). Only meaningful when `arch == Arch::Arm`; ignored otherwise.
    pub fn set_arm_xpsr(&mut self, value: u32) {
        if let Some(core) = self.arm.as_mut() {
            core.xpsr = value;
        }
    }

    /// Read the ARM If-Then block state as `(mask, cond, index)`, or `None`
    /// when the core is not ARM. A zero mask means no IT block is active.
    pub fn arm_it_state(&self) -> Option<(u8, u32, u8)> {
        self.arm
            .as_ref()
            .map(|core| (core.it_mask, core.it_cond, core.it_index))
    }

    /// Write the ARM If-Then block state directly. Only meaningful when
    /// `arch == Arch::Arm`; ignored otherwise.
    pub fn set_arm_it_state(&mut self, mask: u8, cond: u32, index: u8) {
        if let Some(core) = self.arm.as_mut() {
            core.it_mask = mask;
            core.it_cond = cond;
            core.it_index = index;
        }
    }

    /// Set the program counter, routing to the active architecture core so the
    /// PC stays consistent (ARM/Xtensa keep their own PC register).
    pub fn set_program_counter(&mut self, pc: u32) {
        self.pc = pc;
        if let Some(core) = &mut self.mcs51 {
            core.pc = pc as u16;
        } else if let Some(core) = &mut self.x51 {
            core.pc = pc;
        } else if let Some(core) = &mut self.c166 {
            core.pc = pc as u16;
        } else if let Some(core) = &mut self.arm {
            core.set_pc(pc);
        } else if let Some(core) = &mut self.xtensa {
            core.set_pc(pc);
        } else if let Some(core) = &mut self.avr {
            core.set_pc(pc);
        } else if let Some(core) = &mut self.msp430 {
            core.set_pc(pc);
        } else if let Some(core) = &mut self.v850 {
            core.set_pc(pc);
        } else if let Some(core) = &mut self.stm8 {
            core.set_pc(pc);
        } else if let Some(core) = &mut self.rx {
            core.set_pc(pc);
        } else if let Some(core) = &mut self.rl78 {
            core.set_pc(pc);
        }
    }

    /// Advance cycle-based peripherals (GPIO sampling, timers) by one core
    /// cycle. Equivalent to [`Self::tick_peripherals_for`] with 1.
    pub fn tick_peripherals(&mut self) {
        self.tick_peripherals_for(1);
    }

    /// Advance cycle-based peripherals by `cycles` core cycles. In
    /// cycle-accurate mode each instruction advances `cycle_count` by its
    /// estimated latency, so peripherals must be ticked by that same amount or
    /// timers run slow relative to the CPU clock.
    pub fn tick_peripherals_for(&mut self, cycles: u64) {
        for timer in self.peripherals.timers.iter_mut() {
            let mut overflow = false;
            for _ in 0..cycles.max(1) {
                if timer.tick() > 0 {
                    overflow = true;
                }
            }
            if overflow && timer.irq_line < self.peripherals.irq_pending.len() {
                self.peripherals.irq_pending[timer.irq_line] = true;
            }
        }
        // Advance the watchdog. When it overflows, record a reset event (the
        // firmware failed to feed it in time); the IDE surfaces `reset_count`
        // so runaway firmware is visible instead of silently stalling.
        if let Some(watchdog) = self.peripherals.watchdog.as_mut() {
            if watchdog.enabled && watchdog.timeout_cycles > 0 {
                watchdog.elapsed = watchdog.elapsed.saturating_add(cycles.max(1));
                if watchdog.elapsed >= watchdog.timeout_cycles {
                    watchdog.reset_count = watchdog.reset_count.saturating_add(1);
                    watchdog.last_reset_pc = self.pc;
                    watchdog.elapsed = 0;
                }
            }
        }
    }

    /// Run until a breakpoint, halt, or `max_steps` instructions.
    pub fn run(&mut self, max_steps: u64) -> StepResult {
        let mut steps = 0u64;
        while steps < max_steps {
            let result = self.step();
            if result != StepResult::Advanced {
                return result;
            }
            steps += 1;
        }
        StepResult::Advanced
    }

    /// Single-step over a call/loop by running until PC returns to the line
    /// after the current one, or until a breakpoint/halt.
    pub fn step_over(&mut self, max_steps: u64) -> StepResult {
        let pc = self.program_counter();
        let arch = self.arch;
        let read = |addr: u32| self.read_code(addr);
        let insn = disassemble_one(arch, pc, &read);
        // A call runs to the caller's next instruction; anything else is a
        // single step. The return address is `pc + instruction_size`.
        if !is_call_mnemonic(&insn) {
            return self.step();
        }
        let width = instruction_size(arch, pc, &read, self.arm_state());
        let return_pc = pc.wrapping_add(width);
        let mut steps = 0u64;
        while steps < max_steps {
            let result = self.step();
            if result != StepResult::Advanced {
                return result;
            }
            if self.program_counter() == return_pc {
                return StepResult::Advanced;
            }
            steps += 1;
        }
        StepResult::Advanced
    }

    /// Single-step out of the current function by running until a return
    /// instruction is executed (the caller's next instruction), or until a
    /// breakpoint/halt. Architecture-independent: it inspects the decoded
    /// mnemonic at the current PC for a return class (`ret`/`reti`/`rts`/
    /// `rti`/`rfe`/`rte`/`retf`) before stepping. A function that never
    /// returns runs to `max_steps`.
    pub fn step_out(&mut self, max_steps: u64) -> StepResult {
        let mut steps = 0u64;
        while steps < max_steps {
            let pc = self.program_counter();
            let arch = self.arch;
            let read = |addr: u32| self.read_code(addr);
            let insn = disassemble_one(arch, pc, &read);
            if is_return_mnemonic(&insn) {
                // Execute the return so the PC lands on the caller's next
                // instruction; stop there.
                return self.step();
            }
            let result = self.step();
            if result != StepResult::Advanced {
                return result;
            }
            steps += 1;
        }
        StepResult::Advanced
    }


    pub fn read_reg(&self, index: usize) -> u32 {
        if let Some(core) = &self.arm {
            return core.regs[index.min(15)];
        }
        if let Some(core) = &self.mcs51 {
            return match index {
                0 => core.acc as u32,
                1 => core.b as u32,
                2 => core.psw as u32,
                3 => core.sp as u32,
                4 => core.dptr as u32,
                // R0-R7 of the active register bank (RS1:RS0).
                n if n >= 5 && n < 13 => {
                    let bank = ((core.psw >> 3) & 0x03) as usize;
                    core.idram[bank * 8 + (n - 5)] as u32
                }
                _ => 0,
            };
        }
        if let Some(core) = &self.x51 {
            return match index {
                0 => core.acc as u32,
                1 => core.b as u32,
                2 => core.psw as u32,
                3 => core.sp as u32,
                4 => core.dptr as u32,
                n if n >= 5 && n < 21 => core.regs[n - 5] as u32,
                _ => 0,
            };
        }
        if let Some(core) = &self.c166 {
            return core.regs[index.min(15)] as u32;
        }
        if let Some(core) = &self.avr {
            return core.regs[index.min(31)] as u32;
        }
        if let Some(core) = &self.msp430 {
            return core.read_reg(index);
        }
        if let Some(core) = &self.v850 {
            return core.read_reg(index);
        }
        if let Some(core) = &self.stm8 {
            return match index {
                0 => core.a as u32,
                1 => core.x as u32,
                2 => core.y as u32,
                3 => core.sp as u32,
                4 => core.cc as u32,
                _ => 0,
            };
        }
        if let Some(core) = &self.rx {
            return core.read_reg(index);
        }
        if let Some(core) = &self.rl78 {
            return core.read_reg(index);
        }
        if let Some(core) = &self.xtensa {
            return core.reg(index & 15);
        }
        self.registers[index.min(REG_COUNT - 1)]
    }

    /// Write a register value, routing to the active architecture core (the
    /// inverse of `read_reg`). x0 (RISC-V) is read-only and ignored.
    pub fn write_reg(&mut self, index: usize, value: u32) {
        if let Some(core) = &mut self.arm {
            core.regs[index.min(15)] = value;
            return;
        }
        if let Some(core) = &mut self.mcs51 {
            match index {
                0 => core.acc = value as u8,
                1 => core.b = value as u8,
                2 => core.psw = value as u8,
                3 => core.sp = value as u8,
                4 => core.dptr = value as u16,
                n if n >= 5 && n < 13 => {
                    let bank = ((core.psw >> 3) & 0x03) as usize;
                    core.idram[bank * 8 + (n - 5)] = value as u8;
                }
                _ => {}
            }
            return;
        }
        if let Some(core) = &mut self.x51 {
            match index {
                0 => core.acc = value as u16,
                1 => core.b = value as u8,
                2 => core.psw = value as u8,
                3 => core.sp = value as u8,
                4 => core.dptr = value,
                n if n >= 5 && n < 21 => core.regs[n - 5] = value as u16,
                _ => {}
            }
            return;
        }
        if let Some(core) = &mut self.c166 {
            core.regs[index.min(15)] = value as u16;
            return;
        }
        if let Some(core) = &mut self.avr {
            core.regs[index.min(31)] = value as u8;
            return;
        }
        if let Some(core) = &mut self.msp430 {
            core.write_reg(index, value);
            return;
        }
        if let Some(core) = &mut self.v850 {
            core.write_reg(index, value);
            return;
        }
        if let Some(core) = &mut self.stm8 {
            match index {
                0 => core.a = value as u8,
                1 => core.x = value as u16,
                2 => core.y = value as u16,
                3 => core.sp = value as u16,
                4 => core.cc = value as u8,
                _ => {}
            }
            return;
        }
        if let Some(core) = &mut self.rx {
            core.write_reg(index, value);
            return;
        }
        if let Some(core) = &mut self.rl78 {
            core.write_reg(index, value);
            return;
        }
        if let Some(core) = &mut self.xtensa {
            core.set_reg(index & 15, value);
            return;
        }
        if index != 0 && index < REG_COUNT {
            self.registers[index] = value;
        }
    }

    pub fn read_u32(&self, address: u32) -> u32 {
        if let Some(core) = &self.arm {
            return core.read_u32(address);
        }
        if let Some(core) = &self.xtensa {
            return core.read_u32(address);
        }
        if let Some(core) = &self.avr {
            let mut value = 0u32;
            for i in 0..4 {
                value |= (self.read_u8(address.wrapping_add(i as u32)) as u32) << (8 * i);
            }
            let _ = core;
            return value;
        }
        if let Some(core) = &self.msp430 {
            return core.read_u32(address);
        }
        if let Some(core) = &self.v850 {
            return core.read_u32(address);
        }
        if let Some(core) = &self.stm8 {
            return core.read_mem_u32(address);
        }
        if let Some(core) = &self.rx {
            return core.read_u32(address);
        }
        if let Some(core) = &self.rl78 {
            return core.read_u32(address);
        }
        let base = (address as usize) & !3;
        let mut value = 0u32;
        for i in 0..4 {
            value |= (self.memory[base.wrapping_add(i) % MEMORY_SIZE] as u32) << (8 * i);
        }
        value
    }

    pub fn read_u8(&self, address: u32) -> u8 {
        if let Some(core) = &self.arm {
            return core.read_u8(address);
        }
        if let Some(core) = &self.xtensa {
            return core.read_u8(address);
        }
        if let Some(core) = &self.avr {
            return match address {
                0..=0x1F => core.regs[address as usize],
                0x20..=0x5F => core.io[(address as usize) - 0x20],
                0x60.. => {
                    let idx = (address as usize) - 0x60;
                    if idx < core.sram.len() {
                        core.sram[idx]
                    } else {
                        0
                    }
                }
            };
        }
        if let Some(core) = &self.msp430 {
            return core.read_u8(address);
        }
        if let Some(core) = &self.v850 {
            return core.read_u8(address);
        }
        if let Some(core) = &self.stm8 {
            return core.read_mem_u8(address);
        }
        if let Some(core) = &self.rx {
            return core.read_u8(address);
        }
        if let Some(core) = &self.rl78 {
            return core.read_u8(address);
        }
        self.memory[(address as usize) % MEMORY_SIZE]
    }

    /// Read a byte from the architecture's **code** space (for disassembly /
    /// instruction fetching). Cores with a separate program ROM (MCS-51 /
    /// 80251 / C166 / AVR) do not mirror code into `memory`, so the ROM is read
    /// directly; every other core routes through the generic byte accessor.
    pub fn read_code(&self, address: u32) -> u8 {
        if let Some(core) = &self.mcs51 {
            let i = address as usize;
            return if i < core.rom.len() { core.rom[i] } else { 0 };
        }
        if let Some(core) = &self.x51 {
            let i = address as usize;
            return if i < core.rom.len() { core.rom[i] } else { 0 };
        }
        if let Some(core) = &self.c166 {
            let i = address as usize;
            return if i < core.rom.len() { core.rom[i] } else { 0 };
        }
        if let Some(core) = &self.avr {
            let i = address as usize;
            return if i < core.rom.len() { core.rom[i] } else { 0 };
        }
        self.read_u8(address)
    }

     pub fn write_u32(&mut self, address: u32, value: u32) {
         if let Some(core) = self.arm.as_mut() {
             core.write_u32(address, value);
            self.peripheral_write(address, value as u8);
             return;
         }
         if let Some(core) = self.xtensa.as_mut() {
             core.write_u32(address, value);
            self.peripheral_write(address, value as u8);
             return;
         }
        if let Some(core) = self.avr.as_mut() {
             for i in 0..4 {
                let byte = (value >> (8 * i)) as u8;
                let addr = address.wrapping_add(i as u32);
                match addr {
                    0..=0x1F => core.regs[addr as usize] = byte,
                    0x20..=0x5F => core.io[(addr as usize) - 0x20] = byte,
                    0x60.. => {
                        let idx = (addr as usize) - 0x60;
                        if idx < core.sram.len() {
                            core.sram[idx] = byte;
                        }
                    }
                }
            }
            return;
        }
         if let Some(core) = self.msp430.as_mut() {
             core.write_u32(address, value);
            self.peripheral_write(address, value as u8);
             return;
         }
         if let Some(core) = self.v850.as_mut() {
             core.write_u32(address, value);
            self.peripheral_write(address, value as u8);
             return;
         }
         if let Some(core) = self.stm8.as_mut() {
             core.write_mem_u32(address, value);
            self.peripheral_write(address, value as u8);
             return;
         }
         if let Some(core) = self.rx.as_mut() {
             core.write_u32(address, value);
            self.peripheral_write(address, value as u8);
             return;
         }
         if let Some(core) = self.rl78.as_mut() {
             core.write_u32(address, value);
            self.peripheral_write(address, value as u8);
             return;
         }
        let base = (address as usize) & !3;
        for i in 0..4 {
            let idx = base.wrapping_add(i) % MEMORY_SIZE;
            self.memory[idx] = (value >> (8 * i)) as u8;
        }
    }

    pub fn write_bytes(&mut self, base: u32, bytes: &[u8]) {
        if let Some(core) = self.arm.as_mut() {
            core.write_bytes(base, bytes);
            return;
        }
        if let Some(core) = self.avr.as_mut() {
            core.load_image(base, bytes);
            return;
        }
        if let Some(core) = self.xtensa.as_mut() {
            core.write_bytes(base, bytes);
            return;
        }
        if let Some(core) = self.msp430.as_mut() {
            core.write_bytes(base, bytes);
            return;
        }
        if let Some(core) = self.v850.as_mut() {
            core.write_bytes(base, bytes);
            return;
        }
        if let Some(core) = self.stm8.as_mut() {
            core.write_mem_bytes(base, bytes);
            return;
        }
        if let Some(core) = self.rx.as_mut() {
            core.write_bytes(base, bytes);
            return;
        }
        if let Some(core) = self.rl78.as_mut() {
            core.write_bytes(base, bytes);
            return;
        }
        for (i, byte) in bytes.iter().enumerate() {
            let idx = (base as usize).wrapping_add(i) % MEMORY_SIZE;
            self.memory[idx] = *byte;
        }
    }


    fn zero_bytes(&mut self, base: u32, count: usize) {
        if let Some(core) = self.arm.as_mut() {
            for i in 0..count {
                core.write_u8(base.wrapping_add(i as u32), 0);
            }
            return;
        }
        if let Some(core) = self.avr.as_mut() {
            for i in 0..count {
                let addr = base as usize + i;
                if addr < core.rom.len() {
                    core.rom[addr] = 0;
                }
            }
            return;
        }
        if let Some(core) = self.xtensa.as_mut() {
            core.zero_bytes(base, count as u32);
            return;
        }
        if let Some(core) = self.msp430.as_mut() {
            core.zero_bytes(base, count as u32);
            return;
        }
        if let Some(core) = self.v850.as_mut() {
            core.zero_bytes(base, count as u32);
            return;
        }
        if let Some(core) = self.stm8.as_mut() {
            core.zero_mem_bytes(base, count as u32);
            return;
        }
        if let Some(core) = self.rx.as_mut() {
            for i in 0..count {
                core.write_u8(base.wrapping_add(i as u32), 0);
            }
            return;
        }
        if let Some(core) = self.rl78.as_mut() {
            core.zero_bytes(base, count as u32);
            return;
        }
        for i in 0..count {
            let idx = (base as usize).wrapping_add(i) % MEMORY_SIZE;
            self.memory[idx] = 0;
        }
    }


    fn execute(&mut self, insn: u32) {
        let opcode = insn & 0x7f;
        let rd = ((insn >> 7) & 0x1f) as usize;
        let rs1 = ((insn >> 15) & 0x1f) as usize;
        let rs2 = ((insn >> 20) & 0x1f) as usize;
        let funct3 = (insn >> 12) & 0x7;
        let funct7 = (insn >> 25) & 0x7f;
        let imm_i = ((insn as i32) >> 20) as u32; // sign-extended
        let imm_s = (((insn >> 7) & 0x1f) | (((insn >> 25) & 0x7f) << 5)) as u32;
        let imm_s = sign_extend(imm_s, 12);

        match opcode {
            0x37 => {
                // LUI
                let value = insn & 0xffff_f000;
                self.set_reg(rd, value);
                self.pc = self.pc.wrapping_add(4);
            }
            0x17 => {
                // AUIPC
                let value = (insn & 0xffff_f000).wrapping_add(self.pc);
                self.set_reg(rd, value);
                self.pc = self.pc.wrapping_add(4);
            }
            0x6f => {
                // JAL
                let imm = jal_immediate(insn);
                let target = self.pc.wrapping_add(imm);
                self.set_reg(rd, self.pc.wrapping_add(4));
                self.pc = target;
            }
            0x67 => {
                // JALR
                let target = self.registers[rs1].wrapping_add(imm_i) & !1;
                self.set_reg(rd, self.pc.wrapping_add(4));
                self.pc = target;
            }
            0x63 => {
                // Branch
                let imm = branch_immediate(insn);
                let take = match funct3 {
                    0 => self.registers[rs1] == self.registers[rs2],
                    1 => self.registers[rs1] != self.registers[rs2],
                    4 => (self.registers[rs1] as i32) < (self.registers[rs2] as i32),
                    5 => (self.registers[rs1] as i32) >= (self.registers[rs2] as i32),
                    6 => self.registers[rs1] < self.registers[rs2],
                    7 => self.registers[rs1] >= self.registers[rs2],
                    _ => false,
                };
                self.pc = if take {
                    self.pc.wrapping_add(imm)
                } else {
                    self.pc.wrapping_add(4)
                };
            }
            0x03 => {
                // Loads
                let addr = self.registers[rs1].wrapping_add(imm_i);
                let value = if let Some(periph) = self.peripheral_read(addr) {
                    periph
                } else {
                    match funct3 {
                        0 => self.read_u8(addr) as u32,             // LB
                        1 => self.read_u16(addr) as u32,            // LH
                        2 => self.read_u32(addr),                   // LW
                        4 => (self.read_u8(addr) as u32) & 0xff,    // LBU
                        5 => (self.read_u16(addr) as u32) & 0xffff, // LHU
                        _ => 0,
                    }
                };
                self.set_reg(rd, value);
                self.pc = self.pc.wrapping_add(4);
            }
            0x23 => {
                // Stores
                let addr = self.registers[rs1].wrapping_add(imm_s);
                let value = self.registers[rs2];
                match funct3 {
                    0 => {
                        self.memory[(addr as usize) % MEMORY_SIZE] = value as u8;
                        self.peripheral_write(addr, value as u8);
                    }
                    1 => {
                        self.write_bytes(addr, &value.to_le_bytes()[..2]);
                        self.peripheral_write(addr, value as u8);
                    }
                    2 => {
                        self.write_u32(addr, value);
                        self.peripheral_write(addr, value as u8);
                    }
                    _ => {}
                }
                self.pc = self.pc.wrapping_add(4);
            }
            0x13 => {
                // OP-IMM
                let value = match funct3 {
                    0 => self.registers[rs1].wrapping_add(imm_i),
                    2 => self.registers[rs1].wrapping_sub(imm_i),
                    3 => (self.registers[rs1] as i32).wrapping_shl((imm_i & 0x1f) as u32) as u32,
                    4 => {
                        let shamt = (insn >> 20) & 0x1f;
                        (self.registers[rs1] as i32 >> shamt) as u32
                    }
                    5 => {
                        let shamt = (insn >> 20) & 0x1f;
                        if funct7 == 0x20 {
                            self.registers[rs1] >> shamt
                        } else {
                            (self.registers[rs1] as i32 >> shamt) as u32
                        }
                    }
                    6 => self.registers[rs1] ^ imm_i,
                    7 => self.registers[rs1] & imm_i,
                    1 => self.registers[rs1] | imm_i,
                    _ => 0,
                };
                self.set_reg(rd, value);
                self.pc = self.pc.wrapping_add(4);
            }
            0x33 => {
                // OP
                let value = match funct3 {
                    0 if funct7 == 0x20 => self.registers[rs1].wrapping_sub(self.registers[rs2]),
                    0 => self.registers[rs1].wrapping_add(self.registers[rs2]),
                    1 => self.registers[rs1].wrapping_shl(self.registers[rs2] & 0x1f),
                    2 => ((self.registers[rs1] as i32) < (self.registers[rs2] as i32)) as u32,
                    3 => (self.registers[rs1] < self.registers[rs2]) as u32,
                    4 => self.registers[rs1] ^ self.registers[rs2],
                    5 if funct7 == 0x20 => self.registers[rs1] >> (self.registers[rs2] & 0x1f),
                    5 => ((self.registers[rs1] as i32) >> (self.registers[rs2] & 0x1f)) as u32,
                    6 => self.registers[rs1] | self.registers[rs2],
                    7 => self.registers[rs1] & self.registers[rs2],
                    _ => 0,
                };
                self.set_reg(rd, value);
                self.pc = self.pc.wrapping_add(4);
            }
            0x0f => {
                // FENCE / ECALL / EBREAK
                if insn & 0xfff == 0x73 {
                    // ECALL: emulate a `printf`-style halt via register a7.
                    let syscall = self.registers[17]; // a7
                    if syscall == 0 {
                        self.halted = true;
                    }
                }
                self.pc = self.pc.wrapping_add(4);
            }
            0x73 => {
                // SYSTEM: MRET, CSRRW/CSRRS/CSRRC, CSRRWI/CSRRSI/CSRRCI.
                let funct12 = (insn >> 20) & 0xfff;
                let funct3 = (insn >> 12) & 0x7;
                if funct12 == 0x302 && funct3 == 0 && rd == 0 && rs1 == 0 {
                    // MRET: return from machine trap.
                    let mepc = self.csr.mepc;
                    self.csr.mstatus |= 0x8; // MIE restored
                    self.pc = mepc & !1;
                    return;
                }
                let csr = funct12 as u16;
                let old = self.read_csr(csr);
                let new = match funct3 {
                    1 => {
                        // CSRRW: write rs1 into csr, return old.
                        self.registers[rs1]
                    }
                    2 => old | self.registers[rs1], // CSRRS
                    3 => old & !self.registers[rs1], // CSRRC
                    5 => rs1 as u32,                 // CSRRWI
                    6 => old | (rs1 as u32),         // CSRRSI
                    7 => old & !(rs1 as u32),        // CSRRCI
                    _ => old,
                };
                self.write_csr(csr, new);
                self.set_reg(rd, old);
                self.pc = self.pc.wrapping_add(4);
            }
            _ => {
                // Unknown opcode: halt to avoid runaway.
                self.halted = true;
            }
        }
    }
    /// Read a machine-mode CSR by its 12-bit address.
    fn read_csr(&self, csr: u16) -> u32 {
        match csr {
            0x300 => self.csr.mstatus,
            0x305 => self.csr.mtvec,
            0x341 => self.csr.mepc,
            0x342 => self.csr.mcause,
            0x304 => self.csr.mie,
            0xf14 => self.csr.mhartid,
            _ => 0,
        }
    }

    /// Write a machine-mode CSR by its 12-bit address. Read-only CSRs (e.g.
    /// `mhartid`) ignore the write, matching real hardware semantics.
    fn write_csr(&mut self, csr: u16, value: u32) {
        match csr {
            0x300 => self.csr.mstatus = value,
            0x305 => self.csr.mtvec = value,
            0x341 => self.csr.mepc = value,
            0x342 => self.csr.mcause = value,
            0x304 => self.csr.mie = value,
            _ => {}
        }
    }

    /// Take the lowest-numbered pending and enabled interrupt, if the global
    /// interrupt enable (`MIE` bit of `mstatus`) is set. Saves `mepc`/`mcause`
    /// and redirects PC to the trap vector: `mtvec` directly (mode 0) or
    /// `mtvec + 4*line` (vectored mode 1). Returns true if a trap was taken.
    pub fn take_interrupt(&mut self) -> bool {
        if self.csr.mstatus & 0x8 == 0 {
            return false; // MIE clear: interrupts are globally masked.
        }
        let pending = self
            .peripherals
            .irq_pending
            .iter()
            .enumerate()
            .find(|(line, p)| **p && (self.csr.mie >> line) & 1 == 1);
        let Some((line, _)) = pending else {
            return false;
        };
        self.csr.mepc = self.pc;
        self.csr.mcause = 0x8000_0000 | (line as u32);
        self.csr.mstatus &= !0x8; // MIE clear during trap
        let mode = self.csr.mtvec & 0x3;
        let base = self.csr.mtvec & !0x3;
        self.pc = if mode == 1 {
            base.wrapping_add((line as u32).wrapping_mul(4))
        } else {
            base
        };
        true
    }

    /// Whether any interrupt is currently pending and enabled (for UI display).
    pub fn any_pending(&self) -> bool {
        self.peripherals
            .irq_pending
            .iter()
            .enumerate()
            .any(|(line, p)| *p && (self.csr.mie >> line) & 1 == 1)
    }

    /// Read a memory-mapped peripheral register (GPIO / timers / watchdog).
    /// Returns `None` when the address does not map to a peripheral register.
    pub fn peripheral_read_reg(&self, address: u32) -> Option<u32> {
        self.peripheral_read(address)
    }

    /// Route a single byte write to the peripheral handler (UART output,
    /// GPIO / timer / watchdog control registers). The byte is also written
    /// to the backing memory so later reads observe it.
    pub fn peripheral_write_reg(&mut self, address: u32, byte: u8) {
        self.write_bytes(address, &[byte]);
        self.peripheral_write(address, byte);
    }


    fn set_reg(&mut self, index: usize, value: u32) {
        if index != 0 {
            self.registers[index] = value;
        }
    }

    fn read_u16(&self, address: u32) -> u16 {
        let base = (address as usize) & !1;
        (self.memory[base % MEMORY_SIZE] as u16)
            | ((self.memory[(base + 1) % MEMORY_SIZE] as u16) << 8)
    }

     /// Route a store to virtual peripherals if the address is in their range.
    fn peripheral_write(&mut self, address: u32, byte: u8) {
         if address >= self.peripherals.uart_base && address < self.peripherals.uart_base + 8 {
            if byte != 0 {
                self.peripherals.outputs.push(byte);
                (self.peripherals.putchar)(byte);
             }
         }
         self.gpio_write(address);
         self.timer_write(address);
         self.watchdog_write(address);
         // Fall back to SVD-defined registers: store the full 32-bit value
         // written at the register address (read back from memory).
         if self.peripherals.external_regs.contains_key(&address) {
             let value = self.read_u32(address);
             if let Some(entry) = self.peripherals.external_regs.get_mut(&address) {
                 *entry = value;
             }
         }
     }

    /// Handle watchdog register writes: base+0x00 feed (0xAA55 resets the
    /// timer), base+0x04 enable/disable, base+0x08 timeout cycles.
    fn watchdog_write(&mut self, address: u32) {
        // Read the written value first (it borrows `self` immutably through
        // `read_u32`), then take the mutable watchdog borrow.
        let value = self.read_u32(address);
        let Some(watchdog) = self.peripherals.watchdog.as_mut() else {
            return;
        };
        let offset = address.wrapping_sub(watchdog.base);
        if offset >= 0x10 {
            return;
        }
        match offset {
            0x00 => {
                // Feed: write 0xAA55 to reload the countdown. Any other write
                // (e.g. 0x00) leaves the watchdog alone.
                if value & 0xFFFF == 0xAA55 {
                    watchdog.elapsed = 0;
                }
            }
            0x04 => watchdog.enabled = value & 1 != 0,
            0x08 => watchdog.timeout_cycles = value as u64,
            _ => {}
        }
    }

    /// Handle GPIO register writes (OUT, OUT_SET, OUT_CLR, DIR, IN).
    fn gpio_write(&mut self, address: u32) {
        let base = self.peripherals.gpio_base;
        let value = self.read_u32(address);
        let offset = address.wrapping_sub(base);
        let old = self.peripherals.gpio_out;
        match offset {
            0x00 => {
                // OUT
                let new = value & self.peripherals.gpio_dir;
                self.peripherals.gpio_out = new;
            }
            0x04 => {
                // OUT_SET
                self.peripherals.gpio_out |= value & self.peripherals.gpio_dir;
            }
            0x08 => {
                // OUT_CLR
                self.peripherals.gpio_out &= !(value & self.peripherals.gpio_dir);
            }
            0x0c => {
                // DIR
                self.peripherals.gpio_dir = value;
            }
            _ => {}
        }
        if old != self.peripherals.gpio_out {
            let diff = old ^ self.peripherals.gpio_out;
            for pin in 0..32 {
                if diff & (1 << pin) != 0 {
                    let value = (self.peripherals.gpio_out >> pin) & 1;
                    self.peripherals.gpio_events.push(GpioEvent {
                        cycle: self.cycle_count,
                        pin,
                        value,
                        pc: self.pc,
                    });
                }
            }
        }
    }

    /// Handle timer register writes (CR, PSC, CNT, ARR, SR).
    fn timer_write(&mut self, address: u32) {
        let value = self.read_u32(address);
        for timer in self.peripherals.timers.iter_mut() {
            let offset = address.wrapping_sub(timer.base);
            match offset {
                0x00 => timer.enabled = value & 1 != 0,
                0x04 => timer.prescaler = value & 0xffff,
                0x08 => timer.counter = value,
                0x0c => timer.auto_reload = value,
                0x10 => {
                    // SR: clear update interrupt.
                    if timer.irq_line < self.peripherals.irq_pending.len() {
                        self.peripherals.irq_pending[timer.irq_line] = false;
                    }
                }
                _ => {}
            }
        }
    }

    /// Read a memory-mapped peripheral register (GPIO / timers / UART status).
    fn peripheral_read(&self, address: u32) -> Option<u32> {
        if address >= self.peripherals.gpio_base
            && address < self.peripherals.gpio_base + 0x20
        {
            let offset = address.wrapping_sub(self.peripherals.gpio_base);
            return Some(match offset {
                0x00 => self.peripherals.gpio_out,
                0x04 => self.peripherals.gpio_in,
                0x08 => self.peripherals.gpio_dir,
                _ => 0,
            });
        }
        for timer in &self.peripherals.timers {
            let offset = address.wrapping_sub(timer.base);
            if offset < 0x14 {
                return Some(match offset {
                    0x00 => timer.enabled as u32,
                    0x04 => timer.prescaler,
                    0x08 => timer.counter,
                    0x0c => timer.auto_reload,
                    0x10 => {
                        if timer.irq_line < self.peripherals.irq_pending.len()
                            && self.peripherals.irq_pending[timer.irq_line]
                        {
                            1
                        } else {
                            0
                        }
                    }
                    _ => 0,
                });
            }
        }
        if let Some(watchdog) = &self.peripherals.watchdog {
            let offset = address.wrapping_sub(watchdog.base);
            if offset < 0x10 {
                return Some(match offset {
                    0x00 => watchdog.enabled as u32,
                    0x04 => watchdog.elapsed as u32,
                    0x08 => watchdog.reset_count as u32,
                    0x0c => watchdog.last_reset_pc,
                    _ => 0,
                });
            }
        }
        if let Some(value) = self.peripherals.external_regs.get(&address) {
            return Some(*value);
        }
        None
    }
}

impl Default for Simulator {
    fn default() -> Self {
        Self::new()
    }
}

fn sign_extend(value: u32, bits: u32) -> u32 {
    let shift = 32 - bits;
    ((value << shift) as i32 >> shift) as u32
}

fn jal_immediate(insn: u32) -> u32 {
    let imm20 = (insn >> 31) & 1;
    let imm10_1 = (insn >> 21) & 0x3ff;
    let imm11 = (insn >> 20) & 1;
    let imm19_12 = (insn >> 12) & 0xff;
    let value = (imm20 << 20) | (imm10_1 << 1) | (imm11 << 11) | (imm19_12 << 12);
    sign_extend(value, 21)
}

fn branch_immediate(insn: u32) -> u32 {
    let imm12 = (insn >> 31) & 1;
    let imm10_5 = (insn >> 25) & 0x3f;
    let imm4_1 = (insn >> 8) & 0xf;
    let imm11 = (insn >> 7) & 1;
    let value = (imm12 << 12) | (imm10_5 << 5) | (imm4_1 << 1) | (imm11 << 11);
    sign_extend(value, 13)
}

fn read_u32(bytes: &[u8], offset: usize, little: bool) -> u32 {
    if little {
        u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
    } else {
        u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }
}

fn read_u16(bytes: &[u8], offset: usize, little: bool) -> u16 {
    if little {
        u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
    } else {
        u16::from_be_bytes(bytes[offset..offset + 2].try_into().unwrap())
    }
}

/// Human-readable register name (ABI).
pub fn reg_name(index: usize) -> &'static str {
    const NAMES: [&str; 32] = [
        "zero", "ra", "sp", "gp", "tp", "t0", "t1", "t2", "s0", "s1", "a0", "a1", "a2", "a3",
        "a4", "a5", "a6", "a7", "s2", "s3", "s4", "s5", "s6", "s7", "s8", "s9", "s10", "s11",
        "t3", "t4", "t5", "t6",
    ];
    NAMES[index.min(31)]
}

/// Number of register-file entries the IDE should display for an
/// architecture. RISC-V shows 32 (x0-x31); ARM/Xtensa show 16 (r0-r15 /
/// a0-a15); 8051/80251/C166 show the special registers plus their bank.
pub fn reg_count_for(arch: Arch) -> usize {
    match arch {
        Arch::RiscV => REG_COUNT,
        Arch::Arm | Arch::Xtensa => 16,
        // 8051: ACC/B/PSW/SP/DPTR + R0-R7; 80251: ACC/B/PSW/SP/DPTR + R0-R15;
        // C166: R0-R15.
        Arch::Mcs51 => 13,
        Arch::Mcs251 => 21,
        Arch::C166 => 16,
        Arch::Avr => 32,
        Arch::Msp430 => 16,
        Arch::V850 => 32,
        Arch::Stm8 => 5,
        Arch::Rx => 16,
        Arch::Rl78 => 16,
    }
}

/// Human-readable register label for an architecture and index.
pub fn reg_label_for(arch: Arch, index: usize) -> String {
    match arch {
        Arch::RiscV => reg_name(index).to_string(),
        Arch::Arm => format!("r{}", index.min(15)),
        Arch::Xtensa => format!("a{}", index & 15),
        Arch::Mcs51 => match index {
            0 => "ACC".to_string(),
            1 => "B".to_string(),
            2 => "PSW".to_string(),
            3 => "SP".to_string(),
            4 => "DPTR".to_string(),
            n if n < 13 => format!("R{}", n - 5),
            _ => format!("r{}", index),
        },
        Arch::Mcs251 => match index {
            0 => "ACC".to_string(),
            1 => "B".to_string(),
            2 => "PSW".to_string(),
            3 => "SP".to_string(),
            4 => "DPTR".to_string(),
            n if n < 21 => format!("R{}", n - 5),
            _ => format!("r{}", index),
        },
        Arch::C166 => format!("R{}", index.min(15)),
        Arch::Avr => format!("R{}", index.min(31)),
        Arch::Msp430 => msp430::Msp430Core::register_label(index),
        Arch::V850 => v850::V850Core::register_label(index),
        Arch::Stm8 => match index {
            0 => "A".to_string(),
            1 => "X".to_string(),
            2 => "Y".to_string(),
            3 => "SP".to_string(),
            4 => "CC".to_string(),
            _ => format!("r{}", index),
        },
        Arch::Rx => rx::RxCore::register_label(index),
        Arch::Rl78 => rl78::Rl78Core::register_label(index),
    }

}

/// Disassemble a single instruction at the current PC for an architecture.
/// `read` is a closure returning the byte at an address (the simulator's
/// code space). Returns a mnemonic string like `add a0, a1, a2`.
pub fn disassemble_one(arch: Arch, pc: u32, read: &dyn Fn(u32) -> u8) -> String {
    match arch {
        Arch::RiscV => disassemble_riscv(pc, read),
        Arch::Arm => disassemble_arm_state(pc, read, false),
        Arch::Xtensa => disassemble_xtensa(pc, read),
        Arch::Mcs51 => disassemble_mcs51(pc, read),
        Arch::Mcs251 => disassemble_mcs251(pc, read),
        Arch::C166 => disassemble_c166(pc, read),
        Arch::Avr => avr::disassemble(pc, read),
        Arch::Msp430 => msp430::disassemble(pc, read),
        Arch::V850 => v850::disassemble(pc, read),
        Arch::Stm8 => stm8::disassemble(pc, read),
        Arch::Rx => rx::disassemble(pc, read),
        Arch::Rl78 => rl78::disassemble(pc, read),
    }
}

/// Whether a disassembled mnemonic is a return-class instruction. Used by
/// `step_out` to detect the function epilogue; matches the mnemonics emitted
/// across all supported architectures.
pub fn is_return_mnemonic(insn: &str) -> bool {
    // Disassemblers emit mnemonics in different cases (e.g. MSP430 renders
    // `RETI`/`CALL` uppercase), so compare case-insensitively.
    let insn = insn.trim_start().to_ascii_lowercase();
    let insn = insn.as_str();
    for prefix in ["ret", "reti", "iret", "rts", "rti", "rfe", "rte", "retf", "mret"] {
        if insn == prefix
            || insn
                .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_whitespace() || c == ','))
        {
            return true;
        }
    }
    // ARM-family returns: `bx lr`/`blx lr` (or any `bx r14`) and
    // `pop {.., pc}` / `ldm/ldmia ... {.., pc}` restore the link register or
    // the program counter, i.e. they leave the current function.
    let body = insn
        .split_once(|c: char| c.is_ascii_whitespace() || c == ',')
        .map(|(m, _)| m)
        .unwrap_or(insn);
    if body == "bx" || body == "blx" {
        let target = insn
            .split_whitespace()
            .nth(1)
            .unwrap_or("")
            .trim_end_matches(',');
        return target == "lr" || target == "r14";
    }
    if body == "pop" || body == "ldm" || body == "ldmia" || body == "ldmfd" {
        return insn.contains("pc") || insn.contains("r15");
    }
    // MSP430 assembles `RET` as `MOV @SP+, PC`: popping the return address
    // into the program counter leaves the current function.
    if body == "mov"
        && insn.contains("@sp+")
        && insn.split_whitespace().next_back() == Some("pc")
    {
        return true;
    }
    false
}

/// Whether a disassembled mnemonic is a call-class instruction (used by
/// `step_over` to decide whether to run to the caller's next instruction).
/// Covers the mnemonics emitted across all supported architectures.
pub fn is_call_mnemonic(insn: &str) -> bool {
    let body = insn
        .trim_start()
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_end_matches(',')
        .to_ascii_lowercase();
    matches!(
        body.as_str(),
        // RISC-V / ARM / Xtensa.
        "jal" | "jalr" | "bl" | "blx" | "call0" | "call4" | "call8" | "call12"
        | "callx0" | "callx4" | "callx8" | "callx12"
        // MCS-51 / 80251 / C166 / AVR / MSP430.
        | "lcall" | "acall" | "call" | "callf" | "callr" | "callt" | "rcall" | "icall"
        // V850 / RX.
        | "jarl" | "bsr" | "bsr.a" | "bsr.w" | "bsr.l" | "jsr"
    )
}

/// Disassemble one instruction at `pc`, choosing ARM (A32) or Thumb decoding
/// based on `arm_state`. Used by the IDE when the core is an ARM-family CPU
/// (Cortex-M always Thumb; ARM7/ARM9/Cortex-A run A32).
pub fn disassemble_one_state(
    arch: Arch,
    pc: u32,
    read: &dyn Fn(u32) -> u8,
    arm_state: bool,
) -> String {
    if arch == Arch::Arm {
        disassemble_arm_state(pc, read, arm_state)
    } else {
        disassemble_one(arch, pc, read)
    }
}

/// Decoded instruction width in bytes for the architecture at `pc`. Used to
/// advance the disassembly cursor. ARM A32 is 4 bytes; Thumb instructions are
/// 2 bytes, except Thumb-2 32-bit encodings (top halfword `0xE000`) which are
/// 4 bytes. Fixed-width architectures return their known width.
pub fn instruction_size(
    arch: Arch,
    pc: u32,
    read: &dyn Fn(u32) -> u8,
    arm_state: bool,
) -> u32 {
    if arch == Arch::Arm {
        if arm_state {
            return 4;
        }
        let half = read(pc) as u16 | ((read(pc + 1) as u16) << 8);
        return if half & 0xF800 >= 0xE800 { 4 } else { 2 };
    }
    match arch {
        Arch::C166 => c166_instruction_len(read(pc)),
        Arch::Msp430 => msp430::instruction_len(pc, read),
        Arch::Xtensa => 3,
        Arch::Avr => avr_instruction_len(read(pc), read(pc + 1)),
        Arch::Mcs51 => mcs51_instruction_len(read(pc)),
        Arch::Mcs251 => mcs251_instruction_len(read(pc)),
        Arch::V850 => v850::instruction_len(pc, read),
        Arch::Stm8 => stm8::instruction_len(pc, read),
        Arch::Rl78 => rl78::instruction_len(pc, read),
        Arch::Rx => rx::instruction_len(pc, read),
        _ => 4,
    }
}

/// Number of code bytes consumed by a C166 instruction in this simulator's
/// byte-fetch model. Most forms are 2 bytes; `MOV Rn,#imm16` and `CALL` carry
/// a 16-bit operand (3 bytes) and `NOP`/`RET` are 1 byte.
fn c166_instruction_len(op: u8) -> u32 {
    match op {
        0x00 | 0xC1 => 1, // NOP / RET
        0x20..=0x2F | 0xC0 => 3, // MOV Rn,#imm16 / CALL
        _ => 2,
    }
}

/// Number of code bytes consumed by an AVR instruction. AVR is 16-bit-fixed
/// except the 32-bit LDS/STS/JMP/CALL forms (matching the core's own
/// `current_instruction_size`).
fn avr_instruction_len(lo: u8, hi: u8) -> u32 {
    let word = ((hi as u16) << 8) | lo as u16;
    match word & 0xFE0F {
        0x9000 | 0x9200 | 0x940C | 0x940E => 4,
        _ => 2,
    }
}

/// Number of code bytes consumed by an 8051 (MCS-51) instruction. MCS-51 is
/// variable-length: most operations are 1 byte, immediate/direct/relative
/// forms carry one operand byte (2 total), and absolute jumps/calls and
/// two-operand direct forms carry two operand bytes (3 total).
fn mcs51_instruction_len(op: u8) -> u32 {
    // 3-byte encodings: absolute jumps/calls, DPTR loads, and forms that carry
    // two operand bytes.
    if op == 0x02 || op == 0x12      // LJMP / LCALL addr16
        || op == 0x90                 // MOV DPTR, #imm16
        || op == 0x75                 // MOV direct, #imm
        || op == 0x85                 // MOV direct, direct
        || op == 0x43 || op == 0x53 || op == 0x63 // ANL/ORL/XRL direct, #imm
        || op == 0x10 || op == 0x20 || op == 0x30 // JBC / JB / JNB bit, rel
        || (0xB4..=0xBF).contains(&op) // CJNE A/Rn/@Ri, #imm|direct, rel
        || op == 0xD5                  // DJNZ direct, rel
    {
        return 3;
    }
    // 1-byte encodings: accumulator/register/indirect operations with no
    // operand byte.
    if op == 0x00 // NOP
        || op == 0x04 || op == 0x06 || op == 0x07 || (0x08..=0x0F).contains(&op) // INC A/@Ri/Rn
        || op == 0x03 || op == 0x13 || op == 0x14 || op == 0x16 || op == 0x17 || (0x18..=0x1F).contains(&op) // RR/RRC/DEC
        || op == 0x22 || op == 0x23 || op == 0x26 || op == 0x27 || (0x28..=0x2F).contains(&op) // RET/RL/ADD
        || op == 0x32 || op == 0x33 || op == 0x36 || op == 0x37 || (0x38..=0x3F).contains(&op) // RETI/RRC/ADDC
        || op == 0x46 || op == 0x47 || (0x48..=0x4F).contains(&op) // ORL A,@Ri/Rn
        || op == 0x56 || op == 0x57 || (0x58..=0x5F).contains(&op) // ANL A,@Ri/Rn
        || op == 0x66 || op == 0x67 || (0x68..=0x6F).contains(&op) // XRL A,@Ri/Rn
        || op == 0x73 || op == 0x83 || op == 0x84 || op == 0x93 // JMP @A+DPTR / MOVC / DIV
        || op == 0x96 || op == 0x97 || (0x98..=0x9F).contains(&op) // SUBB A,@Ri/Rn
        || op == 0xA3 || op == 0xA4 || op == 0xB3 // INC DPTR / MUL / CPL C
        || op == 0xC3 || op == 0xC4 || op == 0xC6 || op == 0xC7 || (0xC8..=0xCF).contains(&op) // CLR C/SWAP/XCH
        || op == 0xD3 || op == 0xD4 || op == 0xD6 || op == 0xD7 // SETB C / DA / XCHD
        || op == 0xE0 || op == 0xE2 || op == 0xE3 || op == 0xE4 || op == 0xE6 || op == 0xE7 || (0xE8..=0xEF).contains(&op) // MOVX/MOV A
        || op == 0xF0 || op == 0xF2 || op == 0xF3 || op == 0xF4 || op == 0xF6 || op == 0xF7 || (0xF8..=0xFF).contains(&op) // MOVX/MOV Rn
    {
        return 1;
    }
    // Remaining encodings carry one operand byte: immediate, relative, direct,
    // or the addr11 page byte of AJMP/ACALL.
    2
}

/// Number of code bytes consumed by an 80251 (MCS-251) instruction. The 80251
/// is variable-length: register/accumulator/indirect ops are 1 byte, relative
/// branches and direct forms are 2, `MOV Rn,#imm16` is 3, and 24-bit absolute
/// jumps/calls and DPTR loads are 4.
fn mcs251_instruction_len(op: u8) -> u32 {
    // 4-byte encodings: 24-bit absolute jumps/calls and DPTR loads.
    if op == 0x02 || op == 0x12 || op == 0x90 {
        return 4;
    }
    // 3-byte encodings: MOV Rn, #imm16.
    if (0x74..=0x7F).contains(&op) {
        return 3;
    }
    // 2-byte encodings: relative branches and MOV A, direct.
    if op == 0x40 || op == 0x50 || op == 0x60 || op == 0x70 || op == 0x80 || op == 0xE5 {
        return 2;
    }
    // Everything else (register/accumulator/indirect operations) is 1 byte.
    1
}

fn disassemble_riscv(pc: u32, read: &dyn Fn(u32) -> u8) -> String {
    let insn = read(pc) as u32
        | ((read(pc + 1) as u32) << 8)
        | ((read(pc + 2) as u32) << 16)
        | ((read(pc + 3) as u32) << 24);
    let opcode = insn & 0x7f;
    let rd = ((insn >> 7) & 0x1f) as usize;
    let rs1 = ((insn >> 15) & 0x1f) as usize;
    let rs2 = ((insn >> 20) & 0x1f) as usize;
    let funct3 = (insn >> 12) & 0x7;
    let funct7 = (insn >> 25) & 0x7f;
    let imm_i = ((insn as i32) >> 20) as i32;
    let imm_s = sign_extend(
        (((insn >> 7) & 0x1f) | (((insn >> 25) & 0x7f) << 5)) as u32,
        12,
    ) as i32;
    match opcode {
        0x37 => format!("lui {}, 0x{:x}", reg_name(rd), insn >> 12),
        0x17 => format!("auipc {}, 0x{:x}", reg_name(rd), insn >> 12),
        0x6f => {
            let imm = jal_immediate(insn);
            format!("jal {}, {:#x}", reg_name(rd), pc.wrapping_add(imm))
        }
        0x67 => {
            // Canonical `ret` / `jr` encodings: `jalr x0, ra, 0` → "ret",
            // `jalr x0, rs1, 0` → "jr rs1".
            if rd == 0 && imm_i == 0 {
                if rs1 == 1 {
                    "ret".to_string()
                } else {
                    format!("jr {}", reg_name(rs1))
                }
            } else {
                format!("jalr {}, {}, {}", reg_name(rd), reg_name(rs1), imm_i)
            }
        }
        0x63 => {
            let imm = branch_immediate(insn);
            let cond = match funct3 {
                0 => "beq",
                1 => "bne",
                4 => "blt",
                5 => "bge",
                6 => "bltu",
                7 => "bgeu",
                _ => "br",
            };
            format!("{cond} {}, {}, {:#x}", reg_name(rs1), reg_name(rs2), pc.wrapping_add(imm))
        }
        0x03 => {
            let name = match funct3 {
                0 => "lb",
                1 => "lh",
                2 => "lw",
                4 => "lbu",
                5 => "lhu",
                _ => "ld",
            };
            format!("{name} {}, {}({})", reg_name(rd), imm_i, reg_name(rs1))
        }
        0x23 => {
            let name = match funct3 {
                0 => "sb",
                1 => "sh",
                2 => "sw",
                _ => "sd",
            };
            format!("{name} {}, {}({})", reg_name(rs2), imm_s, reg_name(rs1))
        }
        0x13 => {
            let name = match funct3 {
                0 => "addi",
                2 => "slti",
                3 => "sltiu",
                4 => "xori",
                6 => "ori",
                7 => "andi",
                1 => "slli",
                5 if funct7 == 0x20 => "srli",
                5 => "srai",
                _ => "opimm",
            };
            format!("{name} {}, {}, {}", reg_name(rd), reg_name(rs1), imm_i)
        }
        0x33 => {
            let name = match funct3 {
                0 if funct7 == 0x20 => "sub",
                0 => "add",
                1 => "sll",
                2 => "slt",
                3 => "sltu",
                4 => "xor",
                5 if funct7 == 0x20 => "srl",
                5 => "sra",
                6 => "or",
                7 => "and",
                _ => "op",
            };
            format!("{name} {}, {}, {}", reg_name(rd), reg_name(rs1), reg_name(rs2))
        }
        0x0f => {
            if insn & 0xfff == 0x73 {
                "ecall".to_string()
            } else {
                "fence".to_string()
            }
        }
        0x73 => {
            let funct12 = (insn >> 20) & 0xfff;
            if funct12 == 0x302 && funct3 == 0 && rd == 0 && rs1 == 0 {
                // MRET: machine-mode interrupt return.
                return "mret".to_string();
            }
            let csr = ((insn >> 20) & 0xfff) as u16;
            let name = match funct3 {
                1 => "csrrw",
                2 => "csrrs",
                3 => "csrrc",
                5 => "csrrwi",
                6 => "csrrsi",
                7 => "csrrci",
                _ => "csr",
            };
            format!("{name} {}, 0x{:x}, {}", reg_name(rd), csr, reg_name(rs1))
        }
        _ => format!(".word 0x{insn:08x}"),
    }
}

fn disassemble_arm_state(pc: u32, read: &dyn Fn(u32) -> u8, arm_state: bool) -> String {
    if arm_state {
        return disassemble_a32(pc, read);
    }
    let half = read(pc) as u16 | ((read(pc + 1) as u16) << 8);
    let (insn, width) = if half & 0xF800 >= 0xE800 {
        let second = read(pc + 2) as u16 | ((read(pc + 3) as u16) << 8);
        (((half as u32) << 16) | second as u32, 4)
    } else {
        (half as u32, 2)
    };
    if width == 4 {
        return disassemble_thumb2(insn, pc, read);
    }
    disassemble_thumb1(half, pc)
}

/// Decode a 16-bit Thumb-1 instruction, mirroring `ArmCore::execute_thumb1`.
fn disassemble_thumb1(half: u16, pc: u32) -> String {
    let top = half >> 12;
    let rm = ((half >> 3) & 0x0F) as usize;
    let rd = (half & 0x07) as usize;
    let imm8 = (half & 0xFF) as u32;
    let imm3 = ((half >> 6) & 0x07) as u32;
    let imm5 = ((half >> 6) & 0x1F) as u32;
    let reg_list = |h: u16| -> Vec<String> {
        (0..8)
            .filter(|i| h & (1 << i) != 0)
            .map(|i| format!("r{i}"))
            .collect()
    };
    match top {
        // LSL/LSR immediate (top = 0).
        0x0 => {
            let name = match (half >> 11) & 0x03 {
                0 => "lsls",
                1 => "lsrs",
                _ => "asrs",
            };
            format!("{name} r{rd}, r{}, #{imm5}", (half >> 3) & 0x07)
        }
        // ASR immediate: top = 1 with bit 11 clear (bits 15-11 = 00010).
        0x1 if half & 0x0800 == 0 => {
            format!("asrs r{rd}, r{}, #{imm5}", (half >> 3) & 0x07)
        }
        // ADD/SUB register or immediate (bit 10 = sub, bit 9 = immediate).
        0x1 => {
            // Format-1 ADD/SUB: bits 11-9 = 100 (ADD reg), 101 (SUB reg),
            // 110 (ADD imm3), 111 (SUB imm3); bit 9 selects SUB and bit 10
            // selects the immediate form.
            let sub = (half >> 9) & 1 != 0;
            let immediate = (half >> 10) & 1 != 0;
            let name = if sub { "subs" } else { "adds" };
            let rn = ((half >> 3) & 0x07) as usize;
            if immediate {
                format!("{name} r{rd}, r{rn}, #{imm3}")
            } else {
                format!("{name} r{rd}, r{rn}, r{}", (half >> 6) & 0x07)
            }
        }
        // MOV/CMP immediate 8-bit.
        0x2 => {
            let rd = ((half >> 8) & 0x07) as usize;
            if (half >> 11) & 1 != 0 {
                format!("cmp r{rd}, #{imm8}")
            } else {
                format!("movs r{rd}, #{imm8}")
            }
        }
        // ADD/SUB immediate 8-bit.
        0x3 => {
            let rd = ((half >> 8) & 0x07) as usize;
            let name = if (half >> 11) & 1 != 0 { "subs" } else { "adds" };
            format!("{name} r{rd}, #{imm8}")
        }
        // ALU / high-register / PC-relative LDR.
        0x4 => {
            if half & 0x0800 != 0 {
                // PC-relative LDR: rd = bits 10-8, imm8 scaled by 4.
                let rd = ((half >> 8) & 0x07) as usize;
                format!("ldr r{rd}, [pc, #{}]", imm8.wrapping_mul(4))
            } else if half & 0x0400 != 0 {
                // High-register ops / BX (op = bits 9-8).
                let rd_high = rd | (((half >> 4) & 0x08) as usize);
                let rm_high = rm | (((half >> 3) & 0x08) as usize);
                match (half >> 8) & 0x03 {
                    0 => format!("add r{rd_high}, r{rm_high}"),
                    1 => format!("cmp r{rd_high}, r{rm_high}"),
                    2 => format!("mov r{rd_high}, r{rm_high}"),
                    _ => {
                        let mnem = if half & 0x80 != 0 { "blx" } else { "bx" };
                        format!("{mnem} r{rm_high}")
                    }
                }
            } else {
                // ALU (op = bits 9-6).
                let op = (half >> 6) & 0x0F;
                let name = match op {
                    0x0 => "ands", 0x1 => "eors", 0x2 => "lsls", 0x3 => "lsrs",
                    0x4 => "asrs", 0x5 => "adcs", 0x6 => "sbcs", 0x7 => "rors",
                    0x8 => "tst", 0x9 => "rsbs", 0xA => "cmp", 0xB => "cmn",
                    0xC => "orrs", 0xD => "muls", 0xE => "bics", _ => "mvns",
                };
                format!("{name} r{rd}, r{}", (half >> 3) & 7)
            }
        }
        // Load/store register offset (op = bits 11-9).
        0x5 => {
            let op = (half >> 9) & 0x07;
            let rd = rd;
            let rn = ((half >> 3) & 0x07) as usize;
            let rm = ((half >> 6) & 0x07) as usize;
            let name = match op {
                0 => "str", 1 => "strh", 2 => "strb", 3 => "ldrsb",
                4 => "ldr", 5 => "ldrh", 6 => "ldrb", _ => "ldrsh",
            };
            format!("{name} r{rd}, [r{rn}, r{rm}]")
        }
        // STR/LDR word immediate (bit 11 = load).
        0x6 => {
            let load = (half >> 11) & 1 != 0;
            let base = (half & 0x07) as usize;
            let data = ((half >> 3) & 0x07) as usize;
            if load {
                format!("ldr r{data}, [r{base}, #{}]", imm5.wrapping_mul(4))
            } else {
                format!("str r{data}, [r{base}, #{}]", imm5.wrapping_mul(4))
            }
        }
        // STRB/LDRB byte immediate (bit 11 = load).
        0x7 => {
            let load = (half >> 11) & 1 != 0;
            let base = (half & 0x07) as usize;
            let data = ((half >> 3) & 0x07) as usize;
            if load {
                format!("ldrb r{data}, [r{base}, #{imm5}]")
            } else {
                format!("strb r{data}, [r{base}, #{imm5}]")
            }
        }
        // STRH/LDRH half immediate (bit 11 = load).
        0x8 => {
            let load = (half >> 11) & 1 != 0;
            let base = (half & 0x07) as usize;
            let data = ((half >> 3) & 0x07) as usize;
            if load {
                format!("ldrh r{data}, [r{base}, #{}]", imm5.wrapping_mul(2))
            } else {
                format!("strh r{data}, [r{base}, #{}]", imm5.wrapping_mul(2))
            }
        }
        // SP-relative load/store word (bit 11 = load).
        0x9 => {
            let load = (half >> 11) & 1 != 0;
            let rd = ((half >> 8) & 0x07) as usize;
            if load {
                format!("ldr r{rd}, [sp, #{}]", imm8.wrapping_mul(4))
            } else {
                format!("str r{rd}, [sp, #{}]", imm8.wrapping_mul(4))
            }
        }
        // Load address: ADD rd, PC/SP, #imm8*4.
        0xA => {
            let rd = ((half >> 8) & 0x07) as usize;
            let base = if (half >> 11) & 1 != 0 { "sp" } else { "pc" };
            format!("add r{rd}, {base}, #{}", imm8.wrapping_mul(4))
        }
        // ADD/SUB SP (0xB0-0xB3) or PUSH/POP (0xB4-0xBF).
        0xB => {
            // CBZ/CBNZ (1011 i 0 0 1 imm5 Rn): 0xB1xx CBZ, 0xB9xx CBNZ.
            if half & 0x0F00 == 0x0100 || half & 0x0F00 == 0x0900 {
                let cbz = half & 0x0800 == 0;
                let imm5 = ((half >> 3) & 0x1F) as u32;
                let rn = (half & 0x07) as usize;
                format!(
                    "{} r{rn}, {:#x}",
                    if cbz { "cbz" } else { "cbnz" },
                    pc.wrapping_add(4).wrapping_add(imm5 * 2)
                )
            } else if half & 0xFF00 == 0xB000 {
                let sub = (half >> 7) & 1 != 0;
                let name = if sub { "sub" } else { "add" };
                format!("{name} sp, sp, #{}", (imm8 & 0x7F).wrapping_mul(4))
            } else if half & 0xFF00 == 0xB200 {
                // SXTH/SXTB/UXTH/UXTB (1011 0010 0 op Rm Rd).
                let op = (half >> 6) & 0x03;
                let name = match op {
                    0 => "sxth",
                    1 => "sxtb",
                    2 => "uxth",
                    _ => "uxtb",
                };
                format!("{name} r{}, r{}", half & 0x07, (half >> 3) & 0x07)
            } else if half & 0xFFF0 == 0xB660 || half & 0xFFF0 == 0xB670 {
                // CPSIE/CPSID (interrupt mask): bit 4 = disable, `i` bit = 0x02.
                let disable = half & 0x10 != 0;
                format!("{} i", if disable { "cpsid" } else { "cpsie" })
            } else if half & 0xFF00 == 0xBA00 {
                // REV/REV16/REVSH (1011 1010 0 op Rm Rd): op = bits 7-6.
                let op = (half >> 6) & 0x03;
                let name = match op {
                    0 => "rev",
                    1 => "rev16",
                    _ => "revsh",
                };
                format!("{name} r{}, r{}", half & 0x07, (half >> 3) & 0x07)
            } else if half & 0xFF00 == 0xBE00 {
                format!("bkpt #{:#x}", half & 0xFF)
            } else if half & 0xFF00 == 0xBF00 {
                let mask = half & 0x0F;
                if mask == 0 {
                    // When the mask is zero the condition field selects a hint:
                    // 0 = NOP, 1 = YIELD, 2 = WFE, 3 = WFI, 4 = SEV.
                    let hint = match (half >> 4) & 0x0F {
                        1 => "yield",
                        2 => "wfe",
                        3 => "wfi",
                        4 => "sev",
                        _ => "nop",
                    };
                    format!("{hint}")
                } else {
                    // IT (If-Then): the mnemonic is `it` followed by `t`/`e`
                    // for each arm after the first (determined by the mask).
                    let cond = (half >> 4) & 0x0F;
                    let n = 4 - (mask as u32).trailing_zeros();
                    let mut mnem = String::from("it");
                    for i in 1..n {
                        mnem.push(if ((mask >> (4 - i)) & 1) == 0 { 't' } else { 'e' });
                    }
                    format!("{mnem} {}", cond_suffix(cond as u8))
                }
            } else {
                let push = (half >> 11) & 1 == 0;
                let mut regs = reg_list(half & 0xFF);
                if push {
                    if half & 0x100 != 0 {
                        regs.push("lr".to_string());
                    }
                    format!("push {{{}}}", regs.join(", "))
                } else {
                    if half & 0x100 != 0 {
                        regs.push("pc".to_string());
                    }
                    format!("pop {{{}}}", regs.join(", "))
                }
            }
        }
        // Multiple load/store (bit 11 = load).
        0xC => {
            let load = (half >> 11) & 1 != 0;
            let rn = ((half >> 8) & 0x07) as usize;
            let regs = reg_list(half & 0xFF);
            if load {
                format!("ldmia r{rn}!, {{{}}}", regs.join(", "))
            } else {
                format!("stmia r{rn}!, {{{}}}", regs.join(", "))
            }
        }
        // Conditional branch or SVC.
        0xD => {
            if half & 0xFF00 == 0xDF00 {
                format!("svc #{}", half & 0xFF)
            } else {
                let cond = ((half >> 8) & 0x0F) as u8;
                format!(
                    "b{} {:#x}",
                    cond_suffix(cond),
                    pc.wrapping_add(4 + ((half as i8 as i32) * 2) as u32)
                )
            }
        }
        // Unconditional branch.
        0xE => format!(
            "b {:#x}",
            pc.wrapping_add(4 + (((half & 0x7FF) as i16) * 2) as u32)
        ),
        _ => format!(".hword 0x{half:04x}"),
    }
}

/// Decode a 32-bit Thumb-2 instruction, mirroring `ArmCore::execute_wide`.
/// Canonical register name for a Thumb-2 base address register, matching the
/// Thumb-1 disassembly convention (`sp`/`lr`/`pc` rather than `r13`/`r14`/`r15`).
fn thumb2_reg_name(index: usize) -> String {
    match index {
        13 => "sp".to_string(),
        14 => "lr".to_string(),
        15 => "pc".to_string(),
        n => format!("r{n}"),
    }
}

fn disassemble_thumb2(insn: u32, pc: u32, read: &dyn Fn(u32) -> u8) -> String {
    let first = (insn >> 16) as u16;
    let second = insn as u16;

    // BL/BLX immediate (0xF000-0xF7FF prefix, second bits 14-12 = 101).
    if first & 0xF800 == 0xF000 && second & 0xD000 == 0xD000 {
        let s = ((first >> 10) & 1) as u32;
        let j1 = ((second >> 13) & 1) as u32;
        let j2 = ((second >> 11) & 1) as u32;
        let imm10 = (first & 0x3FF) as u32;
        let imm11 = (second & 0x7FF) as u32;
        let mut imm32 = (s << 24) | (j2 << 23) | (j1 << 22) | (imm10 << 12) | (imm11 << 1);
        imm32 = sign_extend(imm32, 25);
        let link = second & 0x4000 != 0;
        // Thumb-2 is 4 bytes wide, so the architectural PC used for branch
        // targets is current + 4 (like the Thumb-1 branches below).
        let target = pc.wrapping_add(4).wrapping_add(imm32);
        return if link {
            format!("bl {:#x}", target)
        } else {
            format!("blx {:#x}", target)
        };
    }

    // MOVW/MOVT (T3): `1111 0 i 10 0op imm4 | 0 imm3 Rd imm8`, where the
    // opcode is bits 7-4 = 0100 (MOVW) or 1100 (MOVT, bit 7 set). The 16-bit
    // immediate packs
    // imm4 (first bits 3-0) at bits 15-12, i (first bit 10) at bit 11,
    // imm3 (second bits 14-12) at bits 10-8, imm8 (second bits 7-0) at 7-0.
    if first & 0xF800 == 0xF000
        && first & 0x0300 == 0x0200
        && (first & 0x00F0 == 0x0040 || first & 0x00F0 == 0x00C0)
        && second & 0x8000 == 0
    {
        let rd = ((second >> 8) & 0x0F) as usize;
        let i = ((first >> 10) & 1) as u32;
        let imm4 = (first & 0x000F) as u32;
        let imm3 = ((second >> 12) & 0x7) as u32;
        let imm8 = (second & 0x00FF) as u32;
        let imm16 = (imm4 << 12) | (i << 11) | (imm3 << 8) | imm8;
        let top = first & 0x0080 != 0;
        return if top {
            format!("movt r{rd}, #0x{imm16:x}")
        } else {
            format!("movw r{rd}, #0x{imm16:x}")
        };
    }

    // MUL Rd, Rn, Rm (0xFB00 prefix, second bits 15-12 = 1111, bits 7-4 = 0).
    if first & 0xFFF0 == 0xFB00 && second & 0xF0F0 == 0xF000 {
        let rn = (first & 0x0F) as usize;
        let rd = ((second >> 8) & 0x0F) as usize;
        let rm = (second & 0x0F) as usize;
        return format!("mul r{rd}, r{rn}, r{rm}");
    }

    // MLA/MLS Rd, Rn, Rm, Ra (0xFB00 prefix, second bits 7-5 = 0, Ra != 15).
    // Bit 4 of the second halfword selects the operation (0 = MLA, 1 = MLS).
    if first & 0xFFF0 == 0xFB00 && second & 0x00E0 == 0 && second & 0xF000 != 0xF000 {
        let rn = (first & 0x0F) as usize;
        let rd = ((second >> 8) & 0x0F) as usize;
        let rm = (second & 0x0F) as usize;
        let ra = ((second >> 12) & 0x0F) as usize;
        let mnem = if second & 0x0010 != 0 { "mls" } else { "mla" };
        return format!("{mnem} r{rd}, r{rn}, r{rm}, r{ra}");
    }

    // UMULL/SMULL/UMLAL/SMLAL RdLo, RdHi, Rn, Rm. The first halfword is
    // `1111 1011 1s10 Rn` (bit 6 selects signed); the second halfword is
    // `RdLo RdHi 000a Rm` (bit 4 selects accumulate). The second halfword bits
    // 7-5 are 000, which separates them from SMLALD/SMLSLD (bits 7-5 = 110).
    if (first & 0xFFF0 == 0xFBA0 || first & 0xFFF0 == 0xFBC0)
        && second & 0x00E0 == 0
    {
        let rn = (first & 0x0F) as usize;
        let rdlo = ((second >> 12) & 0x0F) as usize;
        let rdhi = ((second >> 8) & 0x0F) as usize;
        let rm = (second & 0x0F) as usize;
        let mnem = match (first & 0x0040 != 0, second & 0x0010 != 0) {
            (false, false) => "umull",
            (false, true) => "umlal",
            (true, false) => "smull",
            (true, true) => "smlal",
        };
        return format!("{mnem} r{rdlo}, r{rdhi}, r{rn}, r{rm}");
    }

    // SDIV/UDIV Rd, Rn, Rm (0xFB90/0xFBB0 prefix, second bits 7-4 = 1111).
    if (first & 0xFFF0 == 0xFB90 || first & 0xFFF0 == 0xFBB0)
        && second & 0xF0F0 == 0xF0F0
    {
        let rn = (first & 0x0F) as usize;
        let rd = ((second >> 8) & 0x0F) as usize;
        let rm = (second & 0x0F) as usize;
        let mnem = if first & 0x0020 != 0 { "udiv" } else { "sdiv" };
        return format!("{mnem} r{rd}, r{rn}, r{rm}");
    }

    // ADDW/SUBW (T3): `1111 0 i 10 0s00 Rn | 0 imm3 Rd imm8`, where s = bit 5
    // (0 = ADDW, 1 = SUBW); the opcode is bits 7-4 = 0000 (ADDW) or 1010
    // (SUBW). The 12-bit immediate is i:imm3:imm8.
    if first & 0xF800 == 0xF000
        && first & 0x0300 == 0x0200
        && (first & 0x00F0 == 0x0000 || first & 0x00F0 == 0x00A0)
        && second & 0x8000 == 0
    {
        let rd = ((second >> 8) & 0x0F) as usize;
        let rn = (first & 0x0F) as usize;
        let i = ((first >> 10) & 1) as u32;
        let imm3 = ((second >> 12) & 0x7) as u32;
        let imm8 = (second & 0x00FF) as u32;
        let imm12 = (i << 11) | (imm3 << 8) | imm8;
        let sub = first & 0x00F0 == 0x00A0;
        return if sub {
            format!("subw r{rd}, r{rn}, #0x{imm12:x}")
        } else {
            format!("addw r{rd}, r{rn}, #0x{imm12:x}")
        };
    }

    // B.W unconditional (0xF000 prefix, second bits 15-14 = 10, bit 12 = 1).
    if first & 0xF800 == 0xF000
        && second & 0xD000 == 0x9000
    {
        let s = ((first >> 10) & 1) as u32;
        let j1 = ((second >> 13) & 1) as u32;
        let j2 = ((second >> 11) & 1) as u32;
        let imm10 = (first & 0x3FF) as u32;
        let imm11 = (second & 0x7FF) as u32;
        let mut imm32 = (s << 24) | (j2 << 23) | (j1 << 22) | (imm10 << 12) | (imm11 << 1);
        imm32 = sign_extend(imm32, 25);
        return format!("b.w {:#x}", pc.wrapping_add(4).wrapping_add(imm32));
    }

    // LDM/STM (multiple), T32: bits 15-12 = 1110, bits 11-9 = 100,
    // bit 6 = 0 (this distinguishes it from the 0xF8xx loads/stores).
    if first & 0xF000 == 0xE000 && first & 0x0E40 == 0x0800 {
        let rn = (first & 0x0F) as usize;
        let list = second;
        let load = first & 0x0010 != 0;
        let wb = first & 0x0020 != 0;
        let pre = first & 0x0100 != 0;
        let up = first & 0x0080 != 0;
        let mode = match (up, pre) {
            (true, false) => "ia",
            (true, true) => "ib",
            (false, false) => "da",
            (false, true) => "db",
        };
        let regs: Vec<String> = (0..16)
            .filter(|i| list & (1 << i) != 0)
            .map(|i| format!("r{i}"))
            .collect();
        let mnem = if load { "ldm" } else { "stm" };
        let wbs = if wb { "!" } else { "" };
        return format!("{mnem}{mode} r{rn}{wbs}, {{{}}}", regs.join(", "));
    }

    // TBB/TBH (0xE8D0-0xE8DF prefix, second bits 15-12 = 1111).
    if first & 0xFFF0 == 0xE8D0 && second & 0xF000 == 0xF000 {
        let rn = (first & 0x0F) as usize;
        let rm = (second & 0x0F) as usize;
        let mnem = if second & 0x0010 != 0 { "tbh" } else { "tbb" };
        return format!("{mnem} [r{rn}, r{rm}]");
    }

    // LDREX/STREX (exclusive access). Bits 11-8 = 1000, bit 5 = 0, and bits
    // 7-6 are 01 (word) or 11 (byte/half). Word forms: second bits 7-0 = 0,
    // Rt = bits 15-12, and the status register is bits 11-8 (store) or unused
    // (load). Byte/half forms: second bits 7-4 = 0100 (byte) / 0101 (half),
    // bits 11-8 = 1111, Rt = bits 15-12, and the status register is bits 3-0
    // (store) or unused (load).
    if first & 0x0F00 == 0x0800
        && first & 0x0020 == 0
        && ((first & 0x00C0 == 0x0040 && second & 0x00FF == 0)
            || (first & 0x00C0 == 0x00C0 && second & 0x00C0 == 0x0040))
    {
        let rn = (first & 0x0F) as usize;
        let rt = ((second >> 12) & 0x0F) as usize;
        let byte_half = first & 0x00C0 == 0x00C0;
        let size = if byte_half {
            (second >> 4) & 0x1 // 0 = byte, 1 = half
        } else {
            2 // word
        };
        let is_store = first & 0x0010 == 0;
        let rd = if is_store && byte_half {
            (second & 0x0F) as usize
        } else if is_store {
            ((second >> 8) & 0x0F) as usize
        } else {
            0
        };
        let suffix = match (size, is_store) {
            (0, false) => "ldrexb",
            (0, true) => "strexb",
            (1, false) => "ldrexh",
            (1, true) => "strexh",
            (_, false) => "ldrex",
            (_, true) => "strex",
        };
        return if is_store {
            format!("{suffix} r{rd}, r{rt}, [r{rn}]")
        } else {
            format!("{suffix} r{rt}, [r{rn}]")
        };
    }

    // LDRD/STRD (bits 15-9 = 1110100, bit 6 = 1, second bits 15-12 != 1111).
    if first & 0xFE40 == 0xE840 && second & 0xF000 != 0xF000 {
        let rn = (first & 0x0F) as usize;
        let rt = ((second >> 12) & 0x0F) as usize;
        let rt2 = ((second >> 8) & 0x0F) as usize;
        let imm8 = (second & 0xFF) as u32;
        let pre = first & 0x0100 != 0;
        let up = first & 0x0080 != 0;
        let wb = first & 0x0020 != 0;
        let load = first & 0x0010 != 0;
        let mnem = if load { "ldrd" } else { "strd" };
        let off = imm8.wrapping_mul(4);
        let off_str = if off == 0 {
            String::new()
        } else if up {
            format!(", #{off}")
        } else {
            format!(", #-{off}")
        };
        let regs = format!("r{rt}, r{rt2}");
        if pre {
            let wbs = if wb { "!" } else { "" };
            return format!("{mnem} {regs}, [r{rn}{off_str}]{wbs}");
        }
        // Post-index always carries an offset (even when imm8 is zero).
        let post = if up {
            format!("#{off}")
        } else {
            format!("#-{off}")
        };
        return format!("{mnem} {regs}, [r{rn}], {post}");
    }

    // LDR (literal): LDR Rt, [PC, #imm12] (0xF8DF).
    if first == 0xF8DF {
        let rd = ((second >> 12) & 0x0F) as usize;
        let imm12 = second & 0x0FFF;
        return format!("ldr r{rd}, [pc, #0x{imm12:x}]");
    }

    // LDR/STR (immediate 12-bit, 0xF8D0-0xF8DF / 0xF8C0-0xF8CF).
    if first & 0xFFF0 == 0xF8D0 || first & 0xFFF0 == 0xF8C0 {
        let load = first & 0x0010 != 0;
        let rn = (first & 0x000F) as usize;
        let rd = ((second >> 12) & 0x0F) as usize;
        let imm12 = (second & 0x0FFF) as u32;
        return if load {
            format!("ldr r{rd}, [{}, #0x{imm12:x}]", thumb2_reg_name(rn))
        } else {
            format!("str r{rd}, [{}, #0x{imm12:x}]", thumb2_reg_name(rn))
        };
    }

    // LDRB (literal): LDRB Rt, [PC, #imm12] (0xF89F).
    if first == 0xF89F {
        let rd = ((second >> 12) & 0x0F) as usize;
        let imm12 = second & 0x0FFF;
        return format!("ldrb r{rd}, [pc, #0x{imm12:x}]");
    }

    // LDRB/STRB (0xF890-0xF89F / 0xF880-0xF88F).
    if first & 0xFFF0 == 0xF890 || first & 0xFFF0 == 0xF880 {
        let load = first & 0x0010 != 0;
        let rn = (first & 0x000F) as usize;
        let rd = ((second >> 12) & 0x0F) as usize;
        let imm12 = (second & 0x0FFF) as u32;
        return if load {
            format!("ldrb r{rd}, [{}, #0x{imm12:x}]", thumb2_reg_name(rn))
        } else {
            format!("strb r{rd}, [{}, #0x{imm12:x}]", thumb2_reg_name(rn))
        };
    }

    // LDRH (literal): LDRH Rt, [PC, #imm12] (0xF8BF).
    if first == 0xF8BF {
        let rd = ((second >> 12) & 0x0F) as usize;
        let imm12 = second & 0x0FFF;
        return format!("ldrh r{rd}, [pc, #0x{imm12:x}]");
    }

    // LDRH/STRH (0xF8B0-0xF8BF / 0xF8A0-0xF8AF).
    if first & 0xFFF0 == 0xF8B0 || first & 0xFFF0 == 0xF8A0 {
        let load = first & 0x0010 != 0;
        let rn = (first & 0x000F) as usize;
        let rd = ((second >> 12) & 0x0F) as usize;
        let imm12 = (second & 0x0FFF) as u32;
        return if load {
            format!("ldrh r{rd}, [{}, #0x{imm12:x}]", thumb2_reg_name(rn))
        } else {
            format!("strh r{rd}, [{}, #0x{imm12:x}]", thumb2_reg_name(rn))
        };
    }

    // LDR/STR (immediate 8-bit, with optional writeback). First halfword is
    // `1111 1000 0 size L Rn`; second is `Rt 1 P U W imm8` (bit 10 = pre-index,
    // bit 9 = add, bit 8 = writeback). Post-index forms (P = 0) always write
    // back. Distinguished from register-offset encodings by bit 11 of the
    // second halfword being set.
    if first & 0xFF80 == 0xF800 && second & 0x0800 != 0 {
        let rn = (first & 0x000F) as usize;
        let rt = ((second >> 12) & 0x0F) as usize;
        let imm8 = (second & 0x00FF) as u32;
        let pre = second & 0x0400 != 0;
        let up = second & 0x0200 != 0;
        let wb = second & 0x0100 != 0;
        let load = first & 0x0010 != 0;
        let width = (first >> 5) & 0x03;
        let mnem = match (width, load) {
            (0, false) => "strb",
            (0, true) => "ldrb",
            (1, false) => "strh",
            (1, true) => "ldrh",
            (_, false) => "str",
            (_, true) => "ldr",
        };
        let offset = if up {
            format!("#0x{imm8:x}")
        } else {
            format!("#-0x{imm8:x}")
        };
        let suffix = if pre {
            if wb {
                format!(", {offset}]!")
            } else {
                format!(", {offset}]")
            }
        } else {
            format!("], {offset}")
        };
        return format!("{mnem} r{rt}, [{}{suffix}", thumb2_reg_name(rn));
    }

    // Register-offset loads/stores (0xF800-0xF85F): [Rn, Rm {, LSL #imm2}].
    if first & 0xFFF0 >= 0xF800 && first & 0xFFF0 <= 0xF85F && second & 0x0FC0 == 0 {
        let rn = (first & 0x000F) as usize;
        let rt = ((second >> 12) & 0x0F) as usize;
        let rm = (second & 0x000F) as usize;
        let shift = (second >> 4) & 0x03;
        let load = first & 0x0010 != 0;
        let width = (first >> 5) & 0x03;
        let mnem = match (width, load) {
            (0, false) => "strb",
            (0, true) => "ldrb",
            (1, false) => "strh",
            (1, true) => "ldrh",
            (_, false) => "str",
            (_, true) => "ldr",
        };
        let shift_str = if shift == 0 {
            String::new()
        } else {
            format!(", lsl #{shift}")
        };
        return format!("{mnem} r{rt}, [{}, r{rm}{shift_str}]", thumb2_reg_name(rn));
    }

    // LDRSB/LDRSH wide forms. Register offset: first = 0xF910|Rn / 0xF930|Rn
    // (bit 7 = 0), second = `Rt 0000 00shift Rm`. Immediate and PC-relative
    // literal (Rn = PC): first = 0xF990|Rn / 0xF9B0|Rn (bit 7 = 1),
    // second = `Rt imm12`.
    let ldr_sb_reg = first & 0xFFF0 == 0xF910 && second & 0x0FC0 == 0;
    let ldr_sh_reg = first & 0xFFF0 == 0xF930 && second & 0x0FC0 == 0;
    let ldr_sb_imm = first & 0xFFF0 == 0xF990;
    let ldr_sh_imm = first & 0xFFF0 == 0xF9B0;
    if ldr_sb_reg || ldr_sh_reg || ldr_sb_imm || ldr_sh_imm {
        let rt = ((second >> 12) & 0x0F) as usize;
        let rn = (first & 0x0F) as usize;
        let mnem = if ldr_sb_reg || ldr_sb_imm { "ldrsb" } else { "ldrsh" };
        let is_reg = first & 0x0080 == 0;
        if is_reg {
            let rm = (second & 0x0F) as usize;
            let shift = (second >> 4) & 0x03;
            let shift_str = if shift == 0 {
                String::new()
            } else {
                format!(", lsl #{shift}")
            };
            return format!("{mnem} r{rt}, [{}, r{rm}{shift_str}]", thumb2_reg_name(rn));
        }
        let imm12 = second & 0x0FFF;
        return format!("{mnem} r{rt}, [{}, #0x{imm12:x}]", thumb2_reg_name(rn));
    }

    // Memory barriers DMB/DSB/ISB (0xF3BF 0x8F5F/8F6F/8F7F).
    if first == 0xF3BF && second & 0xFFF0 >= 0x8F50 && second & 0xFFF0 <= 0x8F70 {
        let name = match (second >> 4) & 0x0F {
            0x6 => "dsb",
            0x7 => "isb",
            _ => "dmb",
        };
        return format!("{name} sy");
    }

    // Hint instructions (0xF3AF 0x8000-0x8004): NOP.W / YIELD.W / WFE.W /
    // WFI.W / SEV.W.
    if first == 0xF3AF && second & 0xFFF0 == 0x8000 {
        let name = match second & 0x0F {
            0 => "nop.w",
            1 => "yield.w",
            2 => "wfe.w",
            3 => "wfi.w",
            _ => "sev.w",
        };
        return name.to_string();
    }

    // MRS Rd, <apsr|ipsr|epsr|xpsr> (0xF3EF 0x8Rd0SYSm).
    if first == 0xF3EF && second & 0x00F0 == 0x0000 {
        let rd = (second >> 8) & 0x0F;
        let name = match second & 0x0F {
            0 => "apsr",
            1 | 5 => "ipsr",
            2 | 6 => "epsr",
            _ => "xpsr",
        };
        return format!("mrs r{rd}, {name}");
    }
    // MSR <apsr|xpsr>_<mask>, Rm (0xF380-0xF38F 0x8maskSYSm). Second bits 15-12
    // are 1000, which separates it from USAT (0xF380|Rn, bits 15-12 = 0).
    if first & 0xFFF0 == 0xF380
        && second & 0xF000 == 0x8000
        && second & 0x00F0 == 0x0000
    {
        let mask = (second >> 8) & 0x0F;
        let rm = first & 0x0F;
        let mut mask_str = String::new();
        if mask & 0x8 != 0 {
            mask_str.push_str("nzcvq");
        }
        if mask & 0x4 != 0 {
            mask_str.push('g');
        }
        if mask & 0x1 != 0 {
            mask_str.push_str("c");
        }
        if mask_str.is_empty() {
            mask_str.push_str("nzcvq");
        }
        return format!("msr xpsr_{mask_str}, r{rm}");
    }

    // CLZ Rd, Rm (0xFAB0-0xFABF, second halfword `0xF0Rd 1000 0Rm`).
    if first & 0xFFF0 == 0xFAB0 && second & 0x00F8 == 0x0080 {
        let rd = ((second >> 8) & 0x0F) as usize;
        let rm = (first & 0x0F) as usize;
        return format!("clz r{rd}, r{rm}");
    }

    // REV/REV16/REVSH/RBIT wide forms. First halfword is
    // `1111 1010 1001 Rm` (0xFA90-0xFA9F); second halfword is
    // `1111 Rd 10op2 Rm`, where bits 7-4 select the operation and Rm is
    // duplicated in both halfwords' low nibble.
    if first & 0xFFF0 == 0xFA90 && second & 0xF000 == 0xF000 {
        let rd = ((second >> 8) & 0x0F) as usize;
        let rm = (first & 0x0F) as usize;
        let mnem = match (second >> 4) & 0x0F {
            0x8 => "rev.w",
            0x9 => "rev16.w",
            0xA => "rbit",
            _ => "revsh.w",
        };
        return format!("{mnem} r{rd}, r{rm}");
    }

    // SXTH/UXTH/SXTB/UXTB wide forms (with optional ROR #(8*RR)). First
    // halfword is `1111 1010 0op1 1111` (0xFA0F/0xFA1F/0xFA4F/0xFA5F),
    // selecting the operation in bits 7-4; second halfword is
    // `1111 Rd 10RR Rm` (bits 7-6 = 10, RR in bits 5-4).
    if first & 0xFF0F == 0xFA0F && second & 0x00C0 == 0x0080 {
        let op = (first >> 4) & 0x7;
        if op == 0 || op == 1 || op == 4 || op == 5 {
            let rd = ((second >> 8) & 0x0F) as usize;
            let rm = (second & 0x0F) as usize;
            let mnem = match op {
                0 => "sxth.w",
                1 => "uxth.w",
                4 => "sxtb.w",
                _ => "uxtb.w",
            };
            let rotate = ((second >> 4) & 0x03) * 8;
            return if rotate == 0 {
                format!("{mnem} r{rd}, r{rm}")
            } else {
                format!("{mnem} r{rd}, r{rm}, ror #{rotate}")
            };
        }
    }

    // QADD/QSUB/QDADD/QDSUB Rd, Rm, Rn (`0xFA80|Rn` prefix). Second halfword
    // `1111 Rd op Rm`; bits 7-4 select QADD=1000 / QDADD=1001 / QSUB=1010 /
    // QDSUB=1011. Checked before SXTAB (which shares `first & 0xFF0F == 0xFA00`).
    if first & 0xFFF0 == 0xFA80 {
        let rd = ((second >> 8) & 0x0F) as usize;
        let rm = (second & 0x0F) as usize;
        let rn = (first & 0x0F) as usize;
        let mnem = match (second >> 4) & 0x0F {
            0x8 => "qadd",
            0x9 => "qdadd",
            0xA => "qsub",
            _ => "qdsub",
        };
        return format!("{mnem} r{rd}, r{rm}, r{rn}");
    }

    // SEL Rd, Rn, Rm (`0xFAA0|Rn`, second `1111 Rd 1000 Rm`). Checked before
    // SXTAB (which shares `first & 0xFF0F == 0xFA00`).
    if first & 0xFFF0 == 0xFAA0 && second & 0x00F0 == 0x0080 {
        let rd = ((second >> 8) & 0x0F) as usize;
        let rm = (second & 0x0F) as usize;
        let rn = (first & 0x0F) as usize;
        return format!("sel r{rd}, r{rn}, r{rm}");
    }

    // SXTAH/SXTB/UXTAH/UXTB Rd, Rn, Rm (extend and add). First halfword
    // `0xFA00|Rn` (SXTAH) / `0xFA10|Rn` (UXTAH) / `0xFA40|Rn` (SXTAB) /
    // `0xFA50|Rn` (UXTAB), with Rn != PC; second `1111 Rd 1000 Rm`. The
    // operation is encoded in bits 7-4 of the first halfword.
    let ext_op = (first >> 4) & 0x0F;
    if first & 0xFF00 == 0xFA00
        && second & 0x00F0 == 0x0080
        && (ext_op == 0 || ext_op == 1 || ext_op == 4 || ext_op == 5)
        && first & 0x0F != 0x0F
    {
        let rd = ((second >> 8) & 0x0F) as usize;
        let rm = (second & 0x0F) as usize;
        let rn = (first & 0x0F) as usize;
        let mnem = match ext_op {
            0 => "sxtah",
            1 => "uxtah",
            4 => "sxtab",
            _ => "uxtab",
        };
        return format!("{mnem} r{rd}, r{rn}, r{rm}");
    }

    // SSAT Rd, #sat, Rm {, shift} (`0xF300|Rn` prefix, bit 5 = ASR). Second
    // `0 sh sat Rd 00 imm5`; sat = imm5 + 1, shift = bit12:bits7-6. Second bit
    // 15 is clear, which separates SSAT from B<cond>.W (which sets bit 15).
    if first & 0xFFC0 == 0xF300 && second & 0x8000 == 0 {
        let rd = ((second >> 8) & 0x0F) as usize;
        let rn = (first & 0x0F) as usize;
        let sat = (second & 0x1F) + 1;
        let shift = ((((second >> 12) & 1) << 2) | ((second >> 6) & 0x3)) as u32;
        let asr = first & 0x0020 != 0;
        let shift_str = if shift == 0 {
            String::new()
        } else {
            format!(", {} #{shift}", if asr { "asr" } else { "lsl" })
        };
        return format!("ssat r{rd}, #{sat}, r{rn}{shift_str}");
    }

    // USAT Rd, #sat, Rm {, shift} (`0xF380|Rn` prefix, bit 5 = ASR). Second
    // `0 sh sat Rd 00 imm5`; sat = imm5 (0-31), shift = bit12:bits7-6. Second
    // bit 15 is clear, which separates USAT from B<cond>.W and MSR.
    if first & 0xFFC0 == 0xF380 && second & 0x8000 == 0 {
        let rd = ((second >> 8) & 0x0F) as usize;
        let rn = (first & 0x0F) as usize;
        let sat = second & 0x1F;
        let shift = ((((second >> 12) & 1) << 2) | ((second >> 6) & 0x3)) as u32;
        let asr = first & 0x0020 != 0;
        let shift_str = if shift == 0 {
            String::new()
        } else {
            format!(", {} #{shift}", if asr { "asr" } else { "lsl" })
        };
        return format!("usat r{rd}, #{sat}, r{rn}{shift_str}");
    }

    // SMULxy/SMLAxy Rd, Rn, Rm {, Ra} (signed 16x16 multiply / multiply-
    // accumulate, `0xFB10|Rn`). Second `Ra Rd 00 yx Rm`: bits 15-12 = Ra (F =
    // SMUL, no accumulate), bit 5 = Rn half (B/T), bit 4 = Rm half (B/T).
    if first & 0xFFF0 == 0xFB10 {
        let rn = (first & 0x0F) as usize;
        let rd = ((second >> 8) & 0x0F) as usize;
        let rm = (second & 0x0F) as usize;
        let ra = (second >> 12) & 0x0F;
        let x = if second & 0x0020 != 0 { 't' } else { 'b' };
        let y = if second & 0x0010 != 0 { 't' } else { 'b' };
        return if ra == 0xF {
            format!("smul{x}{y} r{rd}, r{rn}, r{rm}")
        } else {
            format!("smla{x}{y} r{rd}, r{rn}, r{rm}, r{ra}")
        };
    }

    // SMMUL/SMMLA/SMMLS Rd, Rn, Rm {, Ra} (signed most-significant word
    // multiply, `0xFB50|Rn` / `0xFB60|Rn`). Second `Ra Rd 0000 Rm`.
    if first & 0xFFF0 == 0xFB50 || first & 0xFFF0 == 0xFB60 {
        let rn = (first & 0x0F) as usize;
        let rd = ((second >> 8) & 0x0F) as usize;
        let rm = (second & 0x0F) as usize;
        let ra = (second >> 12) & 0x0F;
        let sub = first & 0xFFF0 == 0xFB60;
        return if ra == 0xF {
            format!("smmul r{rd}, r{rn}, r{rm}")
        } else if sub {
            format!("smmls r{rd}, r{rn}, r{rm}, r{ra}")
        } else {
            format!("smmla r{rd}, r{rn}, r{rm}, r{ra}")
        };
    }

    // SMLALD/SMLSLD RdLo, RdHi, Rn, Rm (signed 64-bit multiply-accumulate,
    // `0xFBC0|Rn` / `0xFBD0|Rn`, second bits 7-4 = 1100). Second `RdLo RdHi
    // 1100 Rm`; separated from UMULL/SMULL (second bits 7-4 = 0000).
    if (first & 0xFFF0 == 0xFBC0 || first & 0xFFF0 == 0xFBD0)
        && second & 0x00F0 == 0x00C0
    {
        let rn = (first & 0x0F) as usize;
        let rdlo = ((second >> 12) & 0x0F) as usize;
        let rdhi = ((second >> 8) & 0x0F) as usize;
        let rm = (second & 0x0F) as usize;
        let mnem = if first & 0xFFF0 == 0xFBD0 {
            "smlsld"
        } else {
            "smlald"
        };
        return format!("{mnem} r{rdlo}, r{rdhi}, r{rn}, r{rm}");
    }

    // Bit-field extract/insert/clear: SBFX (0xF340), BFI (0xF360 with
    // Rn != PC), BFC (0xF36F), UBFX (0xF3C0).
    // Bit-field instructions (SBFX/UBFX/BFI/BFC) always encode bit 15 of the
    // second halfword as 0, which separates them from the shared 0xF3xx first
    // halfword space (e.g. `ble.w` = 0xF340 0x8000 sets bit 15).
    if second & 0x8000 == 0
        && second & 0x0020 == 0
        && (first & 0xFFF0 == 0xF340 || first & 0xFFF0 == 0xF360
            || first == 0xF36F || first & 0xFFF0 == 0xF3C0)
    {
        let rn = (first & 0x0F) as usize;
        let rd = ((second >> 8) & 0x0F) as usize;
        let lsb = ((((second >> 12) & 0x7) as u32) << 2) | ((second >> 6) & 0x3) as u32;
        let mnem = if first & 0xFFF0 == 0xF340 {
            "sbfx"
        } else if first & 0xFFF0 == 0xF3C0 {
            "ubfx"
        } else if first == 0xF36F {
            "bfc"
        } else {
            "bfi"
        };
        // SBFX/UBFX encode the width as `lsb5 + 1`; BFC/BFI encode the field's
        // most-significant bit in `lsb5` (width = msb - lsb + 1).
        let width = if mnem == "sbfx" || mnem == "ubfx" {
            (second & 0x1F) as u32 + 1
        } else {
            (second & 0x1F) as u32 + 1 - lsb
        };
        return if mnem == "bfc" {
            format!("bfc r{rd}, #{lsb}, #{width}")
        } else if mnem == "bfi" {
            format!("bfi r{rd}, r{rn}, #{lsb}, #{width}")
        } else {
            format!("{mnem} r{rd}, r{rn}, #{lsb:#x}, #{width:#x}")
        };
    }

    // B<cond>.W conditional branch (0xF000 prefix, second bits 15-14 = 10,
    // bit 12 = 0). First halfword is `1111 0 S cond imm6`; second halfword is
    // `10 J1 0 J2 imm10`. The 20-bit offset is S:J1:imm6:J2:imm10:0. This is
    // checked after the specific 0xF3xx/0xFAxx handlers (memory barriers,
    // hints, MRS/MSR, bit-field) because they share the `11110` top-five bits;
    // it precedes the modified-immediate data-processing handler, whose first
    // halfword (bit 9 = 0, S = 0) would otherwise claim forward `B<cond>.W`.
    if first & 0xF800 == 0xF000
        && second & 0xD000 == 0x8000
    {
        let cond = ((first >> 6) & 0x0F) as u8;
        let s = ((first >> 10) & 1) as u32;
        let j1 = ((second >> 11) & 1) as u32;
        let imm6 = (first & 0x3F) as u32;
        let j2 = ((second >> 10) & 1) as u32;
        let imm10 = (second & 0x3FF) as u32;
        let mut imm32 = (s << 19) | (j1 << 18) | (imm6 << 12) | (j2 << 11) | (imm10 << 1);
        imm32 = sign_extend(imm32, 20);
        return format!("b{}w {:#x}", cond_suffix(cond), pc.wrapping_add(4).wrapping_add(imm32));
    }

    // Data processing (modified immediate): AND/BIC/ORR/ORN/EOR/ADD/SUB/RSB
    // plus MOV/MVN/TST/TEQ/CMP/CMN. Bits 15-11 = 11110, bit 9 = 0.
    if first & 0xFA00 == 0xF000 {
        let op = ((first >> 5) & 0xF) as u32;
        let set_flags = first & 0x0010 != 0;
        let rn = (first & 0x0F) as usize;
        let i = ((first >> 10) & 1) as u32;
        let imm3 = ((second >> 12) & 0x7) as u32;
        let rd = ((second >> 8) & 0x0F) as usize;
        let imm8 = (second & 0xFF) as u32;
        let imm12 = (i << 11) | (imm3 << 8) | imm8;
        let (imm, _) = crate::arm::thumb_expand_imm12(imm12, false);
        let mnem = match op {
            0 if set_flags && rd == 0xF => "tst",
            4 if set_flags && rd == 0xF => "teq",
            8 if set_flags && rd == 0xF => "cmn",
            13 if set_flags && rd == 0xF => "cmp",
            2 if rn == 0xF => "mov",
            3 if rn == 0xF => "mvn",
            0 => "and",
            1 => "bic",
            2 => "orr",
            3 => "orn",
            4 => "eor",
            8 => "add",
            13 => "sub",
            14 => "rsb",
            _ => return format!(".word 0x{:08x}", ((first as u32) << 16) | second as u32),
        };
        let s = if set_flags { "s" } else { "" };
        let imm_str = format!("#0x{imm:x}");
        return match mnem {
            "tst" | "teq" | "cmn" | "cmp" => format!("{mnem} r{rn}, {imm_str}"),
            "mov" | "mvn" => format!("{mnem}{s} r{rd}, {imm_str}"),
            _ => format!("{mnem}{s} r{rd}, r{rn}, {imm_str}"),
        };
    }

    // Data processing (register): AND/BIC/ORR/ORN/EOR/ADD/ADC/SBC/SUB/RSB plus
    // MOV/MVN/TST/TEQ/CMP/CMN. Bits 15-9 = 1110101.
    if first & 0xFE00 == 0xEA00 {
        let op = (((first >> 8) & 1) << 3) | ((first >> 5) & 0x7);
        let set_flags = first & 0x0010 != 0;
        let rn = (first & 0x0F) as usize;
        // MOV/MVN (ORR/ORN with Rn=PC) share the general data-processing
        // layout: Rd = bits 11-8, Rm = bits 3-0.
        let rd = ((second >> 8) & 0x0F) as usize;
        let rm = (second & 0x0F) as usize;
        // The shifted-register amount is `imm3:imm2`, where imm3 = bits 14-12
        // and imm2 = bits 7-6; a zero amount means LSR/ASR #32 or RRX.
        let imm5 = (((second >> 12) & 0x7) << 2) | ((second >> 6) & 0x3);
        let shift_type = (second >> 4) & 0x03;
        let shift_name = match shift_type {
            0 => "lsl",
            1 => "lsr",
            2 => "asr",
            _ => "ror",
        };
        let shift = match (shift_type, imm5) {
            (0, 0) => String::new(),
            (1, 0) => format!(", {shift_name} #32"),
            (2, 0) => format!(", {shift_name} #32"),
            (3, 0) => ", rrx".to_string(),
            (_, a) => format!(", {shift_name} #{a}"),
        };
        let rd_name = format!("r{rd}");
        let mnem = match op {
            0 if set_flags && rd == 0xF => "tst",
            4 if set_flags && rd == 0xF => "teq",
            8 if set_flags && rd == 0xF => "cmn",
            13 if set_flags && rd == 0xF => "cmp",
            2 if rn == 0xF => "mov",
            3 if rn == 0xF => "mvn",
            0 => "and",
            1 => "bic",
            2 => "orr",
            3 => "orn",
            4 => "eor",
            8 => "add",
            10 => "adc",
            11 => "sbc",
            13 => "sub",
            14 => "rsb",
            _ => return format!(".word 0x{:08x}", ((first as u32) << 16) | second as u32),
        };
        let s = if set_flags { "s" } else { "" };
        return match mnem {
            "tst" | "teq" | "cmn" | "cmp" => {
                format!("{mnem} r{rn}, r{rm}{shift}")
            }
            "mov" | "mvn" => {
                format!("{mnem}{s} {rd_name}, r{rm}{shift}")
            }
            _ => {
                format!("{mnem}{s} {rd_name}, r{rn}, r{rm}{shift}")
            }
        };
    }

    let _ = read;
    format!(".word 0x{insn:08x}")
}

/// Format the ARM operand-2 register (optionally shifted) for a data
/// processing / single-transfer instruction, mirroring `ArmCore::arm_operand2`.
fn arm_operand2_shift_str(insn: u32) -> String {
    let rm = (insn & 0x0F) as usize;
    let shift_type = (insn >> 5) & 0x03;
    let shift_name = match shift_type {
        0 => "lsl",
        1 => "lsr",
        2 => "asr",
        _ => "ror",
    };
    if insn & 0x10 != 0 {
        // Register-specified shift: RS in bits 11-8.
        let rs = ((insn >> 8) & 0x0F) as usize;
        return format!("r{rm}, {shift_name} r{rs}");
    }
    let amount = (insn >> 7) & 0x1F;
    match (shift_type, amount) {
        (0, 0) => format!("r{rm}"),
        (1, 0) => format!("r{rm}, lsr #32"),
        (2, 0) => format!("r{rm}, asr #32"),
        (3, 0) => format!("r{rm}, rrx"),
        (0, a) => format!("r{rm}, lsl #{a}"),
        (1, a) => format!("r{rm}, lsr #{a}"),
        (2, a) => format!("r{rm}, asr #{a}"),
        (_, a) => format!("r{rm}, ror #{a}"),
    }
}

/// Decode a single A32 (ARM-state) instruction for the disassembly view.
fn disassemble_a32(pc: u32, read: &dyn Fn(u32) -> u8) -> String {
    let insn = read(pc) as u32
        | ((read(pc + 1) as u32) << 8)
        | ((read(pc + 2) as u32) << 16)
        | ((read(pc + 3) as u32) << 24);
    let cond = (insn >> 28) as u8;
    let cc = cond_suffix(cond);
    let rn = ((insn >> 16) & 0x0F) as usize;
    let rd = ((insn >> 12) & 0x0F) as usize;
    let rm = (insn & 0x0F) as usize;
    let rname = |r: usize| format!("r{r}");
    let pc_base = pc.wrapping_add(8);
    // Branch / branch-with-link: 101x (cond != 0xF).
    if insn & 0x0E00_0000 == 0x0A00_0000 {
        let link = insn & (1 << 24) != 0;
        let offset = ((insn & 0x00FF_FFFF) << 8) as i32 >> 8;
        let target = pc_base.wrapping_add((offset << 2) as u32);
        let mnem = if link { "bl" } else { "b" };
        return format!("{mnem}{cc} {target:#x}");
    }
    // BX / BLX (register).
    if insn & 0x0FFF_FFF0 == 0x012F_FF10 || insn & 0x0FFF_FFF0 == 0x012F_FF30 {
        let link = insn & 0x20 != 0;
        let mnem = if link { "blx" } else { "bx" };
        return format!("{mnem}{cc} {}", rname(rm));
    }
    // Block data transfer: 100x.
    if insn & 0x0E00_0000 == 0x0800_0000 {
        let pre = insn & (1 << 24) != 0;
        let add = insn & (1 << 23) != 0;
        let writeback = insn & (1 << 21) != 0;
        let load = insn & (1 << 20) != 0;
        let list = insn & 0xFFFF;
        let regs: Vec<String> = (0..16).filter(|i| list & (1 << i) != 0).map(rname).collect();
        let mode = match (add, pre) {
            (true, false) => "ia",
            (true, true) => "ib",
            (false, false) => "da",
            (false, true) => "db",
        };
        let mnem = if load { "ldm" } else { "stm" };
        let wb = if writeback { "!" } else { "" };
        return format!("{mnem}{mode}{cc} {}{wb}, {{{}}}", rname(rn), regs.join(", "));
    }
    // Single data transfer (word/byte): 01xx.
    if insn & 0x0C00_0000 == 0x0400_0000 {
        let reg_off = insn & (1 << 25) != 0;
        let pre = insn & (1 << 24) != 0;
        let add = insn & (1 << 23) != 0;
        let byte = insn & (1 << 22) != 0;
        let writeback = insn & (1 << 21) != 0;
        let load = insn & (1 << 20) != 0;
        let mnem = match (load, byte) {
            (true, false) => "ldr",
            (true, true) => "ldrb",
            (false, false) => "str",
            (false, true) => "strb",
        };
        let offset = if reg_off {
            arm_operand2_shift_str(insn)
        } else {
            format!("#{}", insn & 0x0FFF)
        };
        let wb = if writeback { "!" } else { "" };
        // Pre-indexed: [rn, offset]; post-indexed: [rn], offset. Positive
        // offsets omit the `+` sign per ARM assembly convention.
        let off_str = if add { offset } else { format!("-{offset}") };
        if pre {
            return format!("{mnem}{cc} {}, [{}, {off_str}]{wb}", rname(rd), rname(rn));
        }
        return format!("{mnem}{cc} {}, [{}{wb}], {off_str}", rname(rd), rname(rn));
    }
    // MOVW / MOVT.
    if insn & 0x0FF0_0000 == 0x0300_0000 || insn & 0x0FF0_0000 == 0x0340_0000 {
        let movt = insn & 0x0040_0000 != 0;
        let imm16 = ((insn >> 4) & 0xF000) | (insn & 0x0FFF);
        return format!("{}{cc} {}, #{imm16:#x}", if movt { "movt" } else { "movw" }, rname(rd));
    }
    // MRS Rd, CPSR.
    if insn & 0x0FBF_0FFF == 0x010F_0000 {
        return format!("mrs{cc} {}, cpsr", rname(rd));
    }
    // MSR CPSR_<fields>, Rm / #imm.
    if insn & 0x0FB0_FFF0 == 0x0120_F000 {
        let fields = (insn >> 16) & 0x0F;
        return format!("msr{cc} cpsr_{fields:#x}, {}", rname(rm));
    }
    if insn & 0x0FB0_F000 == 0x0320_F000 {
        let fields = (insn >> 16) & 0x0F;
        let imm = (insn & 0xFF).rotate_right(((insn >> 8) & 0x0F) * 2);
        return format!("msr{cc} cpsr_{fields:#x}, #{imm:#x}");
    }
    // Multiply / multiply-accumulate.
    if insn & 0x0FC0_00F0 == 0x0000_0090 {
        let acc = insn & (1 << 21) != 0;
        let rd = ((insn >> 16) & 0x0F) as usize;
        let rn = ((insn >> 12) & 0x0F) as usize;
        let rs = ((insn >> 8) & 0x0F) as usize;
        if acc {
            return format!("mla{cc} {}, {}, {}, {}", rname(rd), rname(rm), rname(rs), rname(rn));
        }
        return format!("mul{cc} {}, {}, {}", rname(rd), rname(rm), rname(rs));
    }
    // Long multiply.
    if insn & 0x0F80_00F0 == 0x0080_0090 {
        let rdhi = ((insn >> 16) & 0x0F) as usize;
        let rdlo = ((insn >> 12) & 0x0F) as usize;
        let rs = ((insn >> 8) & 0x0F) as usize;
        let signed = insn & (1 << 22) != 0;
        let acc = insn & (1 << 21) != 0;
        let mnem = match (signed, acc) {
            (false, false) => "umull",
            (false, true) => "umlal",
            (true, false) => "smull",
            (true, true) => "smlal",
        };
        return format!("{mnem}{cc} {}, {}, {}, {}", rname(rdlo), rname(rdhi), rname(rm), rname(rs));
    }
    // Halfword / signed transfer.
    if insn & 0x0E00_0090 == 0x0000_0090 && insn & 0x60 != 0 {
        let load = insn & (1 << 20) != 0;
        let sh = (insn >> 5) & 0x03;
        let mnem = match (load, sh) {
            (true, 1) => "ldrh",
            (true, 2) => "ldrsb",
            (true, _) => "ldrsh",
            (false, 1) => "strh",
            (false, 2) => "ldrd",
            (false, _) => "strd",
        };
        // LDRD/STRD operate on a register pair (Rd, Rd+1); the register
        // offset form places Rm in bits 11-8 (not bits 3-0 as LDRH/STRH).
        let reg_pair = !load && sh >= 2;
        let reg_str = if reg_pair {
            format!("r{rd}, r{}", rd + 1)
        } else {
            rname(rd)
        };
        let pre = insn & (1 << 24) != 0;
        let add = insn & (1 << 23) != 0;
        // Positive offsets omit the `+` sign per ARM assembly convention.
        let off_str = if insn & (1 << 22) != 0 {
            let imm = ((insn >> 4) & 0xF0) | (insn & 0x0F);
            if add { format!("#{imm}") } else { format!("#-{imm}") }
        } else if add {
            rname(if reg_pair {
                ((insn >> 8) & 0x0F) as usize
            } else {
                (insn & 0x0F) as usize
            })
        } else {
            format!("-{}", rname(if reg_pair {
                ((insn >> 8) & 0x0F) as usize
            } else {
                (insn & 0x0F) as usize
            }))
        };
        if pre {
            return format!("{mnem}{cc} {reg_str}, [{}, {off_str}]", rname(rn));
        }
        return format!("{mnem}{cc} {reg_str}, [{}], {off_str}", rname(rn));
    }
    // CLZ.
    if insn & 0x0FFF_0FF0 == 0x016F_0F10 {
        return format!("clz{cc} {}, {}", rname(rd), rname(rm));
    }
    // SVC (formerly SWI).
    if insn & 0x0F00_0000 == 0x0F00_0000 {
        return format!("svc{cc} #{:#x}", insn & 0x00FF_FFFF);
    }
    // Data processing (register/immediate).
    if insn & 0x0C00_0000 == 0 {
        let opcode = (insn >> 21) & 0x0F;
        let set_flags = insn & (1 << 20) != 0;
        let s = if set_flags { "s" } else { "" };
        let name = match opcode {
            0x0 => "and", 0x1 => "eor", 0x2 => "sub", 0x3 => "rsb",
            0x4 => "add", 0x5 => "adc", 0x6 => "sbc", 0x7 => "rsc",
            0x8 => "tst", 0x9 => "teq", 0xA => "cmp", 0xB => "cmn",
            0xC => "orr", 0xD => "mov", 0xE => "bic", _ => "mvn",
        };
        let is_imm = insn & (1 << 25) != 0;
        let operand = if is_imm {
            let imm = insn & 0xFF;
            let rotate = ((insn >> 8) & 0x0F) * 2;
            format!("#{:#x}", imm.rotate_right(rotate))
        } else {
            arm_operand2_shift_str(insn)
        };
        if opcode >= 8 && opcode <= 11 {
            // Compare/test: no destination.
            return format!("{name}{cc} {}, {operand}", rname(rn));
        }
        let rd_s = if opcode == 0xD || opcode == 0xF {
            // MOV/MVN have no Rn.
            format!("{}", rname(rd))
        } else {
            format!("{}, {}", rname(rd), rname(rn))
        };
        return format!("{name}{s}{cc} {rd_s}, {operand}");
    }
    format!(".word 0x{insn:08x}")
}

fn cond_suffix(cond: u8) -> &'static str {
    match cond {
        0 => "eq", 1 => "ne", 2 => "cs", 3 => "cc", 4 => "mi", 5 => "pl",
        6 => "vs", 7 => "vc", 8 => "hi", 9 => "ls", 0xA => "ge", 0xB => "lt",
        0xC => "gt", 0xD => "le", _ => "",
    }
}

fn disassemble_xtensa(pc: u32, read: &dyn Fn(u32) -> u8) -> String {
    let b0 = read(pc) as u32;
    let b1 = read(pc + 1) as u32;
    let b2 = read(pc + 2) as u32;
    let insn = (b2 << 16) | (b1 << 8) | b0;
    let op0 = (insn & 0x0F) as u8;
    let op1 = ((insn >> 4) & 0x0F) as u8;
    let r = ((insn >> 8) & 0x0F) as usize;
    let s = ((insn >> 12) & 0x0F) as usize;
    let t = ((insn >> 16) & 0x0F) as usize;
    let op2 = ((insn >> 20) & 0x0F) as u8;
    match (op0, op1) {
        (0x0, 0x2) if insn == 0x20 => "nop".to_string(),
        (0x0, 0x8) => {
            let name = match op2 {
                0x8 => "add", 0x9 => "sub", 0xA => "and", 0xB => "or", 0xC => "xor",
                _ => "rrr",
            };
            format!("{name} a{t}, a{s}, a{r}")
        }
        (0x2, 0xA) => {
            let t = ((insn >> 8) & 0x0F) as usize;
            let imm = ((insn >> 12) & 0xFFF) as i32;
            let imm = ((imm << 20) >> 20) as i32;
            format!("movi a{t}, {imm}")
        }
        (0x2, 0x2) => format!("l32i a{t}, a{s}, {}", (op2 as u32).wrapping_mul(4)),
        (0x2, 0x6) => format!("s32i a{t}, a{s}, {}", (op2 as u32).wrapping_mul(4)),
        (0x2, 0x0) => format!("l8ui a{t}, a{s}, {op2}"),
        (0x2, 0x4) => format!("s8i a{t}, a{s}, {op2}"),
        (0x7, 0x0) => format!("beq a{r}, a{s}, {:#x}", pc.wrapping_add(4).wrapping_add((((insn >> 16) & 0xFF) as u32).wrapping_mul(4))),
        (0x7, 0x4) => format!("bne a{r}, a{s}, {:#x}", pc.wrapping_add(4).wrapping_add((((insn >> 16) & 0xFF) as u32).wrapping_mul(4))),
        (0x7, 0x8) => format!("blt a{r}, a{s}, {:#x}", pc.wrapping_add(4).wrapping_add((((insn >> 16) & 0xFF) as u32).wrapping_mul(4))),
        (0x7, 0xC) => format!("bge a{r}, a{s}, {:#x}", pc.wrapping_add(4).wrapping_add((((insn >> 16) & 0xFF) as u32).wrapping_mul(4))),
        (0x5, 0x0) => {
            let imm = ((insn >> 12) & 0xFFFFF) as u32;
            format!("call0 {:#x}", (pc & !3).wrapping_add(imm.wrapping_mul(4)))
        }
        (0x5, 0x4) => format!("callx0 a{s}"),
        (0x0, 0x0) if r == 0 && op2 == 0 => "ret".to_string(),
        (0x6, 0x0) => format!("mov a{t}, a{s}"),
        (0x4, 0x0) => format!("extui a{t}, a{s}, {}, {}", (insn >> 16) & 0x1f, (insn >> 24) & 0x0f),
        _ => format!(".byte 0x{insn:06x}"),
    }
}

fn disassemble_mcs51(pc: u32, read: &dyn Fn(u32) -> u8) -> String {
    let op = read(pc);
    match op {
        0x00 => "nop".to_string(),
        0x03 => "rr a".to_string(),
        0x04 => "inc a".to_string(),
        0x13 => "rrc a".to_string(),
        0x14 => "dec a".to_string(),
        0x23 => "rl a".to_string(),
        0x33 => "rlc a".to_string(),
        0x22 => "ret".to_string(),
        0x32 => "reti".to_string(),
        0x24 => format!("add a, #0x{:02x}", read(pc + 1)),
        0x34 => format!("addc a, #0x{:02x}", read(pc + 1)),
        0x44 => format!("orl a, #0x{:02x}", read(pc + 1)),
        0x54 => format!("anl a, #0x{:02x}", read(pc + 1)),
        0x64 => format!("xrl a, #0x{:02x}", read(pc + 1)),
        0x74 => format!("mov a, #0x{:02x}", read(pc + 1)),
        0x94 => format!("subb a, #0x{:02x}", read(pc + 1)),
        0xC4 => "swap a".to_string(),
        0xD4 => "da a".to_string(),
        0xE4 => "clr a".to_string(),
        0xF4 => "cpl a".to_string(),
        0x90 => format!("mov dptr, #0x{:02x}{:02x}", read(pc + 1), read(pc + 2)),
        0x75 => format!("mov 0x{:02x}, #0x{:02x}", read(pc + 1), read(pc + 2)),
        0x85 => format!("mov 0x{:02x}, 0x{:02x}", read(pc + 1), read(pc + 2)),
        0xF5 => format!("mov 0x{:02x}, a", read(pc + 1)),
        0xE5 => format!("mov a, 0x{:02x}", read(pc + 1)),
        0xE0 => "movx a, @dptr".to_string(),
        0xF0 => "movx @dptr, a".to_string(),
        0x93 => "movc a, @a+dptr".to_string(),
        0x83 => "movc a, @a+pc".to_string(),
        0xE2 | 0xE3 => format!("movx a, @r{}", op & 1),
        0xF2 | 0xF3 => format!("movx @r{}, a", op & 1),
        0x05 => format!("inc 0x{:02x}", read(pc + 1)),
        0x15 => format!("dec 0x{:02x}", read(pc + 1)),
        0x06 | 0x07 => format!("inc @r{}", op & 1),
        0x16 | 0x17 => format!("dec @r{}", op & 1),
        0xA3 => "inc dptr".to_string(),
        0xC5 => format!("xch a, 0x{:02x}", read(pc + 1)),
        0xC6 | 0xC7 => format!("xch a, @r{}", op & 1),
        0xD6 | 0xD7 => format!("xchd a, @r{}", op & 1),
        0xA4 => "mul ab".to_string(),
        0x84 => "div ab".to_string(),
        0x02 => format!("ljmp 0x{:02x}{:02x}", read(pc + 1), read(pc + 2)),
        0x12 => format!("lcall 0x{:02x}{:02x}", read(pc + 1), read(pc + 2)),
        0x80 => format!("sjmp {:#x}", pc.wrapping_add(2).wrapping_add((read(pc + 1) as i8) as u32)),
        0x60 => format!("jz {:#x}", pc.wrapping_add(2).wrapping_add((read(pc + 1) as i8) as u32)),
        0x70 => format!("jnz {:#x}", pc.wrapping_add(2).wrapping_add((read(pc + 1) as i8) as u32)),
        0x40 => format!("jc {:#x}", pc.wrapping_add(2).wrapping_add((read(pc + 1) as i8) as u32)),
        0x50 => format!("jnc {:#x}", pc.wrapping_add(2).wrapping_add((read(pc + 1) as i8) as u32)),
        0x10 => format!("jbc 0x{:02x}, {:#x}", read(pc + 1), pc.wrapping_add(3).wrapping_add((read(pc + 2) as i8) as u32)),
        0x20 => format!("jb 0x{:02x}, {:#x}", read(pc + 1), pc.wrapping_add(3).wrapping_add((read(pc + 2) as i8) as u32)),
        0x30 => format!("jnb 0x{:02x}, {:#x}", read(pc + 1), pc.wrapping_add(3).wrapping_add((read(pc + 2) as i8) as u32)),
        0x73 => "jmp @a+dptr".to_string(),
        0x82 => format!("anl c, 0x{:02x}", read(pc + 1)),
        0xB0 => format!("anl c, /0x{:02x}", read(pc + 1)),
        0x72 => format!("orl c, 0x{:02x}", read(pc + 1)),
        0xA0 => format!("orl c, /0x{:02x}", read(pc + 1)),
        0xA2 => format!("mov c, 0x{:02x}", read(pc + 1)),
        0x92 => format!("mov 0x{:02x}, c", read(pc + 1)),
        0xC2 => format!("clr 0x{:02x}", read(pc + 1)),
        0xD2 => format!("setb 0x{:02x}", read(pc + 1)),
        0xB2 => format!("cpl 0x{:02x}", read(pc + 1)),
        0xC3 => "clr c".to_string(),
        0xD3 => "setb c".to_string(),
        0xB3 => "cpl c".to_string(),
        0xC0 => format!("push 0x{:02x}", read(pc + 1)),
        0xD0 => format!("pop 0x{:02x}", read(pc + 1)),
        n if (0x78..=0x7F).contains(&n) => format!("mov r{}, #0x{:02x}", n & 7, read(pc + 1)),
        n if (0x76..=0x77).contains(&n) => format!("mov @r{}, #0x{:02x}", n & 1, read(pc + 1)),
        n if (0x86..=0x87).contains(&n) => format!("mov 0x{:02x}, @r{}", read(pc + 1), n & 1),
        n if (0xA6..=0xA7).contains(&n) => format!("mov @r{}, 0x{:02x}", n & 1, read(pc + 1)),
        n if (0xA8..=0xAF).contains(&n) => format!("mov r{}, 0x{:02x}", n & 7, read(pc + 1)),
        n if (0x88..=0x8F).contains(&n) => format!("mov 0x{:02x}, r{}", read(pc + 1), n & 7),
        n if (0xF6..=0xF7).contains(&n) => format!("mov @r{}, a", n & 1),
        n if (0xE6..=0xE7).contains(&n) => format!("mov a, @r{}", n & 1),
        n if (0x08..=0x0F).contains(&n) => format!("inc r{}", n & 7),
        n if (0x18..=0x1F).contains(&n) => format!("dec r{}", n & 7),
        n if (0x28..=0x2F).contains(&n) => format!("add a, r{}", n & 7),
        n if (0x38..=0x3F).contains(&n) => format!("addc a, r{}", n & 7),
        n if (0x98..=0x9F).contains(&n) => format!("subb a, r{}", n & 7),
        n if (0xC8..=0xCF).contains(&n) => format!("xch a, r{}", n & 7),
        n if (0x25..=0x25).contains(&n) => format!("add a, 0x{:02x}", read(pc + 1)),
        n if (0x26..=0x27).contains(&n) => format!("add a, @r{}", n & 1),
        n if (0x35..=0x35).contains(&n) => format!("addc a, 0x{:02x}", read(pc + 1)),
        n if (0x36..=0x37).contains(&n) => format!("addc a, @r{}", n & 1),
        n if (0x95..=0x95).contains(&n) => format!("subb a, 0x{:02x}", read(pc + 1)),
        n if (0x96..=0x97).contains(&n) => format!("subb a, @r{}", n & 1),
        n if (0x52..=0x52).contains(&n) => format!("anl 0x{:02x}, a", read(pc + 1)),
        n if (0x53..=0x53).contains(&n) => format!("anl 0x{:02x}, #0x{:02x}", read(pc + 1), read(pc + 2)),
        n if (0x55..=0x55).contains(&n) => format!("anl a, 0x{:02x}", read(pc + 1)),
        n if (0x56..=0x57).contains(&n) => format!("anl a, @r{}", n & 1),
        n if (0x58..=0x5F).contains(&n) => format!("anl a, r{}", n & 7),
        n if (0x42..=0x42).contains(&n) => format!("orl 0x{:02x}, a", read(pc + 1)),
        n if (0x43..=0x43).contains(&n) => format!("orl 0x{:02x}, #0x{:02x}", read(pc + 1), read(pc + 2)),
        n if (0x45..=0x45).contains(&n) => format!("orl a, 0x{:02x}", read(pc + 1)),
        n if (0x46..=0x47).contains(&n) => format!("orl a, @r{}", n & 1),
        n if (0x48..=0x4F).contains(&n) => format!("orl a, r{}", n & 7),
        n if (0x62..=0x62).contains(&n) => format!("xrl 0x{:02x}, a", read(pc + 1)),
        n if (0x63..=0x63).contains(&n) => format!("xrl 0x{:02x}, #0x{:02x}", read(pc + 1), read(pc + 2)),
        n if (0x65..=0x65).contains(&n) => format!("xrl a, 0x{:02x}", read(pc + 1)),
        n if (0x66..=0x67).contains(&n) => format!("xrl a, @r{}", n & 1),
        n if (0x68..=0x6F).contains(&n) => format!("xrl a, r{}", n & 7),
        n if (0xB4..=0xB4).contains(&n) => format!("cjne a, #0x{:02x}, {:#x}", read(pc + 1), pc.wrapping_add(3).wrapping_add((read(pc + 2) as i8) as u32)),
        n if (0xB5..=0xB5).contains(&n) => format!("cjne a, 0x{:02x}, {:#x}", read(pc + 1), pc.wrapping_add(3).wrapping_add((read(pc + 2) as i8) as u32)),
        n if (0xB6..=0xB7).contains(&n) => format!("cjne @r{}, #0x{:02x}, {:#x}", n & 1, read(pc + 1), pc.wrapping_add(3).wrapping_add((read(pc + 2) as i8) as u32)),
        n if (0xB8..=0xBF).contains(&n) => format!("cjne r{}, #0x{:02x}, {:#x}", n & 7, read(pc + 1), pc.wrapping_add(3).wrapping_add((read(pc + 2) as i8) as u32)),
        n if (0xD5..=0xD5).contains(&n) => format!("djnz 0x{:02x}, {:#x}", read(pc + 1), pc.wrapping_add(3).wrapping_add((read(pc + 2) as i8) as u32)),
        n if (0xD8..=0xDF).contains(&n) => format!("djnz r{}, {:#x}", n & 7, pc.wrapping_add(2).wrapping_add((read(pc + 1) as i8) as u32)),
        n if (0xE8..=0xEF).contains(&n) => format!("mov a, r{}", n & 7),
        n if (0xF8..=0xFF).contains(&n) => format!("mov r{}, a", n & 7),
        // AJMP addr11 (0x01,0x21,...,0xE1) / ACALL addr11 (0x11,0x31,...,0xF1).
        // The page bits A10..A8 are in opcode bits 7..5 and the low address
        // byte follows; odd high nibble selects ACALL, even selects AJMP.
        n if n & 0x0F == 0x01 && ((n >> 4) & 1) == 0 => {
            let page_hi = ((n >> 4) >> 1) & 0x07;
            let addr = ((pc as u16) & 0xF800)
                | ((page_hi as u16) << 8)
                | read(pc + 1) as u16;
            format!("ajmp 0x{addr:04x}")
        }
        n if n & 0x0F == 0x01 && ((n >> 4) & 1) == 1 => {
            let page_hi = ((n >> 4) >> 1) & 0x07;
            let addr = ((pc as u16) & 0xF800)
                | ((page_hi as u16) << 8)
                | read(pc + 1) as u16;
            format!("acall 0x{addr:04x}")
        }
        _ => format!(".byte 0x{op:02x}"),
    }
}

fn disassemble_mcs251(pc: u32, read: &dyn Fn(u32) -> u8) -> String {
    let op = read(pc);
    match op {
        0x00 => "nop".to_string(),
        0x03 => "rr a".to_string(),
        0x04 => "inc a".to_string(),
        0x14 => "dec a".to_string(),
        0x22 => "ret".to_string(),
        0x23 => "rl a".to_string(),
        0x73 => "jmp @a+dptr".to_string(),
        0xE4 => "clr a".to_string(),
        0xF4 => "cpl a".to_string(),
        n if (0x08..=0x0F).contains(&n) => format!("inc r{}", n & 7),
        n if (0x18..=0x1F).contains(&n) => format!("dec r{}", n & 7),
        n if (0x74..=0x7F).contains(&n) => {
            let hi = read(pc + 1) as u16;
            let lo = read(pc + 2) as u16;
            format!("mov r{}, #0x{:04x}", n & 0x0F, (hi << 8) | lo)
        }
        n if (0xE8..=0xEF).contains(&n) => format!("mov a, r{}", n & 7),
        n if (0xF8..=0xFF).contains(&n) => format!("mov r{}, a", n & 7),
        n if (0x28..=0x2F).contains(&n) => format!("add a, r{}", n & 7),
        n if (0x98..=0x9F).contains(&n) => format!("subb a, r{}", n & 7),
        0xA4 => "mul ab".to_string(),
        0x84 => "div ab".to_string(),
        0x90 => format!("mov dptr, #0x{:02x}{:02x}{:02x}", read(pc + 1), read(pc + 2), read(pc + 3)),
        0x02 => format!("ljmp 0x{:02x}{:02x}{:02x}", read(pc + 1), read(pc + 2), read(pc + 3)),
        0x12 => format!("lcall 0x{:02x}{:02x}{:02x}", read(pc + 1), read(pc + 2), read(pc + 3)),
        // 80251 relative branches are 2 bytes (opcode + rel8), so the target
        // is pc + 2 + rel, matching the core's `fetch_s8` post-increment.
        0x80 => format!("sjmp {:#x}", pc.wrapping_add(2).wrapping_add((read(pc + 1) as i8) as u32)),
        0x40 => format!("jc {:#x}", pc.wrapping_add(2).wrapping_add((read(pc + 1) as i8) as u32)),
        0x50 => format!("jnc {:#x}", pc.wrapping_add(2).wrapping_add((read(pc + 1) as i8) as u32)),
        0x60 => format!("jz {:#x}", pc.wrapping_add(2).wrapping_add((read(pc + 1) as i8) as u32)),
        0x70 => format!("jnz {:#x}", pc.wrapping_add(2).wrapping_add((read(pc + 1) as i8) as u32)),
        0xE0 => "movx a, @dptr".to_string(),
        0xF0 => "movx @dptr, a".to_string(),
        0xE5 => format!("mov a, 0x{:02x}", read(pc + 1)),
        0xE2 | 0xE3 => format!("movx a, @r{}", op & 1),
        0xF2 | 0xF3 => format!("movx @r{}, a", op & 1),
        _ => format!(".byte 0x{op:02x}"),
    }
}

fn disassemble_c166(pc: u32, read: &dyn Fn(u32) -> u8) -> String {
    let op = read(pc);
    match op {
        0x00 => "nop".to_string(),
        n if (0x10..=0x1F).contains(&n) => format!("mov r{}, r{}", n & 0x0F, read(pc + 1) & 0x0F),
        n if (0x20..=0x2F).contains(&n) => {
            // C166 16-bit immediates are little-endian (low byte first), as the
            // core's `fetch_u16` reads them.
            let lo = read(pc + 1) as u16;
            let hi = read(pc + 2) as u16;
            format!("mov r{}, #0x{:04x}", n & 0x0F, (hi << 8) | lo)
        }
        n if (0x30..=0x3F).contains(&n) => format!("add r{}, r{}", n & 0x0F, read(pc + 1) & 0x0F),
        n if (0x40..=0x4F).contains(&n) => format!("sub r{}, r{}", n & 0x0F, read(pc + 1) & 0x0F),
        n if (0x50..=0x5F).contains(&n) => {
            // AND/OR/XOR Rn, Rm: bits 3:2 select the operation (00=AND,
            // 01=OR, 1x=XOR), matching the execution core's 0x50-0x5F family.
            let rn = n & 0x0F;
            let rm = read(pc + 1) & 0x0F;
            let name = match n & 0x0C {
                0x00 => "and",
                0x04 => "or",
                _ => "xor",
            };
            format!("{name} r{rn}, r{rm}")
        }
        n if (0x60..=0x6F).contains(&n) => format!("mul r{}, r{}", n & 0x0F, read(pc + 1) & 0x0F),
        n if (0x70..=0x7F).contains(&n) => format!("div r{}, r{}", n & 0x0F, read(pc + 1) & 0x0F),
        n if (0x80..=0x8F).contains(&n) => format!("load r{}, [r{}]", n & 0x0F, read(pc + 1) & 0x0F),
        n if (0x90..=0x9F).contains(&n) => format!("store [r{}], r{}", n & 0x0F, read(pc + 1) & 0x0F),
        0xA0 => format!("jmp {:#x}", pc.wrapping_add(2).wrapping_add((read(pc + 1) as i8) as u32)),
        0xB0 => format!("jz {:#x}", pc.wrapping_add(2).wrapping_add((read(pc + 1) as i8) as u32)),
        0xB1 => format!("jnz {:#x}", pc.wrapping_add(2).wrapping_add((read(pc + 1) as i8) as u32)),
        0xC0 => format!("call 0x{:02x}{:02x}", read(pc + 2), read(pc + 1)),
        0xC1 => "ret".to_string(),
        _ => format!(".byte 0x{op:02x}"),
    }
}

/// A snapshot of simulator state for UI display.
#[derive(Debug, Clone, Serialize)]
pub struct SimState {
    pub pc: u32,
    pub registers: HashMap<String, u32>,
    pub cycle_count: u64,
    pub halted: bool,
    pub uart_output: String,
}

impl Simulator {
    pub fn snapshot(&self) -> SimState {
        let mut registers = HashMap::new();
        for (i, value) in self.registers.iter().enumerate() {
            registers.insert(reg_name(i).to_string(), *value);
        }
        SimState {
            pc: self.pc,
            registers,
            cycle_count: self.cycle_count,
            halted: self.halted,
            uart_output: String::from_utf8_lossy(&self.peripherals.outputs).into_owned(),
        }
    }
}

#[cfg(test)]
mod arm_integration {
    use super::*;

    /// Build a minimal ARM ELF32 (EM_ARM=40) with one PT_LOAD segment
    /// carrying `code` bytes at `vaddr`, plus a vector table at `vaddr`.
    fn arm_elf(vaddr: u32, vector_sp: u32, vector_pc: u32, code: &[u8]) -> Vec<u8> {
        let mut elf = vec![0u8; 0x100];
        elf[0..4].copy_from_slice(b"\x7fELF");
        elf[4] = 1; // ELF32
        elf[5] = 1; // little-endian
        elf[6] = 1;
        elf[18] = 40; // e_machine = EM_ARM
        elf[24..28].copy_from_slice(&vaddr.to_le_bytes()); // e_entry
        elf[28..32].copy_from_slice(&52u32.to_le_bytes()); // e_phoff
        elf[42..44].copy_from_slice(&52u16.to_le_bytes()); // e_phentsize
        elf[44..46].copy_from_slice(&1u16.to_le_bytes()); // e_phnum
        let ph = 52usize;
        elf[ph..ph + 4].copy_from_slice(&1u32.to_le_bytes()); // PT_LOAD
        elf[ph + 4..ph + 8].copy_from_slice(&0x100u32.to_le_bytes()); // p_offset
        elf[ph + 8..ph + 12].copy_from_slice(&vaddr.to_le_bytes()); // p_vaddr
        elf[ph + 16..ph + 20].copy_from_slice(&(code.len() as u32).to_le_bytes());
        elf[ph + 20..ph + 24].copy_from_slice(&(code.len() as u32).to_le_bytes());
        elf.extend_from_slice(code);
        let _ = (vector_sp, vector_pc);
        elf
    }

    #[test]
    fn for_arch_arm_creates_core() {
        let sim = Simulator::for_arch(Arch::Arm).expect("Arm core");
        assert!(sim.arm.is_some());
        assert_eq!(sim.arch, Arch::Arm);
    }

    #[test]
    fn arm_elf_loads_and_steps() {
        // Segment image: [0]=initial SP, [4]=reset|1, then MOVS r0,#1; MOVS r1,#2.
        let mut img_bytes = vec![0u8; 8];
        img_bytes[0..4].copy_from_slice(&0x2000_0000u32.to_le_bytes());
        img_bytes[4..8].copy_from_slice(&0x0800_0009u32.to_le_bytes());
        img_bytes.extend_from_slice(&[0x01u8, 0x20, 0x02, 0x21]); // 0x2001, 0x2102
        let elf = arm_elf(0x0800_0000, 0x2000_0000, 0x0800_0009, &img_bytes);
        let mut sim = Simulator::for_arch(Arch::Arm).unwrap();
        let entry = sim.load_elf(&elf).unwrap();
        assert_eq!(entry, 0x0800_0000);
        assert_eq!(sim.program_counter(), 0x0800_0008);
        assert_eq!(sim.read_reg(13), 0x2000_0000); // SP from vector
        let r = sim.step();
        assert_eq!(r, StepResult::Advanced);
        assert_eq!(sim.read_reg(0), 1);
        let r = sim.step();
        assert_eq!(r, StepResult::Advanced);
        assert_eq!(sim.read_reg(1), 2);
    }

    #[test]
    fn arm_svc_sets_exception() {
        let mut img_bytes = vec![0u8; 8];
        img_bytes[0..4].copy_from_slice(&0x2000_0000u32.to_le_bytes());
        img_bytes[4..8].copy_from_slice(&0x0800_0009u32.to_le_bytes());
        img_bytes.extend_from_slice(&[0x00u8, 0xDF]); // SVC #0 (0xDF00)
        let elf = arm_elf(0x0800_0000, 0x2000_0000, 0x0800_0009, &img_bytes);
        let mut sim = Simulator::for_arch(Arch::Arm).unwrap();
        sim.load_elf(&elf).unwrap();
        let _ = sim.step();
        assert_eq!(sim.arm.as_ref().unwrap().exception_number(), 11);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn return_mnemonic_detection() {
        // Return-class mnemonics across architectures are recognized.
        for insn in ["ret", "reti", "rts", "rti", "rfe", "rte", "retf", "mret"] {
            assert!(is_return_mnemonic(insn), "{insn}");
            assert!(is_return_mnemonic(&format!("{insn} #0x10")), "{insn} with operand");
        }
        // Some disassemblers (e.g. MSP430) render return mnemonics uppercase.
        for insn in ["RET", "RETI", "RTS", "RTI", "RFE", "RTE", "RETF", "MRET"] {
            assert!(is_return_mnemonic(insn), "{insn}");
        }
        // ARM-family returns: `bx lr`/`bx r14` and `pop {.., pc}` / `ldm .. {.., pc}`.
        for insn in [
            "bx lr",
            "bx r14",
            "blx lr",
            "pop {r4, pc}",
            "pop {pc}",
            "ldm sp!, {r4, pc}",
            // MSP430 `RET` is assembled as `MOV @SP+, PC`.
            "MOV @SP+, PC",
            "mov @sp+, pc",
        ] {
            assert!(is_return_mnemonic(insn), "{insn}");
        }
        // Non-return instructions are rejected.
        for insn in [
            "add a0, a1",
            "lw a0, 0(a1)",
            "mov A, #0x2A",
            "mov @r4+, pc",
            "reti_handler",
            "bx r0",
            "bx r1",
            "pop {r4, r5}",
            "ldr r0, [r1]",
        ] {
            assert!(!is_return_mnemonic(insn), "{insn}");
        }
    }

    /// `step_out` stops at the function epilogue across architectures: RISC-V
    /// `ret`/`mret` (interrupt return) and ARM `bx lr` / `pop {.., pc}`.
    #[test]
    fn step_out_stops_at_returns() {
        // RISC-V: a function that ends with `ret` (jalr x0, ra, 0).
        let mut sim = Simulator::for_arch(Arch::RiscV).expect("RISC-V core");
        sim.load_image(0x8000_0000, &[
            0x13, 0x03, 0x00, 0x00, // addi x6, x0, 0   (0x80000000)
            0x67, 0x00, 0x00, 0x00, // ret              (0x80000004)
        ]);
        sim.set_program_counter(0x8000_0000);
        let result = sim.step_out(64);
        assert_eq!(result, StepResult::Advanced);
        // After the ret executes, PC is the caller's next instruction (ra).
        assert_eq!(sim.program_counter(), 0);

        // ARM Thumb: `bx lr` (0x4770) is a return.
        let mut sim = Simulator::for_arch(Arch::Arm).expect("ARM core");
        // Write the bytes directly (no `load_image`: that would reset the core
        // and treat the code as a vector table, flipping the core to ARM state).
        sim.arm.as_mut().unwrap().set_arm_state(false);
        sim.arm.as_mut().unwrap().write_bytes(0x0800_0000, &[0x70, 0x47]);
        sim.set_program_counter(0x0800_0000);
        sim.write_reg(14, 0x0800_0010); // LR = return address
        let result = sim.step_out(64);
        assert_eq!(result, StepResult::Advanced);
        assert_eq!(sim.program_counter(), 0x0800_0010);
    }

    #[test]
    fn step_out_stops_at_mret_after_interrupt() {
        let mut sim = Simulator::for_arch(Arch::RiscV).expect("RISC-V core");
        // Main loop at 0x80000000; an ISR at 0x80000200 that ends with mret.
        sim.load_image(0x8000_0000, &[0x13, 0x01, 0x00, 0x00]); // nop-ish addi
        sim.load_image(0x8000_0200, &[0x73, 0x00, 0x20, 0x30]); // mret (0x30200073)
        sim.set_program_counter(0x8000_0200);
        sim.csr.mepc = 0x8000_0004;
        // Stepping out of the ISR stops after mret, back in the main loop.
        let result = sim.step_out(64);
        assert_eq!(result, StepResult::Advanced);
        assert_eq!(sim.program_counter(), 0x8000_0004);
    }

    #[test]
    fn step_over_and_step_out_use_mcs51_code_space() {
        // MCS-51 LCALL is 3 bytes: step-over must resume at the return address
        // (0x0003), not a hard-coded 4 bytes.
        let mut sim = Simulator::for_arch(Arch::Mcs51).expect("MCS-51 core");
        let mut rom = vec![0u8; 0x40];
        rom[0] = 0x12;
        rom[1] = 0x00;
        rom[2] = 0x20; // LCALL 0x0020
        rom[0x20] = 0x22; // RET
        sim.load_mcs51(0, &rom).expect("load");
        sim.set_program_counter(0);
        assert_eq!(sim.step_over(64), StepResult::Advanced);
        assert_eq!(sim.program_counter(), 0x0003, "step-over must resume after 3-byte LCALL");

        // A non-call instruction (NOP at 0x30) is a single step, not a run to
        // pc + width.
        sim.set_program_counter(0x30);
        assert_eq!(sim.step_over(64), StepResult::Advanced);
        assert_eq!(sim.program_counter(), 0x31, "step-over on NOP is one step");

        // step-out on a fresh sub-function reads RET from the separate code ROM
        // (not data space) and lands on the caller's next instruction.
        let mut sim = Simulator::for_arch(Arch::Mcs51).expect("MCS-51 core");
        let mut rom = vec![0u8; 0x40];
        rom[0x20] = 0x22; // RET
        sim.load_mcs51(0, &rom).expect("load");
        sim.set_program_counter(0x20);
        let core = sim.mcs51.as_mut().unwrap();
        core.sp = 0x09;
        core.idram[0x08] = 0x00; // return high
        core.idram[0x09] = 0x03; // return low
        assert_eq!(sim.step_out(64), StepResult::Advanced);
        assert_eq!(sim.program_counter(), 0x0003, "step-out must return after RET");
    }

    #[test]
    fn step_over_and_step_out_use_rx_call_width() {
        // RX BSR.w is 4 bytes: step-over must resume at the return address
        // (pc+4), not a hard-coded width. BSR.w pushes the return address,
        // so a valid stack pointer is required.
        let mut sim = Simulator::for_arch(Arch::Rx).expect("RX core");
        // bsr.w +4 at 0x8000, nop at 0x8004 (return point), rts at 0x8008.
        let code = [
            0x39, 0x00, 0x00, 0x04, // bsr.w +4
            0x00, 0x03,             // nop
            0x00, 0x00,             // spare
            0x00, 0x02,             // rts
        ];
        sim.load_rx(0x8000, &code).expect("load");
        sim.write_reg(0, 0x2000); // SP
        sim.set_program_counter(0x8000);
        assert_eq!(sim.step_over(64), StepResult::Advanced);
        assert_eq!(
            sim.program_counter(),
            0x8004,
            "step-over must resume after 4-byte BSR.w"
        );

        // step-out from the rts lands on the caller's next instruction.
        let mut sim = Simulator::for_arch(Arch::Rx).expect("RX core");
        let code = [0x00, 0x02]; // rts
        sim.load_rx(0x8008, &code).expect("load");
        sim.write_reg(0, 0x2000);
        // Push a synthetic return address (0x8004).
        sim.rx.as_mut().unwrap().regs[0] = 0x1FFC;
        sim.rx.as_mut().unwrap().write_u32(0x1FFC, 0x8004);
        sim.set_program_counter(0x8008);
        assert_eq!(sim.step_out(64), StepResult::Advanced);
        assert_eq!(sim.program_counter(), 0x8004, "step-out returns after rts");
    }

    #[test]
    fn step_over_and_step_out_use_msp430_return() {
        // MSP430 renders mnemonics uppercase, so the return/call detectors
        // must compare case-insensitively. `RET` is `MOV @SP+, PC`. CALL #imm
        // is 4 bytes; step-over must resume at pc+4.
        let mut sim = Simulator::for_arch(Arch::Msp430).expect("MSP430 core");
        // 0x0200: CALL #0x0300 (0x12B0), 0x0204: return point,
        // 0x0300: MOV @SP+, PC (0x4130, the RET pseudo).
        sim.load_msp430(0x0200, &[0xB0, 0x12, 0x00, 0x03]).expect("load");
        sim.msp430.as_mut().unwrap().write_bytes(0x0300, &[0x30, 0x41]);
        sim.write_reg(1, 0x0400); // SP
        sim.set_program_counter(0x0200);
        assert_eq!(sim.step_over(64), StepResult::Advanced);
        assert_eq!(
            sim.program_counter(),
            0x0204,
            "step-over must resume after 4-byte CALL #imm"
        );

        // step-out from a function ending in `MOV @SP+, PC` lands on the
        // caller's next instruction (the return address pushed by the CALL).
        let mut sim = Simulator::for_arch(Arch::Msp430).expect("MSP430 core");
        sim.load_msp430(0x0300, &[0x30, 0x41]).expect("load"); // MOV @SP+, PC
        sim.write_reg(1, 0x0400); // SP
        // Push a synthetic return address so RET pops PC = 0x0204.
        sim.msp430.as_mut().unwrap().write_u16(0x0400, 0x0204); // return PC
        sim.set_program_counter(0x0300);
        assert_eq!(sim.step_out(64), StepResult::Advanced);
        assert_eq!(
            sim.program_counter(),
            0x0204,
            "step-out must return after MOV @SP+, PC"
        );
    }

    #[test]
    fn timer_counts_and_raises_irq() {
        let mut sim = Simulator::new();
        sim.peripherals.timers.push(Timer::new(0x4000_1000, 3));
        let base = 0x4000_1000;
        // Enable, prescaler 1, auto-reload 4.
        sim.write_u32(base + 0x04, 1);
        sim.timer_write(base + 0x04);
        sim.write_u32(base + 0x0c, 4);
        sim.timer_write(base + 0x0c);
        sim.write_u32(base + 0x00, 1);
        sim.timer_write(base + 0x00);

        for _ in 0..10 {
            sim.tick_peripherals();
        }
        // After 5 ticks counter wraps once; irq should be pending.
        assert!(sim.peripherals.irq_pending[3]);
        assert!(sim.peripherals.timers[0].update_count >= 1);
    }

    /// A timer whose counter is several auto-reloads ahead must fire the exact
    /// number of update events in one `tick()` (not a single bool), so the
    /// `update_count` matches the real overflow count across high-speed runs.
    #[test]
    fn timer_tick_counts_exact_overflows() {
        let mut timer = Timer::new(0x4000_1000, 3);
        timer.enabled = true;
        timer.prescaler = 1;
        timer.auto_reload = 4;
        // Counter already 10 reloads ahead: one tick fires 10 overflows.
        timer.counter = 40;
        let overflows = timer.tick();
        assert_eq!(overflows, 10);
        assert_eq!(timer.update_count, 10);
        assert_eq!(timer.counter, 0);

        // Below the reload: a normal prescaler tick increments the counter.
        let mut timer = Timer::new(0x4000_1000, 3);
        timer.enabled = true;
        timer.prescaler = 1;
        timer.auto_reload = 4;
        timer.counter = 2;
        assert_eq!(timer.tick(), 0);
        assert_eq!(timer.counter, 3);
        assert_eq!(timer.update_count, 0);

        // Exactly at the reload: one overflow, counter returns to 0.
        let mut timer = Timer::new(0x4000_1000, 3);
        timer.enabled = true;
        timer.prescaler = 1;
        timer.auto_reload = 4;
        timer.counter = 4;
        assert_eq!(timer.tick(), 1);
        assert_eq!(timer.counter, 0);
        assert_eq!(timer.update_count, 1);

        // A disabled timer never advances.
        let mut timer = Timer::new(0x4000_1000, 3);
        timer.enabled = false;
        timer.counter = 4;
        assert_eq!(timer.tick(), 0);
        assert_eq!(timer.counter, 4);
        assert_eq!(timer.update_count, 0);
    }

    /// `tick_peripherals_for(n)` must advance timers by exactly `n` core
    /// cycles — cycle-accurate mode advances `cycle_count` by the instruction's
    /// latency, so peripherals must follow the same clock or timers run slow.
    #[test]
    fn tick_peripherals_for_advances_timer_by_cycles() {
        let mut sim = Simulator::new();
        let mut timer = Timer::new(0x4000_1000, 2);
        timer.enabled = true;
        timer.prescaler = 1;
        timer.auto_reload = 4;
        timer.counter = 0;
        sim.peripherals.timers.push(timer);

        // 4 cycles: counter 0→4, no overflow yet.
        sim.tick_peripherals_for(4);
        assert_eq!(sim.peripherals.timers[0].counter, 4);
        assert_eq!(sim.peripherals.timers[0].update_count, 0);
        assert!(!sim.peripherals.irq_pending[2]);

        // One more cycle crosses the reload → overflow + IRQ.
        sim.tick_peripherals_for(1);
        assert_eq!(sim.peripherals.timers[0].update_count, 1);
        assert!(sim.peripherals.irq_pending[2]);
    }

    /// A non-RISC-V architecture (ARM Cortex-M) must advance peripherals on
    /// every `step` — previously the ARM/8051/… branches returned before
    /// `tick_peripherals`, so timers never ran on those targets.
    #[test]
    fn arm_step_advances_peripherals() {
        let mut sim = Simulator::for_arch(Arch::Arm).unwrap();
        let mut timer = Timer::new(0x4000_1000, 1);
        timer.enabled = true;
        timer.prescaler = 1;
        timer.auto_reload = 1;
        timer.counter = 0;
        sim.peripherals.timers.push(timer);

        // Force Thumb state so the two 0x46C0 words decode as Thumb NOPs.
        // Use `write_bytes` (not `load_image`, which resets the vector table
        // and clears the Thumb bit) so PC/state stay under test control.
        sim.arm.as_mut().unwrap().xpsr |= 0x2000_0000;
        sim.write_bytes(0, &[0xC0, 0x46, 0xC0, 0x46]);
        sim.set_program_counter(0);
        let _ = sim.step();
        let _ = sim.step();
        assert!(
            sim.peripherals.timers[0].update_count >= 1,
            "ARM steps must tick peripherals; got {}",
            sim.peripherals.timers[0].update_count
        );
        assert!(sim.peripherals.irq_pending[1]);
    }

    #[test]
    fn gpio_out_records_events() {
        let mut sim = Simulator::new();
        let base = sim.peripherals.gpio_base;
        // All pins output, then set pin 0.
        sim.write_u32(base + 0x0c, 0xffff_ffff);
        sim.gpio_write(base + 0x0c);
        sim.write_u32(base + 0x04, 0x0000_0001);
        sim.gpio_write(base + 0x04);
        assert_eq!(sim.peripherals.gpio_out & 1, 1);
        assert!(!sim.peripherals.gpio_events.is_empty());
        // The event must carry the PC of the store that changed the pin.
        let event = sim.peripherals.gpio_events.last().unwrap();
        assert_eq!(event.pc, sim.pc, "event PC must match the current program counter");
        assert_eq!(event.cycle, sim.cycle_count);
        assert_eq!(event.pin, 0);
        assert_eq!(event.value, 1);
        // Clear pin 0.
        sim.write_u32(base + 0x08, 0x0000_0001);
        sim.gpio_write(base + 0x08);
        assert_eq!(sim.peripherals.gpio_out & 1, 0);
     }

    /// Stores through the ARM core must route to the virtual peripherals:
    /// writing a nonzero byte to the UART data register produces output.
    #[test]
    fn arm_stores_route_to_peripherals() {
        let mut sim = Simulator::for_arch(Arch::Arm).expect("ARM core");
        let uart = sim.peripherals.uart_base;
        sim.write_u32(uart, b'A' as u32);
        assert!(
            sim.peripherals.outputs.contains(&b'A'),
            "ARM UART store must reach the virtual UART; outputs: {:?}",
            sim.peripherals.outputs
        );
        // A zero byte is a no-op (not echoed).
        sim.write_u32(uart, 0);
        assert_eq!(sim.peripherals.outputs.iter().filter(|&&b| b == 0).count(), 0);
     }

    /// A Thumb `str r0, [r1]` executed via `step()` must route the store to the
    /// virtual UART — the ARM core records byte stores and the simulator drains
    /// them into the peripheral handlers after each instruction.
    #[test]
    fn arm_step_str_routes_store_to_peripheral() {
        let mut sim = Simulator::for_arch(Arch::Arm).expect("ARM core");
        let uart = sim.peripherals.uart_base;
        sim.arm.as_mut().unwrap().regs[0] = b'B' as u32;
        sim.arm.as_mut().unwrap().regs[1] = uart;
        // Thumb `str r0, [r1]`: 0x6000 | rn(1) = 0x6001, little-endian bytes.
        sim.write_bytes(0, &[0x01, 0x60]);
        sim.arm.as_mut().unwrap().xpsr |= 0x2000_0000; // Thumb bit
        sim.set_program_counter(0);
        let _ = sim.step();
        assert!(
            sim.peripherals.outputs.contains(&b'B'),
            "ARM `str` must route to the UART; outputs: {:?}",
            sim.peripherals.outputs
        );
    }
 
     /// The watchdog counts core cycles when enabled; feeding it (0xAA55)
    /// reloads the countdown, and an un-fed watchdog fires a reset whose
    /// `reset_count`/`last_reset_pc` the IDE can surface.
    #[test]
    fn watchdog_fires_when_unfed_and_resets_on_feed() {
        let mut sim = Simulator::new();
        let wd_base = 0x4000_3000;
        sim.peripherals.watchdog = Some(Watchdog::new(wd_base));
        let wd = sim.peripherals.watchdog.as_mut().unwrap();
        assert!(!wd.enabled);

        // Enable and set a 10-cycle timeout.
        sim.write_u32(wd_base + 0x08, 10);
        sim.watchdog_write(wd_base + 0x08);
        sim.write_u32(wd_base + 0x04, 1);
        sim.watchdog_write(wd_base + 0x04);
        assert!(sim.peripherals.watchdog.as_ref().unwrap().enabled);

        // 9 cycles: not yet fired.
        sim.tick_peripherals_for(9);
        assert_eq!(sim.peripherals.watchdog.as_ref().unwrap().reset_count, 0);

        // 1 more cycle crosses the timeout → fires a reset at the current PC.
        sim.pc = 0x1000;
        sim.tick_peripherals_for(1);
        let wd = sim.peripherals.watchdog.as_ref().unwrap();
        assert_eq!(wd.reset_count, 1);
        assert_eq!(wd.last_reset_pc, 0x1000);

        // Feeding (0xAA55) reloads the countdown before the next timeout.
        sim.write_u32(wd_base + 0x00, 0xAA55);
        sim.watchdog_write(wd_base + 0x00);
        sim.tick_peripherals_for(5);
        assert_eq!(sim.peripherals.watchdog.as_ref().unwrap().reset_count, 1);

        // A non-feed write (e.g. 0x00) must not reload the countdown.
        sim.write_u32(wd_base + 0x00, 0x0000);
        sim.watchdog_write(wd_base + 0x00);
        sim.tick_peripherals_for(5);
        assert_eq!(sim.peripherals.watchdog.as_ref().unwrap().reset_count, 2);
    }

    #[test]
    fn timer_reads_back_registers() {
        let mut sim = Simulator::new();
        // Enable and configure a timer at its base.
        sim.peripherals.timers.push(Timer::new(0x4000_1000, 0));
        let base = 0x4000_1000;
        sim.write_u32(base + 0x00, 1);
        sim.timer_write(base + 0x00);
        sim.write_u32(base + 0x04, 4);
        sim.timer_write(base + 0x04);
        sim.write_u32(base + 0x0c, 10);
        sim.timer_write(base + 0x0c);
        sim.tick_peripherals_for(1);
        // The SR (offset 0x10) reflects the timer's update flag.
        assert_eq!(sim.peripheral_read(base + 0x10), Some(0));
        assert_eq!(sim.peripheral_read(base + 0x04), Some(4));
        assert_eq!(sim.peripheral_read(base + 0x0c), Some(10));
    }

    #[test]
    fn line_mapping_round_trips() {
        let mut sim = Simulator::new();
        sim.line_table = vec![
            LineEntry { address: 0x1000, file: 0, line: 1 },
            LineEntry { address: 0x1004, file: 0, line: 2 },
            LineEntry { address: 0x1008, file: 0, line: 3 },
        ];
        assert_eq!(sim.address_for_line(0, 2), Some(0x1004));
        assert_eq!(sim.line_for_address(0x1006), Some((0, 2)));
        assert_eq!(sim.line_for_address(0x1008), Some((0, 3)));
        assert_eq!(sim.line_for_address(0x1000), Some((0, 1)));
    }

    #[test]
    fn symbol_lookup_finds_containing_function() {
        let mut sim = Simulator::new();
        sim.symbols = vec![
            Symbol { name: "main".into(), address: 0x2000, size: 16 },
            Symbol { name: "loop".into(), address: 0x2010, size: 8 },
        ];
        assert_eq!(sim.symbol_at(0x2004).map(|s| s.name.as_str()), Some("main"));
        assert_eq!(sim.symbol_at(0x2014).map(|s| s.name.as_str()), Some("loop"));
        assert!(sim.symbol_at(0x3000).is_none());
    }

    #[test]
    fn csr_read_write_and_mret() {
        let mut sim = Simulator::new();
        // CSRRW x5, mtvec, x6 — write 0x1000 to mtvec.
        sim.registers[6] = 0x1000;
        sim.execute(0x73 | (5 << 7) | (0x305 << 20) | (6 << 15) | (1 << 12));
        assert_eq!(sim.csr.mtvec, 0x1000);
        assert_eq!(sim.registers[5], 0, "CSRRW returns old value (0)");
        // CSRRSI x0, mie, 4 — set mie bit 2 (line 2).
        sim.execute(0x73 | (0x304 << 20) | (4 << 15) | (6 << 12));
        assert_eq!(sim.csr.mie, 4);
        // MRET returns to saved mepc.
        sim.csr.mepc = 0x2000;
        sim.csr.mstatus &= !0x8;
        sim.execute(0x3020_0073);
        assert_eq!(sim.pc, 0x2000);
        assert_eq!(sim.csr.mstatus & 0x8, 0x8, "MRET restores MIE");
    }

    #[test]
    fn interrupt_vectors_to_isr() {
        let mut sim = Simulator::new();
        sim.csr.mtvec = 0x8000_0100; // direct mode
        sim.csr.mstatus |= 0x8; // MIE on
        sim.csr.mie = 1 << 3;
        sim.peripherals.irq_pending[3] = true;
        let pc_before = sim.pc;
        assert!(sim.take_interrupt());
        assert_eq!(sim.pc, 0x8000_0100, "jump to trap vector");
        assert_eq!(sim.csr.mepc, pc_before, "save interrupted PC");
        assert_eq!(sim.csr.mcause, 0x8000_0003, "interrupt cause line 3");
        assert_eq!(sim.csr.mstatus & 0x8, 0, "MIE cleared during trap");
    }

    #[test]
    fn interrupt_masked_when_disabled_or_unmatched() {
        let mut sim = Simulator::new();
        sim.csr.mtvec = 0x8000_0100;
        sim.csr.mstatus |= 0x8;
        sim.csr.mie = 1 << 3;
        sim.peripherals.irq_pending[3] = true;
        // MIE clear: no trap.
        sim.csr.mstatus &= !0x8;
        assert!(!sim.take_interrupt());
        assert_eq!(sim.pc, ENTRY_POINT, "no trap when MIE is clear");
        // MIE set but line 3 not enabled in mie: no trap.
        sim.csr.mstatus |= 0x8;
        sim.csr.mie = 1 << 2;
        assert!(!sim.take_interrupt());
        assert_eq!(sim.pc, ENTRY_POINT, "no trap when line not enabled");
    }

    #[test]
    fn vectored_mode_offsets_by_line() {
        let mut sim = Simulator::new();
        sim.csr.mtvec = 0x8000_0100 | 1; // vectored mode
        sim.csr.mstatus |= 0x8;
        sim.csr.mie = 1 << 5;
        sim.peripherals.irq_pending[5] = true;
        assert!(sim.take_interrupt());
        assert_eq!(sim.pc, 0x8000_0100 + 5 * 4, "vectored: base + 4*line");
    }

    #[test]
    fn timer_irq_enters_and_returns() {
        let mut sim = Simulator::new();
        sim.csr.mtvec = 0x8000_0200;
        sim.csr.mstatus |= 0x8;
        sim.csr.mie = 1 << 3;
        sim.peripherals.timers.push(Timer::new(0x4000_1000, 3));
        let base = 0x4000_1000;
        sim.write_u32(base + 0x04, 1);
        sim.timer_write(base + 0x04);
        sim.write_u32(base + 0x0c, 4);
        sim.timer_write(base + 0x0c);
        sim.write_u32(base + 0x00, 1);
        sim.timer_write(base + 0x00);
        // Main loop: an infinite `jal x0, 0` at the entry point, so the core
        // spins in place while the timer advances.
        sim.write_u32(ENTRY_POINT, 0x0000_006f);
        sim.write_u32(0x8000_0200, 0x3020_0073); // ISR: mret
        for _ in 0..7 {
            sim.step();
        }
        // The timer fired, the core trapped into the ISR (mcause line 3), and
        // the ISR returned to the main loop via mret.
        assert!(sim.peripherals.irq_pending[3], "timer IRQ latched");
        assert_eq!(sim.csr.mcause, 0x8000_0003, "entered ISR for line 3");
        assert_eq!(sim.csr.mepc, ENTRY_POINT, "interrupted the main loop PC");
}

    #[test]
    fn arch_maps_from_elf_machine_and_target() {
        assert_eq!(Arch::from_elf_machine(243), Some(Arch::RiscV));
        assert_eq!(Arch::from_elf_machine(40), Some(Arch::Arm));
        assert_eq!(Arch::from_elf_machine(94), Some(Arch::Xtensa));
        assert_eq!(Arch::from_elf_machine(999), None);

        assert_eq!(Arch::from_target("esp32c3"), Arch::RiscV);
        assert_eq!(Arch::from_target("riscv32"), Arch::RiscV);
        assert_eq!(Arch::from_target("stm32f103c8"), Arch::Arm);
        assert_eq!(Arch::from_target("nrf52840"), Arch::Arm);
        assert_eq!(Arch::from_target("rp2040"), Arch::Arm);
        assert_eq!(Arch::from_target("esp32s3"), Arch::Xtensa);
        assert_eq!(Arch::from_target("esp32"), Arch::Xtensa);
        assert_eq!(Arch::from_target("8051"), Arch::Mcs51);
        assert_eq!(Arch::from_target("mcs51"), Arch::Mcs51);
        assert_eq!(Arch::from_target("80251"), Arch::Mcs251);
        assert_eq!(Arch::from_target("mcs251"), Arch::Mcs251);
        assert_eq!(Arch::from_target("c166"), Arch::C166);
        assert_eq!(Arch::from_target("xc166"), Arch::C166);
        assert_eq!(Arch::from_target("xc2000"), Arch::C166);
        assert_eq!(Arch::from_target("lpc2148"), Arch::Arm, "ARM7 maps to Arm");
        assert_eq!(Arch::from_target("s3c2440"), Arch::Arm, "ARM9 maps to Arm");
        assert_eq!(Arch::from_target("at91sam7"), Arch::Arm, "AT91 ARM7 maps to Arm");
        assert_eq!(Arch::from_target("imx6"), Arch::Arm, "i.MX Cortex-A maps to Arm");
        assert_eq!(Arch::from_target("arm926ej-s"), Arch::Arm, "ARM926 maps to Arm");
        // Unknown targets default to the only implemented core.
        assert_eq!(Arch::from_target("custom-riscv"), Arch::RiscV);
    }

    #[test]
    fn for_arch_supports_implemented_cores() {
        assert!(Simulator::for_arch(Arch::RiscV).is_ok());
        assert!(Simulator::for_arch(Arch::Mcs51).is_ok(), "8051 is implemented");
        assert!(Simulator::for_arch(Arch::Arm).is_ok(), "ARM is implemented");
        assert!(
            Simulator::for_arch(Arch::Mcs251).is_ok(),
            "80251 is implemented"
        );
        assert!(Simulator::for_arch(Arch::C166).is_ok(), "C166 is implemented");
        assert!(
            Simulator::for_arch(Arch::Xtensa).is_ok(),
            "Xtensa is implemented"
        );
        assert!(Simulator::for_arch(Arch::Avr).is_ok(), "AVR is implemented");
        assert!(
            Simulator::for_arch(Arch::Msp430).is_ok(),
            "MSP430 is implemented"
        );
        assert!(Simulator::for_arch(Arch::V850).is_ok(), "V850 is implemented");
        assert!(Simulator::for_arch(Arch::Stm8).is_ok(), "STM8 is implemented");
        assert!(Simulator::for_arch(Arch::Rx).is_ok(), "RX is implemented");
        assert!(Simulator::for_arch(Arch::Rl78).is_ok(), "RL78 is implemented");
    }

    #[test]
    fn load_elf_checks_machine_match() {
        // Minimal ELF32 header: e_machine = 243 (RISC-V), class=1, little-endian.
        let mut elf = vec![0u8; 52];
        elf[0..4].copy_from_slice(b"\x7fELF");
        elf[4] = 1; // ELFCLASS32
        elf[5] = 1; // little-endian
        elf[18] = 243; // e_machine lo
        // Same arch loads fine (no program headers, entry 0).
        let mut sim = Simulator::new();
        assert!(sim.load_elf(&elf).is_ok());
        // ARM machine mismatches the RISC-V simulator.
        elf[18] = 40;
        let mut sim = Simulator::new();
        let Err(error) = sim.load_elf(&elf) else {
            panic!("expected machine mismatch error");
        };
        assert!(error.to_string().contains("不匹配"));
    }

    /// Every device-catalog key must map through `from_target` to the
    /// architecture its core belongs to, so a project targeting any catalogued
    /// chip starts the right simulator core.
    #[test]
    fn from_target_maps_every_catalog_key() {
        // (catalog key, expected Arch).
        let pairs = [
            ("stm32f103c8", Arch::Arm),
            ("stm32f407vg", Arch::Arm),
            ("nrf52840", Arch::Arm),
            ("rp2040", Arch::Arm),
            ("stm32g431rb", Arch::Arm),
            ("lpc2148", Arch::Arm),
            ("s3c2440", Arch::Arm),
            ("esp32c3", Arch::RiscV),
            ("esp32", Arch::Xtensa),
            ("esp32s3", Arch::Xtensa),
            ("mcs51", Arch::Mcs51),
            ("80251", Arch::Mcs251),
            ("c166", Arch::C166),
            ("xc166", Arch::C166),
            ("xc2000", Arch::C166),
            ("st10", Arch::C166),
            ("atmega328p", Arch::Avr),
            ("attiny85", Arch::Avr),
            ("at90s8515", Arch::Avr),
            ("msp430g2553", Arch::Msp430),
            ("v850es", Arch::V850),
            ("stm8s003f3", Arch::Stm8),
            ("rx62n", Arch::Rx),
            ("rl78g14", Arch::Rl78),
        ];
        for (key, want) in pairs {
            assert_eq!(
                Arch::from_target(key),
                want,
                "catalog key {key} must map to {want:?}"
            );
        }
    }

    #[test]
    fn x51_core_loads_and_steps_via_simulator() {
        let mut sim = Simulator::for_arch(Arch::Mcs251).unwrap();
        // MOV R4, #0x1234 (0x74, 0x12, 0x34); then NOP.
        let image = [0x74, 0x12, 0x34, 0x00];
        sim.load_x51(0, &image).unwrap();
        assert_eq!(sim.program_counter(), 0);
        let r = sim.step();
        assert_eq!(r, StepResult::Advanced);
        assert_eq!(sim.read_reg(9), 0x1234); // regs[4] is index 9 (0-3 special, 4=dptr)
        assert_eq!(sim.program_counter(), 3);
    }

    #[test]
    fn x51_breakpoint_stops_before_execute() {
        let mut sim = Simulator::for_arch(Arch::Mcs251).unwrap();
        sim.load_x51(0, &[0x00, 0x00]).unwrap();
        sim.breakpoints.push(SimBreakpoint::new(0));
        let r = sim.step();
        assert_eq!(r, StepResult::Breakpoint);
        assert_eq!(sim.program_counter(), 0, "breakpoint stops before opcode");
    }

    #[test]
    fn c166_core_loads_and_steps_via_simulator() {
        let mut sim = Simulator::for_arch(Arch::C166).unwrap();
        // MOV R1, #0xABCD (0x21, 0xCD, 0xAB); then NOP.
        let image = [0x21, 0xCD, 0xAB, 0x00];
        sim.load_c166(0, &image).unwrap();
        assert_eq!(sim.program_counter(), 0);
        let r = sim.step();
        assert_eq!(r, StepResult::Advanced);
        assert_eq!(sim.read_reg(1), 0xABCD);
        assert_eq!(sim.program_counter(), 3);
    }

    #[test]
    fn c166_breakpoint_stops_before_execute() {
        let mut sim = Simulator::for_arch(Arch::C166).unwrap();
        sim.load_c166(0, &[0x00, 0x00]).unwrap();
        sim.breakpoints.push(SimBreakpoint::new(0));
        let r = sim.step();
        assert_eq!(r, StepResult::Breakpoint);
        assert_eq!(sim.program_counter(), 0, "breakpoint stops before opcode");
    }

    #[test]
    fn xtensa_core_loads_and_steps_via_simulator() {
        let mut sim = Simulator::for_arch(Arch::Xtensa).unwrap();
        // MOVI a2, 5 (op0=2, op1=A, t=2 bits 8-11, imm=5 bits 12-23).
        let word = 0x2u32 | (0xAu32 << 4) | (2u32 << 8) | (5u32 << 12);
        let image = [word as u8, (word >> 8) as u8, (word >> 16) as u8];
        sim.load_image(0, &image);
        assert_eq!(sim.program_counter(), 0);
        let r = sim.step();
        assert_eq!(r, StepResult::Advanced);
        assert_eq!(sim.read_reg(2), 5);
        assert_eq!(sim.program_counter(), 3);
    }

    #[test]
    fn xtensa_breakpoint_stops_before_execute() {
        let mut sim = Simulator::for_arch(Arch::Xtensa).unwrap();
        sim.load_image(0, &[0x00, 0x00, 0x00]);
        sim.breakpoints.push(SimBreakpoint::new(0));
        let r = sim.step();
        assert_eq!(r, StepResult::Breakpoint);
        assert_eq!(sim.program_counter(), 0, "breakpoint stops before opcode");
    }

    #[test]
    fn xtensa_load_elf_accepts_machine_94() {
        // Minimal ELF32 header with e_machine = 94 (EM_XTENSA).
        let mut elf = vec![0u8; 52];
        elf[0..4].copy_from_slice(b"\x7fELF");
        elf[4] = 1; // ELFCLASS32
        elf[5] = 1; // little-endian
        elf[18] = 94; // e_machine lo
        let mut sim = Simulator::for_arch(Arch::Xtensa).unwrap();
        assert!(sim.load_elf(&elf).is_ok(), "EM_XTENSA ELF loads");
    }

    #[test]
    fn disassembles_riscv_instructions() {
        // add a0, a1, a2 (opcode 0x33, funct3=0, rd=10, rs1=11, rs2=12).
        let insn = 0x00C58533u32;
        let bytes = [insn as u8, (insn >> 8) as u8, (insn >> 16) as u8, (insn >> 24) as u8];
        let read = |addr: u32| {
            let i = addr as usize;
            if i < 4 { bytes[i] } else { 0 }
        };
        let text = disassemble_one(Arch::RiscV, 0, &read);
        assert_eq!(text, "add a0, a1, a2");
        // addi a0, a1, 5 (opcode 0x13, funct3=0, rd=10, rs1=11, imm=5).
        let insn = 0x00558513u32;
        let bytes = [insn as u8, (insn >> 8) as u8, (insn >> 16) as u8, (insn >> 24) as u8];
        let read = |addr: u32| {
            let i = addr as usize;
            if i < 4 { bytes[i] } else { 0 }
        };
        assert_eq!(disassemble_one(Arch::RiscV, 0, &read), "addi a0, a1, 5");
    }

    #[test]
    fn disassembles_xtensa_and_8051() {
        // Xtensa MOVI a2, 5.
        let word = 0x2u32 | (0xAu32 << 4) | (2u32 << 8) | (5u32 << 12);
        let bytes = [word as u8, (word >> 8) as u8, (word >> 16) as u8];
        let read = |addr: u32| {
            let i = addr as usize;
            if i < 3 { bytes[i] } else { 0 }
        };
        assert_eq!(disassemble_one(Arch::Xtensa, 0, &read), "movi a2, 5");
        // 8051 NOP.
        let read = |addr: u32| if addr == 0 { 0x00 } else { 0 };
        assert_eq!(disassemble_one(Arch::Mcs51, 0, &read), "nop");
        // 8051 MOV A, R0.
        let read = |addr: u32| if addr == 0 { 0xE8 } else { 0 };
        assert_eq!(disassemble_one(Arch::Mcs51, 0, &read), "mov a, r0");
        // C166 MOV R0, R2.
        let bytes = [0x10, 0x02];
        let read = |addr: u32| {
            let i = addr as usize;
            if i < 2 { bytes[i] } else { 0 }
        };
        assert_eq!(disassemble_one(Arch::C166, 0, &read), "mov r0, r2");
    }

    #[test]
    fn disassembles_c166_logic_register_forms() {
        // The 0x50-0x5F family decodes AND/OR/XOR by bits 3:2 (00=AND,
        // 01=OR, 1x=XOR); the low nibble is the destination and the second
        // byte's low nibble is the source.
        for (op, expected) in [
            (0x50, "and r0, r1"),
            (0x54, "or r4, r1"),
            (0x58, "xor r8, r1"),
            (0x5C, "xor r12, r1"),
        ] {
            let bytes = [op, 0x01];
            let read = |addr: u32| {
                let i = addr as usize;
                if i < 2 { bytes[i] } else { 0 }
            };
            assert_eq!(disassemble_one(Arch::C166, 0, &read), expected);
        }
    }

    #[test]
    fn disassembles_mcs251_core_forms() {
        // Mirror the 80251 execution core: single-byte register/accumulator
        // ops, 16-bit immediate moves, 24-bit absolute jumps/calls and DPTR,
        // relative branches, direct/indirect MOVX and MOV A,direct.
        for (bytes, expected) in [
            (&[0x00][..], "nop"),
            (&[0x03][..], "rr a"),
            (&[0x04][..], "inc a"),
            (&[0x14][..], "dec a"),
            (&[0x22][..], "ret"),
            (&[0x23][..], "rl a"),
            (&[0x73][..], "jmp @a+dptr"),
            (&[0xE4][..], "clr a"),
            (&[0xF4][..], "cpl a"),
            (&[0x0F][..], "inc r7"),
            (&[0x1F][..], "dec r7"),
            (&[0x74, 0x12, 0x34][..], "mov r4, #0x1234"),
            (&[0xE8][..], "mov a, r0"),
            (&[0xF8][..], "mov r0, a"),
            (&[0x28][..], "add a, r0"),
            (&[0x98][..], "subb a, r0"),
            (&[0xA4][..], "mul ab"),
            (&[0x84][..], "div ab"),
            (&[0x90, 0x12, 0x34, 0x56][..], "mov dptr, #0x123456"),
            (&[0x02, 0x12, 0x34, 0x56][..], "ljmp 0x123456"),
            (&[0x12, 0x12, 0x34, 0x56][..], "lcall 0x123456"),
            (&[0x80, 0xFE][..], "sjmp 0x0"),
            (&[0x40, 0xFE][..], "jc 0x0"),
            (&[0x50, 0xFE][..], "jnc 0x0"),
            (&[0x60, 0xFE][..], "jz 0x0"),
            (&[0x70, 0xFE][..], "jnz 0x0"),
            (&[0xE0][..], "movx a, @dptr"),
            (&[0xF0][..], "movx @dptr, a"),
            (&[0xE5, 0x30][..], "mov a, 0x30"),
            (&[0xE2][..], "movx a, @r0"),
            (&[0xF3][..], "movx @r1, a"),
        ] {
            let read = |addr: u32| {
                let i = addr as usize;
                if i < bytes.len() { bytes[i] } else { 0 }
            };
            assert_eq!(disassemble_one(Arch::Mcs251, 0, &read), expected);
        }
    }

    #[test]
    fn disassembles_mcs51_core_forms() {
        // Mirror the 8051 execution core's opcode families so disassembly no
        // longer emits `.byte` for instructions the core actually executes.
        for (bytes, expected) in [
            (&[0x03][..], "rr a"),
            (&[0x13][..], "rrc a"),
            (&[0x23][..], "rl a"),
            (&[0x33][..], "rlc a"),
            (&[0x24, 0x10][..], "add a, #0x10"),
            (&[0x34, 0x20][..], "addc a, #0x20"),
            (&[0x44, 0x30][..], "orl a, #0x30"),
            (&[0x54, 0x40][..], "anl a, #0x40"),
            (&[0x64, 0x50][..], "xrl a, #0x50"),
            (&[0x94, 0x60][..], "subb a, #0x60"),
            (&[0xC4][..], "swap a"),
            (&[0xD4][..], "da a"),
            (&[0xE4][..], "clr a"),
            (&[0xF4][..], "cpl a"),
            (&[0x55, 0x30][..], "anl a, 0x30"),
            (&[0x45, 0x30][..], "orl a, 0x30"),
            (&[0x65, 0x30][..], "xrl a, 0x30"),
            (&[0x75, 0x30, 0x55][..], "mov 0x30, #0x55"),
            (&[0x85, 0x30, 0x31][..], "mov 0x30, 0x31"),
            (&[0xE5, 0x30][..], "mov a, 0x30"),
            (&[0xF5, 0x30][..], "mov 0x30, a"),
            (&[0x86, 0x30][..], "mov 0x30, @r0"),
            (&[0xA7, 0x30][..], "mov @r1, 0x30"),
            (&[0x76, 0x12][..], "mov @r0, #0x12"),
            (&[0xF7][..], "mov @r1, a"),
            (&[0xE6][..], "mov a, @r0"),
            (&[0xA8, 0x30][..], "mov r0, 0x30"),
            (&[0x93][..], "movc a, @a+dptr"),
            (&[0x83][..], "movc a, @a+pc"),
            (&[0xE2][..], "movx a, @r0"),
            (&[0xF3][..], "movx @r1, a"),
            (&[0x05, 0x30][..], "inc 0x30"),
            (&[0x17][..], "dec @r1"),
            (&[0xA3][..], "inc dptr"),
            (&[0xC5, 0x30][..], "xch a, 0x30"),
            (&[0xC7][..], "xch a, @r1"),
            (&[0xD6][..], "xchd a, @r0"),
            (&[0x25, 0x30][..], "add a, 0x30"),
            (&[0x27][..], "add a, @r1"),
            (&[0x35, 0x30][..], "addc a, 0x30"),
            (&[0x97][..], "subb a, @r1"),
            (&[0x52, 0x30][..], "anl 0x30, a"),
            (&[0x53, 0x30, 0x12][..], "anl 0x30, #0x12"),
            (&[0x57][..], "anl a, @r1"),
            (&[0x5F][..], "anl a, r7"),
            (&[0x42, 0x30][..], "orl 0x30, a"),
            (&[0x43, 0x30, 0x12][..], "orl 0x30, #0x12"),
            (&[0x46][..], "orl a, @r0"),
            (&[0x4F][..], "orl a, r7"),
            (&[0x62, 0x30][..], "xrl 0x30, a"),
            (&[0x63, 0x30, 0x12][..], "xrl 0x30, #0x12"),
            (&[0x66][..], "xrl a, @r0"),
            (&[0x6F][..], "xrl a, r7"),
            (&[0x82, 0x20][..], "anl c, 0x20"),
            (&[0xB0, 0x20][..], "anl c, /0x20"),
            (&[0x72, 0x20][..], "orl c, 0x20"),
            (&[0xA0, 0x20][..], "orl c, /0x20"),
            (&[0xA2, 0x20][..], "mov c, 0x20"),
            (&[0x92, 0x20][..], "mov 0x20, c"),
            (&[0xC2, 0x20][..], "clr 0x20"),
            (&[0xD2, 0x20][..], "setb 0x20"),
            (&[0xB2, 0x20][..], "cpl 0x20"),
            (&[0xC3][..], "clr c"),
            (&[0xD3][..], "setb c"),
            (&[0xB3][..], "cpl c"),
            (&[0xC0, 0x30][..], "push 0x30"),
            (&[0xD0, 0x30][..], "pop 0x30"),
            (&[0x10, 0x20, 0xFE][..], "jbc 0x20, 0x1"),
            (&[0x20, 0x20, 0xFE][..], "jb 0x20, 0x1"),
            (&[0x30, 0x20, 0xFE][..], "jnb 0x20, 0x1"),
            (&[0xB4, 0x12, 0xFE][..], "cjne a, #0x12, 0x1"),
            (&[0xB5, 0x30, 0xFE][..], "cjne a, 0x30, 0x1"),
            (&[0xB6, 0x12, 0xFE][..], "cjne @r0, #0x12, 0x1"),
            (&[0xBF, 0x12, 0xFE][..], "cjne r7, #0x12, 0x1"),
            (&[0xD5, 0x30, 0xFE][..], "djnz 0x30, 0x1"),
            (&[0xD8, 0xFE][..], "djnz r0, 0x0"),
            (&[0x01, 0x60][..], "ajmp 0x0060"),
            (&[0x31, 0x55][..], "acall 0x0155"),
            (&[0xE1, 0x34][..], "ajmp 0x0734"),
        ] {
            let read = |addr: u32| {
                let i = addr as usize;
                if i < bytes.len() { bytes[i] } else { 0 }
            };
            assert_eq!(disassemble_one(Arch::Mcs51, 0, &read), expected);
        }
    }

    #[test]
    fn byte_opcode_disasm_matches_core() {
        // For every single-byte opcode, if the disassembler prints a raw
        // placeholder the execution core must halt on it (and vice versa the
        // core's supported opcodes must all have a real disassembly).
        for arch in [Arch::Mcs51, Arch::Mcs251, Arch::C166] {
            for op in 0u8..=255 {
                let bytes = [op];
                let read = |a: u32| bytes.get(a as usize).copied().unwrap_or(0);
                let text = disassemble_one(arch, 0, &read);
                let placeholder =
                    text.starts_with("0x") || text.starts_with(".byte") || text.starts_with(".word");
                if placeholder {
                    let mut sim = Simulator::for_arch(arch).unwrap();
                    let r = match arch {
                        Arch::Mcs51 => sim.load_mcs51(0, &bytes).map(|_| sim.step()),
                        Arch::Mcs251 => sim.load_x51(0, &bytes).map(|_| sim.step()),
                        Arch::C166 => sim.load_c166(0, &bytes).map(|_| sim.step()),
                        _ => unreachable!(),
                    };
                    let r = r.expect("load");
                    assert_ne!(
                        r,
                        StepResult::Advanced,
                        "arch {arch:?} opcode 0x{op:02x} disassembles as {text:?} but core executes"
                    );
                }
            }
        }
    }

    #[test]
    fn instruction_size_matches_encoding_width() {
        // ARM A32 is always 4 bytes.
        let read = |_addr: u32| 0u8;
        assert_eq!(instruction_size(Arch::Arm, 0, &read, true), 4);
        // Thumb-1 (16-bit) halfword, e.g. MOVS r0, #1 (0x2001).
        let read = |addr: u32| if addr == 0 { 0x01 } else { 0x20 };
        assert_eq!(instruction_size(Arch::Arm, 0, &read, false), 2);
        // Thumb-1 unconditional branch (0xE001) is still 16-bit, not 32-bit.
        let read = |addr: u32| if addr == 0 { 0x01 } else { 0xE0 };
        assert_eq!(instruction_size(Arch::Arm, 0, &read, false), 2);
        // Thumb-2 (32-bit) starts with a 0xE000-prefixed halfword.
        let read = |addr: u32| match addr {
            0 => 0x00,
            1 => 0xf0,
            2 => 0x00,
            _ => 0xf8,
        };
        assert_eq!(instruction_size(Arch::Arm, 0, &read, false), 4);
        // Fixed-width architectures.
        assert_eq!(instruction_size(Arch::RiscV, 0, &read, false), 4);
        assert_eq!(instruction_size(Arch::Avr, 0, &read, false), 2);
        // MCS-51 is variable-length: NOP (1), LJMP addr16 (3), MOV A,#imm (2),
        // MOV direct,#imm (3), MOV A,R0 (1).
        let read = |addr: u32| match addr {
            0 => 0x00,
            1 => 0x02,
            2 => 0x00,
            3 => 0x10,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::Mcs51, 0, &read, false), 1);
        assert_eq!(instruction_size(Arch::Mcs51, 1, &read, false), 3);
        let read = |addr: u32| match addr {
            0 => 0x74,
            1 => 0x2A,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::Mcs51, 0, &read, false), 2);
        let read = |addr: u32| match addr {
            0 => 0x75,
            1 => 0x30,
            2 => 0x55,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::Mcs51, 0, &read, false), 3);
        let read = |addr: u32| if addr == 0 { 0xE8 } else { 0 };
        assert_eq!(instruction_size(Arch::Mcs51, 0, &read, false), 1);
        // MCS-251 (80251): NOP (1), MOV Rn,#imm16 (3), LJMP addr24 (4),
        // SJMP rel (2), MOV A,R0 (1).
        let read = |addr: u32| match addr {
            0 => 0x00,
            1 => 0x74,
            2 => 0x12,
            3 => 0x34,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::Mcs251, 0, &read, false), 1);
        assert_eq!(instruction_size(Arch::Mcs251, 1, &read, false), 3);
        let read = |addr: u32| match addr {
            0 => 0x02,
            1 => 0x00,
            2 => 0x00,
            3 => 0x10,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::Mcs251, 0, &read, false), 4);
        let read = |addr: u32| match addr {
            0 => 0x80,
            1 => 0x02,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::Mcs251, 0, &read, false), 2);
        let read = |addr: u32| if addr == 0 { 0xE8 } else { 0 };
        assert_eq!(instruction_size(Arch::Mcs251, 0, &read, false), 1);
        // AVR is 16-bit except the 32-bit LDS/STS/JMP/CALL forms (big-endian
        // word: low byte first, high byte second).
        let read = |addr: u32| match addr {
            0 => 0x00,
            1 => 0x00,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::Avr, 0, &read, false), 2);
        let read = |addr: u32| match addr {
            0 => 0x00,
            1 => 0x90, // LDS word 0x9000
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::Avr, 0, &read, false), 4);
        // C166: NOP/RET are 1 byte, MOV Rn,#imm16 and CALL are 3, the rest 2.
        let read = |addr: u32| match addr {
            0 => 0x00,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::C166, 0, &read, false), 1);
        let read = |addr: u32| match addr {
            0 => 0x20,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::C166, 0, &read, false), 3);
        let read = |addr: u32| match addr {
            0 => 0xC0,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::C166, 0, &read, false), 3);
        let read = |addr: u32| match addr {
            0 => 0x10,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::C166, 0, &read, false), 2);
        // Xtensa in this simulator is a 24-bit fetch model (3 bytes).
        let read = |addr: u32| match addr {
            0 => 0x20,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::Xtensa, 0, &read, false), 3);
        // V850 is variable-length: NOP (0x0000) is 2 bytes, DISPOSE 4 bytes.
        let read = |addr: u32| match addr {
            0 => 0x00,
            1 => 0x00,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::V850, 0, &read, false), 2);
        let read = |addr: u32| match addr {
            0 => 0x40,
            1 => 0x06,
            2 => 0x00,
            3 => 0x00,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::V850, 0, &read, false), 4);
        // STM8 is variable-length: NOP (1), JRA rel (2), CALL direct16 (3),
        // CALLF addr24 (4), prefixed Y-indexed (3).
        let read = |addr: u32| match addr {
            0 => 0x9D,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::Stm8, 0, &read, false), 1);
        let read = |addr: u32| match addr {
            0 => 0x20,
            1 => 0x02,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::Stm8, 0, &read, false), 2);
        let read = |addr: u32| match addr {
            0 => 0xCD,
            1 => 0x56,
            2 => 0x34,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::Stm8, 0, &read, false), 3);
        let read = |addr: u32| match addr {
            0 => 0x8D,
            1 => 0x78,
            2 => 0x56,
            3 => 0x34,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::Stm8, 0, &read, false), 4);
        let read = |addr: u32| match addr {
            0 => 0x90,
            1 => 0x65,
            2 => 0x10,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::Stm8, 0, &read, false), 3);
        // MSP430 is 2 bytes plus one extension word per immediate/absolute/
        // indexed operand: MOV Rn,Rn is 2, MOV #imm16,R5 is 4, PUSH #imm16 is 4.
        let read = |addr: u32| match addr {
            0 => 0x00,
            1 => 0x40,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::Msp430, 0, &read, false), 2);
        let read = |addr: u32| match addr {
            0 => 0x35,
            1 => 0x40,
            2 => 0x34,
            3 => 0x12,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::Msp430, 0, &read, false), 4);
        let read = |addr: u32| match addr {
            0 => 0x30,
            1 => 0x12,
            2 => 0x34,
            3 => 0x12,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::Msp430, 0, &read, false), 4);
        // RL78 is variable-length: NOP (1), add A,#imm8 (2), add saddr,#imm8 (3),
        // addw AX,#imm16 (3), add A,ES:[imm16] (3), and 0x61-prefixed forms
        // (2 for reg-reg, 3 for [HL+imm8], 4 for ES:[imm16]).
        let read = |addr: u32| match addr {
            0 => 0x00,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::Rl78, 0, &read, false), 1);
        let read = |addr: u32| match addr {
            0 => 0x0C,
            1 => 0x2A,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::Rl78, 0, &read, false), 2);
        let read = |addr: u32| match addr {
            0 => 0x0A,
            1 => 0x30,
            2 => 0x55,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::Rl78, 0, &read, false), 3);
        let read = |addr: u32| match addr {
            0 => 0x04,
            1 => 0x34,
            2 => 0x12,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::Rl78, 0, &read, false), 3);
        let read = |addr: u32| match addr {
            0 => 0x0F,
            1 => 0x78,
            2 => 0x56,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::Rl78, 0, &read, false), 3);
        let read = |addr: u32| match addr {
            0 => 0x61,
            1 => 0x08,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::Rl78, 0, &read, false), 2);
        let read = |addr: u32| match addr {
            0 => 0x61,
            1 => 0x59,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::Rl78, 0, &read, false), 2);
        let read = |addr: u32| match addr {
            0 => 0x61,
            1 => 0xB8,
            2 => 0x10,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::Rl78, 0, &read, false), 3);
        let read = |addr: u32| match addr {
            0 => 0x61,
            1 => 0xA8,
            2 => 0x78,
            3 => 0x56,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::Rl78, 0, &read, false), 4);
        let read = |addr: u32| match addr {
            0 => 0x61,
            1 => 0xAB,
            2 => 0x30,
            3 => 0x00,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::Rl78, 0, &read, false), 4);
        // RX is variable-length: BRA.s (1), NOP (2), BCnd.b (2),
        // MOV.b #imm8 (3), MOV.b dsp8 (3), ADD #imm16 (4), BRA.a (6).
        let read = |addr: u32| match addr {
            0 => 0x08,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::Rx, 0, &read, false), 1);
        let read = |addr: u32| match addr {
            0 => 0x00,
            1 => 0x03,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::Rx, 0, &read, false), 2);
        let read = |addr: u32| match addr {
            0 => 0x20,
            1 => 0x00,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::Rx, 0, &read, false), 2);
        let read = |addr: u32| match addr {
            0 => 0x3C,
            1 => 0x00,
            2 => 0x10,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::Rx, 0, &read, false), 3);
        let read = |addr: u32| match addr {
            0 => 0x41,
            1 => 0x00,
            2 => 0x10,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::Rx, 0, &read, false), 3);
        let read = |addr: u32| match addr {
            0 => 0x70,
            1 => 0x00,
            2 => 0x34,
            3 => 0x12,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::Rx, 0, &read, false), 4);
        let read = |addr: u32| match addr {
            0 => 0x00,
            1 => 0x04,
            _ => 0,
        };
        assert_eq!(instruction_size(Arch::Rx, 0, &read, false), 6);
    }

    #[test]
    fn disassembles_a32_instructions() {
        // MOV r0, #0x1234 (MOVW r0, #0x1234 => 0xE3010234) disassembles in
        // ARM state but not in Thumb state.
        let bytes = [0x34, 0x02, 0x01, 0xE3];
        let read = |addr: u32| {
            let i = addr as usize;
            if i < 4 { bytes[i] } else { 0 }
        };
        let a32 = disassemble_one_state(Arch::Arm, 0, &read, true);
        assert_eq!(a32, "movw r0, #0x1234");
        // The same bytes in Thumb state decode differently (Thumb-1 group).
        let thumb = disassemble_one_state(Arch::Arm, 0, &read, false);
        assert_ne!(thumb, a32, "ARM state changes the decode");
        // ADD r0, r1, r2 (0xE0810002).
        let insn = 0xE081_0002u32;
        let bytes = [insn as u8, (insn >> 8) as u8, (insn >> 16) as u8, (insn >> 24) as u8];
        let read = |addr: u32| {
            let i = addr as usize;
            if i < 4 { bytes[i] } else { 0 }
        };
        assert_eq!(
            disassemble_one_state(Arch::Arm, 0, &read, true),
            "add r0, r1, r2"
        );
        // LDR r0, [r1, #4] (0xE5910004).
        let insn = 0xE591_0004u32;
        let bytes = [insn as u8, (insn >> 8) as u8, (insn >> 16) as u8, (insn >> 24) as u8];
        let read = |addr: u32| {
            let i = addr as usize;
            if i < 4 { bytes[i] } else { 0 }
        };
        assert_eq!(
            disassemble_one_state(Arch::Arm, 0, &read, true),
            "ldr r0, [r1, #4]"
        );
        // B +0 (0xEA000000) branches to pc+8 = 0x8.
        let insn = 0xEA00_0000u32;
        let bytes = [insn as u8, (insn >> 8) as u8, (insn >> 16) as u8, (insn >> 24) as u8];
        let read = |addr: u32| {
            let i = addr as usize;
            if i < 4 { bytes[i] } else { 0 }
        };
        assert_eq!(disassemble_one_state(Arch::Arm, 0, &read, true), "b 0x8");
    }
    #[test]
    fn disassembles_thumb_alu_instructions() {
        // Regression: the Thumb-1 format-2 ALU guard was `insn & 0xFFFF_8000
        // == 0x4000`, always false for a 16-bit `insn`, so every ALU op fell
        // through to `.hword`. Guard must mask with 0xF800 on `half`.
        let cases: &[(u16, &str)] = &[
            (0x4000, "ands r0, r0"),
            (0x4011, "ands r1, r2"),
            (0x4091, "lsls r1, r2"),
            (0x4100, "asrs r0, r0"),
            (0x4200, "tst r0, r0"),
            (0x4291, "cmp r1, r2"),
            (0x4351, "muls r1, r2"),
            (0x43d1, "mvns r1, r2"),
        ];
        for (insn, expected) in cases {
            let bytes = [*insn as u8, (*insn >> 8) as u8];
            let read = |addr: u32| {
                let i = addr as usize;
                if i < 2 { bytes[i] } else { 0 }
            };
            assert_eq!(
                disassemble_one_state(Arch::Arm, 0, &read, false),
                *expected,
                "insn {insn:#06x}"
            );
        }
    }

    #[test]
    fn disassembles_thumb_full_families() {
        // Thumb-1 families the core executes (LSL/LSR/ASR, ADD/SUB,
        // MOV/CMP/ADD/SUB imm, PC-rel LDR, high-reg/BX, register-offset
        // loads/stores, word/byte/half immediate, SP-relative, PUSH/POP,
        // STM/LDM, conditional branch, SVC) decode with real mnemonics.
        let cases: &[(u16, &str)] = &[
            (0x0148, "lsls r0, r1, #5"),
            (0x1148, "asrs r0, r1, #5"),
            (0x1850, "adds r0, r2, r1"),
            (0x1CC8, "adds r0, r1, #3"),
            (0x2210, "movs r2, #16"),
            (0x2A10, "cmp r2, #16"),
            (0x3A10, "subs r2, #16"),
            (0x4A10, "ldr r2, [pc, #64]"),
            (0x5A00, "ldrh r0, [r0, r0]"),
            (0x6801, "ldr r0, [r1, #0]"),
            (0x7001, "strb r0, [r1, #0]"),
            (0x8001, "strh r0, [r1, #0]"),
            (0x9101, "str r1, [sp, #4]"),
            (0xA001, "add r0, pc, #4"),
            (0xB001, "add sp, sp, #4"),
            (0xB401, "push {r0}"),
            (0xBD01, "pop {r0, pc}"),
            (0xC801, "ldmia r0!, {r0}"),
            (0xDF00, "svc #0"),
            (0xD001, "beq 0x6"),
            (0xE001, "b 0x6"),
            (0xBF08, "it eq"),
            (0xBF04, "itt eq"),
            (0xBF0C, "ite eq"),
            (0xBFAC, "ite ge"),
            (0xBF0B, "itete eq"),
            (0xBF02, "ittt eq"),
            (0xBF01, "itttt eq"),
            (0xBE03, "bkpt #0x3"),
            (0xBF00, "nop"),
            (0xBF10, "yield"),
            (0xBF20, "wfe"),
            (0xBF30, "wfi"),
            (0xBF40, "sev"),
        ];
        for (half, expected) in cases {
            let bytes = [*half as u8, (*half >> 8) as u8];
            let read = |addr: u32| {
                let i = addr as usize;
                if i < 2 { bytes[i] } else { 0 }
            };
            assert_eq!(
                disassemble_one_state(Arch::Arm, 0, &read, false),
                *expected,
                "insn {half:#06x}"
            );
        }
    }

    #[test]
    fn disassembles_thumb2_wide_forms() {
        // Thumb-2 32-bit forms the core executes (BL/BLX, MOVW/MOVT,
        // ADDW/SUBW, LDR/STR/LDRB/STRB/LDRH/STRH immediate, DMB/DSB/ISB)
        // decode with real mnemonics.
        let cases: &[(u32, &str)] = &[
            (0xF000_D000, "bl 0x4"),
            (0xF000_9004, "b.w 0xc"),
            (0xF000_8010, "beqw 0x24"),
            (0xF040_8018, "bnew 0x34"),
            (0xF47F_ADA6, "bnew 0xfffffb50"),
            (0xF049_8620, "bnew 0x9c44"),
            (0xF300_8002, "bgtw 0x8"),
            (0xF240_2000, "movw r0, #0x200"),
            (0xF200_2000, "addw r0, r0, #0x200"),
            (0xF2C1_2134, "movt r1, #0x1234"),
            (0xF64A_32CD, "movw r2, #0xabcd"),
            (0xF6A5_049A, "subw r4, r5, #0x89a"),
            (0xF8D0_0000, "ldr r0, [r0, #0x0]"),
            (0xF8C0_0000, "str r0, [r0, #0x0]"),
            (0xF890_0000, "ldrb r0, [r0, #0x0]"),
            (0xF880_0000, "strb r0, [r0, #0x0]"),
            (0xF8B0_0000, "ldrh r0, [r0, #0x0]"),
            (0xF8A0_0000, "strh r0, [r0, #0x0]"),
            // 8-bit immediate loads/stores with optional writeback.
            (0xF840_1C04, "str r1, [r0, #-0x4]"),
            (0xF850_0C04, "ldr r0, [r0, #-0x4]"),
            (0xF84D_BD04, "str r11, [sp, #-0x4]!"),
            (0xF85D_BB04, "ldr r11, [sp], #0x4"),
            (0xF85D_0904, "ldr r0, [sp], #-0x4"),
            (0xF800_1C01, "strb r1, [r0, #-0x1]"),
            (0xF833_2C02, "ldrh r2, [r3, #-0x2]"),
            (0xF88D_1003, "strb r1, [sp, #0x3]"),
            (0xF3BF_8F5F, "dmb sy"),
            (0xF3BF_8F6F, "dsb sy"),
            (0xF3BF_8F7F, "isb sy"),
            (0xE8D7_F008, "tbb [r7, r8]"),
            (0xE8D7_F018, "tbh [r7, r8]"),
            (0xE9D2_0100, "ldrd r0, r1, [r2]"),
            (0xE9F2_0102, "ldrd r0, r1, [r2, #8]!"),
            (0xE9C6_4504, "strd r4, r5, [r6, #16]"),
            (0xE8E6_4504, "strd r4, r5, [r6], #16"),
            // Data processing (modified immediate).
            (0xF04F_00FF, "mov r0, #0xff"),
            (0xF04F_4000, "mov r0, #0x80000000"),
            (0xF05F_0201, "movs r2, #0x1"),
            (0xF06F_09FF, "mvn r9, #0xff"),
            (0xF500_7080, "add r0, r0, #0x100"),
            (0xF5A6_7500, "sub r5, r6, #0x200"),
            (0xF008_07F0, "and r7, r8, #0xf0"),
            (0xF1B0_0F40, "cmp r0, #0x40"),
            (0xF012_0FFF, "tst r2, #0xff"),
            (0xF093_4F00, "teq r3, #0x80000000"),
            // Bit-field extract/insert/clear.
            (0xF3C2_1107, "ubfx r1, r2, #0x4, #0x8"),
            (0xF344_0385, "sbfx r3, r4, #0x2, #0x6"),
            (0xF36F_150B, "bfc r5, #4, #8"),
            (0xF362_01C7, "bfi r1, r2, #3, #5"),
            // REV family and sign/zero extend.
            (0xFA98_F788, "rev.w r7, r8"),
            (0xFA9A_F99A, "rev16.w r9, r10"),
            (0xFA9C_FBBC, "revsh.w r11, r12"),
            (0xFA91_F0A1, "rbit r0, r1"),
            (0xFA5F_F889, "uxtb.w r8, r9"),
            (0xFA4F_FA8B, "sxtb.w r10, r11"),
            (0xFA1F_F081, "uxth.w r0, r1"),
            (0xFA0F_F283, "sxth.w r2, r3"),
            (0xFA5F_F091, "uxtb.w r0, r1, ror #8"),
            (0xFA1F_F2A3, "uxth.w r2, r3, ror #16"),
            (0xFA4F_F4B5, "sxtb.w r4, r5, ror #24"),
            // Exclusive access.
            (0xE851_0F00, "ldrex r0, [r1]"),
            (0xE844_3200, "strex r2, r3, [r4]"),
            (0xE8D6_5F4F, "ldrexb r5, [r6]"),
            (0xE8C9_8F47, "strexb r7, r8, [r9]"),
            (0xE8DB_AF5F, "ldrexh r10, [r11]"),
            (0xE8C1_0F5C, "strexh r12, r0, [r1]"),
            // Signed byte/halfword loads.
            (0xF917_6018, "ldrsb r6, [r7, r8, lsl #1]"),
            (0xF93A_902B, "ldrsh r9, [r10, r11, lsl #2]"),
            (0xF991_0004, "ldrsb r0, [r1, #0x4]"),
            (0xF9B3_2008, "ldrsh r2, [r3, #0x8]"),
            (0xF99F_0004, "ldrsb r0, [pc, #0x4]"),
            (0xF9BF_1008, "ldrsh r1, [pc, #0x8]"),
            // MLA / MLS.
            (0xFB01_F40E, "mul r4, r1, r14"),
            (0xFB02_E101, "mla r1, r2, r1, r14"),
            (0xFB03_1100, "mla r1, r3, r0, r1"),
            (0xFB02_0011, "mls r0, r2, r1, r0"),
            // Multiply-long (64-bit).
            (0xFBA2_0103, "umull r0, r1, r2, r3"),
            (0xFBC2_0103, "smull r0, r1, r2, r3"),
            (0xFBA6_4517, "umlal r4, r5, r6, r7"),
            (0xFBCA_891B, "smlal r8, r9, r10, r11"),
            // Hint instructions.
            (0xF3AF_8000, "nop.w"),
            (0xF3AF_8001, "yield.w"),
            (0xF3AF_8002, "wfe.w"),
            (0xF3AF_8003, "wfi.w"),
            (0xF3AF_8004, "sev.w"),
            // Data processing (register) with shifted operand.
            (0xEA4F_0809, "mov r8, r9"),
            (0xEA6F_0A0B, "mvn r10, r11"),
            (0xEB01_00C2, "add r0, r1, r2, lsl #3"),
            (0xEB01_1002, "add r0, r1, r2, lsl #4"),
            (0xEB01_0052, "add r0, r1, r2, lsr #1"),
            (0xEB01_1022, "add r0, r1, r2, asr #4"),
            (0xEB01_0072, "add r0, r1, r2, ror #1"),
            (0xEA4F_192A, "mov r9, r10, asr #4"),
            (0xEA4F_0000, "mov r0, r0"),
            (0xEA4F_0102, "mov r1, r2"),
        ];
        for (insn, expected) in cases {
            let bytes = [
                (*insn >> 16) as u8,
                (*insn >> 24) as u8,
                *insn as u8,
                (*insn >> 8) as u8,
            ];
            let read = |addr: u32| {
                let i = addr as usize;
                if i < 4 { bytes[i] } else { 0 }
            };
            assert_eq!(
                disassemble_one_state(Arch::Arm, 0, &read, false),
                *expected,
                "insn {insn:#010x}"
            );
        }
    }

    #[test]
    fn disassembles_a32_operand2_shifts() {
        // ARM-state operand-2 shifts mirror `ArmCore::arm_operand2`: LSL/LSR/
        // ASR/ROR immediate, RRX, and register-specified shifts.
        let cases: &[(u32, &str)] = &[
            (0xE790_0182, "ldr r0, [r0, r2, lsl #3]"),
            (0xE791_0102, "ldr r0, [r1, r2, lsl #2]"),
            (0xE791_0000, "ldr r0, [r1, r0]"),
            (0xE790_0110, "ldr r0, [r0, r0, lsl r1]"),
            (0xE1A0_0201, "mov r0, r1, lsl #4"),
            (0xE1A0_0661, "mov r0, r1, ror #12"),
            (0xE1A0_0061, "mov r0, r1, rrx"),
            // Halfword / signed transfer: immediate vs register offset.
            (0xE1D1_00B2, "ldrh r0, [r1, #2]"),
            (0xE191_00B2, "ldrh r0, [r1, r2]"),
            (0xE180_10B2, "strh r1, [r0, r2]"),
            // Multiply destination is bits 19-16, not bits 15-12.
            (0xE003_0291, "mul r3, r1, r2"),
        ];
        for (insn, expected) in cases {
            let bytes = insn.to_le_bytes();
            let read = |addr: u32| {
                let i = addr as usize;
                if i < 4 { bytes[i] } else { 0 }
            };
            assert_eq!(
                disassemble_one_state(Arch::Arm, 0, &read, true),
                *expected,
                "insn {insn:#010x}"
            );
        }
    }


    #[test]
    fn register_metadata_per_architecture() {
        assert_eq!(reg_count_for(Arch::RiscV), 32);
        assert_eq!(reg_count_for(Arch::Arm), 16);
        assert_eq!(reg_count_for(Arch::Xtensa), 16);
        assert_eq!(reg_count_for(Arch::Mcs51), 13);
        assert_eq!(reg_count_for(Arch::Mcs251), 21);
        assert_eq!(reg_count_for(Arch::C166), 16);
        assert_eq!(reg_label_for(Arch::Mcs51, 0), "ACC");
        assert_eq!(reg_label_for(Arch::Mcs51, 5), "R0");
        assert_eq!(reg_label_for(Arch::Mcs251, 5), "R0");
        assert_eq!(reg_label_for(Arch::C166, 3), "R3");
        assert_eq!(reg_label_for(Arch::Xtensa, 2), "a2");
        assert_eq!(reg_label_for(Arch::Arm, 3), "r3");
        // Newer Keil/IAR architectures.
        assert_eq!(reg_count_for(Arch::Avr), 32);
        assert_eq!(reg_count_for(Arch::Msp430), 16);
        assert_eq!(reg_count_for(Arch::V850), 32);
        assert_eq!(reg_count_for(Arch::Stm8), 5);
        assert_eq!(reg_count_for(Arch::Rx), 16);
        assert_eq!(reg_count_for(Arch::Rl78), 16);
        assert_eq!(reg_label_for(Arch::Avr, 3), "R3");
        assert_eq!(reg_label_for(Arch::Msp430, 1), "SP");
        assert_eq!(reg_label_for(Arch::V850, 3), "sp");
        assert_eq!(reg_label_for(Arch::Stm8, 0), "A");
        assert_eq!(reg_label_for(Arch::Rx, 0), "r0");
        assert_eq!(reg_label_for(Arch::Rl78, 0), "X");
    }

    #[test]
    fn write_reg_routes_per_architecture() {
        let mut sim = Simulator::for_arch(Arch::RiscV).unwrap();
        sim.write_reg(5, 0x1234);
        assert_eq!(sim.read_reg(5), 0x1234, "RISC-V register write");
        sim.write_reg(0, 0xdead);
        assert_eq!(sim.read_reg(0), 0, "RISC-V x0 is read-only");

        let mut sim = Simulator::for_arch(Arch::Arm).unwrap();
        sim.write_reg(3, 0xabcd);
        assert_eq!(sim.read_reg(3), 0xabcd, "ARM register write");

        let mut sim = Simulator::for_arch(Arch::Xtensa).unwrap();
        sim.write_reg(2, 0x99);
        assert_eq!(sim.read_reg(2), 0x99, "Xtensa register write");

        let mut sim = Simulator::for_arch(Arch::C166).unwrap();
        sim.write_reg(4, 0x77);
        assert_eq!(sim.read_reg(4), 0x77, "C166 register write");
    }

    #[test]
    fn write_reg_mcs51_and_x51_special_registers() {
        let mut sim = Simulator::for_arch(Arch::Mcs51).unwrap();
        sim.write_reg(0, 0x55); // ACC
        assert_eq!(sim.read_reg(0), 0x55, "mcs51 ACC write");
        sim.write_reg(5, 0x12); // R0 (bank 0)
        assert_eq!(sim.read_reg(5), 0x12, "mcs51 R0 write");

        let mut sim = Simulator::for_arch(Arch::Mcs251).unwrap();
        sim.write_reg(2, 0x30); // PSW
        assert_eq!(sim.read_reg(2), 0x30, "80251 PSW write");
    }

    #[test]
    fn set_program_counter_routes_per_architecture() {
        let mut sim = Simulator::for_arch(Arch::RiscV).unwrap();
        sim.set_program_counter(0x8000);
        assert_eq!(sim.program_counter(), 0x8000, "RISC-V PC set");

        let mut sim = Simulator::for_arch(Arch::Arm).unwrap();
        sim.set_program_counter(0x0800_1234);
        assert_eq!(sim.program_counter(), 0x0800_1234, "ARM PC set");

        let mut sim = Simulator::for_arch(Arch::Mcs51).unwrap();
        sim.set_program_counter(0x0400);
        assert_eq!(sim.program_counter(), 0x0400, "8051 PC set");

        let mut sim = Simulator::for_arch(Arch::Xtensa).unwrap();
        sim.set_program_counter(0x4000_0100);
        assert_eq!(sim.program_counter(), 0x4000_0100, "Xtensa PC set");
    }

    #[test]
    fn avr_core_loads_and_steps_via_simulator() {
        let mut sim = Simulator::for_arch(Arch::Avr).unwrap();
        // AVR NOP (0x0000) is a single 16-bit word.
        sim.load_avr(0, &[0x00, 0x00]).unwrap();
        let result = sim.step();
        assert_eq!(result, StepResult::Advanced);
        assert_eq!(sim.program_counter(), 2, "AVR PC advances by 2 bytes");
    }

    #[test]
    fn avr_breakpoint_stops_before_execute() {
        let mut sim = Simulator::for_arch(Arch::Avr).unwrap();
        sim.load_avr(0, &[0x00, 0x00]).unwrap();
        sim.set_breakpoint(0);
        let result = sim.step();
        assert_eq!(result, StepResult::Breakpoint);
        assert_eq!(sim.program_counter(), 0, "breakpoint at PC 0");
    }

    #[test]
    fn msp430_core_loads_and_steps_via_simulator() {
        let mut sim = Simulator::for_arch(Arch::Msp430).unwrap();
        // MSP430 NOP is 0x4303 (MOV #0, R3) — actually MOV r3, r3 = 0x4303.
        sim.load_msp430(0, &[0x03, 0x43]).unwrap();
        let result = sim.step();
        assert_eq!(result, StepResult::Advanced);
        assert_eq!(sim.program_counter(), 2, "MSP430 PC advances by 2");
    }

    #[test]
    fn v850_core_loads_and_steps_via_simulator() {
        let mut sim = Simulator::for_arch(Arch::V850).unwrap();
        // V850 NOP (0x0000).
        sim.load_v850(0, &[0x00, 0x00]).unwrap();
        let result = sim.step();
        assert_eq!(result, StepResult::Advanced);
        assert_eq!(sim.program_counter(), 2, "V850 PC advances by 2");
    }

    #[test]
    fn avr_msp430_v850_registers_and_disassembly() {
        let mut sim = Simulator::for_arch(Arch::Avr).unwrap();
        sim.write_reg(5, 0xAB);
        assert_eq!(sim.read_reg(5), 0xAB, "AVR register write");

        let mut sim = Simulator::for_arch(Arch::Msp430).unwrap();
        sim.write_reg(4, 0x1234);
        assert_eq!(sim.read_reg(4), 0x1234, "MSP430 register write");

        let mut sim = Simulator::for_arch(Arch::V850).unwrap();
        sim.write_reg(4, 0xDEAD);
        assert_eq!(sim.read_reg(4), 0xDEAD, "V850 register write");
        // V850 NOP disassembles.
        let bytes = [0x00, 0x00];
        let read = |addr: u32| {
            let i = addr as usize;
            if i < 2 { bytes[i] } else { 0 }
        };
        assert_eq!(disassemble_one(Arch::V850, 0, &read), "nop");
    }

    #[test]
    fn new_archs_supported_by_for_arch_and_labels() {
        for arch in [Arch::Avr, Arch::Msp430, Arch::V850, Arch::Stm8] {
            assert!(Simulator::for_arch(arch).is_ok(), "{arch:?} for_arch");
        }
        assert_eq!(reg_count_for(Arch::Avr), 32);
        assert_eq!(reg_count_for(Arch::Msp430), 16);
        assert_eq!(reg_count_for(Arch::V850), 32);
        assert_eq!(reg_count_for(Arch::Stm8), 5);
        assert_eq!(reg_label_for(Arch::Avr, 3), "R3");
        assert_eq!(reg_label_for(Arch::V850, 3), "sp");
        assert_eq!(reg_label_for(Arch::Stm8, 0), "A");
    }

    #[test]
    fn from_elf_machine_maps_new_architectures() {
        assert_eq!(Arch::from_elf_machine(83), Some(Arch::Avr)); // EM_AVR
        assert_eq!(Arch::from_elf_machine(167), Some(Arch::Msp430)); // EM_MSP430_2
        assert_eq!(Arch::from_elf_machine(87), Some(Arch::V850)); // EM_V850
        assert_eq!(Arch::from_elf_machine(186), Some(Arch::Stm8)); // EM_STM8
    }

    #[test]
    fn stm8_core_loads_and_steps_via_simulator() {
        let mut sim = Simulator::for_arch(Arch::Stm8).unwrap();
        // LD A, #0x55 (0xA6, 0x55); then NOP (0x9D).
        let image = [0xA6, 0x55, 0x9D];
        sim.load_stm8(0x8000, &image).unwrap();
        assert_eq!(sim.program_counter(), 0x8000);
        let result = sim.step();
        assert_eq!(result, StepResult::Advanced);
        assert_eq!(sim.read_reg(0), 0x55, "STM8 A register");
        assert_eq!(sim.program_counter(), 0x8002);
    }

    #[test]
    fn stm8_breakpoint_stops_before_execute() {
        let mut sim = Simulator::for_arch(Arch::Stm8).unwrap();
        sim.load_stm8(0x8000, &[0x9D]).unwrap();
        sim.set_breakpoint(0x8000);
        let result = sim.step();
        assert_eq!(result, StepResult::Breakpoint);
        assert_eq!(sim.program_counter(), 0x8000, "breakpoint at entry");
    }

    #[test]
    fn stm8_alu_add_and_store() {
        let mut sim = Simulator::for_arch(Arch::Stm8).unwrap();
        // LD A, #10 (A6 0A); ADD A, #5 (AB 05); LD 0x2000, A (C7 00 20).
        let image = [0xA6, 0x0A, 0xAB, 0x05, 0xC7, 0x00, 0x20];
        sim.load_stm8(0x8000, &image).unwrap();
        for _ in 0..3 {
            assert_eq!(sim.step(), StepResult::Advanced);
        }
        assert_eq!(sim.read_reg(0), 15, "A = 10 + 5");
        assert_eq!(sim.read_u8(0x2000), 15, "store to direct address");
    }

    /// Cycle-accurate modeling must advance `cycle_count` by the instruction's
    /// estimated latency (loads/stores/branches > 1), while the default mode
    /// stays at 1 cycle per instruction.
    #[test]
    fn cycle_accurate_accounts_instruction_latency() {
        let mut sim = Simulator::new();
        sim.memory[0] = 0x13; // ADDI x1, x0, 1 (1 cycle)
        sim.memory[4] = 0x23; // SW x1, 0(x2) (store, 2 cycles)
        sim.pc = 0;
        // Default: 1 cycle per instruction regardless of type.
        assert_eq!(sim.step(), StepResult::Advanced);
        assert_eq!(sim.cycle_count, 1);
        assert_eq!(sim.last_instruction_cycles, 1);
        sim.pc = 4;
        assert_eq!(sim.step(), StepResult::Advanced);
        assert_eq!(sim.cycle_count, 2);
        // Cycle-accurate: the store costs 2 cycles.
        let mut sim = Simulator::new();
        sim.cycle_accurate = true;
        sim.memory[0] = 0x13;
        sim.memory[4] = 0x23;
        sim.pc = 0;
        assert_eq!(sim.step(), StepResult::Advanced);
        assert_eq!(sim.cycle_count, 1);
        assert_eq!(sim.last_instruction_cycles, 1);
        sim.pc = 4;
        assert_eq!(sim.step(), StepResult::Advanced);
        assert_eq!(sim.cycle_count, 3, "store adds 2 cycles");
        assert_eq!(sim.last_instruction_cycles, 2);
    }

    /// The condition evaluator parses C-like expressions over register names,
    /// `pc`, `cycle`, and literals; malformed input is false, empty is true.
    #[test]
    fn eval_condition_parses_expressions() {
        let regs = SimRegs {
            named: vec![
                ("x1".to_string(), 10),
                ("x2".to_string(), 20),
                ("a0".to_string(), 7),
            ],
        };
        // Comparisons and arithmetic.
        assert!(eval_condition("x1 == 10", &regs, 0, 0));
        assert!(eval_condition("x1 < x2", &regs, 0, 0));
        assert!(eval_condition("x1 + x2 == 30", &regs, 0, 0));
        assert!(eval_condition("x2 % 3 == 2", &regs, 0, 0));
        // pc / cycle synthetics.
        assert!(eval_condition("pc == 0x1000", &regs, 0x1000, 0));
        assert!(eval_condition("cycle >= 100", &regs, 0, 150));
        // Logical ops and unary.
        assert!(eval_condition("x1 > 5 && x2 > 15", &regs, 0, 0));
        assert!(eval_condition("!(x1 > 50)", &regs, 0, 0));
        assert!(eval_condition("x1 > 50 || x2 > 15", &regs, 0, 0));
        // Hex / binary / octal literals.
        assert!(eval_condition("a0 == 0x7", &regs, 0, 0));
        assert!(eval_condition("a0 == 0b111", &regs, 0, 0));
        assert!(eval_condition("a0 == 0o7", &regs, 0, 0));
        // Parentheses change precedence.
        assert!(eval_condition("(x1 + x2) * 2 == 60", &regs, 0, 0));
        // Empty = unconditional; unknown register / malformed = false.
        assert!(eval_condition("", &regs, 0, 0));
        assert!(!eval_condition("zzz == 1", &regs, 0, 0));
        assert!(!eval_condition("x1 ==", &regs, 0, 0));
        assert!(!eval_condition("x1 +", &regs, 0, 0));
    }

    /// A hit-count breakpoint pauses only on the N-th hit; a condition
    /// breakpoint pauses only when the expression is true.
    #[test]
    fn conditional_and_hit_count_breakpoints() {
        // RISC-V ADDI x1, x0, 1 at 0, loop back with BEQ to test hits.
        // Program: x1 increments each pass; BEQ x1, x0, +0 (loop) at 4.
        let mut sim = Simulator::new();
        sim.write_u32(0, 0x00100093); // ADDI x1, x0, 1

        // Hit-count 3: pauses only on the 3rd hit.
        sim.pc = 0;

        sim.set_breakpoint_ex(0, "", 3);
        let mut result = sim.step();
        assert_eq!(result, StepResult::Advanced, "1st hit passes");
        sim.pc = 0;
        result = sim.step();
        assert_eq!(result, StepResult::Advanced, "2nd hit passes");
        sim.pc = 0;
        result = sim.step();
        assert_eq!(result, StepResult::Breakpoint, "3rd hit pauses");
        assert_eq!(sim.breakpoint_hits(0), 3);

    }

    /// A one-shot breakpoint pauses on the first hit and is then removed
    /// automatically, so a subsequent step does not stop there again.
    #[test]
    fn once_breakpoint_autoclears() {
        let mut sim = Simulator::new();
        sim.write_u32(0, 0x00100093); // ADDI x1, x0, 1
        sim.write_u32(4, 0x00108093); // ADDI x1, x1, 1

        sim.set_breakpoint_ex_once(4, "", 0);
        sim.pc = 0;
        assert_eq!(sim.step(), StepResult::Advanced);
        assert_eq!(sim.step(), StepResult::Breakpoint, "stops at the once bp");
        eprintln!("DBG after disabled step pc={:x}", sim.program_counter()); assert_eq!(sim.program_counter(), 4);
        assert!(!sim.breakpoint_is_once(4), "once bp must be removed");
        assert!(sim.breakpoints.is_empty(), "breakpoint set is now empty");

        // A subsequent step runs past 4 without stopping.
        sim.pc = 4;
        assert_eq!(sim.step(), StepResult::Advanced);
        assert_eq!(sim.program_counter(), 8);

    }

    /// A disabled breakpoint stays in the set but never pauses; re-enabling it
    /// restores the pause.
    #[test]
    fn disabled_breakpoint_does_not_pause() {
        let mut sim = Simulator::new();
        sim.write_u32(0, 0x00100093); // ADDI x1, x0, 1
        sim.pc = 0;

        sim.set_breakpoint(0);
        assert!(sim.breakpoint_is_enabled(0));
        sim.set_breakpoint_enabled(0, false);
        assert!(!sim.breakpoint_is_enabled(0));
        // Disabled: the instruction executes normally.
        assert_eq!(sim.step(), StepResult::Advanced);
        assert_eq!(sim.program_counter(), 4);
        // Re-enable and it pauses again at the entry.
        sim.set_breakpoint_enabled(0, true);
        sim.pc = 0;
        assert_eq!(sim.step(), StepResult::Breakpoint);
        assert_eq!(sim.program_counter(), 0);
    }

    /// A condition breakpoint stops only when the register equals the target.
    #[test]
    fn condition_breakpoint_stops_on_true() {
        let mut sim = Simulator::new();
        sim.write_u32(0, 0x00100093); // ADDI x1, x0, 1

        // Stop when x1 == 1 (i.e. after the first pass it holds 1).
        sim.set_breakpoint_ex(0, "x1 == 1", 0);
        sim.pc = 0;
        let r = sim.step();
        // After ADDI, x1 = 1; loop back to 0 and the breakpoint sees x1 == 1.
        sim.pc = 0;
        assert_eq!(r, StepResult::Advanced);
        assert_eq!(sim.step(), StepResult::Breakpoint);
    }

}
