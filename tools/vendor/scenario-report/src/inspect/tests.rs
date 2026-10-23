#![allow(clippy::expect_used, clippy::indexing_slicing)]
use super::*;
use object::{
    Architecture, BinaryFormat, Endianness, RelocationFlags, SectionKind, SymbolFlags, SymbolKind,
    SymbolScope,
    write::{Object, Relocation, Symbol, SymbolSection},
};

fn object(name: &str, words: &[&str], format: bool) -> Vec<u8> {
    let code: Vec<_> = words
        .iter()
        .flat_map(|word| {
            let bytes = u32::from_str_radix(word, 16)
                .expect("instruction word")
                .to_le_bytes();
            bytes[..word.len() / 2].to_vec()
        })
        .collect();
    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = object.add_section(vec![], b".text".to_vec(), SectionKind::Text);
    object.append_section_data(text, &code, 2);
    object.add_symbol(Symbol {
        name: name.as_bytes().to_vec(),
        value: 0,
        size: code.len() as u64,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(text),
        flags: SymbolFlags::None,
    });
    if format {
        let data = object.add_section(
            vec![],
            b".rodata.str1.1".to_vec(),
            SectionKind::ReadOnlyString,
        );
        object.append_section_data(data, b"gain=%d\n\0", 1);
        let format = object.add_symbol(Symbol {
            name: b".Lformat".to_vec(),
            value: 0,
            size: 9,
            kind: SymbolKind::Data,
            scope: SymbolScope::Compilation,
            weak: false,
            section: SymbolSection::Section(data),
            flags: SymbolFlags::None,
        });
        let printf = object.add_symbol(Symbol {
            name: b"printf".to_vec(),
            value: 0,
            size: 0,
            kind: SymbolKind::Text,
            scope: SymbolScope::Linkage,
            weak: false,
            section: SymbolSection::Undefined,
            flags: SymbolFlags::None,
        });
        for (offset, symbol, kind) in [
            (0x14, format, object::elf::R_RISCV_HI20),
            (0x18, format, object::elf::R_RISCV_LO12_I),
            (0x1c, printf, object::elf::R_RISCV_CALL_PLT),
        ] {
            object
                .add_relocation(
                    text,
                    Relocation {
                        offset,
                        symbol,
                        addend: 0,
                        flags: RelocationFlags::Elf { r_type: kind },
                    },
                )
                .expect("fixture relocation");
        }
    }
    object.write().expect("fixture ELF")
}

fn corpus(name: &str, words: &[&str]) -> Corpus {
    let bytes = object(name, words, false);
    Corpus::of(&[("test", &bytes)], Registers::default()).expect("fixture corpus")
}
const SYNC: &[&str] = &[
    "2010e7b7", "83078793", "4398", "080006b7", "8f55", "c398", "8082",
];

#[test]
fn xref_formats_shared_read_modify_write_facts_and_filters_the_exact_range() {
    let corpus = corpus("sync_enable", SYNC);
    let inside = xref(&corpus, 0x2010_d830, 0x2010_d834).expect("xref");
    assert_eq!(inside.lines().count(), 2);
    assert!(
        inside.contains("W test::sync_enable+0x10  sets 0x2010d830: bits 27"),
        "{inside}"
    );
    assert!(
        xref(&corpus, 0x2010_d834, 0x2010_d840)
            .expect("outside")
            .is_empty()
    );
}

#[test]
fn floating_point_memory_accesses_stay_visible() {
    let corpus = corpus(
        "float",
        &["2010e7b7", "83078793", "0007a507", "00a7a227", "8082"],
    );
    let text = xref(&corpus, 0x2010_d830, 0x2010_d838).expect("floating memory");
    assert!(text.contains(" R test::float+0x8"), "{text}");
    assert!(text.contains(" W test::float+0xc"), "{text}");
}

#[test]
fn show_preserves_each_definition_constants_and_store_bits() {
    let bytes = object("sync_enable", SYNC, false);
    let corpus = Corpus::of(
        &[("first", &bytes), ("second", &bytes)],
        Registers::default(),
    )
    .expect("two definitions");
    let text = show(&corpus, "sync_enable").expect("show").expect("found");
    assert!(text.contains("== first::sync_enable"));
    assert!(text.contains("== second::sync_enable"));
    assert!(text.contains("= 0x2010d830"));
    assert!(text.contains("sets 0x2010d830: bits 27"));
    assert!(show(&corpus, "absent").expect("missing name").is_none());
}

#[test]
fn fields_use_blobrays_pointer_path_and_store_provenance() {
    let corpus = corpus(
        "set_flag",
        &[
            "03452783", "0007a703", "004006b7", "00d76733", "00e7a023", "8082",
        ],
    );
    let text = fields(&corpus, &[0x34, 0]).expect("field path");
    assert!(
        text.contains("a0->0x34->0x0 W test::set_flag+0x10  sets bits 22"),
        "{text}"
    );
    assert!(fields(&corpus, &[0x38, 0]).expect("other path").is_empty());
}

#[test]
fn fields_preserve_relocated_pointer_loads_only_when_requested() {
    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = object.add_section(vec![], b".text.pointer".to_vec(), SectionKind::Text);
    // a5 = *(&state); a5 = *(a5 + 0x34); return *a5.
    let mut bytes: Vec<_> = [0x0000_07b7u32, 0x0007_a783, 0x0347_a783, 0x0007_a503]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect();
    bytes.extend(0x8082u16.to_le_bytes());
    object.append_section_data(text, &bytes, 2);
    object.add_symbol(Symbol {
        name: b"global_field".to_vec(),
        value: 0,
        size: bytes.len() as u64,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(text),
        flags: SymbolFlags::None,
    });
    let state = object.add_symbol(Symbol {
        name: b"state".to_vec(),
        value: 0,
        size: 4,
        kind: SymbolKind::Data,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Undefined,
        flags: SymbolFlags::None,
    });
    for (offset, r_type) in [
        (0, object::elf::R_RISCV_HI20),
        (4, object::elf::R_RISCV_LO12_I),
    ] {
        object
            .add_relocation(
                text,
                Relocation {
                    offset,
                    symbol: state,
                    addend: 0,
                    flags: RelocationFlags::Elf { r_type },
                },
            )
            .expect("global pointer relocation");
    }
    let bytes = object.write().expect("global pointer ELF");
    let corpus = Corpus::of(&[("test", &bytes)], Registers::default()).expect("pointer corpus");
    for summaries in [false, true] {
        let memory = WorkingMemory::new(64 << 20).expect("fixture memory");
        let mut load = None;
        app::library::analyze_library(
            &corpus.inputs,
            app::library::Assumptions {
                call_summaries: summaries,
                ..Default::default()
            },
            &RiscvDecoder,
            &memory,
            &mut || Ok(()),
            &mut |result, _| {
                if let app::library::LibraryOutcome::Analyzed(analyzed) = result {
                    load = analyzed.records.iter().find_map(|record| match record {
                        FunctionRecord::Value {
                            offset: 4, value, ..
                        } => Some(value.clone()),
                        _ => None,
                    });
                }
                Ok(())
            },
        )
        .expect("default pointer analysis");
        assert_eq!(load, Some(oer_analysis_riscv_model::AbstractValue::Unknown));
    }
    let text = fields(&corpus, &[0x34, 0]).expect("relocated pointer field");
    assert!(
        text.contains("&state->0x0->0x34->0x0 R test::global_field+0xc"),
        "{text}"
    );
}

#[test]
fn fields_preserve_absolute_loaded_pointer_chains_only_when_requested() {
    let corpus = corpus(
        "absolute_field",
        &[
            "2f0807b7", "f587a783", "3747a783", "0347a783", "0007a503", "8082",
        ],
    );
    let memory = WorkingMemory::new(64 << 20).expect("fixture memory");
    let mut accesses = Vec::new();
    app::library::field_accesses(
        &corpus.inputs,
        app::library::Assumptions::default(),
        0,
        None,
        &RiscvDecoder,
        &memory,
        &mut || Ok(()),
        &mut |result, _| {
            if let app::library::FunctionResult::Matched(function) = result {
                accesses.extend(function.accesses.iter().cloned());
            }
            Ok(())
        },
    )
    .expect("default fields");
    assert!(accesses.is_empty(), "{accesses:?}");
    assert!(!app::library::Assumptions::default().pointer_chains);
    let text = fields(&corpus, &[0x34, 0]).expect("absolute pointer field");
    assert!(
        text.contains("0x2f07ff58->0x374->0x34->0x0 R test::absolute_field+0x10"),
        "{text}"
    );
}

#[test]
fn prints_use_shared_call_wiring_and_relocated_strings_for_calls_and_tail_calls() {
    for (upper, jump) in [("00000097", "000080e7"), ("00000317", "00030067")] {
        let bytes = object(
            "gauge",
            &[
                "2010e7b7", "83078793", "0007a583", "0045d593", "00f5f593", "00000537", "00050513",
                upper, jump, "8082",
            ],
            true,
        );
        let corpus = Corpus::of(&[("test", &bytes)], Registers::default()).expect("print corpus");
        let shown = show(&corpus, "gauge")
            .expect("string listing")
            .expect("gauge");
        assert!(shown.contains("gain=%d\\n"), "{shown}");
        let text = prints(&corpus, 0x2010_d830, 0x2010_d834).expect("prints");
        assert!(
            text.contains("bits 4,5,6,7 >> 4 -> \"gain=%d\" test::gauge+0x20"),
            "{text}"
        );
        assert!(
            prints(&corpus, 0x2010_d834, 0x2010_d838)
                .expect("other word")
                .is_empty()
        );
    }
}
