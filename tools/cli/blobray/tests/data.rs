//! In-process export of exact captured data, named by content.
#![cfg(target_os = "linux")]
#[allow(dead_code)]
mod support;
use blobray_application::data::{DataExport, export};
use blobray_application::in_process::{Executable, Limits};
use blobray_domain::*;
use object::{
    Architecture, BinaryFormat, Endianness, SectionKind, SymbolFlags, SymbolKind, SymbolScope,
    write::{Object, Symbol, SymbolSection},
};
use oer_riscv_model::*;

fn symbol(name: &[u8], section: SymbolSection, size: u64, kind: SymbolKind) -> Symbol {
    Symbol {
        name: name.into(),
        value: 0,
        size,
        kind,
        scope: SymbolScope::Linkage,
        weak: false,
        section,
        flags: SymbolFlags::None,
    }
}

/// An object with read-only `table`, initialized `initial` and zeroed `empty`.
fn data_object() -> Vec<u8> {
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = obj.add_section(Vec::new(), b".text.entry".to_vec(), SectionKind::Text);
    obj.append_section_data(text, &[0x13, 0x05, 0xa0, 0x02, 0x67, 0x80, 0, 0], 2);
    obj.add_symbol(symbol(
        b"entry",
        SymbolSection::Section(text),
        8,
        SymbolKind::Text,
    ));
    let data = obj.add_section(
        Vec::new(),
        b".rodata.table".to_vec(),
        SectionKind::ReadOnlyData,
    );
    obj.append_section_data(data, &[0xfb, 0xff, 3, 0], 2);
    obj.add_symbol(symbol(
        b"table",
        SymbolSection::Section(data),
        4,
        SymbolKind::Data,
    ));
    let initial = obj.add_section(Vec::new(), b".data.initial".to_vec(), SectionKind::Data);
    obj.append_section_data(initial, &[10, 20, 30, 40], 1);
    obj.add_symbol(symbol(
        b"initial",
        SymbolSection::Section(initial),
        4,
        SymbolKind::Data,
    ));
    let bss = obj.add_section(
        Vec::new(),
        b".bss.empty".to_vec(),
        SectionKind::UninitializedData,
    );
    obj.append_section_bss(bss, 16, 4);
    obj.add_symbol(symbol(
        b"empty",
        SymbolSection::Section(bss),
        16,
        SymbolKind::Data,
    ));
    obj.write().unwrap()
}

fn objects(executable: &Executable) -> Vec<ObjectInventory> {
    blobray_application::captured::inventory(
        executable,
        &WorkingMemory::new(16 * 1024 * 1024).unwrap(),
        &mut Limits::new(DEFAULT_WORK_UNITS, std::time::Duration::from_secs(30)),
    )
    .unwrap()
    .objects
}

fn named(object: &ObjectInventory, name: &[u8]) -> SymbolId {
    object
        .elf
        .as_ref()
        .unwrap()
        .symbols
        .iter()
        .find(|s| s.name.as_deref() == Some(name))
        .unwrap()
        .id
        .clone()
}

fn read(request: &DataRequest, executables: &[Executable], memory: u64) -> Result<DataExport> {
    export(
        request,
        executables,
        &WorkingMemory::new(memory).unwrap(),
        &mut Limits::new(DEFAULT_WORK_UNITS, std::time::Duration::from_secs(30)),
    )
}

#[test]
fn symbol_ranges_of_an_archive_member_export_exact_initialization_bytes() {
    let raw = data_object();
    let archive = Executable::new(support::archive(
        &[(b"other.o", &support::elf()), (b"data.o", &raw)],
        false,
    ));
    let member = &objects(&archive)[1];
    let request = DataRequest {
        object: member.id.clone(),
        symbol: Some(named(member, b"entry")),
        ranges: [&b"table"[..], b"initial"]
            .iter()
            .map(|name| DataSelector::Symbol {
                symbol: named(member, name),
                length: None,
            })
            .collect(),
    };
    let exported = read(&request, std::slice::from_ref(&archive), 16 * 1024 * 1024).unwrap();
    assert_eq!(exported.payload, ArtifactId::of_bytes(&raw));
    assert_eq!(exported.bytes, [0xfb, 0xff, 3, 0, 10, 20, 30, 40]);
    assert!(!exported.spans[0].writable);
    assert!(exported.spans[1].writable);
    assert_eq!(exported.spans[1].export_offset, 4);
    for span in &exported.spans {
        let range = span.file_range;
        assert_eq!(
            ArtifactId::of_bytes(&raw[range.start as usize..(range.start + range.length) as usize]),
            span.digest
        );
    }
    // A symbol of another member of the same archive selects nothing here.
    let mut wrong = request.clone();
    if let DataSelector::Symbol { symbol, .. } = &mut wrong.ranges[0] {
        symbol.object.location = ObjectLocation::ArchiveMember { ordinal: 0 };
    }
    assert!(read(&wrong, std::slice::from_ref(&archive), 16 * 1024 * 1024).is_err());
    // The object's executable must be given.
    assert_eq!(
        read(&request, &[], 16 * 1024 * 1024).unwrap_err().code,
        ErrorCode::InvalidRequest
    );
}

#[test]
fn nobits_overflow_bad_selections_and_exhaustion_are_rejected() {
    let archive = Executable::new(support::archive(&[(b"data.o", &data_object())], false));
    let member = &objects(&archive)[0];
    let base = |ranges: Vec<DataSelector>| DataRequest {
        object: member.id.clone(),
        symbol: None,
        ranges,
    };
    let table = named(member, b"table");
    for request in [
        base(vec![DataSelector::Symbol {
            symbol: named(member, b"empty"),
            length: None,
        }]),
        base(vec![DataSelector::Symbol {
            symbol: table.clone(),
            length: Some(u64::MAX),
        }]),
        base(vec![DataSelector::Section {
            section: 1,
            offset: u64::MAX,
            length: 1,
        }]),
        base(vec![]),
        base(vec![
            DataSelector::Symbol {
                symbol: table.clone(),
                length: None
            };
            33
        ]),
    ] {
        assert!(read(&request, std::slice::from_ref(&archive), 16 * 1024 * 1024).is_err());
    }
    let request = base(vec![DataSelector::Symbol {
        symbol: table,
        length: None,
    }]);
    assert_eq!(
        read(&request, std::slice::from_ref(&archive), 1024)
            .unwrap_err()
            .code,
        ErrorCode::ResourceLimited
    );
}

#[test]
fn image_addresses_export_file_backing_and_reject_unmapped_ranges() {
    let image = Executable::new(support::executable_with_symbols(
        &[0x00008067, 0x12345678],
        &[("entry", 0x1000, 8)],
    ));
    let object = ObjectId {
        artifact: image.id().clone(),
        location: ObjectLocation::Standalone,
    };
    let request = |address: u64| DataRequest {
        object: object.clone(),
        symbol: None,
        ranges: vec![DataSelector::Image { address, length: 4 }],
    };
    let exported = read(
        &request(0x1004),
        std::slice::from_ref(&image),
        16 * 1024 * 1024,
    )
    .unwrap();
    assert_eq!(exported.payload, *image.id());
    assert_eq!(exported.bytes, 0x12345678u32.to_le_bytes());
    let span = &exported.spans[0];
    assert_eq!(span.image_address, Some(0x1004));
    assert_eq!(span.file_range.start, 260);
    assert!(
        read(
            &request(0xffff_f000),
            std::slice::from_ref(&image),
            16 * 1024 * 1024
        )
        .is_err()
    );
}
