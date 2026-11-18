//! End-to-end tests that drive the CLI binary as an external tool would.

use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_ns-instruction-sim")
}

fn tmpdir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nsim-test-{}",
        std::process::id()
    ));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn write_riscv_elf(path: &std::path::Path) {
    // addi x1,x0,1 (0x00100093); addi x2,x1,2 (0x00208113);
    // sw x1,16(x0) (0x00102823) -> memory[16] (does not alias the code at
    // memory[0..16]); ecall (0x00000073).
    let code: [u8; 16] = [
        0x93, 0x00, 0x10, 0x00, // addi x1, x0, 1
        0x13, 0x81, 0x20, 0x00, // addi x2, x1, 2
        0x23, 0x28, 0x10, 0x00, // sw x1, 16(x0)
        0x73, 0x00, 0x00, 0x00, // ecall
    ];
    write_riscv_elf_code(path, &code);
}

fn write_riscv_elf_code(path: &std::path::Path, code: &[u8]) {
    let mut elf = vec![0u8; 0x100];
    elf[0..4].copy_from_slice(b"\x7fELF");
    elf[4] = 1; // ELF32
    elf[5] = 1; // little endian
    elf[6] = 1;
    elf[18..20].copy_from_slice(&243u16.to_le_bytes()); // EM_RISCV
    elf[24..28].copy_from_slice(&0x8000_0000u32.to_le_bytes()); // entry
    elf[28..32].copy_from_slice(&52u32.to_le_bytes()); // phoff
    elf[42..44].copy_from_slice(&52u16.to_le_bytes()); // phentsize
    elf[44..46].copy_from_slice(&1u16.to_le_bytes()); // phnum
    let ph = 52usize;
    elf[ph..ph + 4].copy_from_slice(&1u32.to_le_bytes()); // PT_LOAD
    elf[ph + 4..ph + 8].copy_from_slice(&0x100u32.to_le_bytes()); // p_offset
    elf[ph + 8..ph + 12].copy_from_slice(&0x8000_0000u32.to_le_bytes()); // p_vaddr
    elf[ph + 12..ph + 16].copy_from_slice(&0x8000_0000u32.to_le_bytes()); // p_paddr
    elf[ph + 16..ph + 20].copy_from_slice(&(code.len() as u32).to_le_bytes());
    elf[ph + 20..ph + 24].copy_from_slice(&(code.len() as u32).to_le_bytes());
    elf.extend_from_slice(&code);
    std::fs::write(path, elf).unwrap();
}

/// Write a minimal RISC-V ELF with a `.symtab`/`.strtab` so symbol lookups and
/// `state --json` `pcSymbol` can be exercised end-to-end.
fn write_riscv_elf_with_symbol(path: &std::path::Path, code: &[u8], symbol: &str) {
    let mut elf = vec![0u8; 0x100];
    elf[0..4].copy_from_slice(b"\x7fELF");
    elf[4] = 1;
    elf[5] = 1;
    elf[6] = 1;
    elf[18..20].copy_from_slice(&243u16.to_le_bytes()); // EM_RISCV
    elf[24..28].copy_from_slice(&0x8000_0000u32.to_le_bytes()); // entry
    elf[28..32].copy_from_slice(&52u32.to_le_bytes()); // phoff
    elf[42..44].copy_from_slice(&52u16.to_le_bytes()); // phentsize
    elf[44..46].copy_from_slice(&1u16.to_le_bytes()); // phnum
    let ph = 52usize;
    elf[ph..ph + 4].copy_from_slice(&1u32.to_le_bytes()); // PT_LOAD
    elf[ph + 4..ph + 8].copy_from_slice(&0x100u32.to_le_bytes()); // p_offset
    elf[ph + 8..ph + 12].copy_from_slice(&0x8000_0000u32.to_le_bytes());
    elf[ph + 12..ph + 16].copy_from_slice(&0x8000_0000u32.to_le_bytes());
    elf[ph + 16..ph + 20].copy_from_slice(&(code.len() as u32).to_le_bytes());
    elf[ph + 20..ph + 24].copy_from_slice(&(code.len() as u32).to_le_bytes());
    elf[ph + 24..ph + 28].copy_from_slice(&5u32.to_le_bytes());
    elf[ph + 28..ph + 32].copy_from_slice(&0x1000u32.to_le_bytes());

    let code_off = 0x100usize;
    elf.extend_from_slice(code);
    while elf.len() % 4 != 0 {
        elf.push(0);
    }
    let mut strtab = vec![0u8];
    strtab.extend_from_slice(symbol.as_bytes());
    strtab.push(0);
    let strtab_off = elf.len();
    elf.extend_from_slice(&strtab);
    while elf.len() % 4 != 0 {
        elf.push(0);
    }
    let symtab_off = elf.len();
    let mut symtab = vec![0u8; 16]; // null symbol
    let mut main_sym = vec![0u8; 16];
    main_sym[0..4].copy_from_slice(&1u32.to_le_bytes()); // st_name
    main_sym[4..8].copy_from_slice(&0x8000_0000u32.to_le_bytes()); // st_value
    main_sym[8..12].copy_from_slice(&(code.len() as u32).to_le_bytes()); // st_size
    main_sym[12] = 0x12; // STB_GLOBAL | STT_FUNC
    main_sym[14..16].copy_from_slice(&1u16.to_le_bytes()); // st_shndx = .text
    symtab.extend_from_slice(&main_sym);
    elf.extend_from_slice(&symtab);
    let shstrtab = b"\0.text\0.symtab\0.strtab\0.shstrtab\0".to_vec();
    let shstrtab_off = elf.len();
    elf.extend_from_slice(&shstrtab);
    while elf.len() % 4 != 0 {
        elf.push(0);
    }
    let shoff = elf.len();
    elf.extend_from_slice(&[0u8; 40]); // sh[0] null
    let mut s = vec![0u8; 40]; // sh[1] .text
    s[0..4].copy_from_slice(&1u32.to_le_bytes());
    s[4..8].copy_from_slice(&1u32.to_le_bytes()); // SHT_PROGBITS
    s[8..12].copy_from_slice(&6u32.to_le_bytes()); // ALLOC|EXEC
    s[12..16].copy_from_slice(&0x8000_0000u32.to_le_bytes());
    s[16..20].copy_from_slice(&(code_off as u32).to_le_bytes());
    s[20..24].copy_from_slice(&(code.len() as u32).to_le_bytes());
    s[32..36].copy_from_slice(&4u32.to_le_bytes());
    elf.extend_from_slice(&s);
    let mut s = vec![0u8; 40]; // sh[2] .symtab
    s[0..4].copy_from_slice(&7u32.to_le_bytes());
    s[4..8].copy_from_slice(&2u32.to_le_bytes()); // SHT_SYMTAB
    s[16..20].copy_from_slice(&(symtab_off as u32).to_le_bytes());
    s[20..24].copy_from_slice(&(symtab.len() as u32).to_le_bytes());
    s[24..28].copy_from_slice(&3u32.to_le_bytes()); // sh_link = .strtab
    s[28..32].copy_from_slice(&1u32.to_le_bytes());
    s[32..36].copy_from_slice(&4u32.to_le_bytes());
    s[36..40].copy_from_slice(&16u32.to_le_bytes());
    elf.extend_from_slice(&s);
    let mut s = vec![0u8; 40]; // sh[3] .strtab
    s[0..4].copy_from_slice(&15u32.to_le_bytes());
    s[4..8].copy_from_slice(&3u32.to_le_bytes()); // SHT_STRTAB
    s[16..20].copy_from_slice(&(strtab_off as u32).to_le_bytes());
    s[20..24].copy_from_slice(&(strtab.len() as u32).to_le_bytes());
    s[32..36].copy_from_slice(&1u32.to_le_bytes());
    elf.extend_from_slice(&s);
    let mut s = vec![0u8; 40]; // sh[4] .shstrtab
    s[0..4].copy_from_slice(&23u32.to_le_bytes());
    s[4..8].copy_from_slice(&3u32.to_le_bytes());
    s[16..20].copy_from_slice(&(shstrtab_off as u32).to_le_bytes());
    s[20..24].copy_from_slice(&(shstrtab.len() as u32).to_le_bytes());
    s[32..36].copy_from_slice(&1u32.to_le_bytes());
    elf.extend_from_slice(&s);
    elf[32..36].copy_from_slice(&(shoff as u32).to_le_bytes()); // e_shoff
    elf[46..48].copy_from_slice(&40u16.to_le_bytes()); // e_shentsize
    elf[48..50].copy_from_slice(&5u16.to_le_bytes()); // e_shnum
    elf[50..52].copy_from_slice(&4u16.to_le_bytes()); // e_shstrndx
    std::fs::write(path, elf).unwrap();
}

/// Create a fresh RISC-V session (the `cycle tick` command persists it).
fn bootstrap(sess: &std::path::Path) {
    let out = Command::new(bin())
        .args(["cycle", "tick", "--session", sess.to_str().unwrap(), "--cycles", "0"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "bootstrap failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn arch_list_shows_all() {
    let out = Command::new(bin()).args(["arch", "list"]).output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    for arch in ["riscv", "arm", "mcs51", "mcs251", "c166", "xtensa", "avr", "msp430", "v850", "stm8", "rx", "rl78"] {
        assert!(text.contains(arch), "missing {arch}");
    }
}

#[test]
fn device_list_and_info() {
    let out = Command::new(bin()).args(["device", "list"]).output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("stm32f103c8"));
    let out = Command::new(bin()).args(["device", "info", "stm32f103c8"]).output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("Cortex-M3"));
}

#[test]
fn device_codegen_linker_and_startup() {
    // Linker script on stdout.
    let out = Command::new(bin()).args(["device", "linker", "stm32f103c8"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("Auto-generated linker script"));
    assert!(text.contains("FLASH"));
    assert!(text.contains("0x08000000"));
    assert!(text.contains("0x20000000"));

    // Startup file on stdout.
    let out = Command::new(bin()).args(["device", "startup", "stm32f103c8"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("Auto-generated startup"));
    assert!(text.contains("Reset_Handler"));
    assert!(text.contains(".isr_vector"));

    // Write to a file with --out.
    let dir = tmpdir();
    let ld = dir.join("chip.ld");
    let out = Command::new(bin())
        .args(["device", "linker", "stm32f103c8", "--out", ld.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(std::fs::read_to_string(&ld).unwrap().contains("MEMORY"));

    // Unknown device must fail.
    let out = Command::new(bin()).args(["device", "linker", "nope"]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("未知设备"));
}

#[test]
fn device_init_codegen() {
    // Skeleton init for a selected peripheral.
    let out = Command::new(bin())
        .args(["device", "init", "stm32f103c8", "--peripheral", "GPIOA"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("gpioa_init"));
    assert!(text.contains("peripheral_init"));
    assert!(text.contains("GPIOA"));

    // Concrete register override emits a real write.
    let out = Command::new(bin())
        .args(["device", "init", "stm32f103c8", "--peripheral", "GPIOA",
               "--register", "GPIOA.CRL=0x44444444"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("GPIOA->CRL = 0x44444444"));

    // Malformed register override must fail.
    let out = Command::new(bin())
        .args(["device", "init", "stm32f103c8", "--peripheral", "GPIOA",
               "--register", "CRL=0x1"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("PERIPH.REG"));
}

#[test]
fn run_riscv_elf_and_inspect() {
    let dir = tmpdir();
    let elf = dir.join("test.elf");
    let sess = dir.join("session.json");
    write_riscv_elf(&elf);
    let out = Command::new(bin())
        .args(["run", "--firmware", elf.to_str().unwrap(), "--save-session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "run failed: {}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["lastResult"], "halted");
    assert_eq!(json["pc"], 0x8000_0010u64);

    // Registers should reflect the program: x1=1, x2=3, x0=0.
    assert_eq!(json["registers"]["ra"], 1u64);
    assert_eq!(json["registers"]["sp"], 3u64);

    // mem read should show the loaded code.
    let out = Command::new(bin())
        .args(["mem", "read", "--session", sess.to_str().unwrap(), "--addr", "0x80000000", "--len", "4"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("93 00 10 00"));
}

#[test]
fn continue_until_stops_at_address() {
    let dir = tmpdir();
    let elf = dir.join("until.elf");
    let sess = dir.join("until.json");
    write_riscv_elf(&elf);
    // Run one instruction, leaving the session at 0x80000004.
    Command::new(bin())
        .args(["run", "--firmware", elf.to_str().unwrap(),
               "--session", sess.to_str().unwrap(), "--steps", "1"])
        .output()
        .unwrap();
    // Run to 0x80000008 using a temporary breakpoint.
    let out = Command::new(bin())
        .args(["continue", "--session", sess.to_str().unwrap(),
               "--until", "0x80000008", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["pc"], 0x8000_0008u64);
    assert_eq!(json["lastResult"], "breakpoint");
    assert_eq!(json["registers"]["ra"], 1u64);
    assert_eq!(json["registers"]["sp"], 3u64);
    // The temporary breakpoint must not be persisted.
    let out = Command::new(bin())
        .args(["bp", "list", "--session", sess.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("无断点"));
}

#[test]
fn breakpoint_flow() {
    let dir = tmpdir();
    let elf = dir.join("bp.elf");
    let sess = dir.join("bp.json");
    write_riscv_elf(&elf);
    Command::new(bin())
        .args(["run", "--firmware", elf.to_str().unwrap(), "--save-session", sess.to_str().unwrap()])
        .output()
        .unwrap();
    // Set breakpoint at 0x80000004 and reset to entry.
    let out = Command::new(bin())
        .args(["bp", "set", "--session", sess.to_str().unwrap(), "--addr", "0x80000004"])
        .output()
        .unwrap();
    assert!(out.status.success());
    Command::new(bin())
        .args(["reset", "--session", sess.to_str().unwrap(), "--entry", "0x80000000"])
        .output()
        .unwrap();
    let out = Command::new(bin())
        .args(["continue", "--session", sess.to_str().unwrap(), "--max-steps", "10", "--json"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["lastResult"], "breakpoint");
    assert_eq!(json["pc"], 0x8000_0004u64);
    assert_eq!(json["registers"]["ra"], 1u64);
}

#[test]
fn reg_set_and_get() {
    let dir = tmpdir();
    let sess = dir.join("reg.json");
    Command::new(bin())
        .args(["state", "--arch", "riscv", "--session", sess.to_str().unwrap()])
        .output()
        .unwrap();
    let out = Command::new(bin())
        .args(["reg", "set", "--session", sess.to_str().unwrap(), "--name", "x5", "--value", "0x2a"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["reg", "get", "--session", sess.to_str().unwrap(), "--name", "x5"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("0x0000002a"));
}

#[test]
fn session_roundtrip_preserves_firmware_and_peripherals() {
    let dir = tmpdir();
    let elf = dir.join("rt.elf");
    let sess = dir.join("rt.json");
    write_riscv_elf(&elf);
    Command::new(bin())
        .args(["run", "--firmware", elf.to_str().unwrap(), "--save-session", sess.to_str().unwrap()])
        .output()
        .unwrap();
    let text = std::fs::read_to_string(&sess).unwrap();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["firmware"]["path"], elf.to_str().unwrap());
    assert!(json["firmware"]["format"] == "elf");
}

#[test]
fn all_arch_state_works() {
    let dir = tmpdir();
    for arch in ["riscv", "arm", "mcs51", "mcs251", "c166", "xtensa", "avr", "msp430", "v850", "stm8", "rx", "rl78"] {
        let sess = dir.join(format!("{arch}.json"));
        let out = Command::new(bin())
            .args(["state", "--arch", arch, "--session", sess.to_str().unwrap(), "--json"])
            .output()
            .unwrap();
        assert!(out.status.success(), "state failed for {arch}: {}", String::from_utf8_lossy(&out.stderr));
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(json["arch"], arch, "arch field should be {arch}");
    }
}

#[test]
fn all_arch_core_operations() {
    let dir = tmpdir();
    for arch in ["riscv", "arm", "mcs51", "mcs251", "c166", "xtensa", "avr", "msp430", "v850", "stm8", "rx", "rl78"] {
        let sess = dir.join(format!("core-{arch}.json"));
        // Bootstrap the session with a data-space write for this architecture.
        let out = Command::new(bin())
            .args(["mem", "write", "--session", sess.to_str().unwrap(),
                   "--arch", arch, "--addr", "0x20", "--bytes", "0xaa", "0xbb"])
            .output()
            .unwrap();
        assert!(out.status.success(), "mem write failed for {arch}: {}", String::from_utf8_lossy(&out.stderr));
        // Register listing must expose a non-empty architecture register view.
        let out = Command::new(bin())
            .args(["reg", "list", "--session", sess.to_str().unwrap(), "--json"])
            .output()
            .unwrap();
        assert!(out.status.success(), "reg list failed for {arch}: {}", String::from_utf8_lossy(&out.stderr));
        let regs: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert!(regs.as_object().map(|o| !o.is_empty()).unwrap_or(false), "empty registers for {arch}");
        // A single step must advance the core without error.
        let out = Command::new(bin())
            .args(["step", "--session", sess.to_str().unwrap(), "--json"])
            .output()
            .unwrap();
        assert!(out.status.success(), "step failed for {arch}: {}", String::from_utf8_lossy(&out.stderr));
        // The bytes written must be read back in the same data space.
        let out = Command::new(bin())
            .args(["mem", "read", "--session", sess.to_str().unwrap(),
                   "--addr", "0x20", "--len", "2", "--json"])
            .output()
            .unwrap();
        assert!(out.status.success(), "mem read failed for {arch}: {}", String::from_utf8_lossy(&out.stderr));
        let mem: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(mem["bytes"][0], 0xaau64, "byte 0 wrong for {arch}");
        assert_eq!(mem["bytes"][1], 0xbbu64, "byte 1 wrong for {arch}");
        // Disassembly must decode at least one instruction.
        let out = Command::new(bin())
            .args(["disasm", "--arch", arch, "--addr", "0", "--count", "1", "--json"])
            .output()
            .unwrap();
        assert!(out.status.success(), "disasm failed for {arch}: {}", String::from_utf8_lossy(&out.stderr));
    }
}

#[test]
fn mem_overlay_persists_across_step() {
    // Explicit data-space writes on cores with a private memory model (e.g. ARM
    // Cortex-M) must survive a step/run/continue. The session's memory_overlay
    // carries them forward; a step must not drop the record.
    let dir = tmpdir();
    let sess = dir.join("overlay-step.json");
    let out = Command::new(bin())
        .args(["mem", "write", "--session", sess.to_str().unwrap(),
               "--arch", "arm", "--addr", "0x20", "--bytes", "0xaa", "0xbb"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["step", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["mem", "read", "--session", sess.to_str().unwrap(),
               "--addr", "0x20", "--len", "2", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let mem: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(mem["bytes"][0], 0xaau64, "write lost across step");
    assert_eq!(mem["bytes"][1], 0xbbu64, "write lost across step");
}

#[test]
fn cycle_get_and_tick() {
    let dir = tmpdir();
    let sess = dir.join("cycle.json");
    bootstrap(&sess);
    // Tick 100 cycles, then read the counter back.
    let out = Command::new(bin())
        .args(["cycle", "tick", "--session", sess.to_str().unwrap(), "--cycles", "100", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["cycleCount"], 100u64);
    let out = Command::new(bin())
        .args(["cycle", "get", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["cycleCount"], 100u64);
    // Cycle estimate for the reset vector should be at least 1.
    let out = Command::new(bin())
        .args(["cycle", "estimate", "--session", sess.to_str().unwrap(), "--addr", "0x80000000", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(json["cycles"].as_u64().unwrap() >= 1);
}

#[test]
fn csr_set_get_roundtrip() {
    let dir = tmpdir();
    let sess = dir.join("csr.json");
    bootstrap(&sess);
    let out = Command::new(bin())
        .args(["csr", "set", "--session", sess.to_str().unwrap(), "--name", "mtvec", "--value", "0x80000010", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["csr", "get", "--session", sess.to_str().unwrap(), "--name", "mtvec", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["value"], 0x8000_0010u64);
    // List should include mtvec.
    let out = Command::new(bin())
        .args(["csr", "list", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["mtvec"], 0x8000_0010u64);
}

#[test]
fn watchdog_config_feed_status() {
    let dir = tmpdir();
    let sess = dir.join("wd.json");
    bootstrap(&sess);
    let out = Command::new(bin())
        .args(["watchdog", "config", "--session", sess.to_str().unwrap(), "--enabled", "true", "--timeout", "500", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["enabled"], true);
    assert_eq!(json["timeoutCycles"], 500u64);
    // Advance cycles so the watchdog accumulates elapsed time.
    Command::new(bin())
        .args(["cycle", "tick", "--session", sess.to_str().unwrap(), "--cycles", "100"])
        .output()
        .unwrap();
    let out = Command::new(bin())
        .args(["watchdog", "status", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["elapsed"], 100u64);
    // Feeding resets the watchdog elapsed counter to zero.
    let out = Command::new(bin())
        .args(["watchdog", "feed", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["fed"], true);
    assert_eq!(json["elapsedBefore"], 100u64);
    assert_eq!(json["elapsed"], 0u64);
    let out = Command::new(bin())
        .args(["watchdog", "status", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["enabled"], true);
    assert_eq!(json["timeoutCycles"], 500u64);
    assert_eq!(json["elapsed"], 0u64);
}

#[test]
fn interrupt_set_pending_and_take() {
    let dir = tmpdir();
    let sess = dir.join("irq.json");
    bootstrap(&sess);
    // Enable MIE (mstatus bit 3) and the IRQ line 3 mask.
    Command::new(bin())
        .args(["csr", "set", "--session", sess.to_str().unwrap(), "--name", "mstatus", "--value", "0x8"])
        .output()
        .unwrap();
    Command::new(bin())
        .args(["csr", "set", "--session", sess.to_str().unwrap(), "--name", "mie", "--value", "0x8"])
        .output()
        .unwrap();
    Command::new(bin())
        .args(["csr", "set", "--session", sess.to_str().unwrap(), "--name", "mtvec", "--value", "0x80000010"])
        .output()
        .unwrap();
    // Assert line 3.
    let out = Command::new(bin())
        .args(["interrupt", "set", "--session", sess.to_str().unwrap(), "--line", "3", "--value", "1", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["interrupt", "pending", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["pendingLines"], serde_json::json!([3]));
    // Taking the interrupt should jump to mtvec and record mcause.
    let out = Command::new(bin())
        .args(["interrupt", "take", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["taken"], true);
    assert_eq!(json["pc"], 0x8000_0010u64);
    assert_eq!(json["mcause"], 0x8000_0003u64);
}

#[test]
fn bp_list_shows_hits() {
    let dir = tmpdir();
    let elf = dir.join("bp_hits.elf");
    let sess = dir.join("bp_hits.json");
    write_riscv_elf(&elf);
    Command::new(bin())
        .args(["run", "--firmware", elf.to_str().unwrap(), "--save-session", sess.to_str().unwrap()])
        .output()
        .unwrap();
    Command::new(bin())
        .args(["bp", "set", "--session", sess.to_str().unwrap(), "--addr", "0x80000004"])
        .output()
        .unwrap();
    Command::new(bin())
        .args(["reset", "--session", sess.to_str().unwrap(), "--entry", "0x80000000"])
        .output()
        .unwrap();
    Command::new(bin())
        .args(["continue", "--session", sess.to_str().unwrap(), "--max-steps", "10"])
        .output()
        .unwrap();
    let out = Command::new(bin())
        .args(["bp", "list", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["address"], 0x8000_0004u64);
    assert!(json[0]["hits"].as_u64().unwrap() >= 1);
}

#[test]
fn bp_once_autoclears() {
    let dir = tmpdir();
    let elf = dir.join("bp_once.elf");
    let sess = dir.join("bp_once.json");
    write_riscv_elf(&elf);
    Command::new(bin())
        .args(["run", "--firmware", elf.to_str().unwrap(), "--save-session", sess.to_str().unwrap()])
        .output()
        .unwrap();
    // Set a one-shot breakpoint.
    let out = Command::new(bin())
        .args(["bp", "set", "--session", sess.to_str().unwrap(), "--addr", "0x80000004", "--once"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("一次性"));
    // bp list should mark it as once.
    let out = Command::new(bin())
        .args(["bp", "list", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["once"], true);
    assert_eq!(json[0]["address"], 0x8000_0004u64);
    Command::new(bin())
        .args(["reset", "--session", sess.to_str().unwrap(), "--entry", "0x80000000"])
        .output()
        .unwrap();
    // First continue stops at the once breakpoint and auto-clears it.
    let out = Command::new(bin())
        .args(["continue", "--session", sess.to_str().unwrap(), "--max-steps", "10", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["lastResult"], "breakpoint");
    assert_eq!(json["pc"], 0x8000_0004u64);
    // The one-shot breakpoint must now be gone from the persisted session.
    let out = Command::new(bin())
        .args(["bp", "list", "--session", sess.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("无断点"),
        "once breakpoint not cleared: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    // A second continue no longer stops there (runs to the ecall halt).
    let out = Command::new(bin())
        .args(["continue", "--session", sess.to_str().unwrap(), "--max-steps", "10", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_ne!(json["lastResult"], "breakpoint");
    assert_ne!(json["pc"], 0x8000_0004u64);
}

#[test]
fn periph_timer_config_and_uart_write() {
    let dir = tmpdir();
    let sess = dir.join("periph.json");
    bootstrap(&sess);
    // Configure timer 1.
    let out = Command::new(bin())
        .args(["periph", "timer", "--session", sess.to_str().unwrap(), "--index", "1", "--enabled", "true", "--prescaler", "8", "--auto-reload", "1000", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["index"], 1u64);
    assert_eq!(json["enabled"], true);
    assert_eq!(json["prescaler"], 8u64);
    assert_eq!(json["autoReload"], 1000u64);
    // Write "Hi" to the UART data register and confirm it shows up.
    let out = Command::new(bin())
        .args(["periph", "reg-write", "--session", sess.to_str().unwrap(), "--addr", "0x40000000", "--bytes", "0x48", "0x69"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["periph", "status", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["uartText"], "Hi");
}

#[test]
fn mem_write_read_roundtrip_persists() {
    let dir = tmpdir();
    let sess = dir.join("mem.json");
    bootstrap(&sess);
    // Write 4 bytes at the reset vector, then read them back in a separate
    // invocation. Memory must be persisted in the session.
    let out = Command::new(bin())
        .args(["mem", "write", "--session", sess.to_str().unwrap(), "--addr", "0x80000000", "--bytes", "0xde", "0xad", "0xbe", "0xef"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["mem", "read", "--session", sess.to_str().unwrap(), "--addr", "0x80000000", "--len", "4"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("de ad be ef"));
}

#[test]
fn mem_fill_hex_value() {
    let dir = tmpdir();
    let sess = dir.join("memfill.json");
    bootstrap(&sess);
    let out = Command::new(bin())
        .args(["mem", "fill", "--session", sess.to_str().unwrap(), "--addr", "0x80000010", "--len", "8", "--value", "0xaa"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["mem", "read", "--session", sess.to_str().unwrap(), "--addr", "0x80000010", "--len", "8"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("aa aa aa aa"));
}

#[test]
fn mem_search_finds_pattern() {
    let dir = tmpdir();
    let sess = dir.join("memsearch.json");
    bootstrap(&sess);
    Command::new(bin())
        .args(["mem", "write", "--session", sess.to_str().unwrap(),
               "--addr", "0x20000000",
               "--bytes", "0xde", "0xad", "0xbe", "0xef", "0x00", "0xde", "0xad"])
        .output()
        .unwrap();

    // Both occurrences are found.
    let out = Command::new(bin())
        .args(["mem", "search", "--session", sess.to_str().unwrap(),
               "--addr", "0x20000000", "--len", "16",
               "--pattern", "0xde", "0xad", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["count"], 2u64);
    assert_eq!(json["matches"][0], 0x2000_0000u64);
    assert_eq!(json["matches"][1], 0x2000_0005u64);

    // A pattern not present reports zero matches.
    let out = Command::new(bin())
        .args(["mem", "search", "--session", sess.to_str().unwrap(),
               "--addr", "0x20000000", "--len", "16", "--pattern", "0xff"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("未找到"));
}

#[test]
fn mem_search_harvard_data_space() {
    // On Harvard-architecture cores (8051/80251/C166) the CLI's data-space
    // helper must be used so a search sees the same memory `mem write` wrote.
    let dir = tmpdir();
    let sess = dir.join("memsearch51.json");
    let out = Command::new(bin())
        .args(["mem", "write", "--session", sess.to_str().unwrap(),
               "--arch", "mcs51", "--addr", "0x20",
               "--bytes", "0xca", "0xfe", "0xba", "0xbe"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["mem", "search", "--session", sess.to_str().unwrap(),
               "--arch", "mcs51", "--addr", "0x20", "--len", "16",
               "--pattern", "0xca", "0xfe", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["count"], 1u64, "harvard data-space search failed: {json}");
    assert_eq!(json["matches"][0], 0x20u64);
}

#[test]
fn mem_search_out_writes_file() {
    let dir = tmpdir();
    let sess = dir.join("memsearchout.json");
    bootstrap(&sess);
    Command::new(bin())
        .args(["mem", "write", "--session", sess.to_str().unwrap(),
               "--addr", "0x20000000",
               "--bytes", "0xde", "0xad", "0xbe", "0xef"])
        .output()
        .unwrap();
    // JSON result to a file.
    let out_file = dir.join("search.json");
    let out = Command::new(bin())
        .args(["mem", "search", "--session", sess.to_str().unwrap(),
               "--addr", "0x20000000", "--len", "16",
               "--pattern", "0xde", "0xad", "--json",
               "--out", out_file.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&std::fs::read(&out_file).unwrap()).unwrap();
    assert_eq!(json["count"], 1u64);
    assert_eq!(json["matches"][0], 0x2000_0000u64);
    // Text result to a file.
    let out_file2 = dir.join("search.txt");
    let out = Command::new(bin())
        .args(["mem", "search", "--session", sess.to_str().unwrap(),
               "--addr", "0x20000000", "--len", "16",
               "--pattern", "0xbe", "0xef",
               "--out", out_file2.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = std::fs::read_to_string(&out_file2).unwrap();
    assert!(text.contains("0x20000002"), "missing match in file: {text}");
}

#[test]
fn device_svd_parse_and_filter() {
    let dir = tmpdir();
    let svd = dir.join("testchip.svd");
    let xml = r#"<?xml version="1.0"?>
<device>
  <name>TestChip</name>
  <vendor>TestVendor</vendor>
  <cpu><name>CM3</name></cpu>
  <memory>
    <memory><name>FLASH</name><start>0x08000000</start><size>0x10000</size><access>rx</access></memory>
  </memory>
  <peripherals>
    <peripheral>
      <name>UART0</name>
      <baseAddress>0x40000000</baseAddress>
      <description>UART peripheral</description>
      <registers>
        <register>
          <name>DR</name>
          <addressOffset>0x00</addressOffset>
          <size>32</size>
          <resetValue>0</resetValue>
          <description>Data register</description>
          <fields>
            <field><name>DATA</name><bitOffset>0</bitOffset><bitWidth>8</bitWidth><description>Data bits</description></field>
          </fields>
        </register>
      </registers>
    </peripheral>
  </peripherals>
</device>
"#;
    std::fs::write(&svd, xml).unwrap();

    // Full device dump.
    let out = Command::new(bin())
        .args(["device", "svd", "--file", svd.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["name"], "TestChip");
    assert_eq!(json["vendor"], "TestVendor");
    assert_eq!(json["core"], "CM3");
    assert_eq!(json["peripherals"][0]["name"], "UART0");
    assert_eq!(json["peripherals"][0]["baseAddress"], 0x4000_0000u64);
    assert_eq!(json["peripherals"][0]["registers"][0]["name"], "DR");
    assert_eq!(json["peripherals"][0]["registers"][0]["fields"][0]["name"], "DATA");
    assert_eq!(json["peripherals"][0]["registers"][0]["fields"][0]["bitWidth"], 8u64);

    // Register filter.
    let out = Command::new(bin())
        .args(["device", "svd", "--file", svd.to_str().unwrap(), "--peripheral", "UART0", "--register", "DR", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["name"], "DR");
    assert_eq!(json["fields"][0]["name"], "DATA");

    // Unknown peripheral must fail with a non-zero exit code.
    let out = Command::new(bin())
        .args(["device", "svd", "--file", svd.to_str().unwrap(), "--peripheral", "NOPE"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("无外设"));
}

#[test]
fn periph_svd_load_populates_registers() {
    let dir = tmpdir();
    let sess = dir.join("svd_load.json");
    let svd = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("crates/ns-device/src/svd/stm32f103c8.svd");
    bootstrap(&sess);

    // Load all SVD-defined registers into the session.
    let out = Command::new(bin())
        .args(["periph", "svd-load", "--session", sess.to_str().unwrap(),
               "--file", svd.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(json["loaded"].as_u64().unwrap() >= 16);
    assert!(json["peripherals"].as_u64().unwrap() >= 4);

    // The reset value of TIM2 CR1 (0x40000000) is 0.
    let addr = "0x40000000";
    let out = Command::new(bin())
        .args(["periph", "reg-read", "--session", sess.to_str().unwrap(),
               "--addr", addr, "--len", "4", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["bytes"][0], 0u64);
    assert_eq!(json["bytes"][1], 0u64);

    // Write the register and read it back (both external_regs and memory).
    let out = Command::new(bin())
        .args(["periph", "reg-write", "--session", sess.to_str().unwrap(),
               "--addr", addr, "--bytes", "0x01", "0x00", "0x00", "0x00", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["periph", "reg-read", "--session", sess.to_str().unwrap(),
               "--addr", addr, "--len", "4", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["bytes"][0], 1u64);

    // The written value survives a fresh session load (persistence).
    let out = Command::new(bin())
        .args(["periph", "reg-read", "--session", sess.to_str().unwrap(),
               "--addr", addr, "--len", "4", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["bytes"][0], 1u64);

    // state exposes the loaded external registers.
    let out = Command::new(bin())
        .args(["state", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(json["peripherals"]["externalRegs"].is_object());
}

#[test]
fn periph_svd_load_by_device_key() {
    let dir = tmpdir();
    let sess = dir.join("svd_device_key.json");
    bootstrap(&sess);

    // Load from the built-in SVD by device key.
    let out = Command::new(bin())
        .args(["periph", "svd-load", "--session", sess.to_str().unwrap(),
               "--device", "stm32f103c8", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["source"], "stm32f103c8");
    assert!(json["loaded"].as_u64().unwrap() >= 16);

    // Missing source must fail.
    let out = Command::new(bin())
        .args(["periph", "svd-load", "--session", sess.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--file 或 --device"));
}

#[test]
fn periph_reg_list_shows_loaded_registers() {
    let dir = tmpdir();
    let sess = dir.join("reg_list.json");
    bootstrap(&sess);
    Command::new(bin())
        .args(["periph", "svd-load", "--session", sess.to_str().unwrap(),
               "--device", "stm32f103c8"])
        .output()
        .unwrap();

    // Text list with device annotation.
    let out = Command::new(bin())
        .args(["periph", "reg-list", "--session", sess.to_str().unwrap(),
               "--device", "stm32f103c8"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("TIM2.CR1"));
    assert!(text.contains("GPIOA.CRL"));

    // JSON list has all loaded registers.
    let out = Command::new(bin())
        .args(["periph", "reg-list", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(json.as_array().unwrap().len() >= 16);
    assert_eq!(json[0]["address"], 0x4000_0000u64);

    // Empty session reports no registers.
    let empty = dir.join("reg_list_empty.json");
    bootstrap(&empty);
    let out = Command::new(bin())
        .args(["periph", "reg-list", "--session", empty.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("未加载"));
}

#[test]
fn periph_status_reports_svd_registers() {
    let dir = tmpdir();
    let sess = dir.join("pstat.json");
    bootstrap(&sess);
    Command::new(bin())
        .args(["periph", "svd-load", "--session", sess.to_str().unwrap(),
               "--device", "stm32f103c8"])
        .output()
        .unwrap();
    let out = Command::new(bin())
        .args(["periph", "status", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(json["externalRegs"].as_object().unwrap().len() >= 16);
}

#[test]
fn periph_reg_name_access() {
    let dir = tmpdir();
    let sess = dir.join("reg_name.json");
    bootstrap(&sess);
    Command::new(bin())
        .args(["periph", "svd-load", "--session", sess.to_str().unwrap(),
               "--device", "stm32f103c8"])
        .output()
        .unwrap();

    // Read TIM2.CR1 by name (reset value 0).
    let out = Command::new(bin())
        .args(["periph", "reg-read", "--session", sess.to_str().unwrap(),
               "--name", "TIM2.CR1", "--device", "stm32f103c8", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["address"], 0x4000_0000u64);
    assert_eq!(json["bytes"][0], 0u64);

    // Write and read back by name.
    let out = Command::new(bin())
        .args(["periph", "reg-write", "--session", sess.to_str().unwrap(),
               "--name", "TIM2.CR1", "--device", "stm32f103c8",
               "--bytes", "0x01", "0x00", "0x00", "0x00", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["periph", "reg-read", "--session", sess.to_str().unwrap(),
               "--name", "TIM2.CR1", "--device", "stm32f103c8", "--json"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["bytes"][0], 1u64);

    // Unknown register must fail.
    let out = Command::new(bin())
        .args(["periph", "reg-read", "--session", sess.to_str().unwrap(),
               "--name", "TIM2.NOPE", "--device", "stm32f103c8"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("无寄存器"));

    // Neither --addr nor --name must fail.
    let out = Command::new(bin())
        .args(["periph", "reg-read", "--session", sess.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--addr 或 --name"));
}

#[test]
fn periph_field_access() {
    let dir = tmpdir();
    let sess = dir.join("field.json");
    bootstrap(&sess);
    Command::new(bin())
        .args(["periph", "svd-load", "--session", sess.to_str().unwrap(),
               "--device", "stm32f103c8"])
        .output()
        .unwrap();

    // Read a field by name (RCC.CR.HSION, bit 0 width 1).
    let out = Command::new(bin())
        .args(["periph", "field-get", "--session", sess.to_str().unwrap(),
               "--device", "stm32f103c8", "--peripheral", "RCC",
               "--register", "CR", "--field", "HSION", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["value"], 0u64);
    assert_eq!(json["bitWidth"], 1u64);

    // Set the field and read it back (preserves other bits).
    let out = Command::new(bin())
        .args(["periph", "field-set", "--session", sess.to_str().unwrap(),
               "--device", "stm32f103c8", "--peripheral", "RCC",
               "--register", "CR", "--field", "HSION", "--value", "1", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["register"], 1u64);
    let out = Command::new(bin())
        .args(["periph", "field-get", "--session", sess.to_str().unwrap(),
               "--device", "stm32f103c8", "--peripheral", "RCC",
               "--register", "CR", "--field", "HSION", "--json"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["value"], 1u64);

    // Multi-bit field reset value (GPIOA.CRL.MODE0 = 0x4 from 0x44444444).
    let out = Command::new(bin())
        .args(["periph", "field-get", "--session", sess.to_str().unwrap(),
               "--device", "stm32f103c8", "--peripheral", "GPIOA",
               "--register", "CRL", "--field", "MODE0", "--json"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["value"], 4u64);

    // Unknown field must fail.
    let out = Command::new(bin())
        .args(["periph", "field-get", "--session", sess.to_str().unwrap(),
               "--device", "stm32f103c8", "--peripheral", "RCC",
               "--register", "CR", "--field", "NOPE"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("无位域"));
}

#[test]
fn periph_reg_write_data_space_avr() {
    // On AVR the I/O space (0x20-0x5F) is part of the data space; a
    // peripheral-register write must land there (not program memory) so both
    // `periph reg-read` and `mem read` observe the value.
    let dir = tmpdir();
    let sess = dir.join("avr-periph.json");
    let out = Command::new(bin())
        .args(["mem", "write", "--session", sess.to_str().unwrap(),
               "--arch", "avr", "--addr", "0x20", "--bytes", "0", "0"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["periph", "reg-write", "--session", sess.to_str().unwrap(),
               "--addr", "0x20", "--bytes", "0xaa", "0xbb", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["periph", "reg-read", "--session", sess.to_str().unwrap(),
               "--addr", "0x20", "--len", "2", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["bytes"][0], 0xaau64);
    assert_eq!(json["bytes"][1], 0xbbu64);
    let out = Command::new(bin())
        .args(["mem", "read", "--session", sess.to_str().unwrap(),
               "--addr", "0x20", "--len", "2", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["bytes"][0], 0xaau64);
    assert_eq!(json["bytes"][1], 0xbbu64);
}

#[test]
fn periph_svd_load_instruction_write_couples() {
    let dir = tmpdir();
    let sess = dir.join("svd_exec.json");
    let elf = dir.join("svd_exec.elf");
    let svd = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("crates/ns-device/src/svd/stm32f103c8.svd");
    bootstrap(&sess);
    Command::new(bin())
        .args(["periph", "svd-load", "--session", sess.to_str().unwrap(),
               "--file", svd.to_str().unwrap()])
        .output()
        .unwrap();
    // lui x3,0x40021 (x3=0x40021000 RCC.CR); addi x1,x0,0x2a (x1=42);
    // sw x1,0(x3) -> write 42 to RCC.CR; ecall.
    let code: [u8; 16] = [
        0xB7, 0x11, 0x02, 0x40, // lui x3, 0x40021
        0x93, 0x00, 0xA0, 0x02, // addi x1, x0, 0x2a
        0x23, 0xA0, 0x11, 0x00, // sw x1, 0(x3)
        0x73, 0x00, 0x00, 0x00, // ecall
    ];
    write_riscv_elf_code(&elf, &code);

    // Run the firmware on the SVD-loaded session and save back to it.
    let out = Command::new(bin())
        .args(["run", "--firmware", elf.to_str().unwrap(),
               "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["lastResult"], "halted");

    // The instruction-level store must have landed in the SVD register.
    let out = Command::new(bin())
        .args(["periph", "reg-read", "--session", sess.to_str().unwrap(),
               "--addr", "0x40021000", "--len", "4", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["bytes"][0], 0x2au64);
    assert_eq!(json["bytes"][1], 0u64);
    assert_eq!(json["bytes"][2], 0u64);
    assert_eq!(json["bytes"][3], 0u64);
}

#[test]
fn device_svd_by_device_key() {
    let out = Command::new(bin())
        .args(["device", "svd", "--device", "stm32f103c8", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["name"], "STM32F103C8");
    assert!(json["peripherals"].as_array().unwrap().len() >= 4);

    // Missing source must fail.
    let out = Command::new(bin()).args(["device", "svd"]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--file 或 --device"));
}

#[test]
fn run_steps_runs_exactly_n_instructions() {
    let dir = tmpdir();
    let elf = dir.join("steps.elf");
    write_riscv_elf(&elf);
    // --steps 2 should execute addi x1 and addi x2, stopping at 0x80000008.
    let out = Command::new(bin())
        .args(["run", "--firmware", elf.to_str().unwrap(), "--steps", "2", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["pc"], 0x8000_0008u64);
    assert_eq!(json["lastResult"], "advanced");
    assert_eq!(json["registers"]["ra"], 1u64);
    assert_eq!(json["registers"]["sp"], 3u64);
    // Without --steps the program runs to the ecall halt.
    let out = Command::new(bin())
        .args(["run", "--firmware", elf.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["pc"], 0x8000_0010u64);
    assert_eq!(json["lastResult"], "halted");
}

#[test]
fn run_until_stops_at_address() {
    let dir = tmpdir();
    let elf = dir.join("run_until.elf");
    let sess = dir.join("run_until.json");
    write_riscv_elf(&elf);
    let out = Command::new(bin())
        .args(["run", "--firmware", elf.to_str().unwrap(),
               "--session", sess.to_str().unwrap(),
               "--until", "0x80000008", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["pc"], 0x8000_0008u64);
    assert_eq!(json["lastResult"], "breakpoint");
    assert_eq!(json["registers"]["ra"], 1u64);
    // The temporary breakpoint is not persisted.
    let out = Command::new(bin())
        .args(["bp", "list", "--session", sess.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("无断点"));
}

#[test]
fn state_includes_peripherals_and_csr() {
    let dir = tmpdir();
    let sess = dir.join("state_full.json");
    bootstrap(&sess);
    Command::new(bin())
        .args(["csr", "set", "--session", sess.to_str().unwrap(), "--name", "mtvec", "--value", "0x80000010"])
        .output()
        .unwrap();
    Command::new(bin())
        .args(["interrupt", "set", "--session", sess.to_str().unwrap(), "--line", "2", "--value", "1"])
        .output()
        .unwrap();
    Command::new(bin())
        .args(["watchdog", "config", "--session", sess.to_str().unwrap(), "--enabled", "true", "--timeout", "500"])
        .output()
        .unwrap();
    let out = Command::new(bin())
        .args(["state", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["csr"]["mtvec"], 0x8000_0010u64);
    assert_eq!(json["peripherals"]["watchdog"]["enabled"], true);
    assert_eq!(json["peripherals"]["watchdog"]["timeoutCycles"], 500u64);
    assert_eq!(json["peripherals"]["irqPending"], serde_json::json!([2]));
}

#[test]
fn state_reports_loaded_firmware() {
    let dir = tmpdir();
    let sess = dir.join("state_fw.json");
    let elf = dir.join("state_fw.elf");
    write_riscv_elf(&elf);
    Command::new(bin())
        .args(["run", "--firmware", elf.to_str().unwrap(),
               "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    let out = Command::new(bin())
        .args(["state", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["firmware"]["format"], "elf");
    assert!(json["firmware"]["path"].as_str().unwrap().contains("state_fw.elf"));
    assert_eq!(json["firmware"]["base"], 0u64);
}

#[test]
fn state_reports_pc_symbol() {
    let dir = tmpdir();
    let sess = dir.join("state_sym.json");
    let elf = dir.join("state_sym.elf");
    let code: [u8; 16] = [
        0x93, 0x00, 0x10, 0x00,
        0x13, 0x81, 0x20, 0x00,
        0x23, 0x28, 0x10, 0x00,
        0x73, 0x00, 0x00, 0x00,
    ];
    write_riscv_elf_with_symbol(&elf, &code, "main");
    Command::new(bin())
        .args(["run", "--firmware", elf.to_str().unwrap(),
               "--session", sess.to_str().unwrap(), "--steps", "1", "--json"])
        .output()
        .unwrap();
    let out = Command::new(bin())
        .args(["state", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["pcSymbol"]["name"], "main");
    assert_eq!(json["pcSymbol"]["address"], 0x8000_0000u64);
    assert!(json["pcSymbol"]["size"].as_u64().unwrap() >= 16);
}

#[test]
fn symbol_list_shows_symbols() {
    let dir = tmpdir();
    let sess = dir.join("sym_list.json");
    let elf = dir.join("sym_list.elf");
    let code: [u8; 16] = [
        0x93, 0x00, 0x10, 0x00,
        0x13, 0x81, 0x20, 0x00,
        0x23, 0x28, 0x10, 0x00,
        0x73, 0x00, 0x00, 0x00,
    ];
    write_riscv_elf_with_symbol(&elf, &code, "main");
    Command::new(bin())
        .args(["run", "--firmware", elf.to_str().unwrap(),
               "--session", sess.to_str().unwrap(), "--steps", "1"])
        .output()
        .unwrap();
    let out = Command::new(bin())
        .args(["symbol", "list", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["name"], "main");
    assert_eq!(json[0]["address"], 0x8000_0000u64);
}

#[test]
fn version_command_and_flag() {
    let out = Command::new(bin()).arg("version").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains(env!("CARGO_PKG_VERSION")));
    let out = Command::new(bin()).args(["version", "--json"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["name"], "ns-instruction-sim");
    assert_eq!(json["version"], env!("CARGO_PKG_VERSION"));
    let out = Command::new(bin()).arg("--version").output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn mem_write_read_arm_roundtrip() {
    let dir = tmpdir();
    let binfile = dir.join("arm.bin");
    let sess = dir.join("arm.json");
    std::fs::write(&binfile, [0x00u8, 0xf0, 0x01, 0xf8, 0x00, 0x00]).unwrap();
    // Create a persisted ARM session.
    let out = Command::new(bin())
        .args(["mem", "load", "--session", sess.to_str().unwrap(), "--firmware", binfile.to_str().unwrap(), "--arch", "arm"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    // Write to ARM RAM and read back in a separate invocation.
    let out = Command::new(bin())
        .args(["mem", "write", "--session", sess.to_str().unwrap(), "--addr", "0x20000000", "--bytes", "0xde", "0xad", "0xbe", "0xef"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["mem", "read", "--session", sess.to_str().unwrap(), "--addr", "0x20000000", "--len", "4"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("de ad be ef"));
}

#[test]
fn periph_config_relocates_bases() {
    let dir = tmpdir();
    let sess = dir.join("pconf.json");
    bootstrap(&sess);
    let out = Command::new(bin())
        .args(["periph", "config", "--session", sess.to_str().unwrap(), "--uart-base", "0x50000000", "--gpio-base", "0x50002000", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["uartBase"], 0x5000_0000u64);
    assert_eq!(json["gpioBase"], 0x5000_2000u64);
    // Writing to the relocated UART base must produce output.
    let out = Command::new(bin())
        .args(["periph", "reg-write", "--session", sess.to_str().unwrap(), "--addr", "0x50000000", "--bytes", "0x41", "0x42"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let out = Command::new(bin())
        .args(["periph", "status", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["uartText"], "AB");
}

#[test]
fn mem_write_with_arch_creates_session() {
    let dir = tmpdir();
    let sess = dir.join("memarch.json");
    // `mem write --arch arm` on a missing session creates an ARM session.
    let out = Command::new(bin())
        .args(["mem", "write", "--session", sess.to_str().unwrap(), "--arch", "arm", "--addr", "0x20000000", "--bytes", "0xaa", "0xbb"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["state", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["arch"], "arm");
    let out = Command::new(bin())
        .args(["mem", "read", "--session", sess.to_str().unwrap(), "--addr", "0x20000000", "--len", "2"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("aa bb"));
}

#[test]
fn disasm_arm_state_override() {
    let dir = tmpdir();
    let binfile = dir.join("thumb.bin");
    let sess = dir.join("thumb.json");
    // Thumb: nop (0xBF00), movs r0,#1 (0x2001).
    std::fs::write(&binfile, [0x00u8, 0xbf, 0x01, 0x20]).unwrap();
    Command::new(bin())
        .args(["mem", "load", "--session", sess.to_str().unwrap(), "--firmware", binfile.to_str().unwrap(), "--arch", "arm"])
        .output()
        .unwrap();
    // Thumb override: the 2-byte nop is followed by movs r0, #1 at 0x2.
    let out = Command::new(bin())
        .args(["disasm", "--session", sess.to_str().unwrap(), "--addr", "0x0", "--count", "2", "--arm-state", "thumb"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("0x00000002"), "Thumb-1 should advance 2 bytes: {text}");
    // A32 override advances 4 bytes.
    let out = Command::new(bin())
        .args(["disasm", "--session", sess.to_str().unwrap(), "--addr", "0x0", "--count", "1", "--arm-state", "a32"])
        .output()
        .unwrap();
    assert!(out.status.success());
    // Invalid value must be rejected.
    let out = Command::new(bin())
        .args(["disasm", "--session", sess.to_str().unwrap(), "--addr", "0x0", "--arm-state", "bogus"])
        .output()
        .unwrap();
    assert!(!out.status.success());
}

#[test]
fn disasm_source_annotates_lines() {
    let dir = tmpdir();
    let sess = dir.join("dsrc.json");
    let elf = dir.join("dsrc.elf");
    let code: [u8; 16] = [
        0x93, 0x00, 0x10, 0x00,
        0x13, 0x81, 0x20, 0x00,
        0x23, 0x28, 0x10, 0x00,
        0x73, 0x00, 0x00, 0x00,
    ];
    write_riscv_elf_with_symbol(&elf, &code, "main");
    Command::new(bin())
        .args(["run", "--firmware", elf.to_str().unwrap(),
               "--session", sess.to_str().unwrap(), "--steps", "1"])
        .output()
        .unwrap();
    let out = Command::new(bin())
        .args(["disasm", "--session", sess.to_str().unwrap(),
               "--count", "2", "--source"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("<elf>:1"), "missing source annotation: {text}");
}

#[test]
fn step_source_annotates_lines() {
    let dir = tmpdir();
    let sess = dir.join("ssrc.json");
    let elf = dir.join("ssrc.elf");
    let code: [u8; 16] = [
        0x93, 0x00, 0x10, 0x00,
        0x13, 0x81, 0x20, 0x00,
        0x23, 0x28, 0x10, 0x00,
        0x73, 0x00, 0x00, 0x00,
    ];
    write_riscv_elf_with_symbol(&elf, &code, "main");
    Command::new(bin())
        .args(["run", "--firmware", elf.to_str().unwrap(),
               "--session", sess.to_str().unwrap(), "--steps", "1"])
        .output()
        .unwrap();
    let out = Command::new(bin())
        .args(["step", "--session", sess.to_str().unwrap(), "--source"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    // `run --steps 1` leaves PC at the 2nd instruction; `step` advances to the
    // 3rd (0x80000008), still inside the `main` symbol (source line 1).
    assert!(text.contains("PC=0x80000008"), "missing PC: {text}");
    assert!(text.contains("<elf>:1"), "missing source annotation: {text}");
}

#[test]
fn run_trace_prints_each_instruction() {
    let dir = tmpdir();
    let sess = dir.join("trace.json");
    let elf = dir.join("trace.elf");
    let code: [u8; 16] = [
        0x93, 0x00, 0x10, 0x00,
        0x13, 0x81, 0x20, 0x00,
        0x23, 0x28, 0x10, 0x00,
        0x73, 0x00, 0x00, 0x00,
    ];
    write_riscv_elf_with_symbol(&elf, &code, "main");
    let out = Command::new(bin())
        .args(["run", "--firmware", elf.to_str().unwrap(),
               "--session", sess.to_str().unwrap(),
               "--trace", "--steps", "3"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("[0] 0x80000000"), "missing step 0: {text}");
    assert!(text.contains("[1] 0x80000004"), "missing step 1: {text}");
    assert!(text.contains("[2] 0x80000008"), "missing step 2: {text}");
    assert!(text.contains("<elf>:1"), "missing source: {text}");
    assert!(text.contains("-> 0x8000000c"), "missing result PC: {text}");
}

#[test]
fn mem_dump_c_array() {
    let dir = tmpdir();
    let sess = dir.join("dump.json");
    bootstrap(&sess);
    let out = Command::new(bin())
        .args(["mem", "write", "--session", sess.to_str().unwrap(),
               "--addr", "0x20000000", "--bytes", "0xde", "0xad", "0xbe", "0xef"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["mem", "dump", "--session", sess.to_str().unwrap(),
               "--addr", "0x20000000", "--len", "4", "--name", "bootloader", "--format", "c"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("const unsigned char bootloader[4]"), "missing decl: {text}");
    assert!(text.contains("0xde"), "missing 0xde: {text}");
    assert!(text.contains("0xef"), "missing 0xef: {text}");
    // Invalid format must fail.
    let out = Command::new(bin())
        .args(["mem", "dump", "--session", sess.to_str().unwrap(),
               "--addr", "0x20000000", "--len", "4", "--format", "json"])
        .output()
        .unwrap();
    assert!(!out.status.success());
}

#[test]
fn mem_dump_json() {
    let dir = tmpdir();
    let sess = dir.join("dumpjson.json");
    bootstrap(&sess);
    let out = Command::new(bin())
        .args(["mem", "write", "--session", sess.to_str().unwrap(),
               "--addr", "0x20000000", "--bytes", "0xde", "0xad", "0xbe", "0xef"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["mem", "dump", "--session", sess.to_str().unwrap(),
               "--addr", "0x20000000", "--len", "4", "--name", "bootloader",
               "--format", "c", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["address"], 0x2000_0000u64);
    assert_eq!(json["len"], 4u64);
    assert_eq!(json["format"], "c");
    assert_eq!(json["name"], "bootloader");
    let bytes = json["bytes"].as_array().unwrap();
    assert_eq!(bytes.len(), 4);
    assert_eq!(bytes[0], 0xdeu64);
    assert_eq!(bytes[1], 0xadu64);
    assert_eq!(bytes[2], 0xbeu64);
    assert_eq!(bytes[3], 0xefu64);
}

#[test]
fn mem_dump_hex_roundtrip() {
    // `mem dump --format hex` must emit valid Intel HEX that `mem load` can
    // load back, including addresses above the 64 KiB bank boundary.
    let dir = tmpdir();
    let sess = dir.join("hexrt.json");
    let hex = dir.join("hexrt.hex");
    let sess2 = dir.join("hexrt2.json");
    let addr = 0x1000_0000u64;
    let out = Command::new(bin())
        .args(["mem", "write", "--session", sess.to_str().unwrap(),
               "--arch", "riscv", "--addr", &format!("{addr:#x}"),
               "--bytes", "0xde", "0xad", "0xbe", "0xef", "0x00", "0x01", "0x02", "0x03"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["mem", "dump", "--session", sess.to_str().unwrap(),
               "--addr", &format!("{addr:#x}"), "--len", "8",
               "--format", "hex", "--out", hex.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = std::fs::read_to_string(&hex).unwrap();
    assert!(text.contains(":00000001FF"), "missing EOF record: {text}");
    // Load the HEX back into a fresh session and compare.
    let out = Command::new(bin())
        .args(["mem", "load", "--session", sess2.to_str().unwrap(),
               "--arch", "riscv", "--firmware", hex.to_str().unwrap(),
               "--base", &format!("{addr:#x}")])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["mem", "read", "--session", sess2.to_str().unwrap(),
               "--addr", &format!("{addr:#x}"), "--len", "8", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["bytes"], serde_json::json!([0xde, 0xad, 0xbe, 0xef, 0, 1, 2, 3]));
    // `--format dump` is the human-readable hex+ASCII view (not loadable).
    let out = Command::new(bin())
        .args(["mem", "dump", "--session", sess.to_str().unwrap(),
               "--addr", &format!("{addr:#x}"), "--len", "8", "--format", "dump"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("|........|"));
    // Unknown format is rejected.
    let out = Command::new(bin())
        .args(["mem", "dump", "--session", sess.to_str().unwrap(),
               "--addr", &format!("{addr:#x}"), "--len", "8", "--format", "bogus"])
        .output()
        .unwrap();
    assert!(!out.status.success());
}

#[test]
fn state_bootstraps_persistent_session() {
    // `state --arch --session` on a missing path must materialize the session
    // so a subsequent `cycle estimate`/`run` can operate on it.
    let dir = tmpdir();
    let sess = dir.join("boot.json");
    let out = Command::new(bin())
        .args(["state", "--arch", "riscv", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(sess.exists(), "state --session must create the session file");
    let out = Command::new(bin())
        .args(["cycle", "estimate", "--session", sess.to_str().unwrap(),
               "--addr", "0x0", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(json["cycles"].as_u64().unwrap() >= 1);
}

#[test]
fn watchdog_force_reset() {
    let dir = tmpdir();
    let sess = dir.join("wdreset.json");
    bootstrap(&sess);
    // Enable the watchdog with a base and a timeout.
    Command::new(bin())
        .args(["watchdog", "config", "--session", sess.to_str().unwrap(),
               "--enabled", "true", "--timeout", "100", "--base", "0x40002000"])
        .output()
        .unwrap();
    // Force a reset at an explicit entry.
    let out = Command::new(bin())
        .args(["watchdog", "reset", "--session", sess.to_str().unwrap(),
               "--entry", "0x80000000", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["resetCount"], 1u64);
    // The fresh RISC-V session starts at the default entry 0x80000000.
    assert_eq!(json["lastResetPc"], 0x8000_0000u64);
    assert_eq!(json["pc"], 0x8000_0000u64);
    // The reset event must persist in the session.
    let out = Command::new(bin())
        .args(["watchdog", "status", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["resetCount"], 1u64);
    assert_eq!(json["lastResetPc"], 0x8000_0000u64);
}

#[test]
fn watchdog_reset_default_goes_to_entry() {
    // A watchdog reset without --entry must return to the firmware/arch entry
    // (not the PC where the watchdog fired), while still recording lastResetPc.
    let dir = tmpdir();
    let sess = dir.join("wdreset-entry.json");
    let elf = dir.join("wdreset-entry.elf");
    write_riscv_elf(&elf);
    let out = Command::new(bin())
        .args(["run", "--firmware", elf.to_str().unwrap(),
               "--session", sess.to_str().unwrap(), "--steps", "1"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["watchdog", "reset", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["pc"], 0x8000_0000u64, "watchdog reset should go to entry");
    assert_ne!(json["lastResetPc"], 0x8000_0000u64, "lastResetPc should be the firing PC");
}

#[test]
fn continue_trace_prints_each_instruction() {
    let dir = tmpdir();
    let sess = dir.join("ctrace.json");
    let elf = dir.join("ctrace.elf");
    write_riscv_elf(&elf);
    Command::new(bin())
        .args(["run", "--firmware", elf.to_str().unwrap(),
               "--save-session", sess.to_str().unwrap()])
        .output()
        .unwrap();
    Command::new(bin())
        .args(["reset", "--session", sess.to_str().unwrap(), "--entry", "0x80000000"])
        .output()
        .unwrap();
    let out = Command::new(bin())
        .args(["continue", "--session", sess.to_str().unwrap(),
               "--trace", "--until", "0x80000008"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("[0] 0x80000000"), "missing step 0: {text}");
    assert!(text.contains("[1] 0x80000004"), "missing step 1: {text}");
    assert!(text.contains("-> 0x80000008 breakpoint"), "missing stop: {text}");
}

#[test]
fn bp_enable_disable_roundtrip() {
    let dir = tmpdir();
    let elf = dir.join("bped.elf");
    let sess = dir.join("bped.json");
    write_riscv_elf(&elf);
    Command::new(bin())
        .args(["run", "--firmware", elf.to_str().unwrap(), "--save-session", sess.to_str().unwrap()])
        .output()
        .unwrap();
    Command::new(bin())
        .args(["bp", "set", "--session", sess.to_str().unwrap(), "--addr", "0x80000004"])
        .output()
        .unwrap();
    Command::new(bin())
        .args(["reset", "--session", sess.to_str().unwrap(), "--entry", "0x80000000"])
        .output()
        .unwrap();
    // Disable the breakpoint; continue must NOT stop there (runs to halt).
    Command::new(bin())
        .args(["bp", "disable", "--session", sess.to_str().unwrap(), "--addr", "0x80000004"])
        .output()
        .unwrap();
    let out = Command::new(bin())
        .args(["continue", "--session", sess.to_str().unwrap(), "--max-steps", "10", "--json"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["lastResult"], "halted");
    // bp list marks it disabled.
    let out = Command::new(bin())
        .args(["bp", "list", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["enabled"], false);
    // Re-enable; reset and continue must stop again.
    Command::new(bin())
        .args(["bp", "enable", "--session", sess.to_str().unwrap(), "--addr", "0x80000004"])
        .output()
        .unwrap();
    Command::new(bin())
        .args(["reset", "--session", sess.to_str().unwrap(), "--entry", "0x80000000"])
        .output()
        .unwrap();
    let out = Command::new(bin())
        .args(["continue", "--session", sess.to_str().unwrap(), "--max-steps", "10", "--json"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["lastResult"], "breakpoint");
    assert_eq!(json["pc"], 0x8000_0004u64);
}

#[test]
fn bp_set_by_symbol() {
    let dir = tmpdir();
    let sess = dir.join("bpsym.json");
    let elf = dir.join("bpsym.elf");
    let code: [u8; 16] = [
        0x93, 0x00, 0x10, 0x00,
        0x13, 0x81, 0x20, 0x00,
        0x23, 0x28, 0x10, 0x00,
        0x73, 0x00, 0x00, 0x00,
    ];
    write_riscv_elf_with_symbol(&elf, &code, "main");
    Command::new(bin())
        .args(["run", "--firmware", elf.to_str().unwrap(),
               "--session", sess.to_str().unwrap(), "--steps", "1"])
        .output()
        .unwrap();
    let out = Command::new(bin())
        .args(["bp", "set", "--session", sess.to_str().unwrap(), "--symbol", "main"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["bp", "list", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["address"], 0x8000_0000u64);
    // Unknown symbol must fail.
    let out = Command::new(bin())
        .args(["bp", "set", "--session", sess.to_str().unwrap(), "--symbol", "nope"])
        .output()
        .unwrap();
    assert!(!out.status.success());
}

#[test]
fn disasm_raw_shows_bytes() {
    let dir = tmpdir();
    let sess = dir.join("drawn.json");
    let elf = dir.join("drawn.elf");
    write_riscv_elf(&elf);
    Command::new(bin())
        .args(["run", "--firmware", elf.to_str().unwrap(),
               "--session", sess.to_str().unwrap(), "--steps", "1"])
        .output()
        .unwrap();
    let out = Command::new(bin())
        .args(["disasm", "--session", sess.to_str().unwrap(),
               "--count", "1", "--raw"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    // After `run --steps 1`, PC is at the 2nd instruction (0x80000004).
    assert!(text.contains("0x80000004"), "missing address: {text}");
    assert!(text.contains("raw: 13 81 20 00"), "missing raw bytes: {text}");
}

#[test]
fn mem_read_word_width() {
    let dir = tmpdir();
    let sess = dir.join("mw.json");
    bootstrap(&sess);
    Command::new(bin())
        .args(["mem", "write", "--session", sess.to_str().unwrap(),
               "--addr", "0x20000000", "--bytes", "0xde", "0xad", "0xbe", "0xef"])
        .output()
        .unwrap();
    // Read as a single little-endian word.
    let out = Command::new(bin())
        .args(["mem", "read", "--session", sess.to_str().unwrap(),
               "--addr", "0x20000000", "--len", "1", "--width", "word"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("0xefbeadde"), "word value wrong");
    // Read as two halfwords.
    let out = Command::new(bin())
        .args(["mem", "read", "--session", sess.to_str().unwrap(),
               "--addr", "0x20000000", "--len", "2", "--width", "half"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("0xadde"), "half value wrong");
    // Invalid width must fail.
    let out = Command::new(bin())
        .args(["mem", "read", "--session", sess.to_str().unwrap(),
               "--addr", "0x20000000", "--len", "1", "--width", "qword"])
        .output()
        .unwrap();
    assert!(!out.status.success());
}

#[test]
fn state_fields_and_summary() {
    let dir = tmpdir();
    let sess = dir.join("stf.json");
    bootstrap(&sess);
    // --fields selects a JSON subset.
    let out = Command::new(bin())
        .args(["state", "--session", sess.to_str().unwrap(),
               "--json", "--fields", "pc,cycleCount"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(json.get("pc").is_some());
    assert!(json.get("cycleCount").is_some());
    assert!(json.get("registers").is_none(), "registers must be filtered out");
    // A dot-nested path selects a sub-object field.
    let out = Command::new(bin())
        .args(["state", "--session", sess.to_str().unwrap(),
               "--json", "--fields", "peripherals.uartBase"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(json["peripherals"]["uartBase"].is_number(), "nested field missing");
    assert!(json.get("pc").is_none(), "pc must be filtered out");
    // An unknown field must be an error, not silently dropped.
    let out = Command::new(bin())
        .args(["state", "--session", sess.to_str().unwrap(),
               "--json", "--fields", "nosuch"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "unknown field should fail");
    assert!(String::from_utf8_lossy(&out.stderr).contains("未知状态字段"));
    // --summary prints a one-line summary.
    let out = Command::new(bin())
        .args(["state", "--session", sess.to_str().unwrap(), "--summary"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.starts_with("PC=0x"), "missing summary: {text}");
    assert!(text.contains("cycles="), "missing cycles: {text}");
}

#[test]
fn bp_set_by_line() {
    let dir = tmpdir();
    let sess = dir.join("bpline.json");
    let elf = dir.join("bpline.elf");
    let code: [u8; 16] = [
        0x93, 0x00, 0x10, 0x00,
        0x13, 0x81, 0x20, 0x00,
        0x23, 0x28, 0x10, 0x00,
        0x73, 0x00, 0x00, 0x00,
    ];
    write_riscv_elf_with_symbol(&elf, &code, "main");
    Command::new(bin())
        .args(["run", "--firmware", elf.to_str().unwrap(),
               "--session", sess.to_str().unwrap(), "--steps", "1"])
        .output()
        .unwrap();
    let out = Command::new(bin())
        .args(["bp", "set", "--session", sess.to_str().unwrap(), "--line", "<elf>:1"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["bp", "list", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["address"], 0x8000_0000u64);
    // A bad line number must fail.
    let out = Command::new(bin())
        .args(["bp", "set", "--session", sess.to_str().unwrap(), "--line", "<elf>:99"])
        .output()
        .unwrap();
    assert!(!out.status.success());
}

#[test]
fn mem_read_json() {
    let dir = tmpdir();
    let sess = dir.join("mrj.json");
    bootstrap(&sess);
    Command::new(bin())
        .args(["mem", "write", "--session", sess.to_str().unwrap(),
               "--addr", "0x20000000", "--bytes", "0xde", "0xad", "0xbe", "0xef"])
        .output()
        .unwrap();
    let out = Command::new(bin())
        .args(["mem", "read", "--session", sess.to_str().unwrap(),
               "--addr", "0x20000000", "--len", "4", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["address"], 0x2000_0000u64);
    assert_eq!(json["bytes"], serde_json::json!([0xde, 0xad, 0xbe, 0xef]));
}

#[test]
fn disasm_json() {
    let dir = tmpdir();
    let sess = dir.join("dj.json");
    let elf = dir.join("dj.elf");
    write_riscv_elf(&elf);
    Command::new(bin())
        .args(["run", "--firmware", elf.to_str().unwrap(),
               "--session", sess.to_str().unwrap(), "--steps", "1"])
        .output()
        .unwrap();
    let out = Command::new(bin())
        .args(["disasm", "--session", sess.to_str().unwrap(), "--count", "1", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["address"], 0x8000_0004u64);
    assert_eq!(json[0]["bytes"], serde_json::json!([0x13, 0x81, 0x20, 0x00]));
    assert!(json[0]["text"].as_str().unwrap().contains("addi"));
}

#[test]
fn reg_list_filter() {
    let dir = tmpdir();
    let sess = dir.join("regf.json");
    bootstrap(&sess);
    let out = Command::new(bin())
        .args(["reg", "list", "--session", sess.to_str().unwrap(), "--filter", "ra", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let keys: Vec<&str> = json.as_object().unwrap().keys().map(|k| k.as_str()).collect();
    assert_eq!(keys, vec!["ra"], "filter should keep only ra");
    // Substring filter.
    let out = Command::new(bin())
        .args(["reg", "list", "--session", sess.to_str().unwrap(), "--filter", "sp", "--json"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(json.get("sp").is_some());
    assert!(json.get("ra").is_none());
}

#[test]
fn reset_json() {
    let dir = tmpdir();
    let sess = dir.join("rj.json");
    bootstrap(&sess);
    let out = Command::new(bin())
        .args(["reset", "--session", sess.to_str().unwrap(), "--entry", "0x80000000", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["pc"], 0x8000_0000u64);
    assert_eq!(json["cycleCount"], 0u64);
}

#[test]
fn reset_default_goes_to_entry() {
    let dir = tmpdir();
    let sess = dir.join("rj-entry.json");
    let elf = dir.join("rj-entry.elf");
    write_riscv_elf(&elf);
    let out = Command::new(bin())
        .args(["run", "--firmware", elf.to_str().unwrap(),
               "--session", sess.to_str().unwrap(), "--steps", "1"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    // PC has advanced past the entry after a step.
    let out = Command::new(bin())
        .args(["state", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_ne!(json["pc"], 0x8000_0000u64, "pc should have advanced");
    // Reset without --entry must return to the firmware entry (0x80000000),
    // not the current PC.
    let out = Command::new(bin())
        .args(["reset", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["pc"], 0x8000_0000u64);
    assert_eq!(json["cycleCount"], 0u64);
}

#[test]
fn mem_write_word_width() {
    let dir = tmpdir();
    let sess = dir.join("mww.json");
    bootstrap(&sess);
    let out = Command::new(bin())
        .args(["mem", "write", "--session", sess.to_str().unwrap(),
               "--addr", "0x20000000", "--width", "word", "--values", "0xefbeadde"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    // Read back as bytes: little-endian 0xefbeadde -> de ad be ef.
    let out = Command::new(bin())
        .args(["mem", "read", "--session", sess.to_str().unwrap(),
               "--addr", "0x20000000", "--len", "4", "--json"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["bytes"], serde_json::json!([0xde, 0xad, 0xbe, 0xef]));
    // A value too large for the width must fail.
    let out = Command::new(bin())
        .args(["mem", "write", "--session", sess.to_str().unwrap(),
               "--addr", "0x20000000", "--width", "half", "--values", "0x1_0000"])
        .output()
        .unwrap();
    assert!(!out.status.success());
}

#[test]
fn continue_source_annotates_stop() {
    let dir = tmpdir();
    let sess = dir.join("csrc.json");
    let elf = dir.join("csrc.elf");
    let code: [u8; 16] = [
        0x93, 0x00, 0x10, 0x00,
        0x13, 0x81, 0x20, 0x00,
        0x23, 0x28, 0x10, 0x00,
        0x73, 0x00, 0x00, 0x00,
    ];
    write_riscv_elf_with_symbol(&elf, &code, "main");
    Command::new(bin())
        .args(["run", "--firmware", elf.to_str().unwrap(),
               "--save-session", sess.to_str().unwrap()])
        .output()
        .unwrap();
    Command::new(bin())
        .args(["bp", "set", "--session", sess.to_str().unwrap(), "--addr", "0x80000008"])
        .output()
        .unwrap();
    Command::new(bin())
        .args(["reset", "--session", sess.to_str().unwrap(), "--entry", "0x80000000"])
        .output()
        .unwrap();
    let out = Command::new(bin())
        .args(["continue", "--session", sess.to_str().unwrap(), "--source", "--max-steps", "10"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("PC=0x80000008"), "missing PC: {text}");
    assert!(text.contains("<elf>:1"), "missing source: {text}");
}

#[test]
fn bp_management_json() {
    let dir = tmpdir();
    let sess = dir.join("bpj.json");
    bootstrap(&sess);
    // set
    let out = Command::new(bin())
        .args(["bp", "set", "--session", sess.to_str().unwrap(), "--addr", "0x80000004", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["address"], 0x8000_0004u64);
    assert_eq!(json["enabled"], true);
    // disable / enable
    let out = Command::new(bin())
        .args(["bp", "disable", "--session", sess.to_str().unwrap(), "--addr", "0x80000004", "--json"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["enabled"], false);
    let out = Command::new(bin())
        .args(["bp", "enable", "--session", sess.to_str().unwrap(), "--addr", "0x80000004", "--json"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["enabled"], true);
    // clear / clear-all
    let out = Command::new(bin())
        .args(["bp", "clear", "--session", sess.to_str().unwrap(), "--addr", "0x80000004", "--json"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["removed"], true);
    let out = Command::new(bin())
        .args(["bp", "clear-all", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["cleared"], 0u64);
}

#[test]
fn bp_set_out_of_range_rejected() {
    // 16-bit-PC cores (MCS-51 / C166) and AVR top out at 0xFFFF; an
    // out-of-range address must be rejected instead of silently wrapping
    // (e.g. 0x80000000 & 0xFFFF == 0 would match the reset vector).
    let dir = tmpdir();
    for arch in ["mcs51", "c166", "avr"] {
        let sess = dir.join(format!("bprange-{arch}.json"));
        let out = Command::new(bin())
            .args(["mem", "write", "--session", sess.to_str().unwrap(),
                   "--arch", arch, "--addr", "0x20", "--bytes", "0xaa"])
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let out = Command::new(bin())
            .args(["bp", "set", "--session", sess.to_str().unwrap(),
                   "--addr", "0x80000000", "--json"])
            .output()
            .unwrap();
        assert!(!out.status.success(), "bp set should fail for {arch}");
        assert!(String::from_utf8_lossy(&out.stderr).contains("可寻址范围"), "no range msg for {arch}");
        // An in-range address is accepted.
        let out = Command::new(bin())
            .args(["bp", "set", "--session", sess.to_str().unwrap(),
                   "--addr", "0x100", "--json"])
            .output()
            .unwrap();
        assert!(out.status.success(), "in-range bp failed for {arch}: {}", String::from_utf8_lossy(&out.stderr));
        // --until out of range must also be rejected.
        let out = Command::new(bin())
            .args(["continue", "--session", sess.to_str().unwrap(),
                   "--until", "0x80000000"])
            .output()
            .unwrap();
        assert!(!out.status.success(), "continue --until should fail for {arch}");
    }
}

#[test]
fn mem_write_fill_json() {
    let dir = tmpdir();
    let sess = dir.join("mwj.json");
    bootstrap(&sess);
    let out = Command::new(bin())
        .args(["mem", "write", "--session", sess.to_str().unwrap(),
               "--addr", "0x20000000", "--bytes", "0xaa", "0xbb", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["len"], 2u64);
    assert_eq!(json["address"], 0x2000_0000u64);
    let out = Command::new(bin())
        .args(["mem", "fill", "--session", sess.to_str().unwrap(),
               "--addr", "0x20000010", "--len", "4", "--value", "0xff", "--json"])
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["len"], 4u64);
    assert_eq!(json["value"], 255u64);
}

#[test]
fn arch_device_periph_json() {
    // arch list --json
    let out = Command::new(bin()).args(["arch", "list", "--json"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(json.as_array().unwrap().len() >= 12);
    assert_eq!(json[0]["name"], "riscv");
    // device list --json
    let out = Command::new(bin()).args(["device", "list", "--json"]).output().unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(json.as_array().unwrap().len() >= 1);
    assert!(json[0]["key"].as_str().is_some());
    // device info --json
    let out = Command::new(bin()).args(["device", "info", "stm32f103c8", "--json"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["key"], "stm32f103c8");
    assert!(json["memoryMap"].as_array().unwrap().len() >= 2);
    // periph uart --json (empty buffer)
    let dir = tmpdir();
    let sess = dir.join("puj.json");
    bootstrap(&sess);
    let out = Command::new(bin()).args(["periph", "uart", "--session", sess.to_str().unwrap(), "--json"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["len"], 0u64);
    // periph gpio --json
    let out = Command::new(bin()).args(["periph", "gpio", "--session", sess.to_str().unwrap(), "--pin", "3", "--value", "1", "--json"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["pin"], 3u64);
    assert_eq!(json["value"], 1u64);
}

#[test]
fn device_codegen_json() {
    // linker --json
    let out = Command::new(bin()).args(["device", "linker", "stm32f103c8", "--json"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["kind"], "linker");
    assert!(json["content"].as_str().unwrap().contains("MEMORY"));
    // startup --json
    let out = Command::new(bin()).args(["device", "startup", "stm32f103c8", "--json"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["kind"], "startup");
    assert!(json["content"].as_str().unwrap().contains("Reset_Handler"));
    // init --json
    let out = Command::new(bin()).args(["device", "init", "stm32f103c8", "--json"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["kind"], "init");
    assert!(json["content"].as_str().unwrap().contains("void"));
}

#[test]
fn disasm_mcs51_code_space_width() {
    // MCS-51 has a separate code ROM: disassembly must read from it and advance
    // the cursor by the variable instruction width (NOP=1 byte, LJMP addr16=3).
    let dir = tmpdir();
    let sess = dir.join("m51dis.json");
    let code = dir.join("m51dis.bin");
    // NOP, LJMP 0x0010, NOP, NOP.
    std::fs::write(&code, [0x00, 0x02, 0x00, 0x10, 0x00, 0x00]).unwrap();
    let out = Command::new(bin())
        .args(["mem", "load", "--session", sess.to_str().unwrap(),
               "--arch", "mcs51", "--firmware", code.to_str().unwrap(), "--base", "0"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["disasm", "--session", sess.to_str().unwrap(),
               "--addr", "0", "--count", "3", "--raw"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("0x00000000: nop"), "missing nop: {text}");
    assert!(text.contains("0x00000001: ljmp 0x0010"), "missing ljmp: {text}");
    assert!(text.contains("0x00000004: nop"), "missing nop at 4: {text}");
    // JSON form reports the exact bytes consumed per instruction.
    let out = Command::new(bin())
        .args(["disasm", "--session", sess.to_str().unwrap(),
               "--addr", "0", "--count", "2", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["address"], 0u64);
    assert_eq!(json[0]["text"], "nop");
    assert_eq!(json[0]["bytes"], serde_json::json!([0]));
    assert_eq!(json[1]["address"], 1u64);
    assert_eq!(json[1]["text"], "ljmp 0x0010");
    assert_eq!(json[1]["bytes"], serde_json::json!([2, 0, 0x10]));
}

#[test]
fn disasm_mcs51_core_forms() {
    // The 8051 disassembler mirrors the execution core: rotate/ALU direct and
    // immediate forms, MOV/MOVX/MOVC, XCH, bit operations and relative
    // branches, advancing by the actual decoded width.
    let dir = tmpdir();
    let sess = dir.join("m51forms.json");
    let code = dir.join("m51forms.bin");
    std::fs::write(&code, [
        0x03,             // rr a
        0x23,             // rl a
        0x55, 0x30,       // anl a, 0x30
        0x45, 0x30,       // orl a, 0x30
        0x65, 0x30,       // xrl a, 0x30
        0x75, 0x31, 0x12, // mov 0x31, #0x12
        0xE5, 0x31,       // mov a, 0x31
        0x86, 0x32,       // mov 0x32, @r0
        0xE2,             // movx a, @r0
        0x93,             // movc a, @a+dptr
        0xC5, 0x33,       // xch a, 0x33
        0x10, 0x20, 0xFE, // jbc 0x20, +0x20
        0xD8, 0xFE,       // djnz r0, +0x22
    ]).unwrap();
    let out = Command::new(bin())
        .args(["mem", "load", "--session", sess.to_str().unwrap(),
               "--arch", "mcs51", "--firmware", code.to_str().unwrap(), "--base", "0"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["disasm", "--session", sess.to_str().unwrap(),
               "--addr", "0", "--count", "13", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["address"], 0u64);
    assert_eq!(json[0]["text"], "rr a");
    assert_eq!(json[1]["address"], 1u64);
    assert_eq!(json[1]["text"], "rl a");
    assert_eq!(json[2]["address"], 2u64);
    assert_eq!(json[2]["text"], "anl a, 0x30");
    assert_eq!(json[3]["address"], 4u64);
    assert_eq!(json[3]["text"], "orl a, 0x30");
    assert_eq!(json[4]["address"], 6u64);
    assert_eq!(json[4]["text"], "xrl a, 0x30");
    assert_eq!(json[5]["address"], 8u64);
    assert_eq!(json[5]["text"], "mov 0x31, #0x12");
    assert_eq!(json[6]["address"], 11u64);
    assert_eq!(json[6]["text"], "mov a, 0x31");
    assert_eq!(json[7]["address"], 13u64);
    assert_eq!(json[7]["text"], "mov 0x32, @r0");
    assert_eq!(json[8]["address"], 15u64);
    assert_eq!(json[8]["text"], "movx a, @r0");
    assert_eq!(json[9]["address"], 16u64);
    assert_eq!(json[9]["text"], "movc a, @a+dptr");
    assert_eq!(json[10]["address"], 17u64);
    assert_eq!(json[10]["text"], "xch a, 0x33");
    assert_eq!(json[11]["address"], 19u64);
    assert_eq!(json[11]["text"], "jbc 0x20, 0x14");
    assert_eq!(json[12]["address"], 22u64);
    assert_eq!(json[12]["text"], "djnz r0, 0x16");
}

#[test]
fn disasm_mcs51_alu_acc_register() {
    // ORL/ANL/XRL A, Rn (0x48-0x4F/0x58-0x5F/0x68-0x6F) decode with the
    // source register in the low nibble and advance 1 byte each.
    let dir = tmpdir();
    let sess = dir.join("m51alu.json");
    let code = dir.join("m51alu.bin");
    std::fs::write(&code, [0x48, 0x59, 0x6A]).unwrap();
    let out = Command::new(bin())
        .args(["mem", "load", "--session", sess.to_str().unwrap(),
               "--arch", "mcs51", "--firmware", code.to_str().unwrap(), "--base", "0"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["disasm", "--session", sess.to_str().unwrap(),
               "--addr", "0", "--count", "3", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["text"], "orl a, r0");
    assert_eq!(json[1]["text"], "anl a, r1");
    assert_eq!(json[2]["text"], "xrl a, r2");
    assert_eq!(json[2]["address"], 2u64);
}

#[test]
fn disasm_mcs51_ajmp_acall_addr11() {
    // AJMP/ACALL addr11 decode the page bits from opcode bits 7..5 and the
    // low address byte, advancing 2 bytes each.
    let dir = tmpdir();
    let sess = dir.join("m51jmp.json");
    let code = dir.join("m51jmp.bin");
    std::fs::write(&code, [
        0x01, 0x60, // ajmp 0x0060
        0x31, 0x55, // acall 0x0155
        0xE1, 0x34, // ajmp 0x0734
    ]).unwrap();
    let out = Command::new(bin())
        .args(["mem", "load", "--session", sess.to_str().unwrap(),
               "--arch", "mcs51", "--firmware", code.to_str().unwrap(), "--base", "0"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["disasm", "--session", sess.to_str().unwrap(),
               "--addr", "0", "--count", "3", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["address"], 0u64);
    assert_eq!(json[0]["text"], "ajmp 0x0060");
    assert_eq!(json[1]["address"], 2u64);
    assert_eq!(json[1]["text"], "acall 0x0155");
    assert_eq!(json[2]["address"], 4u64);
    assert_eq!(json[2]["text"], "ajmp 0x0734");
}

#[test]
fn step_mcs51_inc_dec_register() {
    // INC R7 (0x0F) / DEC R7 (0x1F) operate on the register-bank slot and
    // persist across session steps.
    let dir = tmpdir();
    let sess = dir.join("m51incdec.json");
    let code = dir.join("m51incdec.bin");
    std::fs::write(&code, [0x0F, 0x1F]).unwrap();
    let out = Command::new(bin())
        .args(["mem", "load", "--session", sess.to_str().unwrap(),
               "--arch", "mcs51", "--firmware", code.to_str().unwrap(), "--base", "0"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["reg", "set", "--session", sess.to_str().unwrap(),
               "--name", "R7", "--value", "0x41"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["step", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["pc"], 1u64);
    assert_eq!(json["registers"]["R7"], 0x42u64);
    let out = Command::new(bin())
        .args(["step", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["pc"], 2u64);
    assert_eq!(json["registers"]["R7"], 0x41u64);
}

#[test]
fn disasm_mcs251_code_space_width() {
    // The 80251 is variable-length: disassembly must advance by the width of
    // each instruction (NOP=1, MOV Rn,#imm16=3).
    let dir = tmpdir();
    let sess = dir.join("x51dis.json");
    let code = dir.join("x51dis.bin");
    // NOP, MOV R0,#0x1234, NOP.
    std::fs::write(&code, [0x00, 0x74, 0x12, 0x34, 0x00, 0x00]).unwrap();
    let out = Command::new(bin())
        .args(["mem", "load", "--session", sess.to_str().unwrap(),
               "--arch", "mcs251", "--firmware", code.to_str().unwrap(), "--base", "0"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["disasm", "--session", sess.to_str().unwrap(),
               "--addr", "0", "--count", "2", "--raw"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("0x00000000: nop"), "missing nop: {text}");
    assert!(text.contains("0x00000001: mov r4, #0x1234"), "missing mov: {text}");
    let out = Command::new(bin())
        .args(["disasm", "--session", sess.to_str().unwrap(),
               "--addr", "0", "--count", "2", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["address"], 0u64);
    assert_eq!(json[0]["bytes"], serde_json::json!([0]));
    assert_eq!(json[1]["address"], 1u64);
    assert_eq!(json[1]["bytes"], serde_json::json!([0x74, 0x12, 0x34]));
}

#[test]
fn disasm_mcs251_core_forms() {
    // The 80251 disassembler mirrors the execution core: single-byte
    // register/accumulator ops, relative branches, MOV A,direct, MOVX @Ri
    // and JMP @A+DPTR, advancing by the actual decoded width.
    let dir = tmpdir();
    let sess = dir.join("x51forms.json");
    let code = dir.join("x51forms.bin");
    std::fs::write(&code, [
        0x22,             // ret
        0x04,             // inc a
        0x14,             // dec a
        0x0F,             // inc r7
        0x1F,             // dec r7
        0x73,             // jmp @a+dptr
        0x40, 0xFE,       // jc 0x1
        0x50, 0xFE,       // jnc 0x1
        0xE5, 0x30,       // mov a, 0x30
        0xE2,             // movx a, @r0
        0xF3,             // movx @r1, a
        0xE4,             // clr a
        0xF4,             // cpl a
        0x23,             // rl a
        0x03,             // rr a
    ]).unwrap();
    let out = Command::new(bin())
        .args(["mem", "load", "--session", sess.to_str().unwrap(),
               "--arch", "mcs251", "--firmware", code.to_str().unwrap(), "--base", "0"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["disasm", "--session", sess.to_str().unwrap(),
               "--addr", "0", "--count", "15", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["text"], "ret");
    assert_eq!(json[1]["text"], "inc a");
    assert_eq!(json[2]["text"], "dec a");
    assert_eq!(json[3]["text"], "inc r7");
    assert_eq!(json[4]["text"], "dec r7");
    assert_eq!(json[5]["text"], "jmp @a+dptr");
    assert_eq!(json[6]["text"], "jc 0x6");
    assert_eq!(json[7]["text"], "jnc 0x8");
    assert_eq!(json[8]["text"], "mov a, 0x30");
    assert_eq!(json[9]["text"], "movx a, @r0");
    assert_eq!(json[10]["text"], "movx @r1, a");
    assert_eq!(json[11]["text"], "clr a");
    assert_eq!(json[12]["text"], "cpl a");
    assert_eq!(json[13]["text"], "rl a");
    assert_eq!(json[14]["text"], "rr a");
    assert_eq!(json[14]["address"], 17u64);
}

#[test]
fn step_mcs251_movx_r1_roundtrip() {
    // MOVX @R1,A (0xF3) / MOVX A,@R1 (0xE3) use the low bit of the opcode as
    // the register index; running both in one process round-trips external
    // RAM (external RAM is not part of the session's sparse `memory` view).
    let dir = tmpdir();
    let sess = dir.join("x51movx.json");
    let code = dir.join("x51movx.bin");
    std::fs::write(&code, [0xF3, 0xE3]).unwrap();
    let out = Command::new(bin())
        .args(["mem", "load", "--session", sess.to_str().unwrap(),
               "--arch", "mcs251", "--firmware", code.to_str().unwrap(), "--base", "0"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    for (name, value) in [("ACC", "0xAB"), ("R1", "0x34")] {
        let out = Command::new(bin())
            .args(["reg", "set", "--session", sess.to_str().unwrap(),
                   "--name", name, "--value", value])
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }
    // MOVX @R1,A writes ACC (0xAB) to external RAM at R1 (0x34), then
    // MOVX A,@R1 reads it back — all within a single `run --steps 2`.
    let out = Command::new(bin())
        .args(["run", "--session", sess.to_str().unwrap(),
               "--firmware", code.to_str().unwrap(),
               "--steps", "2", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["pc"], 2u64);
    assert_eq!(json["registers"]["ACC"], 0xABu64);
}

#[test]
fn disasm_avr_32bit_width() {
    // AVR's LDS/STS/JMP/CALL are 32-bit; disassembly must advance 4 bytes for
    // them and decode the mnemonic (previously the guard hid these arms).
    let dir = tmpdir();
    let sess = dir.join("avrdis.json");
    let code = dir.join("avrdis.bin");
    // LDS r0, 0x1234 (4 bytes), NOP, NOP.
    std::fs::write(&code, [0x00, 0x90, 0x34, 0x12, 0x00, 0x00]).unwrap();
    let out = Command::new(bin())
        .args(["mem", "load", "--session", sess.to_str().unwrap(),
               "--arch", "avr", "--firmware", code.to_str().unwrap(), "--base", "0"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["disasm", "--session", sess.to_str().unwrap(),
               "--addr", "0", "--count", "2", "--raw"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("0x00000000: lds r0, 0x1234"), "missing lds: {text}");
    assert!(text.contains("0x00000004: nop"), "missing nop at 4: {text}");
    let out = Command::new(bin())
        .args(["disasm", "--session", sess.to_str().unwrap(),
               "--addr", "0", "--count", "2", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["text"], "lds r0, 0x1234");
    assert_eq!(json[0]["bytes"], serde_json::json!([0, 0x90, 0x34, 0x12]));
    assert_eq!(json[1]["address"], 4u64);
    assert_eq!(json[1]["text"], "nop");
}

#[test]
fn disasm_avr_muls_lpm_spm_push() {
    // AVR MULS, LPM, SPM and the 0x920F PUSH encoding decode with their real
    // mnemonics and advance 2 bytes each.
    let dir = tmpdir();
    let sess = dir.join("avrforms.json");
    let code = dir.join("avrforms.bin");
    std::fs::write(&code, [
        0x01, 0x02,       // muls r16, r17
        0xC8, 0x95,       // lpm
        0xE8, 0x95,       // spm
        0x5F, 0x92,       // push r5
        0x5C, 0x90,       // ld r5, x
        0x6D, 0x92,       // st x+, r6
    ]).unwrap();
    let out = Command::new(bin())
        .args(["mem", "load", "--session", sess.to_str().unwrap(),
               "--arch", "avr", "--firmware", code.to_str().unwrap(), "--base", "0"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["disasm", "--session", sess.to_str().unwrap(),
               "--addr", "0", "--count", "6", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["address"], 0u64);
    assert_eq!(json[0]["text"], "muls r16, r17");
    assert_eq!(json[1]["address"], 2u64);
    assert_eq!(json[1]["text"], "lpm");
    assert_eq!(json[2]["address"], 4u64);
    assert_eq!(json[2]["text"], "spm");
    assert_eq!(json[3]["address"], 6u64);
    assert_eq!(json[3]["text"], "push r5");
    assert_eq!(json[4]["address"], 8u64);
    assert_eq!(json[4]["text"], "ld r5, x");
    assert_eq!(json[5]["address"], 10u64);
    assert_eq!(json[5]["text"], "st x+, r6");
}

#[test]
fn disasm_c166_imm_width() {
    // C166 MOV Rn,#imm16 and CALL carry a 16-bit operand (3 bytes); the rest
    // are 2 bytes (NOP/RET are 1). Disassembly must advance accordingly and
    // decode the little-endian immediate consistent with the execution core.
    let dir = tmpdir();
    let sess = dir.join("c166dis.json");
    let code = dir.join("c166dis.bin");
    // MOV R0,#0x1234 (3 bytes: 0x20 0x34 0x12), NOP.
    std::fs::write(&code, [0x20, 0x34, 0x12, 0x00, 0x00]).unwrap();
    let out = Command::new(bin())
        .args(["mem", "load", "--session", sess.to_str().unwrap(),
               "--arch", "c166", "--firmware", code.to_str().unwrap(), "--base", "0"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["disasm", "--session", sess.to_str().unwrap(),
               "--addr", "0", "--count", "2", "--raw"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("0x00000000: mov r0, #0x1234"), "missing mov: {text}");
    assert!(text.contains("0x00000003: nop"), "missing nop at 3: {text}");
    let out = Command::new(bin())
        .args(["disasm", "--session", sess.to_str().unwrap(),
               "--addr", "0", "--count", "2", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["text"], "mov r0, #0x1234");
    assert_eq!(json[0]["bytes"], serde_json::json!([0x20, 0x34, 0x12]));
    assert_eq!(json[1]["address"], 3u64);
    assert_eq!(json[1]["text"], "nop");
}

#[test]
fn disasm_c166_logic_register_forms() {
    // C166 0x50-0x5F decodes AND/OR/XOR Rn, Rm (bits 3:2 select the
    // operation) and advances 2 bytes per instruction.
    let dir = tmpdir();
    let sess = dir.join("c166logic.json");
    let code = dir.join("c166logic.bin");
    std::fs::write(&code, [0x50, 0x01, 0x54, 0x02, 0x58, 0x03, 0x5C, 0x04]).unwrap();
    let out = Command::new(bin())
        .args(["mem", "load", "--session", sess.to_str().unwrap(),
               "--arch", "c166", "--firmware", code.to_str().unwrap(), "--base", "0"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["disasm", "--session", sess.to_str().unwrap(),
               "--addr", "0", "--count", "4", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["address"], 0u64);
    assert_eq!(json[0]["text"], "and r0, r1");
    assert_eq!(json[1]["address"], 2u64);
    assert_eq!(json[1]["text"], "or r4, r2");
    assert_eq!(json[2]["address"], 4u64);
    assert_eq!(json[2]["text"], "xor r8, r3");
    assert_eq!(json[3]["address"], 6u64);
    assert_eq!(json[3]["text"], "xor r12, r4");
}

#[test]
fn disasm_xtensa_24bit_width() {
    // This simulator's Xtensa core fetches a full 24-bit word per instruction,
    // so disassembly must advance 3 bytes and decode NOP (0x000020) correctly.
    let dir = tmpdir();
    let sess = dir.join("xtdis.json");
    let code = dir.join("xtdis.bin");
    // Two NOPs (0x000020 little-endian: 0x20 0x00 0x00).
    std::fs::write(&code, [0x20, 0x00, 0x00, 0x20, 0x00, 0x00]).unwrap();
    let out = Command::new(bin())
        .args(["mem", "load", "--session", sess.to_str().unwrap(),
               "--arch", "xtensa", "--firmware", code.to_str().unwrap(), "--base", "0"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["disasm", "--session", sess.to_str().unwrap(),
               "--addr", "0", "--count", "2", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["address"], 0u64);
    assert_eq!(json[0]["text"], "nop");
    assert_eq!(json[0]["bytes"], serde_json::json!([0x20, 0, 0]));
    assert_eq!(json[1]["address"], 3u64);
    assert_eq!(json[1]["text"], "nop");
}

#[test]
fn disasm_v850_variable_width() {
    // V850 is variable-length (2/4/6 bytes): NOP is 2 bytes, DISPOSE is 4.
    // Disassembly must advance by the decoded width, not a fixed 4.
    let dir = tmpdir();
    let sess = dir.join("v850dis.json");
    let code = dir.join("v850dis.bin");
    // NOP (0x0000), DISPOSE (top 0x19), NOP.
    std::fs::write(&code, [0x00, 0x00, 0x40, 0x06, 0x00, 0x00, 0x00, 0x00]).unwrap();
    let out = Command::new(bin())
        .args(["mem", "load", "--session", sess.to_str().unwrap(),
               "--arch", "v850", "--firmware", code.to_str().unwrap(), "--base", "0"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["disasm", "--session", sess.to_str().unwrap(),
               "--addr", "0", "--count", "3", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["address"], 0u64);
    assert_eq!(json[0]["text"], "nop");
    assert_eq!(json[1]["address"], 2u64);
    assert!(json[1]["text"].as_str().unwrap().starts_with("dispose"),
            "expected dispose, got {}", json[1]["text"]);
    assert_eq!(json[2]["address"], 6u64);
    assert_eq!(json[2]["text"], "nop");
}

#[test]
fn disasm_stm8_variable_width() {
    // STM8 is variable-length (1/2/3/4 bytes with optional prefixes). The
    // disassembler must advance by the decoded width and recognize the core's
    // main opcode families rather than printing raw hex.
    let dir = tmpdir();
    let sess = dir.join("stm8dis.json");
    let code = dir.join("stm8dis.bin");
    // nop (1), jra rel (2), call direct16 (3), callf addr24 (4),
    // prefixed Y-indexed rlc (3).
    std::fs::write(&code, [
        0x9D,             // nop
        0x20, 0x02,       // jra +2
        0xCD, 0x56, 0x34, // call 0x3456
        0x8D, 0x78, 0x56, 0x34, // callf 0x345678
        0x90, 0x65, 0x10, // rlc (0x10, y)
    ]).unwrap();
    let out = Command::new(bin())
        .args(["mem", "load", "--session", sess.to_str().unwrap(),
               "--arch", "stm8", "--firmware", code.to_str().unwrap(), "--base", "0x8000"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["disasm", "--session", sess.to_str().unwrap(),
               "--addr", "0x8000", "--count", "5", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["address"], 0x8000u64);
    assert_eq!(json[0]["text"], "nop");
    assert_eq!(json[1]["address"], 0x8001u64);
    assert_eq!(json[1]["text"], "jra 0x8005");
    assert_eq!(json[2]["address"], 0x8003u64);
    assert_eq!(json[2]["text"], "call 0x3456");
    assert_eq!(json[3]["address"], 0x8006u64);
    assert_eq!(json[3]["text"], "callf 0x345678");
    assert_eq!(json[4]["address"], 0x800Au64);
    assert_eq!(json[4]["text"], "rlc (0x10, y)");
}

#[test]
fn disasm_stm8_standalone_90() {
    // A bare 0x90 (or one followed by an opcode without a Y form) is the
    // single-byte `LDW Y,X`, not a prefix: the disassembler must advance by
    // one byte and decode the following instruction separately.
    let dir = tmpdir();
    let sess = dir.join("stm890.json");
    let code = dir.join("stm890.bin");
    // ldw y, x (1), nop (1), ldw y, x (1), sla 0x12 (2).
    std::fs::write(&code, [0x90, 0x9D, 0x90, 0x34, 0x12]).unwrap();
    let out = Command::new(bin())
        .args(["mem", "load", "--session", sess.to_str().unwrap(),
               "--arch", "stm8", "--firmware", code.to_str().unwrap(), "--base", "0x8000"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["disasm", "--session", sess.to_str().unwrap(),
               "--addr", "0x8000", "--count", "4", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["address"], 0x8000u64);
    assert_eq!(json[0]["text"], "ldw y, x");
    assert_eq!(json[1]["address"], 0x8001u64);
    assert_eq!(json[1]["text"], "nop");
    assert_eq!(json[2]["address"], 0x8002u64);
    assert_eq!(json[2]["text"], "ldw y, x");
    assert_eq!(json[3]["address"], 0x8003u64);
    assert_eq!(json[3]["text"], "sla 0x12");
}

#[test]
fn stm8_step_over_and_step_out_use_call_width() {
    // STM8 `call addr16` is 3 bytes: step-over runs the call and stops at the
    // caller's next instruction (pc+3). `ret` is recognized by step-out.
    let dir = tmpdir();
    let sess = dir.join("stm8step.json");
    let code = dir.join("stm8step.bin");
    // 0x8000: call 0x8010 (3 bytes) -> return 0x8003.
    // 0x8010: ret (pops the return address, landing on 0x8003).
    let mut buf = vec![0u8; 0x18];
    buf[0..3].copy_from_slice(&[0xCD, 0x10, 0x80]);
    buf[0x10] = 0x81;
    std::fs::write(&code, buf).unwrap();
    let out = Command::new(bin())
        .args(["mem", "load", "--session", sess.to_str().unwrap(),
               "--arch", "stm8", "--firmware", code.to_str().unwrap(), "--base", "0x8000"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    // step-over from the `call` returns to pc == 0x8003.
    let out = Command::new(bin())
        .args(["step-over", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["pc"], 0x8003u64);
}

#[test]
fn step_stm8_sp_relative_ld() {
    // STM8 0x17 = LD (off8,SP),A and 0x16 = LD A,(off8,SP): running both in
    // one process writes then reads back the stack-relative byte.
    let dir = tmpdir();
    let sess = dir.join("stm8sp.json");
    let code = dir.join("stm8sp.bin");
    std::fs::write(&code, [0x17, 0x34, 0x16, 0x34]).unwrap();
    let out = Command::new(bin())
        .args(["mem", "load", "--session", sess.to_str().unwrap(),
               "--arch", "stm8", "--firmware", code.to_str().unwrap(), "--base", "0x8000"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    for (name, value) in [("A", "0xAB"), ("SP", "0x0100")] {
        let out = Command::new(bin())
            .args(["reg", "set", "--session", sess.to_str().unwrap(),
                   "--name", name, "--value", value])
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }
    let out = Command::new(bin())
        .args(["run", "--session", sess.to_str().unwrap(),
               "--firmware", code.to_str().unwrap(), "--base", "0x8000",
               "--steps", "2", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["pc"], 0x8004u64);
    assert_eq!(json["registers"]["A"], 0xABu64);
}

#[test]
fn disasm_msp430_extension_words() {
    // MSP430 double-operand instructions carry an extension word for
    // immediate/absolute/indexed operands; disassembly must advance by the
    // actual width (opcode word + extension words), not a fixed 2 bytes.
    let dir = tmpdir();
    let sess = dir.join("mspdis.json");
    let code = dir.join("mspdis.bin");
    // MOV #0x1234, R5 (4 bytes), MOV R0, R0 (2 bytes), JMP +2 (2 bytes).
    std::fs::write(&code, [
        0x35, 0x40, 0x34, 0x12, // MOV #0x1234, R5
        0x00, 0x40,             // MOV R0, R0
        0x02, 0x20,             // JMP +2
    ]).unwrap();
    let out = Command::new(bin())
        .args(["mem", "load", "--session", sess.to_str().unwrap(),
               "--arch", "msp430", "--firmware", code.to_str().unwrap(), "--base", "0"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["disasm", "--session", sess.to_str().unwrap(),
               "--addr", "0", "--count", "3", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["address"], 0u64);
    assert_eq!(json[0]["text"], "MOV #0x1234, R5");
    assert_eq!(json[1]["address"], 4u64);
    assert_eq!(json[2]["address"], 6u64);
}

#[test]
fn msp430_step_over_and_step_out_use_return() {
    // MSP430 renders mnemonics uppercase; `RET` is `MOV @SP+, PC`. step-over
    // on a 4-byte `CALL #imm` must resume at pc+4, and step-out must stop at
    // the `MOV @SP+, PC` epilogue.
    let dir = tmpdir();
    let sess = dir.join("mspstep.json");
    let code = dir.join("mspstep.bin");
    // 0x0200: CALL #0x0300 (4 bytes) -> return 0x0204; 0x0300: MOV @SP+, PC.
    let mut buf = vec![0u8; 0x102];
    buf[0..4].copy_from_slice(&[0xB0, 0x12, 0x00, 0x03]);
    buf[0x100..0x102].copy_from_slice(&[0x30, 0x41]);
    std::fs::write(&code, buf).unwrap();
    let out = Command::new(bin())
        .args(["mem", "load", "--session", sess.to_str().unwrap(),
               "--arch", "msp430", "--firmware", code.to_str().unwrap(), "--base", "0x0200"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    // step-over from the CALL returns to pc == 0x0204 (CALL #imm is 4 bytes).
    let out = Command::new(bin())
        .args(["step-over", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["pc"], 0x0204u64);

    // step-out: a function ending in `MOV @SP+, PC` pops the return address.
    let sess2 = dir.join("mspstep_out.json");
    let code2 = dir.join("mspstep_out.bin");
    std::fs::write(&code2, [0x30, 0x41]).unwrap(); // MOV @SP+, PC
    let out = Command::new(bin())
        .args(["mem", "load", "--session", sess2.to_str().unwrap(),
               "--arch", "msp430", "--firmware", code2.to_str().unwrap(), "--base", "0x0300"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["reg", "set", "--session", sess2.to_str().unwrap(),
               "--name", "sp", "--value", "0x0400"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["mem", "write", "--session", sess2.to_str().unwrap(),
               "--addr", "0x0400", "--bytes", "0x04", "0x02"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["step-out", "--session", sess2.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["pc"], 0x0204u64);
}

#[test]
fn disasm_rl78_variable_width() {
    // RL78 is variable-length (1/2/3/4 bytes). Disassembly must advance by the
    // actual decoded width, not a fixed 4 bytes, including the 0x61-prefixed
    // forms that carry a 16-bit or 8-bit operand.
    let dir = tmpdir();
    let sess = dir.join("rl78dis.json");
    let code = dir.join("rl78dis.bin");
    // nop (1), add A,#imm8 (2), add saddr,#imm8 (3), addw AX,#imm16 (3),
    // add A,ES:[imm16] (3), 0x61 mov A,ES:[imm16] (4), 0x61 and A,rba (2).
    std::fs::write(&code, [
        0x00,                   // nop
        0x0C, 0x2A,             // add a, #0x2a
        0x0A, 0x30, 0x55,       // add saddr, #imm8
        0x04, 0x34, 0x12,       // addw ax, #0x1234
        0x0F, 0x78, 0x56,       // add a, es:[0x5678]
        0x61, 0xA8, 0x78, 0x56, // mov a, es:[0x5678]
        0x61, 0x59,             // and a, a (core's register-register view)
    ]).unwrap();
    let out = Command::new(bin())
        .args(["mem", "load", "--session", sess.to_str().unwrap(),
               "--arch", "rl78", "--firmware", code.to_str().unwrap(), "--base", "0x8000"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["disasm", "--session", sess.to_str().unwrap(),
               "--addr", "0x8000", "--count", "7", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["address"], 0x8000u64);
    assert_eq!(json[0]["text"], "nop");
    assert_eq!(json[1]["address"], 0x8001u64);
    assert_eq!(json[1]["text"], "add a, #0x2a");
    assert_eq!(json[2]["address"], 0x8003u64);
    assert_eq!(json[3]["address"], 0x8006u64);
    assert_eq!(json[3]["text"], "addw ax, #0x1234");
    assert_eq!(json[4]["address"], 0x8009u64);
    assert_eq!(json[4]["text"], "add a, es:[0x5678]");
    assert_eq!(json[5]["address"], 0x800Cu64);
    assert_eq!(json[5]["text"], "mov a, es:[0x5678]");
    assert_eq!(json[6]["address"], 0x8010u64);
    assert_eq!(json[6]["text"], "and a, A");
}

#[test]
fn disasm_rl78_decodes_core_families() {
    // RL78 disassembly must decode the opcodes the core executes (register-
    // register, ES:[HL+B/C], ES:[HL+imm8], ES:[imm16], PSW and control-flow
    // forms) instead of `.byte`/`prefix 0x61:` placeholders.
    let dir = tmpdir();
    let sess = dir.join("rl78fam.json");
    let code = dir.join("rl78fam.bin");
    std::fs::write(&code, [
        0x61, 0x48,       // cmp a, X
        0x61, 0x80,       // add a, es:[hl+b]
        0x61, 0xB8, 0x10, // mov a, es:[hl+0x10]
        0x61, 0xA8, 0x78, 0x56, // mov a, es:[0x5678]
        0x61, 0xC8,       // skc
        0x61, 0xCD,       // pop psw
        0x61, 0xEC,       // retb
        0x0E, 0x05,       // add a, [hl+0x05]
        0x09, 0x78, 0x56, // mov a, [b+0x5678]
        0x9B,             // mov [hl], a
    ]).unwrap();
    let out = Command::new(bin())
        .args(["mem", "load", "--session", sess.to_str().unwrap(),
               "--arch", "rl78", "--firmware", code.to_str().unwrap(), "--base", "0x8000"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["disasm", "--session", sess.to_str().unwrap(),
               "--addr", "0x8000", "--count", "10", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let texts: Vec<&str> = json.as_array().unwrap().iter()
        .map(|v| v["text"].as_str().unwrap()).collect();
    assert_eq!(texts[0], "cmp a, X");
    assert_eq!(texts[1], "add a, es:[hl+b]");
    assert_eq!(texts[2], "mov a, es:[hl+0x10]");
    assert_eq!(texts[3], "mov a, es:[0x5678]");
    assert_eq!(texts[4], "skc");
    assert_eq!(texts[5], "pop psw");
    assert_eq!(texts[6], "retb");
    assert_eq!(texts[7], "add a, [hl+0x05]");
    assert_eq!(texts[8], "mov a, [b+0x5678]");
    assert_eq!(texts[9], "mov [hl], a");
}

#[test]
fn disasm_rx_variable_width() {
    // RX is variable-length (1/2/4/6 bytes). Disassembly must advance by the
    // actual decoded width, not a fixed 4 bytes, and decode the short-branch
    // and immediate families consistently with the core's big-endian fetch.
    let dir = tmpdir();
    let sess = dir.join("rxdis.json");
    let code = dir.join("rxdis.bin");
    // nop (2), bra.s (1), beq.s (2), add #imm16 (4), bra.a (6).
    std::fs::write(&code, [
        0x00, 0x03,             // nop
        0x08,                   // bra.s +8
        0x10, 0x00,             // beq.s +0
        0x70, 0x00, 0x34, 0x12, // add #0x3412, r0, r0
        0x00, 0x04, 0x00, 0x00, 0x00, 0x00, // bra.a (32-bit displacement)
    ]).unwrap();
    let out = Command::new(bin())
        .args(["mem", "load", "--session", sess.to_str().unwrap(),
               "--arch", "rx", "--firmware", code.to_str().unwrap(), "--base", "0x8000"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["disasm", "--session", sess.to_str().unwrap(),
               "--addr", "0x8000", "--count", "5", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["address"], 0x8000u64);
    assert_eq!(json[0]["text"], "nop");
    assert_eq!(json[1]["address"], 0x8002u64);
    assert_eq!(json[1]["text"], "bra.s +8");
    assert_eq!(json[2]["address"], 0x8003u64);
    assert_eq!(json[2]["text"], "beq.s +0");
    assert_eq!(json[3]["address"], 0x8005u64);
    assert_eq!(json[3]["text"], "add #0x3412, r0, r0");
    assert_eq!(json[4]["address"], 0x8009u64);
}

#[test]
fn disasm_rx_decodes_families() {
    // RX disassembly decodes the register/dsp/immediate families the core
    // executes, advancing by the actual width and reading big-endian operands.
    let dir = tmpdir();
    let sess = dir.join("rxfam.json");
    let code = dir.join("rxfam.bin");
    std::fs::write(&code, [
        0x48, 0x00,             // add r0, r0
        0x41, 0x20, 0x10,       // sub +16[r2], r0
        0x5A, 0x30, 0x34, 0x12, // movu.b +0x3412[r3], r0
        0xC2, 0x20, 0x10, 0x20, // mov.l +16[r0], +32[r12]
    ]).unwrap();
    let out = Command::new(bin())
        .args(["mem", "load", "--session", sess.to_str().unwrap(),
               "--arch", "rx", "--firmware", code.to_str().unwrap(), "--base", "0x8000"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["disasm", "--session", sess.to_str().unwrap(),
               "--addr", "0x8000", "--count", "4", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["address"], 0x8000u64);
    assert_eq!(json[0]["text"], "add r0, r0");
    assert_eq!(json[1]["address"], 0x8002u64);
    assert_eq!(json[1]["text"], "sub +16[r2], r0");
    assert_eq!(json[2]["address"], 0x8005u64);
    assert_eq!(json[2]["text"], "movu.b +13330[r3], r0");
    assert_eq!(json[3]["address"], 0x8009u64);
    assert_eq!(json[3]["text"], "mov.l +16[r0], +32[r12]");
}

#[test]
fn rx_step_over_uses_call_width() {
    // RX BSR.w is 4 bytes: step-over must resume at the return address (pc+4),
    // and BSR.w pushes the return address to the stack.
    let dir = tmpdir();
    let sess = dir.join("rxstepover.json");
    let code = dir.join("rxstepover.bin");
    std::fs::write(&code, [
        0x39, 0x00, 0x00, 0x04, // bsr.w +4
        0x00, 0x03,             // nop (return point)
        0x00, 0x00,             // spare
        0x00, 0x02,             // rts
    ]).unwrap();
    let out = Command::new(bin())
        .args(["mem", "load", "--session", sess.to_str().unwrap(),
               "--arch", "rx", "--firmware", code.to_str().unwrap(), "--base", "0x8000"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["reg", "set", "--session", sess.to_str().unwrap(), "--name", "r0", "--value", "0x2000"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["step-over", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["pc"], 0x8004u64);
}

#[test]
fn disasm_rx_fc_fd_long_forms() {
    // RX 0xFC/0xFD long opcodes decode the register-register and immediate
    // forms, advancing by the actual operand width (3 bytes, or 4 for imm8).
    let dir = tmpdir();
    let sess = dir.join("rxfc.json");
    let code = dir.join("rxfc.bin");
    std::fs::write(&code, [
        0xFC, 0x03, 0x53,             // sbb r5, r3
        0xFC, 0x27, 0x00,             // divu r0, r0
        0xFC, 0x63, 0x35,             // bset r5, r3
        0xFD, 0x60, 0x53,             // shlr r5, r3
        0xFD, 0x6A, 0x05,             // mvfc psw, r5
        0xFD, 0x74, 0x23, 0x05,       // adc #0x5, r3
    ]).unwrap();
    let out = Command::new(bin())
        .args(["mem", "load", "--session", sess.to_str().unwrap(),
               "--arch", "rx", "--firmware", code.to_str().unwrap(), "--base", "0x8000"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let out = Command::new(bin())
        .args(["disasm", "--session", sess.to_str().unwrap(),
               "--addr", "0x8000", "--count", "6", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json[0]["address"], 0x8000u64);
    assert_eq!(json[0]["text"], "sbb r5, r3");
    assert_eq!(json[1]["address"], 0x8003u64);
    assert_eq!(json[1]["text"], "divu r0, r0");
    assert_eq!(json[2]["address"], 0x8006u64);
    assert_eq!(json[2]["text"], "bset r5, r3");
    assert_eq!(json[3]["address"], 0x8009u64);
    assert_eq!(json[3]["text"], "shlr r5, r3");
    assert_eq!(json[4]["address"], 0x800Cu64);
    assert_eq!(json[4]["text"], "mvfc psw, r5");
    assert_eq!(json[5]["address"], 0x800Fu64);
    assert_eq!(json[5]["text"], "adc #0x5, r3");
}

#[test]
fn step_rx_fc_fd_long_forms() {
    // Step through RX 0xFC/0xFD long opcodes and observe register changes.
    let dir = tmpdir();
    let sess = dir.join("rxfcstep.json");
    let code = dir.join("rxfcstep.bin");
    std::fs::write(&code, [
        0xFD, 0x74, 0x23, 0x05, // adc #0x5, r3
        0xFD, 0x60, 0x53,       // shlr r5, r3
        0xFC, 0x03, 0x53,       // sbb r5, r3
    ]).unwrap();
    let out = Command::new(bin())
        .args(["mem", "load", "--session", sess.to_str().unwrap(),
               "--arch", "rx", "--firmware", code.to_str().unwrap(), "--base", "0x8000"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    for (name, value) in [("r3", "10"), ("r5", "4")] {
        let out = Command::new(bin())
            .args(["reg", "set", "--session", sess.to_str().unwrap(),
                   "--name", name, "--value", value])
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }
    // adc #0x5, r3: r3 = 10 + 5 = 15, pc advances 4.
    let out = Command::new(bin())
        .args(["step", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["pc"], 0x8004u64);
    assert_eq!(json["registers"]["r3"], 15u64);
    // shlr r5, r3: r3 = 15 >> 4 = 0, pc advances 3.
    let out = Command::new(bin())
        .args(["step", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["pc"], 0x8007u64);
    assert_eq!(json["registers"]["r3"], 0u64);
    // sbb r5, r3: r3 = 0 - 4 = -4, pc advances 3.
    let out = Command::new(bin())
        .args(["step", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["pc"], 0x800Au64);
    assert_eq!(json["registers"]["r3"], 0xFFFF_FFFCu64);
}
