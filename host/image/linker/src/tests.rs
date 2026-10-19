use std::ffi::OsString;

use object::{
    Architecture, BinaryFormat, Endianness, SectionKind, SymbolFlags, SymbolKind, SymbolScope,
    write::{Object as WriteObject, Symbol, SymbolSection},
};

use super::*;

/// A RISC-V object whose zeroed-region sections are `PROGBITS`, as LLVM
/// emits named sections: one with a non-zero initializer, one with a
/// relocation and one of zero bytes.
fn object_with_initialized_zeroed_sections() -> Vec<u8> {
    let mut object = WriteObject::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let initialized = object.add_section(
        Vec::new(),
        b".psram.bss.data_in_zeroed".to_vec(),
        SectionKind::Data,
    );
    object.append_section_data(initialized, &[1, 2, 3, 4], 4);
    object.add_symbol(Symbol {
        name: b"NOT_ZERO".to_vec(),
        value: 0,
        size: 4,
        kind: SymbolKind::Data,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(initialized),
        flags: SymbolFlags::None,
    });
    let pointer = object.add_section(
        Vec::new(),
        b".dma.bss.self_pointer".to_vec(),
        SectionKind::Data,
    );
    object.append_section_data(pointer, &[0; 4], 4);
    let target = object.add_symbol(Symbol {
        name: b"SELF_POINTER".to_vec(),
        value: 0,
        size: 4,
        kind: SymbolKind::Data,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(pointer),
        flags: SymbolFlags::None,
    });
    object
        .add_relocation(
            pointer,
            object::write::Relocation {
                offset: 0,
                symbol: target,
                addend: 0,
                flags: object::RelocationFlags::Elf {
                    r_type: object::elf::R_RISCV_32,
                },
            },
        )
        .unwrap();
    let zero = object.add_section(
        Vec::new(),
        b".critical.bss.zero".to_vec(),
        SectionKind::Data,
    );
    object.append_section_data(zero, &[0; 64], 4);
    object.write().unwrap()
}

fn object_without_initialized_zeroed_sections() -> Vec<u8> {
    let mut object = WriteObject::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let zeroed = object.add_section(
        Vec::new(),
        b".critical.bss.zeroed".to_vec(),
        SectionKind::UninitializedData,
    );
    object.append_section_bss(zeroed, 16, 4);
    let zero = object.add_section(Vec::new(), b".psram.bss.zero".to_vec(), SectionKind::Data);
    object.append_section_data(zero, &[0; 32], 4);
    // Not a zeroed region: initialized data and a persistent region pass.
    let data = object.add_section(Vec::new(), b".data.value".to_vec(), SectionKind::Data);
    object.append_section_data(data, &[9; 8], 4);
    let noinit = object.add_section(
        Vec::new(),
        b".psram.noinit.flags".to_vec(),
        SectionKind::Data,
    );
    object.append_section_data(noinit, &[7; 8], 4);
    object.write().unwrap()
}

/// A GNU `ar` archive of `members`.
fn archive(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut archive = b"!<arch>\n".to_vec();
    for (name, data) in members {
        let header = format!(
            "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
            format!("{name}/"),
            0,
            0,
            0,
            644,
            data.len()
        );
        archive.extend_from_slice(header.as_bytes());
        archive.extend_from_slice(data);
        if data.len() % 2 == 1 {
            archive.push(b'\n');
        }
    }
    archive
}

#[test]
fn an_initializer_in_a_zeroed_region_fails_with_its_file_and_symbols() {
    let temporary = tempdir();
    let directory = temporary.path();
    let object = directory.join("bad.o");
    fs::write(&object, object_with_initialized_zeroed_sections()).unwrap();
    let violations = check_inputs(&[OsString::from(&object)], &zeroed()).unwrap();
    assert_eq!(violations.len(), 2, "{violations:?}");
    assert_eq!(violations[0].section, ".psram.bss.data_in_zeroed");
    assert_eq!(violations[0].nonzero_bytes, 4);
    assert_eq!(violations[0].symbols, ["NOT_ZERO"]);
    assert!(violations[0].file.ends_with("bad.o"));
    // A zero-byte initializer that holds an address is still one.
    assert_eq!(violations[1].section, ".dma.bss.self_pointer");
    assert_eq!(violations[1].nonzero_bytes, 0);
    assert_eq!(violations[1].relocations, 1);
}

#[test]
fn zero_initializers_and_sections_outside_zeroed_regions_pass() {
    let temporary = tempdir();
    let directory = temporary.path();
    let object = directory.join("good.o");
    fs::write(&object, object_without_initialized_zeroed_sections()).unwrap();
    assert_eq!(
        check_inputs(&[OsString::from(&object)], &zeroed()).unwrap(),
        []
    );
}

#[test]
fn archive_members_found_on_the_library_path_are_checked() {
    let temporary = tempdir();
    let directory = temporary.path();
    let bad = object_with_initialized_zeroed_sections();
    let good = object_without_initialized_zeroed_sections();
    fs::write(
        directory.join("libvendor.a"),
        archive(&[("good.o", &good), ("bad.o", &bad)]),
    )
    .unwrap();
    let arguments = [
        OsString::from("-L"),
        OsString::from(&directory),
        OsString::from("-lvendor"),
        OsString::from("-o"),
        OsString::from(directory.join("image.elf")),
    ];
    let violations = check_inputs(&arguments, &zeroed()).unwrap();
    // Both of the bad member's sections, none of the good member's.
    assert_eq!(violations.len(), 2, "{violations:?}");
    assert!(
        violations
            .iter()
            .all(|violation| violation.file.ends_with("libvendor.a(bad.o)")),
        "{violations:?}"
    );
}

#[test]
fn response_files_expand_with_gnu_quoting_and_nesting() {
    let temporary = tempdir();
    let directory = temporary.path();
    let inner = directory.join("inner.rsp");
    fs::write(&inner, "-Tlink.x\n").unwrap();
    let outer = directory.join("outer.rsp");
    fs::write(
        &outer,
        format!("-L\n/tmp/a\\ b\n\"quoted arg\"\n@{}\n", inner.display()),
    )
    .unwrap();
    let expanded =
        expand_response_files(vec![OsString::from(format!("@{}", outer.display()))]).unwrap();
    assert_eq!(expanded, ["-L", "/tmp/a b", "quoted arg", "-Tlink.x"]);
}

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// A `.bss.x` that LLVM emitted as `PROGBITS` with an initializer.
fn object_with_initialized_bss() -> Vec<u8> {
    let mut object = WriteObject::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let section = object.add_section(Vec::new(), b".bss.x".to_vec(), SectionKind::Data);
    object.append_section_data(section, &[0x5a; 4], 4);
    object.write().unwrap()
}

#[test]
fn an_initialized_bss_section_fails_the_link_before_the_real_linker_runs() {
    let temporary = tempdir();
    let directory = temporary.path();
    let object = directory.join("x.o");
    fs::write(&object, object_with_initialized_bss()).unwrap();
    let output = directory.join("image.elf");
    // `rust-lld` would write the output; a refused link never reaches it.
    let status = crate::run(
        vec![
            OsString::from(&object),
            OsString::from("-o"),
            OsString::from(&output),
        ],
        &zeroed(),
    );
    assert_eq!(status, 1);
    assert!(!output.exists());
}

fn zeroed() -> Vec<String> {
    [
        ".psram.bss",
        ".psram.bss.*",
        ".bss",
        ".bss.*",
        ".dma.bss",
        ".dma.bss.*",
    ]
    .map(String::from)
    .to_vec()
}
