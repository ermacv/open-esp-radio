use blobray_domain::DiagnosticCode;
use object::{
    Architecture, BinaryFormat, Endianness, LittleEndian, SectionKind, SymbolFlags, SymbolKind,
    SymbolScope, elf,
    read::elf::{FileHeader, SectionHeader},
    write::{Object, Symbol, SymbolSection},
};
use oer_riscv_model::SymbolTableKind;

fn fixture(architecture: Architecture, endian: Endianness) -> Vec<u8> {
    let mut object = Object::new(BinaryFormat::Elf, architecture, endian);
    let section = object.add_section(Vec::new(), b".text".to_vec(), SectionKind::Text);
    object.append_section_data(section, &[0; 8], 4);
    object.add_symbol(Symbol {
        name: b"entry".to_vec(),
        value: 0,
        size: 8,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(section),
        flags: SymbolFlags::None,
    });
    object.write().unwrap()
}

fn inventory(bytes: &[u8]) -> blobray_domain::ObjectInventory {
    collect(bytes, &mut || Ok(())).unwrap()
}
fn collect(
    bytes: &[u8],
    control: &mut dyn oer_riscv_model::RunControl,
) -> oer_riscv_model::Result<blobray_domain::ObjectInventory> {
    use blobray_domain::*;
    use oer_riscv_model::*;
    #[derive(Default)]
    struct Records {
        sections: Vec<SectionRecord>,
        symbols: Vec<SymbolRecord>,
        relocations: Vec<RelocationRecord>,
        diagnostics: Vec<Diagnostic>,
    }
    impl ElfSink for Records {
        fn section(&mut self, record: &SectionRecord, _: &mut dyn RunControl) -> Result<()> {
            self.sections.push(record.clone());
            Ok(())
        }
        fn symbol(&mut self, record: &SymbolRecord, _: &mut dyn RunControl) -> Result<()> {
            self.symbols.push(record.clone());
            Ok(())
        }
        fn relocation(&mut self, record: &RelocationRecord, _: &mut dyn RunControl) -> Result<()> {
            self.relocations.push(record.clone());
            Ok(())
        }
        fn diagnostic(&mut self, record: &Diagnostic, _: &mut dyn RunControl) -> Result<()> {
            self.diagnostics.push(record.clone());
            Ok(())
        }
    }
    let memory = WorkingMemory::new(DEFAULT_WORKING_BYTES)?;
    let id = ObjectId {
        artifact: ArtifactId::of_bytes(bytes),
        location: ObjectLocation::Standalone,
    };
    control.set_position(RunPosition {
        member: Some(0),
        ..Default::default()
    });
    let mut records = Records::default();
    let (content, mut elf) =
        blobray_artifacts::inspect_source(&bytes, &id, &memory, control, &mut records)?;
    if let Some(header) = &mut elf {
        header.sections = records.sections;
        header.symbols = records.symbols;
        header.relocations = records.relocations;
    }
    Ok(ObjectInventory {
        id,
        name: None,
        content: Some(content),
        elf,
        diagnostics: records.diagnostics,
    })
}

fn symbol_offsets(bytes: &[u8]) -> (usize, usize) {
    let header = elf::FileHeader32::<LittleEndian>::parse(bytes).unwrap();
    let sections = header.sections(LittleEndian, bytes).unwrap();
    let (index, section) = sections
        .enumerate()
        .find(|(_, s)| s.sh_type(LittleEndian) == elf::SHT_SYMTAB)
        .unwrap();
    (
        header.e_shoff(LittleEndian) as usize
            + index.0 * usize::from(header.e_shentsize(LittleEndian)),
        section.sh_offset(LittleEndian) as usize,
    )
}

#[test]
fn malformed_symbol_name_keeps_raw_entry_and_following_symbols() {
    let mut bytes = fixture(Architecture::Riscv32, Endianness::Little);
    let (_, table) = symbol_offsets(&bytes);
    let before = inventory(&bytes);
    bytes[table + 16..table + 20].copy_from_slice(&u32::MAX.to_le_bytes());
    bytes[table + 28] = 0xde; // Unknown binding/type must survive structural inventory.
    let after = inventory(&bytes);
    let symbols = &after.elf.as_ref().unwrap().symbols;
    assert_eq!(symbols.len(), before.elf.unwrap().symbols.len());
    assert_eq!(symbols[1].name, None);
    assert_eq!(symbols[1].name_offset, u32::MAX);
    assert_eq!((symbols[1].binding, symbols[1].symbol_type), (13, 14));
    assert!(
        after
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::MalformedName)
    );
}

#[test]
fn missing_string_table_keeps_raw_symbol_inventory() {
    let mut bytes = fixture(Architecture::Riscv32, Endianness::Little);
    let (section, _) = symbol_offsets(&bytes);
    let before = inventory(&bytes).elf.unwrap().symbols.len();
    bytes[section + 24..section + 28].copy_from_slice(&u32::MAX.to_le_bytes());
    let after = inventory(&bytes);
    assert_eq!(after.elf.as_ref().unwrap().symbols.len(), before);
    assert!(after.elf.unwrap().symbols.iter().all(|s| s.name.is_none()));
    assert!(
        after
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::MalformedTable)
    );
}

#[test]
fn dynamic_symbols_have_distinct_table_kind_and_null_entry() {
    let mut bytes = fixture(Architecture::Riscv32, Endianness::Little);
    let (section, _) = symbol_offsets(&bytes);
    bytes[section + 4..section + 8].copy_from_slice(&elf::SHT_DYNSYM.to_le_bytes());
    let after = inventory(&bytes);
    assert!(after.diagnostics.is_empty(), "{:?}", after.diagnostics);
    let symbols = &after.elf.as_ref().unwrap().symbols;
    assert_eq!(symbols[0].id.index, 0);
    assert!(
        symbols
            .iter()
            .all(|s| s.id.table == SymbolTableKind::Dynamic)
    );
}

#[test]
fn elf64_big_endian_is_not_narrowed_to_selected_rv32_target() {
    let bytes = fixture(Architecture::PowerPc64, Endianness::Big);
    let object = inventory(&bytes);
    assert!(object.diagnostics.is_empty(), "{:?}", object.diagnostics);
    let elf = object.elf.unwrap();
    assert_eq!(elf.bits, 64);
    assert!(!elf.little_endian);
    assert_eq!(elf.machine, object::elf::EM_PPC64);
}

#[test]
fn malformed_table_stride_does_not_claim_complete_inventory() {
    let mut bytes = fixture(Architecture::Riscv32, Endianness::Little);
    let (section, _) = symbol_offsets(&bytes);
    bytes[section + 36..section + 40].copy_from_slice(&1u32.to_le_bytes());
    let result = inventory(&bytes);
    assert!(!result.diagnostics.is_empty());
    assert!(result.elf.is_some());
    assert!(
        result
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::MalformedTable)
    );
}

#[test]
fn invalid_symbol_section_remains_raw_and_marks_inventory_incomplete() {
    let mut bytes = fixture(Architecture::Riscv32, Endianness::Little);
    let (_, table) = symbol_offsets(&bytes);
    bytes[table + 30..table + 32].copy_from_slice(&100u16.to_le_bytes());
    let result = inventory(&bytes);
    assert!(!result.diagnostics.is_empty());
    let object = &result;
    assert_eq!(object.elf.as_ref().unwrap().symbols[1].raw_section, 100);
    assert!(
        object
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::MalformedTable)
    );
}

/// A caller stops a selected physical operation; the parser must propagate that
/// stop instead of publishing a malformed/partial inventory.
struct StopAt {
    position: oer_riscv_model::RunPosition,
    member: Option<u64>,
    entry: Option<u64>,
    remaining: u64,
}
impl oer_riscv_model::RunControl for StopAt {
    fn position(&self) -> oer_riscv_model::RunPosition {
        self.position
    }
    fn set_position(&mut self, position: oer_riscv_model::RunPosition) {
        self.position = position;
    }
    fn checkpoint(&mut self, units: u64) -> oer_riscv_model::Result<()> {
        let matches = self.entry.map_or(
            self.position.phase == oer_riscv_model::RunPhase::Members,
            |entry| {
                self.position.phase == oer_riscv_model::RunPhase::Elf
                    && self.position.entry == Some(entry)
            },
        );
        if matches && self.member.is_none_or(|m| self.position.member == Some(m)) {
            if units > self.remaining {
                return Err(oer_riscv_model::Error::new(
                    oer_riscv_model::ErrorCode::ResourceLimited,
                    "caller exhausted work",
                ));
            }
            self.remaining -= units;
        }
        Ok(())
    }
}
#[test]
fn work_stop_in_symbol_table_is_not_a_malformed_object() {
    let bytes = fixture(Architecture::Riscv32, Endianness::Little);
    let mut control = StopAt {
        position: Default::default(),
        member: Some(0),
        entry: Some(1),
        remaining: 0,
    };
    let error = collect(&bytes, &mut control).unwrap_err();
    assert_eq!(error.code, oer_riscv_model::ErrorCode::ResourceLimited);
    assert_eq!(control.position.entry, Some(1));
    assert!(control.position.table.is_some());
}
#[test]
fn long_symbol_name_is_interruptible_inside_the_scan() {
    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    object.add_symbol(Symbol {
        name: vec![b'x'; 8 * oer_riscv_model::WORK_BLOCK],
        value: 0,
        size: 0,
        kind: SymbolKind::Unknown,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Undefined,
        flags: SymbolFlags::None,
    });
    let bytes = object.write().unwrap();
    let mut control = StopAt {
        position: Default::default(),
        member: Some(0),
        entry: Some(1),
        remaining: 3,
    };
    let error = collect(&bytes, &mut control).unwrap_err();
    assert_eq!(error.code, oer_riscv_model::ErrorCode::ResourceLimited);
    assert_eq!(control.position.entry, Some(1));
    assert_eq!(control.remaining, 0);
}
#[test]
fn stopping_member_enumeration_never_returns_partial_success() {
    let mut bytes = b"!<arch>\n".to_vec();
    for name in ["one/", "two/"] {
        bytes.extend_from_slice(
            format!(
                "{name:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
                "0", "0", "0", "644", "0"
            )
            .as_bytes(),
        );
    }
    let mut control = StopAt {
        position: Default::default(),
        member: Some(1),
        entry: None,
        remaining: 0,
    };
    let memory = oer_riscv_model::WorkingMemory::new(1024).unwrap();
    let source = bytes.as_slice();
    let mut cursor = blobray_artifacts::MemberCursor::new(&source, &mut control).unwrap();
    assert!(cursor.next(&memory, &mut control).unwrap().is_some());
    assert!(matches!(
        cursor.next(&memory, &mut control),
        Err(oer_riscv_model::Error {
            code: oer_riscv_model::ErrorCode::ResourceLimited,
            ..
        })
    ));
    assert_eq!(control.position.member, Some(1));
}

#[test]
fn stream_consumer_failure_is_not_malformed_elf_coverage() {
    use blobray_domain::*;
    use oer_riscv_model::*;
    struct Reject;
    impl ElfSink for Reject {
        fn section(&mut self, _: &SectionRecord, _: &mut dyn RunControl) -> Result<()> {
            Err(Error::new(
                ErrorCode::Integrity,
                "consumer rejected a record",
            ))
        }
        fn symbol(&mut self, _: &SymbolRecord, _: &mut dyn RunControl) -> Result<()> {
            Ok(())
        }
        fn relocation(&mut self, _: &RelocationRecord, _: &mut dyn RunControl) -> Result<()> {
            Ok(())
        }
        fn diagnostic(&mut self, _: &Diagnostic, _: &mut dyn RunControl) -> Result<()> {
            Ok(())
        }
    }
    let bytes = fixture(Architecture::Riscv32, Endianness::Little);
    let id = ObjectId {
        artifact: ArtifactId::of_bytes(&bytes),
        location: ObjectLocation::Standalone,
    };
    let memory = WorkingMemory::new(1024 * 1024).unwrap();
    let error =
        blobray_artifacts::inspect_payload(&bytes, &id, &memory, &mut || Ok(()), &mut Reject)
            .unwrap_err();
    assert_eq!(error.code, ErrorCode::Integrity);
    assert_eq!(error.message, "consumer rejected a record");
    assert_eq!(memory.used(), 0);
}
