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

const SRAM: Region = Region {
    origin: 0x2f00_0000,
    length: 0x1000,
};
const ROM: Region = Region {
    origin: 0x2f80_0000,
    length: 0x1000,
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
