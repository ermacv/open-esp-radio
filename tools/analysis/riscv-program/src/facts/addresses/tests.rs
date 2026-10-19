use super::*;
use crate::facts::ImageFacts;
use object::write::{
    Object, Relocation as WriteRelocation, StandardSection, Symbol as WriteSymbol, SymbolSection,
};
use object::{Architecture, BinaryFormat, Endianness, SymbolFlags, SymbolScope};

#[derive(Clone, Copy)]
enum Reference {
    Function,
    Alias,
    Section,
    External,
}

/// Linked-layout fixture with code, read-only data, writable data and debug
/// relocations. An optional preceding section shifts all ELF identities.
fn image(
    base: u32,
    padding: bool,
    overlap: bool,
    uses: &[(usize, u32, Reference, i64)],
) -> Vec<u8> {
    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    if padding {
        let padding = object.add_section(
            Vec::new(),
            b".padding".to_vec(),
            object::SectionKind::ReadOnlyData,
        );
        object.append_section_data(padding, &[0; 4], 4);
    }
    let text = object.section_id(StandardSection::Text);
    let rodata = object.section_id(StandardSection::ReadOnlyData);
    let data = object.section_id(StandardSection::Data);
    let debug = object.add_section(
        Vec::new(),
        b".debug_uses".to_vec(),
        object::SectionKind::Debug,
    );
    let sections = [text, rodata, data, debug];
    for section in sections {
        object.append_section_data(section, &[0; 0x80], 4);
    }
    let mut symbol = |name: &str, value, size, kind, section| {
        object.add_symbol(WriteSymbol {
            name: name.as_bytes().to_vec(),
            value,
            size,
            kind,
            scope: SymbolScope::Linkage,
            weak: false,
            section,
            flags: SymbolFlags::None,
        })
    };
    let function = symbol(
        "f",
        0,
        0x20,
        object::SymbolKind::Text,
        SymbolSection::Section(text),
    );
    let alias = symbol(
        "alias",
        0,
        0x20,
        object::SymbolKind::Text,
        SymbolSection::Section(text),
    );
    symbol(
        "next",
        0x20,
        0x20,
        object::SymbolKind::Text,
        SymbolSection::Section(text),
    );
    if overlap {
        symbol(
            "overlap",
            4,
            8,
            object::SymbolKind::Text,
            SymbolSection::Section(text),
        );
    }
    let external = symbol(
        "external",
        0,
        0,
        object::SymbolKind::Text,
        SymbolSection::Undefined,
    );
    let section_symbol = object.section_symbol(text);
    let mut offsets = [0; 4];
    for &(section, r_type, reference, addend) in uses {
        let symbol = match reference {
            Reference::Function => function,
            Reference::Alias => alias,
            Reference::Section => section_symbol,
            Reference::External => external,
        };
        let offset = offsets[section];
        offsets[section] += 4;
        if section != 0 && r_type == rv32::R_RISCV_32 {
            let value = if matches!(reference, Reference::External) {
                0
            } else {
                u64::from(base)
            };
            object.section_mut(sections[section]).data_mut()[offset as usize..offset as usize + 4]
                .copy_from_slice(&(value.wrapping_add_signed(addend) as u32).to_le_bytes());
        }
        object
            .add_relocation(
                sections[section],
                WriteRelocation {
                    offset,
                    symbol,
                    addend,
                    flags: object::RelocationFlags::Elf { r_type },
                },
            )
            .unwrap();
    }
    let mut bytes = object.write().unwrap();
    // Convert the writer's ET_REL layout to ET_EXEC, placing its sections,
    // symbols and relocation sites at linked addresses. No second parser.
    let file = Elf::parse(&bytes).unwrap();
    let mut patches = Vec::new();
    let shoff = u32::from_le_bytes(bytes[32..36].try_into().unwrap()) as usize;
    for section in file.sections() {
        let address = match section.name {
            ".text" => base,
            ".rodata" => base + 0x1000,
            ".data" => base + 0x2000,
            _ => 0,
        };
        patches.push((shoff + section.index * 40 + 12, address));
        if let Some(target) = section.relocates {
            let target = file.section(target).unwrap();
            let address = match target.name {
                ".text" => base,
                ".rodata" => base + 0x1000,
                ".data" => base + 0x2000,
                _ => 0,
            };
            for (index, entry) in section.data.chunks_exact(12).enumerate() {
                let offset = u32::from_le_bytes(entry[..4].try_into().unwrap());
                patches.push((section.file_offset as usize + index * 12, address + offset));
            }
        }
    }
    let symbols = file.section(file.symbol_table_section().unwrap()).unwrap();
    for symbol in file.symbols() {
        if symbol
            .section
            .is_some_and(|index| file.section(index).unwrap().name == ".text")
        {
            patches.push((
                symbols.file_offset as usize + symbol.index * 16 + 4,
                base + symbol.address as u32,
            ));
        }
    }
    for (at, value) in patches {
        bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes[16..18].copy_from_slice(&2_u16.to_le_bytes());
    bytes
}

fn with_image(bytes: &[u8], body: impl FnOnce(&ImageFacts<'_>)) {
    let memory = WorkingMemory::new(1 << 24).unwrap();
    let image = ImageFacts::new(bytes, &[], &memory, &mut || Ok(())).unwrap();
    body(&image);
}

#[test]
fn symbols_sections_addends_and_aliases_keep_their_original_identities() {
    let bytes = image(
        0x1000,
        false,
        false,
        &[
            (0, rv32::R_RISCV_HI20, Reference::Function, 0),
            (0, rv32::R_RISCV_LO12_I, Reference::Alias, 0),
            (1, rv32::R_RISCV_32, Reference::Section, 0),
            (2, rv32::R_RISCV_32, Reference::Function, 2),
        ],
    );
    with_image(&bytes, |image| {
        let uses = image.address_uses();
        assert_eq!(uses.len(), 4);
        assert_eq!(image.words(), &[(0x2000, 0x1000), (0x3000, 0x1002)]);
        assert_eq!(uses[0].role, rv32::Role::AbsoluteHigh);
        assert_eq!(uses[1].role, rv32::Role::AbsoluteLow);
        assert_ne!(uses[0].relocation.target, uses[1].relocation.target);
        assert_eq!(uses[0].symbol.as_ref().unwrap().name, "f");
        assert_eq!(uses[1].symbol.as_ref().unwrap().name, "alias");
        assert_eq!(image.functions()[0].names, ["alias", "f"]);
        for use_ in &uses[..3] {
            assert_eq!(use_.value, TargetValue::Resolved(0x1000));
            assert_eq!(
                use_.function.nearest,
                Some(FunctionAddress {
                    entry: 0x1000,
                    offset: 0
                })
            );
            assert_eq!(use_.function.matches, 1);
        }
        assert_eq!(
            uses[2].symbol.as_ref().unwrap().kind,
            oer_analysis_elf::SymbolKind::Section
        );
        assert_eq!(uses[3].relocation.addend, 2);
        assert_eq!(uses[3].function.nearest.unwrap().offset, 2);
        assert_eq!(uses[3].symbol_function.nearest.unwrap().offset, 0);
        assert!(!uses[3].relocation.implicit_addend);
    });
}

#[test]
fn odd_interior_cross_function_and_overlapping_targets_are_explicit() {
    let bytes = image(
        0x1000,
        false,
        true,
        &[
            (0, rv32::R_RISCV_HI20, Reference::Section, 1),
            (0, rv32::R_RISCV_HI20, Reference::Function, 6),
            (0, rv32::R_RISCV_ADD32, Reference::Alias, 0x20),
            (0, rv32::R_RISCV_LO12_I, Reference::Section, -2),
        ],
    );
    with_image(&bytes, |image| {
        let uses = image.address_uses();
        assert_eq!(uses[0].value, TargetValue::Resolved(0x1001));
        assert_eq!(uses[0].value.canonical(), Some(0x1000));
        assert_eq!(uses[0].function.nearest.unwrap().offset, 1);
        assert_eq!(uses[1].function.matches, 2);
        assert_eq!(
            uses[1].function.nearest,
            Some(FunctionAddress {
                entry: 0x1004,
                offset: 2
            })
        );
        assert_eq!(uses[2].symbol_function.nearest.unwrap().entry, 0x1000);
        assert_eq!(uses[2].function.nearest.unwrap().entry, 0x1020);
        assert_eq!(uses[3].relocation.addend, -2);
        assert_eq!(uses[3].value, TargetValue::Resolved(0xffe));
        assert_eq!(uses[3].function.matches, 0);
    });
}

#[test]
fn unknown_roles_and_external_targets_survive_among_known_uses() {
    let bytes = image(
        0x1000,
        false,
        false,
        &[
            (0, rv32::R_RISCV_HI20, Reference::Function, 0),
            (0, 0xff, Reference::Function, 0),
            (0, rv32::R_RISCV_LO12_I, Reference::External, 7),
            (0, rv32::R_RISCV_CALL, Reference::External, 0),
            (0, rv32::R_RISCV_RELAX, Reference::Function, 0),
            (3, 0xff, Reference::Function, 0),
        ],
    );
    with_image(&bytes, |image| {
        let uses = image.address_uses();
        assert_eq!(
            uses.len(),
            3,
            "only hints, transfers and unallocated uses are excluded"
        );
        assert_eq!(uses[1].role, rv32::Role::Other);
        assert_eq!(uses[1].relocation.r_type, 0xff);
        assert_eq!(uses[1].function.nearest.unwrap().entry, 0x1000);
        assert_eq!(uses[2].value, TargetValue::External(7));
        assert!(!uses[2].symbol.as_ref().unwrap().defined);
        assert_eq!(uses[2].function, FunctionRelation::default());
    });
}

#[test]
fn unsupported_targets_are_retained_and_numerical_projection_fails_explicitly() {
    let bytes = image(0x1000, false, false, &[]);
    let file = Elf::executable(&bytes).unwrap();
    let functions = file.functions().unwrap();
    let memory = WorkingMemory::new(1 << 20).unwrap();
    let lookup = FunctionLookup::new(&functions, &memory, &mut || Ok(())).unwrap();
    let relocation = Relocation {
        section: 1,
        at: 0x1000,
        r_type: 0xff,
        target: Target::Other,
        addend: 9,
        implicit_addend: true,
    };
    let use_ = AddressUse::new(&file, relocation, &lookup, &mut || Ok(())).unwrap();
    assert_eq!(use_.relocation, relocation);
    assert_eq!(use_.value, TargetValue::Unsupported);
    assert_eq!(
        use_.value.elf_value().unwrap_err().code,
        ErrorCode::Integrity
    );
    assert_eq!(use_.function, FunctionRelation::default());
}

#[test]
fn a_direct_section_target_resolves_its_addend_in_its_own_elf() {
    let bytes = image(0x1000, false, false, &[]);
    let file = Elf::executable(&bytes).unwrap();
    let functions = file.functions().unwrap();
    let section = file.section_by_name(".text").unwrap().index;
    let memory = WorkingMemory::new(1 << 20).unwrap();
    let lookup = FunctionLookup::new(&functions, &memory, &mut || Ok(())).unwrap();
    let relocation = Relocation {
        section,
        at: 0x1004,
        r_type: rv32::R_RISCV_HI20,
        target: Target::Section(section),
        addend: 3,
        implicit_addend: false,
    };
    let use_ = AddressUse::new(&file, relocation, &lookup, &mut || Ok(())).unwrap();
    assert_eq!(use_.relocation, relocation);
    assert!(use_.symbol.is_none());
    assert_eq!(use_.value, TargetValue::Resolved(0x1003));
    assert_eq!(
        use_.function.nearest,
        Some(FunctionAddress {
            entry: 0x1000,
            offset: 3
        })
    );
}

#[test]
fn each_elf_owns_its_section_and_symbol_identities() {
    let entries = [(0, rv32::R_RISCV_HI20, Reference::Section, 2)];
    let main = image(0x1000, false, false, &entries);
    let companion = image(0x8000, true, false, &entries);
    with_image(&main, |main| {
        with_image(&companion, |companion| {
            let a = &main.address_uses()[0];
            let b = &companion.address_uses()[0];
            assert_ne!(a.relocation.section, b.relocation.section);
            assert_ne!(
                a.symbol.as_ref().unwrap().section,
                b.symbol.as_ref().unwrap().section
            );
            assert_eq!(a.value, TargetValue::Resolved(0x1002));
            assert_eq!(b.value, TargetValue::Resolved(0x8002));
            assert_eq!(a.function.nearest.unwrap().entry, 0x1000);
            assert_eq!(b.function.nearest.unwrap().entry, 0x8000);
        })
    });
}

struct Units(u64);
impl RunControl for Units {
    fn checkpoint(&mut self, units: u64) -> Result<()> {
        self.0 = self
            .0
            .checked_sub(units)
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimited, "work exhausted"))?;
        Ok(())
    }
}

#[test]
fn building_address_uses_obeys_memory_and_work_limits_and_releases_reservations() {
    let entries = vec![(0, rv32::R_RISCV_HI20, Reference::Function, 0); 16];
    let bytes = image(0x1000, false, false, &entries);
    let memory = WorkingMemory::new(1 << 24).unwrap();
    let mut count = Units(u64::MAX);
    let facts = ImageFacts::new(&bytes, &[], &memory, &mut count).unwrap();
    assert_eq!(facts.address_uses().len(), 16);
    let units = u64::MAX - count.0;
    let peak = memory.peak();
    drop(facts);
    assert_eq!(memory.used(), 0);
    let error = ImageFacts::new(&bytes, &[], &memory, &mut Units(units - 1))
        .err()
        .unwrap();
    assert_eq!(error.code, ErrorCode::ResourceLimited);
    assert_eq!(memory.used(), 0);
    let limited = WorkingMemory::new(peak - 1).unwrap();
    let error = ImageFacts::new(&bytes, &[], &limited, &mut || Ok(()))
        .err()
        .unwrap();
    assert_eq!(error.code, ErrorCode::ResourceLimited);
    assert_eq!(limited.used(), 0);
}
