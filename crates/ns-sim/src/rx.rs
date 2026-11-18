//! A Renesas RX (RX600 family, IAR RX) instruction-level simulator core.
//!
//! Models the RX CPU used by IAR/Keil-style RX toolchains: sixteen 32-bit
//! registers (`r0`-`r15`; `r0` is the stack pointer and `r1` the accumulator
//! by the standard RX ABI), a program status word (PSW: C/Z/S/O/I/U plus the
//! interrupt stack bits), a flat 32-bit byte-addressed little-endian address
//! space, and the variable-length (16/32/48-bit) instruction encodings.
//!
//! ## Covered instructions
//!
//! * Moves: `MOV` reg-reg / `#uimm4` / `#imm` / `#imm8`, `MOVU`, `MOV`
//!   `dsp[rd]` short and byte/word/long immediate forms.
//! * Arithmetic/logic: `ADD`, `SUB`, `CMP`, `AND`, `OR`, `XOR`, `MUL`,
//!   `DIV`, `DIVU`, `EMUL`, `EMULU`, `ADC`, `SBB`, `INC`, `DEC`, `NEG`,
//!   `NOT`, `ABS`, `MAX`, `MIN`, `TST`.
//! * Shifts: `SHLL`, `SHLR`, `SHAR` with imm5 or register.
//! * Memory: `LD.b/w/l`, `ST.b/w/l` (register-indirect and displacement),
//!   `PUSH`, `POP`, `PUSHM`, `POPM`.
//! * Control: `BRA`, `BSR`, `BCnd` (short/byte/word/a), `JMP`, `JSR`,
//!   `RTS`, `RTSD`, `BRK`, `NOP`, `WAIT`.
//! * Bit ops: `BTST`, `BSET`, `BCLR`, `BNOT`, `SCCnd`, `STZ`, `STNZ`,
//!   `SETPSW`, `CLRPSW`.
//! * Control registers: `MVFC`, `MVTC`, `MVTIPL` (stubs; PSW/ISP/USP
//!   handled, others ignored).
//!
//! Encodings follow the GCC/LLVM RX backends and the QEMU RX decoder
//! (`target/rx/insns.decode`), cross-checked against the RX user's manual.

/// Flat memory window size (16 MiB covers typical RX600 flash + SRAM).
pub const MEMORY_SIZE: usize = 16 * 1024 * 1024;

/// The RX PSW flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Psw {
    /// Carry flag.
    pub c: bool,
    /// Zero flag.
    pub z: bool,
    /// Sign flag.
    pub s: bool,
    /// Overflow flag.
    pub o: bool,
    /// Interrupt priority mask (3 bits, I).
    pub i: u8,
    /// User stack/instruction flag (U).
    pub u: bool,
}

impl Default for Psw {
    fn default() -> Self {
        Psw { c: false, z: false, s: false, o: false, i: 0, u: false }
    }
}

impl Psw {
    fn to_u32(&self) -> u32 {
        let mut v = 0u32;
        if self.c { v |= 1 << 0; }
        if self.z { v |= 1 << 1; }
        if self.s { v |= 1 << 2; }
        if self.o { v |= 1 << 3; }
        v |= (self.i as u32 & 0x7) << 4;
        if self.u { v |= 1 << 8; }
        v
    }

    fn from_u32(v: u32) -> Self {
        Psw {
            c: v & 1 != 0,
            z: v & 2 != 0,
            s: v & 4 != 0,
            o: v & 8 != 0,
            i: ((v >> 4) & 0x7) as u8,
            u: v & 0x100 != 0,
        }
    }
}

/// The outcome of a single RX step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RxStep {
    Advanced,
    Breakpoint,
    Halted,
}

/// A Renesas RX core.
pub struct RxCore {
    /// Sixteen 32-bit registers; `r0` is the stack pointer per the RX ABI.
    pub regs: [u32; 16],
    /// Program status word.
    pub psw: Psw,
    /// Program counter (byte address).
    pub pc: u32,
    /// Flat byte-addressable memory.
    pub mem: Vec<u8>,
    /// Whether the core has halted (BRK/WAIT).
    pub halted: bool,
}

impl Default for RxCore {
    fn default() -> Self {
        Self::new()
    }
}

impl RxCore {
    pub fn new() -> Self {
        RxCore {
            regs: [0; 16],
            psw: Psw::default(),
            pc: 0,
            mem: vec![0; MEMORY_SIZE],
            halted: false,
        }
    }

    /// Reset to `entry`, zeroing registers and clearing the halt flag.
    pub fn reset(&mut self, entry: u32) {
        self.regs = [0; 16];
        self.psw = Psw::default();
        self.pc = entry;
        self.halted = false;
    }

    pub fn set_pc(&mut self, pc: u32) {
        self.pc = pc;
    }

    /// Load a program image at `base` and start the PC there.
    pub fn load_image(&mut self, base: u32, image: &[u8]) {
        self.write_bytes(base, image);
        self.pc = base;
        self.halted = false;
    }

    /// Register label for the IDE register view.
    pub fn register_label(index: usize) -> String {
        if index < 16 {
            format!("r{}", index)
        } else {
            format!("r{}", index)
        }
    }

    /// Read a register by index (0-15); out-of-range returns 0.
    pub fn read_reg(&self, index: usize) -> u32 {
        if index < 16 { self.regs[index] } else { 0 }
    }

    /// Write a register by index (0-15); out-of-range is ignored.
    pub fn write_reg(&mut self, index: usize, value: u32) {
        if index < 16 {
            self.regs[index] = value;
        }
    }

    /// Read one byte from memory (out-of-range returns 0).
    pub fn read_u8(&self, address: u32) -> u8 {
        let a = address as usize;
        if a < self.mem.len() { self.mem[a] } else { 0 }
    }

    /// Read a little-endian 32-bit word from memory.
    pub fn read_u32(&self, address: u32) -> u32 {
        let a = address as usize;
        if a + 3 < self.mem.len() {
            u32::from_le_bytes([
                self.mem[a],
                self.mem[a + 1],
                self.mem[a + 2],
                self.mem[a + 3],
            ])
        } else {
            let mut v = 0u32;
            for i in 0..4 {
                v |= (self.read_u8(address + i) as u32) << (8 * i);
            }
            v
        }
    }

    /// Write a little-endian 32-bit word to memory.
    pub fn write_u32(&mut self, address: u32, value: u32) {
        let a = address as usize;
        if a + 3 < self.mem.len() {
            self.mem[a..a + 4].copy_from_slice(&value.to_le_bytes());
        } else {
            for i in 0..4 {
                self.write_u8(address + i, (value >> (8 * i)) as u8);
            }
        }
    }

    /// Write one byte to memory (out-of-range ignored).
    pub fn write_u8(&mut self, address: u32, value: u8) {
        let a = address as usize;
        if a < self.mem.len() {
            self.mem[a] = value;
        }
    }

    /// Write a byte slice to memory starting at `base`.
    pub fn write_bytes(&mut self, base: u32, bytes: &[u8]) {
        let b = base as usize;
        for (i, &byte) in bytes.iter().enumerate() {
            let a = b + i;
            if a < self.mem.len() {
                self.mem[a] = byte;
            }
        }
    }

    fn fetch8(&mut self) -> u8 {
        let b = self.read_u8(self.pc);
        self.pc = self.pc.wrapping_add(1);
        b
    }

    fn fetch16(&mut self) -> u16 {
        let b0 = self.fetch8() as u16;
        let b1 = self.fetch8() as u16;
        (b0 << 8) | b1
    }

    fn fetch32(&mut self) -> u32 {
        let b0 = self.fetch8() as u32;
        let b1 = self.fetch8() as u32;
        let b2 = self.fetch8() as u32;
        let b3 = self.fetch8() as u32;
        (b0 << 24) | (b1 << 16) | (b2 << 8) | b3
    }

    // ---- Flags ----

    fn set_nz(&mut self, value: u32) {
        self.psw.z = value == 0;
        self.psw.s = (value as i32) < 0;
    }

    /// Compute and store C/Z/S/O for an addition.
    fn add_flags(&mut self, a: u32, b: u32, carry: u32) -> u32 {
        let sum = a.wrapping_add(b).wrapping_add(carry);
        let mask = 0x8000_0000;
        let o = ((a & mask) == (b & mask)) && ((a & mask) != (sum & mask));
        self.psw.c = (a as u64) + (b as u64) + (carry as u64) > 0xFFFF_FFFF;
        self.psw.o = o;
        self.set_nz(sum);
        sum
    }

    /// Compute and store C/Z/S/O for a subtraction.
    fn sub_flags(&mut self, a: u32, b: u32, borrow: u32) -> u32 {
        let (diff, borrow_out) = a.overflowing_sub(b.wrapping_add(borrow));
        let mask = 0x8000_0000;
        let o = ((a & mask) != (b & mask)) && ((a & mask) != (diff & mask));
        self.psw.c = borrow_out;
        self.psw.o = o;
        self.set_nz(diff);
        diff
    }

    fn cond_true(&self, cond: u8) -> bool {
        match cond & 0x0F {
            0x0 => self.psz(),            // EQ / Z
            0x1 => !self.psz(),           // NE / NZ
            0x2 => self.psc(),            // C
            0x3 => !self.psc(),           // NC
            0x4 => self.pss(),            // N
            0x5 => !self.pss(),           // P (>= 0)
            0x6 => self.pso(),            // O
            0x7 => !self.pso(),           // NO
            0x8 => self.pss() && !self.pso(), // LE
            0x9 => !self.pss() && self.pso(), // LT (signed <)
            0xA => !self.pss() && !self.pso(), // GT
            0xB => self.pss() && self.pso(),   // GE (signed >=)
            0xC => self.psz() && !self.psc(),  // HI (unsigned >)
            0xD => !self.psz() && self.psc(),  // LS (unsigned <=)
            0xE => self.psz() || self.pss(),   // F? (always for cond 0xE in RX)
            0xF => true,                       // Always
            _ => false,
        }
    }

    fn psz(&self) -> bool { self.psw.z }
    fn psc(&self) -> bool { self.psw.c }
    fn pss(&self) -> bool { self.psw.s }
    fn pso(&self) -> bool { self.psw.o }

    // ---- Stack ----

    fn push(&mut self, value: u32) {
        self.regs[0] = self.regs[0].wrapping_sub(4);
        self.write_u32(self.regs[0], value);
    }

    fn pop(&mut self) -> u32 {
        let v = self.read_u32(self.regs[0]);
        self.regs[0] = self.regs[0].wrapping_add(4);
        v
    }

    // ---- Memory helpers ----

    /// Load from memory with size: 0=byte (sign), 1=word (sign), 2=long.
    fn load(&self, addr: u32, sz: u8) -> u32 {
        match sz {
            0 => {
                let b = self.read_u8(addr);
                b as i8 as i32 as u32
            }
            1 => {
                let w = self.read_u8(addr) as u16 | ((self.read_u8(addr + 1) as u16) << 8);
                w as i16 as i32 as u32
            }
            _ => self.read_u32(addr),
        }
    }

    /// Load from memory with size: 0=byte (zero), 1=word (zero), 2=long.
    fn load_u(&self, addr: u32, sz: u8) -> u32 {
        match sz {
            0 => self.read_u8(addr) as u32,
            1 => {
                (self.read_u8(addr) as u32) | ((self.read_u8(addr + 1) as u32) << 8)
            }
            _ => self.read_u32(addr),
        }
    }

    /// Store a value to memory with size: 0=byte, 1=word, 2=long.
    fn store(&mut self, addr: u32, value: u32, sz: u8) {
        match sz {
            0 => self.write_u8(addr, value as u8),
            1 => {
                self.write_u8(addr, value as u8);
                self.write_u8(addr + 1, (value >> 8) as u8);
            }
            _ => self.write_u32(addr, value),
        }
    }

    /// Read the `sz`-sized operand and return the address of the memory form.
    /// `rs` is the base register. `dsp` is a signed displacement.
    fn mem_addr(&self, rs: u32, dsp: i32) -> u32 {
        rs.wrapping_add(dsp as u32)
    }

    // ---- Branch ----

    fn branch_disp(&mut self, disp: i32, link: bool) {
        if link {
            let ret = self.pc;
            self.push(ret);
        }
        self.pc = self.pc.wrapping_add(disp as u32);
    }

    /// Execute one step, stopping at a breakpoint.
    pub fn step(&mut self, breakpoints: &[u32]) -> RxStep {
        if self.halted {
            return RxStep::Halted;
        }
        if breakpoints.contains(&self.pc) {
            return RxStep::Breakpoint;
        }
        let pc = self.pc;
        self.execute();
        if self.halted {
            return RxStep::Halted;
        }
        if self.pc == pc {
            // Guard against infinite loops on unknown instructions.
            return RxStep::Halted;
        }
        RxStep::Advanced
    }

    fn execute(&mut self) {
        let b0 = self.read_u8(self.pc);
        // 1-byte instructions: BRA.s (0x08-0x0F) and BCnd.s (0x10-0x1F).
        if b0 >= 0x08 && b0 <= 0x1F {
            self.pc = self.pc.wrapping_add(1);
            if b0 <= 0x0F {
                // BRA.s: displacement is the low 7 bits, sign-extended.
                let dsp = sign7(b0 as u16 & 0x7F);
                self.pc = self.pc.wrapping_add(dsp as u32);
            } else {
                // BCnd.s: cond is the low nibble; displacement from low 3 bits
                // of b0 plus the next 7 bits.
                let cond = b0 & 0x0F;
                let dsp = sign7(((b0 as u16 & 0x07) << 3) | (self.read_u8(self.pc) as u16 & 0x7F));
                self.pc = self.pc.wrapping_add(1);
                if self.cond_true(cond) {
                    self.pc = self.pc.wrapping_add(dsp as u32);
                }
            }
            return;
        }
        let insn = self.fetch16();
        self.execute_opcode(insn);
    }

    // ---- Main dispatch ----

    fn execute_opcode(&mut self, insn: u16) {
        let op = ((insn >> 8) & 0xFF) as u8;
        let low = (insn & 0xFF) as u8;

        // 0000 0000 = BRK, 0000 0010 = RTS, 0000 0011 = NOP.
        if insn == 0x0000 {
            self.halted = true;
            return;
        }
        if insn == 0x0002 {
            // RTS
            let ret = self.pop();
            self.pc = ret;
            return;
        }
        if insn == 0x0003 {
            // NOP
            return;
        }
        // 0000 0100 = BRA.a (branch, no link), 0000 0101 = BSR.a (call, link).
        if insn == 0x0004 {
            let dsp = self.fetch32() as i32;
            self.branch_disp(dsp, false);
            return;
        }
        if insn == 0x0005 {
            let dsp = self.fetch32() as i32;
            self.branch_disp(dsp, true);
            return;
        }
        // 0111 1111 1001 0110 = WAIT
        if insn == 0x7996 {
            self.halted = true;
            return;
        }
        // 0000 1... = BRA.s (short, dsp7)
        if (insn & 0xF800) == 0x0800 {
            let dsp = sign7(insn & 0x7F);
            self.pc = self.pc.wrapping_add(dsp as u32);
            return;
        }
        // 0001 .... = BCnd.s (short)
        if (op & 0xF0) == 0x10 {
            let cond = ((insn >> 8) & 0x0F) as u8;
            let dsp = sign7(insn & 0x7F);
            if self.cond_true(cond) {
                self.pc = self.pc.wrapping_add(dsp as u32);
            }
            return;
        }
        // 0010 .... = BCnd.b / BRA.b
        if (op & 0xF0) == 0x20 {
            if insn == 0x2E00 {
                // BRA.b dsp8 (op=0x2E)
                let dsp = low as i8 as i32;
                self.pc = self.pc.wrapping_add(dsp as u32);
                return;
            }
            let cond = op & 0x0F;
            let dsp = low as i8 as i32;
            if self.cond_true(cond) {
                self.pc = self.pc.wrapping_add(dsp as u32);
            }
            return;
        }
        // 0011 1000 = BRA.w (branch, no link), 0011 1001 = BSR.w (call, link).
        if insn == 0x3800 {
            let dsp = self.fetch16() as i16 as i32;
            self.branch_disp(dsp, false);
            return;
        }
        if insn == 0x3900 {
            let dsp = self.fetch16() as i16 as i32;
            self.branch_disp(dsp, true);
            return;
        }
        // 0011 101 . = BCnd.w
        if (op & 0xF8) == 0x3A {
            let cond = (op & 0x07) as u8;
            let dsp_lo = low as i16 as i32;
            let dsp_hi = self.fetch8() as i16 as i32;
            let dsp = (dsp_lo | (dsp_hi << 8)) as i32;
            if self.cond_true(cond) {
                self.pc = self.pc.wrapping_add(dsp as u32);
            }
            return;
        }
        // 0011 1100/1101/1110 = MOV.b/w/l #imm8, dsp5[rd]
        if (op & 0xFC) == 0x3C {
            let sz = op & 0x03;
            let rd = ((insn >> 4) & 0x07) as usize;
            let imm = self.fetch8() as u32;
            let dsp = (((insn >> 7) & 1) << 4 | (insn & 0x0F)) as i32;
            let addr = self.regs[rd].wrapping_add(dsp as u32);
            self.store(addr, imm, sz);
            return;
        }
        // 0011 1111 = RTSD #imm, rd-rd2
        if op == 0x3F {
            let rd = ((insn >> 4) & 0x0F) as usize;
            let rd2 = (insn & 0x0F) as usize;
            let imm = self.fetch8() as u32;
            self.regs[0] = self.regs[0].wrapping_add(imm);
            let mut r = rd;
            while r <= rd2 && r < 16 {
                self.regs[r] = self.pop();
                r += 1;
            }
            self.pc = self.pop();
            return;
        }
        // 0100 00.. = SUB dsp[rs].ub / SUB rs,rd
        if (op & 0xFC) == 0x40 {
            let ld = op & 0x03;
            let rs = ((low >> 4) & 0x0F) as usize;
            let rd = (low & 0x0F) as usize;
            self.binop(rs, rd, ld, 0x00);
            return;
        }
        // 0100 01.. = CMP dsp[rs].ub / CMP rs,rd
        if (op & 0xFC) == 0x44 {
            let ld = op & 0x03;
            let rs = ((low >> 4) & 0x0F) as usize;
            let rd = (low & 0x0F) as usize;
            self.binop(rs, rd, ld, 0x01);
            return;
        }
        // 0100 10.. = ADD dsp[rs].ub / ADD rs,rd
        if (op & 0xFC) == 0x48 {
            let ld = op & 0x03;
            let rs = ((low >> 4) & 0x0F) as usize;
            let rd = (low & 0x0F) as usize;
            self.binop(rs, rd, ld, 0x02);
            return;
        }
        // 0100 11.. = MUL dsp[rs].ub / MUL rs,rd
        if (op & 0xFC) == 0x4C {
            let ld = op & 0x03;
            let rs = ((low >> 4) & 0x0F) as usize;
            let rd = (low & 0x0F) as usize;
            self.binop(rs, rd, ld, 0x03);
            return;
        }
        // 0101 00.. = AND dsp[rs].ub / AND rs,rd
        if (op & 0xFC) == 0x50 {
            let ld = op & 0x03;
            let rs = ((low >> 4) & 0x0F) as usize;
            let rd = (low & 0x0F) as usize;
            self.binop(rs, rd, ld, 0x04);
            return;
        }
        // 0101 01.. = OR dsp[rs].ub / OR rs,rd
        if (op & 0xFC) == 0x54 {
            let ld = op & 0x03;
            let rs = ((low >> 4) & 0x0F) as usize;
            let rd = (low & 0x0F) as usize;
            self.binop(rs, rd, ld, 0x05);
            return;
        }
        // 0101 1... = MOVU (byte/word zero-extend loads)
        if (op & 0xF8) == 0x58 {
            let sz = (op >> 2) & 0x01;
            let kind = op & 0x03;
            let rs = ((low >> 4) & 0x0F) as usize;
            let rd = (low & 0x0F) as usize;
            match kind {
                0 => {
                    // MOVU.b/w rs, rd (register indirect)
                    let v = self.load_u(self.regs[rs], sz);
                    self.regs[rd] = v;
                }
                1 => {
                    // MOVU.b/w dsp8[rs], rd
                    let dsp = self.fetch8() as i8 as i32;
                    let addr = self.mem_addr(self.regs[rs], dsp);
                    let v = self.load_u(addr, sz);
                    self.regs[rd] = v;
                }
                2 => {
                    // MOVU.b/w dsp16[rs], rd
                    let dsp = self.fetch16() as i16 as i32;
                    let addr = self.mem_addr(self.regs[rs], dsp);
                    let v = self.load_u(addr, sz);
                    self.regs[rd] = v;
                }
                _ => {
                    // MOVU.b/w #imm, rd
                    let imm = self.fetch16();
                    self.regs[rd] = imm as u32;
                }
            }
            return;
        }
        // 0110 0000 = SUB #uimm4, rd; 0110 0001 = CMP #uimm4, rs2;
        // 0110 0010 = ADD #uimm4, rd; 0110 0011 = MUL #uimm4, rd
        if (op & 0xFC) == 0x60 {
            let sub = op & 0x03;
            let imm = ((insn >> 4) & 0x0F) as u32;
            let rd = (low & 0x0F) as usize;
            match sub {
                0 => {
                    let v = self.regs[rd].wrapping_sub(imm);
                    self.regs[rd] = v;
                    self.sub_flags(self.regs[rd].wrapping_add(imm), imm, 0);
                }
                1 => {
                    // CMP #uimm4, rs2: sets flags only.
                    let v = self.regs[rd];
                    self.sub_flags(v, imm, 0);
                }
                2 => {
                    let v = self.add_flags(self.regs[rd], imm, 0);
                    self.regs[rd] = v;
                }
                _ => {
                    let v = self.regs[rd].wrapping_mul(imm);
                    self.regs[rd] = v;
                    self.set_nz(v);
                }
            }
            return;
        }
        // 0110 0100 = AND #uimm4, rd; 0110 0101 = OR #uimm4, rd;
        // 0110 0110 = MOV #uimm4, rd
        if (op & 0xFC) == 0x64 {
            let kind = op & 0x03;
            let imm = ((insn >> 4) & 0x0F) as u32;
            let rd = (low & 0x0F) as usize;
            match kind {
                0 => {
                    let v = self.regs[rd] & imm;
                    self.regs[rd] = v;
                    self.set_nz(v);
                }
                1 => {
                    let v = self.regs[rd] | imm;
                    self.regs[rd] = v;
                    self.set_nz(v);
                }
                _ => {
                    self.regs[rd] = imm;
                }
            }
            return;
        }
        // 0110 100. = SHLR #imm5, rd; 0110 110. = SHLL #imm5, rd
        if (op & 0xFC) == 0x68 || (op & 0xFC) == 0x6C {
            let left = (op & 0x04) != 0;
            let imm = ((insn >> 4) & 0x1F) as u32;
            let rd = (low & 0x0F) as usize;
            let v = self.regs[rd];
            let (result, carry) = if left {
                if imm == 0 { (v, self.psw.c) } else {
                    let c = ((v >> (32 - imm)) & 1) != 0;
                    (v << imm, c)
                }
            } else {
                if imm == 0 { (v, self.psw.c) } else {
                    let c = ((v >> (imm - 1)) & 1) != 0;
                    (v >> imm, c)
                }
            };
            self.regs[rd] = result;
            self.psw.c = carry;
            self.set_nz(result);
            return;
        }
        // 0110 1110 = PUSHM rs-rs2; 0110 1111 = POPM rd-rd2
        if op == 0x6E {
            let rs = ((insn >> 4) & 0x0F) as usize;
            let rs2 = (low & 0x0F) as usize;
            let mut r = rs;
            while r <= rs2 && r < 16 {
                self.push(self.regs[r]);
                r += 1;
            }
            return;
        }
        if op == 0x6F {
            let rd = ((insn >> 4) & 0x0F) as usize;
            let rd2 = (low & 0x0F) as usize;
            let mut r = rd2;
            while r >= rd && r < 16 {
                self.regs[r] = self.pop();
                if r == 0 { break; }
                r -= 1;
            }
            return;
        }
        // 0111 00.. = ADD #imm, rs, rd (imm16)
        if (op & 0xFC) == 0x70 {
            let rs = ((insn >> 4) & 0x0F) as usize;
            let rd = (low & 0x0F) as usize;
            let imm = self.fetch16() as u32;
            let v = self.add_flags(self.regs[rs], imm, 0);
            self.regs[rd] = v;
            return;
        }
        // 0111 01.. = CMP/AND/OR/MUL #imm, rd (imm16 or imm8)
        if (op & 0xFC) == 0x74 {
            let kind = (low >> 4) & 0x0F;
            let rd = (low & 0x0F) as usize;
            match kind {
                0 => {
                    // CMP #imm, rd
                    let imm = self.fetch16() as u32;
                    self.sub_flags(self.regs[rd], imm, 0);
                }
                1 => {
                    // MUL #imm, rd
                    let imm = self.fetch16() as u32;
                    let v = self.regs[rd].wrapping_mul(imm);
                    self.regs[rd] = v;
                    self.set_nz(v);
                }
                2 => {
                    // AND #imm, rd
                    let imm = self.fetch16() as u32;
                    let v = self.regs[rd] & imm;
                    self.regs[rd] = v;
                    self.set_nz(v);
                }
                _ => {
                    // OR #imm, rd
                    let imm = self.fetch16() as u32;
                    let v = self.regs[rd] | imm;
                    self.regs[rd] = v;
                    self.set_nz(v);
                }
            }
            return;
        }
        // 0111 100. = BSET #imm5, rd; 0111 101. = BCLR #imm5, rd;
        // 0111 110. = BTST #imm5, rd
        if op >= 0x78 && op <= 0x7D {
            let kind = (op >> 1) & 0x03;
            let imm = ((insn >> 8) & 0x1F) as u32;
            let rd = (low & 0x0F) as usize;
            let v = self.regs[rd];
            let bit = 1u32 << (imm & 0x1F);
            match kind {
                0 => { self.regs[rd] = v | bit; }
                1 => { self.regs[rd] = v & !bit; }
                2 => {
                    self.psw.z = (v & bit) == 0;
                }
                _ => { self.regs[rd] = v | bit; }
            }
            return;
        }
        // 0111 1110 0000 rd = NOT rd; 0001 = NEG; 0010 = ABS
        // 0111 1110 10 sz rs = PUSH rs; 1100 = PUSHC cr; 1011 = POP rd;
        // 1101 = ...; 1110 = POPC cr.
        if op == 0x7E {
            let kind = (low >> 4) & 0x0F;
            let rd = (low & 0x0F) as usize;
            match kind {
                0 => {
                    // NOT rd
                    let v = self.regs[rd];
                    let r = !v;
                    self.regs[rd] = r;
                    self.set_nz(r);
                }
                1 => {
                    // NEG rd
                    let v = self.regs[rd];
                    let r = self.sub_flags(0, v, 0);
                    self.regs[rd] = r;
                }
                2 => {
                    // ABS rd
                    let v = self.regs[rd];
                    let r = if (v as i32) < 0 { self.sub_flags(0, v, 0) } else { v };
                    self.regs[rd] = r;
                }
                3 => {
                    // SAT rd (clamp to 0x7FFFFFFF / 0x80000000)
                    let v = self.regs[rd];
                    let r = if (v as i32) > i32::MAX { 0x7FFF_FFFF }
                            else if (v as i32) < i32::MIN { 0x8000_0000 }
                            else { v };
                    self.regs[rd] = r;
                }
                4 => {
                    // RORC rd
                    let v = self.regs[rd];
                    let c = self.psw.c as u32;
                    self.psw.c = (v & 1) != 0;
                    self.regs[rd] = (v >> 1) | (c << 31);
                }
                5 => {
                    // ROLC rd
                    let v = self.regs[rd];
                    let c = self.psw.c as u32;
                    self.psw.c = ((v >> 31) & 1) != 0;
                    self.regs[rd] = (v << 1) | c;
                }
                8 => {
                    // PUSH rs (sz=00/word)
                    let rs = rd;
                    self.push(self.regs[rs]);
                }
                10 => {
                    // POP rd
                    self.regs[rd] = self.pop();
                }
                12 => {
                    // PUSHC cr
                    let cr = rd;
                    if cr == 0 {
                        self.push(self.psw.to_u32());
                    }
                }
                14 => {
                    // POPC cr
                    let cr = rd;
                    if cr == 0 {
                        self.psw = Psw::from_u32(self.pop());
                    }
                }
                _ => {
                    self.halted = true;
                }
            }
            return;
        }
        // 0111 1111 0000 rs = JMP rs; 0001 = JSR rs; 0100 = BRA.l; 0101 = BSR.l
        if op == 0x7F {
            let kind = (low >> 4) & 0x0F;
            let rs = (low & 0x0F) as usize;
            match kind {
                0 => { self.pc = self.regs[rs]; }
                1 => {
                    let ret = self.pc;
                    self.push(ret);
                    self.pc = self.regs[rs];
                }
                4 => { self.pc = self.regs[rs]; }
                5 => {
                    let ret = self.pc;
                    self.push(ret);
                    self.pc = self.regs[rs];
                }
                _ => {
                    // Unknown; halt to avoid runaway.
                    self.halted = true;
                }
            }
            return;
        }
        // 1000 0 = MOV.b rs, dsp5[rd]; 1001 0 = MOV.w; 1010 0 = MOV.l (store)
        if (op & 0xF8) == 0x80 || (op & 0xF8) == 0x90 || (op & 0xF8) == 0xA0 {
            let sz = ((op >> 4) & 0x03) as u8;
            let rd = ((insn >> 4) & 0x07) as usize;
            let rs = (insn & 0x07) as usize;
            let dsp = ((((insn >> 7) & 0x0F) << 1) | ((insn >> 3) & 1)) as i32;
            let addr = self.mem_addr(self.regs[rd], dsp);
            self.store(addr, self.regs[rs], sz);
            return;
        }
        // 1000 1 = MOV.b dsp5[rs], rd; 1001 1 = MOV.w; 1010 1 = MOV.l (load)
        if (op & 0xF8) == 0x88 || (op & 0xF8) == 0x98 || (op & 0xF8) == 0xA8 {
            let sz = ((op >> 4) & 0x03) as u8;
            let rs = ((insn >> 4) & 0x07) as usize;
            let rd = (insn & 0x07) as usize;
            let dsp = ((((insn >> 7) & 0x0F) << 1) | ((insn >> 3) & 1)) as i32;
            let addr = self.mem_addr(self.regs[rs], dsp);
            let v = self.load(addr, sz);
            self.regs[rd] = v;
            return;
        }
        // 1011 sz ... = MOVU with dsp5 (byte/word zero-extend)
        if (op & 0xF0) == 0xB0 {
            let sz = ((insn >> 11) & 0x01) as u8;
            let rs = ((insn >> 4) & 0x07) as usize;
            let rd = (insn & 0x07) as usize;
            let dsp = ((((insn >> 8) & 0x07) << 1) | ((insn >> 3) & 1)) as i32;
            let addr = self.mem_addr(self.regs[rs], dsp);
            let v = self.load_u(addr, sz);
            self.regs[rd] = v;
            return;
        }
        // 1100/1101/1110 = MOV.b/w/l dsp[lds], dsp[ldd]. The 0xE0 family is
        // `11 1 sz ...` (op 0xE0-0xEF); op 0xF0-0xFF are the bit-operation,
        // register-indexed and long-opcode families handled below.
        if (op & 0xE0) == 0xC0 || (op & 0xF0) == 0xE0 {
            let sz = ((op >> 5) & 0x03) as u8;
            let ldd = ((insn >> 8) & 0x03) as i32;
            let lds = ((low >> 4) & 0x03) as i32;
            let rs = (low & 0x0F) as usize;
            let rd = ((insn >> 4) & 0x0F) as usize;
            // Read src displacement (if any) then dst displacement.
            let src_dsp = match lds {
                0 | 1 => 0,
                2 => self.fetch8() as i8 as i32,
                _ => self.fetch16() as i16 as i32,
            };
            let dst_dsp = match ldd {
                0 | 1 => 0,
                2 => self.fetch8() as i8 as i32,
                _ => self.fetch16() as i16 as i32,
            };
            let src_addr = self.mem_addr(self.regs[rs], src_dsp);
            let dst_addr = self.mem_addr(self.regs[rd], dst_dsp);
            let v = self.load_u(src_addr, sz);
            self.store(dst_addr, v, sz);
            return;
        }
        // 1111 0000 = BSET/BCLR #imm, dsp[rd]; 1111 0100 = BTST #imm, dsp[rd]
        // (bit operations on a memory byte addressed by dsp[rd]).
        if (op & 0xFC) == 0xF0 || (op & 0xFC) == 0xF4 {
            let bit = (insn & 0x07) as u32;
            let ld = ((insn >> 8) & 0x03) as i32;
            let rs = ((insn >> 4) & 0x0F) as usize;
            let dsp = match ld {
                0 => 0,
                1 => self.fetch8() as i8 as i32,
                2 => self.fetch16() as i16 as i32,
                _ => self.fetch16() as i16 as i32,
            };
            let addr = self.mem_addr(self.regs[rs], dsp);
            let v = self.read_u8(addr);
            let mask = 1u8 << (bit & 0x07);
            let is_tst = (op & 0xFC) == 0xF4;
            if is_tst {
                self.psw.z = (v & mask) == 0;
            } else {
                let clear = (insn & 0x80) != 0;
                let nv = if clear { v & !mask } else { v | mask };
                self.write_u8(addr, nv);
                self.set_nz(nv as u32);
            }
            return;
        }
        // 1111 01.. = PUSH dsp[rs] (bits 3-2 = 10); otherwise BTST #imm, dsp[rd].
        if (op & 0xFC) == 0xF4 && (insn & 0x0C) == 0x04 {
            let ld = ((insn >> 8) & 0x03) as i32;
            let rs = ((insn >> 4) & 0x0F) as usize;
            let sz = (insn & 0x03) as u8;
            let dsp = match ld {
                0 => 0,
                1 => self.fetch8() as i8 as i32,
                _ => self.fetch16() as i16 as i32,
            };
            let addr = self.mem_addr(self.regs[rs], dsp);
            let v = self.load_u(addr, sz);
            self.push(v);
            return;
        }
        // 1111 10.. = MOV [ri+rb], rd / MOV rd, [ri+rb] (register-indexed)
        if (op & 0xFC) == 0xF8 || (op & 0xFC) == 0xFA {
            let to_reg = (op & 0x04) == 0;
            let sz = ((insn >> 12) & 0x03) as u8;
            let ri = (insn & 0x0F) as usize;
            let rb = ((insn >> 4) & 0x0F) as usize;
            let rd = ((insn >> 8) & 0x0F) as usize;
            let addr = self.regs[ri].wrapping_add(self.regs[rb]);
            if to_reg {
                let v = self.load(addr, sz);
                self.regs[rd] = v;
            } else {
                self.store(addr, self.regs[rd], sz);
            }
            return;
        }
        // 1111 1100 = register-register / memory forms (long opcodes)
        if op == 0xFC {
            self.execute_fc(insn);
            return;
        }
        // 1111 1101 = imm/long forms
        if op == 0xFD {
            self.execute_fd(insn);
            return;
        }
        // Unknown; halt.
        self.halted = true;
    }

    /// Binary operation dispatch (SUB/CMP/ADD/MUL/AND/OR) with memory or
    /// register source. `ld` selects the source addressing, `op` the operation.
    fn binop(&mut self, rs: usize, rd: usize, ld: u8, kind: u8) {
        let src = match ld {
            0 => self.regs[rs],
            1 => {
                let dsp = self.fetch8() as i8 as i32;
                let addr = self.mem_addr(self.regs[rs], dsp);
                self.load_u(addr, 0)
            }
            2 => {
                let dsp = self.fetch16() as i16 as i32;
                let addr = self.mem_addr(self.regs[rs], dsp);
                self.load_u(addr, 0)
            }
            _ => self.regs[rs],
        };
        let a = self.regs[rd];
        match kind {
            0 => {
                let r = self.sub_flags(a, src, 0);
                self.regs[rd] = r;
            }
            1 => {
                self.sub_flags(a, src, 0);
            }
            2 => {
                let r = self.add_flags(a, src, 0);
                self.regs[rd] = r;
            }
            3 => {
                let r = a.wrapping_mul(src);
                self.regs[rd] = r;
                self.set_nz(r);
            }
            4 => {
                let r = a & src;
                self.regs[rd] = r;
                self.set_nz(r);
            }
            _ => {
                let r = a | src;
                self.regs[rd] = r;
                self.set_nz(r);
            }
        }
    }

    /// 1111 1100 opcodes: register-register arithmetic and bit ops. The
    /// second byte (`b1`) is the operation selector and the third byte (`b2`)
    /// carries the register fields, matching the binutils decoder table. Only
    /// the register-addressed forms (ss/sd == 3) are modeled; memory-addressed
    /// and floating-point/DSP variants halt as unsupported.
    fn execute_fc(&mut self, insn: u16) {
        let b1 = (insn & 0xFF) as u8;
        let b2 = self.fetch8();
        match b1 {
            0x03 => {
                // SBB rs, rd
                let rs = ((b2 >> 4) & 0x0F) as usize;
                let rd = (b2 & 0x0F) as usize;
                let r = self.sub_flags(self.regs[rd], self.regs[rs], self.psw.c as u32);
                self.regs[rd] = r;
            }
            0x07 => {
                // NEG rd
                let rd = (b2 & 0x0F) as usize;
                let r = self.sub_flags(0, self.regs[rd], 0);
                self.regs[rd] = r;
            }
            0x0B => {
                // ADC rs, rd
                let rs = ((b2 >> 4) & 0x0F) as usize;
                let rd = (b2 & 0x0F) as usize;
                let r = self.add_flags(self.regs[rd], self.regs[rs], self.psw.c as u32);
                self.regs[rd] = r;
            }
            0x0F => {
                // ABS rd
                let rd = (b2 & 0x0F) as usize;
                let v = self.regs[rd];
                self.regs[rd] = if (v as i32) < 0 { self.sub_flags(0, v, 0) } else { v };
            }
            0x13 | 0x17 | 0x1B | 0x1F | 0x23 | 0x27 | 0x33 | 0x37 => {
                // MAX/MIN/EMUL/EMULU/DIV/DIVU/TST/XOR with a register source
                // (ss == 3). Operand byte is rsrc:rdst.
                let rs = ((b2 >> 4) & 0x0F) as usize;
                let rd = (b2 & 0x0F) as usize;
                self.fc_alu_reg(b1, rs, rd);
            }
            0x3B => {
                // NOT rd
                let rd = (b2 & 0x0F) as usize;
                let r = !self.regs[rd];
                self.regs[rd] = r;
                self.set_nz(r);
            }
            0x43 => {
                // XCHG rs, rd
                let rs = ((b2 >> 4) & 0x0F) as usize;
                let rd = (b2 & 0x0F) as usize;
                self.regs.swap(rs, rd);
            }
            0x4B => {
                // STZ rs, rd
                let rs = ((b2 >> 4) & 0x0F) as usize;
                let rd = (b2 & 0x0F) as usize;
                if self.psw.z {
                    self.regs[rd] = self.regs[rs];
                }
            }
            0x4F => {
                // STNZ rs, rd
                let rs = ((b2 >> 4) & 0x0F) as usize;
                let rd = (b2 & 0x0F) as usize;
                if !self.psw.z {
                    self.regs[rd] = self.regs[rs];
                }
            }
            0x63 | 0x67 | 0x6B | 0x6F => {
                // BSET/BCLR/BTST/BNOT rs, rd (sd == 3). Operand byte is rdst:rsrc.
                let rd = ((b2 >> 4) & 0x0F) as usize;
                let rs = (b2 & 0x0F) as usize;
                self.fc_bit_reg(b1, rs, rd);
            }
            _ if (b1 & 0xE0) == 0xE0 && (b1 & 0x03) == 3 => {
                // BM #imm, rd / BNOT #imm, rd (sd == 3). Operand byte is rdst:cond.
                let bit = ((b1 >> 2) & 0x07) as u32;
                let rd = ((b2 >> 4) & 0x0F) as usize;
                let cond = b2 & 0x0F;
                if cond == 0x0F {
                    self.regs[rd] ^= 1u32 << bit;
                } else if self.cond_true(cond) {
                    self.regs[rd] |= 1u32 << bit;
                } else {
                    self.regs[rd] &= !(1u32 << bit);
                }
            }
            0xD0..=0xDF if (b1 & 0x03) == 3 => {
                // SC cond, rd (store condition, register destination).
                let rd = ((b2 >> 4) & 0x0F) as usize;
                let cond = b2 & 0x0F;
                self.regs[rd] = if self.cond_true(cond) { 1 } else { 0 };
            }
            _ => {
                self.halted = true;
            }
        }
    }

    /// Execute a register-source ALU op from the 0xFC `00ss`/`01ss` families.
    fn fc_alu_reg(&mut self, b1: u8, rs: usize, rd: usize) {
        let a = self.regs[rd];
        let b = self.regs[rs];
        match b1 {
            0x13 => {
                // MAX rs, rd
                let r = if (a as i32) < (b as i32) { b } else { a };
                self.regs[rd] = r;
                self.set_nz(r);
            }
            0x17 => {
                // MIN rs, rd
                let r = if (a as i32) > (b as i32) { b } else { a };
                self.regs[rd] = r;
                self.set_nz(r);
            }
            0x1B => {
                // EMUL rs, rd (signed 32x32 -> 32)
                let r = ((a as i32 as i64).wrapping_mul(b as i32 as i64)) as u32;
                self.regs[rd] = r;
                self.set_nz(r);
            }
            0x1F => {
                // EMULU rs, rd
                let r = (a as u64).wrapping_mul(b as u64) as u32;
                self.regs[rd] = r;
                self.set_nz(r);
            }
            0x23 => {
                // DIV rs, rd (signed)
                if b != 0 {
                    let r = (a as i32).wrapping_div(b as i32) as u32;
                    self.regs[rd] = r;
                    self.set_nz(r);
                }
            }
            0x27 => {
                // DIVU rs, rd
                if b != 0 {
                    let r = a.wrapping_div(b);
                    self.regs[rd] = r;
                    self.set_nz(r);
                }
            }
            0x33 => {
                // TST rs, rd
                self.set_nz(a & b);
            }
            0x37 => {
                // XOR rs, rd
                let r = a ^ b;
                self.regs[rd] = r;
                self.set_nz(r);
            }
            _ => self.halted = true,
        }
    }

    /// Execute a register bit op from the 0xFC `0110` family (sd == 3).
    fn fc_bit_reg(&mut self, b1: u8, rs: usize, rd: usize) {
        let v = self.regs[rd];
        let bit = self.regs[rs] & 0x1F;
        match b1 {
            0x63 => self.regs[rd] = v | (1u32 << bit),   // BSET
            0x67 => self.regs[rd] = v & !(1u32 << bit),  // BCLR
            0x6B => self.psw.z = (v & (1u32 << bit)) == 0, // BTST
            0x6F => self.regs[rd] = v ^ (1u32 << bit),   // BNOT
            _ => self.halted = true,
        }
    }

    /// Read the sign-extended immediate selected by the `im` field (0=imm32,
    /// 1=imm8, 2=imm16, 3=imm24), matching the binutils `immediate` helper.
    fn read_imm_im(&mut self, im: u8) -> u32 {
        match im {
            0 => {
                let b0 = self.fetch8() as u32;
                let b1 = self.fetch8() as u32;
                let b2 = self.fetch8() as u32;
                let b3 = self.fetch8() as u32;
                b0 | (b1 << 8) | (b2 << 16) | ((b3 as u8 as i8 as i32 as u32) << 24)
            }
            1 => self.fetch8() as i8 as i32 as u32,
            2 => self.fetch16() as i16 as i32 as u32,
            _ => {
                let b0 = self.fetch8() as u32;
                let b1 = self.fetch8() as u32;
                let b2 = self.fetch8() as u32;
                let v = b0 | (b1 << 8) | (b2 << 16);
                if v & 0x800000 != 0 { v | 0xFF00_0000 } else { v }
            }
        }
    }

    /// 1111 1101 opcodes: immediate/long forms (li_10, MVFC/MVTC, shifts, etc.).
    fn execute_fd(&mut self, insn: u16) {
        let b1 = (insn & 0xFF) as u8;
        let b2 = self.fetch8();
        let rd = (b2 & 0x0F) as usize;
        let rs = ((b2 >> 4) & 0x0F) as usize;
        match b1 {
            0x20..=0x2E if b1 != 0x27 => {
                // MOV sz rsrc, rdst (register-register). For 0x20-0x27 the
                // operand byte is rdst:rsrc; for 0x28-0x2E it is rsrc:rdst.
                let sz = b1 & 0x03;
                let (src, dst) = if b1 < 0x28 {
                    ((b2 & 0x0F) as usize, ((b2 >> 4) & 0x0F) as usize)
                } else {
                    (((b2 >> 4) & 0x0F) as usize, (b2 & 0x0F) as usize)
                };
                let v = self.regs[src];
                self.regs[dst] = match sz {
                    0 => v as u8 as i8 as i32 as u32,
                    1 => v as u16 as i16 as i32 as u32,
                    _ => v,
                };
            }
            0x60 | 0x61 | 0x62 => {
                // SHLR/SHAR/SHLL rs, rd (register shift count).
                let n = self.regs[rs] & 0x1F;
                let v = self.regs[rd];
                let r = match b1 {
                    0x60 => v >> n,
                    0x61 => ((v as i32) >> n) as u32,
                    _ => v.wrapping_shl(n),
                };
                self.regs[rd] = r;
                self.set_nz(r);
            }
            0x64 | 0x66 => {
                // ROTR/ROTL rs, rd (register rotate count).
                let n = self.regs[rs] & 0x1F;
                let v = self.regs[rd];
                let r = if b1 == 0x64 {
                    v.rotate_right(n)
                } else {
                    v.rotate_left(n)
                };
                self.regs[rd] = r;
                self.set_nz(r);
            }
            0x65 => {
                // REVW rs, rd
                self.regs[rd] = self.regs[rs].swap_bytes();
            }
            0x67 => {
                // REVL rs, rd
                let b = self.regs[rs].to_le_bytes();
                self.regs[rd] = u32::from_le_bytes([b[3], b[2], b[1], b[0]]);
            }
            0x68 | 0x69 => {
                // MVTC rs, cr: cr = (b1 & 1)*16 + (b2 & 0x0F); CR0 is the PSW.
                let cr = ((b1 as u32 & 0x01) << 4) | (b2 & 0x0F) as u32;
                if cr == 0 {
                    self.psw = Psw::from_u32(self.regs[rs]);
                }
            }
            0x6A | 0x6B => {
                // MVFC cr, rd: cr = (b1 & 1)*16 + (b2 >> 4); CR0 is the PSW.
                let cr = ((b1 as u32 & 0x01) << 4) | ((b2 >> 4) & 0x0F) as u32;
                self.regs[rd] = if cr == 0 {
                    self.psw.to_u32()
                } else {
                    0 // Unmodeled control register reads as zero.
                };
            }
            0x6C | 0x6D => {
                // ROTR #imm, rd (i = b1 & 1, imm = (b2 >> 4) & 0x0F).
                let imm = (((b2 >> 4) & 0x0F) as u32) | ((b1 as u32 & 0x01) << 4);
                let r = self.regs[rd].rotate_right(imm);
                self.regs[rd] = r;
                self.set_nz(r);
            }
            0x6E | 0x6F => {
                // ROTL #imm, rd
                let imm = (((b2 >> 4) & 0x0F) as u32) | ((b1 as u32 & 0x01) << 4);
                let r = self.regs[rd].rotate_left(imm);
                self.regs[rd] = r;
                self.set_nz(r);
            }
            0x70 | 0x74 | 0x78 | 0x7C => {
                // ADC/MAX/MIN/EMULU/DIVU/TST/XOR/STZ #imm, rd. The high nibble
                // of b2 selects the operation; `im` selects the immediate width.
                let op = b2 & 0xF0;
                let im = (b1 >> 2) & 0x03;
                let rd = (b2 & 0x0F) as usize;
                let imm = self.read_imm_im(im);
                self.fd_imm_alu(op, rd, imm);
            }
            0x73 | 0x77 | 0x7B | 0x7F => {
                // MVTC #imm, cr (crdst = b2 & 0x1F; CR0 is the PSW).
                let cr = b2 & 0x1F;
                let im = (b1 >> 2) & 0x03;
                let imm = self.read_imm_im(im);
                if cr == 0 {
                    self.psw = Psw::from_u32(imm);
                }
            }
            0x80..=0x9F => {
                // SHLR #imm, rs, rd
                let imm = (b1 & 0x1F) as u32;
                let r = self.regs[rs] >> imm;
                self.regs[rd] = r;
                self.set_nz(r);
            }
            0xA0..=0xBF => {
                // SHAR #imm, rs, rd
                let imm = (b1 & 0x1F) as u32;
                let r = ((self.regs[rs] as i32) >> imm) as u32;
                self.regs[rd] = r;
                self.set_nz(r);
            }
            0xC0..=0xDF => {
                // SHLL #imm, rs, rd
                let imm = (b1 & 0x1F) as u32;
                let r = self.regs[rs].wrapping_shl(imm);
                self.regs[rd] = r;
                self.set_nz(r);
            }
            0xE0..=0xFF => {
                // BM #imm, rd / BNOT #imm, rd (cond = b2 >> 4, rd = b2 & 0x0F).
                let bit = (b1 & 0x1F) as u32;
                let cond = (b2 >> 4) & 0x0F;
                let rd = (b2 & 0x0F) as usize;
                if cond == 0x0F {
                    self.regs[rd] ^= 1u32 << bit;
                } else if self.cond_true(cond) {
                    self.regs[rd] |= 1u32 << bit;
                } else {
                    self.regs[rd] &= !(1u32 << bit);
                }
            }
            _ => {
                self.halted = true;
            }
        }
    }

    /// Execute an immediate ALU op from the 0xFD `0111 im00` family. `op` is
    /// the high nibble of the operand byte (0x2=adc, 0x4=max, 0x5=min, 0x7=emulu,
    /// 0x9=divu, 0xC=tst, 0xD=xor, 0xE=stz).
    fn fd_imm_alu(&mut self, op: u8, rd: usize, imm: u32) {
        let v = self.regs[rd];
        match op {
            0x20 => {
                // ADC #imm, rd
                self.regs[rd] = self.add_flags(v, imm, self.psw.c as u32);
            }
            0x40 => {
                // MAX #imm, rd
                let r = if (v as i32) < (imm as i32) { imm } else { v };
                self.regs[rd] = r;
                self.set_nz(r);
            }
            0x50 => {
                // MIN #imm, rd
                let r = if (v as i32) > (imm as i32) { imm } else { v };
                self.regs[rd] = r;
                self.set_nz(r);
            }
            0x70 => {
                // EMULU #imm, rd
                let r = (v as u64).wrapping_mul(imm as u64) as u32;
                self.regs[rd] = r;
                self.set_nz(r);
            }
            0x90 => {
                // DIVU #imm, rd
                if imm != 0 {
                    let r = v.wrapping_div(imm);
                    self.regs[rd] = r;
                    self.set_nz(r);
                }
            }
            0xC0 => {
                // TST #imm, rd
                self.set_nz(v & imm);
            }
            0xD0 => {
                // XOR #imm, rd
                let r = v ^ imm;
                self.regs[rd] = r;
                self.set_nz(r);
            }
            0xE0 => {
                // STZ #imm, rd
                if self.psw.z {
                    self.regs[rd] = imm;
                }
            }
            _ => self.halted = true,
        }
    }
}

/// Sign-extend a 7-bit value.
fn sign7(value: u16) -> i32 {
    ((value & 0x7F) as i8) as i32
}

/// Number of code bytes consumed by the RX instruction at `pc`. Mirrors the
/// core's byte-fetch model: short branches are 1 byte (BRA.s) or 2 bytes
/// (BCnd.s), all other instructions start with a big-endian opcode word and
/// add one operand byte per immediate/displacement field the family fetches.
pub fn instruction_len(pc: u32, read: &dyn Fn(u32) -> u8) -> u32 {
    let b0 = read(pc) as u16;
    let b1 = read(pc + 1) as u16;
    let insn = (b0 << 8) | b1;
    let op = b0 as u8;
    let low = b1 as u8;

    // 1-byte short branch BRA.s (0x08..=0x0F) and 2-byte BCnd.s (0x10..=0x1F).
    if (0x08..=0x0F).contains(&b0) {
        return 1;
    }
    if (0x10..=0x1F).contains(&b0) {
        return 2;
    }

    // All remaining forms start with a 2-byte opcode word.
    let base = 2u32;

    // Special opcode words with 32-bit branch displacements.
    if insn == 0x0004 || insn == 0x0005 {
        return base + 4;
    }
    // Short branch encodings are unreachable here (handled above), but keep the
    // dispatch in the same order as the core for clarity.
    if (op & 0xF0) == 0x20 {
        return base;
    }
    // BRA.w / BSR.w: 16-bit displacement.
    if insn == 0x3800 || insn == 0x3900 {
        return base + 2;
    }
    // BCnd.w: 8-bit displacement.
    if (op & 0xF8) == 0x3A {
        return base + 1;
    }
    // MOV.b/w/l #imm8, dsp5[rd] and RTSD #imm, rd-rd2: one 8-bit immediate.
    if (op & 0xFC) == 0x3C || op == 0x3F {
        return base + 1;
    }
    // SUB/CMP/ADD/MUL/AND/OR dsp[rs], rd: source addressing selects the width.
    if matches!(op & 0xFC, 0x40 | 0x44 | 0x48 | 0x4C | 0x50 | 0x54) {
        return base + match op & 0x03 {
            1 => 1,
            2 => 2,
            _ => 0,
        };
    }
    // MOVU.b/w: register-indirect (0), dsp8 (1), dsp16/#imm16 (2/3).
    if (op & 0xF8) == 0x58 {
        return base + match op & 0x03 {
            1 => 1,
            2 | 3 => 2,
            _ => 0,
        };
    }
    // Immediate-always (imm in the opcode word): SUB/CMP/ADD/MUL #uimm4,
    // AND/OR/MOV #uimm4, SHLR/SHLL #imm5, BSET/BCLR/BTST #imm5.
    if matches!(op & 0xFC, 0x60 | 0x64 | 0x68 | 0x6C) || (0x78..=0x7D).contains(&op) {
        return base;
    }
    // PUSHM/POPM and the register-only 0x7E/0x7F families.
    if op == 0x6E || op == 0x6F || op == 0x7E || op == 0x7F {
        return base;
    }
    // ADD #imm, rs, rd and CMP/MUL/AND/OR #imm, rd: 16-bit immediate.
    if matches!(op & 0xFC, 0x70 | 0x74) {
        return base + 2;
    }
    // MOV.b/w/l store/load dsp5[reg] and MOVU dsp5: displacement in the word.
    if matches!(op & 0xF8, 0x80 | 0x88 | 0x90 | 0x98 | 0xA0 | 0xA8) || (op & 0xF0) == 0xB0 {
        return base;
    }
    // MOV.b/w/l dsp[lds], dsp[ldd]: sum of src/dst displacement widths.
    if (op & 0xE0) == 0xC0 || (op & 0xF0) == 0xE0 {
        let lds = ((low >> 4) & 0x03) as u32;
        let ldd = ((insn >> 8) & 0x03) as u32;
        let disp = |ld: u32| match ld {
            2 => 1,
            3 => 2,
            _ => 0,
        };
        return base + disp(lds) + disp(ldd);
    }
    // BSET/BCLR/BTST #imm, dsp[rd] and PUSH dsp[rs]: displacement width.
    if matches!(op & 0xFC, 0xF0 | 0xF4) {
        let ld = ((insn >> 8) & 0x03) as u32;
        return base + match ld {
            1 => 1,
            2 | 3 => 2,
            _ => 0,
        };
    }
    // MOV [ri+rb], rd / MOV rd, [ri+rb]: no extra operand.
    if (op & 0xFC) == 0xF8 || (op & 0xFC) == 0xFA {
        return base;
    }
    // 0xFC long opcodes: opcode word plus one operand byte (registers).
    if op == 0xFC {
        return base + 1;
    }
    // 0xFD long opcodes: opcode word plus one operand byte, except the
    // immediate-ALU/MVTC forms whose immediate width is selected by `im`.
    if op == 0xFD {
        let b1 = read(pc + 1) as u8;
        if matches!(b1, 0x70 | 0x74 | 0x78 | 0x7C | 0x73 | 0x77 | 0x7B | 0x7F) {
            let im = (b1 >> 2) & 0x03;
            let imm_bytes = match im {
                0 => 4,
                1 => 1,
                2 => 2,
                _ => 3,
            };
            return base + 1 + imm_bytes;
        }
        return base + 1;
    }
    // Unknown: the core fetches the opcode word then halts.
    base
}

/// Disassemble one RX instruction at `pc` using `read` for bytes.
pub fn disassemble(pc: u32, read: &dyn Fn(u32) -> u8) -> String {
    let b0 = read(pc) as u16;
    let b1 = read(pc + 1) as u16;
    // The core fetches a big-endian opcode word (`(b0 << 8) | b1`), so decode
    // the same way: `op` is the first byte and `low` the second.
    let insn = (b0 << 8) | b1;
    let op = ((insn >> 8) & 0xFF) as u8;
    let low = (insn & 0xFF) as u8;

    if insn == 0x0000 { return "brk".to_string(); }
    if insn == 0x0002 { return "rts".to_string(); }
    if insn == 0x0003 { return "nop".to_string(); }
    if insn == 0x7996 { return "wait".to_string(); }
    if insn == 0x0004 || insn == 0x0005 {
        let dsp = (((read(pc + 2) as u32) << 24)
            | ((read(pc + 3) as u32) << 16)
            | ((read(pc + 4) as u32) << 8)
            | read(pc + 5) as u32) as i32;
        return if insn == 0x0004 {
            format!("bra.a {:+}", dsp)
        } else {
            format!("bsr.a {:+}", dsp)
        };
    }
    if (insn & 0xF800) == 0x0800 {
        // BRA.s is a 1-byte instruction: displacement is the low 7 bits of the
        // first byte.
        let dsp = sign7(b0 & 0x7F);
        return format!("bra.s {:+}", dsp);
    }
    if (op & 0xF0) == 0x10 {
        // BCnd.s is a 2-byte instruction: condition is the low nibble of the
        // first byte, displacement spans bits 5-3 of the first byte and the low
        // 7 bits of the second byte.
        let cond = (b0 & 0x0F) as u8;
        let dsp = sign7(((b0 & 0x07) << 3) | (b1 & 0x7F));
        return format!("b{}.s {:+}", cond_name(cond), dsp);
    }
    if (op & 0xF0) == 0x20 {
        if insn == 0x2E00 {
            let dsp = low as i8;
            return format!("bra.b {:+}", dsp);
        }
        let cond = op & 0x0F;
        let dsp = low as i8;
        return format!("b{}.b {:+}", cond_name(cond), dsp);
    }
    if insn == 0x3800 {
        let dsp = ((read(pc + 2) as i16) << 8) | read(pc + 3) as i16;
        return format!("bra.w {:+}", dsp);
    }
    if insn == 0x3900 {
        let dsp = ((read(pc + 2) as i16) << 8) | read(pc + 3) as i16;
        return format!("bsr.w {:+}", dsp);
    }
    if (op & 0xF8) == 0x3A {
        let cond = (op & 0x07) as u8;
        let dsp = (low as i16 | ((read(pc + 2) as i16) << 8)) as i32;
        return format!("b{}.w {:+}", cond_name(cond), dsp);
    }
    if (op & 0xFC) == 0x3C {
        let sz = op & 0x03;
        let rd = ((insn >> 8) & 0x07) as usize;
        let imm = read(pc + 2);
        let dsp = ((low & 0x1E) as i32) << 0;
        return format!("mov.{} #0x{:02x}, {}[r{}]", sz_name(sz), imm, dsp, rd);
    }
    if op == 0x3F {
        let rd = ((insn >> 4) & 0x0F) as usize;
        let rd2 = ((insn >> 8) & 0x0F) as usize;
        let imm = read(pc + 2);
        return format!("rtsd #0x{:02x}, r{}-r{}", imm, rd, rd2);
    }
    if (op & 0xE0) == 0x80 || (op & 0xE0) == 0xA0 {
        let sz = ((op >> 5) & 0x03) as u8;
        let rd = ((insn >> 8) & 0x07) as usize;
        let rs = ((low >> 4) & 0x07) as usize;
        let dsp = ((insn >> 3) & 0x1E) as i32;
        return format!("mov.{} r{}, {}[r{}]", sz_name(sz), rs, dsp, rd);
    }
    if (op & 0xE0) == 0x90 || (op & 0xE0) == 0xB0 {
        let sz = ((op >> 5) & 0x03) as u8;
        let rs = ((insn >> 8) & 0x07) as usize;
        let rd = ((low >> 4) & 0x07) as usize;
        let dsp = ((insn >> 3) & 0x1E) as i32;
        return format!("mov.{} {}[r{}], r{}", sz_name(sz), dsp, rs, rd);
    }
    if (op & 0xE0) == 0xB8 {
        let sz = ((op >> 5) & 0x01) as u8;
        let rs = ((insn >> 4) & 0x07) as usize;
        let rd = ((low >> 4) & 0x07) as usize;
        let dsp = ((insn >> 1) & 0x1E) as i32;
        return format!("movu.{} {}[r{}], r{}", sz_name(sz), dsp, rs, rd);
    }
    // SUB/CMP/ADD/MUL/AND/OR dsp[rs], rd (register, dsp8 or dsp16 source).
    if matches!(op & 0xFC, 0x40 | 0x44 | 0x48 | 0x4C | 0x50 | 0x54) {
        let name = match op & 0xFC {
            0x40 => "sub",
            0x44 => "cmp",
            0x48 => "add",
            0x4C => "mul",
            0x50 => "and",
            _ => "or",
        };
        let ld = op & 0x03;
        let rs = ((low >> 4) & 0x0F) as usize;
        let rd = (low & 0x0F) as usize;
        return match ld {
            1 => format!("{} {:+}[r{}], r{}", name, read(pc + 2) as i8, rs, rd),
            2 => {
                let dsp = ((read(pc + 2) as i16) << 8) | read(pc + 3) as i16;
                format!("{} {:+}[r{}], r{}", name, dsp, rs, rd)
            }
            _ => format!("{} r{}, r{}", name, rs, rd),
        };
    }
    // MOVU.b/w: register, dsp8, dsp16 or #imm16 source.
    if (op & 0xF8) == 0x58 {
        let sz = ((op >> 2) & 0x01) as u8;
        let kind = op & 0x03;
        let rs = ((low >> 4) & 0x0F) as usize;
        let rd = (low & 0x0F) as usize;
        return match kind {
            0 => format!("movu.{} r{}, r{}", sz_name(sz), rs, rd),
            1 => format!("movu.{} {:+}[r{}], r{}", sz_name(sz), read(pc + 2) as i8, rs, rd),
            2 => {
                let dsp = ((read(pc + 2) as i16) << 8) | read(pc + 3) as i16;
                format!("movu.{} {:+}[r{}], r{}", sz_name(sz), dsp, rs, rd)
            }
            _ => {
                let imm = ((read(pc + 2) as u16) << 8) | read(pc + 3) as u16;
                format!("movu.{} #0x{:04x}, r{}", sz_name(sz), imm, rd)
            }
        };
    }
    if (op & 0xFC) == 0x60 {
        let sub = (op >> 2) & 0x03;
        let imm = ((insn >> 4) & 0x0F) as u32;
        let rd = (low & 0x0F) as usize;
        let m = match sub { 0 => "sub", 1 => "cmp", 2 => "add", _ => "mul" };
        return format!("{} #0x{:x}, r{}", m, imm, rd);
    }
    if (op & 0xFC) == 0x64 {
        let kind = (op >> 2) & 0x03;
        let imm = ((insn >> 4) & 0x0F) as u32;
        let rd = (low & 0x0F) as usize;
        let m = match kind { 0 => "and", 1 => "or", _ => "mov" };
        return format!("{} #0x{:x}, r{}", m, imm, rd);
    }
    if (op & 0xFC) == 0x68 || (op & 0xFC) == 0x6C {
        let left = (op & 0x04) != 0;
        let imm = ((insn >> 8) & 0x1F) as u32;
        let rd = (low & 0x0F) as usize;
        let m = if left { "shll" } else { "shlr" };
        return format!("{} #0x{:x}, r{}", m, imm, rd);
    }
    if op == 0x6E {
        let rs = ((insn >> 4) & 0x0F) as usize;
        let rs2 = (low & 0x0F) as usize;
        return format!("pushm r{}-r{}", rs, rs2);
    }
    if op == 0x6F {
        let rd = ((insn >> 4) & 0x0F) as usize;
        let rd2 = (low & 0x0F) as usize;
        return format!("popm r{}-r{}", rd, rd2);
    }
    if (op & 0xFC) == 0x70 {
        let rs = ((insn >> 4) & 0x0F) as usize;
        let rd = (low & 0x0F) as usize;
        let imm = ((read(pc + 2) as u16) << 8) | read(pc + 3) as u16;
        return format!("add #0x{:04x}, r{}, r{}", imm, rs, rd);
    }
    if (op & 0xFC) == 0x74 {
        let kind = op & 0x03;
        let rd = (low & 0x0F) as usize;
        let m = match kind { 0 => "cmp", 1 => "mul", 2 => "and", _ => "or" };
        return format!("{} #0x{:04x}, r{}", m, ((read(pc + 2) as u16) << 8) | read(pc + 3) as u16, rd);
    }
    if (op & 0xFE) == 0x78 {
        let kind = (op >> 1) & 0x03;
        let imm = ((insn >> 8) & 0x1F) as u32;
        let rd = (low & 0x0F) as usize;
        let m = match kind { 0 => "bset", 1 => "bclr", 2 => "btst", _ => "bset" };
        return format!("{} #0x{:x}, r{}", m, imm, rd);
    }
    if (op & 0xFC) == 0x7C {
        let kind = op & 0x03;
        let rd = (low & 0x0F) as usize;
        let m = match kind { 0 => "not", 1 => "neg", 2 => "abs", _ => "pushc" };
        return format!("{} r{}", m, rd);
    }
    if (op & 0xFE) == 0x7E {
        let kind = op & 0x0F;
        let rs = (low & 0x0F) as usize;
        let m = match kind { 0 => "jmp", 1 => "jsr", 4 => "bra.l", 5 => "bsr.l", _ => "?" };
        return format!("{} r{}", m, rs);
    }
    // MOV.b/w/l dsp[lds], dsp[ldd].
    if (op & 0xE0) == 0xC0 || (op & 0xF0) == 0xE0 {
        let sz = ((op >> 5) & 0x03) as u8;
        let ldd = (b0 & 0x03) as usize;
        let lds = ((low >> 4) & 0x03) as usize;
        let rd = ((b0 >> 4) & 0x0F) as usize;
        let rs = (low & 0x0F) as usize;
        let mut p = pc + 2;
        let src = match lds {
            2 => {
                let d = read(p) as i8;
                p += 1;
                format!("{:+}[r{}]", d, rs)
            }
            3 => {
                let d = ((read(p) as i16) << 8) | read(p + 1) as i16;
                p += 2;
                format!("{:+}[r{}]", d, rs)
            }
            _ => format!("[r{}]", rs),
        };
        let dst = match ldd {
            2 => format!("{:+}[r{}]", read(p) as i8, rd),
            3 => format!("{:+}[r{}]", ((read(p) as i16) << 8) | read(p + 1) as i16, rd),
            _ => format!("[r{}]", rd),
        };
        return format!("mov.{} {}, {}", sz_name(sz), src, dst);
    }
    // BSET/BCLR/BTST #imm, dsp[rd].
    if matches!(op & 0xFC, 0xF0 | 0xF4) {
        let bit = (low & 0x07) as u8;
        let ld = (b0 & 0x03) as usize;
        let rs = ((low >> 4) & 0x0F) as usize;
        let is_tst = (op & 0xFC) == 0xF4;
        let name = if is_tst {
            "btst"
        } else if (low & 0x80) != 0 {
            "bclr"
        } else {
            "bset"
        };
        let dsp = match ld {
            1 => format!("{:+}[r{}]", read(pc + 2) as i8, rs),
            2 | 3 => format!("{:+}[r{}]", ((read(pc + 2) as i16) << 8) | read(pc + 3) as i16, rs),
            _ => format!("[r{}]", rs),
        };
        return format!("{} #{}, {}", name, bit, dsp);
    }
    // MOV.b/w/l [ri+rb], rd / MOV.b/w/l rd, [ri+rb].
    if matches!(op & 0xFC, 0xF8 | 0xFA) {
        let to_reg = (op & 0x04) == 0;
        let sz = ((b0 >> 4) & 0x03) as u8;
        let ri = (low & 0x0F) as usize;
        let rb = ((low >> 4) & 0x0F) as usize;
        let rd = (b0 & 0x0F) as usize;
        let mem = format!("[r{}+r{}]", ri, rb);
        return if to_reg {
            format!("mov.{} {}, r{}", sz_name(sz), mem, rd)
        } else {
            format!("mov.{} r{}, {}", sz_name(sz), rd, mem)
        };
    }
    // 0xFC long opcodes: register-register arithmetic/bit ops.
    if op == 0xFC {
        return disasm_fc(pc, read, low);
    }
    // 0xFD long opcodes: immediate/register forms.
    if op == 0xFD {
        return disasm_fd(pc, read, low);
    }
    format!("0x{:04x}", insn)
}

/// Read the sign-extended immediate selected by the `im` field (0=imm32,
/// 1=imm8, 2=imm16, 3=imm24) for the 0xFD immediate-ALU forms.
fn read_imm_im_static(pc: u32, read: &dyn Fn(u32) -> u8, im: u8) -> u32 {
    match im {
        0 => {
            let b0 = read(pc) as u32;
            let b1 = read(pc + 1) as u32;
            let b2 = read(pc + 2) as u32;
            let b3 = read(pc + 3) as u32;
            b0 | (b1 << 8) | (b2 << 16) | ((b3 as u8 as i8 as i32 as u32) << 24)
        }
        1 => read(pc) as i8 as i32 as u32,
        2 => {
            let w = (read(pc) as u16) | ((read(pc + 1) as u16) << 8);
            w as i16 as i32 as u32
        }
        _ => {
            let b0 = read(pc) as u32;
            let b1 = read(pc + 1) as u32;
            let b2 = read(pc + 2) as u32;
            let v = b0 | (b1 << 8) | (b2 << 16);
            if v & 0x800000 != 0 { v | 0xFF00_0000 } else { v }
        }
    }
}

/// Control-register name for the RX `mvtc`/`mvfc` control register number.
fn cr_name(cr: u32) -> &'static str {
    match cr {
        0 => "psw",
        1 => "pc",
        2 => "usp",
        3 => "fpsw",
        8 => "bpsw",
        9 => "bpc",
        10 => "isp",
        11 => "fintv",
        12 => "intb",
        13 => "extb",
        16 => "a0",
        17 => "a1",
        _ => "cr",
    }
}

/// Disassemble a 0xFC long opcode (register-register arithmetic/bit ops).
fn disasm_fc(pc: u32, read: &dyn Fn(u32) -> u8, b1: u8) -> String {
    let b2 = read(pc + 2);
    let rs = ((b2 >> 4) & 0x0F) as usize;
    let rd = (b2 & 0x0F) as usize;
    match b1 {
        0x03 => format!("sbb r{}, r{}", rs, rd),
        0x07 => format!("neg r{}", rd),
        0x0B => format!("adc r{}, r{}", rs, rd),
        0x0F => format!("abs r{}", rd),
        0x13 => format!("max r{}, r{}", rs, rd),
        0x17 => format!("min r{}, r{}", rs, rd),
        0x1B => format!("emul r{}, r{}", rs, rd),
        0x1F => format!("emulu r{}, r{}", rs, rd),
        0x23 => format!("div r{}, r{}", rs, rd),
        0x27 => format!("divu r{}, r{}", rs, rd),
        0x33 => format!("tst r{}, r{}", rs, rd),
        0x37 => format!("xor r{}, r{}", rs, rd),
        0x3B => format!("not r{}", rd),
        0x43 => format!("xchg r{}, r{}", rs, rd),
        0x4B => format!("stz r{}, r{}", rs, rd),
        0x4F => format!("stnz r{}, r{}", rs, rd),
        0x63 | 0x67 | 0x6B | 0x6F => {
            // op[2] = rdst:rsrc for the register bit ops.
            let rd = ((b2 >> 4) & 0x0F) as usize;
            let rs = (b2 & 0x0F) as usize;
            let n = match b1 {
                0x63 => "bset",
                0x67 => "bclr",
                0x6B => "btst",
                _ => "bnot",
            };
            format!("{} r{}, r{}", n, rs, rd)
        }
        0xD0..=0xDF if (b1 & 0x03) == 3 => {
            let sz = (b1 >> 2) & 0x03;
            let rd = ((b2 >> 4) & 0x0F) as usize;
            let cond = b2 & 0x0F;
            format!("sc{}.{} r{}", cond_name(cond), sz_name(sz), rd)
        }
        _ if (b1 & 0xE0) == 0xE0 && (b1 & 0x03) == 3 => {
            let bit = (b1 >> 2) & 0x07;
            let rd = ((b2 >> 4) & 0x0F) as usize;
            let cond = b2 & 0x0F;
            if cond == 0x0F {
                format!("bnot #{}, r{}", bit, rd)
            } else {
                format!("bm{} #{}, r{}", cond_name(cond), bit, rd)
            }
        }
        _ => format!("0x{:04x}", (0xFCu16 << 8) | b1 as u16),
    }
}

/// Disassemble a 0xFD long opcode (immediate/register forms).
fn disasm_fd(pc: u32, read: &dyn Fn(u32) -> u8, b1: u8) -> String {
    let b2 = read(pc + 2);
    let rs = ((b2 >> 4) & 0x0F) as usize;
    let rd = (b2 & 0x0F) as usize;
    match b1 {
        0x20..=0x2E if b1 != 0x27 => {
            let sz = b1 & 0x03;
            let (src, dst) = if b1 < 0x28 {
                ((b2 & 0x0F) as usize, ((b2 >> 4) & 0x0F) as usize)
            } else {
                (((b2 >> 4) & 0x0F) as usize, (b2 & 0x0F) as usize)
            };
            format!("mov.{} r{}, r{}", sz_name(sz), src, dst)
        }
        0x60 => format!("shlr r{}, r{}", rs, rd),
        0x61 => format!("shar r{}, r{}", rs, rd),
        0x62 => format!("shll r{}, r{}", rs, rd),
        0x64 => format!("rotr r{}, r{}", rs, rd),
        0x65 => format!("revw r{}, r{}", rs, rd),
        0x66 => format!("rotl r{}, r{}", rs, rd),
        0x67 => format!("revl r{}, r{}", rs, rd),
        0x68 | 0x69 => {
            let cr = ((b1 as u32 & 0x01) << 4) | (b2 & 0x0F) as u32;
            format!("mvtc r{}, {}", rs, cr_name(cr))
        }
        0x6A | 0x6B => {
            let cr = ((b1 as u32 & 0x01) << 4) | ((b2 >> 4) & 0x0F) as u32;
            format!("mvfc {}, r{}", cr_name(cr), rd)
        }
        0x6C | 0x6D => {
            let imm = ((b2 >> 4) & 0x0F) | ((b1 & 0x01) << 4);
            format!("rotr #{}, r{}", imm, rd)
        }
        0x6E | 0x6F => {
            let imm = ((b2 >> 4) & 0x0F) | ((b1 & 0x01) << 4);
            format!("rotl #{}, r{}", imm, rd)
        }
        0x70 | 0x74 | 0x78 | 0x7C => {
            let op = b2 & 0xF0;
            let im = (b1 >> 2) & 0x03;
            let rd = (b2 & 0x0F) as usize;
            let imm = read_imm_im_static(pc + 3, read, im);
            let n = match op {
                0x20 => "adc",
                0x40 => "max",
                0x50 => "min",
                0x70 => "emulu",
                0x90 => "divu",
                0xC0 => "tst",
                0xD0 => "xor",
                0xE0 => "stz",
                _ => "?",
            };
            format!("{} #0x{:x}, r{}", n, imm, rd)
        }
        0x73 | 0x77 | 0x7B | 0x7F => {
            let cr = (b2 & 0x1F) as u32;
            let im = (b1 >> 2) & 0x03;
            let imm = read_imm_im_static(pc + 3, read, im);
            format!("mvtc #0x{:x}, {}", imm, cr_name(cr))
        }
        0x80..=0x9F => {
            let imm = b1 & 0x1F;
            format!("shlr #{}, r{}, r{}", imm, rs, rd)
        }
        0xA0..=0xBF => {
            let imm = b1 & 0x1F;
            format!("shar #{}, r{}, r{}", imm, rs, rd)
        }
        0xC0..=0xDF => {
            let imm = b1 & 0x1F;
            format!("shll #{}, r{}, r{}", imm, rs, rd)
        }
        0xE0..=0xFF => {
            let bit = b1 & 0x1F;
            let cond = (b2 >> 4) & 0x0F;
            let rd = (b2 & 0x0F) as usize;
            if cond == 0x0F {
                format!("bnot #{}, r{}", bit, rd)
            } else {
                format!("bm{} #{}, r{}", cond_name(cond), bit, rd)
            }
        }
        _ => format!("0x{:04x}", (0xFDu16 << 8) | b1 as u16),
    }
}

fn cond_name(cond: u8) -> &'static str {
    match cond & 0x0F {
        0x0 => "eq",
        0x1 => "ne",
        0x2 => "c",
        0x3 => "nc",
        0x4 => "n",
        0x5 => "p",
        0x6 => "o",
        0x7 => "no",
        0x8 => "le",
        0x9 => "lt",
        0xA => "gt",
        0xB => "ge",
        0xC => "hi",
        0xD => "ls",
        0xE => "f",
        _ => "t",
    }
}

fn sz_name(sz: u8) -> &'static str {
    match sz {
        0 => "b",
        1 => "w",
        _ => "l",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core_with(code: &[u8]) -> RxCore {
        let mut c = RxCore::new();
        c.write_bytes(0x1000, code);
        c.pc = 0x1000;
        c
    }

    #[test]
    fn nop_advances() {
        // NOP = 0x0003.
        let mut c = core_with(&[0x00, 0x03]);
        let r = c.step(&[]);
        assert_eq!(r, RxStep::Advanced);
        assert_eq!(c.pc, 0x1002);
    }

    #[test]
    fn bitop_memory_sets_bit() {
        // BSET #0, [r0]: op 0xF0, bit=0, dsp=0. With r0=0x2000 it sets bit 0
        // of memory[0x2000]. This exercises the 0xF0 bit-operation family,
        // which the `(op & 0xE0) == 0xE0` MOV family no longer shadows.
        let mut c = core_with(&[0xF0, 0x00]);
        c.regs[0] = 0x2000;
        c.step(&[]);
        assert_eq!(c.read_u8(0x2000) & 0x01, 0x01, "bset sets bit 0");
        assert_eq!(c.pc, 0x1002);
    }

    #[test]
    fn branch_link_flags() {
        // BSR.w (0x3900) is a call: it must push the return address (pc after
        // the opcode word and displacement), while BRA.w (0x3800) must not.
        let mut c = core_with(&[0x39, 0x00, 0x00, 0x04]);
        c.regs[0] = 0x2000;
        c.step(&[]);
        assert_eq!(c.pc, 0x1008, "bsr.w target");
        assert_eq!(c.regs[0], 0x1FFC, "bsr.w pushes the return address");
        assert_eq!(c.read_u32(0x1FFC), 0x1004, "pushed return address");

        let mut c = core_with(&[0x38, 0x00, 0x00, 0x04]);
        c.regs[0] = 0x2000;
        c.step(&[]);
        assert_eq!(c.pc, 0x1008, "bra.w target");
        assert_eq!(c.regs[0], 0x2000, "bra.w must not push");

        // BSR.a (0x0005) is a call; BRA.a (0x0004) is not.
        let mut c = core_with(&[0x00, 0x05, 0x00, 0x00, 0x00, 0x04]);
        c.regs[0] = 0x2000;
        c.step(&[]);
        assert_eq!(c.pc, 0x100A, "bsr.a target");
        assert_eq!(c.regs[0], 0x1FFC, "bsr.a pushes");
        assert_eq!(c.read_u32(0x1FFC), 0x1006, "bsr.a return address");

        let mut c = core_with(&[0x00, 0x04, 0x00, 0x00, 0x00, 0x04]);
        c.regs[0] = 0x2000;
        c.step(&[]);
        assert_eq!(c.pc, 0x100A, "bra.a target");
        assert_eq!(c.regs[0], 0x2000, "bra.a must not push");
    }

    /// Assert that the static decoder width matches the core's actual byte
    /// consumption for a straight-line (non-branching) instruction.
    fn assert_step_width(code: &[u8]) {
        let mut c = core_with(code);
        let pc = c.pc;
        let expected = instruction_len(pc, &|addr| c.read_u8(addr));
        let r = c.step(&[]);
        assert!(
            matches!(r, RxStep::Advanced | RxStep::Halted),
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
        // 2-byte opcode words with no operand.
        assert_step_width(&[0x00, 0x03]); // nop
        assert_step_width(&[0x79, 0x96]); // wait
        assert_step_width(&[0x00, 0x00]); // brk (halts after 2 bytes)
        assert_step_width(&[0x60, 0x0F]); // sub #0, r15
        assert_step_width(&[0x64, 0x0F]); // and #0, r15
        assert_step_width(&[0x68, 0x0F]); // shlr #0, r15
        assert_step_width(&[0x78, 0x0F]); // bset #0, r15
        assert_step_width(&[0x7E, 0x00]); // not r0
        assert_step_width(&[0x7E, 0x08]); // push r0
        assert_step_width(&[0x6E, 0x0F]); // pushm r0-r15
        assert_step_width(&[0x6F, 0x0F]); // popm r0-r15
        assert_step_width(&[0x80, 0x00]); // mov.b r0, 0[r0]
        assert_step_width(&[0x98, 0x00]); // mov.w 0[r0], r0
        assert_step_width(&[0xB0, 0x00]); // movu.b 0[r0], r0
        assert_step_width(&[0xF8, 0x00]); // mov [r0+r0], r0
        assert_step_width(&[0x40, 0x00]); // sub r0, r0 (register source)
        assert_step_width(&[0x43, 0x00]); // sub r0, r0 (ld=3 register)
        assert_step_width(&[0xFC, 0x27, 0x00]); // divu r0, r0 (execute_fc)
        assert_step_width(&[0xFD, 0x60, 0x00]); // shlr r0, r0 (execute_fd)
        assert_step_width(&[0xFD, 0x6C, 0x00]); // rotr #0, r0 (execute_fd)
        // 3-byte: one 8-bit displacement/immediate.
        assert_step_width(&[0x3C, 0x00, 0x10]); // mov.b #0x10, 0[r4]
        assert_step_width(&[0x41, 0x00, 0x10]); // sub dsp8[r0], r0
        assert_step_width(&[0x59, 0x00, 0x10]); // movu.b dsp8[r0], r0
        assert_step_width(&[0x5B, 0x00, 0x34, 0x12]); // movu.b #0x1234, r0 (4 bytes)
        assert_step_width(&[0xF1, 0x00, 0x10]); // bset #0, dsp8[r0]
        assert_step_width(&[0xC0, 0x20, 0x10]); // mov.b dsp8[r2], dsp0[r0] (3 bytes)
        // 4-byte: one 16-bit displacement/immediate.
        assert_step_width(&[0x42, 0x00, 0x34, 0x12]); // sub dsp16[r0], r0
        assert_step_width(&[0x5A, 0x00, 0x34, 0x12]); // movu.b dsp16[r0], r0
        assert_step_width(&[0xF2, 0x00, 0x34, 0x12]); // bset #0, dsp16[r0]
        assert_step_width(&[0x70, 0x00, 0x34, 0x12]); // add #0x1234, r0, r0
        assert_step_width(&[0x74, 0x00, 0x34, 0x12]); // cmp #0x1234, r0
        assert_step_width(&[0xC0, 0x30, 0x34, 0x12]); // mov.b dsp16[r3], dsp0[r0]
        assert_step_width(&[0xC1, 0x20, 0x10]); // mov.b dsp8[r2], dsp8[r0] (3 bytes)
        // Short branches: BRA.s is 1 byte, BCnd.s is 2 bytes.
        assert_eq!(instruction_len(0, &|a| if a == 0 { 0x08 } else { 0 }), 1);
        assert_eq!(instruction_len(0, &|a| if a == 0 { 0x10 } else { 0 }), 2);
        assert_eq!(instruction_len(0, &|a| if a == 0 { 0x0F } else { 0 }), 1);
        assert_eq!(instruction_len(0, &|a| if a == 0 { 0x1F } else { 0 }), 2);
        // BRA.w/BSR.w (0x3800/0x3900) are 4 bytes, BRA.a/BSR.a (0x0004/0x0005)
        // are 6 bytes.
        let read = |a: u32| if a == 0 { 0x38 } else if a == 1 { 0x00 } else { 0 };
        assert_eq!(instruction_len(0, &read), 4);
        let read = |a: u32| if a == 0 { 0x00 } else if a == 1 { 0x04 } else { 0 };
        assert_eq!(instruction_len(0, &read), 6);
    }


    #[test]
    fn mov_uimm4_rd() {
        // MOV #0x5, r3 => 0110 0110 imm4 rd4 => 0x66 0x53
        let mut c = core_with(&[0x66, 0x53]);
        c.step(&[]);
        assert_eq!(c.regs[3], 5);
        assert_eq!(c.pc, 0x1002);
    }

    #[test]
    fn add_uimm4_rd() {
        // ADD #0x2, r3
        let mut c = core_with(&[0x62, 0x23]);
        c.regs[3] = 10;
        c.step(&[]);
        assert_eq!(c.regs[3], 12);
    }

    #[test]
    fn sub_uimm4_rd() {
        // SUB #0x1, r3
        let mut c = core_with(&[0x60, 0x13]);
        c.regs[3] = 10;
        c.step(&[]);
        assert_eq!(c.regs[3], 9);
    }

    #[test]
    fn bra_short_forward() {
        // BRA.s: 1 byte 0x08 (dsp = low 7 bits = 8), target = pc+1+8.
        let mut c = core_with(&[0x08]);
        c.step(&[]);
        assert_eq!(c.pc, 0x1000 + 1 + 8);
    }

    #[test]
    fn mov_register_indirect() {
        // MOV.l [r5], r2: 1010 1 rs:3 . rd:3 dsp5 => rs=5, rd=2, dsp=0.
        // insn = 1010 1 0000 101 0 010 = 0xA852.
        let mut c = core_with(&[0xA8, 0x52]);
        c.regs[5] = 0x2000;
        c.write_u32(0x2000, 0xDEAD_BEEF);
        c.step(&[]);
        assert_eq!(c.regs[2], 0xDEAD_BEEF);
    }

    #[test]
    fn mov_register_indirect_store() {
        // MOV.l r2, [r5]: 1010 0 rd:3 . rs:3 dsp5 => rd=5, rs=2, dsp=0.
        // insn = 1010 0 0000 101 0 010 = 0xA052.
        let mut c = core_with(&[0xA0, 0x52]);
        c.regs[5] = 0x2000;
        c.regs[2] = 0x1234_5678;
        c.step(&[]);
        assert_eq!(c.read_u32(0x2000), 0x1234_5678);
    }

    #[test]
    fn push_pop_uses_r0() {
        // PUSH r5 (0111 1111 0010 ... no; PUSH dsp[rs] form)
        // Use MOV.l r2, [r5] store and manual.
        let mut c = RxCore::new();
        c.regs[0] = 0x2000;
        c.regs[5] = 0xAA;
        c.push(c.regs[5]);
        assert_eq!(c.regs[0], 0x1FFC);
        assert_eq!(c.read_u32(0x1FFC), 0xAA);
        let v = c.pop();
        assert_eq!(v, 0xAA);
        assert_eq!(c.regs[0], 0x2000);
    }

    #[test]
    fn jmp_register() {
        // JMP r5: 0111 1111 0000 rs:4 => 0x7F 0x05
        let mut c = core_with(&[0x7F, 0x05]);
        c.regs[5] = 0x3000;
        c.step(&[]);
        assert_eq!(c.pc, 0x3000);
    }

    #[test]
    fn and_or_imm4() {
        // AND #0xF, r4 => 0x64 0xF4
        let mut c = core_with(&[0x64, 0xF4]);
        c.regs[4] = 0x1F;
        c.step(&[]);
        assert_eq!(c.regs[4], 0x0F);
        // OR #0x0, r4 => 0x65 0x04
        let mut c2 = core_with(&[0x65, 0x04]);
        c2.regs[4] = 0x1F;
        c2.step(&[]);
        assert_eq!(c2.regs[4], 0x1F);
    }

    #[test]
    fn movu_zero_extend_byte() {
        // MOVU.b [r5], r2: 1011 sz:1 ... . rs:3 . rd:3 dsp5
        // sz=0, rs=5, rd=2, dsp=0 => 1011 0 000 0 101 0 010 = 0xB052.
        let mut c = core_with(&[0xB0, 0x52]);
        c.regs[5] = 0x2000;
        c.write_u8(0x2000, 0xFF);
        c.step(&[]);
        assert_eq!(c.regs[2], 0xFF);
    }

    #[test]
    fn add_sets_zero_and_carry_flags() {
        // ADD #0x2, r3 with r3=0xFFFFFFFE => 0, carry, zero.
        let mut c = core_with(&[0x62, 0x23]);
        c.regs[3] = 0xFFFF_FFFE;
        c.step(&[]);
        assert_eq!(c.regs[3], 0);
        assert!(c.psw.z, "zero flag");
        assert!(c.psw.c, "carry flag");
    }

    #[test]
    fn mov_byte_load_sign_extends() {
        // MOV.b dsp5[r5], r2: op=0x88, rs=5, rd=2, dsp=0 => 0x8852.
        let mut c = core_with(&[0x88, 0x52]);
        c.regs[5] = 0x2000;
        c.write_u8(0x2000, 0x80);
        c.step(&[]);
        assert_eq!(c.regs[2] as u8, 0x80, "byte loaded");
        assert_eq!(c.regs[2] as u32 & 0xFFFFFF80, 0xFFFF_FF80, "sign-extended");
    }

    #[test]
    fn bra_conditional_not_taken() {
        // BCnd.s: b0=0x10 (cond=0, BEQ), dsp byte=0x02.
        let mut c = core_with(&[0x10, 0x02]);
        c.psw.z = false;
        c.step(&[]);
        assert_eq!(c.pc, 0x1002, "not taken: pc+2");
        // Taken case.
        let mut c2 = core_with(&[0x10, 0x02]);
        c2.psw.z = true;
        c2.step(&[]);
        assert_eq!(c2.pc, 0x1004, "taken: pc+2+dsp2");
    }

    #[test]
    fn st_byte_memory() {
        // MOV.b r2, dsp5[r5]: op=0x80, rs=2 (data), rd=5 (base), dsp=0 => 0x8052.
        let mut c = core_with(&[0x80, 0x52]);
        c.regs[5] = 0x2000;
        c.regs[2] = 0xABCD;
        c.step(&[]);
        assert_eq!(c.read_u8(0x2000), 0xCD, "low byte stored");
    }

    #[test]
    fn mul_register() {
        // MUL r5, r3: op=0x4C, rs=5, rd=3 => low=0x53, insn=0x4C53.
        let mut c = core_with(&[0x4C, 0x53]);
        c.regs[5] = 6;
        c.regs[3] = 7;
        c.step(&[]);
        assert_eq!(c.regs[3], 42);
    }

    #[test]
    fn fc_alu_register_forms() {
        // SBB r5, r3 (0xFC 0x03 0x53).
        let mut c = core_with(&[0xFC, 0x03, 0x53]);
        c.regs[3] = 10;
        c.regs[5] = 3;
        c.step(&[]);
        assert_eq!(c.regs[3], 7);

        // ADC r5, r3 (0xFC 0x0B 0x53).
        let mut c = core_with(&[0xFC, 0x0B, 0x53]);
        c.regs[3] = 10;
        c.regs[5] = 3;
        c.psw.c = true;
        c.step(&[]);
        assert_eq!(c.regs[3], 14);

        // NEG r3 (0xFC 0x07 0x03).
        let mut c = core_with(&[0xFC, 0x07, 0x03]);
        c.regs[3] = 5;
        c.step(&[]);
        assert_eq!(c.regs[3] as i32, -5);

        // ABS r3 (0xFC 0x0F 0x03).
        let mut c = core_with(&[0xFC, 0x0F, 0x03]);
        c.regs[3] = 0xFFFF_FFFBu32;
        c.step(&[]);
        assert_eq!(c.regs[3], 5);

        // MAX r5, r3 (0xFC 0x13 0x53).
        let mut c = core_with(&[0xFC, 0x13, 0x53]);
        c.regs[3] = 4;
        c.regs[5] = 9;
        c.step(&[]);
        assert_eq!(c.regs[3], 9);

        // MIN r5, r3 (0xFC 0x17 0x53).
        let mut c = core_with(&[0xFC, 0x17, 0x53]);
        c.regs[3] = 4;
        c.regs[5] = 9;
        c.step(&[]);
        assert_eq!(c.regs[3], 4);

        // EMULU r5, r3 (0xFC 0x1F 0x53).
        let mut c = core_with(&[0xFC, 0x1F, 0x53]);
        c.regs[3] = 1000;
        c.regs[5] = 2000;
        c.step(&[]);
        assert_eq!(c.regs[3], 2_000_000);

        // DIVU r5, r3 (0xFC 0x27 0x53).
        let mut c = core_with(&[0xFC, 0x27, 0x53]);
        c.regs[3] = 100;
        c.regs[5] = 7;
        c.step(&[]);
        assert_eq!(c.regs[3], 14);

        // XOR r5, r3 (0xFC 0x37 0x53).
        let mut c = core_with(&[0xFC, 0x37, 0x53]);
        c.regs[3] = 0x0F0F;
        c.regs[5] = 0x00FF;
        c.step(&[]);
        assert_eq!(c.regs[3], 0x0FF0);

        // NOT r3 (0xFC 0x3B 0x03).
        let mut c = core_with(&[0xFC, 0x3B, 0x03]);
        c.regs[3] = 0x0F0F;
        c.step(&[]);
        assert_eq!(c.regs[3], !0x0F0Fu32);

        // XCHG r5, r3 (0xFC 0x43 0x53).
        let mut c = core_with(&[0xFC, 0x43, 0x53]);
        c.regs[3] = 11;
        c.regs[5] = 22;
        c.step(&[]);
        assert_eq!(c.regs[3], 22);
        assert_eq!(c.regs[5], 11);

        // STZ r5, r3 (0xFC 0x4B 0x53).
        let mut c = core_with(&[0xFC, 0x4B, 0x53]);
        c.regs[3] = 0;
        c.regs[5] = 99;
        c.psw.z = true;
        c.step(&[]);
        assert_eq!(c.regs[3], 99);

        // STNZ r5, r3 (0xFC 0x4F 0x53).
        let mut c = core_with(&[0xFC, 0x4F, 0x53]);
        c.regs[3] = 0;
        c.regs[5] = 99;
        c.psw.z = false;
        c.step(&[]);
        assert_eq!(c.regs[3], 99);
    }

    #[test]
    fn fc_bit_register_forms() {
        // BSET r5, r3 (0xFC 0x63 0x35): rd=3, rs=5.
        let mut c = core_with(&[0xFC, 0x63, 0x35]);
        c.regs[3] = 0;
        c.regs[5] = 4;
        c.step(&[]);
        assert_eq!(c.regs[3], 1 << 4);

        // BCLR r5, r3 (0xFC 0x67 0x35).
        let mut c = core_with(&[0xFC, 0x67, 0x35]);
        c.regs[3] = 0xFFFF_FFFF;
        c.regs[5] = 4;
        c.step(&[]);
        assert_eq!(c.regs[3], !(1u32 << 4));

        // BTST r5, r3 (0xFC 0x6B 0x35): Z set when bit 4 is clear.
        let mut c = core_with(&[0xFC, 0x6B, 0x35]);
        c.regs[3] = 0;
        c.regs[5] = 4;
        c.step(&[]);
        assert!(c.psw.z);

        // BNOT r5, r3 (0xFC 0x6F 0x35).
        let mut c = core_with(&[0xFC, 0x6F, 0x35]);
        c.regs[3] = 0;
        c.regs[5] = 4;
        c.step(&[]);
        assert_eq!(c.regs[3], 1 << 4);
    }

    #[test]
    fn fc_bm_sc_immediate() {
        // BNOT #0, r3 (0xFC 0xE3 0x3F): bit=0, rd=3, cond=0xF.
        let mut c = core_with(&[0xFC, 0xE3, 0x3F]);
        c.regs[3] = 0;
        c.step(&[]);
        assert_eq!(c.regs[3], 1);

        // BMNE #1, r3 (0xFC 0xE7 0x31): bit=1, rd=3, cond=1 (ne).
        let mut c = core_with(&[0xFC, 0xE7, 0x31]);
        c.regs[3] = 0;
        c.psw.z = false;
        c.step(&[]);
        assert_eq!(c.regs[3], 1 << 1);

        // SCEQ.b r3 (0xFC 0xD3 0x30): cond=0 (eq), rd=3, sz=0.
        let mut c = core_with(&[0xFC, 0xD3, 0x30]);
        c.psw.z = true;
        c.step(&[]);
        assert_eq!(c.regs[3], 1);
    }

    #[test]
    fn fd_shift_rotate_forms() {
        // SHLR r5, r3 (0xFD 0x60 0x53).
        let mut c = core_with(&[0xFD, 0x60, 0x53]);
        c.regs[3] = 0x8000_0000;
        c.regs[5] = 4;
        c.step(&[]);
        assert_eq!(c.regs[3], 0x0800_0000);

        // SHAR r5, r3 (0xFD 0x61 0x53).
        let mut c = core_with(&[0xFD, 0x61, 0x53]);
        c.regs[3] = 0x8000_0000;
        c.regs[5] = 4;
        c.step(&[]);
        assert_eq!(c.regs[3], 0xF800_0000);

        // SHLL r5, r3 (0xFD 0x62 0x53).
        let mut c = core_with(&[0xFD, 0x62, 0x53]);
        c.regs[3] = 0x0000_0001;
        c.regs[5] = 4;
        c.step(&[]);
        assert_eq!(c.regs[3], 0x0000_0010);

        // ROTR r5, r3 (0xFD 0x64 0x53).
        let mut c = core_with(&[0xFD, 0x64, 0x53]);
        c.regs[3] = 0x8000_0001;
        c.regs[5] = 1;
        c.step(&[]);
        assert_eq!(c.regs[3], 0xC000_0000);

        // ROTL r5, r3 (0xFD 0x66 0x53).
        let mut c = core_with(&[0xFD, 0x66, 0x53]);
        c.regs[3] = 0x8000_0001;
        c.regs[5] = 1;
        c.step(&[]);
        assert_eq!(c.regs[3], 0x0000_0003);

        // REVW r5, r3 (0xFD 0x65 0x53).
        let mut c = core_with(&[0xFD, 0x65, 0x53]);
        c.regs[5] = 0x1122_3344;
        c.step(&[]);
        assert_eq!(c.regs[3], 0x4433_2211);

        // REVL r5, r3 (0xFD 0x67 0x53).
        let mut c = core_with(&[0xFD, 0x67, 0x53]);
        c.regs[5] = 0x1122_3344;
        c.step(&[]);
        assert_eq!(c.regs[3], 0x4433_2211);
    }

    #[test]
    fn fd_imm_shifts_and_rotates() {
        // SHLR #4, r5, r3 (0xFD 0x84 0x53).
        let mut c = core_with(&[0xFD, 0x84, 0x53]);
        c.regs[5] = 0x8000_0000;
        c.step(&[]);
        assert_eq!(c.regs[3], 0x0800_0000);

        // SHLL #4, r5, r3 (0xFD 0xC4 0x53).
        let mut c = core_with(&[0xFD, 0xC4, 0x53]);
        c.regs[5] = 0x0000_0001;
        c.step(&[]);
        assert_eq!(c.regs[3], 0x0000_0010);

        // ROTR #1, r3 (0xFD 0x6C 0x13): imm=1, rd=3.
        let mut c = core_with(&[0xFD, 0x6C, 0x13]);
        c.regs[3] = 0x8000_0001;
        c.step(&[]);
        assert_eq!(c.regs[3], 0xC000_0000);

        // ROTL #1, r3 (0xFD 0x6E 0x13).
        let mut c = core_with(&[0xFD, 0x6E, 0x13]);
        c.regs[3] = 0x8000_0001;
        c.step(&[]);
        assert_eq!(c.regs[3], 0x0000_0003);
    }

    #[test]
    fn fd_mvtc_mvfc() {
        // MVTC r5, psw (0xFD 0x68 0x50): cr = 0*16 + 0 = 0, rs=5.
        let mut c = core_with(&[0xFD, 0x68, 0x50]);
        c.regs[5] = 0x2; // Z flag set.
        c.step(&[]);
        assert!(c.psw.z);
        assert_eq!(c.pc, 0x1003);

        // MVFC psw, r5 (0xFD 0x6A 0x05): cr = 0*16 + 0 = 0, rd=5.
        let mut c = core_with(&[0xFD, 0x6A, 0x05]);
        c.psw.c = true;
        c.psw.z = true;
        c.step(&[]);
        assert_eq!(c.regs[5] & 0x3, 0x3);
    }

    #[test]
    fn fd_mov_reg_register() {
        // MOV.l r3, r5 (0xFD 0x22 0x53): b1=0x22 (p=0, sz=2), op[2]=rdst:rsrc.
        let mut c = core_with(&[0xFD, 0x22, 0x53]);
        c.regs[3] = 0x1234_5678;
        c.step(&[]);
        assert_eq!(c.regs[5], 0x1234_5678);

        // MOV.b r3, r5 sign-extends the low byte (0xFD 0x20 0x53).
        let mut c = core_with(&[0xFD, 0x20, 0x53]);
        c.regs[3] = 0x80;
        c.step(&[]);
        assert_eq!(c.regs[5], 0xFFFF_FF80);

        // MOV.w r5, r3 (0xFD 0x2A 0x53): b1=0x2A (0x28-family, sz=2),
        // op[2]=rsrc:rdst so src=5, dst=3.
        let mut c = core_with(&[0xFD, 0x2A, 0x53]);
        c.regs[5] = 0xDEAD_BEEF;
        c.step(&[]);
        assert_eq!(c.regs[3], 0xDEAD_BEEF);
    }

    #[test]
    fn fd_imm_alu() {
        // ADC #5, r3 (0xFD 0x74 0x23 0x05): im=1 (imm8), op=0x20, rd=3.
        let mut c = core_with(&[0xFD, 0x74, 0x23, 0x05]);
        c.regs[3] = 10;
        c.psw.c = true;
        c.step(&[]);
        assert_eq!(c.regs[3], 16);

        // MAX #5, r3 (0xFD 0x74 0x43 0x05).
        let mut c = core_with(&[0xFD, 0x74, 0x43, 0x05]);
        c.regs[3] = 10;
        c.step(&[]);
        assert_eq!(c.regs[3], 10);

        // MIN #5, r3 (0xFD 0x74 0x53 0x05).
        let mut c = core_with(&[0xFD, 0x74, 0x53, 0x05]);
        c.regs[3] = 10;
        c.step(&[]);
        assert_eq!(c.regs[3], 5);

        // EMULU #5, r3 (0xFD 0x74 0x73 0x05).
        let mut c = core_with(&[0xFD, 0x74, 0x73, 0x05]);
        c.regs[3] = 100;
        c.step(&[]);
        assert_eq!(c.regs[3], 500);

        // DIVU #5, r3 (0xFD 0x74 0x93 0x05).
        let mut c = core_with(&[0xFD, 0x74, 0x93, 0x05]);
        c.regs[3] = 100;
        c.step(&[]);
        assert_eq!(c.regs[3], 20);

        // TST #5, r3 (0xFD 0x74 0xC3 0x05).
        let mut c = core_with(&[0xFD, 0x74, 0xC3, 0x05]);
        c.regs[3] = 0;
        c.step(&[]);
        assert!(c.psw.z);

        // XOR #5, r3 (0xFD 0x74 0xD3 0x05).
        let mut c = core_with(&[0xFD, 0x74, 0xD3, 0x05]);
        c.regs[3] = 0x1F;
        c.step(&[]);
        assert_eq!(c.regs[3], 0x1A);

        // STZ #5, r3 (0xFD 0x74 0xE3 0x05).
        let mut c = core_with(&[0xFD, 0x74, 0xE3, 0x05]);
        c.regs[3] = 0;
        c.psw.z = true;
        c.step(&[]);
        assert_eq!(c.regs[3], 5);
    }

    #[test]
    fn fd_bm_immediate() {
        // BNOT #1, r3 (0xFD 0xE1 0xF3): bittt=1, cond=0xF, rd=3.
        let mut c = core_with(&[0xFD, 0xE1, 0xF3]);
        c.regs[3] = 0;
        c.step(&[]);
        assert_eq!(c.regs[3], 1 << 1);

        // BMNE #1, r3 (0xFD 0xE1 0x13): bittt=1, cond=1 (ne), rd=3.
        let mut c = core_with(&[0xFD, 0xE1, 0x13]);
        c.regs[3] = 0;
        c.psw.z = false;
        c.step(&[]);
        assert_eq!(c.regs[3], 1 << 1);
    }

    #[test]
    fn disassembles_fc_fd_long_forms() {
        let read = |a: u32| -> u8 {
            let code = [
                0xFC, 0x03, 0x53, // sbb r5, r3
                0xFC, 0x27, 0x00, // divu r0, r0
                0xFC, 0x63, 0x35, // bset r5, r3
                0xFD, 0x60, 0x53, // shlr r5, r3
                0xFD, 0x6A, 0x05, // mvfc psw, r5
                0xFD, 0x74, 0x23, 0x05, // adc #5, r3
            ];
            code.get(a as usize).copied().unwrap_or(0)
        };
        assert_eq!(disassemble(0, &read), "sbb r5, r3");
        assert_eq!(disassemble(3, &read), "divu r0, r0");
        assert_eq!(disassemble(6, &read), "bset r5, r3");
        assert_eq!(disassemble(9, &read), "shlr r5, r3");
        assert_eq!(disassemble(12, &read), "mvfc psw, r5");
        assert_eq!(disassemble(15, &read), "adc #0x5, r3");
    }

    #[test]
    fn instruction_len_fc_fd() {
        // 0xFC/0xFD register forms consume the opcode word plus one operand byte.
        let read = |a: u32| -> u8 {
            let code = [0xFD, 0x60, 0x53];
            code.get(a as usize).copied().unwrap_or(0)
        };
        assert_eq!(instruction_len(0, &read), 3);
        // The immediate-ALU forms add the immediate width selected by `im`
        // (im=1 -> imm8, im=0 -> imm32).
        let read8 = |a: u32| -> u8 {
            let code = [0xFD, 0x74, 0x23, 0x05];
            code.get(a as usize).copied().unwrap_or(0)
        };
        assert_eq!(instruction_len(0, &read8), 4);
        let read32 = |a: u32| -> u8 {
            let code = [0xFD, 0x70, 0x23, 0x01, 0x00, 0x00, 0x00];
            code.get(a as usize).copied().unwrap_or(0)
        };
        assert_eq!(instruction_len(0, &read32), 7);
    }
}
