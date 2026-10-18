use super::*;
use object::write::{Object as Writer, StandardSection, Symbol, SymbolSection};
use object::{Architecture, BinaryFormat, Endianness, SymbolFlags, SymbolKind, SymbolScope};

/// `csrrw sp, mscratch, sp`.
const SWAP: u32 = 0x3401_1173;
/// `addi sp, sp, -16`.
const PUSH: u32 = 0xff01_0113;

/// An object whose `.text` holds the trap entry and the 47 PSRAM interrupt
/// entries, each beginning with `first(entry)`.
fn entries(first: impl Fn(usize) -> u32) -> Vec<u8> {
    let mut writer = Writer::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = writer.section_id(StandardSection::Text);
    let mut names = vec!["_start_trap".to_owned()];
    names.extend((1..=47).map(|number| format!("_runtime_psram_irq_entry_{number}")));
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
            name: "_runtime_psram_irq_entry_{n}".into(),
            numbers: Some([1, 47]),
        },
    ]
}

fn audit_entries(bytes: &[u8]) -> Result<()> {
    let elf = oer_elf::Elf::parse(bytes).unwrap();
    audit_stack_swap_entries(&elf, &elf.addresses(), &swapping())
}

#[test]
fn every_psram_entry_swaps_to_the_interrupt_stack_first() {
    audit_entries(&entries(|_| SWAP)).unwrap();
}

#[test]
fn an_entry_that_touches_the_interrupted_stack_first_is_refused() {
    let error = audit_entries(&entries(|index| if index == 5 { PUSH } else { SWAP })).unwrap_err();
    assert!(
        error.to_string().contains("_runtime_psram_irq_entry_5"),
        "{error}"
    );
}
