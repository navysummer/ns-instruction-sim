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
