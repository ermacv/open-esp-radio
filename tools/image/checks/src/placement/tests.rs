use super::*;
use object::write::{Object as Writer, StandardSection, Symbol, SymbolSection};
use object::{Architecture, BinaryFormat, Endianness, SymbolFlags, SymbolKind, SymbolScope};

/// `csrrw sp, mscratch, sp`.
const SWAP: u32 = 0x3401_1173;
/// `addi sp, sp, -16`.
const PUSH: u32 = 0xff01_0113;

/// An object whose `.text` holds the trap entry and the 47 hart-0 interrupt
/// entries, each beginning with `first(entry)`.
fn entries(first: impl Fn(usize) -> u32) -> Vec<u8> {
    let mut writer = Writer::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = writer.section_id(StandardSection::Text);
    let mut names = vec!["_start_trap".to_owned()];
    names.extend((1..=47).map(|number| format!("_runtime_irq_h0_l{number}")));
    for (index, name) in names.iter().enumerate() {
        let offset = writer.append_section_data(text, &first(index).to_le_bytes(), 4);
        writer.add_symbol(Symbol {
            name: name.as_bytes().to_vec(),
            value: offset,
            size: 4,
            kind: SymbolKind::Text,
            scope: SymbolScope::Linkage,
            weak: false,
            section: SymbolSection::Section(text),
            flags: SymbolFlags::None,
        });
    }
    writer.write().unwrap()
}

/// The entries of [`entries`], as a placement contract names them.
fn swapping() -> Vec<Symbols> {
    vec![
        Symbols {
            name: "_start_trap".into(),
            numbers: None,
        },
        Symbols {
            name: "_runtime_irq_h0_l{n}".into(),
            numbers: Some([1, 47]),
        },
    ]
}

fn audit_entries(bytes: &[u8]) -> Result<()> {
    let elf = oer_analysis_elf::Elf::parse(bytes).unwrap();
    audit_stack_swap_entries(&elf, &elf.addresses(), &swapping())
}

#[test]
fn every_psram_entry_swaps_to_the_interrupt_stack_first() {
    audit_entries(&entries(|_| SWAP)).unwrap();
}

#[test]
fn an_entry_that_touches_the_interrupted_stack_first_is_refused() {
    let error = audit_entries(&entries(|index| if index == 5 { PUSH } else { SWAP })).unwrap_err();
    assert!(error.to_string().contains("_runtime_irq_h0_l5"), "{error}");
}

const SRAM: Region = match Region::new(0x2f00_0000, 0x1000) {
    Ok(region) => region,
    Err(_) => panic!("valid region"),
};
const ROM: Region = match Region::new(0x2f80_0000, 0x1000) {
    Ok(region) => region,
    Err(_) => panic!("valid region"),
};

#[test]
fn an_interrupt_handler_in_sram_or_rom_holds_and_one_in_psram_is_named() {
    let names = ["sram".to_owned(), "rom".to_owned()];
    let handlers = [(0, 0x2f00_0010), (3, 0x2f80_0100), (7, 0x5001_0000)];
    assert_eq!(
        handler_violations(&handlers, &[SRAM, ROM], &names),
        ["interrupt table entry 7's handler 0x50010000 is not in sram or rom"]
    );
    // The ROM is a permitted region only where the contract names it.
    assert_eq!(
        handler_violations(&handlers[..2], &[SRAM], &names[..1]).len(),
        1
    );
}

#[test]
fn a_critical_section_outside_sram_is_named_and_an_empty_one_is_not() {
    let sections = [
        (".critical.text", 0x2f00_0000, 0x40),
        (".critical.data", 0x5000_0000, 0x10),
        (".critical.bss", 0x5000_0100, 0),
        (".text", 0x5000_0200, 0x100),
    ];
    assert_eq!(
        section_violations(sections.into_iter(), ".critical", (SRAM, "sram")),
        ["section `.critical.data` (0x50000000+0x10) is not in sram"]
    );
}

#[test]
fn a_mangled_rust_symbol_is_found_by_its_demangled_name() {
    let mut writer = Writer::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = writer.section_id(StandardSection::Text);
    let offset = writer.append_section_data(text, &SWAP.to_le_bytes(), 4);
    writer.add_symbol(Symbol {
        name: b"_RNvCs9wFQrvczXsK_7___rustc17rust_begin_unwind".to_vec(),
        value: offset,
        size: 4,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(text),
        flags: SymbolFlags::None,
    });
    let bytes = writer.write().unwrap();
    let elf = oer_analysis_elf::Elf::parse(&bytes).unwrap();
    assert_eq!(
        by_demangled_name(&elf, "__rustc::rust_begin_unwind"),
        Some(offset)
    );
    assert_eq!(by_demangled_name(&elf, "rust_begin_unwind"), None);
}

const FLASH: Region = match Region::new(0x40000000, 0x20000) {
    Ok(region) => region,
    Err(_) => panic!("valid region"),
};

/// Two objects model an entry calling an out-of-line dependency. The old
/// layout annotates only the entry; the shared script needs no annotation.
/// Constants and explicit flash inputs exercise the actual shared script.
fn bootstrap_fixture(
    directory: &Path,
    shared: bool,
    extra: Option<(&str, object::SectionKind)>,
) -> std::path::PathBuf {
    use object::write::Relocation;
    use object::{RelocationFlags, SectionKind};

    let make = || Writer::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let mut entry = make();
    let code = entry.add_section(
        Vec::new(),
        if shared {
            b".text.entry".to_vec()
        } else {
            b".rwtext.entry".to_vec()
        },
        SectionKind::Text,
    );
    // auipc ra, 0; jalr ra, 0(ra); ret
    for word in [0x00000097_u32, 0x000080e7, 0x00008067] {
        entry.append_section_data(code, &word.to_le_bytes(), 4);
    }
    let symbol = |name: &str, section, size| Symbol {
        name: name.as_bytes().to_vec(),
        value: 0,
        size,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section,
        flags: SymbolFlags::None,
    };
    entry.add_symbol(symbol("_start", SymbolSection::Section(code), 12));
    let helper = entry.add_symbol(symbol("dependency_helper", SymbolSection::Undefined, 0));
    entry
        .add_relocation(
            code,
            Relocation {
                offset: 0,
                symbol: helper,
                addend: 0,
                flags: RelocationFlags::Elf {
                    r_type: object::elf::R_RISCV_CALL_PLT,
                },
            },
        )
        .unwrap();
    let input = directory.join("entry.o");
    fs::write(&input, entry.write().unwrap()).unwrap();

    let mut dependency = make();
    let code = dependency.add_section(
        Vec::new(),
        b".text.dependency_helper".to_vec(),
        SectionKind::Text,
    );
    dependency.append_section_data(code, &0x00008067_u32.to_le_bytes(), 4);
    dependency.add_symbol(symbol("dependency_helper", SymbolSection::Section(code), 4));
    for (section, name, size) in [
        (".rodata.dependency_constant", "dependency_constant", 4),
        (".psram.runtime.payload", "payload", 32),
        (".flash.tuning.reference", "reference", 32),
    ] {
        let section = dependency.add_section(
            Vec::new(),
            section.as_bytes().to_vec(),
            SectionKind::ReadOnlyData,
        );
        dependency.append_section_data(section, &vec![0x31; size], 4);
        dependency.add_symbol(Symbol {
            kind: SymbolKind::Data,
            ..symbol(name, SymbolSection::Section(section), size as u64)
        });
    }
    if let Some((name, kind)) = extra {
        let section = dependency.add_section(Vec::new(), name.as_bytes().to_vec(), kind);
        dependency.append_section_data(section, &[0x13, 0, 0, 0], 4);
    }
    let other = directory.join("dependency.o");
    fs::write(&other, dependency.write().unwrap()).unwrap();

    let memory = format!(
        "ENTRY(_start)\nMEMORY {{\n SRAM (rwx) : ORIGIN = {}, LENGTH = {}\n RODATA (r) : ORIGIN = 0x40000000, LENGTH = 64K\n ROTEXT (rx) : ORIGIN = 0x40010000, LENGTH = 64K\n PSRAM (rw) : ORIGIN = 0x50000000, LENGTH = 64K\n RETAINED (rw) : ORIGIN = 0x21000000, LENGTH = 256\n}}\n",
        SRAM.origin(),
        SRAM.length(),
    );
    let body = if shared {
        "INCLUDE bootstrap/sections.x"
    } else {
        "SECTIONS { .rwtext : { *(.rwtext*) } > SRAM\n .text : { *(.text*) } > ROTEXT\n .rodata : { *(.rodata*) } > RODATA\n .flash.rodata : { *(.psram.runtime.payload) *(.flash.tuning.reference) } > RODATA }"
    };
    let script = directory.join("fixture.x");
    let extra_script = extra.map_or_else(String::new, |(name, _)| {
        format!("SECTIONS {{ {name} : {{ *({name}) }} > RODATA }}")
    });
    fs::write(&script, format!("{memory}{body}\n{extra_script}\n")).unwrap();
    let output = directory.join("bootstrap.elf");
    let linker_scripts =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../chips/espressif/staged-layout/linker");
    let result = std::process::Command::new(oer_os_toolchain::lld().unwrap())
        .arg("-L")
        .arg(linker_scripts)
        .arg("-T")
        .arg(script)
        .args([
            "--defsym=MMU_PAGE_BYTES=4096",
            "--defsym=IMAGE_SEGMENT_HEADER_BYTES=32",
            "--defsym=FLASH_TUNING_REFERENCE_BYTES=32",
            "--defsym=RETAINED_ORIGIN=0x21000000",
        ])
        .arg(format!(
            "--defsym=SECOND_STAGE_LOADER_START={}",
            u64::from(SRAM.origin()) + u64::from(SRAM.length())
        ))
        .arg(input)
        .arg(other)
        .arg("-o")
        .arg(&output)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    output
}

#[test]
fn an_sram_entry_does_not_excuse_a_cached_callee_or_constant() {
    let temporary = tempfile::tempdir().unwrap();
    let path = bootstrap_fixture(temporary.path(), false, None);
    let error = bootstrap(SRAM, FLASH, &path).unwrap_err().to_string();
    assert!(error.contains("dependency_helper"), "{error}");
    assert!(error.contains("dependency_constant"), "{error}");
}

#[test]
fn the_bootstrap_script_places_out_of_line_code_and_constants_in_sram() {
    let temporary = tempfile::tempdir().unwrap();
    let path = bootstrap_fixture(temporary.path(), true, None);
    assert!(
        bootstrap(SRAM, FLASH, &path)
            .unwrap()
            .contains("result=PASS")
    );
    let bytes = fs::read(path).unwrap();
    let elf = oer_analysis_elf::Elf::executable(&bytes).unwrap();
    for name in ["_start", "dependency_helper", "dependency_constant"] {
        let symbol = elf.symbol(name).unwrap();
        assert!(
            SRAM.contains_range(symbol.address, symbol.address + symbol.size),
            "{name}"
        );
    }
    for name in ["payload", "reference"] {
        assert!(
            !SRAM.contains_range(
                elf.symbol(name).unwrap().address,
                elf.symbol(name).unwrap().address + 1
            ),
            "{name}"
        );
    }
}

#[test]
fn flash_data_requires_an_exact_allowlisted_section_and_never_allows_code_or_writes() {
    use object::SectionKind;
    for (name, kind, allowed) in [
        (".unrecognized_constant", SectionKind::ReadOnlyData, false),
        (".flash.rodata.extra", SectionKind::ReadOnlyData, false),
        (".metadata", SectionKind::ReadOnlyData, true),
        (
            ".flash.tuning.reference.extra",
            SectionKind::ReadOnlyData,
            false,
        ),
        (".flash.tuning.reference", SectionKind::Text, false),
        (".flash.tuning.reference", SectionKind::Data, false),
        (".metadata", SectionKind::Text, false),
        (".metadata", SectionKind::Data, false),
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let path = bootstrap_fixture(temporary.path(), true, Some((name, kind)));
        let result = bootstrap(SRAM, FLASH, &path);
        assert_eq!(result.is_ok(), allowed, "{name} {kind:?}: {result:?}");
        if let Err(error) = result {
            assert!(error.to_string().contains(name), "{error}");
        }
    }
}
