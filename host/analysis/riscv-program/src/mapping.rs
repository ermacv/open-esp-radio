//! One owner for ELF mapping-symbol interpretation and admitted interval storage.
use oer_analysis_riscv_model::*;

pub struct DataRanges<'a> {
    pub ranges: Vec<CodeRange>,
    _capacity: MemoryReservation<'a>,
}
fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::Integrity, message)
}
fn allocation() -> Error {
    Error::new(
        ErrorCode::ResourceLimited,
        "mapping symbol allocation refused",
    )
}
// https://riscv-non-isa.github.io/riscv-elf-psabi-doc/#_mapping_symbol
// ISA-qualified markers delimit code too. They do not expand decoder support.
fn kind(name: &[u8]) -> Result<Option<bool>> {
    if name == b"$d" || name.starts_with(b"$d.") {
        Ok(Some(true))
    } else if name == b"$x" || name.starts_with(b"$x.") || name.starts_with(b"$xrv32") {
        Ok(Some(false))
    } else if name.starts_with(b"$xrv") {
        Err(Error::new(
            ErrorCode::Incompatible,
            "mapping ISA is outside RV32",
        ))
    } else {
        Ok(None)
    }
}
/// The name of a symbol of the section being mapped: one that cannot be read
/// is an invalid mapping symbol. Other sections' symbols are never read.
fn name<'d>(symbol: &oer_analysis_elf::RawSymbol<'d>) -> Result<&'d [u8]> {
    symbol
        .name
        .ok_or_else(|| invalid("invalid mapping symbol name"))
}
pub fn data_ranges<'a>(
    file: &oer_analysis_elf::Elf<'_>,
    section_index: usize,
    memory: &'a WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<DataRanges<'a>> {
    let section = file
        .section(section_index)
        .map_err(|_| invalid("invalid mapping section"))?;
    let end = section
        .address
        .checked_add(section.size)
        .ok_or_else(|| invalid("mapping section extent overflow"))?;
    let mut count = 0usize;
    for symbol in file.raw_symbols() {
        control.checkpoint(1)?;
        if symbol.section == Some(section_index) && kind(name(&symbol)?)?.is_some() {
            count = count
                .checked_add(1)
                .ok_or_else(|| invalid("mapping count overflow"))?;
        }
    }
    let capacity = memory.reserve(
        (count as u64)
            .checked_mul(
                (std::mem::size_of::<(u64, bool)>() + std::mem::size_of::<CodeRange>()) as u64,
            )
            .ok_or_else(|| invalid("mapping capacity overflow"))?,
        control.position(),
    )?;
    let mut mappings = Vec::new();
    mappings
        .try_reserve_exact(count)
        .map_err(|_| allocation())?;
    let mut ranges = Vec::new();
    ranges.try_reserve_exact(count).map_err(|_| allocation())?;
    for symbol in file.raw_symbols() {
        control.checkpoint(1)?;
        if symbol.section != Some(section_index) {
            continue;
        }
        let Some(is_data) = kind(name(&symbol)?)? else {
            continue;
        };
        if symbol.global
            || symbol.kind != oer_analysis_elf::SymbolKind::Unknown
            || symbol.size != 0
            || symbol.address < section.address
            || symbol.address > end
            || (!is_data && symbol.address & 1 != 0)
        {
            return Err(invalid("invalid ELF mapping symbol"));
        }
        mappings.push((symbol.address, is_data));
    }
    // In-place, bounded storage; charge the comparison bound before sorting.
    control.checkpoint(
        (count as u64).saturating_mul(u64::from(usize::BITS - count.leading_zeros())),
    )?;
    mappings.sort_unstable();
    mappings.dedup();
    for (i, &(start, is_data)) in mappings.iter().enumerate() {
        control.checkpoint(1)?;
        let next = mappings.get(i + 1).map_or(end, |m| m.0);
        if next == start && i + 1 < mappings.len() {
            return Err(invalid("conflicting ELF mapping symbols"));
        }
        if is_data && next > start {
            ranges.push(CodeRange {
                start,
                length: next - start,
            });
        }
    }
    Ok(DataRanges {
        ranges,
        _capacity: capacity,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use object::write::{Object, StandardSection, Symbol, SymbolSection};
    use object::{Architecture, BinaryFormat, Endianness, SymbolFlags, SymbolKind, SymbolScope};

    /// A relocatable RV32 object whose `.text` holds eight bytes and a `$d`
    /// mapping symbol at offset 4, with the `.text` section index.
    fn object() -> (Vec<u8>, usize) {
        let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
        let text = object.section_id(StandardSection::Text);
        object.append_section_data(text, &[0; 8], 4);
        object.add_symbol(Symbol {
            name: b"$d".to_vec(),
            value: 4,
            size: 0,
            kind: SymbolKind::Label,
            scope: SymbolScope::Compilation,
            weak: false,
            section: SymbolSection::Section(text),
            flags: SymbolFlags::None,
        });
        let bytes = object.write().unwrap();
        let index = oer_analysis_elf::Elf::parse(&bytes)
            .unwrap()
            .section_by_name(".text")
            .unwrap()
            .index;
        (bytes, index)
    }

    #[test]
    fn a_mapping_symbol_marks_data_and_an_unreadable_name_fails() {
        let memory = WorkingMemory::new(1 << 20).unwrap();
        let (mut bytes, text) = object();
        let ranges = data_ranges(
            &oer_analysis_elf::Elf::parse(&bytes).unwrap(),
            text,
            &memory,
            &mut || Ok(()),
        )
        .unwrap()
        .ranges;
        assert_eq!(
            ranges,
            [CodeRange {
                start: 4,
                length: 4
            }]
        );
        // Point the `$d` symbol's name outside the string table.
        let elf = oer_analysis_elf::Elf::parse(&bytes).unwrap();
        let table = elf.section(elf.symbol_table_section().unwrap()).unwrap();
        let entry = elf
            .raw_symbols()
            .find(|s| s.name == Some(b"$d".as_slice()))
            .unwrap()
            .index;
        let at = (table.file_offset + 16 * entry as u64) as usize;
        bytes[at..at + 4].copy_from_slice(&0x00ff_ffffu32.to_le_bytes());
        let error = data_ranges(
            &oer_analysis_elf::Elf::parse(&bytes).unwrap(),
            text,
            &memory,
            &mut || Ok(()),
        )
        .err()
        .unwrap();
        assert_eq!(error.message, "invalid mapping symbol name");
    }

    #[test]
    fn an_unreadable_name_in_another_section_is_never_read() {
        let memory = WorkingMemory::new(1 << 20).unwrap();
        let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
        let text = object.section_id(StandardSection::Text);
        object.append_section_data(text, &[0; 8], 4);
        let data = object.section_id(StandardSection::Data);
        object.append_section_data(data, &[0; 4], 4);
        object.add_symbol(Symbol {
            name: b"other".to_vec(),
            value: 0,
            size: 4,
            kind: SymbolKind::Data,
            scope: SymbolScope::Compilation,
            weak: false,
            section: SymbolSection::Section(data),
            flags: SymbolFlags::None,
        });
        let mut bytes = object.write().unwrap();
        let elf = oer_analysis_elf::Elf::parse(&bytes).unwrap();
        let text = elf.section_by_name(".text").unwrap().index;
        let table = elf.section(elf.symbol_table_section().unwrap()).unwrap();
        let entry = elf
            .raw_symbols()
            .find(|s| s.name == Some(b"other".as_slice()))
            .unwrap()
            .index;
        let at = (table.file_offset + 16 * entry as u64) as usize;
        bytes[at..at + 4].copy_from_slice(&0x00ff_ffffu32.to_le_bytes());
        let ranges = data_ranges(
            &oer_analysis_elf::Elf::parse(&bytes).unwrap(),
            text,
            &memory,
            &mut || Ok(()),
        )
        .unwrap()
        .ranges;
        assert!(ranges.is_empty());
    }
}
