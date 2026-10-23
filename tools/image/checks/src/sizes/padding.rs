//! Input-free linker ranges, identified by existing boundary contracts, never
//! by output section names: the runtime minimum payload tail and bootstrap
//! alignment between its embedded payload and cold-reference pages.
use oer_analysis_elf::{Elf, Section};

pub(super) fn ranges(file: &Elf<'_>, section: &Section<'_>) -> Vec<super::Filler> {
    let mut ranges = Vec::new();
    if let Some((start, end)) = payload_tail(file)
        && section.address == start
        && section.address.checked_add(section.size) == Some(end)
        && input_free_zeroes(file, section, start, end)
    {
        ranges.push(super::Filler {
            address: start,
            bytes: end - start,
            contract: "runtime minimum payload boundary".into(),
        });
    }
    if let (Some(start), Some(end)) = (
        file.address("__psram_runtime_payload_flash_end"),
        file.address("__flash_tuning_reference_start"),
    ) && start.checked_add(4095).map(|n| n & !4095) == Some(end)
        && start < end
        && input_free_zeroes(file, section, start, end)
    {
        ranges.push(super::Filler {
            address: start,
            bytes: end - start,
            contract: "bootstrap cold-reference page alignment".into(),
        });
    }
    ranges
}

fn input_free_zeroes(file: &Elf<'_>, section: &Section<'_>, start: u64, end: u64) -> bool {
    let Some(from) = start
        .checked_sub(section.address)
        .and_then(|n| usize::try_from(n).ok())
    else {
        return false;
    };
    let Some(to) = end
        .checked_sub(section.address)
        .and_then(|n| usize::try_from(n).ok())
    else {
        return false;
    };
    !section.executable
        && !section.nobits
        && section
            .data
            .get(from..to)
            .is_some_and(|bytes| bytes.iter().all(|&b| b == 0))
        && !file.symbols().any(|s| {
            s.defined
                && s.section == Some(section.index)
                && s.size != 0
                && s.address < end
                && s.address
                    .checked_add(s.size)
                    .is_none_or(|last| last > start)
        })
        && file
            .relocations(section.index)
            .is_ok_and(|mut r| !r.any(|rel| rel.at >= start && rel.at < end))
}

fn payload_tail(file: &Elf<'_>) -> Option<(u64, u64)> {
    let initialized_end = file.address("__runtime_data_load_end")?;
    let start = initialized_end.checked_add(15)? & !15;
    let image_start = file.address("__runtime_image_start")?;
    let end = file.address("__runtime_payload_end")?;
    (end == start
        .checked_add(1)?
        .max(image_start.checked_add(0x1_0000)?))
    .then_some((start, end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use object::{
        Architecture, BinaryFormat, Endianness, SectionKind, SymbolFlags, SymbolKind, SymbolScope,
        write::{Object, Symbol, SymbolSection},
    };

    fn object(
        name: &str,
        markers: bool,
        input_symbol: bool,
        nonzero: bool,
        writable: bool,
    ) -> Vec<u8> {
        let mut writer = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
        let kind = if writable {
            SectionKind::Data
        } else {
            SectionKind::ReadOnlyData
        };
        let section = writer.add_section(vec![], name.as_bytes().to_vec(), kind);
        let mut bytes = vec![0; 65536];
        if nonzero {
            bytes[100] = 1;
        }
        writer.append_section_data(section, &bytes, 16);
        if markers {
            for (name, value) in [
                ("__runtime_image_start", 0),
                ("__runtime_data_load_end", 0),
                ("__runtime_payload_end", 65536),
            ] {
                writer.add_symbol(Symbol {
                    name: name.as_bytes().to_vec(),
                    value,
                    size: 0,
                    kind: SymbolKind::Data,
                    scope: SymbolScope::Linkage,
                    weak: false,
                    section: SymbolSection::Absolute,
                    flags: SymbolFlags::None,
                });
            }
        }
        if input_symbol {
            writer.add_symbol(Symbol {
                name: b"REAL_OBJECT".to_vec(),
                value: 0,
                size: 4,
                kind: SymbolKind::Data,
                scope: SymbolScope::Linkage,
                weak: false,
                section: SymbolSection::Section(section),
                flags: SymbolFlags::None,
            });
        }
        writer.write().unwrap()
    }

    #[test]
    fn linker_boundary_contract_not_section_name_identifies_padding() {
        for (name, writable) in [
            (".runtime.payload_end", false),
            (".renamed.output", false),
            (".inherited_flags", true),
        ] {
            let bytes = object(name, true, false, false, writable);
            let elf = Elf::parse(&bytes).unwrap();
            let section = elf.sections().find(|s| s.name == name).unwrap();
            assert_eq!(section.writable, writable);
            assert_eq!(ranges(&elf, &section).len(), 1);
        }
        for (markers, input, nonzero) in [
            (false, false, false),
            (true, true, false),
            (true, false, true),
        ] {
            let bytes = object(".runtime.payload_end", markers, input, nonzero, false);
            let elf = Elf::parse(&bytes).unwrap();
            let section = elf
                .sections()
                .find(|s| s.name == ".runtime.payload_end")
                .unwrap();
            assert!(ranges(&elf, &section).is_empty());
        }
    }

    #[test]
    fn bootstrap_alignment_ignores_real_contents_and_relocations_outside_the_gap() {
        for conflict in [false, true] {
            let mut writer =
                Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
            let section = writer.add_section(
                vec![],
                b".arbitrary.constants".to_vec(),
                SectionKind::ReadOnlyData,
            );
            writer.append_section_data(section, &[0; 8192], 16);
            for (name, value) in [
                ("__psram_runtime_payload_flash_end", 128),
                ("__flash_tuning_reference_start", 4096),
            ] {
                writer.add_symbol(Symbol {
                    name: name.as_bytes().to_vec(),
                    value,
                    size: 0,
                    kind: SymbolKind::Data,
                    scope: SymbolScope::Linkage,
                    weak: false,
                    section: SymbolSection::Absolute,
                    flags: SymbolFlags::None,
                });
            }
            let value = if conflict { 1000 } else { 4096 };
            let target = writer.add_symbol(Symbol {
                name: b"ACTUAL_CONSTANT".to_vec(),
                value,
                size: 4,
                kind: SymbolKind::Data,
                scope: SymbolScope::Linkage,
                weak: false,
                section: SymbolSection::Section(section),
                flags: SymbolFlags::None,
            });
            writer
                .add_relocation(
                    section,
                    object::write::Relocation {
                        offset: value,
                        symbol: target,
                        addend: 0,
                        flags: object::RelocationFlags::Elf {
                            r_type: object::elf::R_RISCV_32,
                        },
                    },
                )
                .unwrap();
            let bytes = writer.write().unwrap();
            let elf = Elf::parse(&bytes).unwrap();
            let section = elf
                .sections()
                .find(|s| s.name == ".arbitrary.constants")
                .unwrap();
            let found = ranges(&elf, &section);
            assert_eq!(found.len(), usize::from(!conflict));
            if !conflict {
                assert_eq!((found[0].address, found[0].bytes), (128, 3968));
            }
        }
    }
}
