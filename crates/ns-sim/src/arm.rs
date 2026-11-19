//! An ARM Cortex-M (Thumb/Thumb-2) instruction-set simulator core.
//!
//! Models the ARMv7-M architecture as used by STM32, nRF52 and RP2040: 16
//! general-purpose registers (R13=SP, R14=LR, R15=PC), the xPSR program
//! status register, PRIMASK/BASEPRI interrupt masking, the vector table
//! (initial SP + reset vector + exception vectors), and the Thumb instruction
//! set (16-bit Thumb-1 plus the commonly used 32-bit Thumb-2 encodings such
//! as `BL` and wide data-processing forms).
//!
//! The memory model is a flat 32-bit address space backed by a byte vector
//! large enough to cover the common Cortex-M flash/RAM windows; accesses
//! outside the simulated range read 0 and drop writes (mirroring an unbacked
//! address) rather than panicking.

/// Simulated address-space size (64 MiB covers flash + peripheral + RAM
/// windows for the common Cortex-M parts).
pub const ADDR_SIZE: usize = 64 * 1024 * 1024;

/// Stack pointer register index.
pub const SP: usize = 13;
/// Link register index.
pub const LR: usize = 14;
/// Program counter register index.
pub const PC: usize = 15;

/// An ARM Cortex-M core.
pub struct ArmCore {
    /// Register file R0-R15 (R13=SP, R14=LR, R15=PC).
    pub regs: [u32; 16],
    /// Combined program status register (APSR/IPSR/EPSR). Bits 31-28 = N/Z/C/V
    /// condition flags, bit 24 = Thumb, bits 0-8 = exception number.
    pub xpsr: u32,
    /// Interrupt-priority mask register (bit 0 = mask all).
    pub primask: u32,
    /// Base priority mask (values >= this are masked; 0 = disabled).
    pub basepri: u32,
    /// Fault mask register (bit 0 = mask all faults).
    pub faultmask: u32,
    /// Control register (bit 0 = thread stack pointer select, bit 1 = FPU).
    pub control: u32,
    /// Flat memory image.
    pub memory: Vec<u8>,
    /// Vector-table base address (VTOR).
    pub vtor: u32,
    /// Whether the core executes in ARM (A32) state rather than Thumb state.
    /// ARM7/ARM9/Cortex-A reset with a vector whose bit 0 is clear; Cortex-M
    /// always boots in Thumb state (this stays `false`).
    pub arm_state: bool,
     /// Whether execution should stop.
     pub halted: bool,
     /// Number of instructions executed since reset.
     pub cycle_count: u64,
    /// Byte-level stores performed since the last drain, as (address, byte).
    /// The `Simulator` routes these to its virtual peripherals after each step.
    pub store_events: Vec<(u32, u8)>,
    /// Active If-Then (IT) block: 0 when no IT is active, otherwise the 4-bit
    /// IT mask (bits 3-0 of the IT encoding). The base condition and the index
    /// of the instruction being evaluated are kept alongside.
    pub it_mask: u8,
    /// Base condition for the active IT block (bits 3-0 of the IT encoding).
    pub it_cond: u32,
    /// Index (0-based) of the instruction currently inside the IT block.
    pub it_index: u8,
 }

impl ArmCore {
    pub fn new() -> Self {
        Self {
            regs: [0; 16],
            xpsr: 0x0100_0000, // Thumb bit set
            primask: 0,
            basepri: 0,
            faultmask: 0,
            control: 0,
            memory: vec![0; ADDR_SIZE],
            vtor: 0,
            arm_state: false,
             halted: false,
            cycle_count: 0,
            store_events: Vec::new(),
            it_mask: 0,
            it_cond: 0,
            it_index: 0,
         }
    }

    /// Set the program counter (keeps the Thumb bit in the low bit).
    pub fn set_pc(&mut self, pc: u32) {
        self.regs[PC] = pc & !1;
    }

    /// Select ARM (A32) or Thumb decoding. ARM7/ARM9/Cortex-A firmware runs
    /// in ARM state; Cortex-M never does.
    pub fn set_arm_state(&mut self, arm: bool) {
        self.arm_state = arm;
        self.set_thumb(!arm);
    }

    // -------- condition flags --------
    pub fn flag_n(&self) -> bool {
        self.xpsr & 0x8000_0000 != 0
    }
    pub fn flag_z(&self) -> bool {
        self.xpsr & 0x4000_0000 != 0
    }
    pub fn flag_c(&self) -> bool {
        self.xpsr & 0x2000_0000 != 0
    }
    pub fn flag_v(&self) -> bool {
        self.xpsr & 0x1000_0000 != 0
    }
    fn set_nz(&mut self, value: u32) {
        self.xpsr = (self.xpsr & !0xC000_0000)
            | (if value & 0x8000_0000 != 0 { 0x8000_0000 } else { 0 });
        if value == 0 {
            self.xpsr |= 0x4000_0000;
        }
    }

    /// Set the overflow (V) flag, bit 28 of xPSR.
    fn set_v(&mut self, v: bool) {
        if v {
            self.xpsr |= 0x1000_0000;
        } else {
            self.xpsr &= !0x1000_0000;
        }
    }

    /// Compute and set V for an add/sub operation from the operands and result.
    fn set_add_sub_v(&mut self, a: u32, b: u32, result: u32, sub: bool) {
        let v = if sub {
            (a ^ b) & (a ^ result) & 0x8000_0000 != 0
        } else {
            (a ^ result) & (b ^ result) & 0x8000_0000 != 0
        };
        self.set_v(v);
    }

    /// The Thumb bit of xPSR (bit 24).
    pub fn thumb(&self) -> bool {
        self.xpsr & 0x0100_0000 != 0
    }
    fn set_thumb(&mut self, value: bool) {
        if value {
            self.xpsr |= 0x0100_0000;
        } else {
            self.xpsr &= !0x0100_0000;
        }
    }

    /// Exception number currently active (bits 0-8 of IPSR).
    pub fn exception_number(&self) -> u32 {
        self.xpsr & 0x1FF
    }
    fn set_exception(&mut self, number: u32) {
        self.xpsr = (self.xpsr & !0x1FF) | (number & 0x1FF);
    }
    // -------- memory access --------
    /// Map a bus address onto the flat simulation buffer. The flash window
    /// (0x0800_0000) maps to offset 0, the Cortex-M SRAM window (0x2000_0000)
    /// maps to offset 0x2000_0000, the ARM7/ARM9 SRAM windows (0x3000_0000 for
    /// S3C2440, 0x4000_0000 for LPC2148) map to 0x3000_0000/0x4000_0000, and
    /// low addresses (code loaded at 0 for tests) map to themselves. Returns
    /// None for unbacked addresses.
    fn map_addr(&self, address: u32) -> Option<usize> {
        let a = address;
        let offset = if (0x0800_0000..0x1000_0000).contains(&a) {
            a - 0x0800_0000
        } else if (0x2000_0000..0x2400_0000).contains(&a) {
            a - 0x2000_0000 + 0x0200_0000
        } else if (0x3000_0000..0x3400_0000).contains(&a) {
            a - 0x3000_0000 + 0x3000_0000
        } else if (0x4000_0000..0x4400_0000).contains(&a) {
            a - 0x4000_0000 + 0x4000_0000
        } else if a < 0x2000_0000 {
            a
        } else {
            return None;
        };
        if (offset as usize) < self.memory.len() {
            Some(offset as usize)
        } else {
            None
        }
    }

    pub fn read_u32(&self, address: u32) -> u32 {
        let Some(base) = self.map_addr(address & !3) else {
            return 0;
        };
        if base + 4 > self.memory.len() {
            return 0;
        }
        u32::from_le_bytes([
            self.memory[base],
            self.memory[base + 1],
            self.memory[base + 2],
            self.memory[base + 3],
        ])
    }
     pub fn write_u32(&mut self, address: u32, value: u32) {
         let Some(base) = self.map_addr(address & !3) else {
             for i in 0..4 {
                 self.store_events.push((address.wrapping_add(i as u32), (value >> (8 * i)) as u8));
             }
             return; // unbacked address: drop write
         };
         if base + 4 > self.memory.len() {
             for i in 0..4 {
                 self.store_events.push((address.wrapping_add(i as u32), (value >> (8 * i)) as u8));
             }
             return;
         }
         self.memory[base..base + 4].copy_from_slice(&value.to_le_bytes());
         for i in 0..4 {
             self.store_events.push((address.wrapping_add(i as u32), (value >> (8 * i)) as u8));
         }
     }
    pub fn read_u16(&self, address: u32) -> u16 {
        let Some(base) = self.map_addr(address) else {
            return 0;
        };
        if base + 2 > self.memory.len() {
            return 0;
        }
        u16::from_le_bytes([self.memory[base], self.memory[base + 1]])
    }
     pub fn write_u16(&mut self, address: u32, value: u16) {
         let Some(base) = self.map_addr(address) else {
            self.store_events.push((address, value as u8));
            self.store_events.push((address.wrapping_add(1), (value >> 8) as u8));
             return;
         };
         if base + 2 > self.memory.len() {
            self.store_events.push((address, value as u8));
            self.store_events.push((address.wrapping_add(1), (value >> 8) as u8));
             return;
         }
         self.memory[base..base + 2].copy_from_slice(&value.to_le_bytes());
         self.store_events.push((address, value as u8));
         self.store_events.push((address.wrapping_add(1), (value >> 8) as u8));
     }
     pub fn read_u8(&self, address: u32) -> u8 {
         let Some(base) = self.map_addr(address) else {
             return 0;
         };
         self.memory[base]
     }
     pub fn write_u8(&mut self, address: u32, value: u8) {
         let Some(base) = self.map_addr(address) else {
            self.store_events.push((address, value));
             return;
         };
         self.memory[base] = value;
         self.store_events.push((address, value));
     }

    /// Write a block of bytes (e.g. an ELF segment).
    pub fn write_bytes(&mut self, base: u32, bytes: &[u8]) {
        for (i, byte) in bytes.iter().enumerate() {
            let Some(addr) = self.map_addr(base.wrapping_add(i as u32)) else {
                continue;
            };
            self.memory[addr] = *byte;
        }
    }

    // -------- vector table --------
    /// Reset the core from the vector table at `vtor` (like real hardware).
    ///
    /// Cortex-M stores initial SP at `vtor` and reset PC (Thumb bit set) at
    /// `vtor+4`. ARM7/ARM9/Cortex-A (A32) instead keep *code* at the vector
    /// table — the reset vector is an ARM branch instruction — so the reset PC
    /// comes from the ELF entry (bit 0 clear = ARM state) and SP stays default.
    pub fn reset_from_vector_table(&mut self, vtor: u32, entry: u32) {
        self.vtor = vtor;
        let word0 = self.read_u32(vtor);
        let word1 = self.read_u32(vtor + 4);
        // ARM7/ARM9/Cortex-A keep *code* at the vector table: word0 is an ARM
        // branch instruction (top nibble 0xE). Cortex-M stores data: word0 is
        // the initial SP (high address) and word1 the reset PC (Thumb bit set).
        let is_code_vector = word0 & 0xF000_0000 == 0xE000_0000;
        if is_code_vector {
            // A32 device: the ELF entry (bit 0 clear) is the reset PC.
            let thumb = entry & 1 != 0;
            self.regs[SP] = 0x2000_0000;
            self.regs[PC] = entry & !1;
            self.arm_state = !thumb;
            self.set_thumb(thumb);
        } else {
            // Cortex-M data vector: word0 = initial SP, word1 = reset PC.
            let initial_sp = if word0 >= 0x2000_0000 { word0 } else { 0x2000_0000 };
            let thumb = word1 & 1 != 0;
            let pc = if word1 != 0 { word1 } else { entry };
            self.regs[SP] = initial_sp;
            self.regs[PC] = pc & !1;
            self.arm_state = !thumb;
            self.set_thumb(thumb);
        }
        self.xpsr &= !0x1FF; // clear IPSR
        self.halted = false;
        self.cycle_count = 0;
    }

    /// Read the exception vector for `number` (0 = reset, others from the
    /// table at VTOR + 4*number). Returns None for unbacked/zero vectors.
    pub fn vector(&self, number: u32) -> Option<u32> {
        let addr = self.vtor.wrapping_add(number.wrapping_mul(4));
        if (addr as usize) + 4 > self.memory.len() {
            return None;
        }
        let value = self.read_u32(addr);
        if value == 0 {
            None
        } else {
            Some(value)
        }
    }
}

impl ArmCore {
    /// Execute one Thumb instruction. The instruction at the current PC is
    /// decoded as 16-bit Thumb-1 or 32-bit Thumb-2 (when the first halfword
    /// has the 0xF000-0xF800 prefix). Returns the outcome.
    pub fn step(&mut self, breakpoints: &[u32]) -> ArmStep {
        if self.halted {
            return ArmStep::Halted;
        }
        let pc = self.regs[PC] & !1;
        if breakpoints.iter().any(|bp| *bp & !1 == pc) {
            return ArmStep::Breakpoint;
        }
        if self.arm_state {
            // ARM state: every instruction is a 32-bit word.
            let insn = self.read_u32(pc);
            self.regs[PC] = pc.wrapping_add(4);
            self.execute_arm(insn);
            self.cycle_count += 1;
            if self.halted {
                return ArmStep::Halted;
            }
            return ArmStep::Advanced;
        }
        let half = self.read_u16(pc);
        let (insn, width) = if half & 0xF800 >= 0xE800 {
            let second = self.read_u16(pc.wrapping_add(2));
            (((half as u32) << 16) | second as u32, 4)
        } else {
            (half as u32, 2)
        };
        self.regs[PC] = pc.wrapping_add(width as u32);
        self.execute(insn, width == 4);
        self.cycle_count += 1;
        if self.halted {
            ArmStep::Halted
        } else {
            ArmStep::Advanced
        }
    }

    fn execute(&mut self, insn: u32, wide: bool) {
        // An active IT block gates this instruction: if the condition fails
        // the instruction is skipped (PC was already advanced) and the block
        // advances. The instruction that *sets up* the block (the IT itself)
        // starts with no active IT, so it is never gated or counted.
        let in_it = self.it_mask != 0;
        if in_it {
            let cond = self.it_current_condition();
            if !self.condition_true(cond) {
                self.it_advance();
                return;
            }
        }
        if wide {
            self.execute_wide(insn);
        } else {
            self.execute_thumb1(insn as u16);
        }
        if in_it {
            self.it_advance();
        }
    }

    /// Condition code for the instruction at the current IT-block index.
    fn it_current_condition(&self) -> u32 {
        let mask = self.it_mask as u32;
        let index = self.it_index as u32;
        // The first instruction is always the "then" arm; later arms are
        // "else" when the bit at position `4 - index` of the mask is set.
        let then = index == 0 || ((mask >> (4 - index)) & 1) == 0;
        if then {
            self.it_cond
        } else {
            self.it_cond ^ 1
        }
    }

    /// Advance the active IT block after one instruction was evaluated.
    fn it_advance(&mut self) {
        if self.it_mask == 0 {
            return;
        }
        let n = 4 - (self.it_mask as u32).trailing_zeros();
        self.it_index += 1;
        if self.it_index as u32 >= n {
            self.it_mask = 0;
            self.it_index = 0;
        }
    }
}

/// Outcome of a single ARM step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmStep {
    Advanced,
    Breakpoint,
    Halted,
}

impl ArmCore {

    /// Decode and execute a 16-bit Thumb-1 instruction.
    fn execute_thumb1(&mut self, insn: u16) {
        let top = insn >> 12; // bits 15-12: the Thumb-1 major group
        let rd = (insn & 0x07) as usize;
        let imm8 = (insn & 0xFF) as u32;
        let imm3 = ((insn >> 6) & 0x07) as u32;
        let imm5 = ((insn >> 6) & 0x1F) as u32;
        match top {
            // 0x0: LSL/LSR/ASR immediate (shift op = bits 12-11)
            0x0 => {
                let shift_op = (insn >> 11) & 0x03;
                let shift = imm5;
                let rm = ((insn >> 3) & 0x07) as usize;
                let value = self.regs[rm];
                let (r, carry, changed) = thumb_shift_value(value, shift, shift_op as u32);
                if changed {
                    self.xpsr = (self.xpsr & !0x2000_0000) | ((carry as u32) << 29);
                }
                self.set_nz(r);
                self.regs[rd] = r;
            }
            // ASR immediate: top = 1 with bit 11 clear (bits 15-11 = 00010).
            0x1 if insn & 0x0800 == 0 => {
                let shift = imm5;
                let rm = ((insn >> 3) & 0x07) as usize;
                let value = self.regs[rm];
                let (r, carry, changed) = thumb_shift_value(value, shift, 2);
                if changed {
                    self.xpsr = (self.xpsr & !0x2000_0000) | ((carry as u32) << 29);
                }
                self.set_nz(r);
                self.regs[rd] = r;
            }
            // 0x1: ADD/SUB (bit 10 = sub, bit 9 = immediate)
            0x1 => {
                // bits 11-9: 100 ADD reg, 101 SUB reg, 110 ADD imm3,
                // 111 SUB imm3 → bit 9 selects SUB, bit 10 selects immediate.
                let sub = (insn >> 9) & 1 != 0;
                let immediate = (insn >> 10) & 1 != 0;
                let rn = ((insn >> 3) & 0x07) as usize;
                let value = self.regs[rn];
                let operand = if immediate {
                    imm3
                } else {
                    self.regs[((insn >> 6) & 0x07) as usize]
                };
                let r = if sub {
                    self.set_add_carry(value, operand, true);
                    value.wrapping_sub(operand)
                } else {
                    self.set_add_carry(value, operand, false);
                    value.wrapping_add(operand)
                };
                self.set_nz(r);
                self.set_add_sub_v(value, operand, r, sub);
                self.regs[rd] = r;
            }
            // 0x2: MOV/CMP immediate 8-bit (bit 11 = CMP)
            0x2 => {
                let rd = ((insn >> 8) & 0x07) as usize;
                let cmp = (insn >> 11) & 1 != 0;
                if cmp {
                    let value = self.regs[rd];
                    self.set_add_carry(value, imm8, true);
                    let r = value.wrapping_sub(imm8);
                    self.set_nz(r);
                    self.set_add_sub_v(value, imm8, r, true);
                } else {
                    // MOVS: writes the register and updates N/Z.
                    self.regs[rd] = imm8;
                    self.set_nz(imm8);
                }
            }
            // 0x3: ADD/SUB immediate 8-bit (bit 10 = sub)
            0x3 => {
                let rd = ((insn >> 8) & 0x07) as usize;
                let sub = (insn >> 11) & 1 != 0;
                let value = self.regs[rd];
                let r = if sub {
                    self.set_add_carry(value, imm8, true);
                    value.wrapping_sub(imm8)
                } else {
                    self.set_add_carry(value, imm8, false);
                    value.wrapping_add(imm8)
                };
                self.set_nz(r);
                self.set_add_sub_v(value, imm8, r, sub);
                self.regs[rd] = r;
            }
            // 0x4: ALU (0x4000-0x43FF), high-register/BX (0x4400-0x47FF),
            // or PC-relative LDR (0x4800-0x4FFF).
            0x4 => {
                if insn & 0x0800 != 0 {
                    // PC-relative LDR: rd = bits 10-8, imm8 scaled by 4.
                    let rd = ((insn >> 8) & 0x07) as usize;
                    // Thumb-1 is 16-bit, so `step` left PC at the next
                    // instruction (current + 2); the architectural PC used for
                    // PC-relative addressing is current + 4 (two halfwords
                    // ahead), word-aligned. The literal base is Align(PC, 4).
                    let arch_pc = self.regs[PC].wrapping_add(2);
                    let addr = (arch_pc & !3).wrapping_add(imm8.wrapping_mul(4));
                    self.regs[rd] = self.read_u32(addr);
                } else if insn & 0x0400 != 0 {
                    // High-register ops / BX (op = bits 9-8)
                    let rd_high = rd | (((insn >> 4) & 0x08) as usize);
                    let rm_high = ((insn >> 3) & 0x0F) as usize;
                    let opcode = (insn >> 8) & 0x03;
                    match opcode {
                        0 => {
                            let r = self.regs[rd_high].wrapping_add(self.regs[rm_high]);
                            self.regs[rd_high] = r;
                        }
                        1 => {
                            self.set_add_carry(self.regs[rd_high], self.regs[rm_high], true);
                            let r = self.regs[rd_high].wrapping_sub(self.regs[rm_high]);
                            self.set_nz(r);
                            // CMP (register) updates N, Z, C and V; the
                            // high-register form has the same semantics.
                            self.set_add_sub_v(self.regs[rd_high], self.regs[rm_high], r, true);
                        }
                        2 => {
                            self.regs[rd_high] = self.regs[rm_high];
                        }
                        _ => {
                            let target = self.regs[rm_high];
                            // BLX (bit 7 set) links the return address; BX
                            // does not. Both exchange the Thumb bit from bit 0.
                            if insn & 0x80 != 0 {
                                self.regs[LR] = self.regs[PC];
                            }
                            self.set_thumb(target & 1 != 0);
                            self.regs[PC] = target & !1;
                        }
                    }
                } else {
                    // ALU
                    let alu = (insn >> 6) & 0x0F;
                    self.execute_alu(alu as u32, rd, ((insn >> 3) & 0x07) as usize);
                }
            }
            // 0x5: load/store register offset (opcode = bits 11-9)
            0x5 => {
                let opcode = (insn >> 9) & 0x07;
                let rn = ((insn >> 3) & 0x07) as usize;
                let rm = ((insn >> 6) & 0x07) as usize;
                let addr = self.regs[rn].wrapping_add(self.regs[rm]);
                match opcode {
                    0 => self.write_u32(addr, self.regs[rd]),
                    1 => self.write_u16(addr, self.regs[rd] as u16),
                    2 => self.write_u8(addr, self.regs[rd] as u8),
                    3 => {
                        // LDRSB: sign-extend byte
                        let v = self.read_u8(addr) as i8 as i32 as u32;
                        self.regs[rd] = v;
                    }
                    4 => self.regs[rd] = self.read_u32(addr),
                    5 => self.regs[rd] = self.read_u16(addr) as u32,
                    6 => self.regs[rd] = self.read_u8(addr) as u32,
                    7 => {
                        // LDRSH: sign-extend half
                        let v = self.read_u16(addr) as i16 as i32 as u32;
                        self.regs[rd] = v;
                    }
                    _ => {}
                }
            }
             // 0x6: load/store word immediate offset (bit 11 = load)
             0x6 => {
                 let load = (insn >> 11) & 1 != 0;
                // Thumb: `STR/LDR Rt, [Rn, #imm]` — Rn = bits 2-0, Rt = bits 5-3.
                let base = (insn & 0x07) as usize;
                let data = ((insn >> 3) & 0x07) as usize;
                let addr = self.regs[base].wrapping_add(imm5 * 4);
                 if load {
                    self.regs[data] = self.read_u32(addr);
                 } else {
                    self.write_u32(addr, self.regs[data]);
                 }
             }
             // 0x7: load/store byte immediate offset (bit 11 = load)
             0x7 => {
                 let load = (insn >> 11) & 1 != 0;
                let base = (insn & 0x07) as usize;
                let data = ((insn >> 3) & 0x07) as usize;
                let addr = self.regs[base].wrapping_add(imm5);
                 if load {
                    self.regs[data] = self.read_u8(addr) as u32;
                 } else {
                    self.write_u8(addr, self.regs[data] as u8);
                 }
             }
             // 0x8: load/store half immediate offset (bit 11 = load)
             0x8 => {
                 let load = (insn >> 11) & 1 != 0;
                let base = (insn & 0x07) as usize;
                let data = ((insn >> 3) & 0x07) as usize;
                let addr = self.regs[base].wrapping_add(imm5 * 2);
                 if load {
                    self.regs[data] = self.read_u16(addr) as u32;
                 } else {
                    self.write_u16(addr, self.regs[data] as u16);
                 }
             }
            // 0x9: SP-relative load/store word (bit 11 = load)
            0x9 => {
                let load = (insn >> 11) & 1 != 0;
                let rd = ((insn >> 8) & 0x07) as usize;
                let addr = self.regs[SP].wrapping_add(imm8.wrapping_mul(4));
                if load {
                    self.regs[rd] = self.read_u32(addr);
                } else {
                    self.write_u32(addr, self.regs[rd]);
                }
            }
            // 0xA: load address (bit 11 = SP-relative)
            0xA => {
                let sp_relative = (insn >> 11) & 1 != 0;
                let rd = ((insn >> 8) & 0x07) as usize;
                let base = if sp_relative {
                    self.regs[SP]
                } else {
                    // ADR/`add rd, pc, #imm`: the PC base is Align(PC, 4)
                    // where the architectural PC is current + 4 (see above).
                    let arch_pc = self.regs[PC].wrapping_add(2);
                    arch_pc & !3
                };
                self.regs[rd] = base.wrapping_add(imm8.wrapping_mul(4));
            }
            // 0xB: CBZ/CBNZ (0xB1xx/0xB9xx), ADD/SUB SP (0xB0-0xB3), sign/zero
            // extend (0xB2xx), NOP (0xBF00) or PUSH/POP (0xB4-0xBF).
            0xB => {
                // CBZ/CBNZ (1011 i 0 0 1 imm5 Rn): 0xB1xx CBZ, 0xB9xx CBNZ.
                if insn & 0x0F00 == 0x0100 || insn & 0x0F00 == 0x0900 {
                    let cbz = insn & 0x0800 == 0;
                    let imm5 = ((insn >> 3) & 0x1F) as u32;
                    let rn = (insn & 0x07) as usize;
                    let taken = if cbz {
                        self.regs[rn] == 0
                    } else {
                        self.regs[rn] != 0
                    };
                    if taken {
                        // Architectural PC = current + 4; `step` left PC at
                        // current + 2, so add 2 to reach the branch base.
                        let arch_pc = self.regs[PC].wrapping_add(2);
                        self.regs[PC] = (arch_pc.wrapping_add(imm5 * 2)) & !1;
                    }
                } else if insn & 0xFF00 == 0xB000 {
                    // ADD/SUB SP (1011 0000 0/1 imm7): bit 7 = sub, bits 6-0
                    // = imm7 (scaled by 4). The low byte includes the sub bit,
                    // so mask it off when forming the byte offset.
                    let sub = (insn >> 7) & 1 != 0;
                    let value = (imm8 & 0x7F).wrapping_mul(4);
                    self.regs[SP] = if sub {
                        self.regs[SP].wrapping_sub(value)
                    } else {
                        self.regs[SP].wrapping_add(value)
                    };
                } else if insn & 0xFF00 == 0xB200 {
                    // SXTH/SXTB/UXTH/UXTB (1011 0010 0 op Rm Rd): op = bits
                    // 7-6, Rm = bits 5-3, Rd = bits 2-0. SXTH/SXTB
                    // sign-extend; UXTH/UXTB zero-extend.
                    let op = (insn >> 6) & 0x03;
                    let rm = ((insn >> 3) & 0x07) as usize;
                    let rd = (insn & 0x07) as usize;
                    let v = self.regs[rm];
                    let r = match op {
                        0 => v as u16 as i16 as i32 as u32, // SXTH
                        1 => v as u8 as i8 as i32 as u32,   // SXTB
                        2 => (v as u16) as u32,             // UXTH
                        _ => (v as u8) as u32,              // UXTB
                    };
                    self.regs[rd] = r;
                } else if insn & 0xFFF0 == 0xB660 || insn & 0xFFF0 == 0xB670 {
                    // CPSIE/CPSID (interrupt mask): bit 4 = disable, the `i`
                    // bit (0x02) selects the interrupt mask (PRIMASK).
                    if insn & 0x02 != 0 {
                        let disable = insn & 0x10 != 0;
                        self.primask = if disable { 1 } else { 0 };
                    }
                } else if insn & 0xFF00 == 0xBA00 {
                    // REV/REV16/REVSH (1011 1010 0 op Rm Rd): op = bits 7-6.
                    let op = (insn >> 6) & 0x03;
                    let rm = ((insn >> 3) & 0x07) as usize;
                    let rd = (insn & 0x07) as usize;
                    let v = self.regs[rm];
                    let r = match op {
                        0 => v.swap_bytes(), // REV
                        1 => {
                            // REV16: swap the bytes of each halfword.
                            (v & 0x00FF_00FF).rotate_left(8)
                                | ((v & 0xFF00_FF00) >> 8)
                        }
                        _ => {
                            // REVSH: reverse the low halfword bytes, sign-extend.
                            let low = (v as u16).swap_bytes();
                            low as i16 as i32 as u32
                        }
                    };
                    self.regs[rd] = r;
                } else if insn & 0xFF00 == 0xBE00 {
                    // BKPT (breakpoint): stop the core so the caller can
                    // observe the stopped state.
                    self.halted = true;
                } else if insn & 0xFF00 == 0xBF00 {
                    // IT (If-Then) / NOP. Condition = bits 7-4, mask = bits
                    // 3-0; a zero mask is NOP, otherwise it arms the block.
                    let mask = (insn & 0x0F) as u8;
                    if mask != 0 {
                        self.it_cond = ((insn >> 4) & 0x0F) as u32;
                        self.it_mask = mask;
                        self.it_index = 0;
                    }
                    // mask == 0: NOP, no effect.
                } else {
                    let push = (insn >> 11) & 1 == 0;
                    let rlist = (insn & 0xFF) as u32;
                    if push {
                        let mut sp = self.regs[SP];
                        // PUSH stores the lowest-numbered register at the
                        // lowest address, so push in descending order (and LR
                        // first, as it has the highest number).
                        if insn & 0x100 != 0 {
                            sp = sp.wrapping_sub(4);
                            self.write_u32(sp, self.regs[LR]);
                        }
                        for i in (0..8).rev() {
                            if rlist & (1 << i) != 0 {
                                sp = sp.wrapping_sub(4);
                                self.write_u32(sp, self.regs[i as usize]);
                            }
                        }
                        self.regs[SP] = sp;
                    } else {
                        let mut sp = self.regs[SP];
                        for i in 0..8 {
                            if rlist & (1 << i) != 0 {
                                self.regs[i as usize] = self.read_u32(sp);
                                sp = sp.wrapping_add(4);
                            }
                        }
                        if insn & 0x100 != 0 {
                            self.regs[PC] = self.read_u32(sp) & !1;
                            sp = sp.wrapping_add(4);
                        }
                        self.regs[SP] = sp;
                    }
                }
            }
            // 0xC: multiple load/store (bit 11 = load)
            0xC => {
                let load = (insn >> 11) & 1 != 0;
                let rlist = (insn & 0xFF) as u32;
                let rn = ((insn >> 8) & 0x07) as usize;
                let base = self.regs[rn];
                let mut addr = base;
                for i in 0..8 {
                    if rlist & (1 << i) != 0 {
                        if load {
                            self.regs[i as usize] = self.read_u32(addr);
                        } else {
                            self.write_u32(addr, self.regs[i as usize]);
                        }
                        addr = addr.wrapping_add(4);
                    }
                }
                // Thumb-1 STMIA/LDMIA always use writeback. For a load, the
                // base register is only written back when it is not itself in
                // the register list (otherwise the loaded value wins).
                if !load || rlist & (1 << rn) == 0 {
                    self.regs[rn] = addr;
                }
            }
            // 0xD: conditional branch, or SVC (0xDF00-0xDFFF)
            0xD => {
                if insn & 0xFF00 == 0xDF00 {
                    self.svc();
                } else {
                    let cond = (insn >> 8) & 0x0F;
                    if self.condition_true(cond as u32) {
                        let offset = sign_extend_thumb((insn & 0xFF) as u32, 8) * 2;
                        // Architectural PC is current + 4; `step` left PC at
                        // current + 2, so add 2 to reach the branch base.
                        self.regs[PC] =
                            ((self.regs[PC].wrapping_add(2)) & !1).wrapping_add(offset as u32);
                    }
                }
            }
            // 0xE: unconditional branch (0xE000-0xEFFF)
            0xE => {
                let offset = sign_extend_thumb((insn & 0x7FF) as u32, 11) * 2;
                self.regs[PC] =
                    ((self.regs[PC].wrapping_add(2)) & !1).wrapping_add(offset as u32);
            }
            // 0xF: long branch / SVC
            0xF => {
                if insn & 0xFF00 == 0xDF00 {
                    self.svc();
                } else {
                    self.halted = true;
                }
            }
            _ => {
                self.halted = true;
            }
        }
    }

    fn set_add_carry(&mut self, a: u32, b: u32, sub: bool) {
        let (result, carry) = if sub {
            let r = (a as i64) - (b as i64);
            let c = a >= b;
            (r as u32, c)
        } else {
            let r = a.wrapping_add(b);
            let c = r < a;
            (r, c)
        };
        let _ = result;
        if carry {
            self.xpsr |= 0x2000_0000;
        } else {
            self.xpsr &= !0x2000_0000;
        }
    }

    fn condition_true(&self, cond: u32) -> bool {
        let n = self.flag_n();
        let z = self.flag_z();
        let c = self.flag_c();
        let v = self.flag_v();
        match cond {
            0 => z,                                  // EQ
            1 => !z,                                 // NE
            2 => c,                                  // CS
            3 => !c,                                 // CC
            4 => n,                                  // MI
            5 => !n,                                 // PL
            6 => v,                                  // VS
            7 => !v,                                 // VC
            8 => c && !z,                            // HI
            9 => !c || z,                            // LS
            10 => n == v,                            // GE
            11 => n != v,                            // LT
            12 => !z && n == v,                      // GT
            13 => z || n != v,                       // LE
            _ => true,
        }
    }

    fn svc(&mut self) {
        // Supervisor call: set IPSR to 11 (SVCall) — the caller observes it.
        self.set_exception(11);
    }

    fn execute_alu(&mut self, alu: u32, rd: usize, rm: usize) {
        let a = self.regs[rd];
        let b = self.regs[rm];
        // Thumb-1 ALU opcodes (Format 4, op = (insn >> 6) & 0xF).
        let (result, write, flags) = match alu {
            0 => (a & b, true, true),            // AND
            1 => (a ^ b, true, true),            // EOR
            2 => {
                // LSL Rd,Rs
                let shift = b & 0xFF;
                let (r, carry, changed) = thumb_shift_value(a, shift, 0);
                if changed {
                    self.xpsr = (self.xpsr & !0x2000_0000) | ((carry as u32) << 29);
                }
                (r, true, true)
            }
            3 => {
                // LSR Rd,Rs
                let shift = b & 0xFF;
                let (r, carry, changed) = thumb_shift_value(a, shift, 1);
                if changed {
                    self.xpsr = (self.xpsr & !0x2000_0000) | ((carry as u32) << 29);
                }
                (r, true, true)
            }
            4 => {
                // ASR Rd,Rs
                let shift = b & 0xFF;
                let (r, carry, changed) = thumb_shift_value(a, shift, 2);
                if changed {
                    self.xpsr = (self.xpsr & !0x2000_0000) | ((carry as u32) << 29);
                }
                (r, true, true)
            }
            5 => {
                // ADC Rd,Rd,Rm (with carry)
                let carry = self.flag_c() as u32;
                let (r, c) = a.overflowing_add(b);
                let (r, c2) = r.overflowing_add(carry);
                self.xpsr = (self.xpsr & !0x2000_0000) | ((c || c2) as u32) << 29;
                self.set_add_sub_v(a, b, r, false);
                (r, true, true)
            }
            6 => {
                // SBC Rd,Rd,Rm (with carry)
                let carry = if self.flag_c() { 1 } else { 0 };
                let (r, c) = a.overflowing_sub(b);
                let (r, c2) = r.overflowing_sub(carry);
                self.xpsr = (self.xpsr & !0x2000_0000) | ((c || c2) as u32) << 29;
                self.set_add_sub_v(a, b, r, true);
                (r, true, true)
            }
            7 => {
                // ROR Rd,Rs
                let shift = b & 0x1F;
                let r = if shift == 0 { a } else { a.rotate_right(shift) };
                if shift != 0 {
                    self.xpsr = (self.xpsr & !0x2000_0000)
                        | (((a >> (shift - 1)) & 1) << 29);
                }
                (r, true, true)
            }
            8 => (a & b, false, true),           // TST
            9 => {
                // NEG Rd,0,Rm
                let (r, c) = 0u32.overflowing_sub(b);
                self.xpsr = (self.xpsr & !0x2000_0000) | ((c as u32) << 29);
                self.set_add_sub_v(0, b, r, true);
                (r, true, true)
            }
            10 => {
                // CMP Rd,Rm
                let (r, c) = a.overflowing_sub(b);
                self.xpsr = (self.xpsr & !0x2000_0000) | ((c as u32) << 29);
                self.set_add_sub_v(a, b, r, true);
                (r, false, true)
            }
            11 => {
                // CMN Rd,Rm
                let (r, c) = a.overflowing_add(b);
                self.xpsr = (self.xpsr & !0x2000_0000) | ((c as u32) << 29);
                self.set_add_sub_v(a, b, r, false);
                (r, false, true)
            }
            12 => (a | b, true, true),           // ORR
            13 => {
                // MUL Rd,Rm
                let r = a.wrapping_mul(b);
                (r, true, true)
            }
            14 => (a & !b, true, true),          // BIC
            _ => (!a, true, true),               // MVN
        };
        if flags {
            self.set_nz(result);
        }
        if write {
            self.regs[rd] = result;
        }
    }

    fn execute_wide(&mut self, insn: u32) {
        // Thumb-2 wide encodings. `first` is the high halfword, `second` the
        // low. Only the forms a bare-metal Cortex-M toolchain actually emits
        // are decoded; anything else halts so the caller surfaces it.
        let first = (insn >> 16) as u16;
        let second = insn as u16;
        let pc = self.regs[PC];

        // BL/BLX immediate (0xF000-0xF7FF prefix, second bits 14-12 = 101).
        if first & 0xF800 == 0xF000 && second & 0xD000 == 0xD000 {
            let s = ((first >> 10) & 1) as u32;
            let j1 = ((second >> 13) & 1) as u32;
            let j2 = ((second >> 11) & 1) as u32;
            let imm10 = (first & 0x3FF) as u32;
            let imm11 = (second & 0x7FF) as u32;
            let mut imm32 = (s << 24) | (j2 << 23) | (j1 << 22) | (imm10 << 12) | (imm11 << 1);
            imm32 = sign_extend_thumb(imm32, 25);
            let link = second & 0x4000 != 0; // bit 14: BL vs BLX
            if link {
                self.regs[LR] = (pc | 1) & !1;
            }
            self.regs[PC] = pc.wrapping_add(imm32 as u32) & !1;
            self.set_thumb(true);
            return;
        }

        // MOVW/MOVT (0xF240-0xF2FF, second bits 15-12 = 0100).
        if first & 0xFBF0 == 0xF240 && second & 0xF000 == 0x4000 {
            let rd = ((second >> 8) & 0x0F) as usize;
            let imm16 = ((first & 0x000F) as u32) << 12
                | ((first & 0x0F00) as u32)
                | ((second & 0x00FF) as u32);
            let top = second & 0x0800 != 0; // MOVT when bit 11 set
            if top {
                self.regs[rd] = (self.regs[rd] & 0x0000_FFFF) | (imm16 << 16);
            } else {
                self.regs[rd] = (self.regs[rd] & 0xFFFF_0000) | imm16;
            }
            return;
        }

        // MUL Rd, Rn, Rm (0xFB00 prefix, second bits 15-12 = 1111, bits 7-4 = 0).
        if first & 0xFFF0 == 0xFB00 && second & 0xF0F0 == 0xF000 {
            let rn = (first & 0x0F) as usize;
            let rd = ((second >> 8) & 0x0F) as usize;
            let rm = (second & 0x0F) as usize;
            self.regs[rd] = self.regs[rn].wrapping_mul(self.regs[rm]);
            return;
        }

        // SDIV/UDIV Rd, Rn, Rm (0xFB90/0xFBB0 prefix, second bits 15-12 = 1111,
        // bits 7-4 = 1111). Division by zero yields 0 on Cortex-M.
        if (first & 0xFFF0 == 0xFB90 || first & 0xFFF0 == 0xFBB0)
            && second & 0xF0F0 == 0xF0F0
        {
            let rn = (first & 0x0F) as usize;
            let rd = ((second >> 8) & 0x0F) as usize;
            let rm = (second & 0x0F) as usize;
            let n = self.regs[rn];
            let d = self.regs[rm];
            let signed = first & 0xFFF0 == 0xFB90; // 0xFB90 = SDIV, 0xFBB0 = UDIV
            self.regs[rd] = if d == 0 {
                0
            } else if signed {
                ((n as i32).wrapping_div(d as i32)) as u32
            } else {
                n.wrapping_div(d)
            };
            return;
        }

        // ADDW/SUBW (0xF200-0xF2FF, second bits 14-12 = 010, bit 15 = 0).
        if first & 0xFBF0 == 0xF200 && second & 0xF000 == 0x2000 {
            let rd = ((second >> 8) & 0x0F) as usize;
            let rn = (first & 0x000F) as usize;
            let imm12 = ((first & 0x0F00) as u32) << 4 | ((second & 0x00FF) as u32);
            let sub = second & 0x0400 != 0; // bit 10
            if sub {
                self.regs[rd] = self.regs[rn].wrapping_sub(imm12);
            } else {
                self.regs[rd] = self.regs[rn].wrapping_add(imm12);
            }
            return;
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
            imm32 = sign_extend_thumb(imm32, 25);
            self.regs[PC] = pc.wrapping_add(imm32 as u32) & !1;
            self.set_thumb(true);
            return;
        }

        // LDM/STM (multiple), T32: bits 15-12 = 1110, bits 11-9 = 100,
        // bit 6 = 0 (this distinguishes it from the 0xF8xx loads/stores).
        if first & 0xF000 == 0xE000 && first & 0x0E40 == 0x0800 {
            let rn = (first & 0x0F) as usize;
            let list = second as u32;
            let count = list.count_ones();
            let load = first & 0x0010 != 0; // L bit
            let wb = first & 0x0020 != 0; // W bit
            let pre = first & 0x0100 != 0; // P bit
            let up = first & 0x0080 != 0; // U bit
            if count == 0 {
                self.halted = true;
                return;
            }
            let base = self.regs[rn];
            let start = match (up, pre) {
                (true, false) => base,                            // IA
                (true, true) => base.wrapping_add(4),             // IB
                (false, false) => base.wrapping_sub(4 * count).wrapping_add(4), // DA
                (false, true) => base.wrapping_sub(4 * count),    // DB
            };
            let end = if up {
                base.wrapping_add(4 * count)
            } else {
                base.wrapping_sub(4 * count)
            };
            let mut address = start;
            if !load {
                for index in 0..16 {
                    if list & (1 << index) == 0 {
                        continue;
                    }
                    self.write_u32(address & !3, self.regs[index]);
                    address = address.wrapping_add(4);
                }
                if wb && rn != PC {
                    self.regs[rn] = end;
                }
                return;
            }
            for index in 0..16 {
                if list & (1 << index) == 0 {
                    continue;
                }
                let value = self.read_u32(address & !3);
                if index == PC {
                    self.branch_exchange_value(value);
                } else {
                    self.regs[index] = value;
                }
                address = address.wrapping_add(4);
            }
            if wb && rn != PC && list & (1 << rn) == 0 {
                self.regs[rn] = end;
            }
            return;
        }

        // TBB/TBH (0xE8D0-0xE8DF prefix, second bits 15-12 = 1111): table
        // branch. TBB reads a byte index at [Rn, Rm]; TBH reads a halfword
        // index at [Rn, Rm, LSL #1]. The target is Align(PC, 4) + 2*index.
        if first & 0xFFF0 == 0xE8D0 && second & 0xF000 == 0xF000 {
            let rn = (first & 0x0F) as usize;
            let rm = (second & 0x0F) as usize;
            let table_addr = if second & 0x0010 != 0 {
                self.regs[rn].wrapping_add(self.regs[rm].wrapping_shl(1))
            } else {
                self.regs[rn].wrapping_add(self.regs[rm])
            };
            let index = if second & 0x0010 != 0 {
                (self.read_u16(table_addr) as u32) << 1
            } else {
                (self.read_u8(table_addr) as u32) << 1
            };
            // `pc` is the already-advanced next-instruction address (current+4);
            // ARMv7-M defines PC = current+4 in Thumb state, so the TBB/TBH
            // table base is Align(PC, 4) = Align(current+4, 4).
            self.regs[PC] = (pc & !3).wrapping_add(index);
            self.set_thumb(true);
            return;
        }

        // LDREX/STREX (exclusive access). Bits 11-8 = 1000, bit 5 = 0, and
        // bits 7-6 are 01 (word) or 11 (byte/half). Word forms: second bits
        // 7-0 = 0, Rt = bits 15-12, and the status register is bits 11-8
        // (store) or unused (load). Byte/half forms: second bits 7-4 =
        // 0100 (byte) / 0101 (half), bits 11-8 = 1111, Rt = bits 15-12, and
        // the status register is bits 3-0 (store) or unused (load). On the
        // single-core Cortex-M the exclusive monitor always grants, so STREX
        // writes and records success (Rd = 0).
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
            // The status register is bits 11-8 for word stores and bits 3-0
            // for byte/half stores; loads do not use it.
            let rd = if is_store && byte_half {
                (second & 0x0F) as usize
            } else if is_store {
                ((second >> 8) & 0x0F) as usize
            } else {
                0
            };
            let addr = self.regs[rn];
            if is_store {
                let value = self.regs[rt];
                match size {
                    0 => self.write_u8(addr, value as u8),
                    1 => self.write_u16(addr, value as u16),
                    _ => self.write_u32(addr, value),
                }
                self.regs[rd] = 0; // exclusive store succeeds
            } else {
                let value = match size {
                    0 => self.read_u8(addr) as u32,
                    1 => self.read_u16(addr) as u32,
                    _ => self.read_u32(addr),
                };
                self.regs[rt] = value;
            }
            return;
        }

        // LDRD/STRD (bits 15-9 = 1110100, bit 6 = 1, second bits 15-12 is the
        // first of a register pair, so != 1111): doubleword transfer with an
        // 8-bit scaled immediate offset.
        if first & 0xFE40 == 0xE840 && second & 0xF000 != 0xF000 {
            let rn = (first & 0x0F) as usize;
            let rt = ((second >> 12) & 0x0F) as usize;
            let rt2 = ((second >> 8) & 0x0F) as usize;
            let imm8 = (second & 0xFF) as u32;
            let pre = first & 0x0100 != 0; // P
            let up = first & 0x0080 != 0; // U
            let wb = first & 0x0020 != 0; // W
            let load = first & 0x0010 != 0; // L
            let offset = imm8.wrapping_mul(4);
            let base = self.regs[rn];
            let addr = if up {
                base.wrapping_add(offset)
            } else {
                base.wrapping_sub(offset)
            };
            let effective = if pre { addr } else { base };
            if load {
                let v0 = self.read_u32(effective);
                let v1 = self.read_u32(effective.wrapping_add(4));
                if rt == PC {
                    self.branch_exchange_value(v0);
                } else {
                    self.regs[rt] = v0;
                }
                if rt2 == PC {
                    self.branch_exchange_value(v1);
                } else {
                    self.regs[rt2] = v1;
                }
            } else {
                self.write_u32(effective, self.regs[rt]);
                self.write_u32(effective.wrapping_add(4), self.regs[rt2]);
            }
            if (wb || !pre) && rn != PC && !(load && (rn == rt || rn == rt2)) {
                self.regs[rn] = addr;
            }
            return;
        }

        // LDR (literal): LDR Rt, [PC, #imm12] (0xF8DF). The base is the
        // architectural PC (current + 4) aligned down to a word.
        if first == 0xF8DF {
            let rd = ((second >> 12) & 0x0F) as usize;
            let imm12 = (second & 0x0FFF) as u32;
            let addr = (pc & !3).wrapping_add(imm12);
            self.regs[rd] = self.read_u32(addr);
            return;
        }

        // LDR/STR (immediate 12-bit, 0xF8D0-0xF8DF / 0xF8C0-0xF8CF).
        if first & 0xFFF0 == 0xF8D0 || first & 0xFFF0 == 0xF8C0 {
            let load = first & 0x0010 != 0; // LDR when bit 4 of first set
            let rn = (first & 0x000F) as usize;
            let rd = ((second >> 12) & 0x0F) as usize;
            let imm12 = (second & 0x0FFF) as u32;
            let addr = self.regs[rn].wrapping_add(imm12);
            if load {
                self.regs[rd] = self.read_u32(addr);
            } else {
                self.write_u32(addr, self.regs[rd]);
            }
            return;
        }

        // LDRB (literal): LDRB Rt, [PC, #imm12] (0xF89F).
        if first == 0xF89F {
            let rd = ((second >> 12) & 0x0F) as usize;
            let imm12 = (second & 0x0FFF) as u32;
            let addr = (pc & !3).wrapping_add(imm12);
            self.regs[rd] = self.read_u8(addr) as u32;
            return;
        }

        // LDRB/STRB (0xF890-0xF89F / 0xF880-0xF88F).
        if first & 0xFFF0 == 0xF890 || first & 0xFFF0 == 0xF880 {
            let load = first & 0x0010 != 0;
            let rn = (first & 0x000F) as usize;
            let rd = ((second >> 12) & 0x0F) as usize;
            let imm12 = (second & 0x0FFF) as u32;
            let addr = self.regs[rn].wrapping_add(imm12);
            if load {
                self.regs[rd] = self.read_u8(addr) as u32;
            } else {
                self.write_u8(addr, self.regs[rd] as u8);
            }
            return;
        }

        // LDRH (literal): LDRH Rt, [PC, #imm12] (0xF8BF).
        if first == 0xF8BF {
            let rd = ((second >> 12) & 0x0F) as usize;
            let imm12 = (second & 0x0FFF) as u32;
            let addr = (pc & !3).wrapping_add(imm12);
            self.regs[rd] = self.read_u16(addr) as u32;
            return;
        }

        // LDRH/STRH (0xF8B0-0xF8BF / 0xF8A0-0xF8AF).
        if first & 0xFFF0 == 0xF8B0 || first & 0xFFF0 == 0xF8A0 {
            let load = first & 0x0010 != 0;
            let rn = (first & 0x000F) as usize;
            let rd = ((second >> 12) & 0x0F) as usize;
            let imm12 = (second & 0x0FFF) as u32;
            let addr = self.regs[rn].wrapping_add(imm12);
            if load {
                self.regs[rd] = self.read_u16(addr) as u32;
            } else {
                self.write_u16(addr, self.regs[rd] as u16);
            }
            return;
        }

        // Register-offset loads/stores (0xF800-0xF85F): STRB/LDRB/STRH/LDRH/
        // STR/LDR with [Rn, Rm {, LSL #imm2}]. Width is bits 7-5 of the first
        // halfword (0=byte, 1=half, 2=word); bit 4 selects load.
        if first & 0xFFF0 >= 0xF800 && first & 0xFFF0 <= 0xF85F && second & 0x0FC0 == 0 {
            let rn = (first & 0x000F) as usize;
            let rt = ((second >> 12) & 0x0F) as usize;
            let rm = (second & 0x000F) as usize;
            let shift = ((second >> 4) & 0x03) as u32;
            let load = first & 0x0010 != 0;
            let addr = self.regs[rn].wrapping_add(self.regs[rm].wrapping_shl(shift));
            let width = (first >> 5) & 0x03;
            match (width, load) {
                (0, false) => self.write_u8(addr, self.regs[rt] as u8),
                (0, true) => self.regs[rt] = self.read_u8(addr) as u32,
                (1, false) => self.write_u16(addr, self.regs[rt] as u16),
                (1, true) => self.regs[rt] = self.read_u16(addr) as u32,
                (_, false) => self.write_u32(addr, self.regs[rt]),
                (_, true) => self.regs[rt] = self.read_u32(addr),
            }
            return;
        }

        // LDRSB/LDRSH wide forms. Register offset: first = 0xF910|Rn /
        // 0xF930|Rn (bit 7 = 0), second = `Rt 0000 00shift Rm`. Immediate and
        // PC-relative literal (Rn = PC): first = 0xF990|Rn / 0xF9B0|Rn
        // (bit 7 = 1), second = `Rt imm12`.
        let ldr_sb_reg = first & 0xFFF0 == 0xF910 && second & 0x0FC0 == 0;
        let ldr_sh_reg = first & 0xFFF0 == 0xF930 && second & 0x0FC0 == 0;
        let ldr_sb_imm = first & 0xFFF0 == 0xF990;
        let ldr_sh_imm = first & 0xFFF0 == 0xF9B0;
        if ldr_sb_reg || ldr_sh_reg || ldr_sb_imm || ldr_sh_imm {
            let rt = ((second >> 12) & 0x0F) as usize;
            let rn = (first & 0x0F) as usize;
            let is_reg = first & 0x0080 == 0;
            let addr = if is_reg {
                let rm = (second & 0x0F) as usize;
                let shift = ((second >> 4) & 0x03) as u32;
                self.regs[rn].wrapping_add(self.regs[rm].wrapping_shl(shift))
            } else {
                let imm12 = (second & 0x0FFF) as u32;
                let base = if rn == PC { pc & !3 } else { self.regs[rn] };
                base.wrapping_add(imm12)
            };
            self.regs[rt] = if ldr_sb_reg || ldr_sb_imm {
                self.read_u8(addr) as i8 as i32 as u32
            } else {
                self.read_u16(addr) as i16 as i32 as u32
            };
            return;
        }

        // Memory barriers DMB/DSB/ISB (0xF3BF 0x8F5F/8F6F/8F7F): no-ops here.
        if first == 0xF3BF && second & 0xFFF0 >= 0x8F50 && second & 0xFFF0 <= 0x8F70 {
            return;
        }

        // MRS Rd, <apsr|ipsr|epsr|xpsr> (0xF3EF 0x8Rd0SYSm): Rd = bits 11-8
        // of the second halfword, the special register = bits 3-0.
        if first == 0xF3EF && second & 0x00F0 == 0x0000 {
            let rd = ((second >> 8) & 0x0F) as usize;
            let special = (second & 0x0F) as u32;
            let v = self.special_register(special);
            self.regs[rd] = v;
            return;
        }
        // MSR <apsr|xpsr>_<mask>, Rm (0xF380-0xF38F 0x8maskSYSm): the source
        // register is bits 3-0 of the first halfword, the special register is
        // bits 3-0 of the second, and the mask selects which fields to write.
        if first & 0xFFF0 == 0xF380 && second & 0x00F0 == 0x0000 {
            let special = second & 0x0F;
            let mask = ((second >> 8) & 0x0F) as u32;
            let rm = (first & 0x0F) as usize;
            let value = self.regs[rm];
            if special == 3 || special == 0 {
                // xPSR / APSR: flags (mask 8) and GE (mask 4) are writable.
                if mask & 0x8 != 0 {
                    self.arm_msr(0x8, value);
                }
                if mask & 0x4 != 0 {
                    self.xpsr = (self.xpsr & !0x000F_0000) | (value & 0x000F_0000);
                }
            }
            return;
        }

        // CLZ Rd, Rm (0xFAB0-0xFABF 0xF0Rd0087).
        if first & 0xFFF0 == 0xFAB0 && second & 0x00FF == 0x0087 {
            let rd = ((second >> 8) & 0x0F) as usize;
            let rm = (first & 0x0F) as usize;
            self.regs[rd] = self.regs[rm].leading_zeros();
            return;
        }

        // REV/REV16/REVSH/RBIT wide forms. First halfword is
        // `1111 1010 1001 Rm` (0xFA90-0xFA9F); second halfword is
        // `1111 Rd 10op2 Rm`, where bits 7-4 select the operation and Rm is
        // duplicated in both halfwords' low nibble.
        if first & 0xFFF0 == 0xFA90 && second & 0xF000 == 0xF000 {
            let rd = ((second >> 8) & 0x0F) as usize;
            let rm = (first & 0x0F) as usize;
            let v = self.regs[rm];
            let r = match (second >> 4) & 0x0F {
                0x8 => v.swap_bytes(), // REV
                0x9 => {
                    // REV16: swap the bytes of each halfword.
                    (v & 0x00FF_00FF).rotate_left(8) | ((v & 0xFF00_FF00) >> 8)
                }
                0xA => v.reverse_bits(), // RBIT
                _ => {
                    // REVSH: reverse the low halfword bytes, sign-extend.
                    let low = (v as u16).swap_bytes();
                    low as i16 as i32 as u32
                }
            };
            self.regs[rd] = r;
            return;
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
                let rotate = ((second >> 4) & 0x03) as u32 * 8;
                let v = self.regs[rm].rotate_right(rotate);
                let r = match op {
                    0 => v as u16 as i16 as i32 as u32, // SXTH
                    1 => (v as u16) as u32,             // UXTH
                    4 => v as u8 as i8 as i32 as u32,   // SXTB
                    _ => (v as u8) as u32,              // UXTB
                };
                self.regs[rd] = r;
                return;
            }
        }

        // Bit-field extract/insert/clear: SBFX (0xF340), BFI (0xF360 with
        // Rn != PC), BFC (0xF36F), UBFX (0xF3C0). Second halfword layout is
        // `0 imm3(3) Rd(4) imm2(2) 0 lsb(5)`; the field starts at
        // `lsb = (imm3 << 2) | imm2` and has `width = lsb5 + 1` bits.
        if second & 0x0020 == 0
            && (first & 0xFFF0 == 0xF340 || first & 0xFFF0 == 0xF360
                || first == 0xF36F || first & 0xFFF0 == 0xF3C0)
        {
            let rn = (first & 0x0F) as usize;
            let rd = ((second >> 8) & 0x0F) as usize;
            let lsb = ((((second >> 12) & 0x7) as u32) << 2) | ((second >> 6) & 0x3) as u32;
            let sbfx = first & 0xFFF0 == 0xF340;
            let ubfx = first & 0xFFF0 == 0xF3C0;
            // SBFX/UBFX encode the width as `lsb5 + 1`; BFC/BFI encode the
            // field's most-significant bit in `lsb5` (width = msb - lsb + 1).
            let width = if sbfx || ubfx {
                (second & 0x1F) as u32 + 1
            } else {
                (second & 0x1F) as u32 + 1 - lsb
            };
            let mask = if width >= 32 {
                0xFFFF_FFFF
            } else {
                (1u32 << width) - 1
            };
            if sbfx || ubfx {
                let v = (self.regs[rn] >> lsb) & mask;
                let result = if sbfx && width < 32 && (v & (1u32 << (width - 1))) != 0 {
                    v | !mask
                } else {
                    v
                };
                self.regs[rd] = result;
            } else {
                let clear = !(mask << lsb);
                if first == 0xF36F {
                    // BFC: clear the field in Rd.
                    self.regs[rd] &= clear;
                } else {
                    // BFI: insert the low `width` bits of Rn into Rd at lsb.
                    self.regs[rd] = (self.regs[rd] & clear) | ((self.regs[rn] & mask) << lsb);
                }
            }
            return;
        }

        // Data processing (register): AND/BIC/ORR/ORN/EOR/ADD/ADC/SBC/SUB/RSB
        // plus MOV/MVN/TST/TEQ/CMP/CMN. Bits 15-9 = 1110101.
        if first & 0xFE00 == 0xEA00 {
            self.thumb2_data_processing(first, second, pc);
            return;
        }

        // Data processing (modified immediate): AND/BIC/ORR/ORN/EOR/ADD/SUB/
        // RSB plus MOV/MVN/TST/TEQ/CMP/CMN. Bits 15-11 = 11110, bit 9 = 0.
        if first & 0xFA00 == 0xF000 {
            self.thumb2_data_processing_imm(first, second, pc);
            return;
        }

        self.halted = true;
    }
}

impl ArmCore {
    /// ARM state (A32) condition-code evaluation.
    fn condition_passed(&self, cond: u32) -> bool {
        match cond {
            0x0 => self.flag_z(),
            0x1 => !self.flag_z(),
            0x2 => self.flag_c(),
            0x3 => !self.flag_c(),
            0x4 => self.flag_n(),
            0x5 => !self.flag_n(),
            0x6 => self.flag_v(),
            0x7 => !self.flag_v(),
            0x8 => self.flag_c() && !self.flag_z(),
            0x9 => !self.flag_c() || self.flag_z(),
            0xA => self.flag_n() == self.flag_v(),
            0xB => self.flag_n() != self.flag_v(),
            0xC => !self.flag_z() && self.flag_n() == self.flag_v(),
            0xD => self.flag_z() || self.flag_n() != self.flag_v(),
            0xE => true,
            // 0xF is the pre-ARMv5 "never" condition.
            _ => false,
        }
    }

    /// Read an ARM-state register, substituting the pipeline-adjusted PC
    /// value for R15 (reads of PC see the instruction address + 8).
    fn arm_reg(&self, index: usize, pc_base: u32) -> u32 {
        if index == PC { pc_base } else { self.regs[index] }
    }

    fn set_nzcv(&mut self, n: bool, z: bool, c: bool, v: bool) {
        let mut value = self.xpsr & 0x0FFF_FFFF;
        if n {
            value |= 0x8000_0000;
        }
        if z {
            value |= 0x4000_0000;
        }
        if c {
            value |= 0x2000_0000;
        }
        if v {
            value |= 0x1000_0000;
        }
        self.xpsr = value;
    }

    /// Branch through a value that may select Thumb state via bit 0 (BX/BLX
    /// and `LDR pc, ...` on ARMv5T).
    fn branch_exchange_value(&mut self, target: u32) {
        let thumb = target & 1 != 0;
        self.arm_state = !thumb;
        self.set_thumb(thumb);
        self.regs[PC] = target & !1;
    }

    fn arm_add(a: u32, b: u32, carry: bool) -> (u32, bool, bool) {
        let sum = a as u64 + b as u64 + carry as u64;
        let result = sum as u32;
        let c = sum > 0xFFFF_FFFF;
        let v = (a ^ result) & (b ^ result) & 0x8000_0000 != 0;
        (result, c, v)
    }

    /// `a - b - !C` (the carry input is the ARM C flag; a set C means no
    /// borrow). Returns `(result, carry_out, overflow)`.
    fn arm_sub(a: u32, b: u32, carry: bool) -> (u32, bool, bool) {
        let borrow = if carry { 0u64 } else { 1u64 };
        let diff = a as u64 as i64 - b as i64 - borrow as i64;
        let result = diff as u32;
        let c = diff >= 0;
        let v = (a ^ b) & (a ^ result) & 0x8000_0000 != 0;
        (result, c, v)
    }

    /// Compute the (shifted) operand 2 and the shifter carry-out.
    fn arm_operand2(&self, insn: u32, pc_base: u32) -> (u32, bool) {
        if insn & (1 << 25) != 0 {
            let imm = insn & 0xFF;
            let rotate = ((insn >> 8) & 0x0F) * 2;
            return (imm.rotate_right(rotate), self.flag_c());
        }
        let rm = (insn & 0x0F) as usize;
        let value = self.arm_reg(rm, pc_base);
        let shift_type = (insn >> 5) & 0x03;
        let (amount, by_register) = if insn & 0x10 != 0 {
            let rs = ((insn >> 8) & 0x0F) as usize;
            (self.arm_reg(rs, pc_base) & 0xFF, true)
        } else {
            ((insn >> 7) & 0x1F, false)
        };
        if !by_register && amount == 0 {
            if shift_type == 3 {
                // RRX: rotate right by one through the carry flag.
                let carry_in = if self.flag_c() { 0x8000_0000 } else { 0 };
                return ((value >> 1) | carry_in, value & 1 != 0);
            }
            return (value, self.flag_c());
        }
        let carry_in = self.flag_c();
        match shift_type {
            0 => shift_lsl(value, amount, carry_in),
            1 => shift_lsr(value, amount, carry_in),
            2 => shift_asr(value, amount, carry_in),
            _ => shift_ror(value, amount, carry_in),
        }
    }

    /// Decode and execute a 32-bit ARM-state (A32) instruction.
    fn execute_arm(&mut self, insn: u32) {
        let cond = insn >> 28;
        if !self.condition_passed(cond) {
            return;
        }
        // PC reads see instruction address + 8; `step` already advanced PC by
        // 4, so this is one word ahead of the current PC register.
        let pc_base = self.regs[PC].wrapping_add(4);

        // Branch and branch-with-link: 101x.
        if insn & 0x0E00_0000 == 0x0A00_0000 {
            let link = insn & (1 << 24) != 0;
            let offset = sign_extend_thumb(insn & 0x00FF_FFFF, 24) << 2;
            let target = pc_base.wrapping_add(offset);
            if link {
                self.regs[LR] = self.regs[PC];
            }
            self.regs[PC] = target & !3;
            return;
        }
        // BX / BLX (register).
        if insn & 0x0FFF_FFF0 == 0x012F_FF10 || insn & 0x0FFF_FFF0 == 0x012F_FF30 {
            let rm = (insn & 0x0F) as usize;
            let target = self.arm_reg(rm, pc_base);
            if insn & 0x20 != 0 {
                self.regs[LR] = self.regs[PC];
            }
            self.branch_exchange_value(target);
            return;
        }
        // Block data transfer: 100x.
        if insn & 0x0E00_0000 == 0x0800_0000 {
            self.arm_block_transfer(insn, pc_base);
            return;
        }
        // Single data transfer (word/byte): 01xx.
        if insn & 0x0C00_0000 == 0x0400_0000 {
            self.arm_single_transfer(insn, pc_base);
            return;
        }
        // MSR (immediate) and MOVW/MOVT live in the data-processing space.
        if insn & 0x0FF0_0000 == 0x0300_0000 || insn & 0x0FF0_0000 == 0x0340_0000 {
            // MOVW (bit 22 clear) / MOVT (bit 22 set).
            let rd = ((insn >> 12) & 0x0F) as usize;
            let imm16 = ((insn >> 4) & 0xF000) | (insn & 0x0FFF);
            if insn & 0x0040_0000 == 0 {
                self.regs[rd] = imm16;
            } else {
                self.regs[rd] = (self.regs[rd] & 0x0000_FFFF) | (imm16 << 16);
            }
            return;
        }
        if insn & 0x0FBF_0FFF == 0x010F_0000 {
            // MRS Rd, CPSR
            let rd = ((insn >> 12) & 0x0F) as usize;
            self.regs[rd] = self.xpsr;
            return;
        }
        if insn & 0x0FB0_FFF0 == 0x0120_F000 {
            // MSR CPSR_<fields>, Rm
            let rm = (insn & 0x0F) as usize;
            let fields = (insn >> 16) & 0x0F;
            self.arm_msr(fields, self.arm_reg(rm, pc_base));
            return;
        }
        if insn & 0x0FB0_F000 == 0x0320_F000 {
            // MSR CPSR_<fields>, #imm
            let fields = (insn >> 16) & 0x0F;
            let imm = (insn & 0xFF).rotate_right(((insn >> 8) & 0x0F) * 2);
            self.arm_msr(fields, imm);
            return;
        }
        // Multiply / multiply-accumulate: bits 27-22 = 000000, bits 7-4 = 1001.
        if insn & 0x0FC0_00F0 == 0x0000_0090 {
            let accumulate = insn & (1 << 21) != 0;
            let set_flags = insn & (1 << 20) != 0;
            let rd = ((insn >> 16) & 0x0F) as usize;
            let rn = ((insn >> 12) & 0x0F) as usize;
            let rs = ((insn >> 8) & 0x0F) as usize;
            let rm = (insn & 0x0F) as usize;
            let mut result = self.arm_reg(rm, pc_base).wrapping_mul(self.arm_reg(rs, pc_base));
            if accumulate {
                result = result.wrapping_add(self.arm_reg(rn, pc_base));
            }
            if set_flags {
                self.set_nz(result);
            }
            self.regs[rd] = result;
            return;
        }
        // Long multiply: 00001 U A S.
        if insn & 0x0F80_00F0 == 0x0080_0090 {
            let signed = insn & (1 << 22) != 0;
            let accumulate = insn & (1 << 21) != 0;
            let set_flags = insn & (1 << 20) != 0;
            let rdhi = ((insn >> 16) & 0x0F) as usize;
            let rdlo = ((insn >> 12) & 0x0F) as usize;
            let rs = ((insn >> 8) & 0x0F) as usize;
            let rm = (insn & 0x0F) as usize;
            let (hi, lo) = if signed {
                let product = self.arm_reg(rm, pc_base) as i32 as i64
                    * self.arm_reg(rs, pc_base) as i32 as i64;
                ((product >> 32) as u32, product as u32)
            } else {
                let product =
                    self.arm_reg(rm, pc_base) as u64 * self.arm_reg(rs, pc_base) as u64;
                ((product >> 32) as u32, product as u32)
            };
            let (mut hi, mut lo) = (hi, lo);
            if accumulate {
                let sum = ((self.regs[rdhi] as u64) << 32)
                    .wrapping_add(self.regs[rdlo] as u64)
                    .wrapping_add(((hi as u64) << 32) | lo as u64);
                hi = (sum >> 32) as u32;
                lo = sum as u32;
            }
            self.regs[rdlo] = lo;
            self.regs[rdhi] = hi;
            if set_flags {
                self.set_nzcv(hi & 0x8000_0000 != 0, hi == 0 && lo == 0, self.flag_c(), self.flag_v());
            }
            return;
        }
        // Halfword / signed transfer: 000 P U I W L, bit 7 = 1, bit 4 = 1.
        if insn & 0x0E00_0090 == 0x0000_0090 && insn & 0x60 != 0 {
            self.arm_halfword_transfer(insn, pc_base);
            return;
        }
        // CLZ Rd, Rm (0x016F0F10).
        if insn & 0x0FFF_0FF0 == 0x016F_0F10 {
            let rd = ((insn >> 12) & 0x0F) as usize;
            let rm = (insn & 0x0F) as usize;
            self.regs[rd] = self.arm_reg(rm, pc_base).leading_zeros();
            return;
        }
        // Supervisor call.
        if insn & 0x0F00_0000 == 0x0F00_0000 {
            self.set_exception(11);
            self.halted = true;
            return;
        }
        // Data processing (register/immediate) with optional flag update.
        if insn & 0x0C00_0000 == 0 {
            self.arm_data_processing(insn, pc_base);
            return;
        }
        // SWP/SWPB (single word swap) is rare in compiler output; halt
        // honestly rather than mis-executing.
        self.halted = true;
    }

    fn arm_msr(&mut self, fields: u32, value: u32) {
        // Only the application/status flag byte (field mask 8) is modelled; the
        // processor mode/control byte is kept as-is.
        if fields & 0x8 != 0 {
            self.set_nzcv(
                value & 0x8000_0000 != 0,
                value & 0x4000_0000 != 0,
                value & 0x2000_0000 != 0,
                value & 0x1000_0000 != 0,
            );
        }
    }

    /// Read an ARM special register selected by the MRS/MSR SYSm field:
    /// 0 = APSR (flags + GE), 1/5 = IPSR, 2/6 = EPSR, 3 = xPSR.
    fn special_register(&self, special: u32) -> u32 {
        match special {
            0 => (self.xpsr & 0xF000_0000) | (self.xpsr & 0x000F_0000), // APSR
            1 | 5 => self.xpsr & 0x1FF,                                 // IPSR
            2 | 6 => (self.xpsr & 0x0700_0000) | (self.xpsr & 0x0000_FC00), // EPSR
            3 => self.xpsr,                                             // xPSR
            _ => 0,
        }
    }

    fn arm_data_processing(&mut self, insn: u32, pc_base: u32) {
        let opcode = (insn >> 21) & 0x0F;
        let set_flags = insn & (1 << 20) != 0;
        let rn = ((insn >> 16) & 0x0F) as usize;
        let rd = ((insn >> 12) & 0x0F) as usize;
        let (b, shifter_carry) = self.arm_operand2(insn, pc_base);
        let a = self.arm_reg(rn, pc_base);
        let compare_only = (8..=11).contains(&opcode);
        let mut flags: Option<(bool, bool, bool, bool)> = None;
        let result = match opcode {
            0 => a & b,                              // AND
            1 => a ^ b,                              // EOR
            2 | 3 => {
                let (r, c, v) = if opcode == 2 {
                    Self::arm_sub(a, b, true)
                } else {
                    Self::arm_sub(b, a, true)
                };
                flags = Some((r & 0x8000_0000 != 0, r == 0, c, v));
                r
            }
            4 | 5 | 6 | 7 => {
                let (r, c, v) = match opcode {
                    4 => Self::arm_add(a, b, false),
                    5 => Self::arm_add(a, b, self.flag_c()),
                    6 => Self::arm_sub(a, b, self.flag_c()),
                    _ => Self::arm_sub(b, a, self.flag_c()),
                };
                flags = Some((r & 0x8000_0000 != 0, r == 0, c, v));
                r
            }
            8 => {
                // TST
                let r = a & b;
                flags = Some((r & 0x8000_0000 != 0, r == 0, shifter_carry, self.flag_v()));
                r
            }
            9 => {
                // TEQ
                let r = a ^ b;
                flags = Some((r & 0x8000_0000 != 0, r == 0, shifter_carry, self.flag_v()));
                r
            }
            10 => {
                // CMP
                let (r, c, v) = Self::arm_sub(a, b, true);
                flags = Some((r & 0x8000_0000 != 0, r == 0, c, v));
                r
            }
            11 => {
                // CMN
                let (r, c, v) = Self::arm_add(a, b, false);
                flags = Some((r & 0x8000_0000 != 0, r == 0, c, v));
                r
            }
            12 => a | b,                            // ORR
            13 => b,                                // MOV
            14 => a & !b,                           // BIC
            _ => !b,                                // MVN
        };
        if (set_flags || compare_only) && flags.is_none() {
            // Logical operations only update N/Z from the result, C from the
            // shifter and leave V untouched.
            flags = Some((result & 0x8000_0000 != 0, result == 0, shifter_carry, self.flag_v()));
        }
        if set_flags || compare_only {
            if let Some((n, z, c, v)) = flags {
                self.set_nzcv(n, z, c, v);
            }
        }
        if !compare_only {
            if rd == PC {
                self.branch_exchange_value(result);
            } else {
                self.regs[rd] = result;
            }
        }
    }

    /// Execute a Thumb-2 data-processing (register) instruction. `first` and
    /// `second` are the two halfwords of the 32-bit encoding; `pc` is the
    /// architectural PC (current instruction + 4) already advanced by `step`.
    fn thumb2_data_processing(&mut self, first: u16, second: u16, pc: u32) {
        // op = (bit8 << 3) | bits7-5; bit4 = S; bits3-0 = Rn.
        let op = (((first >> 8) & 1) << 3) | ((first >> 5) & 0x7);
        let set_flags = first & 0x0010 != 0;
        let rn = (first & 0x0F) as usize;
        // MOV/MVN (ORR/ORN with Rn=PC) place Rd in bits 3-0 and Rm in bits
        // 11-8, which is the reverse of the general data-processing layout.
        let is_mov = op == 2 && rn == PC;
        let is_mvn = op == 3 && rn == PC;
        let (rd, rm) = if is_mov || is_mvn {
            ((second & 0x0F) as usize, ((second >> 8) & 0x0F) as usize)
        } else {
            (((second >> 8) & 0x0F) as usize, (second & 0x0F) as usize)
        };
        let imm2 = ((second >> 6) & 0x03) as u32;
        let shift_type = ((second >> 4) & 0x03) as u32;
        // PC operand (Rn) reads as Align(PC, 4).
        let pc_val = pc & !3;
        let rn_val = if rn == PC { pc_val } else { self.regs[rn] };
        let rm_val = if rm == PC { pc_val } else { self.regs[rm] };
        let (b, shifter_carry) = self.thumb_shifted(rm_val, imm2, shift_type);
        // TST/TEQ/CMP/CMN are the op with S=1 and Rd=PC (0xF); MOV/MVN are
        // ORR/ORN with Rn=PC.
        let is_compare = set_flags && rd == PC && matches!(op, 0 | 4 | 8 | 13);
        let result = match op {
            0 => rn_val & b,       // AND
            1 => rn_val & !b,      // BIC
            2 if is_mov => b,      // MOV
            2 => rn_val | b,       // ORR
            3 if is_mvn => !b,     // MVN
            3 => !rn_val | b,      // ORN
            4 => rn_val ^ b,       // EOR
            8 | 10 => {
                let (r, _, _) = if op == 8 {
                    Self::arm_add(rn_val, b, false)
                } else {
                    Self::arm_add(rn_val, b, self.flag_c())
                };
                r
            }
            11 | 13 => {
                let (r, _, _) = if op == 11 {
                    Self::arm_sub(rn_val, b, self.flag_c())
                } else {
                    Self::arm_sub(rn_val, b, true)
                };
                r
            }
            14 => {
                let (r, _, _) = Self::arm_sub(b, rn_val, true);
                r
            }
            _ => {
                self.halted = true;
                return;
            }
        };
        let (n, z, c, v) = match op {
            0 | 1 | 2 | 3 | 4 => {
                // Logical: N/Z from result, C from shifter, V unchanged.
                (result & 0x8000_0000 != 0, result == 0, shifter_carry, self.flag_v())
            }
            8 | 10 => {
                let (r, c, v) = if op == 8 {
                    Self::arm_add(rn_val, b, false)
                } else {
                    Self::arm_add(rn_val, b, self.flag_c())
                };
                (r & 0x8000_0000 != 0, r == 0, c, v)
            }
            11 | 13 => {
                let (r, c, v) = if op == 11 {
                    Self::arm_sub(rn_val, b, self.flag_c())
                } else {
                    Self::arm_sub(rn_val, b, true)
                };
                (r & 0x8000_0000 != 0, r == 0, c, v)
            }
            14 => {
                let (r, c, v) = Self::arm_sub(b, rn_val, true);
                (r & 0x8000_0000 != 0, r == 0, c, v)
            }
            _ => (self.flag_n(), self.flag_z(), self.flag_c(), self.flag_v()),
        };
        if is_compare || set_flags {
            self.set_nzcv(n, z, c, v);
        }
        if !is_compare {
            if rd == PC {
                self.branch_exchange_value(result);
            } else {
                self.regs[rd] = result;
            }
        }
    }

    /// Execute a Thumb-2 data-processing (modified immediate) instruction.
    /// `first`/`second` are the two halfwords; `pc` is the architectural PC
    /// (current + 4) already advanced by `step`. The 12-bit immediate is
    /// expanded by [`thumb_expand_imm12`].
    fn thumb2_data_processing_imm(&mut self, first: u16, second: u16, pc: u32) {
        // op = bits 8-5; bit4 = S; bits3-0 = Rn; bit10 = i (top of imm12).
        let op = ((first >> 5) & 0xF) as u32;
        let set_flags = first & 0x0010 != 0;
        let rn = (first & 0x0F) as usize;
        let i = ((first >> 10) & 1) as u32;
        let imm3 = ((second >> 12) & 0x7) as u32;
        let rd = ((second >> 8) & 0x0F) as usize;
        let imm8 = (second & 0xFF) as u32;
        let imm12 = (i << 11) | (imm3 << 8) | imm8;
        let (b, imm_carry) = thumb_expand_imm12(imm12, self.flag_c());
        // MOV/MVN are ORR/ORN with Rn=PC; TST/TEQ/CMP/CMN are op with S=1 and
        // Rd=PC (no writeback).
        let is_mov = op == 2 && rn == PC;
        let is_mvn = op == 3 && rn == PC;
        let is_compare = set_flags && rd == PC && matches!(op, 0 | 4 | 8 | 13);
        let pc_val = pc & !3;
        let rn_val = if rn == PC { pc_val } else { self.regs[rn] };
        let result = match op {
            0 => rn_val & b,
            1 => rn_val & !b,
            2 if is_mov => b,
            2 => rn_val | b,
            3 if is_mvn => !b,
            3 => !rn_val | b,
            4 => rn_val ^ b,
            8 => {
                let (r, _, _) = Self::arm_add(rn_val, b, false);
                r
            }
            13 => {
                let (r, _, _) = Self::arm_sub(rn_val, b, true);
                r
            }
            14 => {
                let (r, _, _) = Self::arm_sub(b, rn_val, true);
                r
            }
            _ => {
                self.halted = true;
                return;
            }
        };
        let (n, z, c, v) = match op {
            0 | 1 | 2 | 3 | 4 => {
                // Logical: N/Z from result, C from the immediate expansion,
                // V unchanged.
                (
                    result & 0x8000_0000 != 0,
                    result == 0,
                    imm_carry,
                    self.flag_v(),
                )
            }
            8 | 13 | 14 => {
                let (r, c, v) = if op == 8 {
                    Self::arm_add(rn_val, b, false)
                } else if op == 13 {
                    Self::arm_sub(rn_val, b, true)
                } else {
                    Self::arm_sub(b, rn_val, true)
                };
                (r & 0x8000_0000 != 0, r == 0, c, v)
            }
            _ => (self.flag_n(), self.flag_z(), self.flag_c(), self.flag_v()),
        };
        if is_compare || set_flags {
            self.set_nzcv(n, z, c, v);
        }
        if !is_compare {
            if rd == PC {
                self.branch_exchange_value(result);
            } else {
                self.regs[rd] = result;
            }
        }
    }

    /// Compute the Thumb-2 shifted-register operand (Rm with the shift encoded
    /// by `imm2`/`shift_type`), returning the shifted value and the shifter
    /// carry-out. shift_type: 0=LSL, 1=LSR, 2=ASR, 3=ROR (imm2=0 → RRX).
    fn thumb_shifted(&self, value: u32, imm2: u32, shift_type: u32) -> (u32, bool) {
        match shift_type {
            0 => {
                if imm2 == 0 {
                    (value, self.flag_c())
                } else {
                    (value << imm2, (value >> (32 - imm2)) & 1 != 0)
                }
            }
            1 => {
                if imm2 == 0 {
                    (0, value & 0x8000_0000 != 0)
                } else {
                    (value >> imm2, (value >> (imm2 - 1)) & 1 != 0)
                }
            }
            2 => {
                if imm2 == 0 {
                    (((value as i32) >> 31) as u32, value & 0x8000_0000 != 0)
                } else {
                    (
                        ((value as i32) >> imm2) as u32,
                        (value >> (imm2 - 1)) & 1 != 0,
                    )
                }
            }
            _ => {
                if imm2 == 0 {
                    let c = self.flag_c();
                    ((value >> 1) | ((c as u32) << 31), value & 1 != 0)
                } else {
                    (
                        (value >> imm2) | (value << (32 - imm2)),
                        (value >> (imm2 - 1)) & 1 != 0,
                    )
                }
            }
        }
    }

    fn arm_single_transfer(&mut self, insn: u32, pc_base: u32) {
        let register_offset = insn & (1 << 25) != 0;
        let pre_index = insn & (1 << 24) != 0;
        let add = insn & (1 << 23) != 0;
        let byte = insn & (1 << 22) != 0;
        let writeback = insn & (1 << 21) != 0;
        let load = insn & (1 << 20) != 0;
        let rn = ((insn >> 16) & 0x0F) as usize;
        let rd = ((insn >> 12) & 0x0F) as usize;
        let offset = if register_offset {
            let rm = (insn & 0x0F) as usize;
            let (value, _) = self.arm_operand2(insn & !(1 << 25), pc_base);
            let _ = rm;
            value
        } else {
            insn & 0x0FFF
        };
        let base = self.arm_reg(rn, pc_base);
        let address = if add { base.wrapping_add(offset) } else { base.wrapping_sub(offset) };
        let effective = if pre_index { address } else { base };
        if load {
            let value = if byte {
                self.read_u8(effective) as u32
            } else {
                self.read_u32(effective & !3)
            };
            if rd == PC {
                self.branch_exchange_value(value);
            } else {
                self.regs[rd] = value;
            }
        } else {
            let value = self.arm_reg(rd, pc_base);
            if byte {
                self.write_u8(effective, value as u8);
            } else {
                self.write_u32(effective & !3, value);
            }
        }
        if (!pre_index || writeback) && rn != PC {
            self.regs[rn] = if add {
                base.wrapping_add(offset)
            } else {
                base.wrapping_sub(offset)
            };
        }
    }

    fn arm_halfword_transfer(&mut self, insn: u32, pc_base: u32) {
        let pre_index = insn & (1 << 24) != 0;
        let add = insn & (1 << 23) != 0;
        let immediate = insn & (1 << 22) != 0;
        let writeback = insn & (1 << 21) != 0;
        let load = insn & (1 << 20) != 0;
        let rn = ((insn >> 16) & 0x0F) as usize;
        let rd = ((insn >> 12) & 0x0F) as usize;
        let sh = (insn >> 5) & 0x03;
        // LDRD/STRD (doubleword transfer): bit 20 = 0 and sh = 2 (LDRD) or
        // sh = 3 (STRD). The register pair is Rt/Rt+1 and the offset is imm8
        // (byte, immediate form) or Rm (bits 11-8, register form).
        if !load && sh >= 2 {
            let offset = if immediate {
                ((insn >> 4) & 0xF0) | (insn & 0x0F)
            } else {
                self.arm_reg(((insn >> 8) & 0x0F) as usize, pc_base)
            };
            let base = self.arm_reg(rn, pc_base);
            let address = if add {
                base.wrapping_add(offset)
            } else {
                base.wrapping_sub(offset)
            };
            let effective = if pre_index { address } else { base };
            if sh == 2 {
                // LDRD: load the consecutive word pair.
                self.regs[rd] = self.read_u32(effective & !3);
                self.regs[rd + 1] = self.read_u32((effective.wrapping_add(4)) & !3);
            } else {
                // STRD: store the consecutive word pair.
                self.write_u32(effective & !3, self.regs[rd]);
                self.write_u32((effective.wrapping_add(4)) & !3, self.regs[rd + 1]);
            }
            if (!pre_index || writeback) && rn != PC {
                self.regs[rn] = if add {
                    base.wrapping_add(offset)
                } else {
                    base.wrapping_sub(offset)
                };
            }
            return;
        }
        let offset = if immediate {
            ((insn >> 4) & 0xF0) | (insn & 0x0F)
        } else {
            self.arm_reg((insn & 0x0F) as usize, pc_base)
        };
        let base = self.arm_reg(rn, pc_base);
        let address = if add { base.wrapping_add(offset) } else { base.wrapping_sub(offset) };
        let effective = if pre_index { address } else { base };
        if load {
            let value = match sh {
                1 => self.read_u16(effective) as u32,
                2 => self.read_u8(effective) as i8 as i32 as u32,
                _ => self.read_u16(effective) as i16 as i32 as u32,
            };
            self.regs[rd] = value;
        } else {
            let value = self.arm_reg(rd, pc_base);
            match sh {
                1 => self.write_u16(effective, value as u16),
                2 => self.write_u8(effective, value as u8),
                _ => self.write_u16(effective, value as u16),
            }
        }
        if (!pre_index || writeback) && rn != PC {
            self.regs[rn] = if add {
                base.wrapping_add(offset)
            } else {
                base.wrapping_sub(offset)
            };
        }
    }

    fn arm_block_transfer(&mut self, insn: u32, pc_base: u32) {
        let pre_index = insn & (1 << 24) != 0;
        let add = insn & (1 << 23) != 0;
        let writeback = insn & (1 << 21) != 0;
        let load = insn & (1 << 20) != 0;
        let rn = ((insn >> 16) & 0x0F) as usize;
        let list = insn & 0xFFFF;
        let count = list.count_ones();
        if count == 0 {
            return;
        }
        let base = self.arm_reg(rn, pc_base);
        let start = match (add, pre_index) {
            (true, false) => base,                          // IA
            (true, true) => base.wrapping_add(4),           // IB
            (false, false) => base.wrapping_sub(4 * count).wrapping_add(4), // DA (reverse)
            (false, true) => base.wrapping_sub(4 * count),  // DB
        };
        let mut address = start;
        let writeback_value = if add {
            base.wrapping_add(4 * count)
        } else {
            base.wrapping_sub(4 * count)
        };
        if !load {
            for index in 0..16 {
                if list & (1 << index) == 0 {
                    continue;
                }
                let value = self.arm_reg(index, pc_base);
                self.write_u32(address & !3, value);
                address = address.wrapping_add(4);
            }
            if writeback && rn != PC {
                self.regs[rn] = writeback_value;
            }
            return;
        }
        for index in 0..16 {
            if list & (1 << index) == 0 {
                continue;
            }
            let value = self.read_u32(address & !3);
            if index == PC {
                self.branch_exchange_value(value);
            } else {
                self.regs[index] = value;
            }
            address = address.wrapping_add(4);
        }
        if writeback && rn != PC && list & (1 << rn) == 0 {
            self.regs[rn] = writeback_value;
        }
    }
}

fn shift_lsl(value: u32, amount: u32, carry_in: bool) -> (u32, bool) {
    if amount == 0 {
        (value, carry_in)
    } else if amount < 32 {
        (value << amount, (value >> (32 - amount)) & 1 != 0)
    } else if amount == 32 {
        (0, value & 1 != 0)
    } else {
        (0, false)
    }
}

fn shift_lsr(value: u32, amount: u32, carry_in: bool) -> (u32, bool) {
    if amount == 0 {
        (value, carry_in)
    } else if amount < 32 {
        (value >> amount, (value >> (amount - 1)) & 1 != 0)
    } else if amount == 32 {
        (0, value >> 31 != 0)
    } else {
        (0, false)
    }
}

fn shift_asr(value: u32, amount: u32, carry_in: bool) -> (u32, bool) {
    if amount == 0 {
        (value, carry_in)
    } else if amount < 32 {
        (
            ((value as i32) >> amount) as u32,
            (value >> (amount - 1)) & 1 != 0,
        )
    } else if value & 0x8000_0000 != 0 {
        (0xFFFF_FFFF, true)
    } else {
        (0, false)
    }
}

fn shift_ror(value: u32, amount: u32, carry_in: bool) -> (u32, bool) {
    let n = amount % 32;
    if amount == 0 {
        (value, carry_in)
    } else if n == 0 {
        (value, value >> 31 != 0)
    } else {
        let result = value.rotate_right(n);
        (result, result >> 31 != 0)
    }
}

/// Compute a Thumb shift result and carry-out. `amount` is the raw shift
/// amount; for LSR/ASR a value of 0 means 32 (ARM semantics). Returns
/// `(result, carry_out, carry_changed)`. For LSL #0 the carry is unchanged.
fn thumb_shift_value(value: u32, amount: u32, kind: u32) -> (u32, bool, bool) {
    match kind {
        0 => {
            // LSL
            if amount == 0 {
                (value, false, false)
            } else if amount < 32 {
                (value << amount, (value >> (32 - amount)) & 1 != 0, true)
            } else {
                (0, false, true)
            }
        }
        1 => {
            // LSR (amount 0 means 32)
            let amount = if amount == 0 { 32 } else { amount };
            if amount < 32 {
                (value >> amount, (value >> (amount - 1)) & 1 != 0, true)
            } else if amount == 32 {
                (0, value >> 31 != 0, true)
            } else {
                (0, false, true)
            }
        }
        _ => {
            // ASR (amount 0 means 32)
            let amount = if amount == 0 { 32 } else { amount };
            if amount < 32 {
                (((value as i32) >> amount) as u32, (value >> (amount - 1)) & 1 != 0, true)
            } else if amount == 32 {
                (((value as i32) >> 31) as u32, value & 0x8000_0000 != 0, true)
            } else if value & 0x8000_0000 != 0 {
                (0xFFFF_FFFF, true, true)
            } else {
                (0, false, true)
            }
        }
    }
}

fn sign_extend_thumb(value: u32, bits: u32) -> u32 {
    let shift = 32 - bits;
    ((value << shift) as i32 >> shift) as u32
}

/// Expand a Thumb-2 modified immediate (12-bit `imm12`) to its 32-bit value.
/// Mirrors ARMv7-M `ThumbExpandImm_C`: the top two bits select a byte pattern
/// when clear (carry preserved), otherwise the low byte with bit 7 forced is
/// rotated right by `2*rot + (byte >> 7)` (carry = top bit of the result).
pub(crate) fn thumb_expand_imm12(imm12: u32, carry_in: bool) -> (u32, bool) {
    if imm12 & 0xC00 == 0 {
        let xy = imm12 & 0xFF;
        let value = match (imm12 >> 8) & 3 {
            0 => xy,
            1 => xy | (xy << 16),
            2 => ((xy << 8) | (xy << 24)) & 0xFFFF_FFFF,
            _ => xy | (xy << 8) | (xy << 16) | (xy << 24),
        };
        (value, carry_in)
    } else {
        let byte = imm12 & 0xFF;
        let rot = (imm12 >> 8) & 0xF;
        let value = (byte | 0x80).rotate_right(2 * rot + (byte >> 7));
        (value, value & 0x8000_0000 != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core() -> ArmCore {
        ArmCore::new()
    }

    fn run(core: &mut ArmCore, insns: &[u16], steps: usize) {
        for (i, insn) in insns.iter().enumerate() {
            core.write_u16((i * 2) as u32, *insn);
        }
        core.regs[PC] = 0;
        core.set_thumb(true);
        for _ in 0..steps {
            let r = core.step(&[]);
            if r == ArmStep::Halted {
                break;
            }
        }
    }

    #[test]
    fn mov_immediate_sets_register() {
        let mut c = core();
        // MOVS r0, #5  (0x2005)
        run(&mut c, &[0x2005], 1);
        assert_eq!(c.regs[0], 5);
    }

    #[test]
    fn add_immediate_8bit() {
        let mut c = core();
        c.regs[0] = 10;
        // ADDS r0, r0, #7  (0x3007)
        run(&mut c, &[0x3007], 1);
        assert_eq!(c.regs[0], 17);
    }

    #[test]
    fn sub_immediate_8bit_sets_flags() {
        let mut c = core();
        c.regs[0] = 3;
        // SUBS r0, r0, #3  (0x3803)
        run(&mut c, &[0x3803], 1);
        assert_eq!(c.regs[0], 0);
        assert!(c.flag_z(), "zero flag set");
        assert!(c.flag_c(), "borrow sets carry (no borrow => carry)");
    }

    #[test]
    fn thumb1_field_extraction_matches_encoding() {
        // Format 0: LSL reads Rm=bits5-3 (bit 6 belongs to the shift amount).
        let mut c = core();
        c.regs[1] = 0x20;
        c.regs[9] = 0xDEAD_BEEF; // sentinel: bit 6 of 0x0148 is set.
        run(&mut c, &[0x0148], 1); // lsls r0, r1, #5
        assert_eq!(c.regs[0], 0x400);
        assert_eq!(c.regs[9], 0xDEAD_BEEF);

        // Format 1 register: SUB reads Rn=bits5-3, Rm=bits8-6; bit 9 selects
        // SUB (not bit 10) and bit 10 selects the immediate form.
        let mut c = core();
        c.regs[2] = 5;
        c.regs[1] = 3;
        run(&mut c, &[0x1A50], 1); // subs r0, r2, r1
        assert_eq!(c.regs[0], 2);

        // Format 1 immediate: SUBS r0, r2, #3 (0x1ED0).
        let mut c = core();
        c.regs[2] = 10;
        run(&mut c, &[0x1ED0], 1);
        assert_eq!(c.regs[0], 7);

        // Format 4 ALU: ANDS reads Rm=bits5-3 (bit 6 belongs to the opcode).
        let mut c = core();
        c.regs[0] = 0b1100;
        c.regs[1] = 0b1010;
        c.regs[9] = 0xDEAD_BEEF;
        run(&mut c, &[0x4148], 1); // ADCS r0, r1 (op=5, bit 6 set)
        assert_eq!(c.regs[9], 0xDEAD_BEEF);
        assert_eq!(c.regs[0], 0b1100 + 0b1010);
    }

    #[test]
    fn thumb1_high_register_and_sp_relative_fields() {
        // Format 9: SP-relative STR uses Rt=bits10-8 (0x9201: str r2, [sp,#4]).
        let mut c = core();
        c.regs[2] = 0x1234_5678;
        c.regs[SP] = 0x1000;
        run(&mut c, &[0x9201], 1);
        assert_eq!(c.read_u32(0x1004), 0x1234_5678);
        assert_eq!(c.read_u32(0x1000), 0); // bit 0-2 reg (r1) must not be used.

        // Format A: ADD Rd, PC, #imm uses Rd=bits10-8 (0xA201: add r2, pc, #4).
        let mut c = core();
        run(&mut c, &[0xA201], 1);
        assert_eq!(c.regs[2], 8); // PC aligned to 4, +4 (imm8=1*4).
        assert_eq!(c.regs[1], 0); // bits 0-2 reg must not be written.

        // Format C: LDMIA uses Rn=bits10-8 (0xC801: ldmia r0!, {r0}).
        let mut c = core();
        c.regs[0] = 0x1000;
        c.regs[2] = 0xDEAD_BEEF; // sentinel base register.
        c.write_u32(0x1000, 0x1111_2222);
        run(&mut c, &[0xC801], 1);
        assert_eq!(c.regs[0], 0x1111_2222);
        assert_eq!(c.regs[2], 0xDEAD_BEEF);
    }

    #[test]
    fn thumb1_register_offset_addressing() {
        // Format 5: LDRH r0, [r1, r2] (0x5A88) sums Rn+Rm regardless of which
        // operand is named first; both must be read from the encoded fields.
        let mut c = core();
        c.regs[1] = 0x1000;
        c.regs[2] = 4;
        c.write_u16(0x1004, 0xABCD);
        run(&mut c, &[0x5A88], 1);
        assert_eq!(c.regs[0], 0xABCD);
    }

    #[test]
    fn thumb1_pc_relative_uses_architectural_pc() {
        // PC-relative LDR (0x4800: ldr r0, [pc, #0]) at a 4-aligned address:
        // the literal base is Align(current + 4, 4), so it must read word at
        // address 4 (not address 0).
        let mut c = core();
        c.write_u32(0x4, 0xCAFE_F00D);
        c.write_u32(0x0, 0xDEAD_BEEF); // must not be read.
        run(&mut c, &[0x4800], 1);
        assert_eq!(c.regs[0], 0xCAFE_F00D);

        // ADR/`add r2, pc, #4` (0xA201) at a 2-aligned (not 4-aligned) address:
        // Align(current + 4, 4) + imm8*4 = Align(2 + 4, 4) + 4 = 4 + 4 = 8.
        let mut c = core();
        c.write_u16(0x2, 0xA201);
        c.regs[PC] = 2;
        c.set_thumb(true);
        let r = c.step(&[]);
        assert_eq!(r, ArmStep::Advanced);
        assert_eq!(c.regs[2], 8);
    }

    #[test]
    fn alu_operations() {
        let mut c = core();
        c.regs[0] = 0b1100;
        c.regs[1] = 0b1010;
        // ANDS r0, r1  (0x4008)
        run(&mut c, &[0x4008], 1);
        assert_eq!(c.regs[0], 0b1000);
    }

    #[test]
    fn thumb1_alu_sets_nz_flags() {
        // LSLS r0, r1 (0x4088): shift=1 → r0 = 1<<1 = 2, N=0, Z=0.
        let mut c = core();
        c.regs[0] = 1;
        c.regs[1] = 1;
        run(&mut c, &[0x4088], 1);
        assert_eq!(c.regs[0], 2);
        assert!(!c.flag_n() && !c.flag_z());

        // LSLS r0, r1 with r0=0: result 0 → Z set.
        let mut c = core();
        c.regs[0] = 0;
        c.regs[1] = 1;
        run(&mut c, &[0x4088], 1);
        assert!(c.flag_z());

        // RSBS r0, r1 (0x4248): r0 = 0 - 5 = 0xFFFFFFFB → N set, C set.
        let mut c = core();
        c.regs[1] = 5;
        run(&mut c, &[0x4248], 1);
        assert_eq!(c.regs[0], 0xFFFF_FFFB);
        assert!(c.flag_n() && !c.flag_z() && c.flag_c());

        // MULS r0, r1 (0x4348): r0 = 3 * 0x80000000 = 0x80000000*3 → N set.
        let mut c = core();
        c.regs[0] = 3;
        c.regs[1] = 0x8000_0000;
        run(&mut c, &[0x4348], 1);
        assert_eq!(c.regs[0], 3u32.wrapping_mul(0x8000_0000));
        assert!(c.flag_n());
    }

    #[test]
    fn thumb1_add_sub_sets_v_flag() {
        // ADDS r0, r1, r2 (0x1888): 0x7FFFFFFF + 1 = 0x80000000 overflows.
        let mut c = core();
        c.regs[1] = 0x7FFF_FFFF;
        c.regs[2] = 1;
        run(&mut c, &[0x1888], 1);
        assert_eq!(c.regs[0], 0x8000_0000);
        assert!(c.flag_v());
        assert!(c.flag_n() && !c.flag_z()); // result N set.

        // SUBS r0, r1, r2 (0x1A88): 0x80000000 - 1 = 0x7FFFFFFF overflows.
        let mut c = core();
        c.regs[1] = 0x8000_0000;
        c.regs[2] = 1;
        run(&mut c, &[0x1A88], 1);
        assert_eq!(c.regs[0], 0x7FFF_FFFF);
        assert!(c.flag_v());
        assert!(!c.flag_n() && !c.flag_z());
    }

    #[test]
    fn thumb1_high_cmp_sets_v_flag() {
        // High-register CMP r0, r1 (0x4508) updates N, Z, C and V.
        // 0x80000000 - 1 = 0x7FFFFFFF: overflow (negative minus positive).
        let mut c = core();
        c.regs[0] = 0x8000_0000;
        c.regs[1] = 1;
        run(&mut c, &[0x4508], 1);
        assert!(c.flag_v());
        assert!(!c.flag_n() && !c.flag_z());
        assert!(c.flag_c()); // no borrow: 0x80000000 >= 1.

        // Non-overflowing subtraction leaves V clear.
        let mut c = core();
        c.regs[0] = 5;
        c.regs[1] = 3;
        run(&mut c, &[0x4508], 1);
        assert!(!c.flag_v());
        assert!(!c.flag_n() && !c.flag_z());
        assert!(c.flag_c());
    }

    #[test]
    fn thumb1_cbz_cbnz_branch() {
        // CBZ r0, +6 (0xB100 | (3 << 3) = 0xB118) at address 0: taken when
        // r0 == 0 → target = 0 + 4 + 3*2 = 0xA.
        let mut c = core();
        c.regs[0] = 0;
        run(&mut c, &[0xB118], 1);
        assert_eq!(c.regs[PC], 10);

        // Not taken when r0 != 0 → falls through to current + 2 = 2.
        let mut c = core();
        c.regs[0] = 1;
        run(&mut c, &[0xB118], 1);
        assert_eq!(c.regs[PC], 2);

        // CBNZ r2, +2 (0xB900 | (1 << 3) | (2 << 0) = 0xB90A): taken when
        // r2 != 0 → target = 0 + 4 + 1*2 = 6.
        let mut c = core();
        c.regs[2] = 5;
        run(&mut c, &[0xB90A], 1);
        assert_eq!(c.regs[PC], 6);

        // CBNZ not taken when r2 == 0 → falls through to current + 2 = 2.
        let mut c = core();
        c.regs[2] = 0;
        run(&mut c, &[0xB90A], 1);
        assert_eq!(c.regs[PC], 2);
    }

    #[test]
    fn thumb1_add_sub_sp_immediate() {
        // ADD sp, sp, #8 (0xB002): SP += 8.
        let mut c = core();
        c.regs[SP] = 0x2000_0000;
        run(&mut c, &[0xB002], 1);
        assert_eq!(c.regs[SP], 0x2000_0008);

        // SUB sp, sp, #16 (0xB084): the low byte 0x84 includes the sub bit,
        // so the offset must be (0x84 & 0x7F) * 4 = 16.
        let mut c = core();
        c.regs[SP] = 0x2000_0000;
        run(&mut c, &[0xB084], 1);
        assert_eq!(c.regs[SP], 0x2000_0000 - 16);
    }

    #[test]
    fn thumb1_nop_no_effect() {
        // NOP (0xBF00) must advance PC by 2 without touching SP or registers.
        let mut c = core();
        c.regs[SP] = 0x2000_0000;
        c.regs[0] = 0x1234_5678;
        run(&mut c, &[0xBF00], 1);
        assert_eq!(c.regs[PC], 2);
        assert_eq!(c.regs[SP], 0x2000_0000);
        assert_eq!(c.regs[0], 0x1234_5678);
    }

    #[test]
    fn thumb1_extend_sign_zero() {
        // SXTB r3, r4 (0xB263): sign-extend the low byte.
        let mut c = core();
        c.regs[4] = 0x80;
        run(&mut c, &[0xB263], 1);
        assert_eq!(c.regs[3], 0xFFFF_FF80);

        // SXTH r3, r4 (0xB223): sign-extend the low halfword.
        let mut c = core();
        c.regs[4] = 0x8000;
        run(&mut c, &[0xB223], 1);
        assert_eq!(c.regs[3], 0xFFFF_8000);

        // UXTB r3, r4 (0xB2E3): zero-extend the low byte.
        let mut c = core();
        c.regs[4] = 0xFFFF_FF80;
        run(&mut c, &[0xB2E3], 1);
        assert_eq!(c.regs[3], 0x80);

        // UXTH r3, r4 (0xB2A3): zero-extend the low halfword.
        let mut c = core();
        c.regs[4] = 0xFFFF_8000;
        run(&mut c, &[0xB2A3], 1);
        assert_eq!(c.regs[3], 0x8000);
    }

    #[test]
    fn thumb2_mul() {
        // MUL r0, r1, r2 (0xFB01F002): low 32 bits of the product.
        let mut c = core();
        c.regs[1] = 0x0001_0000;
        c.regs[2] = 0x0002_0000;
        run(&mut c, &[0xFB01, 0xF002], 1);
        assert_eq!(c.regs[0], 0x0000_0000); // 0x200000000 truncated to 32 bits.
        assert_eq!(c.regs[PC], 4); // 32-bit Thumb-2 instruction.

        // 3 * 5 = 15.
        let mut c = core();
        c.regs[1] = 3;
        c.regs[2] = 5;
        run(&mut c, &[0xFB01, 0xF002], 1);
        assert_eq!(c.regs[0], 15);
    }

    #[test]
    fn thumb2_mrs_msr() {
        // MRS r0, xPSR (0xF3EF8003): reads the full status register.
        let mut c = core();
        c.xpsr = 0x4100_0000; // N clear, Z set, Thumb bit set.
        run(&mut c, &[0xF3EF, 0x8003], 1);
        assert_eq!(c.regs[0], 0x4100_0000);

        // MRS r0, IPSR (0xF3EF8005): reads only the exception number.
        let mut c = core();
        c.xpsr = 0x4100_0003; // exception 3.
        run(&mut c, &[0xF3EF, 0x8005], 1);
        assert_eq!(c.regs[0], 3);

        // MSR xPSR_nzcvq, r0 (0xF3808803): sets N/Z/C/V from r0.
        let mut c = core();
        c.regs[0] = 0x8000_0000; // N set, Z/C/V clear.
        run(&mut c, &[0xF380, 0x8803], 1);
        assert!(c.flag_n());
        assert!(!c.flag_z() && !c.flag_c() && !c.flag_v());
    }

    #[test]
    fn thumb1_cps_primask() {
        // CPSID i (0xB672): set PRIMASK.
        let mut c = core();
        run(&mut c, &[0xB672], 1);
        assert_eq!(c.primask, 1);
        // CPSIE i (0xB662): clear PRIMASK.
        run(&mut c, &[0xB662], 1);
        assert_eq!(c.primask, 0);
    }

    #[test]
    fn thumb1_it_block() {
        // `it eq` (0xBF08) with Z set: the following `moveq r0, r1` (0x4608)
        // executes; with Z clear it is skipped.
        let mut c = core();
        c.xpsr |= 0x4000_0000; // Z set
        c.regs[1] = 0xAB;
        run(&mut c, &[0xBF08, 0x4608], 2);
        assert_eq!(c.regs[0], 0xAB, "IT then-arm executes when the condition holds");
        assert_eq!(c.it_mask, 0, "IT block clears after its last instruction");

        let mut c = core();
        c.regs[1] = 0xAB;
        run(&mut c, &[0xBF08, 0x4608], 2);
        assert_eq!(c.regs[0], 0, "IT then-arm is skipped when the condition fails");

        // `ite ge` (0xBFAC): the then-arm is `movge r0, r1` (0x4608) and the
        // else-arm is `movlt r2, r3` (0x461A). With GE true only the then-arm
        // executes; with GE false only the else-arm executes.
        let mut c = core();
        c.xpsr = 0x0100_0000; // N=0, V=0 → GE true, LT false
        c.regs[1] = 0x11;
        c.regs[3] = 0x22;
        run(&mut c, &[0xBFAC, 0x4608, 0x461A], 3);
        assert_eq!(c.regs[0], 0x11, "ITE then-arm executes when GE holds");
        assert_eq!(c.regs[2], 0, "ITE else-arm is skipped when LT fails");

        let mut c = core();
        c.xpsr = 0x8100_0000; // N=1, V=0 → GE false, LT true
        c.regs[1] = 0x11;
        c.regs[3] = 0x22;
        run(&mut c, &[0xBFAC, 0x4608, 0x461A], 3);
        assert_eq!(c.regs[0], 0, "ITE then-arm is skipped when GE fails");
        assert_eq!(c.regs[2], 0x22, "ITE else-arm executes when LT holds");

        // `itt eq` (0xBF04): both arms are then; with Z set both execute.
        let mut c = core();
        c.xpsr |= 0x4000_0000; // Z set
        c.regs[1] = 0x11;
        c.regs[3] = 0x22;
        run(&mut c, &[0xBF04, 0x4608, 0x461A], 3);
        assert_eq!(c.regs[0], 0x11);
        assert_eq!(c.regs[2], 0x22);

        // `bkpt #3` (0xBE03): halts the core.
        let mut c = core();
        run(&mut c, &[0xBE03], 1);
        assert!(c.halted, "BKPT halts the core");
    }

    #[test]
    fn thumb2_bfx() {
        // ubfx r1, r2, #4, #8 (0xF3C21107): extract bits [11:4].
        let mut c = core();
        c.regs[2] = 0xABCD_EF12;
        run(&mut c, &[0xF3C2, 0x1107], 1);
        assert_eq!(c.regs[1], 0xF1);

        // sbfx r3, r4, #2, #6 (0xF3440385): extract bits [7:2], sign-extend.
        let mut c = core();
        c.regs[4] = 0x80;
        run(&mut c, &[0xF344, 0x0385], 1);
        assert_eq!(c.regs[3], 0xFFFF_FFE0);

        // bfc r5, #4, #8 (0xF36F150B): clear bits [11:4].
        let mut c = core();
        c.regs[5] = 0xFFFF_FFFF;
        run(&mut c, &[0xF36F, 0x150B], 1);
        assert_eq!(c.regs[5], 0xFFFF_F00F);

        // bfi r6, r7, #3, #5 (0xF36706C7): insert r7 bits [4:0] at [7:3].
        let mut c = core();
        c.regs[6] = 0x0000_0000;
        c.regs[7] = 0x1F;
        run(&mut c, &[0xF367, 0x06C7], 1);
        assert_eq!(c.regs[6], 0xF8);
    }

    #[test]
    fn thumb1_rev_family() {
        // REV r0, r1 (0xBA08): reverse all four bytes.
        let mut c = core();
        c.regs[1] = 0x1234_5678;
        run(&mut c, &[0xBA08], 1);
        assert_eq!(c.regs[0], 0x7856_3412);

        // REV16 r2, r3 (0xBA5A): swap bytes within each halfword.
        let mut c = core();
        c.regs[3] = 0x1234_5678;
        run(&mut c, &[0xBA5A], 1);
        assert_eq!(c.regs[2], 0x3412_7856);

        // REVSH r4, r5 (0xBAEC): reverse the low halfword bytes, sign-extend.
        let mut c = core();
        c.regs[5] = 0x1234_0080; // low halfword 0x0080 reverses to 0x8000.
        run(&mut c, &[0xBAEC], 1);
        assert_eq!(c.regs[4], 0xFFFF_8000);
    }

    #[test]
    fn thumb2_clz() {
        // CLZ r6, r7 (0xFAB7F687): count leading zeros.
        let mut c = core();
        c.regs[7] = 0x0000_0001;
        run(&mut c, &[0xFAB7, 0xF687], 1);
        assert_eq!(c.regs[6], 31);
        assert_eq!(c.regs[PC], 4);

        let mut c = core();
        c.regs[7] = 0x8000_0000;
        run(&mut c, &[0xFAB7, 0xF687], 1);
        assert_eq!(c.regs[6], 0);
    }

    #[test]
    fn thumb2_rev_family_and_extend() {
        // rev.w r7, r8 (0xFA98F788): reverse all four bytes.
        let mut c = core();
        c.regs[8] = 0x1234_5678;
        run(&mut c, &[0xFA98, 0xF788], 1);
        assert_eq!(c.regs[7], 0x7856_3412);
        assert_eq!(c.regs[PC], 4);

        // rev16.w r9, r10 (0xFA9AF99A): swap bytes within each halfword.
        let mut c = core();
        c.regs[10] = 0x1234_5678;
        run(&mut c, &[0xFA9A, 0xF99A], 1);
        assert_eq!(c.regs[9], 0x3412_7856);

        // revsh.w r11, r12 (0xFA9CFBBC): reverse low halfword, sign-extend.
        let mut c = core();
        c.regs[12] = 0x1234_0080;
        run(&mut c, &[0xFA9C, 0xFBBC], 1);
        assert_eq!(c.regs[11], 0xFFFF_8000);

        // rbit r0, r1 (0xFA91F0A1): reverse the bit order.
        let mut c = core();
        c.regs[1] = 0x0000_0001;
        run(&mut c, &[0xFA91, 0xF0A1], 1);
        assert_eq!(c.regs[0], 0x8000_0000);

        // uxtb.w r8, r9 (0xFA5FF889): zero-extend the low byte.
        let mut c = core();
        c.regs[9] = 0xFFFF_FF80;
        run(&mut c, &[0xFA5F, 0xF889], 1);
        assert_eq!(c.regs[8], 0x80);

        // sxtb.w r10, r11 (0xFA4FFA8B): sign-extend the low byte.
        let mut c = core();
        c.regs[11] = 0x0000_0080;
        run(&mut c, &[0xFA4F, 0xFA8B], 1);
        assert_eq!(c.regs[10], 0xFFFF_FF80);

        // uxth.w r0, r1 (0xFA1FF081): zero-extend the low halfword.
        let mut c = core();
        c.regs[1] = 0xFFFF_8000;
        run(&mut c, &[0xFA1F, 0xF081], 1);
        assert_eq!(c.regs[0], 0x8000);

        // sxth.w r2, r3 (0xFA0FF283): sign-extend the low halfword.
        let mut c = core();
        c.regs[3] = 0x0000_8000;
        run(&mut c, &[0xFA0F, 0xF283], 1);
        assert_eq!(c.regs[2], 0xFFFF_8000);

        // uxtb.w r0, r1, ror #8 (0xFA5FF091): rotate then zero-extend.
        let mut c = core();
        c.regs[1] = 0x1234_5600;
        run(&mut c, &[0xFA5F, 0xF091], 1);
        assert_eq!(c.regs[0], 0x56);

        // uxth.w r2, r3, ror #16 (0xFA1FF2A3): rotate then zero-extend.
        let mut c = core();
        c.regs[3] = 0x5678_0000;
        run(&mut c, &[0xFA1F, 0xF2A3], 1);
        assert_eq!(c.regs[2], 0x5678);

        // sxtb.w r4, r5, ror #24 (0xFA4FF4B5): rotate then sign-extend.
        let mut c = core();
        c.regs[5] = 0x8000_0000;
        run(&mut c, &[0xFA4F, 0xF4B5], 1);
        assert_eq!(c.regs[4], 0xFFFF_FF80);
    }

    #[test]
    fn thumb2_ldrex_strex() {
        // ldrex r0, [r1] (0xE8510F00): reads the word at r1.
        let mut c = core();
        c.regs[1] = 0x100;
        c.write_u32(0x100, 0xDEAD_BEEF);
        run(&mut c, &[0xE851, 0x0F00], 1);
        assert_eq!(c.regs[0], 0xDEAD_BEEF);
        assert_eq!(c.regs[PC], 4);

        // strex r2, r3, [r4] (0xE8443200): stores the word and records success.
        let mut c = core();
        c.regs[4] = 0x100;
        c.regs[3] = 0xCAFE_BABE;
        run(&mut c, &[0xE844, 0x3200], 1);
        assert_eq!(c.regs[2], 0, "exclusive store succeeds on Cortex-M");
        assert_eq!(c.read_u32(0x100), 0xCAFE_BABE);

        // ldrexb r5, [r6] (0xE8D65F4F): reads the byte at r6.
        let mut c = core();
        c.regs[6] = 0x100;
        c.write_u8(0x100, 0x7F);
        run(&mut c, &[0xE8D6, 0x5F4F], 1);
        assert_eq!(c.regs[5], 0x7F);

        // strexb r7, r8, [r9] (0xE8C98F47): stores the byte and records success.
        let mut c = core();
        c.regs[9] = 0x100;
        c.regs[8] = 0xAB;
        run(&mut c, &[0xE8C9, 0x8F47], 1);
        assert_eq!(c.regs[7], 0, "exclusive byte store succeeds");
        assert_eq!(c.read_u8(0x100), 0xAB);

        // ldrexh r10, [r11] (0xE8DBAF5F): reads the halfword at r11.
        let mut c = core();
        c.regs[11] = 0x100;
        c.write_u16(0x100, 0xBEEF);
        run(&mut c, &[0xE8DB, 0xAF5F], 1);
        assert_eq!(c.regs[10], 0xBEEF);

        // strexh r12, r0, [r1] (0xE8C10F5C): stores the halfword and records
        // success.
        let mut c = core();
        c.regs[1] = 0x100;
        c.regs[0] = 0x1234;
        run(&mut c, &[0xE8C1, 0x0F5C], 1);
        assert_eq!(c.regs[12], 0, "exclusive halfword store succeeds");
        assert_eq!(c.read_u16(0x100), 0x1234);
    }

    #[test]
    fn thumb2_ldrsb_ldrsh() {
        // ldrsb.w r6, [r7, r8, lsl #1] (0xF9176018): sign-extend byte at
        // r7 + (r8 << 1).
        let mut c = core();
        c.regs[7] = 0x100;
        c.regs[8] = 1;
        c.write_u8(0x102, 0x80);
        run(&mut c, &[0xF917, 0x6018], 1);
        assert_eq!(c.regs[6], 0xFFFF_FF80);
        assert_eq!(c.regs[PC], 4);

        // ldrsh.w r9, [r10, r11, lsl #2] (0xF93A902B): sign-extend halfword
        // at r10 + (r11 << 2).
        let mut c = core();
        c.regs[10] = 0x200;
        c.regs[11] = 1;
        c.write_u16(0x204, 0x8000);
        run(&mut c, &[0xF93A, 0x902B], 1);
        assert_eq!(c.regs[9], 0xFFFF_8000);

        // ldrsb.w r0, [r1, #4] (0xF9910004): sign-extend byte at r1 + 4.
        let mut c = core();
        c.regs[1] = 0x100;
        c.write_u8(0x104, 0x7F);
        run(&mut c, &[0xF991, 0x0004], 1);
        assert_eq!(c.regs[0], 0x7F);

        // ldrsh.w r2, [r3, #8] (0xF9B32008): sign-extend halfword at r3 + 8.
        let mut c = core();
        c.regs[3] = 0x200;
        c.write_u16(0x208, 0x1234);
        run(&mut c, &[0xF9B3, 0x2008], 1);
        assert_eq!(c.regs[2], 0x1234);

        // ldrsb.w r0, [pc, #4] (0xF99F0004): literal base is Align(PC, 4).
        let mut c = core();
        c.write_u8(0x8, 0x80);
        run(&mut c, &[0xF99F, 0x0004], 1);
        assert_eq!(c.regs[0], 0xFFFF_FF80);

        // ldrsh.w r1, [pc, #8] (0xF9BF1008): literal base is Align(PC, 4).
        let mut c = core();
        c.write_u16(0xC, 0x8000);
        run(&mut c, &[0xF9BF, 0x1008], 1);
        assert_eq!(c.regs[1], 0xFFFF_8000);
    }

    #[test]
    fn thumb2_sdiv_udiv() {
        // SDIV r0, r1, r2 (0xFB91F0F2): signed division.
        let mut c = core();
        c.regs[1] = (-7i32) as u32;
        c.regs[2] = 2;
        run(&mut c, &[0xFB91, 0xF0F2], 1);
        assert_eq!(c.regs[0], (-3i32) as u32); // -7 / 2 truncates toward zero.
        assert_eq!(c.regs[PC], 4);

        // UDIV r3, r4, r5 (0xFBB4F3F5): unsigned division.
        let mut c = core();
        c.regs[4] = 0xFFFF_FFFF;
        c.regs[5] = 0x10000;
        run(&mut c, &[0xFBB4, 0xF3F5], 1);
        assert_eq!(c.regs[3], 0xFFFF); // 0xFFFFFFFF / 0x10000.

        // Division by zero yields zero.
        let mut c = core();
        c.regs[1] = 100;
        c.regs[2] = 0;
        run(&mut c, &[0xFB91, 0xF0F2], 1);
        assert_eq!(c.regs[0], 0);
    }

    #[test]
    fn thumb2_ldr_str_register_offset() {
        // LDR r0, [r1, r2] (0xF8510002): word load at r1 + r2.
        let mut c = core();
        c.regs[1] = 0x100;
        c.regs[2] = 4;
        c.write_u32(0x104, 0xDEAD_BEEF);
        run(&mut c, &[0xF851, 0x0002], 1);
        assert_eq!(c.regs[0], 0xDEAD_BEEF);

        // LDR r0, [r1, r2, lsl #2] (0xF8510022): word load at r1 + r2<<2.
        let mut c = core();
        c.regs[1] = 0x100;
        c.regs[2] = 4; // imm2 = 2 -> r2<<2 = 16.
        c.write_u32(0x110, 0xCAFE_1234);
        run(&mut c, &[0xF851, 0x0022], 1);
        assert_eq!(c.regs[0], 0xCAFE_1234);

        // STR r3, [r4, r5] (0xF8443005): word store.
        let mut c = core();
        c.regs[3] = 0x1234_5678;
        c.regs[4] = 0x200;
        c.regs[5] = 0x10;
        run(&mut c, &[0xF844, 0x3005], 1);
        assert_eq!(c.read_u32(0x210), 0x1234_5678);

        // LDRB r6, [r7, r8] (0xF8176008): byte load.
        let mut c = core();
        c.regs[7] = 0x300;
        c.regs[8] = 2;
        c.write_u8(0x302, 0xAB);
        run(&mut c, &[0xF817, 0x6008], 1);
        assert_eq!(c.regs[6], 0xAB);

        // LDRH r12, [r0, r1] (0xF830C001): halfword load.
        let mut c = core();
        c.regs[0] = 0x400;
        c.regs[1] = 6;
        c.write_u16(0x406, 0xCDEF);
        run(&mut c, &[0xF830, 0xC001], 1);
        assert_eq!(c.regs[12], 0xCDEF);
    }

    #[test]
    fn thumb2_ldr_literal() {
        // LDR r0, [pc, #0x10] (0xF8DF0010): PC aligned + imm12.
        let mut c = core();
        c.write_u32(0x14, 0x1122_3344);
        run(&mut c, &[0xF8DF, 0x0010], 1);
        assert_eq!(c.regs[0], 0x1122_3344);
        assert_eq!(c.regs[PC], 4);

        // LDRB r6, [pc, #0x20] (0xF89F6020).
        let mut c = core();
        c.write_u8(0x24, 0x77);
        run(&mut c, &[0xF89F, 0x6020], 1);
        assert_eq!(c.regs[6], 0x77);

        // LDRH r7, [pc, #0x40] (0xF8BF7040).
        let mut c = core();
        c.write_u16(0x44, 0x8899);
        run(&mut c, &[0xF8BF, 0x7040], 1);
        assert_eq!(c.regs[7], 0x8899);
    }

    #[test]
    fn thumb2_ldm_stm_multiple() {
        // STMIA r0!, {r1,r2,r3} (0xE8A0000E): store ascending, write back.
        let mut c = core();
        c.regs[0] = 0x100;
        c.regs[1] = 1;
        c.regs[2] = 2;
        c.regs[3] = 3;
        run(&mut c, &[0xE8A0, 0x000E], 1);
        assert_eq!(c.read_u32(0x100), 1);
        assert_eq!(c.read_u32(0x104), 2);
        assert_eq!(c.read_u32(0x108), 3);
        assert_eq!(c.regs[0], 0x10C);
        assert_eq!(c.regs[PC], 4);

        // LDMIA r0!, {r1,r2,r3} (0xE8B0000E): load ascending, write back.
        let mut c = core();
        c.regs[0] = 0x100;
        c.write_u32(0x100, 11);
        c.write_u32(0x104, 22);
        c.write_u32(0x108, 33);
        run(&mut c, &[0xE8B0, 0x000E], 1);
        assert_eq!(c.regs[1], 11);
        assert_eq!(c.regs[2], 22);
        assert_eq!(c.regs[3], 33);
        assert_eq!(c.regs[0], 0x10C);

        // LDMDB r0!, {r1,r2} (0xE9300006): decrement before, write back.
        let mut c = core();
        c.regs[0] = 0x108;
        c.write_u32(0x100, 0xAA);
        c.write_u32(0x104, 0xBB);
        run(&mut c, &[0xE930, 0x0006], 1);
        assert_eq!(c.regs[1], 0xAA);
        assert_eq!(c.regs[2], 0xBB);
        assert_eq!(c.regs[0], 0x100);

        // STMDB sp!, {r4, lr} (0xE92D4010) = PUSH {r4, lr}.
        let mut c = core();
        c.regs[13] = 0x200;
        c.regs[4] = 0x44;
        c.regs[14] = 0x55;
        run(&mut c, &[0xE92D, 0x4010], 1);
        assert_eq!(c.read_u32(0x1F8), 0x44);
        assert_eq!(c.read_u32(0x1FC), 0x55);
        assert_eq!(c.regs[13], 0x1F8);
    }

    #[test]
    fn thumb2_data_processing_register() {
        // AND r0, r1, r2 (0xEA010002).
        let mut c = core();
        c.regs[1] = 0xF0F0;
        c.regs[2] = 0xFF00;
        run(&mut c, &[0xEA01, 0x0002], 1);
        assert_eq!(c.regs[0], 0xF000);

        // ORR r6, r7, r8 (0xEA470608).
        let mut c = core();
        c.regs[7] = 0x0F0F;
        c.regs[8] = 0xF0F0;
        run(&mut c, &[0xEA47, 0x0608], 1);
        assert_eq!(c.regs[6], 0xFFFF);

        // EOR r3, r4, r5 (0xEA840305).
        let mut c = core();
        c.regs[4] = 0xFFFF;
        c.regs[5] = 0x00FF;
        run(&mut c, &[0xEA84, 0x0305], 1);
        assert_eq!(c.regs[3], 0xFF00);

        // BIC r9, r10, r11 (0xEA2A090B).
        let mut c = core();
        c.regs[10] = 0xFFFF;
        c.regs[11] = 0x00FF;
        run(&mut c, &[0xEA2A, 0x090B], 1);
        assert_eq!(c.regs[9], 0xFF00);

        // ADD r3, r4, r5 (0xEB040305).
        let mut c = core();
        c.regs[4] = 5;
        c.regs[5] = 7;
        run(&mut c, &[0xEB04, 0x0305], 1);
        assert_eq!(c.regs[3], 12);

        // ADDS r0, r1, r2 (0xEB110002): 0xFFFFFFFF + 1 = 0, C set, Z set.
        let mut c = core();
        c.regs[1] = 0xFFFF_FFFF;
        c.regs[2] = 1;
        run(&mut c, &[0xEB11, 0x0002], 1);
        assert_eq!(c.regs[0], 0);
        assert!(c.flag_z() && c.flag_c() && !c.flag_n() && !c.flag_v());

        // ADC r6, r7, r8 (0xEB470608) with carry in: 5 + 7 + 1 = 13.
        let mut c = core();
        c.regs[7] = 5;
        c.regs[8] = 7;
        c.set_nzcv(false, false, true, false);
        run(&mut c, &[0xEB47, 0x0608], 1);
        assert_eq!(c.regs[6], 13);

        // SUB r0, r1, r2 (0xEBA10002).
        let mut c = core();
        c.regs[1] = 10;
        c.regs[2] = 3;
        run(&mut c, &[0xEBA1, 0x0002], 1);
        assert_eq!(c.regs[0], 7);

        // RSB r3, r4, r5 (0xEBC40305): 10 - 3 = 7.
        let mut c = core();
        c.regs[4] = 3;
        c.regs[5] = 10;
        run(&mut c, &[0xEBC4, 0x0305], 1);
        assert_eq!(c.regs[3], 7);
        assert_eq!(c.regs[PC], 4);
    }

    #[test]
    fn thumb2_mov_mvn_compare() {
        // MOV r8, r9 (0xEA4F0908).
        let mut c = core();
        c.regs[9] = 0x1234_5678;
        run(&mut c, &[0xEA4F, 0x0908], 1);
        assert_eq!(c.regs[8], 0x1234_5678);

        // MVN r10, r11 (0xEA6F0B0A): logical NOT.
        let mut c = core();
        c.regs[11] = 0x0000_FFFF;
        run(&mut c, &[0xEA6F, 0x0B0A], 1);
        assert_eq!(c.regs[10], 0xFFFF_0000);

        // CMP r0, r1 (0xEBB00F01): 5 - 3, C set, Z clear, N clear.
        let mut c = core();
        c.regs[0] = 5;
        c.regs[1] = 3;
        run(&mut c, &[0xEBB0, 0x0F01], 1);
        assert!(c.flag_c() && !c.flag_z() && !c.flag_n());
        assert_eq!(c.regs[0], 5); // CMP does not write the destination.

        // CMP r0, r1 equal: Z set.
        let mut c = core();
        c.regs[0] = 5;
        c.regs[1] = 5;
        run(&mut c, &[0xEBB0, 0x0F01], 1);
        assert!(c.flag_z());

        // TST r2, r3 (0xEA120F03): 0xF & 0xF = 0xF, Z clear.
        let mut c = core();
        c.regs[2] = 0xF;
        c.regs[3] = 0xF;
        run(&mut c, &[0xEA12, 0x0F03], 1);
        assert!(!c.flag_z());
    }

    #[test]
    fn thumb2_data_processing_shifted_register() {
        // ADD r2, r3, r4, lsl #3 (0xEB0302C4): 1 + (2 << 3) = 17.
        let mut c = core();
        c.regs[3] = 1;
        c.regs[4] = 2;
        run(&mut c, &[0xEB03, 0x02C4], 1);
        assert_eq!(c.regs[2], 17);

        // ADD r2, r3, r4, asr #2 (0xEB0302A4): 0x80000000 >> 2 = 0xE0000000.
        let mut c = core();
        c.regs[3] = 0;
        c.regs[4] = 0x8000_0000;
        run(&mut c, &[0xEB03, 0x02A4], 1);
        assert_eq!(c.regs[2], 0xE000_0000);

        // ADD r2, r3, r4, rrx (0xEB030234): RRX of 1 with C=1 → 0x80000000.
        let mut c = core();
        c.regs[3] = 0;
        c.regs[4] = 1;
        c.set_nzcv(false, false, true, false);
        run(&mut c, &[0xEB03, 0x0234], 1);
        assert_eq!(c.regs[2], 0x8000_0000);

        // ADDS r6, r6, r4, lsr #2 (0xEB160694): (0x100 >> 2) = 0x40, Z clear.
        let mut c = core();
        c.regs[6] = 0;
        c.regs[4] = 0x100;
        run(&mut c, &[0xEB16, 0x0694], 1);
        assert_eq!(c.regs[6], 0x40);
        assert!(!c.flag_z());
    }

    #[test]
    fn thumb2_data_processing_modified_immediate() {
        // mov.w r0, #0xff (0xF04F00FF): byte-pattern immediate.
        let mut c = core();
        run(&mut c, &[0xF04F, 0x00FF], 1);
        assert_eq!(c.regs[0], 0xFF);

        // mov.w r0, #0x80000000 (0xF04F4000): rotation immediate.
        let mut c = core();
        run(&mut c, &[0xF04F, 0x4000], 1);
        assert_eq!(c.regs[0], 0x8000_0000);

        // mov.w r0, #0xff00ff (0xF04F10FF): 0x00FF00FF byte pattern.
        let mut c = core();
        run(&mut c, &[0xF04F, 0x10FF], 1);
        assert_eq!(c.regs[0], 0x00FF_00FF);

        // mov.w r0, #0x100 (0xF44F7080).
        let mut c = core();
        run(&mut c, &[0xF44F, 0x7080], 1);
        assert_eq!(c.regs[0], 0x100);

        // mov.w r0, #0xffffffff (0xF04F30FF).
        let mut c = core();
        run(&mut c, &[0xF04F, 0x30FF], 1);
        assert_eq!(c.regs[0], 0xFFFF_FFFF);

        // movs.w r2, #1 (0xF05F0201): Z clear.
        let mut c = core();
        run(&mut c, &[0xF05F, 0x0201], 1);
        assert_eq!(c.regs[2], 1);
        assert!(!c.flag_z());

        // add.w r3, r4, #0x40 (0xF1040340): 5 + 0x40 = 0x45.
        let mut c = core();
        c.regs[4] = 5;
        run(&mut c, &[0xF104, 0x0340], 1);
        assert_eq!(c.regs[3], 0x45);

        // add.w r0, r0, #0x100 (0xF5007080).
        let mut c = core();
        c.regs[0] = 1;
        run(&mut c, &[0xF500, 0x7080], 1);
        assert_eq!(c.regs[0], 0x101);

        // sub.w r5, r6, #0x200 (0xF5A67500).
        let mut c = core();
        c.regs[6] = 0x300;
        run(&mut c, &[0xF5A6, 0x7500], 1);
        assert_eq!(c.regs[5], 0x100);

        // and r7, r8, #0xf0 (0xF00807F0).
        let mut c = core();
        c.regs[8] = 0xFF;
        run(&mut c, &[0xF008, 0x07F0], 1);
        assert_eq!(c.regs[7], 0xF0);

        // orr r9, r10, #0x8000 (0xF44A4900).
        let mut c = core();
        c.regs[10] = 1;
        run(&mut c, &[0xF44A, 0x4900], 1);
        assert_eq!(c.regs[9], 0x8001);

        // cmp.w r0, #0x40 (0xF1B00F40): r0 = 0x40 → Z set, C set.
        let mut c = core();
        c.regs[0] = 0x40;
        run(&mut c, &[0xF1B0, 0x0F40], 1);
        assert!(c.flag_z() && c.flag_c() && !c.flag_n() && !c.flag_v());

        // mvn r9, #0xff (0xF06F09FF).
        let mut c = core();
        run(&mut c, &[0xF06F, 0x09FF], 1);
        assert_eq!(c.regs[9], 0xFFFF_FF00);

        // rsb.w r6, r7, #0xff (0xF1C706FF): 0 - r7 + 0xff.
        let mut c = core();
        c.regs[7] = 0x10;
        run(&mut c, &[0xF1C7, 0x06FF], 1);
        assert_eq!(c.regs[6], 0xEF);

        // tst.w r2, #0xff (0xF0120FFF): 0x100 & 0xff = 0 → Z set.
        let mut c = core();
        c.regs[2] = 0x100;
        run(&mut c, &[0xF012, 0x0FFF], 1);
        assert!(c.flag_z());
    }

    #[test]
    fn thumb_expand_imm12_matches_encodings() {
        // Cross-check the modified-immediate expansion against values emitted
        // by clang for a single-bit immediate across every rotation.
        let cases: &[(u32, u32)] = &[
            (0x0FF, 0x0000_00FF),
            (0x1FF, 0x00FF_00FF),
            (0x2FF, 0xFF00_FF00),
            (0x3FF, 0xFFFF_FFFF),
            (0x400, 0x8000_0000),
            (0x47F, 0xFF00_0000),
            (0x480, 0x4000_0000),
            (0x500, 0x2000_0000),
            (0x580, 0x1000_0000),
            (0x600, 0x0800_0000),
            (0x680, 0x0400_0000),
            (0x700, 0x0200_0000),
            (0x780, 0x0100_0000),
            (0x800, 0x0080_0000),
            (0x880, 0x0040_0000),
            (0x900, 0x0020_0000),
            (0x980, 0x0010_0000),
            (0xA00, 0x0008_0000),
            (0xA80, 0x0004_0000),
            (0xB00, 0x0002_0000),
            (0xB80, 0x0001_0000),
            (0xC00, 0x0000_8000),
            (0xC80, 0x0000_4000),
            (0xD00, 0x0000_2000),
            (0xD80, 0x0000_1000),
            (0xE00, 0x0000_0800),
            (0xE80, 0x0000_0400),
            (0xF00, 0x0000_0200),
            (0xF80, 0x0000_0100),
            (0x080, 0x0000_0080),
            (0x87F, 0x00FF_0000),
        ];
        for (imm12, expected) in cases {
            let (value, _) = thumb_expand_imm12(*imm12, false);
            assert_eq!(value, *expected, "imm12 {imm12:#05x}");
        }
    }

    #[test]
    fn thumb2_tbb_tbh() {
        // TBB [r3, r4] (0xE8D3F004): index byte at r3 + r4.
        let mut c = core();
        c.write_u16(0x1000, 0xE8D3); // TBB at 0x1000.
        c.write_u16(0x1002, 0xF004);
        c.regs[PC] = 0x1000;
        c.set_thumb(true);
        c.regs[3] = 0x1100;
        c.regs[4] = 0x02;
        c.write_u8(0x1102, 0x20);
        c.step(&[]);
        // ARMv7-M: PC = current+4 (0x1004), base = Align(PC,4) = 0x1004.
        assert_eq!(c.regs[PC], 0x1004 + 0x40);

        // TBH [r3, r4, lsl #1] (0xE8D3F014): halfword index at r3 + r4<<1.
        let mut c = core();
        c.write_u16(0x2000, 0xE8D3); // TBH at 0x2000.
        c.write_u16(0x2002, 0xF014);
        c.regs[PC] = 0x2000;
        c.set_thumb(true);
        c.regs[3] = 0x2100;
        c.regs[4] = 0x02;
        c.write_u16(0x2104, 0x10);
        c.step(&[]);
        // PC = current+4 (0x2004), base = Align(PC,4) = 0x2004.
        assert_eq!(c.regs[PC], 0x2004 + 0x20);
    }

    #[test]
    fn thumb2_ldrd_strd() {
        // LDRD r0, r1, [r2] (0xE9D20100): no writeback, offset 0.
        let mut c = core();
        c.regs[2] = 0x200;
        c.write_u32(0x200, 0x1111_1111);
        c.write_u32(0x204, 0x2222_2222);
        run(&mut c, &[0xE9D2, 0x0100], 1);
        assert_eq!(c.regs[0], 0x1111_1111);
        assert_eq!(c.regs[1], 0x2222_2222);
        assert_eq!(c.regs[2], 0x200); // no writeback.
        assert_eq!(c.regs[PC], 4);

        // LDRD r0, r1, [r2, #8]! (0xE9F20102): pre-index with writeback.
        let mut c = core();
        c.regs[2] = 0x200;
        c.write_u32(0x208, 0xAAAA_AAAA);
        c.write_u32(0x20C, 0xBBBB_BBBB);
        run(&mut c, &[0xE9F2, 0x0102], 1);
        assert_eq!(c.regs[0], 0xAAAA_AAAA);
        assert_eq!(c.regs[1], 0xBBBB_BBBB);
        assert_eq!(c.regs[2], 0x208); // writeback to 0x200 + 8.

        // LDRD r8, r9, [r10, #-4] (0xE95A8901): negative offset, no writeback.
        let mut c = core();
        c.regs[10] = 0x300;
        c.write_u32(0x2FC, 0x1234_5678);
        c.write_u32(0x300, 0x9ABC_DEF0);
        run(&mut c, &[0xE95A, 0x8901], 1);
        assert_eq!(c.regs[8], 0x1234_5678);
        assert_eq!(c.regs[9], 0x9ABC_DEF0);

        // STRD r4, r5, [r6, #16] (0xE9C64504): store pair, no writeback.
        let mut c = core();
        c.regs[4] = 0xCAFE;
        c.regs[5] = 0xBEEF;
        c.regs[6] = 0x300;
        run(&mut c, &[0xE9C6, 0x4504], 1);
        assert_eq!(c.read_u32(0x310), 0xCAFE);
        assert_eq!(c.read_u32(0x314), 0xBEEF);
    }

    #[test]
    fn thumb1_shift_zero_and_wide_amounts() {
        // ASRS r0, r1, #0 (0x1008) = ASR #32: result sign-extends, C = bit 31.
        let mut c = core();
        c.regs[1] = 0x8000_0000;
        run(&mut c, &[0x1008], 1);
        assert_eq!(c.regs[0], 0xFFFF_FFFF);
        assert!(c.flag_n() && c.flag_c());

        // LSRS r0, r1, #0 (0x0808) = LSR #32: result 0, C = bit 31.
        let mut c = core();
        c.regs[1] = 0x8000_0000;
        run(&mut c, &[0x0808], 1);
        assert_eq!(c.regs[0], 0);
        assert!(c.flag_z() && c.flag_c());

        // LSLS r0, r1, #0 (0x0008): no shift, carry unchanged.
        let mut c = core();
        c.regs[1] = 5;
        run(&mut c, &[0x0008], 1);
        assert_eq!(c.regs[0], 5);

        // Register shift with a wide amount must not panic: LSRS r0, r1
        // (0x40C8) with r1 = 40 yields 0.
        let mut c = core();
        c.regs[1] = 40;
        run(&mut c, &[0x40C8], 1);
        assert_eq!(c.regs[0], 0);
        assert!(c.flag_z());
    }

    #[test]
    fn thumb1_ldm_stm_writeback() {
        // STMIA r0!, {r1} (0xC002): stores r1 and writes back r0.
        let mut c = core();
        c.regs[0] = 0x1000;
        c.regs[1] = 0x1234_5678;
        run(&mut c, &[0xC002], 1);
        assert_eq!(c.read_u32(0x1000), 0x1234_5678);
        assert_eq!(c.regs[0], 0x1004);

        // LDMIA r0!, {r1} (0xC802): loads r1 and writes back r0.
        let mut c = core();
        c.regs[0] = 0x1000;
        c.write_u32(0x1000, 0xCAFE_BABE);
        run(&mut c, &[0xC802], 1);
        assert_eq!(c.regs[1], 0xCAFE_BABE);
        assert_eq!(c.regs[0], 0x1004);

        // LDMIA r0!, {r0} (0xC801): rn in the list, writeback suppressed.
        let mut c = core();
        c.regs[0] = 0x1000;
        c.write_u32(0x1000, 0xDEAD_BEEF);
        run(&mut c, &[0xC801], 1);
        assert_eq!(c.regs[0], 0xDEAD_BEEF);
    }

    #[test]
    fn load_store_word() {
        let mut c = core();
        c.regs[0] = 0x2000_0000;
        c.regs[1] = 0xDEAD_BEEF;
         // STR r1, [r0, #0]  (0x6008: Rn=0, Rt=1)
         // LDR r2, [r0, #0]  (0x6810: Rn=0, Rt=2)
         run(&mut c, &[0x6008, 0x6810], 2);
        assert_eq!(c.regs[2], 0xDEAD_BEEF);
    }

    #[test]
    fn push_pop_restores_stack() {
        let mut c = core();
        c.regs[SP] = 0x2000_1000;
        c.regs[0] = 0x1111_1111;
        c.regs[1] = 0x2222_2222;
        // PUSH {r0, r1}  (0xB403)
        // POP {r0, r1}   (0xBC03)
        run(&mut c, &[0xB403, 0xBC03], 2);
        assert_eq!(c.regs[0], 0x1111_1111);
        assert_eq!(c.regs[1], 0x2222_2222);
        assert_eq!(c.regs[SP], 0x2000_1000);
    }

    #[test]
    fn conditional_branch_taken() {
        let mut c = core();
        c.regs[0] = 0;
        // MOVS r0, #0 (0x2000); BEQ +2 (0xD000): EQ is cond 0, offset 0
        // The BEQ with offset 0 loops; use a branch forward.
        // BEQ .+4: 0xD001 (offset 1 halfword = 2 bytes)
        run(&mut c, &[0x2000, 0xD001, 0x2000], 2);
        assert_eq!(c.regs[0], 0);
    }

    #[test]
    fn unconditional_branch() {
        let mut c = core();
        // B .+6: 0xE001 (offset 1 halfword = 2 bytes, base = current + 4).
        run(&mut c, &[0xE001, 0xE000], 1);
        assert_eq!(c.regs[PC], 6);
    }

    #[test]
    fn conditional_branch_target_base() {
        // BEQ 0xD001 at address 2 (offset 2 bytes): with Z set the target is
        // current + 4 + 2 = 8 (architectural PC base, not current + 2).
        let mut c = core();
        c.regs[0] = 0;
        run(&mut c, &[0x2000, 0xD001], 2); // MOVS r0,#0 sets Z, then BEQ taken.
        assert_eq!(c.regs[PC], 8);

        // The same BEQ with Z clear falls through to current + 2 = 2.
        let mut c = core();
        c.regs[0] = 1;
        run(&mut c, &[0x2001, 0xD001], 2); // MOVS r0,#1 clears Z, BEQ not taken.
        assert_eq!(c.regs[PC], 4);
    }

    #[test]
    fn bl_sets_link_register() {
        let mut c = core();
        // BL with offset 0: first=0xF000, second=0xD000 (link set)
        c.write_u16(0, 0xF000);
        c.write_u16(2, 0xD000);
        c.regs[PC] = 0;
        c.set_thumb(true);
        let r = c.step(&[]);
        assert_eq!(r, ArmStep::Advanced);
        assert_eq!(c.regs[LR], 4);
        assert_eq!(c.regs[PC], 4);
    }

    #[test]
    fn bw_unconditional_branches() {
        // B.W at address 0 with imm32=8: target = (current + 4) + 8 = 0xc.
        // first=0xF000, second=0x9004 (imm11=4, j1=j2=s=0).
        let mut c = core();
        c.write_u16(0, 0xF000);
        c.write_u16(2, 0x9004);
        c.regs[PC] = 0;
        c.set_thumb(true);
        assert_eq!(c.step(&[]), ArmStep::Advanced);
        assert_eq!(c.regs[PC], 0xc);
    }

    #[test]
    fn execute_wide_ldrh_strh_and_barriers() {
        // LDRH r0, [r0, #0x0] (0xF8B00000).
        let mut c = core();
        c.write_u16(0, 0xF8B0);
        c.write_u16(2, 0x0000);
        c.regs[PC] = 0;
        c.regs[0] = 0x1000;
        c.write_u16(0x1000, 0xABCD);
        c.set_thumb(true);
        assert_eq!(c.step(&[]), ArmStep::Advanced);
        assert_eq!(c.regs[0], 0xABCD);
        assert_eq!(c.regs[PC], 4);

        // STRH r1, [r0, #0x0] (0xF8A01000): stores the low halfword.
        let mut c = core();
        c.write_u16(0, 0xF8A0);
        c.write_u16(2, 0x1000);
        c.regs[PC] = 0;
        c.regs[0] = 0x1000;
        c.regs[1] = 0xABCD_EF01;
        c.set_thumb(true);
        assert_eq!(c.step(&[]), ArmStep::Advanced);
        assert_eq!(c.read_u16(0x1000), 0xEF01);
        assert_eq!(c.regs[PC], 4);

        // DMB/DSB/ISB (0xF3BF 0x8F5F/8F6F/8F7F) are no-ops, not halts.
        for second in [0x8F5Fu16, 0x8F6F, 0x8F7F] {
            let mut c = core();
            c.write_u16(0, 0xF3BF);
            c.write_u16(2, second);
            c.regs[PC] = 0;
            c.set_thumb(true);
            assert_eq!(c.step(&[]), ArmStep::Advanced, "barrier {second:#06x}");
            assert_eq!(c.regs[PC], 4);
        }
    }

    #[test]
    fn thumb1_bx_blx_register() {
        // BX r0 (0x4700) branches through bit 0 without linking.
        let mut c = core();
        c.regs[0] = 0x0800_0009; // Thumb target.
        run(&mut c, &[0x4700], 1);
        assert_eq!(c.regs[PC], 0x0800_0008);
        assert!(c.thumb());
        assert_eq!(c.regs[LR], 0);

        // BLX r0 (0x4780) also links the return address (PC after advance).
        let mut c = core();
        c.regs[0] = 0x0800_0009;
        run(&mut c, &[0x4780], 1);
        assert_eq!(c.regs[PC], 0x0800_0008);
        assert!(c.thumb());
        assert_eq!(c.regs[LR], 2);
    }

    #[test]
    fn arm_a32_halfword_transfer_register_offset() {
        // LDRH r0, [r1, r2] (0xE19100B2): base + register offset.
        let mut c = core();
        c.write_u32(0, 0xE191_00B2);
        c.set_arm_state(true);
        c.regs[PC] = 0;
        c.regs[1] = 0x1000;
        c.regs[2] = 4;
        c.write_u16(0x1004, 0xABCD);
        assert_eq!(c.step(&[]), ArmStep::Advanced);
        assert_eq!(c.regs[0], 0xABCD);
        assert_eq!(c.regs[PC], 4);

        // STRH r1, [r0, r2] (0xE18010B2): stores the low halfword.
        let mut c = core();
        c.write_u32(0, 0xE180_10B2);
        c.set_arm_state(true);
        c.regs[PC] = 0;
        c.regs[0] = 0x1000;
        c.regs[1] = 0xABCD_EF01;
        c.regs[2] = 4;
        assert_eq!(c.step(&[]), ArmStep::Advanced);
        assert_eq!(c.read_u16(0x1004), 0xEF01);
        assert_eq!(c.regs[PC], 4);
    }

    #[test]
    fn arm_a32_ldrd_strd() {
        // LDRD r0, r1, [r2, #8] (0xE1C200D8): loads the word pair at base+8.
        let mut c = core();
        c.write_u32(0, 0xE1C2_00D8);
        c.set_arm_state(true);
        c.regs[PC] = 0;
        c.regs[2] = 0x1000;
        c.write_u32(0x1008, 0x1111_2222);
        c.write_u32(0x100C, 0x3333_4444);
        assert_eq!(c.step(&[]), ArmStep::Advanced);
        assert_eq!(c.regs[0], 0x1111_2222);
        assert_eq!(c.regs[1], 0x3333_4444);
        assert_eq!(c.regs[PC], 4);

        // LDRD r0, r1, [r2] (0xE1C200D0): offset 0, no writeback.
        let mut c = core();
        c.write_u32(0, 0xE1C2_00D0);
        c.set_arm_state(true);
        c.regs[PC] = 0;
        c.regs[2] = 0x1000;
        c.write_u32(0x1000, 0xAAAA_BBBB);
        c.write_u32(0x1004, 0xCCCC_DDDD);
        assert_eq!(c.step(&[]), ArmStep::Advanced);
        assert_eq!(c.regs[0], 0xAAAA_BBBB);
        assert_eq!(c.regs[1], 0xCCCC_DDDD);

        // STRD r4, r5, [r6, #8]! (0xE1E640F8): stores the pair and writes back.
        let mut c = core();
        c.write_u32(0, 0xE1E6_40F8);
        c.set_arm_state(true);
        c.regs[PC] = 0;
        c.regs[4] = 0x0102_0304;
        c.regs[5] = 0x0506_0708;
        c.regs[6] = 0x1000;
        assert_eq!(c.step(&[]), ArmStep::Advanced);
        assert_eq!(c.read_u32(0x1008), 0x0102_0304);
        assert_eq!(c.read_u32(0x100C), 0x0506_0708);
        assert_eq!(c.regs[6], 0x1008); // writeback
        assert_eq!(c.regs[PC], 4);
    }

    #[test]
    fn reset_from_vector_table() {
        let mut c = core();
        c.write_u32(0, 0x2000_0000); // initial SP
        c.write_u32(4, 0x0000_0009); // reset vector | 1 (Thumb)
        c.reset_from_vector_table(0, 0x0000_0008);
        assert_eq!(c.regs[SP], 0x2000_0000);
        assert_eq!(c.regs[PC], 0x0000_0008);
        assert!(c.thumb());
    }

    #[test]
    fn memory_barrier_is_nop() {
        let mut c = core();
        // DMB: 0xF3BF 0x8F5F
        c.write_u16(0, 0xF3BF);
        c.write_u16(2, 0x8F5F);
        c.regs[PC] = 0;
        c.set_thumb(true);
        let r = c.step(&[]);
        assert_eq!(r, ArmStep::Advanced);
        assert_eq!(c.regs[PC], 4);
    }

    #[test]
    fn movw_wide_loads_immediate() {
        let mut c = core();
        // MOVW r0, #0x1234: first=0xF241 (imm4=1), second=0x4034
        c.write_u16(0, 0xF241);
        c.write_u16(2, 0x4034);
        c.regs[PC] = 0;
        c.set_thumb(true);
        let r = c.step(&[]);
        assert_eq!(r, ArmStep::Advanced);
        assert_eq!(c.regs[0], 0x1234);
    }

    // -------- ARM state (A32) --------

    fn run_arm(core: &mut ArmCore, insns: &[u32], steps: usize) {
        for (i, insn) in insns.iter().enumerate() {
            core.write_u32((i * 4) as u32, *insn);
        }
        core.regs[PC] = 0;
        core.set_thumb(false);
        core.arm_state = true;
        for _ in 0..steps {
            let r = core.step(&[]);
            if r == ArmStep::Halted {
                break;
            }
        }
    }

    #[test]
    fn arm_mov_immediate_and_add_register() {
        let mut c = core();
        // MOV r0, #5 (0xE3A00005); ADD r0, r0, #7 (0xE2800007)
        run_arm(&mut c, &[0xE3A0_0005, 0xE280_0007], 2);
        assert_eq!(c.regs[0], 12);
        assert_eq!(c.regs[PC], 8);
    }

    #[test]
    fn arm_a32_breakpoint_stops_before_execute() {
        let mut c = core();
        // MOV r0, #5 (0xE3A00005); ADD r0, r0, #7 (0xE2800007)
        c.write_u32(0, 0xE3A0_0005);
        c.write_u32(4, 0xE280_0007);
        c.regs[PC] = 0;
        c.set_thumb(false);
        c.arm_state = true;
        // Breakpoint at the ADD (address 4); the first MOV executes, then stop.
        assert_eq!(c.step(&[4]), ArmStep::Advanced);
        assert_eq!(c.regs[0], 5);
        assert_eq!(c.step(&[4]), ArmStep::Breakpoint);
        assert_eq!(c.regs[PC], 4, "breakpoint stops before the ADD executes");
        assert_eq!(c.regs[0], 5, "ADD must not have run");
    }

    #[test]
    fn arm_data_processing_flags_and_conditional_execution() {
        let mut c = core();
        // MOV r0, #3; CMP r0, #3; MOVEQ r1, #7 (EQ executes)
        run_arm(
            &mut c,
            &[0xE3A0_0003, 0xE350_0003, 0x03A0_1007],
            3,
        );
        assert!(c.flag_z(), "CMP should set Z");
        assert_eq!(c.regs[1], 7);
    }

    #[test]
    fn arm_conditional_instruction_skipped() {
        let mut c = core();
        // MOV r0, #1; CMP r0, #1 (Z=1); MOVNE r1, #9 must not run
        run_arm(&mut c, &[0xE3A0_0001, 0xE350_0001, 0x13A0_1009], 3);
        assert_eq!(c.regs[1], 0);
    }

    #[test]
    fn arm_branch_and_link() {
        let mut c = core();
        // BL .+4: target = pc_base(8) + 0, LR = 4
        run_arm(&mut c, &[0xEB00_0000], 1);
        assert_eq!(c.regs[LR], 4);
        assert_eq!(c.regs[PC], 8);
    }

    #[test]
    fn arm_branch_backward() {
        let mut c = core();
        // B at 0x100 back to 0: offset = 0 - (0x100 + 8) = -0x108, imm24 = -0x42
        c.write_u32(0x100, 0xEAFF_FFBE);
        c.regs[PC] = 0x100;
        c.set_thumb(false);
        c.arm_state = true;
        let r = c.step(&[]);
        assert_eq!(r, ArmStep::Advanced);
        assert_eq!(c.regs[PC], 0);
    }

    #[test]
    fn arm_load_store_word_and_byte() {
        let mut c = core();
        c.regs[1] = 0x2000_0100;
        c.regs[2] = 0xDEAD_BEEF;
        // STR r2, [r1, #4] (0xE5812004); LDR r3, [r1, #4] (0xE5913004)
        run_arm(&mut c, &[0xE581_2004, 0xE591_3004], 2);
        assert_eq!(c.regs[3], 0xDEAD_BEEF);
        // MOV r4, #0xAB; STRB r4, [r1, #8] (0xE5C14008); LDRB r5, [r1, #8]
        run_arm(&mut c, &[0xE3A0_40AB, 0xE5C1_4008, 0xE5D1_5008], 3);
        assert_eq!(c.regs[5], 0xAB);
    }

    #[test]
    fn arm_halfword_transfer() {
        let mut c = core();
        c.regs[1] = 0x2000_0200;
        c.regs[2] = 0x1234_5678;
        // STRH r2, [r1, #2] (0xE1C120B2); LDRH r3, [r1, #2] (0xE1D130B2)
        run_arm(&mut c, &[0xE1C1_20B2, 0xE1D1_30B2], 2);
        assert_eq!(c.regs[3], 0x5678);
    }

    #[test]
    fn arm_block_transfer() {
        let mut c = core();
        c.regs[0] = 0x2000_0300;
        c.regs[1] = 0x1111_1111;
        c.regs[2] = 0x2222_2222;
        // STMIA r0, {r1,r2} (0xE8800006); LDMIA r0, {r3,r4} (0xE8900018)
        run_arm(&mut c, &[0xE880_0006, 0xE890_0018], 2);
        assert_eq!(c.regs[3], 0x1111_1111);
        assert_eq!(c.regs[4], 0x2222_2222);
    }

    #[test]
    fn arm_bx_switches_to_thumb() {
        let mut c = core();
        c.regs[0] = 0x0000_0021;
        // BX r0 (0xE12FFF10)
        run_arm(&mut c, &[0xE12F_FF10], 1);
        assert_eq!(c.regs[PC], 0x20);
        assert!(c.thumb());
        assert!(!c.arm_state);
    }

    #[test]
    fn arm_multiply_accumulate() {
        let mut c = core();
        c.regs[1] = 6;
        c.regs[2] = 7;
        c.regs[3] = 100;
        // MLA r0, r1, r2, r3 (0xE0203291)
        run_arm(&mut c, &[0xE020_3291], 1);
        assert_eq!(c.regs[0], 142);
    }

    #[test]
    fn arm_movw_movt_build_constant() {
        let mut c = core();
        // MOVW r0, #0x5678 (0xE3050678); MOVT r0, #0x1234 (0xE3410234)
        run_arm(&mut c, &[0xE305_0678, 0xE341_0234], 2);
        assert_eq!(c.regs[0], 0x1234_5678);
    }

    #[test]
    fn arm_reset_vector_selects_arm_state() {
        let mut c = core();
        c.write_u32(0, 0x2000_0000);
        c.write_u32(4, 0x0000_0100); // bit 0 clear => ARM state
        c.reset_from_vector_table(0, 0);
        assert_eq!(c.regs[PC], 0x0100);
        assert!(!c.thumb());
        assert!(c.arm_state);
    }

    #[test]
    fn arm7_code_vector_resets_to_arm_state_entry() {
        let mut c = core();
        // ARM7TDMI vector table: word0 is a B instruction (0xEA000000),
        // word1 is the undefined-instruction vector (also code).
        c.write_u32(0, 0xEA00_0000);
        c.write_u32(4, 0xEA00_0004);
        // ELF entry 0x0000_0100 (bit 0 clear => ARM state).
        c.reset_from_vector_table(0, 0x0000_0100);
        assert_eq!(c.regs[PC], 0x0100);
        assert!(c.arm_state);
        assert!(!c.thumb());
        assert_eq!(c.regs[SP], 0x2000_0000);
    }

    #[test]
    fn cortex_m_data_vector_resets_sp_and_pc() {
        let mut c = core();
        c.write_u32(0, 0x2000_0000); // initial SP
        c.write_u32(4, 0x0800_0009); // reset PC | 1 (Thumb)
        c.reset_from_vector_table(0, 0x0800_0008);
        assert_eq!(c.regs[SP], 0x2000_0000);
        assert_eq!(c.regs[PC], 0x0800_0008);
        assert!(c.thumb());
        assert!(!c.arm_state);
    }
}
