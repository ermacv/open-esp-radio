//! The jump-table entries a relocatable object's relocations prove.
//!
//! A compiler keeps a `switch` table in read-only data with one `R_RISCV_32`
//! relocation per entry naming its case label. A table has no label of its
//! own, so its length comes from the dispatch's bounds check; each entry is
//! the label its word's relocation names. Only `SHT_RELA` relocations count:
//! `SHT_REL` keeps an entry's addend in the table word itself, which this
//! source does not read, so a REL table is never proven, and a section that
//! two RELA sections relocate names no entry.
use oer_analysis_elf::{SectionKind, Target, rv32};
use oer_analysis_riscv_model::*;

/// The relocated words of one object's read-only data.
pub struct ObjectTables<'a> {
    file: oer_analysis_elf::Elf<'a>,
    /// The static symbol table's section.
    symbol_table: Option<usize>,
    /// `((section, offset), (target section, target offset))`, sorted.
    words: AdmittedVec<'a, ((usize, u64), (usize, u64))>,
}

fn invalid(message: impl Into<String>) -> Error {
    Error::new(ErrorCode::Integrity, message)
}

impl<'a> ObjectTables<'a> {
    /// Read the table words of the relocatable object `bytes`.
    pub fn new(
        bytes: &'a [u8],
        memory: &'a WorkingMemory,
        control: &mut dyn RunControl,
    ) -> Result<Self> {
        control.bytes(bytes.len())?;
        let file =
            oer_analysis_elf::Elf::parse(bytes).map_err(|error| invalid(error.to_string()))?;
        // The sections each RELA section relocates, sorted, to count the
        // RELA sections of a section by a search.
        let mut relocated = AdmittedVec::new(memory);
        for section in file.sections() {
            control.checkpoint(1)?;
            if section.kind == SectionKind::Rela
                && let Some(target) = section.relocates
            {
                relocated.push(target, control.position())?;
            }
        }
        control.checkpoint(relocated.len() as u64)?;
        relocated.sort_unstable();
        let mut words = AdmittedVec::new(memory);
        for section in file.sections() {
            control.checkpoint(1)?;
            let read_only_data = section.kind == SectionKind::Program
                && section.allocated
                && !section.writable
                && !section.executable;
            if !read_only_data {
                continue;
            }
            let first = relocated.partition_point(|&target| target < section.index);
            let rela = relocated[first..]
                .iter()
                .take_while(|&&target| target == section.index)
                .count();
            if rela != 1 {
                continue;
            }
            let relocations = file
                .relocations(section.index)
                .map_err(|error| invalid(error.to_string()))?;
            for relocation in relocations {
                control.checkpoint(1)?;
                // An address word, `R_RISCV_32`, with an explicit addend.
                if relocation.implicit_addend
                    || rv32::kind(relocation.r_type).role != rv32::Role::Word
                {
                    continue;
                }
                let (target_section, value) = match relocation.target {
                    Target::Symbol(index) => {
                        let Ok(symbol) = file.raw_symbol_at(index) else {
                            continue;
                        };
                        let Some(target_section) = symbol.section else {
                            continue;
                        };
                        (target_section, symbol.address)
                    }
                    Target::Section(index) => (index, 0),
                    Target::Absolute | Target::Other => continue,
                };
                let Some(target) = value.checked_add_signed(relocation.addend) else {
                    continue;
                };
                words.push(
                    ((section.index, relocation.at), (target_section, target)),
                    control.position(),
                )?;
            }
        }
        control.checkpoint(words.len() as u64)?;
        words.sort_unstable_by_key(|(key, _)| *key);
        drop(relocated);
        let symbol_table = file.symbol_table_section();
        Ok(Self {
            file,
            symbol_table,
            words,
        })
    }

    /// The `(section, offset)` the relocated word at `offset` of read-only
    /// data `section` names.
    fn word(&self, section: usize, offset: u64) -> Option<(usize, u64)> {
        let at = self
            .words
            .binary_search_by_key(&(section, offset), |(key, _)| *key)
            .ok()?;
        Some(self.words[at].1)
    }
}

impl TableEntries for ObjectTables<'_> {
    fn entries<'m>(
        &self,
        base: &AbstractValue,
        count: Option<u32>,
        section: u32,
        memory: &'m WorkingMemory,
        control: &mut dyn RunControl,
    ) -> Result<Option<TableWords<'m>>> {
        // A table of an object is a symbol of its static symbol table, and
        // only a bounds check states its length.
        let (AbstractValue::Symbol { symbol, addend }, Some(count)) = (base, count) else {
            return Ok(None);
        };
        if symbol.table != SymbolTableKind::Static
            || self.symbol_table != Some(symbol.table_section as usize)
        {
            return Ok(None);
        }
        let Ok(index) = usize::try_from(symbol.index) else {
            return Ok(None);
        };
        let Ok(table) = self.file.raw_symbol_at(index) else {
            return Ok(None);
        };
        let (Some(table_section), Some(start)) =
            (table.section, table.address.checked_add_signed(*addend))
        else {
            return Ok(None);
        };
        let bytes = u64::from(count) * size_of::<u64>() as u64;
        let reserved = memory.reserve(bytes, control.position())?;
        let mut entries = Vec::new();
        entries
            .try_reserve_exact(count as usize)
            .map_err(|_| Error::new(ErrorCode::ResourceLimited, "table allocation refused"))?;
        for i in 0..u64::from(count) {
            control.checkpoint(1)?;
            match self.word(table_section, start + 4 * i) {
                Some((target_section, target)) if target_section == section as usize => {
                    entries.push(target)
                }
                _ => break,
            }
        }
        Ok(Some(TableWords {
            source: TableSource::Bound,
            entries,
            reserved,
        }))
    }
}

/// The jump-table entries a static image's facts prove: a table the
/// compiler labelled (`.LJTI*`) is the run of words an `--emit-relocs` link
/// kept relocated from its label to the next symbol of its section; a sized
/// read-only data object is its words; with a bounds check, the bound's
/// count of read-only words.
pub struct ImageTables<'f, 'a> {
    pub image: &'f crate::facts::ImageFacts<'a>,
}

impl TableEntries for ImageTables<'_, '_> {
    fn entries<'m>(
        &self,
        base: &AbstractValue,
        count: Option<u32>,
        _: u32,
        memory: &'m WorkingMemory,
        control: &mut dyn RunControl,
    ) -> Result<Option<TableWords<'m>>> {
        let (AbstractValue::Constant { value: base }
        | AbstractValue::ImageAddress { address: base }) = *base
        else {
            return Ok(None);
        };
        let image = self.image;
        let limit = count.map(u64::from);
        // Each entry is reserved and charged before it is read.
        let mut reserved = memory.reserve(0, control.position())?;
        let mut entries = Vec::new();
        let mut push = |entry: u32, control: &mut dyn RunControl| -> Result<()> {
            control.checkpoint(1)?;
            if entries.len() == entries.capacity() {
                let grown = (entries.capacity() * 2).max(8);
                let next = memory.reserve((grown * size_of::<u64>()) as u64, control.position())?;
                entries
                    .try_reserve_exact(grown - entries.len())
                    .map_err(|_| {
                        Error::new(ErrorCode::ResourceLimited, "table allocation refused")
                    })?;
                reserved = next;
            }
            entries.push(u64::from(entry));
            Ok(())
        };
        let source = if let Some(words) = image.jump_table(base) {
            for (i, word) in words.enumerate() {
                if limit.is_some_and(|limit| i as u64 == limit) {
                    break;
                }
                push(word, control)?;
            }
            TableSource::Labelled
        } else {
            let words = match (limit, image.data_object(base)) {
                (Some(limit), _) => limit,
                (None, Some(size)) => size / 4,
                (None, None) => return Ok(None),
            };
            for i in 0..words {
                let Some(word) = image.read_only_word(base.wrapping_add(4 * i as u32)) else {
                    break;
                };
                push(word, control)?;
            }
            if limit.is_some() {
                TableSource::Bound
            } else {
                TableSource::DataObject
            }
        };
        Ok(Some(TableWords {
            source,
            entries,
            reserved,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use object::write::{Object, Relocation, StandardSection, Symbol, SymbolSection};
    use object::{Architecture, BinaryFormat, Endianness, SymbolFlags, SymbolKind, SymbolScope};

    /// A relocatable object: `.text` with case labels at 0x20, 0x30 and
    /// 0x38, `.rodata` holding `.Ltable`, three words relocated by
    /// `R_RISCV_32` of type `r_type` to the labels, and `.data` with one
    /// relocated word.
    fn object(r_type: u32) -> (Vec<u8>, u64) {
        let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
        let text = object.section_id(StandardSection::Text);
        object.append_section_data(text, &[0; 0x40], 4);
        let rodata = object.section_id(StandardSection::ReadOnlyData);
        object.append_section_data(rodata, &[0; 12], 4);
        let data = object.section_id(StandardSection::Data);
        object.append_section_data(data, &[0; 4], 4);
        let symbol = |object: &mut Object, name: &str, section, value| {
            object.add_symbol(Symbol {
                name: name.as_bytes().to_vec(),
                value,
                size: 0,
                kind: SymbolKind::Label,
                scope: SymbolScope::Compilation,
                weak: false,
                section: SymbolSection::Section(section),
                flags: SymbolFlags::None,
            })
        };
        symbol(&mut object, ".Ltable", rodata, 0);
        let labels = [0x20, 0x30, 0x38].map(|value| symbol(&mut object, "", text, value));
        for (entry, label) in labels.into_iter().enumerate() {
            object
                .add_relocation(
                    rodata,
                    Relocation {
                        offset: 4 * entry as u64,
                        symbol: label,
                        addend: 0,
                        flags: object::RelocationFlags::Elf { r_type },
                    },
                )
                .unwrap();
        }
        object
            .add_relocation(
                data,
                Relocation {
                    offset: 0,
                    symbol: labels[0],
                    addend: 0,
                    flags: object::RelocationFlags::Elf { r_type: 1 },
                },
            )
            .unwrap();
        let bytes = object.write().unwrap();
        let index = oer_analysis_elf::Elf::parse(&bytes)
            .unwrap()
            .raw_symbols()
            .find(|s| s.name == Some(b".Ltable".as_slice()))
            .unwrap()
            .index as u64;
        (bytes, index)
    }

    /// The section header of the section named `name`: its file offset.
    fn header(bytes: &[u8], name: &str) -> usize {
        let file = oer_analysis_elf::Elf::parse(bytes).unwrap();
        let index = file.sections().find(|s| s.name == name).unwrap().index;
        let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let half = |at: usize| u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap());
        word(0x20) as usize + half(0x2e) as usize * index
    }

    fn entries(bytes: &[u8], table: u64, count: Option<u32>) -> Option<Vec<u64>> {
        let memory = WorkingMemory::new(1 << 20).unwrap();
        let tables = ObjectTables::new(bytes, &memory, &mut || Ok(())).unwrap();
        let file = oer_analysis_elf::Elf::parse(bytes).unwrap();
        let symbol = SymbolId {
            object: ObjectId {
                artifact: ArtifactId::of_bytes(b"object"),
                location: ObjectLocation::Standalone,
            },
            table: SymbolTableKind::Static,
            table_section: file.symbol_table_section().unwrap() as u32,
            index: table,
        };
        let text = file.sections().find(|s| s.name == ".text").unwrap().index as u32;
        let base = AbstractValue::Symbol { symbol, addend: 0 };
        tables
            .entries(&base, count, text, &memory, &mut || Ok(()))
            .unwrap()
            .map(|words| {
                assert_eq!(words.source, TableSource::Bound);
                words.entries
            })
    }

    #[test]
    fn a_bounded_table_is_the_labels_its_rela_words_name() {
        let (bytes, table) = object(1);
        assert_eq!(
            entries(&bytes, table, Some(3)),
            Some(vec![0x20, 0x30, 0x38])
        );
        // A shorter bound reads only its entries; a table has no label of
        // its own, so without a bound there is none.
        assert_eq!(entries(&bytes, table, Some(2)), Some(vec![0x20, 0x30]));
        assert_eq!(entries(&bytes, table, None), None);
        // A fourth entry has no relocation: reading stops there.
        assert_eq!(
            entries(&bytes, table, Some(4)),
            Some(vec![0x20, 0x30, 0x38])
        );
    }

    #[test]
    fn only_rela_address_words_name_entries() {
        // `R_RISCV_64` words are not RV32 address words.
        let (bytes, table) = object(2);
        assert_eq!(entries(&bytes, table, Some(3)), Some(vec![]));
        // A REL table keeps its addends in its words, which are not read.
        let (mut rel, table) = object(1);
        let at = header(&rel, ".rela.rodata");
        let field = |bytes: &[u8], offset: usize| {
            u32::from_le_bytes(bytes[at + offset..at + offset + 4].try_into().unwrap()) as usize
        };
        let (start, size) = (field(&rel, 0x10), field(&rel, 0x14));
        for entry in 0..size / 12 {
            let from = start + 12 * entry;
            let to = start + 8 * entry;
            rel.copy_within(from..from + 8, to);
        }
        rel[at + 4..at + 8].copy_from_slice(&9u32.to_le_bytes()); // SHT_REL
        rel[at + 0x14..at + 0x18].copy_from_slice(&((size / 12 * 8) as u32).to_le_bytes());
        rel[at + 0x24..at + 0x28].copy_from_slice(&8u32.to_le_bytes());
        assert_eq!(entries(&rel, table, Some(3)), Some(vec![]));
    }

    #[test]
    fn a_section_two_rela_sections_relocate_names_no_entry() {
        let (mut bytes, table) = object(1);
        let rodata = oer_analysis_elf::Elf::parse(&bytes)
            .unwrap()
            .sections()
            .find(|s| s.name == ".rodata")
            .unwrap()
            .index as u32;
        // `.rela.data` now claims `.rodata` too.
        let at = header(&bytes, ".rela.data");
        bytes[at + 0x1c..at + 0x20].copy_from_slice(&rodata.to_le_bytes());
        assert_eq!(entries(&bytes, table, Some(3)), Some(vec![]));
    }
}
