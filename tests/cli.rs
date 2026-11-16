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
    let out = Command::new(bin())
        .args(["watchdog", "feed", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["fed"], true);
    let out = Command::new(bin())
        .args(["watchdog", "status", "--session", sess.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["enabled"], true);
    assert_eq!(json["timeoutCycles"], 500u64);
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
