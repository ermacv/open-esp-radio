//! The facts of one static RV32 executable that more than one analysis
//! reads: its functions, executable ranges, read-only words, sized data
//! objects, the address words an `--emit-relocs` link kept and the
//! compiler's jump-table labels. One parse reads them all under the caller's
//! working memory and run control; lookups never parse again.
use oer_tool_analysis_elf::rv32::{self, Role};
use oer_tool_analysis_elf::{Function, SymbolKind};
use oer_tool_analysis_riscv_model::*;

pub struct ImageFacts<'a> {
    bytes: &'a [u8],
    file: oer_tool_analysis_elf::Elf<'a>,
    functions: Vec<Function>,
    /// Admits `functions` with every name it holds.
    _functions: MemoryReservation<'a>,
    executable: AdmittedVec<'a, (u32, u32)>,
    /// Allocated sections with file contents, neither executable nor
    /// writable unless a read-only window covers them: address, size and
    /// bytes.
    read_only: AdmittedVec<'a, (u64, u64, &'a [u8])>,
    /// Sized data objects whose size is a multiple of a word: the first
    /// symbol at each address, by address.
    objects: AdmittedVec<'a, (u64, u64)>,
    /// Relocated address words of allocated data: address and the address
    /// it holds, by address.
    words: AdmittedVec<'a, (u32, u32)>,
    /// Starts of the compiler's jump tables (`.LJTI*` labels) and the next
    /// symbol of their section or its end, by start.
    jump_tables: AdmittedVec<'a, (u32, u32)>,
}

fn invalid(message: impl Into<String>) -> Error {
    Error::new(ErrorCode::Integrity, message)
}

/// Keep the last entry of each run of equal keys in `values`, sorted stably
/// by key: a later fact replaces an earlier one at the same address.
fn last_by_key<K: Ord + Copy, T: Copy>(values: &mut AdmittedVec<'_, T>, key: impl Fn(&T) -> K) {
    values.sort_by_key(&key);
    let mut kept = 0;
    for at in 0..values.len() {
        if kept > 0 && key(&values[kept - 1]) == key(&values[at]) {
            values[kept - 1] = values[at];
        } else {
            values[kept] = values[at];
            kept += 1;
        }
    }
    values.truncate(kept);
}

impl<'a> ImageFacts<'a> {
    /// Read the facts of the static little-endian RV32 executable `bytes`,
    /// whose sections inside the caller's `read_only` windows hold constants
    /// whatever their flags say.
    pub fn new(
        bytes: &'a [u8],
        read_only: &[ReadOnlyWindow],
        memory: &'a WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<Self> {
        c.bytes(bytes.len())?;
        let file = oer_tool_analysis_elf::Elf::executable(bytes)
            .map_err(|_| invalid("a static little-endian RV32 executable is required"))?;
        // `functions` holds at most one entry and one name per symbol.
        let (mut symbols, mut names) = (0u64, 0u64);
        for symbol in file.symbols() {
            c.checkpoint(1)?;
            symbols += 1;
            names += symbol.name.len() as u64;
        }
        let functions_bytes = symbols
            .saturating_mul((size_of::<Function>() + size_of::<String>()) as u64)
            .saturating_add(names);
        let reservation = memory.reserve(functions_bytes, c.position())?;
        let functions = file
            .functions()
            .map_err(|error| invalid(error.to_string()))?;
        let mut facts = Self {
            functions,
            _functions: reservation,
            executable: AdmittedVec::new(memory),
            read_only: AdmittedVec::new(memory),
            objects: AdmittedVec::new(memory),
            words: AdmittedVec::new(memory),
            jump_tables: AdmittedVec::new(memory),
            bytes,
            file,
        };
        facts.sections(read_only, c)?;
        facts.symbols(memory, c)?;
        Ok(facts)
    }

    /// Executable ranges, read-only sections and relocated words.
    fn sections(&mut self, read_only: &[ReadOnlyWindow], c: &mut dyn RunControl) -> Result<()> {
        let file = &self.file;
        for section in file.sections() {
            c.checkpoint(1)?;
            if section.executable {
                let start =
                    u32::try_from(section.address).map_err(|_| invalid("section beyond RV32"))?;
                let end = u32::try_from(section.address + section.size)
                    .map_err(|_| invalid("section beyond RV32"))?;
                self.executable.push((start, end), c.position())?;
            }
            if section.allocated
                && !section.executable
                && (!section.writable
                    || ReadOnlyWindow::cover(read_only, section.address, section.size))
            {
                self.read_only
                    .push((section.address, section.size, section.data), c.position())?;
            }
            if !section.allocated || section.executable || section.data.is_empty() {
                continue;
            }
            let relocations = file
                .relocations(section.index)
                .map_err(|error| invalid(error.to_string()))?;
            for relocation in relocations {
                c.checkpoint(1)?;
                if rv32::kind(relocation.r_type).role != Role::Word {
                    continue;
                }
                let target = file
                    .target_address(&relocation)
                    .map_err(|error| invalid(error.to_string()))?;
                let target =
                    u32::try_from(target).map_err(|_| invalid("relocation target beyond RV32"))?;
                let address =
                    u32::try_from(relocation.at).map_err(|_| invalid("relocation beyond RV32"))?;
                let held = section.word(relocation.at).ok_or_else(|| {
                    invalid(format!("relocation at {address:#010x} outside its section"))
                })?;
                if held != target {
                    return Err(invalid(format!(
                        "relocation at {address:#010x} names {target:#010x}, the word holds {held:#010x}"
                    )));
                }
                self.words.push((address, target), c.position())?;
            }
        }
        c.checkpoint(self.words.len() as u64)?;
        last_by_key(&mut self.words, |&(address, _)| address);
        Ok(())
    }

    /// Sized data objects and jump-table labels.
    fn symbols(&mut self, memory: &'a WorkingMemory, c: &mut dyn RunControl) -> Result<()> {
        let file = &self.file;
        // Every symbol address of each section, to find where a table ends.
        let mut addresses: AdmittedVec<'_, (usize, u64)> = AdmittedVec::new(memory);
        for symbol in file.symbols() {
            c.checkpoint(1)?;
            if let Some(index) = symbol.section {
                addresses.push((index, symbol.address), c.position())?;
            }
            if symbol.kind == SymbolKind::Data && symbol.size > 0 && symbol.size % 4 == 0 {
                self.objects
                    .push((symbol.address, symbol.size), c.position())?;
            }
        }
        c.checkpoint((addresses.len() + self.objects.len()) as u64)?;
        addresses.sort_unstable();
        // The first symbol at an address names its object.
        self.objects.sort_by_key(|&(address, _)| address);
        let mut kept = 0;
        for at in 0..self.objects.len() {
            if kept == 0 || self.objects[kept - 1].0 != self.objects[at].0 {
                self.objects[kept] = self.objects[at];
                kept += 1;
            }
        }
        self.objects.truncate(kept);
        for symbol in file.symbols() {
            c.checkpoint(1)?;
            if !symbol.name.starts_with(".LJTI") {
                continue;
            }
            let start = symbol.address;
            let Some(index) = symbol.section else {
                continue;
            };
            let Ok(section) = file.section(index) else {
                continue;
            };
            let after = addresses.partition_point(|&entry| entry <= (index, start));
            let next = addresses
                .get(after)
                .filter(|&&(other, _)| other == index)
                .map_or(section.address + section.size, |&(_, address)| address);
            let (Ok(start), Ok(next)) = (u32::try_from(start), u32::try_from(next)) else {
                continue;
            };
            self.jump_tables.push((start, next), c.position())?;
        }
        c.checkpoint(self.jump_tables.len() as u64)?;
        last_by_key(&mut self.jump_tables, |&(start, _)| start);
        Ok(())
    }

    /// The file's bytes.
    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }

    /// The parsed file.
    pub fn file(&self) -> &oer_tool_analysis_elf::Elf<'a> {
        &self.file
    }

    /// The defined code symbols as functions, ascending by address
    /// ([`oer_tool_analysis_elf::Elf::functions`]).
    pub fn functions(&self) -> &[Function] {
        &self.functions
    }

    /// Address ranges of the executable sections.
    pub fn executable_ranges(&self) -> &[(u32, u32)] {
        &self.executable
    }

    /// The little-endian word at `address` of an allocated section with file
    /// contents that is neither writable nor executable, such as `.rodata`,
    /// or writable and covered whole by a read-only window given to
    /// [`ImageFacts::new`], such as ESP-IDF's flash rodata: code loaded into
    /// RAM may hold tables the program rewrites, such as a vector table in an
    /// `AX` section.
    pub fn read_only_word(&self, address: u32) -> Option<u32> {
        let address = u64::from(address);
        self.read_only.iter().find_map(|&(start, size, data)| {
            if address < start || address + 4 > start + size {
                return None;
            }
            let at = usize::try_from(address - start).ok()?;
            let bytes = data.get(at..at + 4)?;
            Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
        })
    }

    /// The size of the sized data object at `address` whose size is a
    /// multiple of a word.
    pub fn data_object(&self, address: u32) -> Option<u64> {
        let address = u64::from(address);
        let at = self
            .objects
            .binary_search_by_key(&address, |&(start, _)| start)
            .ok()?;
        Some(self.objects[at].1)
    }

    /// Every relocated word: its address and the address it holds,
    /// ascending by address.
    pub fn words(&self) -> &[(u32, u32)] {
        &self.words
    }

    /// The address the relocated word at `address` holds, if a relocation
    /// names it.
    pub fn word(&self, address: u32) -> Option<u32> {
        let at = self
            .words
            .binary_search_by_key(&address, |&(at, _)| at)
            .ok()?;
        Some(self.words[at].1)
    }

    /// Whether the compiler labelled a jump table at `base`.
    pub fn is_jump_table(&self, base: u32) -> bool {
        self.jump_tables
            .binary_search_by_key(&base, |&(start, _)| start)
            .is_ok()
    }

    /// The entries of the jump table the compiler labelled at `base`: its
    /// relocated words up to the first word without one or the next symbol.
    pub fn jump_table(&self, base: u32) -> Option<impl Iterator<Item = u32> + '_> {
        let at = self
            .jump_tables
            .binary_search_by_key(&base, |&(start, _)| start)
            .ok()?;
        let end = self.jump_tables[at].1;
        let first = self.words.partition_point(|&(address, _)| address < base);
        let entries = self.words[first..]
            .iter()
            .zip((base..end).step_by(4))
            .take_while(|&(&(address, _), expected)| address == expected)
            .map(|(&(_, holds), _)| holds);
        (self.word(base).is_some() && base < end).then_some(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{A, PF_R, PF_W, W, image};

    /// The read-only word at 0x3000 of an image whose one data section
    /// there has `flags`, under `windows`.
    fn word(flags: u32, windows: &[ReadOnlyWindow]) -> Option<u32> {
        let bytes = image(
            &[(0x3000, 0, 8, 8, PF_R | PF_W)],
            None,
            &[(1, flags, 0x3000, 0, 8)],
            &[0x11, 0x22, 0x33, 0x44, 0, 0, 0, 0],
        );
        let memory = WorkingMemory::new(1 << 20).unwrap();
        let facts = ImageFacts::new(&bytes, windows, &memory, &mut || Ok(())).unwrap();
        facts.read_only_word(0x3000)
    }

    #[test]
    fn a_writable_section_is_read_only_only_inside_a_window_covering_it() {
        assert_eq!(word(A, &[]), Some(0x4433_2211));
        assert_eq!(word(A | W, &[]), None);
        let window = |length| ReadOnlyWindow {
            start: 0x3000,
            length,
        };
        assert_eq!(word(A | W, &[window(0x1000)]), Some(0x4433_2211));
        // A window covering part of the section leaves it writable.
        assert_eq!(word(A | W, &[window(4)]), None);
    }
}
