//! One capture through the analyzer, Blobray callback, human renderer and JSON.
use object::{
    Architecture, BinaryFormat, Endianness, RelocationFlags, SectionKind, SymbolFlags, SymbolKind,
    SymbolScope,
    write::{Object, Relocation, Symbol, SymbolSection},
};
use oer_analysis_blobray_application::{
    in_process::Executable,
    library::{Assumptions, LibraryOutcome, analyze_library},
};
use oer_analysis_riscv_model::{JumpDispatch, WorkingMemory};
use oer_tool_analysis_cli::{jump_table::human, wire::NamedFunction};

fn object(alternative: bool) -> Vec<u8> {
    let mut code: Vec<u32> = vec![
        0x0000_0737, // lui a4, %hi(table)
        0x0007_0713, // addi a4, a4, %lo(table)
        0x0006_1663, // bnez a2, 0x14
        0x0007_0713, // keep the first table
        0x0080_006f, // j 0x18
        if alternative {
            0x00c7_0713
        } else {
            0x0007_0713
        }, // second table at +12
        0x0020_0593, // li a1, 2
        0x02a5_e263, // bltu a1, a0, 0x40
        0x0025_1513, // slli a0, a0, 2
        0x00e5_0533, // add a0, a0, a4
        0x0005_2503, // lw a0, 0(a0)
        0x0005_0067, // jr a0
    ];
    code.resize(0x5c / 4, 0x0000_0013);
    for offset in [0x40, 0x50, 0x58] {
        code[offset / 4] = 0x0000_8067;
    }
    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = object.add_section(Vec::new(), b".text.entry".to_vec(), SectionKind::Text);
    let bytes: Vec<_> = code.iter().flat_map(|word| word.to_le_bytes()).collect();
    object.append_section_data(text, &bytes, 4);
    let data = object.add_section(
        Vec::new(),
        b".rodata.table".to_vec(),
        SectionKind::ReadOnlyData,
    );
    object.append_section_data(data, &[0; 24], 4);
    let symbol = |name: &[u8], section, size, kind| Symbol {
        name: name.to_vec(),
        value: 0,
        size,
        kind,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(section),
        flags: SymbolFlags::None,
    };
    let entry = object.add_symbol(symbol(b"entry", text, bytes.len() as u64, SymbolKind::Text));
    let table = object.add_symbol(symbol(b"table", data, 24, SymbolKind::Data));
    for (offset, kind) in [
        (0, object::elf::R_RISCV_HI20),
        (4, object::elf::R_RISCV_LO12_I),
    ] {
        object
            .add_relocation(
                text,
                Relocation {
                    offset,
                    symbol: table,
                    addend: 0,
                    flags: RelocationFlags::Elf { r_type: kind },
                },
            )
            .expect("table address relocation");
    }
    for (i, target) in [0x50, 0x40, 0x50, 0x58, 0x50, 0x40].into_iter().enumerate() {
        object
            .add_relocation(
                data,
                Relocation {
                    offset: i as u64 * 4,
                    symbol: entry,
                    addend: target,
                    flags: RelocationFlags::Elf {
                        r_type: object::elf::R_RISCV_32,
                    },
                },
            )
            .expect("table entry relocation");
    }
    object.write().expect("fixture ELF")
}

#[test]
fn blobray_and_both_cli_formats_preserve_the_dispatch_kind() {
    for alternative in [false, true] {
        let memory = WorkingMemory::new(64 * 1024 * 1024).expect("working memory");
        let mut outcomes = Vec::new();
        analyze_library(
            &[Executable::new(object(alternative))],
            Assumptions::default(),
            &oer_analysis_riscv_lift::RiscvDecoder,
            &memory,
            &mut || Ok(()),
            &mut |outcome, _| {
                if let LibraryOutcome::Analyzed(analyzed) = outcome {
                    outcomes.push(NamedFunction::Analyzed {
                        function: analyzed.function.clone(),
                        complete: analyzed.complete(),
                        coverage: analyzed.coverage,
                        semantics: analyzed.semantics,
                        records: analyzed.records.to_vec(),
                        jump_tables: analyzed.jump_tables.to_vec(),
                    });
                } else {
                    panic!("fixture function must be analyzed");
                }
                Ok(())
            },
        )
        .expect("Blobray analysis");
        assert_eq!(
            memory.used(),
            0,
            "the borrowed dispatch owner is released after the visit"
        );
        let [outcome] = &outcomes[..] else {
            panic!("one function outcome")
        };
        let NamedFunction::Analyzed { jump_tables, .. } = outcome else {
            panic!("analyzed outcome")
        };
        let expected = if alternative {
            JumpDispatch::TargetSet {
                site: 0x2c,
                targets: vec![0x40, 0x50, 0x58],
            }
        } else {
            JumpDispatch::Table {
                site: 0x2c,
                first_case: Some(0),
                entries: vec![0x50, 0x40, 0x50],
            }
        };
        assert_eq!(jump_tables, &[expected]);
        let text = human(&jump_tables[0]);
        let json = serde_json::to_value(outcome).expect("serialize function outcome");
        let mut ambiguous = json.clone();
        let extra = if alternative { "entries" } else { "targets" };
        ambiguous["jump_tables"][0][extra] = serde_json::json!([0x40, 0x50, 0x58]);
        assert!(
            serde_json::from_value::<NamedFunction>(ambiguous).is_err(),
            "JSON must not mix table entries with a target set"
        );
        if alternative {
            assert_eq!(text, "jump targets +2c: +40, +50, +58");
            assert_eq!(
                json["jump_tables"][0],
                serde_json::json!({
                    "kind": "target-set", "site": 0x2c, "targets": [0x40, 0x50, 0x58],
                })
            );
        } else {
            assert_eq!(text, "jump table +2c: +40 <- case 1; +50 <- cases 0, 2");
            assert_eq!(
                json["jump_tables"][0],
                serde_json::json!({
                    "kind": "table", "site": 0x2c, "first_case": 0, "entries": [0x50, 0x40, 0x50],
                })
            );
        }
        let decoded: NamedFunction = serde_json::from_value(json).expect("decode function outcome");
        let NamedFunction::Analyzed {
            jump_tables: decoded,
            ..
        } = decoded
        else {
            panic!("analyzed roundtrip")
        };
        assert_eq!(&decoded, jump_tables);
    }
}
