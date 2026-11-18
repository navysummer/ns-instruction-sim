//! An ST Microelectronics STM8 (ST8 core) instruction-level simulator.
//!
//! Models the STM8 CPU family used by IAR/Keil/SDCC STM8 toolchains: an 8-bit
//! accumulator (`A`), two 16-bit index registers (`X`, `Y`), a 16-bit stack
//! pointer (`SP`), a 24-bit program counter (`PC`), and a condition-code
//! register (`CC`: C=bit0, Z=bit1, N=bit2, I1=bit3, H=bit5, I0=bit6, V=bit7).
//!
//! The memory model is a flat 16 MiB image covering the 24-bit program space
//! and the 16-bit data window. Opcode encodings follow ST PM0044 and the SDCC
//! ucsim `disass_stm8` table.
//!
//! ## Encoding
//!
//! For the arithmetic/logic family (`0xA0-0xFF`) the **high nibble** selects
//! the addressing mode and the **low nibble** selects the operation:
//!
//! | high nibble | addressing mode |
//! |-------------|-----------------|
//! | `0xA` | immediate byte `#imm8` |
//! | `0xB` | direct short `addr8` |
//! | `0xC` | direct long `addr16` (or `[addr16]` with `0x92`) |
//! | `0xD` | indexed `(off16,X)` / `(off16,Y)` with `0x90` |
//! | `0xE` | indexed `(off8,X)` / `(off8,Y)` with `0x90` |
//! | `0xF` | indexed `(X)` / `(Y)` with `0x90` |
//!
//! Low nibble operation codes: `0`=SUB, `1`=CP, `2`=SBC, `3`=CPW, `4`=AND,
//! `5`=BCP, `6`=LD A,op, `7`=LD op,A, `8`=XOR, `9`=ADC, `A`=OR, `B`=ADD,
//! `C`=JP, `D`=CALL, `E`=LDW X,op, `F`=LDW op,X.
//!
//! The unary family (`0x30-0x7F`) uses the high nibble for the addressing
//! mode (`3`=`addr8`, `4`=A, `5`=`addr16`, `6`=`(off8,X)`, `7`=`(X)`) and the
//! low nibble for the operation (`0`=NEG, `1`=CPL, `2`=SRL, `3`=SRA, `4`=SLA,
//! `5`=RLC, `6`=RRC, `7`=SWAP, `8`=INC, `9`=DEC, `A`=CLR, `B`=TNZ).
//!
//! ## Prefixes
//!
//! * `0x90`: Y-indexed forms.
//! * `0x91`: SP-relative long `(off16,SP)`.
//! * `0x92`: indirect word-pointer `[addr16]`.
//! * `0x72`: extended forms (long direct, indirect with X).
//! * `0x72 0x90` / `0x90 0x72`: extended Y forms.
//!
//! ## Covered subset
//!
//! Loads/stores (`LD`, `LDW`, `LDF`), arithmetic (`ADD`/`ADC`/`SUB`/`SBC`/
//! `CP`, `MUL`/`DIV`/`DIVW`, `INC`/`DEC`, `NEG`, `CLR`, `ADDW`/`SUBW`), logic
//! (`AND`/`OR`/`XOR`/`CPL`/`BCP`, `SCF`/`RCF`/`CCF`/`RIM`/`SIM`), shifts
//! (`SLL`/`SRL`/`SRA`/`RLC`/`RRC`/`SWAP`/`RLWA`/`RRWA`), bit ops (`BSET`/
//! `BRES`/`BCPL`/`BCCM`, `BTJT`/`BTJF`), control (`JRA`/`JRF`/`JRxx`, `JP`,
//! `CALL`/`CALLR`/`CALLF`, `RET`/`RETF`/`IRET`, `NOP`, `HALT`, `TRAP`,
//! `BREAK`, `WFI`/`WFE`), stack (`PUSH`/`POP`, `ADDW SP`, `SUB SP`), and
//! exchange (`EXG`, `EXGW`, `MOV`).
//!
//! ## Simplifications
//!
//! * Interrupt vector dispatch is not modelled; `IRET`/`RETF` restore `PC`
//!   from the stack, enough to return from an ISR entered explicitly.
//! * Unknown opcodes set `halted` so the caller surfaces an honest stop.

/// Address-space size (16 MiB covers the 24-bit PC and 16-bit data window).
pub const ADDR_SIZE: usize = 16 * 1024 * 1024;

/// Typical reset stack pointer for small STM8 parts (RAM end).
pub const RESET_SP: u32 = 0x17FF;

// CC register bit masks.
pub const CC_C: u8 = 0x01;
pub const CC_Z: u8 = 0x02;
pub const CC_N: u8 = 0x04;
pub const CC_I1: u8 = 0x08;
pub const CC_H: u8 = 0x20;
pub const CC_I0: u8 = 0x40;
pub const CC_V: u8 = 0x80;

/// Outcome of a single STM8 step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stm8Step {
    Advanced,
    Breakpoint,
    Halted,
}

/// A resolved memory/register operand target for read/write.
#[derive(Debug, Clone, Copy)]
enum Target {
    /// Immediate byte: the next code byte is consumed but not written.
    Imm,
    /// Direct short: `addr8` (page zero).
    Direct8,
    /// Direct long: `addr16`.
    Direct16,
    /// Indexed `(X)`.
    IndexX,
    /// Indexed `(off8,X)`.
    IndexX8,
    /// Indexed `(off16,X)`.
    IndexX16,
    /// Indexed `(Y)`.
    IndexY,
    /// Indexed `(off8,Y)`.
    IndexY8,
    /// Indexed `(off16,Y)`.
    IndexY16,
    /// SP-relative `(off16,SP)`.
    IndexSp16,
    /// Indirect `[addr16]` (word pointer).
    Indirect,
    /// Indirect with X: `([addr16],X)`.
    IndirectX,
    /// Indirect with Y: `([addr16],Y)`.
    IndirectY,
}

/// An STM8 core.
pub struct Stm8Core {
    /// Accumulator (8-bit).
    pub a: u8,
    /// X index register (16-bit).
    pub x: u16,
    /// Y index register (16-bit).
    pub y: u16,
    /// Stack pointer (16-bit).
    pub sp: u16,
    /// Program counter (24-bit).
    pub pc: u32,
    /// Condition-code register.
    pub cc: u8,
    /// Flat memory image (16 MiB address space).
    pub memory: Vec<u8>,
    /// Whether execution should stop.
    pub halted: bool,
    /// Instructions executed since reset.
    pub cycle_count: u64,
}

impl Default for Stm8Core {
    fn default() -> Self {
        Self::new()
    }
}

impl Stm8Core {
    pub fn new() -> Self {
        Self {
            a: 0,
            x: 0,
            y: 0,
            sp: RESET_SP as u16,
            pc: 0,
            cc: 0x28, // reset: I0/I1 set, H clear
            memory: vec![0; ADDR_SIZE],
            halted: false,
            cycle_count: 0,
        }
    }

    /// Reset the core to `entry` (24-bit address). Memory is preserved.
    pub fn reset(&mut self, entry: u32) {
        self.a = 0;
        self.x = 0;
        self.y = 0;
        self.sp = RESET_SP as u16;
        self.pc = entry & 0xFF_FFFF;
        self.cc = 0x28;
        self.halted = false;
        self.cycle_count = 0;
    }

    /// Load a program image at `base` and start execution there.
    pub fn load_image(&mut self, base: u32, image: &[u8]) {
        self.write_bytes(base, image);
        self.set_pc(base);
        self.halted = false;
    }

    /// Set the program counter (24-bit).
    pub fn set_pc(&mut self, pc: u32) {
        self.pc = pc & 0xFF_FFFF;
    }

    // -------- condition flags --------
    pub fn flag_c(&self) -> bool {
        self.cc & CC_C != 0
    }
    fn set_c(&mut self, value: bool) {
        if value {
            self.cc |= CC_C;
        } else {
            self.cc &= !CC_C;
        }
    }
    pub fn flag_z(&self) -> bool {
        self.cc & CC_Z != 0
    }
    fn set_z(&mut self, value: bool) {
        if value {
            self.cc |= CC_Z;
        } else {
            self.cc &= !CC_Z;
        }
    }
    pub fn flag_n(&self) -> bool {
        self.cc & CC_N != 0
    }
    fn set_n(&mut self, value: bool) {
        if value {
            self.cc |= CC_N;
        } else {
            self.cc &= !CC_N;
        }
    }
    pub fn flag_v(&self) -> bool {
        self.cc & CC_V != 0
    }
    fn set_v(&mut self, value: bool) {
        if value {
            self.cc |= CC_V;
        } else {
            self.cc &= !CC_V;
        }
    }
    pub fn flag_h(&self) -> bool {
        self.cc & CC_H != 0
    }
    fn set_h(&mut self, value: bool) {
        if value {
            self.cc |= CC_H;
        } else {
            self.cc &= !CC_H;
        }
    }

    // -------- memory access --------
    fn read_u8(&self, addr: u32) -> u8 {
        self.memory
            .get((addr & 0xFF_FFFF) as usize)
            .copied()
            .unwrap_or(0)
    }
    fn read_u16(&self, addr: u32) -> u16 {
        let lo = self.read_u8(addr) as u16;
        let hi = self.read_u8(addr.wrapping_add(1)) as u16;
        (hi << 8) | lo
    }
    fn write_u8(&mut self, addr: u32, value: u8) {
        let idx = (addr & 0xFF_FFFF) as usize;
        if let Some(byte) = self.memory.get_mut(idx) {
            *byte = value;
        }
    }
    fn write_u16(&mut self, addr: u32, value: u16) {
        self.write_u8(addr, value as u8);
        self.write_u8(addr.wrapping_add(1), (value >> 8) as u8);
    }
    fn write_bytes(&mut self, base: u32, bytes: &[u8]) {
        for (i, byte) in bytes.iter().enumerate() {
            self.write_u8(base.wrapping_add(i as u32), *byte);
        }
    }
    fn zero_bytes(&mut self, base: u32, count: u32) {
        for i in 0..count {
            self.write_u8(base.wrapping_add(i), 0);
        }
    }

    // -------- public memory accessors (for the IDE) --------
    pub fn read_mem_u8(&self, addr: u32) -> u8 {
        self.read_u8(addr)
    }
    pub fn read_mem_u16(&self, addr: u32) -> u16 {
        self.read_u16(addr)
    }
    pub fn read_mem_u32(&self, addr: u32) -> u32 {
        let mut value = 0u32;
        for i in 0..4 {
            value |= (self.read_u8(addr.wrapping_add(i as u32)) as u32) << (8 * i);
        }
        value
    }
    pub fn write_mem_u8(&mut self, addr: u32, value: u8) {
        self.write_u8(addr, value);
    }
    pub fn write_mem_u16(&mut self, addr: u32, value: u16) {
        self.write_u16(addr, value);
    }
    pub fn write_mem_u32(&mut self, addr: u32, value: u32) {
        for i in 0..4 {
            self.write_u8(addr.wrapping_add(i as u32), (value >> (8 * i)) as u8);
        }
    }
    pub fn write_mem_bytes(&mut self, base: u32, bytes: &[u8]) {
        self.write_bytes(base, bytes);
    }
    pub fn zero_mem_bytes(&mut self, base: u32, count: u32) {
        self.zero_bytes(base, count);
    }

    // -------- stack --------
    fn push8(&mut self, value: u8) {
        self.write_u8(self.sp as u32, value);
        self.sp = self.sp.wrapping_sub(1);
    }
    fn pop8(&mut self) -> u8 {
        self.sp = self.sp.wrapping_add(1);
        self.read_u8(self.sp as u32)
    }
    fn push16(&mut self, value: u16) {
        self.push8((value >> 8) as u8);
        self.push8(value as u8);
    }
    fn pop16(&mut self) -> u16 {
        let lo = self.pop8() as u16;
        let hi = self.pop8() as u16;
        (hi << 8) | lo
    }

    // -------- flag helpers --------
    fn set_nz(&mut self, value: u8) {
        self.set_n(value & 0x80 != 0);
        self.set_z(value == 0);
    }
    fn add8(&mut self, a: u8, b: u8) -> u8 {
        let sum = a as u16 + b as u16;
        let result = sum as u8;
        self.set_c(sum > 0xFF);
        self.set_h((a & 0x0F) as u16 + (b & 0x0F) as u16 > 0x0F);
        self.set_v((a ^ result) & (b ^ result) & 0x80 != 0);
        self.set_nz(result);
        result
    }
    fn adc8(&mut self, a: u8, b: u8) -> u8 {
        let carry = self.flag_c() as u16;
        let sum = a as u16 + b as u16 + carry;
        let result = sum as u8;
        self.set_c(sum > 0xFF);
        self.set_h((a & 0x0F) as u16 + (b & 0x0F) as u16 + carry > 0x0F);
        self.set_v((a ^ result) & (b ^ result) & 0x80 != 0);
        self.set_nz(result);
        result
    }
    fn sub8(&mut self, a: u8, b: u8, borrow: bool) -> u8 {
        let borrow = borrow as i16;
        let diff = a as i16 - b as i16 - borrow;
        let result = diff as u8;
        self.set_c(diff < 0);
        self.set_h((a & 0x0F) as i16 - (b & 0x0F) as i16 - borrow < 0);
        self.set_v((a ^ b) & (a ^ result) & 0x80 != 0);
        self.set_nz(result);
        result
    }
    fn cmp8(&mut self, a: u8, b: u8) {
        let _ = self.sub8(a, b, false);
    }
    fn sub16(&mut self, a: u16, b: u16) -> u16 {
        let diff = a as i32 - b as i32;
        let result = diff as u16;
        self.set_c(diff < 0);
        self.set_n(result & 0x8000 != 0);
        self.set_z(result == 0);
        self.set_v((a ^ b) & (a ^ result) & 0x8000 != 0);
        result
    }

    // -------- fetch --------
    fn fetch8(&mut self) -> u8 {
        let b = self.read_u8(self.pc);
        self.pc = self.pc.wrapping_add(1) & 0xFF_FFFF;
        b
    }
    fn fetch16(&mut self) -> u16 {
        let lo = self.fetch8() as u16;
        let hi = self.fetch8() as u16;
        (hi << 8) | lo
    }
    fn fetch24(&mut self) -> u32 {
        let b0 = self.fetch8() as u32;
        let b1 = self.fetch8() as u32;
        let b2 = self.fetch8() as u32;
        (b2 << 16) | (b1 << 8) | b0
    }

    // -------- operand decode (addressing mode by HIGH nibble) --------
    /// Decode the addressing mode from the ALU-family high nibble and prefix.
    fn decode_alu_target(&mut self, high: u8, prefix: u8) -> Target {
        match (prefix, high & 0x0F) {
            (0x00, 0xA) => Target::Imm,
            (0x00, 0xB) => Target::Direct8,
            (0x00, 0xC) => Target::Direct16,
            (0x00, 0xD) => Target::IndexX16,
            (0x00, 0xE) => Target::IndexX8,
            (0x00, 0xF) => Target::IndexX,
            (0x90, 0xD) => Target::IndexY16,
            (0x90, 0xE) => Target::IndexY8,
            (0x90, 0xF) => Target::IndexY,
            (0x91, 0xB) => Target::IndexSp16,
            (0x92, 0xC) => Target::Indirect,
            (0x72, 0xC) => Target::Direct16,
            (0x72, 0xD) => Target::IndirectX,
            (0xF2, 0xD) => Target::IndirectY,
            _ => Target::Direct8,
        }
    }

    /// Read a target's value (advancing PC over operand bytes).
    fn read_target(&mut self, target: Target) -> u8 {
        match target {
            Target::Imm => self.fetch8(),
            Target::Direct8 => {
                let addr = self.fetch8() as u32;
                self.read_u8(addr)
            }
            Target::Direct16 => {
                let addr = self.fetch16() as u32;
                self.read_u8(addr)
            }
            Target::IndexX => self.read_u8(self.x as u32),
            Target::IndexX8 => {
                let off = self.fetch8() as u32;
                self.read_u8(self.x.wrapping_add(off as u16) as u32)
            }
            Target::IndexX16 => {
                let off = self.fetch16() as u32;
                self.read_u8(self.x.wrapping_add(off as u16) as u32)
            }
            Target::IndexY => self.read_u8(self.y as u32),
            Target::IndexY8 => {
                let off = self.fetch8() as u32;
                self.read_u8(self.y.wrapping_add(off as u16) as u32)
            }
            Target::IndexY16 => {
                let off = self.fetch16() as u32;
                self.read_u8(self.y.wrapping_add(off as u16) as u32)
            }
            Target::IndexSp16 => {
                let off = self.fetch16() as u32;
                self.read_u8(self.sp.wrapping_add(off as u16) as u32)
            }
            Target::Indirect => {
                let ptr = self.fetch16() as u32;
                self.read_u8(self.read_u16(ptr) as u32)
            }
            Target::IndirectX => {
                let ptr = self.fetch16() as u32;
                let addr = self.read_u16(ptr) as u32;
                self.read_u8(addr.wrapping_add(self.x as u32))
            }
            Target::IndirectY => {
                let ptr = self.fetch16() as u32;
                let addr = self.read_u16(ptr) as u32;
                self.read_u8(addr.wrapping_add(self.y as u32))
            }
        }
    }

    /// Write a value to a target (advancing PC over operand bytes).
    fn write_target(&mut self, target: Target, value: u8) {
        match target {
            Target::Imm => {
                let _ = self.fetch8();
            }
            Target::Direct8 => {
                let addr = self.fetch8() as u32;
                self.write_u8(addr, value);
            }
            Target::Direct16 => {
                let addr = self.fetch16() as u32;
                self.write_u8(addr, value);
            }
            Target::IndexX => self.write_u8(self.x as u32, value),
            Target::IndexX8 => {
                let off = self.fetch8() as u32;
                self.write_u8(self.x.wrapping_add(off as u16) as u32, value);
            }
            Target::IndexX16 => {
                let off = self.fetch16() as u32;
                self.write_u8(self.x.wrapping_add(off as u16) as u32, value);
            }
            Target::IndexY => self.write_u8(self.y as u32, value),
            Target::IndexY8 => {
                let off = self.fetch8() as u32;
                self.write_u8(self.y.wrapping_add(off as u16) as u32, value);
            }
            Target::IndexY16 => {
                let off = self.fetch16() as u32;
                self.write_u8(self.y.wrapping_add(off as u16) as u32, value);
            }
            Target::IndexSp16 => {
                let off = self.fetch16() as u32;
                self.write_u8(self.sp.wrapping_add(off as u16) as u32, value);
            }
            Target::Indirect => {
                let ptr = self.fetch16() as u32;
                let addr = self.read_u16(ptr) as u32;
                self.write_u8(addr, value);
            }
            Target::IndirectX => {
                let ptr = self.fetch16() as u32;
                let addr = self.read_u16(ptr) as u32;
                self.write_u8(addr.wrapping_add(self.x as u32), value);
            }
            Target::IndirectY => {
                let ptr = self.fetch16() as u32;
                let addr = self.read_u16(ptr) as u32;
                self.write_u8(addr.wrapping_add(self.y as u32), value);
            }
        }
    }

    /// Read a 16-bit word from a target.
    fn read_target16(&mut self, target: Target) -> u16 {
        match target {
            Target::Imm => self.fetch16(),
            Target::Direct8 => {
                let addr = self.fetch8() as u32;
                self.read_u16(addr)
            }
            Target::Direct16 => {
                let addr = self.fetch16() as u32;
                self.read_u16(addr)
            }
            Target::IndexX => self.read_u16(self.x as u32),
            Target::IndexX8 => {
                let off = self.fetch8() as u32;
                self.read_u16(self.x.wrapping_add(off as u16) as u32)
            }
            Target::IndexX16 => {
                let off = self.fetch16() as u32;
                self.read_u16(self.x.wrapping_add(off as u16) as u32)
            }
            Target::IndexY => self.read_u16(self.y as u32),
            Target::IndexY8 => {
                let off = self.fetch8() as u32;
                self.read_u16(self.y.wrapping_add(off as u16) as u32)
            }
            Target::IndexY16 => {
                let off = self.fetch16() as u32;
                self.read_u16(self.y.wrapping_add(off as u16) as u32)
            }
            Target::IndexSp16 => {
                let off = self.fetch16() as u32;
                self.read_u16(self.sp.wrapping_add(off as u16) as u32)
            }
            Target::Indirect => {
                let ptr = self.fetch16() as u32;
                self.read_u16(self.read_u16(ptr) as u32)
            }
            Target::IndirectX => {
                let ptr = self.fetch16() as u32;
                let addr = self.read_u16(ptr) as u32;
                self.read_u16(addr.wrapping_add(self.x as u32))
            }
            Target::IndirectY => {
                let ptr = self.fetch16() as u32;
                let addr = self.read_u16(ptr) as u32;
                self.read_u16(addr.wrapping_add(self.y as u32))
            }
        }
    }

    /// Write a 16-bit word to a target.
    fn write_target16(&mut self, target: Target, value: u16) {
        match target {
            Target::Imm => {
                let _ = self.fetch16();
            }
            Target::Direct8 => {
                let addr = self.fetch8() as u32;
                self.write_u16(addr, value);
            }
            Target::Direct16 => {
                let addr = self.fetch16() as u32;
                self.write_u16(addr, value);
            }
            Target::IndexX => self.write_u16(self.x as u32, value),
            Target::IndexX8 => {
                let off = self.fetch8() as u32;
                self.write_u16(self.x.wrapping_add(off as u16) as u32, value);
            }
            Target::IndexX16 => {
                let off = self.fetch16() as u32;
                self.write_u16(self.x.wrapping_add(off as u16) as u32, value);
            }
            Target::IndexY => self.write_u16(self.y as u32, value),
            Target::IndexY8 => {
                let off = self.fetch8() as u32;
                self.write_u16(self.y.wrapping_add(off as u16) as u32, value);
            }
            Target::IndexY16 => {
                let off = self.fetch16() as u32;
                self.write_u16(self.y.wrapping_add(off as u16) as u32, value);
            }
            Target::IndexSp16 => {
                let off = self.fetch16() as u32;
                self.write_u16(self.sp.wrapping_add(off as u16) as u32, value);
            }
            Target::Indirect => {
                let ptr = self.fetch16() as u32;
                let addr = self.read_u16(ptr) as u32;
                self.write_u16(addr, value);
            }
            Target::IndirectX => {
                let ptr = self.fetch16() as u32;
                let addr = self.read_u16(ptr) as u32;
                self.write_u16(addr.wrapping_add(self.x as u32), value);
            }
            Target::IndirectY => {
                let ptr = self.fetch16() as u32;
                let addr = self.read_u16(ptr) as u32;
                self.write_u16(addr.wrapping_add(self.y as u32), value);
            }
        }
    }

    /// Execute one instruction. Breakpoints are checked before any state change.
    pub fn step(&mut self, breakpoints: &[u32]) -> Stm8Step {
        if self.halted {
            return Stm8Step::Halted;
        }
        if breakpoints.iter().any(|bp| *bp & 0xFF_FFFF == self.pc) {
            return Stm8Step::Breakpoint;
        }
        self.execute();
        self.cycle_count += 1;
        if self.halted {
            Stm8Step::Halted
        } else {
            Stm8Step::Advanced
        }
    }

    /// Decode and execute one instruction, advancing PC as needed.
    fn execute(&mut self) {
        // Collect prefixes: 0x90, 0x91, 0x92, 0x72 may appear in a chain.
        let mut prefixes: Vec<u8> = Vec::new();
        let opcode;
        loop {
            let b = self.fetch8();
            match b {
                0x91 | 0x92 | 0x72 => prefixes.push(b),
                0x90 => {
                    // `0x90` is both the Y-index prefix and the standalone
                    // `LDW Y,X` instruction. It stays a prefix only when the
                    // next byte is another prefix or an opcode that actually
                    // has a Y form; otherwise it is the standalone load.
                    let next = self.read_u8(self.pc);
                    if is_stm8_prefix(next) || stm8_y_prefix_applies(next) {
                        prefixes.push(b);
                    } else {
                        opcode = 0x90;
                        break;
                    }
                }
                other => {
                    opcode = other;
                    break;
                }
            }
        }
        let has_72 = prefixes.contains(&0x72);
        let has_90 = prefixes.contains(&0x90);
        let prefix_key = if has_72 && has_90 {
            0xF2
        } else if has_72 {
            0x72
        } else if has_90 {
            0x90
        } else if prefixes.contains(&0x91) {
            0x91
        } else if prefixes.contains(&0x92) {
            0x92
        } else {
            0x00
        };
        self.execute_opcode(opcode, prefix_key);
    }

    /// Execute a decoded opcode. `prefix` is the effective prefix key
    /// (0x00, 0x72, 0x90, 0x91, 0x92, 0xF2).
    fn execute_opcode(&mut self, opcode: u8, prefix: u8) {
        // ---- 0x20-0x2F: short conditional/unconditional jumps ----
        if (0x20..=0x2F).contains(&opcode) && prefix == 0x00 {
            let offset = self.fetch8() as i8 as i32;
            self.branch(opcode, offset);
            return;
        }

        // ---- 0x00-0x1F: bit ops and SP-relative forms ----
        if (0x00..=0x1F).contains(&opcode) && prefix == 0x00 {
            self.execute_sp_and_bit(opcode);
            return;
        }

        // ---- 0x30-0x7F: unary family (high nibble = addressing mode) ----
        if (0x30..=0x7F).contains(&opcode) {
            self.execute_unary(opcode, prefix);
            return;
        }

        // ---- 0x80-0x9F: inherent/register ----
        if (0x80..=0x9F).contains(&opcode) {
            self.execute_inherent(opcode, prefix);
            return;
        }

        // ---- 0xA0-0xFF: ALU family (high nibble = addressing, low = op) ----
        if (0xA0..=0xFF).contains(&opcode) {
            self.execute_alu(opcode, prefix);
            return;
        }

        // Fallback: stop honestly.
        self.halted = true;
    }

    /// Conditional/unconditional short branch for opcodes 0x20-0x2F.
    fn branch(&mut self, opcode: u8, offset: i32) {
        let taken = match opcode & 0x0F {
            0x0 => true,                                                  // JRA
            0x1 => false,                                                 // JRF
            0x2 => self.flag_c() && !self.flag_z(),                       // JRUGT
            0x3 => self.flag_c() || self.flag_z(),                        // JRULE
            0x4 => self.flag_n() == self.flag_v(),                        // JRUGE
            0x5 => self.flag_n() != self.flag_v(),                        // JRULT
            0x6 => !self.flag_z(),                                        // JRNE
            0x7 => self.flag_z(),                                         // JREQ
            0x8 => !self.flag_n(),                                        // JRPL
            0x9 => self.flag_n(),                                         // JRMI
            0xA => !self.flag_z() && (self.flag_n() == self.flag_v()),    // JRSGT
            0xB => self.flag_z() || (self.flag_n() != self.flag_v()),     // JRSLE
            0xC => self.flag_n() == self.flag_v(),                        // JRSGE
            _ => self.flag_n() != self.flag_v(),                          // JRSLT
        };
        if taken {
            self.pc = self.pc.wrapping_add(offset as u32) & 0xFF_FFFF;
        }
    }

    /// Push PC+3 (return address) and jump.
    fn call(&mut self, target: u32) {
        let ret = self.pc & 0xFF_FFFF;
        self.push16(ret as u16);
        self.push8((ret >> 16) as u8);
        self.pc = target;
    }

    /// 0x00-0x1F: bit ops (btjt/btjf/bset/bres/bcpl/bccm) and SP-relative ALU.
    fn execute_sp_and_bit(&mut self, opcode: u8) {
        let low = opcode & 0x0F;
        let bit = (opcode >> 4) & 0x07;
        match low {
            // SP-relative ALU operations (0x10-0x1F low nibble, high 0x1).
            _ if opcode >= 0x10 => {
                let off = self.fetch8() as u32;
                let addr = self.sp.wrapping_add(off as u16) as u32;
                if low == 0x7 {
                    // LD (off8, SP), A: write A to the stack-relative byte.
                    self.write_u8(addr, self.a);
                } else if low == 0x6 {
                    // LD A, (off8, SP): load into A.
                    self.a = self.read_u8(addr);
                } else {
                    let operand = self.read_u8(addr);
                    self.alu_a_byte(low, operand, addr);
                }
            }
            // Bit test / unary on (off8,SP) for 0x00-0x0F.
            0x0 | 0x2 | 0x4 | 0x6 | 0x8 | 0xA | 0xC | 0xE => {
                // BTJT bit, addr8, rel (bit in high nibble)
                let addr = self.fetch8() as u32;
                let offset = self.fetch8() as i8 as i32;
                let value = self.read_u8(addr);
                if value & (1 << bit) != 0 {
                    self.pc = self.pc.wrapping_add(offset as u32) & 0xFF_FFFF;
                }
            }
            _ => {
                // BTJF bit, addr8, rel
                let addr = self.fetch8() as u32;
                let offset = self.fetch8() as i8 as i32;
                let value = self.read_u8(addr);
                if value & (1 << bit) == 0 {
                    self.pc = self.pc.wrapping_add(offset as u32) & 0xFF_FFFF;
                }
            }
        }
    }

    /// Apply an ALU byte operation to A with a memory operand.
    fn alu_a_byte(&mut self, op: u8, operand: u8, _addr: u32) {
        match op {
            0x0 => self.a = self.sub8(self.a, operand, false), // SUB
            0x1 => self.cmp8(self.a, operand),                 // CP
            0x2 => self.a = self.sub8(self.a, operand, self.flag_c()), // SBC
            0x4 => {
                let r = self.a & operand;
                self.set_nz(r);
                self.set_c(false);
                self.a = r;
            } // AND
            0x5 => {
                let r = self.a & operand;
                self.set_nz(r);
                self.set_c(false);
            } // BCP
            0x6 => self.a = operand, // LD A, op
            0x7 => self.a = operand, // (unused here; LD op,A handled separately)
            0x8 => {
                let r = self.a ^ operand;
                self.set_nz(r);
                self.set_c(false);
                self.a = r;
            } // XOR
            0x9 => self.a = self.adc8(self.a, operand), // ADC
            0xA => {
                let r = self.a | operand;
                self.set_nz(r);
                self.set_c(false);
                self.a = r;
            } // OR
            0xB => self.a = self.add8(self.a, operand), // ADD
            _ => {}
        }
    }

    /// Unary family 0x30-0x7F (high nibble = addressing mode).
    fn execute_unary(&mut self, opcode: u8, prefix: u8) {
        let high = (opcode >> 4) & 0x0F;
        let low = opcode & 0x0F;
        let addr = match (prefix, high) {
            (0x00, 0x3) => {
                let a = self.fetch8() as u32;
                a
            } // %d
            (0x00, 0x4) => {
                // A-register unary: operate directly.
                self.unary_reg(low);
                return;
            }
            (0x00, 0x5) => {
                let a = self.fetch16() as u32;
                a
            } // %x
            (0x00, 0x6) => {
                let off = self.fetch8() as u32;
                self.x.wrapping_add(off as u16) as u32
            } // (off8,X)
            (0x00, 0x7) => self.x as u32, // (X)
            (0x90, 0x6) => {
                let off = self.fetch8() as u32;
                self.y.wrapping_add(off as u16) as u32
            } // (off8,Y)
            (0x90, 0x7) => self.y as u32, // (Y)
            (0x92, 0x3) => {
                let ptr = self.fetch16() as u32;
                self.read_u16(ptr) as u32
            } // [%w]
            (0x72, 0x3) => {
                let ptr = self.fetch16() as u32;
                self.read_u16(ptr).wrapping_add(self.x) as u32
            } // ([%w],X)
            (0xF2, 0x3) => {
                let ptr = self.fetch16() as u32;
                self.read_u16(ptr).wrapping_add(self.y) as u32
            } // ([%w],Y)
            _ => {
                self.halted = true;
                return;
            }
        };
        let value = self.read_u8(addr);
        let result = self.unary_op(low, value);
        if low != 0xB {
            self.write_u8(addr, result);
        }
    }

    /// Unary op on A (0x40-0x4F high nibble).
    fn unary_reg(&mut self, low: u8) {
        self.a = self.unary_op(low, self.a);
    }

    /// Unary operation core (low nibble = op).
    fn unary_op(&mut self, op: u8, value: u8) -> u8 {
        match op & 0x0F {
            0x0 => {
                let r = self.sub8(0, value, false);
                self.set_z(r == 0);
                r
            } // NEG
            0x1 => {
                let r = !value;
                self.set_nz(r);
                r
            } // CPL
            0x2 => {
                self.set_c(value & 1 != 0);
                let r = value >> 1;
                self.set_nz(r);
                r
            } // SRL
            0x3 => {
                self.set_c(value & 1 != 0);
                let r = ((value as i8) >> 1) as u8;
                self.set_nz(r);
                r
            } // SRA
            0x4 => {
                self.set_c(value & 0x80 != 0);
                let r = value << 1;
                self.set_nz(r);
                r
            } // SLA
            0x5 => {
                let r = (value << 1) | self.flag_c() as u8;
                self.set_c(value & 0x80 != 0);
                self.set_nz(r);
                r
            } // RLC
            0x6 => {
                let r = (value >> 1) | ((self.flag_c() as u8) << 7);
                self.set_c(value & 1 != 0);
                self.set_nz(r);
                r
            } // RRC
            0x7 => {
                let r = value.rotate_left(4);
                self.set_nz(r);
                r
            } // SWAP
            0x8 => {
                let r = value.wrapping_add(1);
                self.set_nz(r);
                self.set_c(r == 0);
                r
            } // INC
            0x9 => {
                let r = value.wrapping_sub(1);
                self.set_nz(r);
                self.set_c(r == 0xFF);
                r
            } // DEC
            0xA => {
                self.set_z(true);
                self.set_n(false);
                self.set_c(false);
                0
            } // CLR
            0xB => {
                self.set_nz(value);
                value
            } // TNZ
            _ => value,
        }
    }

    /// Inherent/register group 0x80-0x9F.
    fn execute_inherent(&mut self, opcode: u8, prefix: u8) {
        let has_90 = prefix == 0x90 || prefix == 0xF2;
        match opcode {
            0x80 => {
                // IRET
                let ret = self.pop16();
                self.pc = ((self.pop8() as u32) << 16) | ret as u32;
                self.cc = self.pop8();
            }
            0x81 => {
                // RET
                let ret = self.pop16();
                self.pc = ((self.pop8() as u32) << 16) | ret as u32;
            }
            0x82 => {
                // INT addr24
                let target = self.fetch24();
                self.push16(self.pc as u16);
                self.push8((self.pc >> 16) as u8);
                self.push8(self.cc);
                self.pc = target;
            }
            0x83 => self.halted = true,   // TRAP
            0x84 => self.a = self.pop8(), // POP A
            0x85 => {
                self.x = self.pop16();
                self.y = self.pop16();
            } // POPW X/Y
            0x86 => self.cc = self.pop8(), // POP CC
            0x87 => {
                // RETF
                let ret = self.pop16();
                self.pc = ((self.pop8() as u32) << 16) | ret as u32;
            }
            0x88 => self.push8(self.a), // PUSH A
            0x89 => {
                self.push16(self.x);
                self.push16(self.y);
            } // PUSHW X/Y
            0x8A => self.push8(self.cc), // PUSH CC
            0x8B => self.halted = true,  // BREAK
            0x8C => self.cc ^= CC_C,     // CCF
            0x8D => {
                let target = self.fetch24();
                self.call(target);
            } // CALLF
            0x8E => self.halted = true, // HALT
            0x8F => self.halted = true, // WFI/WFE
            0x90 => self.y = self.x,    // LDW Y, X
            0x93 => self.x = self.y,    // LDW X, Y
            0x94 => {
                if has_90 {
                    self.sp = self.y;
                } else {
                    self.sp = self.x;
                }
            } // LDW SP, X/Y
            0x95 => {
                if has_90 {
                    self.y = (self.y & 0x00FF) | ((self.a as u16) << 8);
                } else {
                    self.x = (self.x & 0x00FF) | ((self.a as u16) << 8);
                }
            } // LD XH/YH, A
            0x96 => {
                if has_90 {
                    self.y = self.sp;
                } else {
                    self.x = self.sp;
                }
            } // LDW X/Y, SP
            0x97 => {
                if has_90 {
                    self.y = (self.y & 0xFF00) | self.a as u16;
                } else {
                    self.x = (self.x & 0xFF00) | self.a as u16;
                }
            } // LD XL/YL, A
            0x98 => self.cc &= !CC_C, // RCF
            0x99 => self.cc |= CC_C,  // SCF
            0x9A => self.cc &= !(CC_I0 | CC_I1), // RIM
            0x9B => self.cc |= CC_I0 | CC_I1,    // SIM
            0x9C => self.cc &= !CC_H,  // RVF
            0x9D => {}                 // NOP
            0x9E => {
                self.a = if has_90 { (self.y >> 8) as u8 } else { (self.x >> 8) as u8 };
            } // LD A, XH/YH
            0x9F => {
                self.a = if has_90 { (self.y & 0xFF) as u8 } else { (self.x & 0xFF) as u8 };
            } // LD A, XL/YL
            _ => self.halted = true,
        }
    }

    /// ALU family 0xA0-0xFF (high nibble = addressing mode, low = operation).
    fn execute_alu(&mut self, opcode: u8, prefix: u8) {
        let high = (opcode >> 4) & 0x0F;
        let low = opcode & 0x0F;
        let target = self.decode_alu_target(high, prefix);
        match low {
            0x0 => {
                // SUB A, op
                let operand = self.read_target(target);
                self.a = self.sub8(self.a, operand, false);
            }
            0x1 => {
                // CP A, op
                let operand = self.read_target(target);
                self.cmp8(self.a, operand);
            }
            0x2 => {
                // SBC A, op
                let operand = self.read_target(target);
                self.a = self.sub8(self.a, operand, self.flag_c());
            }
            0x3 => {
                // CPW X/Y, word
                let operand16 = self.read_target16(target);
                if high == 0xA || prefix == 0x90 {
                    let _ = self.sub16(self.y, operand16);
                } else {
                    let _ = self.sub16(self.x, operand16);
                }
            }
            0x4 => {
                // AND A, op
                let operand = self.read_target(target);
                let r = self.a & operand;
                self.set_nz(r);
                self.set_c(false);
                self.a = r;
            }
            0x5 => {
                // BCP A, op
                let operand = self.read_target(target);
                let r = self.a & operand;
                self.set_nz(r);
                self.set_c(false);
            }
            0x6 => {
                // LD A, op
                self.a = self.read_target(target);
            }
            0x7 => {
                // LD op, A
                let value = self.a;
                self.write_target(target, value);
            }
            0x8 => {
                // XOR A, op
                let operand = self.read_target(target);
                let r = self.a ^ operand;
                self.set_nz(r);
                self.set_c(false);
                self.a = r;
            }
            0x9 => {
                // ADC A, op
                let operand = self.read_target(target);
                self.a = self.adc8(self.a, operand);
            }
            0xA => {
                // OR A, op
                let operand = self.read_target(target);
                let r = self.a | operand;
                self.set_nz(r);
                self.set_c(false);
                self.a = r;
            }
            0xB => {
                // ADD A, op
                let operand = self.read_target(target);
                self.a = self.add8(self.a, operand);
            }
            0xC => {
                // JP op
                let addr = self.read_target16(target);
                self.pc = addr as u32 & 0xFF_FFFF;
            }
            0xD => {
                // CALL op
                let addr = self.read_target16(target);
                self.call(addr as u32);
            }
            0xE => {
                // LDW X, op
                self.x = self.read_target16(target);
            }
            0xF => {
                // LDW op, X
                let value = self.x;
                self.write_target16(target, value);
            }
            _ => {}
        }
    }
}

/// Disassemble one STM8 instruction at `pc` using `read` for bytes.
/// Effective prefix key, mirroring the core's `execute` prefix resolution:
/// `0x72`+`0x90` becomes the combined `0xF2` key, otherwise the single prefix
/// wins, and no prefix is `0x00`.
fn prefix_key(has_72: bool, has_90: bool, has_91: bool, has_92: bool) -> u8 {
    if has_72 && has_90 {
        0xF2
    } else if has_72 {
        0x72
    } else if has_90 {
        0x90
    } else if has_91 {
        0x91
    } else if has_92 {
        0x92
    } else {
        0x00
    }
}

/// Whether `b` is an STM8 prefix byte (`0x90`/`0x91`/`0x92`/`0x72`).
fn is_stm8_prefix(b: u8) -> bool {
    matches!(b, 0x90 | 0x91 | 0x92 | 0x72)
}

/// Whether `opcode` has a Y-index form that the `0x90` prefix selects.
///
/// `0x90` is both the Y-index prefix and the standalone `LDW Y,X`. It is only
/// a prefix when the following opcode actually has a Y form; otherwise it
/// decodes as the standalone load and the following byte starts a new
/// instruction. The set mirrors the addressing forms the core decodes:
/// unary `(off8,Y)`/`(Y)` (`0x60-0x7F`), the inherent X/Y-select group
/// (`0x94-0x97`/`0x9E-0x9F`) and the ALU indexed-Y family (`0xD0-0xFF`).
fn stm8_y_prefix_applies(opcode: u8) -> bool {
    match opcode {
        0x60..=0x7F => true,
        0x94..=0x97 | 0x9E..=0x9F => true,
        0xD0..=0xFF => true,
        _ => false,
    }
}

/// Decode the ALU addressing mode selected by the high nibble and prefix,
/// matching `Stm8Core::decode_alu_target`.
fn alu_target(pk: u8, high: u8) -> Target {
    match (pk, high & 0x0F) {
        (0x00, 0xA) => Target::Imm,
        (0x00, 0xB) => Target::Direct8,
        (0x00, 0xC) => Target::Direct16,
        (0x00, 0xD) => Target::IndexX16,
        (0x00, 0xE) => Target::IndexX8,
        (0x00, 0xF) => Target::IndexX,
        (0x90, 0xD) => Target::IndexY16,
        (0x90, 0xE) => Target::IndexY8,
        (0x90, 0xF) => Target::IndexY,
        (0x91, 0xB) => Target::IndexSp16,
        (0x92, 0xC) => Target::Indirect,
        (0x72, 0xC) => Target::Direct16,
        (0x72, 0xD) => Target::IndirectX,
        (0xF2, 0xD) => Target::IndirectY,
        _ => Target::Direct8,
    }
}

/// Number of operand bytes consumed by a target. Word operations read a
/// 16-bit immediate while byte operations read an 8-bit immediate; all other
/// addressing modes consume the same number of bytes either way.
fn target_len(target: Target, word: bool) -> u32 {
    match target {
        Target::Imm => {
            if word {
                2
            } else {
                1
            }
        }
        Target::Direct8 => 1,
        Target::Direct16 => 2,
        Target::IndexX | Target::IndexY => 0,
        Target::IndexX8 | Target::IndexY8 => 1,
        Target::IndexX16 | Target::IndexY16 => 2,
        Target::IndexSp16 => 2,
        Target::Indirect | Target::IndirectX | Target::IndirectY => 2,
    }
}

/// Number of operand bytes consumed by an unary-family instruction.
fn unary_operand_len(opcode: u8, pk: u8) -> u32 {
    let high = (opcode >> 4) & 0x0F;
    match (pk, high) {
        (0x00, 0x3) | (0x00, 0x6) | (0x90, 0x6) => 1,
        (0x00, 0x5) | (0x92, 0x3) | (0x72, 0x3) | (0xF2, 0x3) => 2,
        (0x00, 0x4) | (0x00, 0x7) | (0x90, 0x7) => 0,
        _ => 0,
    }
}

/// Number of operand bytes consumed by an instruction's opcode under the
/// effective prefix `pk`.
fn operand_len(opcode: u8, pk: u8) -> u32 {
    if opcode <= 0x1F {
        return if pk == 0x00 {
            if opcode <= 0x0F {
                2 // BTJT/BTJF bit, addr8, rel
            } else {
                1 // SP-relative ALU off8
            }
        } else {
            0
        };
    }
    if opcode <= 0x2F {
        return if pk == 0x00 { 1 } else { 0 }; // short jump rel
    }
    if opcode <= 0x7F {
        return unary_operand_len(opcode, pk);
    }
    if opcode <= 0x9F {
        return if opcode == 0x82 || opcode == 0x8D { 3 } else { 0 }; // INT/CALLF addr24
    }
    let high = (opcode >> 4) & 0x0F;
    let low = opcode & 0x0F;
    let word = low == 0x3 || low >= 0xC;
    target_len(alu_target(pk, high), word)
}

/// Number of code bytes consumed by the STM8 instruction at `pc`, including
/// any `0x90`/`0x91`/`0x92`/`0x72` prefixes. Matches the core's fetch model.
pub fn instruction_len(pc: u32, read: &dyn Fn(u32) -> u8) -> u32 {
    let mut off = 0u32;
    let (mut has_72, mut has_90, mut has_91, mut has_92) = (false, false, false, false);
    loop {
        match read(pc.wrapping_add(off)) {
            0x72 => has_72 = true,
            0x91 => has_91 = true,
            0x92 => has_92 = true,
            0x90 => {
                let next = read(pc.wrapping_add(off + 1));
                if is_stm8_prefix(next) || stm8_y_prefix_applies(next) {
                    has_90 = true;
                } else {
                    // Standalone `LDW Y,X`: one byte, no operand.
                    return off + 1;
                }
            }
            opcode => {
                let pk = prefix_key(has_72, has_90, has_91, has_92);
                return off + 1 + operand_len(opcode, pk);
            }
        }
        off += 1;
    }
}

fn rd16(pc: u32, read: &dyn Fn(u32) -> u8) -> u32 {
    read(pc) as u32 | ((read(pc + 1) as u32) << 8)
}

fn rd24(pc: u32, read: &dyn Fn(u32) -> u8) -> u32 {
    read(pc) as u32 | ((read(pc + 1) as u32) << 8) | ((read(pc + 2) as u32) << 16)
}

fn render_target(target: Target, word: bool, base: u32, read: &dyn Fn(u32) -> u8) -> String {
    match target {
        Target::Imm => {
            if word {
                format!("#0x{:04x}", rd16(base, read))
            } else {
                format!("#0x{:02x}", read(base))
            }
        }
        Target::Direct8 => format!("0x{:02x}", read(base)),
        Target::Direct16 => format!("0x{:04x}", rd16(base, read)),
        Target::IndexX => "(x)".to_string(),
        Target::IndexX8 => format!("(0x{:02x}, x)", read(base)),
        Target::IndexX16 => format!("(0x{:04x}, x)", rd16(base, read)),
        Target::IndexY => "(y)".to_string(),
        Target::IndexY8 => format!("(0x{:02x}, y)", read(base)),
        Target::IndexY16 => format!("(0x{:04x}, y)", rd16(base, read)),
        Target::IndexSp16 => format!("(0x{:04x}, sp)", rd16(base, read)),
        Target::Indirect => format!("[0x{:04x}]", rd16(base, read)),
        Target::IndirectX => format!("([0x{:04x}], x)", rd16(base, read)),
        Target::IndirectY => format!("([0x{:04x}], y)", rd16(base, read)),
    }
}

fn decode_alu(opcode: u8, pk: u8, base: u32, read: &dyn Fn(u32) -> u8) -> String {
    let high = (opcode >> 4) & 0x0F;
    let low = opcode & 0x0F;
    let word = low == 0x3 || low >= 0xC;
    let operand = render_target(alu_target(pk, high), word, base, read);
    match low {
        0x0 => format!("sub a, {operand}"),
        0x1 => format!("cp a, {operand}"),
        0x2 => format!("sbc a, {operand}"),
        0x3 => {
            let dst = if high == 0xA || pk == 0x90 { "y" } else { "x" };
            format!("cpw {dst}, {operand}")
        }
        0x4 => format!("and a, {operand}"),
        0x5 => format!("bcp a, {operand}"),
        0x6 => format!("ld a, {operand}"),
        0x7 => format!("ld {operand}, a"),
        0x8 => format!("xor a, {operand}"),
        0x9 => format!("adc a, {operand}"),
        0xA => format!("or a, {operand}"),
        0xB => format!("add a, {operand}"),
        0xC => format!("jp {operand}"),
        0xD => format!("call {operand}"),
        0xE => format!("ldw x, {operand}"),
        0xF => format!("ldw {operand}, x"),
        _ => format!("0x{opcode:02x}"),
    }
}

fn decode_unary(opcode: u8, pk: u8, base: u32, read: &dyn Fn(u32) -> u8) -> String {
    let high = (opcode >> 4) & 0x0F;
    let low = opcode & 0x0F;
    let op = match low {
        0x0 => "neg",
        0x1 => "cpl",
        0x2 => "srl",
        0x3 => "sra",
        0x4 => "sla",
        0x5 => "rlc",
        0x6 => "rrc",
        0x7 => "swap",
        0x8 => "inc",
        0x9 => "dec",
        0xA => "clr",
        0xB => "tnz",
        _ => "?",
    };
    let operand = match (pk, high) {
        (0x00, 0x3) => format!("0x{:02x}", read(base)),
        (0x00, 0x4) => "a".to_string(),
        (0x00, 0x5) => format!("0x{:04x}", rd16(base, read)),
        (0x00, 0x6) => format!("(0x{:02x}, x)", read(base)),
        (0x00, 0x7) => "(x)".to_string(),
        (0x90, 0x6) => format!("(0x{:02x}, y)", read(base)),
        (0x90, 0x7) => "(y)".to_string(),
        (0x92, 0x3) => format!("[0x{:04x}]", rd16(base, read)),
        (0x72, 0x3) => format!("([0x{:04x}], x)", rd16(base, read)),
        (0xF2, 0x3) => format!("([0x{:04x}], y)", rd16(base, read)),
        _ => return format!("0x{opcode:02x}"),
    };
    format!("{op} {operand}")
}

fn decode_inherent(opcode: u8, pk: u8, base: u32, read: &dyn Fn(u32) -> u8) -> String {
    let has_90 = pk == 0x90 || pk == 0xF2;
    match opcode {
        0x80 => "iret".to_string(),
        0x81 => "ret".to_string(),
        0x82 => format!("int 0x{:06x}", rd24(base, read)),
        0x83 => "trap".to_string(),
        0x84 => "pop a".to_string(),
        0x85 => "popw x, y".to_string(),
        0x86 => "pop cc".to_string(),
        0x87 => "retf".to_string(),
        0x88 => "push a".to_string(),
        0x89 => "pushw x, y".to_string(),
        0x8A => "push cc".to_string(),
        0x8B => "break".to_string(),
        0x8C => "ccf".to_string(),
        0x8D => format!("callf 0x{:06x}", rd24(base, read)),
        0x8E => "halt".to_string(),
        0x8F => "wfi".to_string(),
        0x90 => "ldw y, x".to_string(),
        0x93 => "ldw x, y".to_string(),
        0x94 => {
            if has_90 {
                "ldw sp, y".to_string()
            } else {
                "ldw sp, x".to_string()
            }
        }
        0x95 => {
            if has_90 {
                "ld yh, a".to_string()
            } else {
                "ld xh, a".to_string()
            }
        }
        0x96 => {
            if has_90 {
                "ldw y, sp".to_string()
            } else {
                "ldw x, sp".to_string()
            }
        }
        0x97 => {
            if has_90 {
                "ld yl, a".to_string()
            } else {
                "ld xl, a".to_string()
            }
        }
        0x98 => "rcf".to_string(),
        0x99 => "scf".to_string(),
        0x9A => "rim".to_string(),
        0x9B => "sim".to_string(),
        0x9C => "rvf".to_string(),
        0x9D => "nop".to_string(),
        0x9E => {
            if has_90 {
                "ld a, yh".to_string()
            } else {
                "ld a, xh".to_string()
            }
        }
        0x9F => {
            if has_90 {
                "ld a, yl".to_string()
            } else {
                "ld a, xl".to_string()
            }
        }
        _ => format!("0x{opcode:02x}"),
    }
}

fn decode_bit_sp(opcode: u8, base: u32, read: &dyn Fn(u32) -> u8) -> String {
    let low = opcode & 0x0F;
    let bit = (opcode >> 4) & 0x07;
    if opcode >= 0x10 {
        let op = match low {
            0x0 => "sub",
            0x1 => "cp",
            0x2 => "sbc",
            0x4 => "and",
            0x5 => "bcp",
            0x6 => "ld",
            0x7 => "ld",
            0x8 => "xor",
            0x9 => "adc",
            0xA => "or",
            0xB => "add",
            _ => "?",
        };
        if low == 0x7 {
            format!("ld (0x{:02x}, sp), a", read(base))
        } else {
            format!("{op} a, (0x{:02x}, sp)", read(base))
        }
    } else {
        let addr = read(base);
        let rel = read(base + 1) as i8 as i32;
        let m = if low & 1 == 0 { "btjt" } else { "btjf" };
        format!("{m} #{bit}, 0x{addr:02x}, {rel:+}")
    }
}

fn decode_short_jump(
    opcode: u8,
    base: u32,
    start: u32,
    off: u32,
    read: &dyn Fn(u32) -> u8,
) -> String {
    let rel = read(base) as i8 as i32;
    let names = [
        "jra", "jrf", "jrugt", "jrule", "jruge", "jrult", "jrne", "jreq", "jrpl", "jrmi",
        "jrsgt", "jrsle", "jrsge", "jrslt",
    ];
    let name = names
        .get(opcode as usize - 0x20)
        .copied()
        .unwrap_or("jr?");
    let target = start
        .wrapping_add(off)
        .wrapping_add(2)
        .wrapping_add(rel as u32)
        & 0xFF_FFFF;
    format!("{name} 0x{target:x}")
}

fn decode_opcode(
    opcode: u8,
    pk: u8,
    base: u32,
    start: u32,
    off: u32,
    read: &dyn Fn(u32) -> u8,
) -> String {
    if opcode <= 0x1F {
        if pk == 0x00 {
            return decode_bit_sp(opcode, base, read);
        }
        return format!("0x{opcode:02x}");
    }
    if opcode <= 0x2F {
        if pk == 0x00 {
            return decode_short_jump(opcode, base, start, off, read);
        }
        return format!("0x{opcode:02x}");
    }
    if opcode <= 0x7F {
        return decode_unary(opcode, pk, base, read);
    }
    if opcode <= 0x9F {
        return decode_inherent(opcode, pk, base, read);
    }
    decode_alu(opcode, pk, base, read)
}

pub fn disassemble(pc: u32, read: &dyn Fn(u32) -> u8) -> String {
    let mut off = 0u32;
    let (mut has_72, mut has_90, mut has_91, mut has_92) = (false, false, false, false);
    loop {
        match read(pc.wrapping_add(off)) {
            0x72 => has_72 = true,
            0x91 => has_91 = true,
            0x92 => has_92 = true,
            0x90 => {
                let next = read(pc.wrapping_add(off + 1));
                if is_stm8_prefix(next) || stm8_y_prefix_applies(next) {
                    has_90 = true;
                } else {
                    let pk = prefix_key(has_72, false, has_91, has_92);
                    let base = pc.wrapping_add(off + 1);
                    return decode_opcode(0x90, pk, base, pc, off, read);
                }
            }
            opcode => {
                let pk = prefix_key(has_72, has_90, has_91, has_92);
                let base = pc.wrapping_add(off + 1);
                return decode_opcode(opcode, pk, base, pc, off, read);
            }
        }
        off += 1;
    }
}

#[cfg(test)]
mod disasm_tests {
    use super::*;

    fn reader(bytes: &[u8]) -> impl Fn(u32) -> u8 {
        let bytes = bytes.to_vec();
        move |addr: u32| {
            bytes
                .get(addr as usize)
                .copied()
                .unwrap_or(0)
        }
    }

    #[test]
    fn instruction_len_matches_core_fetch() {
        // nop (1), ret (1), halt (1).
        let r = reader(&[0x9D]);
        assert_eq!(instruction_len(0, &r), 1);
        let r = reader(&[0x81]);
        assert_eq!(instruction_len(0, &r), 1);
        // jra rel: 1 + 1.
        let r = reader(&[0x20, 0x02]);
        assert_eq!(instruction_len(0, &r), 2);
        // BTJT bit, addr8, rel: 1 + 2.
        let r = reader(&[0x00, 0x10, 0x02]);
        assert_eq!(instruction_len(0, &r), 3);
        // SP-relative ALU: 1 + 1.
        let r = reader(&[0x12, 0x34]);
        assert_eq!(instruction_len(0, &r), 2);
        // Unary direct8: 1 + 1.
        let r = reader(&[0x34, 0x12]);
        assert_eq!(instruction_len(0, &r), 2);
        // Unary direct16: 1 + 2.
        let r = reader(&[0x56, 0x34, 0x12]);
        assert_eq!(instruction_len(0, &r), 3);
        // CALLF addr24: 1 + 3.
        let r = reader(&[0x8D, 0x78, 0x56, 0x34]);
        assert_eq!(instruction_len(0, &r), 4);
        // CALL direct16: 1 + 2.
        let r = reader(&[0xCD, 0x56, 0x34]);
        assert_eq!(instruction_len(0, &r), 3);
        // ALU immediate byte: 1 + 1.
        let r = reader(&[0xA0, 0x05]);
        assert_eq!(instruction_len(0, &r), 2);
        // ALU immediate word (LDW X, #imm16): 1 + 2.
        let r = reader(&[0xAE, 0x34, 0x12]);
        assert_eq!(instruction_len(0, &r), 3);
        // Prefixed Y-indexed (0x90 + (off8,Y)): 1 + 1 + 1.
        let r = reader(&[0x90, 0x65, 0x10]);
        assert_eq!(instruction_len(0, &r), 3);
        // 0x72+0x90 combined key on indirect-Y CPW: 2 + 1 + 2.
        let r = reader(&[0x72, 0x90, 0xD3, 0x34, 0x12]);
        assert_eq!(instruction_len(0, &r), 5);
    }

    #[test]
    fn disassembles_common_forms() {
        let r = reader(&[0x9D]);
        assert_eq!(disassemble(0, &r), "nop");
        let r = reader(&[0x81]);
        assert_eq!(disassemble(0, &r), "ret");
        let r = reader(&[0x8E]);
        assert_eq!(disassemble(0, &r), "halt");
        // jra target = start + 2 + rel (rel +2) = 0x4.
        let r = reader(&[0x20, 0x02]);
        assert_eq!(disassemble(0, &r), "jra 0x4");
        // CALL direct16.
        let r = reader(&[0xCD, 0x56, 0x34]);
        assert_eq!(disassemble(0, &r), "call 0x3456");
        // CALLF addr24.
        let r = reader(&[0x8D, 0x78, 0x56, 0x34]);
        assert_eq!(disassemble(0, &r), "callf 0x345678");
        // ADD A, #imm8.
        let r = reader(&[0xAB, 0x05]);
        assert_eq!(disassemble(0, &r), "add a, #0x05");
        // LDW X, #imm16.
        let r = reader(&[0xAE, 0x34, 0x12]);
        assert_eq!(disassemble(0, &r), "ldw x, #0x1234");
        // RLC (off8, Y) with 0x90 prefix.
        let r = reader(&[0x90, 0x65, 0x10]);
        assert_eq!(disassemble(0, &r), "rlc (0x10, y)");
    }

    #[test]
    fn sp_relative_ld() {
        // 0x16/0x17 are LD A,(off,SP) / LD (off,SP),A.
        let r = reader(&[0x16, 0x34]);
        assert_eq!(disassemble(0, &r), "ld a, (0x34, sp)");
        let r = reader(&[0x17, 0x34]);
        assert_eq!(disassemble(0, &r), "ld (0x34, sp), a");
        // Execution: LD (off,SP),A writes A; LD A,(off,SP) reads it back.
        let mut core = Stm8Core::new();
        core.load_image(0, &[0x17, 0x34, 0x16, 0x34]);
        core.sp = 0x100;
        core.a = 0xAB;
        core.step(&[]);
        assert_eq!(core.read_u8(0x100 + 0x34), 0xAB);
        core.a = 0;
        core.step(&[]);
        assert_eq!(core.a, 0xAB);
    }

    #[test]
    fn standalone_90_is_ldw_y_x() {
        // A bare 0x90 (or one followed by an opcode without a Y form) is the
        // single-byte `LDW Y,X`, not a prefix.
        let r = reader(&[0x90]);
        assert_eq!(disassemble(0, &r), "ldw y, x");
        let r = reader(&[0x90, 0x9D]);
        assert_eq!(disassemble(0, &r), "ldw y, x");
        // NEG addr8 (0x34) has no Y form, so 0x90 stays the standalone load.
        let r = reader(&[0x90, 0x34, 0x12]);
        assert_eq!(disassemble(0, &r), "ldw y, x");
        // Two consecutive 0x90 bytes are two standalone loads.
        let r = reader(&[0x90, 0x90]);
        assert_eq!(disassemble(0, &r), "ldw y, x");
        // A 0x90 followed by a Y-form opcode remains the prefix.
        let r = reader(&[0x90, 0x65, 0x10]);
        assert_eq!(disassemble(0, &r), "rlc (0x10, y)");
        // Instruction lengths: standalone 0x90 is one byte.
        let r = reader(&[0x90, 0x34, 0x12]);
        assert_eq!(instruction_len(0, &r), 1);
        let r = reader(&[0x90, 0x65, 0x10]);
        assert_eq!(instruction_len(0, &r), 3);
    }

    #[test]
    fn executes_standalone_90_then_next() {
        // LDW Y,X (0x90) followed by NOP (0x9D): two steps.
        let mut core = Stm8Core::new();
        core.load_image(0, &[0x90, 0x9D]);
        core.x = 0x1234;
        core.y = 0;
        core.step(&[]);
        assert_eq!(core.y, 0x1234, "0x90 loads Y from X");
        assert_eq!(core.pc, 1);
        core.step(&[]);
        assert_eq!(core.pc, 2, "NOP follows the standalone load");
    }
}
