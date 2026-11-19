//! Firmware loading and simulator construction helpers.

use std::path::Path;

use anyhow::{Context, Result};

use ns_sim::{Arch, Simulator, StepResult};

/// The supported firmware formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirmwareFormat {
    Elf,
    Hex,
    Bin,
}

impl FirmwareFormat {
    pub fn label(self) -> &'static str {
        match self {
            FirmwareFormat::Elf => "elf",
            FirmwareFormat::Hex => "hex",
            FirmwareFormat::Bin => "bin",
        }
    }
}

/// Detect the firmware format from a path (extension first, then content).
pub fn detect_format(path: &Path) -> FirmwareFormat {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "elf" | "axf" | "out" => FirmwareFormat::Elf,
        "hex" | "ihex" | "ihx" => FirmwareFormat::Hex,
        "bin" => FirmwareFormat::Bin,
        _ => {
            // Content sniffing: ELF magic or Intel-HEX record.
            if let Ok(bytes) = std::fs::read(path) {
                if bytes.starts_with(b"\x7fELF") {
                    return FirmwareFormat::Elf;
                }
                if bytes.starts_with(b":") {
                    return FirmwareFormat::Hex;
                }
            }
            FirmwareFormat::Bin
        }
    }
}

/// Load a firmware file into the simulator, handling ELF / Intel HEX / BIN.
///
/// `override_base` is only used for raw BIN (and Intel HEX, where a non-zero
/// value overrides the decoded base); ELF derives its own entry/segments.
pub fn load_firmware_into(
    sim: &mut Simulator,
    path: &Path,
    override_base: u32,
) -> Result<()> {
    let format = detect_format(path);
    let bytes = std::fs::read(path)
        .with_context(|| format!("读取固件 {} 失败", path.display()))?;

    match format {
        FirmwareFormat::Elf => {
            sim.load_elf(&bytes)
                .with_context(|| format!("加载 ELF {} 失败", path.display()))?;
        }
        FirmwareFormat::Hex => {
            let text = std::str::from_utf8(&bytes)
                .map_err(|_| anyhow::anyhow!("Intel HEX 不是有效 UTF-8: {}", path.display()))?;
            let decoded = ns_flash_hex::decode(text)
                .with_context(|| format!("解析 Intel HEX {} 失败", path.display()))?;
            let base = if override_base != 0 { override_base } else { decoded.base as u32 };
            load_by_arch(sim, base, &decoded.bytes)?;
        }
        FirmwareFormat::Bin => {
            load_by_arch(sim, override_base, &bytes)?;
        }
    }
    Ok(())
}

/// Architecture-specific raw image loading. Mirrors the IDE's `start_simulation`.
pub fn load_by_arch(sim: &mut Simulator, base: u32, image: &[u8]) -> Result<()> {
    match sim.arch {
        Arch::Mcs51 => sim.load_mcs51(base as u16, image)?,
        Arch::Mcs251 => sim.load_x51(base, image)?,
        Arch::C166 => sim.load_c166(base as u16, image)?,
        Arch::Stm8 => sim.load_stm8(base, image)?,
        Arch::Avr => sim.load_avr(base, image)?,
        Arch::Msp430 => sim.load_msp430(base, image)?,
        Arch::V850 => sim.load_v850(base, image)?,
        Arch::Rx => sim.load_rx(base, image)?,
        Arch::Rl78 => sim.load_rl78(base, image)?,
        Arch::RiscV | Arch::Arm | Arch::Xtensa => sim.load_image(base, image),
    }
    Ok(())
}

/// The default memory-view base address for an architecture (matches the IDE).
pub fn default_mem_base(arch: Arch) -> u32 {
    match arch {
        Arch::RiscV => 0x1000_0000,
        Arch::Arm => 0x2000_0000,
        Arch::Mcs51 => 0,
        Arch::Mcs251 => 0,
        Arch::C166 => 0,
        Arch::Xtensa => 0x3FF0_0000,
        Arch::Avr => 0x0100,
        Arch::Msp430 => 0x0200,
        Arch::Stm8 => 0x0000,
        Arch::Rx => 0x0010_0000,
        Arch::V850 => 0x1F_0000,
        Arch::Rl78 => 0xFFE00,
    }
}

/// Parse a numeric value that may be hex (`0x…`) or decimal.
pub fn parse_num(text: &str) -> Result<u32> {
    let text = text.trim();
    if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16)
            .map_err(|e| anyhow::anyhow!("无效十六进制数 '{text}': {e}"))
    } else if let Some(bin) = text.strip_prefix("0b").or_else(|| text.strip_prefix("0B")) {
        u32::from_str_radix(bin, 2)
            .map_err(|e| anyhow::anyhow!("无效二进制数 '{text}': {e}"))
    } else {
        text.parse::<u32>()
            .map_err(|e| anyhow::anyhow!("无效数值 '{text}': {e}"))
    }
}

/// Encode `bytes` starting at `addr` as Intel HEX records (loadable by
/// `mem load` / the firmware loader). Addresses above 0xFFFF are split into
/// 64 KiB banks using extended-linear-address (type 04) records.
pub fn encode_intel_hex(addr: u32, bytes: &[u8]) -> String {
    fn record(data: &[u8], address: u16, record_type: u8) -> String {
        let mut out = String::from(":");
        out.push_str(&format!("{:02X}{:04X}{:02X}", data.len(), address, record_type));
        let mut sum = (data.len() as u8)
            .wrapping_add((address >> 8) as u8)
            .wrapping_add((address & 0xFF) as u8)
            .wrapping_add(record_type);
        for byte in data {
            out.push_str(&format!("{byte:02X}"));
            sum = sum.wrapping_add(*byte);
        }
        out.push_str(&format!("{:02X}", 0u8.wrapping_sub(sum)));
        out.push('\n');
        out
    }

    let mut out = String::new();
    let mut index = 0usize;
    let mut current_bank = u32::MAX;
    while index < bytes.len() {
        let current = addr.wrapping_add(index as u32);
        let bank = current >> 16;
        if bank != current_bank {
            out.push_str(&record(&[(bank >> 8) as u8, bank as u8], 0, 0x04));
            current_bank = bank;
        }
        let low = (current & 0xFFFF) as u16;
        let chunk_len = (16usize)
            .min(bytes.len() - index)
            .min((0x10000 - low as usize) as usize);
        out.push_str(&record(&bytes[index..index + chunk_len], low, 0x00));
        index += chunk_len;
    }
    out.push_str(&record(&[], 0, 0x01));
    out
}

/// Decode Intel HEX using an embedded copy of the parser from ns-flash.
///
/// We intentionally avoid a direct dependency on ns-flash (which pulls in
/// ns-project and GPUI-free but heavier deps); the parser is self-contained
/// and verified against the same test vectors.
mod ns_flash_hex {
    use anyhow::{Result, bail};

    /// Decoded Intel HEX: contiguous bytes + base address.
    pub struct Decoded {
        pub bytes: Vec<u8>,
        pub base: u64,
    }

    /// Decode Intel HEX records. Supports 00 (data), 01 (EOF), 02 (extended
    /// segment address), 04 (extended linear address).
    pub fn decode(text: &str) -> Result<Decoded> {
        let mut segments: Vec<(u64, Vec<u8>)> = Vec::new();
        let mut base: u64 = 0;
        let mut saw_eof = false;

        for (index, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() {
                continue;
            }
            if !line.starts_with(':') {
                bail!("第 {} 行不是合法 Intel HEX 记录: {}", index + 1, line);
            }
            let record = parse_record(line)
                .map_err(|e| anyhow::anyhow!("解析 Intel HEX 第 {} 行失败: {e}", index + 1))?;
            match record.record_type {
                0x00 => {
                    let addr = base + record.address as u64;
                    if let Some((_, seg)) = segments.iter_mut().find(|(a, _)| *a == addr) {
                        seg.extend_from_slice(&record.data);
                    } else {
                        segments.push((addr, record.data));
                    }
                }
                0x01 => saw_eof = true,
                0x02 => {
                    if record.data.len() != 2 {
                        bail!("扩展段地址记录长度应为 2");
                    }
                    base = u16::from_be_bytes([record.data[0], record.data[1]]) as u64 * 16;
                }
                0x04 => {
                    if record.data.len() != 2 {
                        bail!("扩展线性地址记录长度应为 2");
                    }
                    base = u16::from_be_bytes([record.data[0], record.data[1]]) as u64 * 65536;
                }
                _ => {}
            }
            if saw_eof {
                break;
            }
        }
        if !saw_eof {
            bail!("Intel HEX 文件缺少 EOF 记录");
        }
        segments.sort_by_key(|(a, _)| *a);
        let base_addr = segments.first().map(|(a, _)| *a).unwrap_or(0);
        let mut bytes = Vec::new();
        for (addr, data) in segments {
            let offset = (addr - base_addr) as usize;
            if offset > bytes.len() {
                bytes.resize(offset, 0);
            }
            bytes.extend_from_slice(&data);
        }
        Ok(Decoded { bytes, base: base_addr })
    }

    struct Record {
        address: u16,
        record_type: u8,
        data: Vec<u8>,
    }

    fn parse_record(line: &str) -> Result<Record> {
        let hex = line.strip_prefix(':').unwrap_or(line);
        if hex.len() < 10 || hex.len() % 2 != 0 {
            bail!("记录长度无效");
        }
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|_| anyhow::anyhow!("非法十六进制字符"))?;
        let byte_count = bytes[0] as usize;
        if bytes.len() != byte_count + 5 {
            bail!("记录长度与字节数不符");
        }
        let address = u16::from_be_bytes([bytes[1], bytes[2]]);
        let record_type = bytes[3];
        let data = bytes[4..4 + byte_count].to_vec();
        let checksum = bytes[4 + byte_count];
        let sum: u8 = bytes[..4 + byte_count].iter().fold(0, |a, b| a.wrapping_add(*b));
        if sum.wrapping_add(checksum) != 0 {
            bail!("校验和错误");
        }
        Ok(Record { address, record_type, data })
    }
}

/// Read a single byte from the simulator's data space.
///
/// The `ns-sim::Simulator::read_u8` accessor only routes ARM/Xtensa/AVR/
/// MSP430/V850/STM8/RX/RL78 cores; the 8051/80251/C166 cores keep their own
/// Harvard memory model. This helper addresses the data space for those cores
/// so the CLI's `mem read` sees the same memory the CPU writes.
pub fn read_mem_byte(sim: &Simulator, addr: u32) -> u8 {
    if let Some(core) = &sim.mcs51 {
        // Internal RAM 0x00-0xFF, external data 0x100+.
        return match addr {
            0x00..=0xFF => core.idram[addr as usize],
            0x100..=0xFFFF => {
                let i = (addr - 0x100) as usize;
                if i < core.xram.len() { core.xram[i] } else { 0 }
            }
            _ => 0,
        };
    }
    if let Some(core) = &sim.x51 {
        return match addr {
            0x00..=0xFF => core.idram[addr as usize],
            0x100..=0xFFFF => {
                let i = (addr - 0x100) as usize;
                if i < core.xram.len() { core.xram[i] } else { 0 }
            }
            _ => 0,
        };
    }
    if let Some(core) = &sim.c166 {
        return match addr {
            0x0000..=0xFFFF => {
                let i = addr as usize;
                if i < core.data.len() { core.data[i] } else { 0 }
            }
            _ => 0,
        };
    }
    sim.read_u8(addr)
}

/// Read a little-endian 32-bit word through the data-space byte accessor
/// (so Harvard/AVR cores see the same memory `mem write`/`read` uses).
pub fn read_mem_u32(sim: &Simulator, addr: u32) -> u32 {
    (0..4).fold(0u32, |acc, i| {
        acc | ((read_mem_byte(sim, addr.wrapping_add(i)) as u32) << (8 * i))
    })
}

/// Read a byte from the architecture's **code** space (for disassembly /
/// instruction fetching). Cores with a separate program ROM (MCS-51 / 80251 /
/// C166 / AVR) do not mirror their code into `sim.memory`, so the ROM is read
/// directly; every other core routes through the generic byte accessor.
pub fn read_code_byte(sim: &Simulator, addr: u32) -> u8 {
    if let Some(core) = &sim.mcs51 {
        let i = addr as usize;
        return if i < core.rom.len() { core.rom[i] } else { 0 };
    }
    if let Some(core) = &sim.x51 {
        let i = addr as usize;
        return if i < core.rom.len() { core.rom[i] } else { 0 };
    }
    if let Some(core) = &sim.c166 {
        let i = addr as usize;
        return if i < core.rom.len() { core.rom[i] } else { 0 };
    }
    if let Some(core) = &sim.avr {
        let i = addr as usize;
        return if i < core.rom.len() { core.rom[i] } else { 0 };
    }
    sim.read_u8(addr)
}

/// Write a single byte into the simulator's data space (mirrors read_mem_byte).
pub fn write_mem_byte(sim: &mut Simulator, addr: u32, value: u8) {
    if let Some(core) = &mut sim.mcs51 {
        match addr {
            0x00..=0xFF => core.idram[addr as usize] = value,
            0x100..=0xFFFF => {
                let i = (addr - 0x100) as usize;
                if i < core.xram.len() { core.xram[i] = value; }
            }
            _ => {}
        }
    } else if let Some(core) = &mut sim.x51 {
        match addr {
            0x00..=0xFF => core.idram[addr as usize] = value,
            0x100..=0xFFFF => {
                let i = (addr - 0x100) as usize;
                if i < core.xram.len() { core.xram[i] = value; }
            }
            _ => {}
        }
    } else if let Some(core) = &mut sim.c166 {
        match addr {
            0x0000..=0xFFFF => {
                let i = addr as usize;
                if i < core.data.len() { core.data[i] = value; }
            }
            _ => {}
        }
    } else if let Some(core) = &mut sim.avr {
        // AVR data space: R0-R31 (0x00-0x1F), I/O (0x20-0x5F), SRAM (0x60+).
        // Note the generic `load_image` path targets program memory, so the
        // data space must be written explicitly to mirror `read_mem_byte`.
        match addr {
            0x00..=0x1F => core.regs[addr as usize] = value,
            0x20..=0x5F => core.io[(addr - 0x20) as usize] = value,
            0x60.. => {
                let i = (addr - 0x60) as usize;
                if i < core.sram.len() { core.sram[i] = value; }
            }
        }
    } else {
        // The generic path routes through write_bytes which handles most cores.
        sim.write_bytes(addr, &[value]);
    }
    // Record the explicit data-space write so it survives session reloads
    // (cores with their own memory model do not mirror into `memory`).
    if let Some(entry) = sim.memory_overlay.iter_mut().find(|(a, _)| *a == addr) {
        entry.1 = value;
    } else {
        sim.memory_overlay.push((addr, value));
    }
}

/// One executed instruction, captured during an analysed run.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TraceEntry {
    pub pc: u32,
    pub text: String,
    pub bytes: Vec<u8>,
    pub source: String,
    pub cycle: u64,
    /// Register file snapshot after the instruction (only populated when the
    /// caller requests register tracking).
    #[serde(default)]
    pub regs: std::collections::BTreeMap<String, u32>,
}

/// Result of an analysed run: optional per-instruction trace, an execution
/// coverage map (PC -> hit count) and a mnemonic histogram.
#[derive(Debug, serde::Serialize)]
pub struct RunAnalysis {
    pub trace: Vec<TraceEntry>,
    /// PC address -> number of times executed.
    pub coverage: std::collections::BTreeMap<u32, u64>,
    /// Disassembly text -> number of times executed.
    pub histogram: std::collections::BTreeMap<String, u64>,
    pub result: String,
    pub steps: u64,
    pub cycle_count: u64,
}

fn step_result_label(result: StepResult) -> &'static str {
    match result {
        StepResult::Advanced => "advanced",
        StepResult::Breakpoint => "breakpoint",
        StepResult::Halted => "halted",
    }
}

fn source_for(sim: &Simulator, address: u32) -> String {
    sim.line_for_address(address)
        .and_then(|(file, line)| sim.source_files.get(file).map(|s| (s.clone(), line)))
        .map(|(file, line)| format!("{file}:{line}"))
        .unwrap_or_default()
}

/// Step the simulator while collecting per-instruction trace, coverage and a
/// mnemonic histogram. Stops at a breakpoint, halt, or after `max_steps`.
pub fn analyze_run(
    sim: &mut Simulator,
    max_steps: u64,
    collect_trace: bool,
    collect_regs: bool,
) -> RunAnalysis {
    let mut analysis = RunAnalysis {
        trace: Vec::new(),
        coverage: std::collections::BTreeMap::new(),
        histogram: std::collections::BTreeMap::new(),
        result: "advanced".to_string(),
        steps: 0,
        cycle_count: 0,
    };
    let mut count = 0u64;
    while count < max_steps {
        let pc_before = sim.program_counter();
        let result = sim.step();
        let read = |addr: u32| -> u8 { read_code_byte(sim, addr) };
        let text = ns_sim::disassemble_one_state(sim.arch, pc_before, &read, sim.arm_state());
        let width = ns_sim::instruction_size(sim.arch, pc_before, &read, sim.arm_state());
        let bytes: Vec<u8> = (0..width)
            .map(|i| read_code_byte(sim, pc_before.wrapping_add(i as u32)))
            .collect();
        *analysis.coverage.entry(pc_before).or_insert(0) += 1;
        *analysis.histogram.entry(text.clone()).or_insert(0) += 1;
        if collect_trace {
            analysis.trace.push(TraceEntry {
                pc: pc_before,
                text,
                bytes,
                source: source_for(sim, pc_before),
                cycle: sim.cycle_count,
                regs: std::collections::BTreeMap::new(),
            });
            if collect_regs {
                let mut regs = std::collections::BTreeMap::new();
                let count = ns_sim::reg_count_for(sim.arch);
                for i in 0..count {
                    regs.insert(ns_sim::reg_label_for(sim.arch, i), sim.read_reg(i));
                }
                if let Some(last) = analysis.trace.last_mut() {
                    last.regs = regs;
                }
            }
        }
        count += 1;
        if result != StepResult::Advanced {
            analysis.result = step_result_label(result).to_string();
            break;
        }
    }
    analysis.steps = count;
    analysis.cycle_count = sim.cycle_count;
    analysis
}

#[cfg(test)]
mod tests {
    use super::encode_intel_hex;

    #[test]
    fn intel_hex_encodes_and_decodes_roundtrip() {
        // The encoder's own output must decode back to the same bytes via the
        // embedded parser, including addresses above the 64 KiB bank.
        let bytes: Vec<u8> = (0u8..32).collect();
        for (addr, data) in [(0u32, &bytes[..]), (0x1000_0000u32, &bytes[..])] {
            let text = encode_intel_hex(addr, data);
            let decoded = super::ns_flash_hex::decode(&text).unwrap();
            assert_eq!(decoded.base as u32, addr, "base mismatch for {addr:#x}");
            assert_eq!(&decoded.bytes, data, "payload mismatch for {addr:#x}");
        }
    }
}
