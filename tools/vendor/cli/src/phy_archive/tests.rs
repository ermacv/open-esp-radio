use super::*;

/// The crates a chip's PHY may call into, as the gate passes them.
fn sources() -> Vec<String> {
    ["oer_chip_a_hal", "oer_chip_a_pac", "oer_chip_a_pac_raw"]
        .map(String::from)
        .to_vec()
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
