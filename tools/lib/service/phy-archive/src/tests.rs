use super::*;

/// The crates a chip's PHY may call into, as the gate passes them.
fn sources() -> Vec<String> {
    ["oer_chip_a_hal", "oer_chip_a_pac", "oer_chip_a_pac_raw"]
        .map(String::from)
        .to_vec()
}

fn context() -> Checkout {
    Checkout::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")).unwrap()
}

fn compile_rlib(source: &str, bitcode: bool) -> (tempfile::TempDir, PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let input = temporary.path().join("fixture.rs");
    let output = temporary.path().join("fixture.rlib");
    std::fs::write(&input, source).unwrap();
    let ctx = context();
    let mut command = ctx.command(oer_toolchain::program(oer_toolchain::Tool::Rustc).unwrap());
    command
        .args([
            "--edition=2024",
            "--crate-type=rlib",
            "--crate-name=phy_fixture",
            "-Cpanic=abort",
            "-Copt-level=1",
            "-Ccodegen-units=1",
        ])
        .arg(&input)
        .arg("-o")
        .arg(&output);
    if bitcode {
        command.arg("-Clinker-plugin-lto");
    }
    process::capture(&mut command).unwrap();
    (temporary, output)
}

const SOURCE_ONLY: &str = r#"
#![no_std]
unsafe extern "C" { fn memset(destination: *mut u8, value: i32, size: usize) -> *mut u8; }
#[unsafe(no_mangle)]
pub unsafe extern "C" fn public_model(destination: *mut u8, size: usize) {
    unsafe { memset(destination, 0, size); }
}
"#;

#[test]
fn native_elf_archive_accepts_compiled_source_only_support() {
    let (_temporary, path) = compile_rlib(SOURCE_ONLY, false);
    let bytes = std::fs::read(&path).unwrap();
    assert!(
        oer_elf::members(&bytes)
            .unwrap()
            .iter()
            .any(|(name, data)| {
                name != "lib.rmeta" && name != "lib.rmeta-link" && data.starts_with(b"\x7fELF")
            })
    );
    audit_phy(&context(), &path, &sources()).unwrap();
}

#[test]
fn llvm_bitcode_archive_uses_the_matching_toolchain() {
    let (_temporary, path) = compile_rlib(SOURCE_ONLY, true);
    let bytes = std::fs::read(&path).unwrap();
    assert!(
        oer_elf::members(&bytes)
            .unwrap()
            .iter()
            .any(|(_, data)| is_bitcode(data))
    );
    audit_phy(&context(), &path, &sources()).unwrap();
}

#[test]
fn compiled_vendor_abi_is_rejected_even_when_defined_locally() {
    let (_temporary, path) = compile_rlib(
        r#"
#![no_std]
#[unsafe(no_mangle)]
pub extern "C" fn register_chipv7_phy() {}
"#,
        false,
    );
    let error = audit_phy(&context(), &path, &sources())
        .unwrap_err()
        .to_string();
    assert!(error.contains("radio ROM/vendor ABI symbol"), "{error}");
}

#[test]
fn compiled_unreviewed_external_is_rejected() {
    let (_temporary, path) = compile_rlib(
        r#"
#![no_std]
unsafe extern "C" { fn vendor_radio_start(); }
#[unsafe(no_mangle)]
pub unsafe extern "C" fn public_model() { unsafe { vendor_radio_start(); } }
"#,
        false,
    );
    let error = audit_phy(&context(), &path, &sources())
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("unexpected external symbol") && error.contains("vendor_radio_start"),
        "{error}"
    );
}

#[test]
fn defined_archive_members_resolve_internal_undefined_references() {
    let mut symbols = Symbols::default();
    symbols.defined.insert("internal_worker".into());
    symbols.undefined.insert("internal_worker".into());
    symbols.undefined.insert("memcpy".into());
    check_symbols(&symbols, &sources()).unwrap();
}

#[test]
fn source_only_symbol_policy_preserves_qualified_trait_and_core_boundaries() {
    for allowed in [
        "oer_chip_a_hal::radio::start",
        "oer_chip_a_pac::Wlan::read",
        "oer_chip_a_pac_raw::Register::read",
        "<oer_chip_a_hal::owner::Radio as external::Trait>::start",
        "<other::Type as core::fmt::Debug>::fmt",
        "core::fmt::write",
        "core::panicking::panic_fmt",
        "core::slice::len_mismatch_fail",
        "__divdi3",
        "__udivdi3",
        "memcmp",
        "memcpy",
        "memmove",
        "memset",
    ] {
        assert!(allowed_external(allowed, &sources()), "{allowed}");
    }
    for rejected in [
        "oer_chip_a_hal_fake::start",
        "external::oer_chip_a_hal::start",
        "<external::Type as external::Trait>::fmt",
        "core::slice::unexpected",
        "vendor::panic_fmt",
        "core::fmt_fake::write",
        "malloc",
        "__muldi3",
    ] {
        assert!(!allowed_external(rejected, &sources()), "{rejected}");
    }
}

#[test]
fn exact_forbidden_abi_names_and_prefixes_cover_defined_and_undefined_symbols() {
    for name in [
        "phy_wifi_get_tx_gain",
        "register_chipv7_phy",
        "g_phyFuns",
        "phy_param",
        "esp_wifi_start",
        "pp_worker",
        "net80211_input",
    ] {
        for defined in [false, true] {
            let mut symbols = Symbols::default();
            if defined {
                symbols.defined.insert(name.into());
            } else {
                symbols.undefined.insert(name.into());
            }
            assert!(
                check_symbols(&symbols, &sources())
                    .unwrap_err()
                    .to_string()
                    .contains("radio ROM/vendor ABI")
            );
        }
    }
    for name in ["phy_param_model", "my_esp_wifi_start", "pp", "net80211"] {
        assert!(!forbidden_radio_symbol(name));
    }
}

fn archive_member(name: &str, data: &[u8]) -> Vec<u8> {
    let mut archive = b"!<arch>\n".to_vec();
    let name = format!("{name}/");
    let header = format!(
        "{name:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
        0,
        0,
        0,
        "100644",
        data.len()
    );
    assert_eq!(header.len(), 60);
    archive.extend_from_slice(header.as_bytes());
    archive.extend_from_slice(data);
    if !data.len().is_multiple_of(2) {
        archive.push(b'\n');
    }
    archive
}

#[test]
fn empty_unknown_truncated_and_thin_archives_fail_closed() {
    for bytes in [
        b"!<arch>\n".to_vec(),
        b"!<thin>\n".to_vec(),
        b"!<arch>\ntruncated".to_vec(),
        archive_member("unexpected.o", b"not an object"),
    ] {
        assert!(archive_symbols(&context(), &bytes).is_err());
    }
}

#[test]
fn corrupt_bitcode_tool_failure_cannot_become_an_empty_symbol_set() {
    let bytes = archive_member("broken.o", b"BC\xc0\xdeinvalid");
    let error = match archive_symbols(&context(), &bytes) {
        Ok(_) => panic!("corrupt bitcode unexpectedly accepted"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("cannot inspect bitcode member"), "{error}");
}

#[test]
fn llvm_symbol_output_rejects_headings_and_malformed_records() {
    for text in ["member.bc:\n", "symbol U 0\n", " a_symbol\n"] {
        assert!(parse_nm_names(text, &mut BTreeSet::new()).is_err());
    }
    let mut symbols = BTreeSet::new();
    parse_nm_names("memcpy\n\nmemcpy\nmemset\n", &mut symbols).unwrap();
    assert_eq!(
        symbols,
        ["memcpy".into(), "memset".into()].into_iter().collect()
    );
}

#[test]
fn an_elf_without_a_symbol_table_cannot_hide_external_references() {
    // A valid ELF64 relocatable header with no sections, like a fully stripped
    // object. Parsing the container alone cannot establish its symbol policy.
    let mut elf = [0u8; 64];
    elf[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
    elf[16..18].copy_from_slice(&1u16.to_le_bytes());
    elf[18..20].copy_from_slice(&243u16.to_le_bytes());
    elf[20..24].copy_from_slice(&1u32.to_le_bytes());
    elf[52..54].copy_from_slice(&64u16.to_le_bytes());
    elf[58..60].copy_from_slice(&64u16.to_le_bytes());
    let error = elf_symbols(&elf, &mut Symbols::default(), true)
        .unwrap_err()
        .to_string();
    assert!(error.contains("no symbol table"), "{error}");
}
