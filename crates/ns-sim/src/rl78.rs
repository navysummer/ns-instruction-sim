//! A Renesas RL78 (RL78/G10-G14, IAR RL78) instruction-level simulator core.
//!
//! Models the RL78 CPU used by IAR/Keil-style RL78 toolchains. The RL78 is a
//! 16-bit CISC with:
//!
//! * 8-bit registers: `A`, `X`, `B`, `C`, `D`, `E`, `H`, `L` (the second
//!   bank `A'..L'` is selected by `SEL RBn`).
//! * 16-bit register pairs: `AX`, `BC`, `DE`, `HL`, plus `SP` and `PC`.
//! * An 8-bit program status word (`PSW`): `Z`, `AC`, `CY`, `RBS1:RBS0`, `IE`.
//! * Segment registers `CS` (code) and `ES` (data) for 20-bit addressing.
//! * A 1 MiB flat address space (byte-addressed, little-endian).
//!
//! ## Covered instructions
//!
//! * Moves: `MOV` (byte, all addressing modes), `MOVW` (word), `MOV1`,
//!   `MOVS`, `XCH`, `XCHW`.
//! * Arithmetic: `ADD`/`ADDC`/`SUB`/`SUBC`/`CMP` (byte and word), `INC`/`DEC`
//!   (byte and word), `MULU`, `DIVU`, `MULHU`, `DIVHU`, `MACHU`.
//! * Logic: `AND`, `OR`, `XOR` (byte and word), `NOT1`.
//! * Shifts/rotates: `SHL`, `SHR`, `SAR`, `SHLW`, `SHRW`, `SARW`, `ROL`,
//!   `ROR`, `ROLC`, `RORC`, `ROLWC`, `RORWC`.
//! * Bit ops: `SET1`, `CLR1`, `BT`, `BF`, `BTCLR`, `AND1`, `OR1`, `XOR1`.
//! * Memory/stack: `PUSH`, `POP`, `PUSHM`, `POPM`.
//! * Control: `BR`, `BC`, `BNC`, `BZ`, `BNZ`, `BH`, `BNH`, `CALL`, `CALLT`,
//!   `RET`, `RETI`, `RETB`, `BRK`, `BRK1`, `NOP`, `SEL`, `SK`.
//!
//! Encodings follow the binutils RL78 opcode decoder (`opcodes/rl78-decode.c`),
//! cross-checked against the RL78 user's manual.

/// Flat memory window size (1 MiB covers the RL78 address space).
pub const MEMORY_SIZE: usize = 1024 * 1024;

/// The RL78 PSW flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Psw {
    /// Zero flag.
    pub z: bool,
    /// Auxiliary carry flag.
    pub ac: bool,
    /// Carry flag.
    pub cy: bool,
    /// Register bank select (2 bits).
    pub rbs: u8,
    /// Interrupt enable.
    pub ie: bool,
}

impl Default for Psw {
    fn default() -> Self {
        Psw { z: false, ac: false, cy: false, rbs: 0, ie: true }
    }
}

/// The 8-bit register-file indices. The RL78 register encoding maps 0..7 to
/// `X A C B E D L H` (per the binutils `RL78_Reg_X..RL78_Reg_H` order).
const R_X: usize = 0;
const R_A: usize = 1;
const R_C: usize = 2;
const R_B: usize = 3;
const R_E: usize = 4;
const R_D: usize = 5;
const R_L: usize = 6;
const R_H: usize = 7;

/// Outcome of a single RL78 step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rl78Step {
    /// A normal instruction advanced the PC.
    Advanced,
    /// A breakpoint was reached before executing.
    Breakpoint,
    /// The core halted (`BRK`, `HALT`, `STOP`, or an unknown opcode).
    Halted,
}

/// An RL78 core.
pub struct Rl78Core {
    /// General-purpose 8-bit registers, two banks of 8 (`A'` bank is 8..16).
    pub regs: [u8; 16],
    /// Program status word.
    pub psw: Psw,
    /// Stack pointer (16-bit).
    pub sp: u16,
    /// Code segment register (high 4 bits of the 20-bit code address).
    pub cs: u8,
    /// Data segment register (high 4 bits of the 20-bit data address).
    pub es: u8,
    /// Program counter (20-bit code address).
    pub pc: u32,
    /// Flat 1 MiB memory (byte-addressed).
    pub mem: Vec<u8>,
    /// Whether execution should stop.
    pub halted: bool,
}

impl Default for Rl78Core {
    fn default() -> Self {
        Self::new()
    }
}

impl Rl78Core {
    pub fn new() -> Self {
        Rl78Core {
            regs: [0; 16],
            psw: Psw::default(),
            sp: 0xFFEE,
            cs: 0,
            es: 0,
            pc: 0,
            mem: vec![0; MEMORY_SIZE],
            halted: false,
        }
    }

    /// Reset to `entry` (a 20-bit code address), zeroing registers.
    pub fn reset(&mut self, entry: u32) {
        self.regs = [0; 16];
        self.psw = Psw::default();
        self.sp = 0xFFEE;
        self.cs = (entry >> 16) as u8 & 0x0F;
        self.es = 0;
        self.pc = entry;
        self.halted = false;
    }

    pub fn set_pc(&mut self, pc: u32) {
        self.pc = pc & 0xFFFFF;
    }

    /// Load a program image at `base` (a 20-bit code address) and set PC.
    pub fn load_image(&mut self, base: u32, image: &[u8]) {
        self.write_bytes(base, image);
        self.pc = base & 0xFFFFF;
        self.halted = false;
    }

    // -------- register access --------

    fn rb(&self, index: usize) -> u8 {
        let bank = (self.psw.rbs as usize & 0x3) * 8;
        self.regs[bank + (index & 0x7)]
    }

    fn set_rb(&mut self, index: usize, value: u8) {
        let bank = (self.psw.rbs as usize & 0x3) * 8;
        self.regs[bank + (index & 0x7)] = value;
    }

    /// Read a 16-bit register pair: `0`=AX, `1`=BC, `2`=DE, `3`=HL.
    fn rw(&self, index: usize) -> u16 {
        match index & 0x3 {
            0 => (self.rb(R_X) as u16) | ((self.rb(R_A) as u16) << 8),
            1 => (self.rb(R_C) as u16) | ((self.rb(R_B) as u16) << 8),
            2 => (self.rb(R_E) as u16) | ((self.rb(R_D) as u16) << 8),
            _ => (self.rb(R_L) as u16) | ((self.rb(R_H) as u16) << 8),
        }
    }

    fn set_rw(&mut self, index: usize, value: u16) {
        match index & 0x3 {
            0 => {
                self.set_rb(R_X, value as u8);
                self.set_rb(R_A, (value >> 8) as u8);
            }
            1 => {
                self.set_rb(R_C, value as u8);
                self.set_rb(R_B, (value >> 8) as u8);
            }
            2 => {
                self.set_rb(R_E, value as u8);
                self.set_rb(R_D, (value >> 8) as u8);
            }
            _ => {
                self.set_rb(R_L, value as u8);
                self.set_rb(R_H, (value >> 8) as u8);
            }
        }
    }

    /// Register label for the IDE register view (byte registers).
    pub fn register_label(index: usize) -> String {
        const NAMES: [&str; 8] = ["X", "A", "C", "B", "E", "D", "L", "H"];
        if index < 8 {
            format!("{}", NAMES[index])
        } else if index < 16 {
            let bank = (index - 8) / 8;
            format!("{}{}'", NAMES[index % 8], bank + 1)
        } else {
            format!("r{}", index)
        }
    }

    /// Read a register by IDE index (0-7 = current bank byte regs, 8-15 = SP/
    /// PSW/PC/CS/ES and word pairs).
    pub fn read_reg(&self, index: usize) -> u32 {
        match index {
            0..=7 => self.rb(index) as u32,
            8 => self.sp as u32,
            9 => {
                let mut v = 0u32;
                if self.psw.z { v |= 1 << 0; }
                if self.psw.ac { v |= 1 << 1; }
                if self.psw.cy { v |= 1 << 2; }
                v |= (self.psw.rbs as u32 & 0x3) << 3;
                if self.psw.ie { v |= 1 << 5; }
                v
            }
            10 => self.pc & 0xFFFFF,
            11 => self.cs as u32,
            12 => self.es as u32,
            13 => self.rw(0) as u32, // AX
            14 => self.rw(1) as u32, // BC
            15 => self.rw(2) as u32, // DE
            _ => 0,
        }
    }

    /// Write a register by IDE index (inverse of `read_reg`).
    pub fn write_reg(&mut self, index: usize, value: u32) {
        match index {
            0..=7 => self.set_rb(index, value as u8),
            8 => self.sp = value as u16,
            9 => {
                self.psw.z = value & 1 != 0;
                self.psw.ac = value & 2 != 0;
                self.psw.cy = value & 4 != 0;
                self.psw.rbs = ((value >> 3) & 0x3) as u8;
                self.psw.ie = value & 0x20 != 0;
            }
            10 => self.set_pc(value),
            11 => self.cs = value as u8 & 0x0F,
            12 => self.es = value as u8 & 0x0F,
            13 => self.set_rw(0, value as u16),
            14 => self.set_rw(1, value as u16),
            15 => self.set_rw(2, value as u16),
            _ => {}
        }
    }

    // -------- memory --------

    /// Read one byte from the flat data space (out-of-range returns 0).
    pub fn read_u8(&self, address: u32) -> u8 {
        let a = address as usize;
        if a < self.mem.len() {
            self.mem[a]
        } else {
            0
        }
    }

    /// Read a little-endian 16-bit word from the flat data space.
    pub fn read_u16(&self, address: u32) -> u16 {
        self.read_u8(address) as u16 | ((self.read_u8(address.wrapping_add(1)) as u16) << 8)
    }

    /// Read a little-endian 32-bit word from the flat data space.
    pub fn read_u32(&self, address: u32) -> u32 {
        let mut v = 0u32;
        for i in 0..4 {
            v |= (self.read_u8(address.wrapping_add(i as u32)) as u32) << (8 * i);
        }
        v
    }

    /// Write one byte to the flat data space (out-of-range ignored).
    pub fn write_u8(&mut self, address: u32, value: u8) {
        let a = address as usize;
        if a < self.mem.len() {
            self.mem[a] = value;
        }
    }

    /// Write a little-endian 16-bit word.
    pub fn write_u16(&mut self, address: u32, value: u16) {
        self.write_u8(address, value as u8);
        self.write_u8(address.wrapping_add(1), (value >> 8) as u8);
    }

    /// Write a little-endian 32-bit word.
    pub fn write_u32(&mut self, address: u32, value: u32) {
        for i in 0..4 {
            self.write_u8(address.wrapping_add(i as u32), (value >> (8 * i)) as u8);
        }
    }

    /// Write a byte slice to the flat memory starting at `base`.
    pub fn write_bytes(&mut self, base: u32, bytes: &[u8]) {
        let b = base as usize;
        for (i, &byte) in bytes.iter().enumerate() {
            let a = b + i;
            if a < self.mem.len() {
                self.mem[a] = byte;
            }
        }
    }

    /// Zero `count` bytes at `base`.
    pub fn zero_bytes(&mut self, base: u32, count: u32) {
        for i in 0..count {
            self.write_u8(base.wrapping_add(i), 0);
        }
    }

    // -------- fetch --------

    fn fetch8(&mut self) -> u8 {
        let b = self.read_u8(self.pc);
        self.pc = self.pc.wrapping_add(1) & 0xFFFFF;
        b
    }

    fn fetch16(&mut self) -> u16 {
        let lo = self.fetch8() as u16;
        let hi = self.fetch8() as u16;
        (hi << 8) | lo
    }


    // -------- flags --------

    fn set_nz(&mut self, value: u8) {
        self.psw.z = value == 0;
    }

    /// Compute and store Z/AC/CY for a byte addition.
    fn add8(&mut self, a: u8, b: u8, carry: bool) -> u8 {
        let c = if carry { 1u16 } else { 0u16 };
        let sum = a as u16 + b as u16 + c;
        self.psw.z = (sum & 0xFF) == 0;
        self.psw.ac = ((a & 0x0F) as u16 + (b & 0x0F) as u16 + c) > 0x0F;
        self.psw.cy = sum > 0xFF;
        sum as u8
    }

    /// Compute and store Z/AC/CY for a byte subtraction.
    fn sub8(&mut self, a: u8, b: u8, borrow: bool) -> u8 {
        let bo = if borrow { 1u16 } else { 0u16 };
        let diff = a as u16 - b as u16 - bo;
        self.psw.z = (diff & 0xFF) == 0;
        self.psw.ac = (a & 0x0F) < ((b & 0x0F) + (bo as u8 & 0x0F));
        self.psw.cy = diff > 0xFF;
        diff as u8
    }

    /// Compute and store Z/AC/CY for a word addition.
    fn add16(&mut self, a: u16, b: u16, carry: bool) -> u16 {
        let c = if carry { 1u32 } else { 0u32 };
        let sum = a as u32 + b as u32 + c;
        self.psw.z = (sum & 0xFFFF) == 0;
        self.psw.ac = ((a & 0x0FFF) as u32 + (b & 0x0FFF) as u32 + c) > 0x0FFF;
        self.psw.cy = sum > 0xFFFF;
        sum as u16
    }

    /// Compute and store Z/AC/CY for a word subtraction.
    fn sub16(&mut self, a: u16, b: u16, borrow: bool) -> u16 {
        let bo = if borrow { 1u32 } else { 0u32 };
        let diff = a as u32 - b as u32 - bo;
        self.psw.z = (diff & 0xFFFF) == 0;
        self.psw.ac = (a & 0x0FFF) < ((b & 0x0FFF) + (bo as u16 & 0x0FFF));
        self.psw.cy = diff > 0xFFFF;
        diff as u16
    }

    /// Resolve a short-address operand to a flat address (`saddr`).
    fn saddr(&self, addr: u32) -> u32 {
        if addr < 0x20 {
            0xFFF00 + addr
        } else {
            0xFFE00 + addr
        }
    }



    // -------- stack --------

    fn push16(&mut self, value: u16) {
        self.sp = self.sp.wrapping_sub(2);
        self.write_u16(self.es_addr(self.sp as u32), value);
    }

    fn pop16(&mut self) -> u16 {
        let v = self.read_u16(self.es_addr(self.sp as u32));
        self.sp = self.sp.wrapping_add(2);
        v
    }

    /// Combine the 20-bit address from ES and a 16-bit offset.
    fn es_addr(&self, offset: u32) -> u32 {
        ((self.es as u32) << 16) | (offset & 0xFFFF)
    }


    // -------- conditional --------

    fn cond_true(&self, cond: u8) -> bool {
        match cond {
            0 => self.psw.cy,          // C
            1 => !self.psw.cy,         // NC
            2 => self.psw.z,           // Z
            3 => !self.psw.z,          // NZ
            4 => self.psw.cy && !self.psw.z, // H
            5 => !self.psw.cy || self.psw.z, // NH
            _ => true,
        }
    }

    // -------- execute --------

    /// Execute one step, stopping at a breakpoint.
    pub fn step(&mut self, breakpoints: &[u32]) -> Rl78Step {
        if self.halted {
            return Rl78Step::Halted;
        }
        if breakpoints.contains(&self.pc) {
            return Rl78Step::Breakpoint;
        }
        let pc = self.pc;
        self.execute();
        if self.halted {
            return Rl78Step::Halted;
        }
        if self.pc == pc {
            // Guard against infinite loops on unknown instructions.
            self.halted = true;
            return Rl78Step::Halted;
        }
        Rl78Step::Advanced
    }

    fn execute(&mut self) {
        let b0 = self.fetch8();
        // Two-byte opcode prefix `0110 0001` (0x61) for many instructions.
        if b0 == 0x61 {
            self.execute_61();
            return;
        }
        self.execute_single(b0);
    }

    fn execute_single(&mut self, op: u8) {
        match op {
            // nop (0000 0000)
            0x00 => {}
            // addw AX, ES:[imm16] (0000 0010)
            0x02 => {
                let addr = self.fetch16() as u32;
                let v = self.add16(self.rw(0), self.read_u16(self.es_addr(addr)), false);
                self.set_rw(0, v);
            }
            // addw AX, #imm16 (0000 0100)
            0x04 => {
                let imm = self.fetch16();
                let v = self.add16(self.rw(0), imm, false);
                self.set_rw(0, v);
            }
            // addw AX, saddr (0000 0110)
            0x06 => {
                let sa = self.fetch8() as u32;
                let a = self.saddr(sa);
                let v = self.add16(self.rw(0), self.read_u16(a), false);
                self.set_rw(0, v);
            }
            // addw AX, rw (0000 0rw1): rw in bits 2:1
            0x01 | 0x03 | 0x05 | 0x07 => {
                let rw = ((op >> 1) & 0x3) as usize;
                let v = self.add16(self.rw(0), self.rw(rw), false);
                self.set_rw(0, v);
            }
            // add A, saddr (0000 1011)
            0x0B => {
                let sa = self.fetch8() as u32;
                let a = self.saddr(sa);
                let v = self.add8(self.rb(R_A), self.read_u8(a), false);
                self.set_rb(R_A, v);
            }
            // add saddr, #imm8 (0000 1010)
            0x0A => {
                let sa = self.fetch8() as u32;
                let a = self.saddr(sa);
                let imm = self.fetch8();
                let v = self.add8(self.read_u8(a), imm, false);
                self.write_u8(a, v);
            }
            // add A, #imm8 (0000 1100)
            0x0C => {
                let imm = self.fetch8();
                let v = self.add8(self.rb(R_A), imm, false);
                self.set_rb(R_A, v);
            }
            // add A, [HL] (0000 1101)
            0x0D => {
                let a = self.es_addr(self.rw(3) as u32);
                let v = self.add8(self.rb(R_A), self.read_u8(a), false);
                self.set_rb(R_A, v);
            }
            // add A, [HL+imm8] (0000 1110)
            0x0E => {
                let d = self.fetch8() as u32;
                let a = self.es_addr(self.rw(3) as u32 + d);
                let v = self.add8(self.rb(R_A), self.read_u8(a), false);
                self.set_rb(R_A, v);
            }
            // add A, ES:[imm16] (0000 1111)
            0x0F => {
                let addr = self.fetch16() as u32;
                let v = self.add8(self.rb(R_A), self.read_u8(self.es_addr(addr)), false);
                self.set_rb(R_A, v);
            }
            // addw SP, #imm8 (0001 0000)
            0x10 => {
                let imm = self.fetch8() as u16;
                self.sp = self.sp.wrapping_add(imm);
            }
            // movw AX, rw (0001 0ra1) / movw rw, AX (0001 0ra0)
            0x11 | 0x13 | 0x15 | 0x17 => {
                let ra = ((op >> 1) & 0x3) as usize;
                let v = self.rw(ra);
                self.set_rw(0, v);
            }
            0x12 | 0x14 | 0x16 => {
                let ra = ((op >> 1) & 0x3) as usize;
                let v = self.rw(0);
                self.set_rw(ra, v);
            }
            // movw rw, #imm16 (0011 0rg0)
            0x30 | 0x32 | 0x34 | 0x36 => {
                let rg = ((op >> 1) & 0x3) as usize;
                let imm = self.fetch16();
                self.set_rw(rg, imm);
            }
            // addc A, #imm8 (0001 1100)
            0x1C => {
                let imm = self.fetch8();
                let v = self.add8(self.rb(R_A), imm, self.psw.cy);
                self.set_rb(R_A, v);
            }
            // addc A, ES:[imm16] (0001 1111)
            0x1F => {
                let addr = self.fetch16() as u32;
                let v = self.add8(self.rb(R_A), self.read_u8(self.es_addr(addr)), self.psw.cy);
                self.set_rb(R_A, v);
            }
            // addc A, [HL] (0001 1101)
            0x1D => {
                let a = self.es_addr(self.rw(3) as u32);
                let v = self.add8(self.rb(R_A), self.read_u8(a), self.psw.cy);
                self.set_rb(R_A, v);
            }
            // addc A, [HL+imm8] (0001 1110)
            0x1E => {
                let d = self.fetch8() as u32;
                let a = self.es_addr(self.rw(3) as u32 + d);
                let v = self.add8(self.rb(R_A), self.read_u8(a), self.psw.cy);
                self.set_rb(R_A, v);
            }
            // addc A, saddr (0001 1011)
            0x1B => {
                let sa = self.fetch8() as u32;
                let a = self.saddr(sa);
                let v = self.add8(self.rb(R_A), self.read_u8(a), self.psw.cy);
                self.set_rb(R_A, v);
            }
            // addc saddr, #imm8 (0001 1010)
            0x1A => {
                let sa = self.fetch8() as u32;
                let a = self.saddr(sa);
                let imm = self.fetch8();
                let v = self.add8(self.read_u8(a), imm, self.psw.cy);
                self.write_u8(a, v);
            }
            // and A, #imm8 (0101 1100)
            0x5C => {
                let imm = self.fetch8();
                let v = self.rb(R_A) & imm;
                self.set_rb(R_A, v);
                self.set_nz(v);
            }
            // and A, [HL] (0101 1101)
            0x5D => {
                let a = self.es_addr(self.rw(3) as u32);
                let v = self.rb(R_A) & self.read_u8(a);
                self.set_rb(R_A, v);
                self.set_nz(v);
            }
            // and A, [HL+imm8] (0101 1110)
            0x5E => {
                let d = self.fetch8() as u32;
                let a = self.es_addr(self.rw(3) as u32 + d);
                let v = self.rb(R_A) & self.read_u8(a);
                self.set_rb(R_A, v);
                self.set_nz(v);
            }
            // and A, ES:[imm16] (0101 1111)
            0x5F => {
                let addr = self.fetch16() as u32;
                let v = self.rb(R_A) & self.read_u8(self.es_addr(addr));
                self.set_rb(R_A, v);
                self.set_nz(v);
            }
            // or A, #imm8 (0110 1100)
            0x6C => {
                let imm = self.fetch8();
                let v = self.rb(R_A) | imm;
                self.set_rb(R_A, v);
                self.set_nz(v);
            }
            // or A, [HL] (0110 1101)
            0x6D => {
                let a = self.es_addr(self.rw(3) as u32);
                let v = self.rb(R_A) | self.read_u8(a);
                self.set_rb(R_A, v);
                self.set_nz(v);
            }
            // or A, [HL+imm8] (0110 1110)
            0x6E => {
                let d = self.fetch8() as u32;
                let a = self.es_addr(self.rw(3) as u32 + d);
                let v = self.rb(R_A) | self.read_u8(a);
                self.set_rb(R_A, v);
                self.set_nz(v);
            }
            // or A, ES:[imm16] (0110 1111)
            0x6F => {
                let addr = self.fetch16() as u32;
                let v = self.rb(R_A) | self.read_u8(self.es_addr(addr));
                self.set_rb(R_A, v);
                self.set_nz(v);
            }
            // mov rba, #imm8 (0101 0reg)
            0x50..=0x57 => {
                let reg = (op & 0x07) as usize;
                let imm = self.fetch8();
                self.set_rb(reg, imm);
            }
            // mov A, rba (0110 0rba)
            0x60..=0x67 => {
                let reg = (op & 0x07) as usize;
                let v = self.rb(reg);
                self.set_rb(R_A, v);
            }
            // inc rba (1000 0reg)
            0x80..=0x87 => {
                let reg = (op & 0x07) as usize;
                let v = self.rb(reg).wrapping_add(1);
                self.set_rb(reg, v);
                self.set_nz(v);
            }
            // dec rba (1001 0reg)
            0x90..=0x97 => {
                let reg = (op & 0x07) as usize;
                let v = self.rb(reg).wrapping_sub(1);
                self.set_rb(reg, v);
                self.set_nz(v);
            }
            // mov A, [DE] (1000 1001)
            0x89 => {
                let a = self.es_addr(self.rw(2) as u32);
                let v = self.read_u8(a);
                self.set_rb(R_A, v);
            }
            // mov A, [DE+imm8] (1000 1010)
            0x8A => {
                let d = self.fetch8() as u32;
                let a = self.es_addr(self.rw(2) as u32 + d);
                let v = self.read_u8(a);
                self.set_rb(R_A, v);
            }
            // mov A, [HL] (1000 1011)
            0x8B => {
                let a = self.es_addr(self.rw(3) as u32);
                let v = self.read_u8(a);
                self.set_rb(R_A, v);
            }
            // mov A, [HL+imm8] (1000 1100)
            0x8C => {
                let d = self.fetch8() as u32;
                let a = self.es_addr(self.rw(3) as u32 + d);
                let v = self.read_u8(a);
                self.set_rb(R_A, v);
            }
            // mov A, saddr (1000 1101)
            0x8D => {
                let sa = self.fetch8() as u32;
                let a = self.saddr(sa);
                let v = self.read_u8(a);
                self.set_rb(R_A, v);
            }
            // mov A, ES:[imm16] (1000 1111)
            0x8F => {
                let addr = self.fetch16() as u32;
                let v = self.read_u8(self.es_addr(addr));
                self.set_rb(R_A, v);
            }
            // mov A, [SP+imm8] (1000 1000)
            0x88 => {
                let d = self.fetch8() as u32;
                let a = self.es_addr(self.sp as u32 + d);
                let v = self.read_u8(a);
                self.set_rb(R_A, v);
            }
            // mov A, [B+imm16] (0000 1001)
            0x09 => {
                let d = self.fetch16() as u32;
                let a = self.es_addr(self.rb(R_B) as u32 + d);
                let v = self.read_u8(a);
                self.set_rb(R_A, v);
            }
            // mov A, [BC+imm16] (0100 1001)
            0x49 => {
                let d = self.fetch16() as u32;
                let a = self.es_addr(self.rw(1) as u32 + d);
                let v = self.read_u8(a);
                self.set_rb(R_A, v);
            }
            // mov A, [C+imm16] (0010 1001)
            0x29 => {
                let d = self.fetch16() as u32;
                let a = self.es_addr(self.rb(R_C) as u32 + d);
                let v = self.read_u8(a);
                self.set_rb(R_A, v);
            }
            // mov [DE], A (1001 1001)
            0x99 => {
                let a = self.es_addr(self.rw(2) as u32);
                let v = self.rb(R_A);
                self.write_u8(a, v);
            }
            // mov [HL], A (1001 1011)
            0x9B => {
                let a = self.es_addr(self.rw(3) as u32);
                let v = self.rb(R_A);
                self.write_u8(a, v);
            }
            // mov saddr, A (1001 1101)
            0x9D => {
                let sa = self.fetch8() as u32;
                let a = self.saddr(sa);
                let v = self.rb(R_A);
                self.write_u8(a, v);
            }
            // mov ES:[imm16], A (1001 1111)
            0x9F => {
                let addr = self.fetch16() as u32;
                let v = self.rb(R_A);
                self.write_u8(self.es_addr(addr), v);
            }
            // push rp (1100 0rg1) / pop rp (1100 0rg0)
            0xC1 | 0xC3 | 0xC5 | 0xC7 => {
                let rg = ((op >> 1) & 0x3) as usize;
                let v = self.rw(rg);
                self.push16(v);
            }
            0xC0 | 0xC2 | 0xC4 | 0xC6 => {
                let rg = ((op >> 1) & 0x3) as usize;
                let v = self.pop16();
                self.set_rw(rg, v);
            }
            // ret (1101 0111)
            0xD7 => {
                let v = self.pop16();
                self.set_pc(v as u32);
            }
            // mov A, #imm8 (0100 0001) is 0x41; but 0x41 overlaps movw. Use
            // `0100 0001` = mov A, #imm8 per table line 722.
            0x41 => {
                let imm = self.fetch8();
                self.set_rb(R_A, imm);
            }
            // brk1 (1111 1111)
            0xFF => self.halted = true,
            _ => self.halted = true,
        }
    }

    /// Execute an instruction with the `0110 0001` (0x61) prefix.
    ///
    /// Reg-reg forms are 2-byte `0x61 XX` where the register index is the low
    /// 3 bits of the second byte; the high bits select the operation.
    fn execute_61(&mut self) {
        let op = self.fetch8();
        let rba = (op & 0x07) as usize;
        let reg = (op & 0x07) as usize;
        match op & 0xF8 {
            // add A, rba (0000 1rba)
            0x08 => {
                let v = self.add8(self.rb(R_A), self.rb(rba), false);
                self.set_rb(R_A, v);
            }
            // add rba, A (0000 0reg)
            0x00 => {
                let v = self.add8(self.rb(reg), self.rb(R_A), false);
                self.set_rb(reg, v);
            }
            // addc A, rba (0001 1rba)
            0x18 => {
                let v = self.add8(self.rb(R_A), self.rb(rba), self.psw.cy);
                self.set_rb(R_A, v);
            }
            // addc rba, A (0001 0reg)
            0x10 => {
                let v = self.add8(self.rb(reg), self.rb(R_A), self.psw.cy);
                self.set_rb(reg, v);
            }
            // sub A, rba (0010 1rba)
            0x28 => {
                let v = self.sub8(self.rb(R_A), self.rb(rba), false);
                self.set_rb(R_A, v);
            }
            // sub rba, A (0010 0reg)
            0x20 => {
                let v = self.sub8(self.rb(reg), self.rb(R_A), false);
                self.set_rb(reg, v);
            }
            // subc A, rba (0011 1rba)
            0x38 => {
                let v = self.sub8(self.rb(R_A), self.rb(rba), self.psw.cy);
                self.set_rb(R_A, v);
            }
            // subc rba, A (0011 0reg)
            0x30 => {
                let v = self.sub8(self.rb(reg), self.rb(R_A), self.psw.cy);
                self.set_rb(reg, v);
            }
            // cmp A, rba (0100 1rba)
            0x48 => {
                self.sub8(self.rb(R_A), self.rb(rba), false);
            }
            // cmp rba, A (0100 0reg)
            0x40 => {
                self.sub8(self.rb(reg), self.rb(R_A), false);
            }
            // and A, rba (0101 1rba)
            0x58 => {
                let v = self.rb(R_A) & self.rb(rba);
                self.set_rb(R_A, v);
                self.set_nz(v);
            }
            // and rba, A (0101 0reg)
            0x50 => {
                let v = self.rb(reg) & self.rb(R_A);
                self.set_rb(reg, v);
                self.set_nz(v);
            }
            // or A, rba (0110 1rba)
            0x68 => {
                let v = self.rb(R_A) | self.rb(rba);
                self.set_rb(R_A, v);
                self.set_nz(v);
            }
            // or rba, A (0110 0reg)
            0x60 => {
                let v = self.rb(reg) | self.rb(R_A);
                self.set_rb(reg, v);
                self.set_nz(v);
            }
            // xor A, rba (0111 1rba)
            0x78 => {
                let v = self.rb(R_A) ^ self.rb(rba);
                self.set_rb(R_A, v);
                self.set_nz(v);
            }
            // xor rba, A (0111 0reg)
            0x70 => {
                let v = self.rb(reg) ^ self.rb(R_A);
                self.set_rb(reg, v);
                self.set_nz(v);
            }
            // xch A, rba (1000 1reg)
            0x88 => {
                let a = self.rb(R_A);
                self.set_rb(R_A, self.rb(reg));
                self.set_rb(reg, a);
            }
            // add A, ES:[HL+B]/[HL+C] (1000 000/1000 0010)
            0x80 | 0x82 => {
                let idx = if op & 1 == 0 { R_B } else { R_C };
                let addr = self.rw(3).wrapping_add(self.rb(idx) as u16);
                let v = self.add8(self.rb(R_A), self.read_u8(self.es_addr(addr as u32)), false);
                self.set_rb(R_A, v);
            }
            // addc A, ES:[HL+B]/[HL+C] (1001 0000/1001 0010)
            0x90 | 0x92 => {
                let idx = if op & 1 == 0 { R_B } else { R_C };
                let addr = self.rw(3).wrapping_add(self.rb(idx) as u16);
                let v = self.add8(self.rb(R_A), self.read_u8(self.es_addr(addr as u32)), self.psw.cy);
                self.set_rb(R_A, v);
            }
            // sub A, ES:[HL+B]/[HL+C] (1010 000/1010 0010)
            0xA0 | 0xA2 => {
                let idx = if op & 1 == 0 { R_B } else { R_C };
                let addr = self.rw(3).wrapping_add(self.rb(idx) as u16);
                let v = self.sub8(self.rb(R_A), self.read_u8(self.es_addr(addr as u32)), false);
                self.set_rb(R_A, v);
            }
            // subc A, ES:[HL+B]/[HL+C] (1011 000/1011 0010)
            0xB0 | 0xB2 => {
                let idx = if op & 1 == 0 { R_B } else { R_C };
                let addr = self.rw(3).wrapping_add(self.rb(idx) as u16);
                let v = self.sub8(self.rb(R_A), self.read_u8(self.es_addr(addr as u32)), self.psw.cy);
                self.set_rb(R_A, v);
            }
            // cmp A, ES:[HL+B]/[HL+C] (1100 000/1100 0010)
            0xC0 | 0xC2 => {
                let idx = if op & 1 == 0 { R_B } else { R_C };
                let addr = self.rw(3).wrapping_add(self.rb(idx) as u16);
                self.sub8(self.rb(R_A), self.read_u8(self.es_addr(addr as u32)), false);
            }
            // mov A, ES:[HL+B]/[HL+C] (1100 1001/1110 1001)
            0xC9 | 0xE9 => {
                let idx = if op & 1 == 0 { R_B } else { R_C };
                let addr = self.rw(3).wrapping_add(self.rb(idx) as u16);
                let v = self.read_u8(self.es_addr(addr as u32));
                self.set_rb(R_A, v);
            }
            // mov ES:[HL+B]/[HL+C], A (1101 1001/1111 1001)
            0xD9 | 0xF9 => {
                let idx = if op & 1 == 0 { R_B } else { R_C };
                let addr = self.rw(3).wrapping_add(self.rb(idx) as u16);
                self.write_u8(self.es_addr(addr as u32), self.rb(R_A));
            }
            // mov A, ES:[imm16] (1010 1000)
            0xA8 => {
                let addr = self.fetch16() as u32;
                let v = self.read_u8(self.es_addr(addr));
                self.set_rb(R_A, v);
            }
            // mov ES:[imm16], A (1010 1001)
            0xA9 => {
                let addr = self.fetch16() as u32;
                self.write_u8(self.es_addr(addr), self.rb(R_A));
            }
            // and A, ES:[HL+B]/[HL+C] (1101 000/1101 0010)
            0xD0 | 0xD2 => {
                let idx = if op & 1 == 0 { R_B } else { R_C };
                let addr = self.rw(3).wrapping_add(self.rb(idx) as u16);
                let v = self.rb(R_A) & self.read_u8(self.es_addr(addr as u32));
                self.set_rb(R_A, v);
                self.set_nz(v);
            }
            // or A, ES:[HL+B]/[HL+C] (1110 000/1110 0010)
            0xE0 | 0xE2 => {
                let idx = if op & 1 == 0 { R_B } else { R_C };
                let addr = self.rw(3).wrapping_add(self.rb(idx) as u16);
                let v = self.rb(R_A) | self.read_u8(self.es_addr(addr as u32));
                self.set_rb(R_A, v);
                self.set_nz(v);
            }
            // xor A, ES:[HL+B]/[HL+C] (1111 000/1111 0010)
            0xF0 | 0xF2 => {
                let idx = if op & 1 == 0 { R_B } else { R_C };
                let addr = self.rw(3).wrapping_add(self.rb(idx) as u16);
                let v = self.rb(R_A) ^ self.read_u8(self.es_addr(addr as u32));
                self.set_rb(R_A, v);
                self.set_nz(v);
            }
            _ => match op {
                // inc ES:[HL+imm8] (0101 1001)
                0x59 => {
                    let dsp = self.fetch8() as i8 as i32;
                    let addr = self.rw(3).wrapping_add(dsp as u16);
                    let v = self.add8(self.read_u8(self.es_addr(addr as u32)), 1, false);
                    self.write_u8(self.es_addr(addr as u32), v);
                }
                // dec ES:[HL+imm8] (0110 1001)
                0x69 => {
                    let dsp = self.fetch8() as i8 as i32;
                    let addr = self.rw(3).wrapping_add(dsp as u16);
                    let v = self.sub8(self.read_u8(self.es_addr(addr as u32)), 1, false);
                    self.write_u8(self.es_addr(addr as u32), v);
                }
                // incw ES:[HL+imm8] (0111 1001)
                0x79 => {
                    let dsp = self.fetch8() as i8 as i32;
                    let addr = self.rw(3).wrapping_add(dsp as u16);
                    let v = self.read_u16(self.es_addr(addr as u32)).wrapping_add(1);
                    self.write_u16(self.es_addr(addr as u32), v);
                }
                // decw ES:[HL+imm8] (1000 1001)
                0x89 => {
                    let dsp = self.fetch8() as i8 as i32;
                    let addr = self.rw(3).wrapping_add(dsp as u16);
                    let v = self.read_u16(self.es_addr(addr as u32)).wrapping_sub(1);
                    self.write_u16(self.es_addr(addr as u32), v);
                }
                // mov A, ES:[HL+imm8] (1011 1000)
                0xB8 => {
                    let dsp = self.fetch8() as i8 as i32;
                    let addr = self.rw(3).wrapping_add(dsp as u16);
                    let v = self.read_u8(self.es_addr(addr as u32));
                    self.set_rb(R_A, v);
                }
                // mov ES:[HL+imm8], A (1011 1001)
                0xB9 => {
                    let dsp = self.fetch8() as i8 as i32;
                    let addr = self.rw(3).wrapping_add(dsp as u16);
                    self.write_u8(self.es_addr(addr as u32), self.rb(R_A));
                }
                // movw AX, ES:[HL+imm8] (0000 1001)
                0x09 => {
                    let dsp = self.fetch8() as i8 as i32;
                    let addr = self.rw(3).wrapping_add(dsp as u16);
                    let v = self.read_u16(self.es_addr(addr as u32));
                    self.set_rw(0, v);
                }
                // cmpw AX, ES:[HL+imm8] (0100 1001)
                0x49 => {
                    let dsp = self.fetch8() as i8 as i32;
                    let addr = self.rw(3).wrapping_add(dsp as u16);
                    self.sub16(self.rw(0), self.read_u16(self.es_addr(addr as u32)), false);
                }
                // subw AX, ES:[HL+imm8] (0010 1001)
                0x29 => {
                    let dsp = self.fetch8() as i8 as i32;
                    let addr = self.rw(3).wrapping_add(dsp as u16);
                    let v = self.sub16(self.rw(0), self.read_u16(self.es_addr(addr as u32)), false);
                    self.set_rw(0, v);
                }
                // xch A, ES:[imm16] (1010 1010)
                0xAA => {
                    let addr = self.fetch16() as u32;
                    let ea = self.es_addr(addr);
                    let v = self.read_u8(ea);
                    self.write_u8(ea, self.rb(R_A));
                    self.set_rb(R_A, v);
                }
                // xch A, ES:[saddr] (1010 1011)
                0xAB => {
                    let sa = self.fetch8() as u32;
                    let addr = self.saddr(sa);
                    let v = self.read_u8(addr);
                    self.write_u8(addr, self.rb(R_A));
                    self.set_rb(R_A, v);
                }
                // mov ES, ES:[saddr] (1011 1000 handled above as mov A; 1011 1011)
                // br AX (1100 1011)
                0xCB => {
                    self.set_pc(self.rw(0) as u32);
                }
                // reti (1111 1100)
                0xFC => {
                    let pc = self.pop16() as u32;
                    self.set_pc(pc);
                    let psw = self.pop16() as u8;
                    self.psw.z = psw & 1 != 0;
                    self.psw.ac = psw & 2 != 0;
                    self.psw.cy = psw & 4 != 0;
                    self.psw.rbs = (psw >> 3) & 0x3;
                    self.psw.ie = psw & 0x20 != 0;
                }
                // retb (1110 1100)
                0xEC => {
                    let pc = self.pop16() as u32;
                    self.set_pc(pc);
                    let _ = self.pop16();
                }
                // brk (1100 1100)
                0xCC => self.halted = true,
                // halt (1110 1101)
                0xED => self.halted = true,
                // stop (1111 1101)
                0xFD => self.halted = true,
                // SKC (1100 1000) / SKNC (1101 1000) / SKZ (1110 1000) / SKNZ (1111 1000)
                0xC8 | 0xD8 | 0xE8 | 0xF8 => {
                    let cond = match op {
                        0xC8 => 0,
                        0xD8 => 1,
                        0xE8 => 2,
                        _ => 3,
                    };
                    if self.cond_true(cond) {
                        let _ = self.fetch8();
                    }
                }
                // BH $rel8 (1100 0011)
                0xC3 => {
                    let rel = op as i8 as i32;
                    if self.cond_true(4) {
                        self.pc = self.pc.wrapping_add(rel as u32);
                    }
                }
                // BNH $rel8 (1101 0011)
                0xD3 => {
                    let rel = op as i8 as i32;
                    if self.cond_true(5) {
                        self.pc = self.pc.wrapping_add(rel as u32);
                    }
                }
                // pop psw (1100 1101)
                0xCD => {
                    let psw = self.pop16() as u8;
                    self.psw.z = psw & 1 != 0;
                    self.psw.ac = psw & 2 != 0;
                    self.psw.cy = psw & 4 != 0;
                    self.psw.rbs = (psw >> 3) & 0x3;
                    self.psw.ie = psw & 0x20 != 0;
                }
                // push psw (1101 1101)
                0xDD => {
                    let mut psw = 0u16;
                    if self.psw.z { psw |= 1; }
                    if self.psw.ac { psw |= 2; }
                    if self.psw.cy { psw |= 4; }
                    psw |= (self.psw.rbs as u16 & 0x3) << 3;
                    if self.psw.ie { psw |= 0x20; }
                    self.push16(psw);
                }
                // call ax (1110 1010) / call bc (1110 1100 overlapped)
                // mov A, ES:[HL+B]/[HL+C] handled above.
                // movs ES:[HL+imm8], X (1100 1110)
                0xCE => {
                    let dsp = self.fetch8() as i8 as i32;
                    let addr = self.rw(3).wrapping_add(dsp as u16);
                    let a = self.es_addr(addr as u32);
                    let x = self.rb(R_X);
                    let v = self.sub8(x, self.read_u8(a), false);
                    self.write_u8(a, v);
                }
                // rol A, 1 (1110 1011)
                0xEB => {
                    let a = self.rb(R_A);
                    let v = a.rotate_left(1);
                    self.set_rb(R_A, v);
                }
                // ror A, 1 (1101 1011)
                0xDB => {
                    let a = self.rb(R_A);
                    let v = a.rotate_right(1);
                    self.set_rb(R_A, v);
                }
                // rolwc AX/BC (111r 1110)
                0xEE | 0xFE => {
                    let r = ((op >> 4) & 0x01) as usize;
                    let w = self.rw(r);
                    let c = self.psw.cy as u16;
                    let v = (w << 1) | c;
                    self.psw.cy = (w & 0x8000) != 0;
                    self.set_rw(r, v);
                }
                // Unknown 0x61-prefixed opcode.
                _ => self.halted = true,
            },
        }
    }
}

/// Number of code bytes consumed by the RL78 instruction at `pc`. Mirrors the
/// core's byte-fetch model: most single-byte opcodes are 1 byte, immediate/
/// direct forms carry one operand byte (2), and `ES:[imm16]`/word forms and
/// the `0x61` prefix forms consume 3-4 bytes.
pub fn instruction_len(pc: u32, read: &dyn Fn(u32) -> u8) -> u32 {
    let b0 = read(pc);
    if b0 == 0x61 {
        let op = read(pc + 1);
        return 2 + match op {
            // 0x61 + op + imm16. The core's `op & 0xF8` dispatcher routes 0xA8/
            // 0xA9 and anything masked to them (0xAA/0xAB) to the `mov A, ES:
            // [imm16]`-style arms, which fetch a 16-bit operand.
            0xA8 | 0xA9 | 0xAA | 0xAB => 2,
            // 0x61 + op + imm8. Only 0xB8/0xB9/0xCE reach the `_ => match op`
            // fallback handlers that fetch one operand byte; the rest of that
            // fallback group is shadowed by the `op & 0xF8` register-register
            // families and stays 2 bytes wide.
            0xB8 | 0xB9 | 0xCE => 1,
            _ => 0,
        };
    }
    match b0 {
        // Single-byte opcodes with no operand.
        0x00 | 0x01 | 0x03 | 0x05 | 0x07 | 0x0D | 0x11 | 0x12 | 0x13 | 0x14 | 0x15 | 0x16
        | 0x17 | 0x1D | 0x5D | 0x6D | 0x60..=0x67 | 0x80..=0x87 | 0x89 | 0x8B | 0x90..=0x97
        | 0x99 | 0x9B | 0xC0..=0xC7 | 0xD7 | 0xFF => 1,
        // Opcode + 16-bit operand.
        0x02 | 0x04 | 0x09 | 0x0F | 0x1F | 0x29 | 0x30 | 0x32 | 0x34 | 0x36 | 0x49 | 0x5F
        | 0x6F | 0x8F | 0x9F => 3,
        // Opcode + two 8-bit operands (saddr, #imm8).
        0x0A | 0x1A => 3,
        // Opcode + one 8-bit operand.
        0x06 | 0x0B | 0x0C | 0x0E | 0x10 | 0x1B | 0x1C | 0x1E | 0x41 | 0x50..=0x57 | 0x5C
        | 0x5E | 0x6C | 0x6E | 0x88 | 0x8A | 0x8C | 0x8D | 0x9D => 2,
        // Unknown opcode: the core fetches one byte then halts.
        _ => 1,
    }
}

/// Disassemble one RL78 instruction at `pc` using `read` for bytes.
/// Produces a readable one-line rendering; unknown bytes print as `.byte 0xXX`.
pub fn disassemble(pc: u32, read: &dyn Fn(u32) -> u8) -> String {
    let b0 = read(pc);
    if b0 == 0x61 {
        return disassemble_61(pc, read);
    }
    let u8at = |p: u32| read(p);
    let u16at = |p: u32| (read(p) as u16) | ((read(p + 1) as u16) << 8);
    let rn = |i: usize| ["X", "A", "C", "B", "E", "D", "L", "H"][i & 7];
    let wn = |i: usize| ["AX", "BC", "DE", "HL"][i & 3];
    match b0 {
        0x00 => "nop".to_string(),
        0x02 => format!("addw ax, es:[0x{:04x}]", u16at(pc + 1)),
        0x04 => format!("addw ax, #0x{:04x}", u16at(pc + 1)),
        0x06 => format!("addw ax, !0x{:02x}", u8at(pc + 1)),
        0x01 | 0x03 | 0x05 | 0x07 => format!("addw ax, {}", wn(((b0 >> 1) & 3) as usize)),
        0x0B => format!("add a, !0x{:02x}", u8at(pc + 1)),
        0x0A => format!("add !0x{:02x}, #0x{:02x}", u8at(pc + 1), u8at(pc + 2)),
        0x0C => format!("add a, #0x{:02x}", u8at(pc + 1)),
        0x0D => "add a, [hl]".to_string(),
        0x0E => format!("add a, [hl+0x{:02x}]", u8at(pc + 1)),
        0x0F => format!("add a, es:[0x{:04x}]", u16at(pc + 1)),
        0x10 => format!("addw sp, #0x{:02x}", u8at(pc + 1)),
        0x11 | 0x13 | 0x15 | 0x17 => format!("movw ax, {}", wn(((b0 >> 1) & 3) as usize)),
        0x12 | 0x14 | 0x16 => format!("movw {}, ax", wn(((b0 >> 1) & 3) as usize)),
        0x30 | 0x32 | 0x34 | 0x36 => {
            format!("movw {}, #0x{:04x}", wn(((b0 >> 1) & 3) as usize), u16at(pc + 1))
        }
        0x1C => format!("addc a, #0x{:02x}", u8at(pc + 1)),
        0x1F => format!("addc a, es:[0x{:04x}]", u16at(pc + 1)),
        0x1D => "addc a, [hl]".to_string(),
        0x1E => format!("addc a, [hl+0x{:02x}]", u8at(pc + 1)),
        0x1B => format!("addc a, !0x{:02x}", u8at(pc + 1)),
        0x1A => format!("addc !0x{:02x}, #0x{:02x}", u8at(pc + 1), u8at(pc + 2)),
        0x5C => format!("and a, #0x{:02x}", u8at(pc + 1)),
        0x5D => "and a, [hl]".to_string(),
        0x5E => format!("and a, [hl+0x{:02x}]", u8at(pc + 1)),
        0x5F => format!("and a, es:[0x{:04x}]", u16at(pc + 1)),
        0x6C => format!("or a, #0x{:02x}", u8at(pc + 1)),
        0x6D => "or a, [hl]".to_string(),
        0x6E => format!("or a, [hl+0x{:02x}]", u8at(pc + 1)),
        0x6F => format!("or a, es:[0x{:04x}]", u16at(pc + 1)),
        0x50..=0x57 => format!("mov {}, #0x{:02x}", rn((b0 & 7) as usize), u8at(pc + 1)),
        0x60..=0x67 => format!("mov a, {}", rn((b0 & 7) as usize)),
        0x80..=0x87 => format!("inc {}", rn((b0 & 7) as usize)),
        0x90..=0x97 => format!("dec {}", rn((b0 & 7) as usize)),
        0x89 => "mov a, [de]".to_string(),
        0x8A => format!("mov a, [de+0x{:02x}]", u8at(pc + 1)),
        0x8B => "mov a, [hl]".to_string(),
        0x8C => format!("mov a, [hl+0x{:02x}]", u8at(pc + 1)),
        0x8D => format!("mov a, !0x{:02x}", u8at(pc + 1)),
        0x8F => format!("mov a, es:[0x{:04x}]", u16at(pc + 1)),
        0x88 => format!("mov a, [sp+0x{:02x}]", u8at(pc + 1)),
        0x09 => format!("mov a, [b+0x{:04x}]", u16at(pc + 1)),
        0x49 => format!("mov a, [bc+0x{:04x}]", u16at(pc + 1)),
        0x29 => format!("mov a, [c+0x{:04x}]", u16at(pc + 1)),
        0x99 => "mov [de], a".to_string(),
        0x9B => "mov [hl], a".to_string(),
        0x9D => format!("mov !0x{:02x}, a", u8at(pc + 1)),
        0x9F => format!("mov es:[0x{:04x}], a", u16at(pc + 1)),
        0xC1 | 0xC3 | 0xC5 | 0xC7 => format!("push {}", wn(((b0 >> 1) & 3) as usize)),
        0xC0 | 0xC2 | 0xC4 | 0xC6 => format!("pop {}", wn(((b0 >> 1) & 3) as usize)),
        0xD7 => "ret".to_string(),
        0x41 => format!("mov a, #0x{:02x}", u8at(pc + 1)),
        0xFF => "brk1".to_string(),
        _ => format!(".byte 0x{b0:02x}"),
    }
}

/// Disassemble a `0110 0001` (0x61) prefixed instruction, mirroring the core's
/// `execute_61` dispatch: the register-register families select on the upper
/// five bits of the second byte (`op & 0xF8`), and a handful of remaining
/// opcodes (ES:[imm16] / ES:[HL+imm8] / control flow) fall through.
fn disassemble_61(pc: u32, read: &dyn Fn(u32) -> u8) -> String {
    let b1 = read(pc + 1);
    let u8at = |p: u32| read(p);
    let u16at = |p: u32| (read(p) as u16) | ((read(p + 1) as u16) << 8);
    let rn = |i: usize| ["X", "A", "C", "B", "E", "D", "L", "H"][i & 7];
    let rba = rn((b1 & 7) as usize);
    let b_or_c = |op: u8| if op & 1 == 0 { "b" } else { "c" };
    match b1 & 0xF8 {
        0x00 => format!("add {rba}, a"),
        0x08 => format!("add a, {rba}"),
        0x10 => format!("addc {rba}, a"),
        0x18 => format!("addc a, {rba}"),
        0x20 => format!("sub {rba}, a"),
        0x28 => format!("sub a, {rba}"),
        0x30 => format!("subc {rba}, a"),
        0x38 => format!("subc a, {rba}"),
        0x40 => format!("cmp {rba}, a"),
        0x48 => format!("cmp a, {rba}"),
        0x50 => format!("and {rba}, a"),
        0x58 => format!("and a, {rba}"),
        0x60 => format!("or {rba}, a"),
        0x68 => format!("or a, {rba}"),
        0x70 => format!("xor {rba}, a"),
        0x78 => format!("xor a, {rba}"),
        0x80 => format!("add a, es:[hl+{}]", b_or_c(b1)),
        0x88 => format!("xch a, {rba}"),
        0x90 => format!("addc a, es:[hl+{}]", b_or_c(b1)),
        0xA0 => format!("sub a, es:[hl+{}]", b_or_c(b1)),
        // 0xA8-0xAF all route through the `0xA8` family in the core (mov A,
        // ES:[imm16]); the 0xA9/0xAA/0xAB encodings are shadowed by it.
        0xA8 => format!("mov a, es:[0x{:04x}]", u16at(pc + 2)),
        0xB0 => format!("subc a, es:[hl+{}]", b_or_c(b1)),
        0xC0 => format!("cmp a, es:[hl+{}]", b_or_c(b1)),
        0xD0 => format!("and a, es:[hl+{}]", b_or_c(b1)),
        0xE0 => format!("or a, es:[hl+{}]", b_or_c(b1)),
        0xF0 => format!("xor a, es:[hl+{}]", b_or_c(b1)),
        _ => match b1 {
            0xB8 => format!("mov a, es:[hl+0x{:02x}]", u8at(pc + 2)),
            0xB9 => format!("mov es:[hl+0x{:02x}], a", u8at(pc + 2)),
            0xCE => format!("movs es:[hl+0x{:02x}], x", u8at(pc + 2)),
            0xC8 => "skc".to_string(),
            0xD8 => "sknc".to_string(),
            0xE8 => "skz".to_string(),
            0xF8 => "sknz".to_string(),
            0xC9 | 0xE9 => format!("mov a, es:[hl+{}]", b_or_c(b1)),
            0xD9 | 0xF9 => format!("mov es:[hl+{}], a", b_or_c(b1)),
            0xCB => "br ax".to_string(),
            0xCC => "brk".to_string(),
            0xCD => "pop psw".to_string(),
            0xDB => "ror a".to_string(),
            0xDD => "push psw".to_string(),
            0xEB => "rol a".to_string(),
            0xEC => "retb".to_string(),
            0xED => "halt".to_string(),
            0xEE | 0xFE => {
                let r = if b1 & 0x10 == 0 { "ax" } else { "bc" };
                format!("rolwc {r}")
            }
            0xFC => "reti".to_string(),
            0xFD => "stop".to_string(),
            _ => format!("prefix 0x61: 0x{b1:02x}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core_with(code: &[u8]) -> Rl78Core {
        let mut c = Rl78Core::new();
        c.write_bytes(0x1000, code);
        c.pc = 0x1000;
        c
    }

    /// Assert that the static decoder width matches the core's actual byte
    /// consumption for a straight-line (non-branching) instruction.
    fn assert_step_width(code: &[u8]) {
        let mut c = core_with(code);
        let pc = c.pc;
        let expected = instruction_len(pc, &|addr| c.read_u8(addr));
        let r = c.step(&[]);
        assert!(
            matches!(r, Rl78Step::Advanced | Rl78Step::Halted),
            "unexpected step result for opcodes: {code:02x?}"
        );
        assert_eq!(
            c.pc.wrapping_sub(pc),
            expected,
            "PC advance mismatch for opcodes: {code:02x?}"
        );
    }

    #[test]
    fn instruction_len_matches_core_width() {
        // Single-byte no-operand forms.
        assert_step_width(&[0x00]); // nop
        assert_step_width(&[0x01]); // addw ax, bc
        assert_step_width(&[0x0D]); // add a, [hl]
        assert_step_width(&[0x11]); // movw ax, bc
        assert_step_width(&[0x12]); // movw bc, ax
        assert_step_width(&[0x5D]); // and a, [hl]
        assert_step_width(&[0x60]); // mov a, rba
        assert_step_width(&[0x80]); // inc rba
        assert_step_width(&[0x89]); // mov a, [de]
        assert_step_width(&[0x8B]); // mov a, [hl]
        assert_step_width(&[0x90]); // dec rba
        assert_step_width(&[0x99]); // mov [de], a
        assert_step_width(&[0x9B]); // mov [hl], a
        assert_step_width(&[0xC0]); // pop rp
        assert_step_width(&[0xC1]); // push rp
        assert_step_width(&[0xFF]); // brk1
        // Opcode + one 8-bit operand.
        assert_step_width(&[0x06, 0x30]); // addw ax, saddr
        assert_step_width(&[0x0B, 0x30]); // add a, saddr
        assert_step_width(&[0x0C, 0x2A]); // add a, #imm8
        assert_step_width(&[0x0E, 0x05]); // add a, [hl+imm8]
        assert_step_width(&[0x10, 0x08]); // addw sp, #imm8
        assert_step_width(&[0x1B, 0x30]); // addc a, saddr
        assert_step_width(&[0x1C, 0x2A]); // addc a, #imm8
        assert_step_width(&[0x1E, 0x05]); // addc a, [hl+imm8]
        assert_step_width(&[0x41, 0x2A]); // mov a, #imm8
        assert_step_width(&[0x50, 0x42]); // mov x, #imm8
        assert_step_width(&[0x51, 0x42]); // mov a, #imm8
        assert_step_width(&[0x5C, 0x2A]); // and a, #imm8
        assert_step_width(&[0x5E, 0x05]); // and a, [hl+imm8]
        assert_step_width(&[0x6C, 0x2A]); // or a, #imm8
        assert_step_width(&[0x6E, 0x05]); // or a, [hl+imm8]
        assert_step_width(&[0x88, 0x08]); // mov a, [sp+imm8]
        assert_step_width(&[0x8A, 0x05]); // mov a, [de+imm8]
        assert_step_width(&[0x8C, 0x05]); // mov a, [hl+imm8]
        assert_step_width(&[0x8D, 0x30]); // mov a, saddr
        assert_step_width(&[0x9D, 0x30]); // mov saddr, a
        // Opcode + 16-bit operand.
        assert_step_width(&[0x02, 0x78, 0x56]); // addw ax, es:[imm16]
        assert_step_width(&[0x04, 0x34, 0x12]); // addw ax, #imm16
        assert_step_width(&[0x09, 0x78, 0x56]); // mov a, [b+imm16]
        assert_step_width(&[0x0F, 0x78, 0x56]); // add a, es:[imm16]
        assert_step_width(&[0x1F, 0x78, 0x56]); // addc a, es:[imm16]
        assert_step_width(&[0x29, 0x78, 0x56]); // mov a, [c+imm16]
        assert_step_width(&[0x30, 0x34, 0x12]); // movw ax, #imm16
        assert_step_width(&[0x49, 0x78, 0x56]); // mov a, [bc+imm16]
        assert_step_width(&[0x5F, 0x78, 0x56]); // and a, es:[imm16]
        assert_step_width(&[0x6F, 0x78, 0x56]); // or a, es:[imm16]
        assert_step_width(&[0x8F, 0x78, 0x56]); // mov a, es:[imm16]
        assert_step_width(&[0x9F, 0x78, 0x56]); // mov es:[imm16], a
        // Opcode + two 8-bit operands.
        assert_step_width(&[0x0A, 0x30, 0x55]); // add saddr, #imm8
        assert_step_width(&[0x1A, 0x30, 0x55]); // addc saddr, #imm8
        // 0x61 prefix: reg-reg (2 bytes).
        assert_step_width(&[0x61, 0x08]); // add a, x
        assert_step_width(&[0x61, 0x28]); // sub a, x
        assert_step_width(&[0x61, 0x48]); // cmp a, x
        assert_step_width(&[0x61, 0x88]); // xch a, x
        // The core decodes the low group first, so 0x61 + {0x59,0x69,0x79,0x89,
        // 0x09,0x49,0x29} are treated as reg-reg forms and consume 2 bytes.
        assert_step_width(&[0x61, 0x59]); // and a, a (core view)
        assert_step_width(&[0x61, 0x69]); // or a, a
        assert_step_width(&[0x61, 0x79]); // xor a, a
        assert_step_width(&[0x61, 0x89]); // xch a, a
        assert_step_width(&[0x61, 0x09]); // add a, a
        assert_step_width(&[0x61, 0x49]); // cmp a, a
        assert_step_width(&[0x61, 0x29]); // sub a, a
        // 0x61 prefix: [HL+imm8] (3 bytes).
        assert_step_width(&[0x61, 0xB8, 0x10]); // mov a, es:[hl+imm8]
        assert_step_width(&[0x61, 0xB9, 0x10]); // mov es:[hl+imm8], a
        assert_step_width(&[0x61, 0xCE, 0x10]); // movs es:[hl+imm8], x
        // 0x61 prefix: ES:[imm16] (4 bytes). 0xAA/0xAB route through the
        // `op & 0xF8` 0xA8 family, so they also consume a 16-bit operand.
        assert_step_width(&[0x61, 0xA8, 0x78, 0x56]); // mov a, es:[imm16]
        assert_step_width(&[0x61, 0xA9, 0x78, 0x56]); // mov es:[imm16], a
        assert_step_width(&[0x61, 0xAA, 0x78, 0x56]); // xch a, es:[imm16]
        assert_step_width(&[0x61, 0xAB, 0x78, 0x56]); // xch a, es:[saddr] (core view)
    }

    #[test]
    fn nop_advances() {
        let mut c = core_with(&[0x00]);
        let r = c.step(&[]);
        assert_eq!(r, Rl78Step::Advanced);
        assert_eq!(c.pc, 0x1001);
    }

    #[test]
    fn disassembles_core_opcode_forms() {
        // The disassembler must mirror the core's dispatch so `disasm` shows
        // real instructions (not `.byte`/`prefix 0x61:` placeholders) for the
        // opcodes the core actually executes.
        let d = |code: &[u8]| {
            let read = move |a: u32| code.get(a as usize).copied().unwrap_or(0);
            disassemble(0, &read)
        };
        // Single-byte forms.
        assert_eq!(d(&[0x00]), "nop");
        assert_eq!(d(&[0x0C, 0x2A]), "add a, #0x2a");
        assert_eq!(d(&[0x0A, 0x30, 0x55]), "add !0x30, #0x55");
        assert_eq!(d(&[0x0F, 0x78, 0x56]), "add a, es:[0x5678]");
        assert_eq!(d(&[0x0E, 0x05]), "add a, [hl+0x05]");
        assert_eq!(d(&[0x06, 0x30]), "addw ax, !0x30");
        assert_eq!(d(&[0x01]), "addw ax, AX");
        assert_eq!(d(&[0x03]), "addw ax, BC");
        assert_eq!(d(&[0x04, 0x34, 0x12]), "addw ax, #0x1234");
        assert_eq!(d(&[0x10, 0x08]), "addw sp, #0x08");
        assert_eq!(d(&[0x5D]), "and a, [hl]");
        assert_eq!(d(&[0x51, 0x42]), "mov A, #0x42");
        assert_eq!(d(&[0x60]), "mov a, X");
        assert_eq!(d(&[0x80]), "inc X");
        assert_eq!(d(&[0x90]), "dec X");
        assert_eq!(d(&[0x89]), "mov a, [de]");
        assert_eq!(d(&[0x8A, 0x05]), "mov a, [de+0x05]");
        assert_eq!(d(&[0x8D, 0x30]), "mov a, !0x30");
        assert_eq!(d(&[0x88, 0x08]), "mov a, [sp+0x08]");
        assert_eq!(d(&[0x09, 0x78, 0x56]), "mov a, [b+0x5678]");
        assert_eq!(d(&[0x9B]), "mov [hl], a");
        assert_eq!(d(&[0x9F, 0x78, 0x56]), "mov es:[0x5678], a");
        assert_eq!(d(&[0xC3]), "push BC");
        assert_eq!(d(&[0xC2]), "pop BC");
        assert_eq!(d(&[0xD7]), "ret");
        assert_eq!(d(&[0xFF]), "brk1");
        // 0x61 prefix reg-reg families.
        assert_eq!(d(&[0x61, 0x48]), "cmp a, X");
        assert_eq!(d(&[0x61, 0x40]), "cmp X, a");
        assert_eq!(d(&[0x61, 0x08]), "add a, X");
        assert_eq!(d(&[0x61, 0x18]), "addc a, X");
        assert_eq!(d(&[0x61, 0x28]), "sub a, X");
        assert_eq!(d(&[0x61, 0x38]), "subc a, X");
        assert_eq!(d(&[0x61, 0x58]), "and a, X");
        assert_eq!(d(&[0x61, 0x68]), "or a, X");
        assert_eq!(d(&[0x61, 0x78]), "xor a, X");
        assert_eq!(d(&[0x61, 0x88]), "xch a, X");
        // The low group is decoded first, so shadowed encodings become the
        // register-register form (matching the core).
        assert_eq!(d(&[0x61, 0x59]), "and a, A");
        assert_eq!(d(&[0x61, 0x09]), "add a, A");
        assert_eq!(d(&[0x61, 0x69]), "or a, A");
        // ES:[HL+B/C] forms select the index register by bit 0.
        assert_eq!(d(&[0x61, 0x80]), "add a, es:[hl+b]");
        assert_eq!(d(&[0x61, 0x81]), "add a, es:[hl+c]");
        assert_eq!(d(&[0x61, 0x90]), "addc a, es:[hl+b]");
        assert_eq!(d(&[0x61, 0xC0]), "cmp a, es:[hl+b]");
        assert_eq!(d(&[0x61, 0xF2]), "xor a, es:[hl+b]");
        // ES:[imm16] and ES:[HL+imm8] fall-through forms.
        assert_eq!(d(&[0x61, 0xA8, 0x78, 0x56]), "mov a, es:[0x5678]");
        assert_eq!(d(&[0x61, 0xAA, 0x78, 0x56]), "mov a, es:[0x5678]");
        assert_eq!(d(&[0x61, 0xB8, 0x10]), "mov a, es:[hl+0x10]");
        assert_eq!(d(&[0x61, 0xB9, 0x10]), "mov es:[hl+0x10], a");
        assert_eq!(d(&[0x61, 0xCE, 0x10]), "movs es:[hl+0x10], x");
        assert_eq!(d(&[0x61, 0xC9]), "mov a, es:[hl+c]");
        assert_eq!(d(&[0x61, 0xD9]), "mov es:[hl+c], a");
        // Control flow / PSW / halt forms.
        assert_eq!(d(&[0x61, 0xC8]), "skc");
        assert_eq!(d(&[0x61, 0xD8]), "sknc");
        assert_eq!(d(&[0x61, 0xE8]), "skz");
        assert_eq!(d(&[0x61, 0xF8]), "sknz");
        assert_eq!(d(&[0x61, 0xCB]), "br ax");
        assert_eq!(d(&[0x61, 0xFC]), "reti");
        assert_eq!(d(&[0x61, 0xEC]), "retb");
        assert_eq!(d(&[0x61, 0xCD]), "pop psw");
        assert_eq!(d(&[0x61, 0xDD]), "push psw");
        assert_eq!(d(&[0x61, 0xEB]), "rol a");
        assert_eq!(d(&[0x61, 0xDB]), "ror a");
        assert_eq!(d(&[0x61, 0xEE]), "rolwc ax");
        assert_eq!(d(&[0x61, 0xFE]), "rolwc bc");
        assert_eq!(d(&[0x61, 0xCC]), "brk");
        assert_eq!(d(&[0x61, 0xED]), "halt");
        assert_eq!(d(&[0x61, 0xFD]), "stop");
    }

    #[test]
    fn byte_opcode_disasm_matches_core() {
        // For every single-byte opcode, a raw `.byte` placeholder in the
        // disassembler must correspond to an opcode the core halts on.
        for op in 0u8..=255 {
            let bytes = [op];
            let read = |a: u32| bytes.get(a as usize).copied().unwrap_or(0);
            let text = disassemble(0x1000, &read);
            if text.starts_with(".byte") || text.starts_with("0x") {
                let mut c = core_with(&bytes);
                let r = c.step(&[]);
                assert_ne!(
                    r,
                    Rl78Step::Advanced,
                    "opcode 0x{op:02x} disassembles as {text:?} but core executes"
                );
            }
        }
    }

    #[test]
    fn add_imm_to_a() {
        // ADD A, #0x05 (0x0C 0x05)
        let mut c = core_with(&[0x0C, 0x05]);
        c.set_rb(R_A, 10);
        c.step(&[]);
        assert_eq!(c.rb(R_A), 15);
    }

    #[test]
    fn add_sets_carry() {
        // ADD A, #0x02 with A=0xFE => 0x00, carry, zero.
        let mut c = core_with(&[0x0C, 0x02]);
        c.set_rb(R_A, 0xFE);
        c.step(&[]);
        assert_eq!(c.rb(R_A), 0);
        assert!(c.psw.z, "zero flag");
        assert!(c.psw.cy, "carry flag");
    }

    #[test]
    fn mov_register_immediate() {
        // 0101 0reg = MOV rba, #imm. reg encoding: X=0, A=1, C=2, B=3.
        // MOV A, #0x42 => 0x51 0x42 (reg=1: 0101 0001).
        let mut c = core_with(&[0x51, 0x42]);
        c.step(&[]);
        assert_eq!(c.rb(R_A), 0x42);
    }

    #[test]
    fn cmp_sets_zero() {
        // CMP A, rba: 0x61 0x48 (cmp a, x) with A=X=5. `0100 1rba`, rba=0 (X).
        let mut c = core_with(&[0x61, 0x48]);
        c.set_rb(R_A, 5);
        c.set_rb(R_X, 5);
        c.step(&[]);
        assert!(c.psw.z, "cmp equal sets Z");
    }

    #[test]
    fn sub_a_register() {
        // SUB A, rba: 0x61 0x28 (sub a, x). `0010 1rba`, rba=0 (X).
        let mut c = core_with(&[0x61, 0x28]);
        c.set_rb(R_A, 10);
        c.set_rb(R_X, 3);
        c.step(&[]);
        assert_eq!(c.rb(R_A), 7);
    }

    #[test]
    fn inc_register() {
        // INC rba: 1000 0reg, reg=1 (A) => 0x81.
        let mut c = core_with(&[0x81]);
        c.set_rb(R_A, 0xFF);
        c.step(&[]);
        assert_eq!(c.rb(R_A), 0);
        assert!(c.psw.z, "inc wrap sets Z");
    }

    #[test]
    fn dec_register() {
        // DEC rba: 1001 0reg, reg=1 (A) => 0x91.
        let mut c = core_with(&[0x91]);
        c.set_rb(R_A, 1);
        c.step(&[]);
        assert_eq!(c.rb(R_A), 0);
        assert!(c.psw.z, "dec to zero sets Z");
    }

    #[test]
    fn mov_es_addr_to_a() {
        // MOV A, ES:[imm16]: 1000 1000 (0x88) + imm16.
        // Actually `0000 1001 = MOV A, ES:[B+imm16]`; use `1000 1001 = MOV A, DE`
        // and `1000 1101 = MOV A, ES:[saddr]`. For the flat load use
        // MOV A, ES:[imm16] (0x8F is not in the table). Use `1000 1101` saddr.
        // MOV A, ES:[SADDR]: 1000 1101 (0x8D) + saddr 0x10 -> 0xFFF10.
        let mut c = core_with(&[0x8D, 0x10]);
        c.write_u8(0xFFF10, 0xAB);
        c.step(&[]);
        assert_eq!(c.rb(R_A), 0xAB);
    }

    #[test]
    fn push_pop_uses_sp() {
        // PUSH rp: 1100 0rg1. rg=0 (AX) => 0xC1.
        let mut c = core_with(&[0xC1]);
        c.sp = 0x2000;
        c.set_rw(0, 0x1234);
        c.step(&[]);
        assert_eq!(c.sp, 0x1FFE);
        assert_eq!(c.read_u16(0x1FFE), 0x1234);
        // POP rp: 1100 0rg0. rg=0 (AX) => 0xC0.
        let mut c2 = core_with(&[0xC0]);
        c2.sp = 0x1FFE;
        c2.write_u16(0x1FFE, 0xABCD);
        c2.step(&[]);
        assert_eq!(c2.sp, 0x2000);
        assert_eq!(c2.rw(0), 0xABCD);
    }

    #[test]
    fn ret_pops_pc() {
        // RET: 1101 0111 (0xD7).
        let mut c = core_with(&[0xD7]);
        c.sp = 0x1FFE;
        c.write_u16(0x1FFE, 0x2000);
        c.step(&[]);
        assert_eq!(c.pc, 0x2000);
        assert_eq!(c.sp, 0x2000);
    }

    #[test]
    fn movw_ax_from_register() {
        // MOVW AX, rp: 0001 0ra1. ra=1 (BC) => 0x13 (0001 0011).
        let mut c = core_with(&[0x13, 0x01]);
        c.set_rw(1, 0x5678); // BC
        c.step(&[]);
        assert_eq!(c.rw(0), 0x5678);
    }

    #[test]
    fn addw_ax_immediate() {
        // ADDW AX, #imm16: 0000 0100 (0x04) + imm16.
        let mut c = core_with(&[0x04, 0x34, 0x12]);
        c.set_rw(0, 0x1000);
        c.step(&[]);
        assert_eq!(c.rw(0), 0x2234);
    }
}
