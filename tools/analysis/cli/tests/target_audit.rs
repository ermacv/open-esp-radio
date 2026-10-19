use std::{fs, process::Command};
fn elf(code: &[u32]) -> Vec<u8> {
    let names = b"\0.text\0.shstrtab\0";
    let names_offset = 256 + code.len() * 4;
    let shoff = (names_offset + names.len()).next_multiple_of(4);
    let mut bytes = vec![0; shoff + 120];
    bytes[names_offset..names_offset + names.len()].copy_from_slice(names);
    bytes[..7].copy_from_slice(b"\x7fELF\x01\x01\x01");
    for (offset, value) in [
        (16, 2u16),
        (18, 243),
        (40, 52),
        (42, 32),
        (44, 1),
        (46, 40),
        (48, 3),
        (50, 2),
    ] {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    for (offset, value) in [
        (20, 1u32),
        (24, 0x1000),
        (28, 52),
        (32, shoff as u32),
        (52, 1),
        (56, 256),
        (60, 0x1000),
        (64, 0x1000),
        (68, (code.len() * 4) as u32),
        (72, (code.len() * 4) as u32),
        (76, 5),
        (80, 4),
        (shoff + 40, 1),
        (shoff + 44, 1),
        (shoff + 48, 6),
        (shoff + 52, 0x1000),
        (shoff + 56, 256),
        (shoff + 60, (code.len() * 4) as u32),
        (shoff + 72, 4),
        (shoff + 80, 7),
        (shoff + 84, 3),
        (shoff + 96, names_offset as u32),
        (shoff + 100, names.len() as u32),
        (shoff + 112, 1),
    ] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    for (i, op) in code.iter().enumerate() {
        bytes[256 + i * 4..260 + i * 4].copy_from_slice(&op.to_le_bytes());
    }
    bytes
}
fn run(bytes: &[u8], extra: &[&str]) -> std::process::Output {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("input.elf");
    fs::write(&path, bytes).unwrap();
    Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["--format", "json", "audit-targets", "--artifact"])
        .arg(path)
        .args(["--forbid", "rom=0x3000..0x3010"])
        .args(extra)
        .output()
        .unwrap()
}
#[test]
fn scans_symbol_less_code_and_resolves_local_jalr() {
    let clean = run(&elf(&[0x00000013, 0x00008067]), &[]);
    assert!(
        clean.status.success(),
        "{}",
        String::from_utf8_lossy(&clean.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&clean.stdout).unwrap();
    assert_eq!(result["summary"]["unresolved_indirect"], 1);
    assert_eq!(result["verdict"], "pass");
    let bad = run(&elf(&[0x000032b7, 0x00028067]), &[]);
    assert!(!bad.status.success());
    let result: serde_json::Value = serde_json::from_slice(&bad.stdout).unwrap();
    assert_eq!(result["summary"]["forbidden_targets"], 1);
    assert_eq!(result["records"][0]["kind"], "forbidden");
    assert_eq!(result["verdict"], "fail");
}
#[test]
fn direct_jump_and_unknown_clobber_have_distinct_coverage() {
    // JAL x0,+8192; target 0x3000, no function symbol is needed.
    let bad = run(&elf(&[0x0000206f]), &[]);
    assert!(!bad.status.success());
    let result: serde_json::Value = serde_json::from_slice(&bad.stdout).unwrap();
    assert_eq!(result["summary"]["forbidden_targets"], 1);
    // A decoded single-precision fadd.s has no integer effect: t0 keeps its
    // constant and the jump reaches the forbidden range.
    let float = run(&elf(&[0x000032b7, 0x00000053, 0x00028067]), &[]);
    assert!(!float.status.success());
    let result: serde_json::Value = serde_json::from_slice(&float.stdout).unwrap();
    assert_eq!(result["summary"]["forbidden_targets"], 1);
    assert_eq!(result["summary"]["unsupported_non_control"], 0);
    // An undecoded double-precision fadd.d cannot be an integer transfer but
    // must erase constants.
    let unknown = run(&elf(&[0x000032b7, 0x02000053, 0x00028067]), &[]);
    assert!(
        unknown.status.success(),
        "{}",
        String::from_utf8_lossy(&unknown.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&unknown.stdout).unwrap();
    assert_eq!(result["summary"]["unsupported_non_control"], 1);
    assert_eq!(result["summary"]["unresolved_indirect"], 1);
    let gap = run(&elf(&[0x0000001f]), &[]);
    assert!(!gap.status.success());
}
#[test]
fn invalid_input_and_limits_never_report_a_clean_audit() {
    for bytes in [vec![0; 64], elf(&[0x13])[..100].to_vec()] {
        let output = run(&bytes, &[]);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
    let mut no_sections = elf(&[0x13]);
    no_sections[32..36].fill(0);
    no_sections[48..52].fill(0);
    assert!(!run(&no_sections, &[]).status.success());
    let limited = run(&elf(&[0x13; 100]), &["--max-work-units", "1"]);
    assert!(!limited.status.success());
    assert!(limited.stdout.is_empty());
}

#[test]
fn csr_clobber_and_trap_return_do_not_claim_static_targets() {
    let result = run(&elf(&[0x000032b7, 0x340022f3, 0x00028067, 0x30200073]), &[]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(result["summary"]["unresolved_indirect"], 2);
    assert_eq!(result["summary"]["forbidden_targets"], 0);
    // Reserved SYSTEM function is not silently treated as an ordinary CSR access.
    let result = run(&elf(&[0x00004073]), &[]);
    assert!(!result.status.success());
    let result: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(result["summary"]["coverage_gaps"], 1);
}

fn with_mappings(mut bytes: Vec<u8>, mappings: &[(&str, u32)]) -> Vec<u8> {
    let shoff = u32::from_le_bytes(bytes[32..36].try_into().unwrap()) as usize;
    bytes.resize(shoff + 200, 0);
    bytes[48..50].copy_from_slice(&5u16.to_le_bytes());
    let strings = bytes.len();
    bytes.push(0);
    let mut names = Vec::new();
    for (name, _) in mappings {
        names.push((bytes.len() - strings) as u32);
        bytes.extend_from_slice(name.as_bytes());
        bytes.push(0);
    }
    let strings_len = bytes.len() - strings;
    bytes.resize(bytes.len().next_multiple_of(4), 0);
    let symbols = bytes.len();
    bytes.resize(symbols + (mappings.len() + 1) * 16, 0);
    for (i, ((_, address), name)) in mappings.iter().zip(names).enumerate() {
        let at = symbols + (i + 1) * 16;
        bytes[at..at + 4].copy_from_slice(&name.to_le_bytes());
        bytes[at + 4..at + 8].copy_from_slice(&address.to_le_bytes());
        bytes[at + 14..at + 16].copy_from_slice(&1u16.to_le_bytes());
    }
    for (offset, value) in [
        (shoff + 124, 2u32),
        (shoff + 136, symbols as u32),
        (shoff + 140, ((mappings.len() + 1) * 16) as u32),
        (shoff + 144, 4),
        (shoff + 148, (mappings.len() + 1) as u32),
        (shoff + 152, 4),
        (shoff + 156, 16),
        (shoff + 164, 3),
        (shoff + 176, strings as u32),
        (shoff + 180, strings_len as u32),
        (shoff + 192, 1),
    ] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes
}
#[test]
fn mapping_symbols_skip_data_and_reset_local_facts() {
    // Constant established before embedded data must not survive into the next code run.
    let bytes = with_mappings(
        elf(&[0x000032b7, 0x501fce26, 0x00028067]),
        &[
            ("$x", 0x1000),
            ("$d", 0x1004),
            ("$xrv32i2p1_m2p0_c2p0.1", 0x1008),
        ],
    );
    let output = run(&bytes, &[]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["summary"]["embedded_data_bytes"], 4);
    assert_eq!(result["summary"]["unresolved_indirect"], 1);
    assert_eq!(result["summary"]["instructions"], 2);
    // Scanning resumes after data, even without any function symbols.
    let output = run(
        &with_mappings(
            elf(&[0x501fce26, 0x0000206f]),
            &[("$d", 0x1000), ("$x", 0x1004)],
        ),
        &[],
    );
    assert!(!output.status.success());
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["summary"]["forbidden_targets"], 1);
}
#[test]
fn invalid_mapping_symbols_fail_closed() {
    for mappings in [
        vec![("$d", 0x1000), ("$x", 0x1000)],
        vec![("$d", 0x0ffe)],
        vec![("$x", 0x1001)],
        vec![("$d", 0x1006)],
        vec![("$xrv64i2p1", 0x1000)],
    ] {
        let output = run(&with_mappings(elf(&[0x13]), &mappings), &[]);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
    // An ordinary label cannot suppress unsupported code.
    let output = run(&with_mappings(elf(&[0x0000001f]), &[("data", 0x1000)]), &[]);
    assert!(!output.status.success());
}
