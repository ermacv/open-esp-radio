use super::*;

#[test]
fn declaration_grammar_preserves_abi_and_special_adapters() {
    let declaration: Declaration = syn::parse_str(
        "pub fn sample(a: i8, output: &mut [u32; 4]) -> i32 => project(run(a, output));",
    )
    .unwrap();
    let entry = declaration.entry();
    assert_eq!(entry.arguments.len(), 2);
    assert_eq!(entry.arguments[0].rust_type, "i8");
    let expanded: syn::ItemFn = syn::parse2(declaration.expand()).unwrap();
    assert_eq!(expanded.sig.abi.unwrap().name.unwrap().value(), "C");
    let naked: Declaration = syn::parse_str(
        "#[unsafe(naked)] pub unsafe fn jump(entry: u32) { core::arch::naked_asm!(\"jr a0\"); }",
    )
    .unwrap();
    assert!(!naked.expand().to_string().contains("inline"));
    for bad in [
        "pub async fn f() {}",
        "pub fn f<T>(a: T) {}",
        "#[cfg(feature = \"x\")] pub fn f() {}",
        "pub extern \"C\" fn f() {}",
        "pub fn f((a,b): (u32,u32)) {}",
    ] {
        assert!(syn::parse_str::<Declaration>(bad).is_err(), "{bad}");
    }
}

#[test]
fn collection_tracks_additions_relocation_and_duplicate_or_unregistered_entries() {
    let temporary = tempfile::tempdir().unwrap();
    let original = temporary.path().join("original");
    std::fs::create_dir(&original).unwrap();
    std::fs::write(
        original.join("lib.rs"),
        "mod child; probe! { pub fn first() => 1; }",
    )
    .unwrap();
    std::fs::write(original.join("child.rs"), "probe! { pub fn second() {} }").unwrap();
    let (catalog, files) = collect(&original.join("lib.rs"), "fixture").unwrap();
    assert_eq!(
        catalog
            .entries
            .iter()
            .map(|e| e.symbol.as_str())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    assert_eq!(files.len(), 2);
    let moved = temporary.path().join("relocated with spaces");
    std::fs::rename(&original, &moved).unwrap();
    std::fs::write(
        moved.join("child.rs"),
        "probe! { pub fn added() {} } probe! { pub fn second() {} }",
    )
    .unwrap();
    let (catalog, files) = collect(&moved.join("lib.rs"), "fixture").unwrap();
    assert_eq!(catalog.entries.len(), 3);
    assert!(files.iter().all(|p| p.starts_with(&moved)));
    build(&moved.join("lib.rs"), "fixture", temporary.path()).unwrap();
    assert!(
        std::fs::read_to_string(temporary.path().join("probe_catalog.rs"))
            .unwrap()
            .contains("added")
    );
    std::fs::write(moved.join("child.rs"), "probe! { pub fn first() {} }").unwrap();
    assert!(collect(&moved.join("lib.rs"), "fixture").is_err());
    std::fs::write(
        moved.join("child.rs"),
        "pub extern \"C\" fn open_missing() {}",
    )
    .unwrap();
    assert!(collect(&moved.join("lib.rs"), "fixture").is_err());
    std::fs::write(moved.join("lib.rs"), "#[cfg(feature = \"x\")] mod child;").unwrap();
    assert!(collect(&moved.join("lib.rs"), "fixture").is_err());
}

fn fixture(catalog: &[u8], symbol_name: &str, kind: object::SectionKind) -> Vec<u8> {
    use object::write::{Object, Symbol, SymbolSection};
    use object::{Object as _, ObjectSection as _};
    let mut object = Object::new(
        object::BinaryFormat::Elf,
        object::Architecture::Riscv32,
        object::Endianness::Little,
    );
    let section = object.add_section(Vec::new(), b".entry".to_vec(), kind);
    object.append_section_data(section, &[0x67, 0x80, 0, 0], 4);
    object.add_symbol(Symbol {
        name: symbol_name.as_bytes().to_vec(),
        value: 0,
        size: 4,
        kind: object::SymbolKind::Text,
        scope: object::SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(section),
        flags: object::SymbolFlags::None,
    });
    let section = object.add_section(
        Vec::new(),
        SECTION.as_bytes().to_vec(),
        object::SectionKind::ReadOnlyData,
    );
    object.append_section_data(section, catalog, 1);
    let mut bytes = object.write().unwrap();
    // Supply an executable PT_LOAD for the text bytes in this synthetic ELF.
    let (offset, size) = object::File::parse(bytes.as_slice())
        .unwrap()
        .section_by_name(".entry")
        .unwrap()
        .file_range()
        .unwrap();
    let header = bytes.len() as u32;
    bytes[16..18].copy_from_slice(&object::elf::ET_EXEC.to_le_bytes());
    bytes[28..32].copy_from_slice(&header.to_le_bytes());
    bytes[42..44].copy_from_slice(&32u16.to_le_bytes());
    bytes[44..46].copy_from_slice(&1u16.to_le_bytes());
    for word in [1, offset as u32, 0, 0, size as u32, size as u32, 5, 1] {
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    bytes
}

#[test]
fn elf_validation_rejects_missing_noncode_wrong_image_and_malformed_catalogs() {
    let declaration: Declaration =
        syn::parse_str("pub fn example(value: i32) -> u32 => value as u32;").unwrap();
    let mut catalog = Catalog {
        schema: SCHEMA,
        image: "fixture".into(),
        entries: vec![declaration.entry()],
    };
    let bytes = serde_json::to_vec(&catalog).unwrap();
    assert_eq!(
        validate_elf(
            &fixture(&bytes, "example", object::SectionKind::Text),
            "fixture"
        )
        .unwrap()
        .entries
        .len(),
        1
    );
    assert!(
        validate_elf(
            &fixture(&bytes, "absent", object::SectionKind::Text),
            "fixture"
        )
        .is_err()
    );
    assert!(
        validate_elf(
            &fixture(&bytes, "example", object::SectionKind::Data),
            "fixture"
        )
        .is_err()
    );
    assert!(
        validate_elf(
            &fixture(&bytes, "example", object::SectionKind::Text),
            "another-image"
        )
        .is_err()
    );
    assert!(
        validate_elf(
            &fixture(b"not-json", "example", object::SectionKind::Text),
            "fixture"
        )
        .is_err()
    );
    let mut non_executable = fixture(&bytes, "example", object::SectionKind::Text);
    non_executable[16..18].copy_from_slice(&object::elf::ET_REL.to_le_bytes());
    assert!(validate_elf(&non_executable, "fixture").is_err());
    let mut unmapped = fixture(&bytes, "example", object::SectionKind::Text);
    unmapped[44..46].copy_from_slice(&0u16.to_le_bytes());
    assert!(validate_elf(&unmapped, "fixture").is_err());
    catalog.entries.push(declaration.entry());
    assert!(
        validate_elf(
            &fixture(
                &serde_json::to_vec(&catalog).unwrap(),
                "example",
                object::SectionKind::Text
            ),
            "fixture"
        )
        .is_err()
    );
    catalog.entries.pop();
    catalog.schema += 1;
    assert!(
        validate_elf(
            &fixture(
                &serde_json::to_vec(&catalog).unwrap(),
                "example",
                object::SectionKind::Text
            ),
            "fixture"
        )
        .is_err()
    );
}
