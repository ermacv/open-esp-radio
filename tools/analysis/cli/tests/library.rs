#![cfg(target_os = "linux")]
#[allow(dead_code)]
mod support;
use object::{
    Architecture, BinaryFormat, Endianness, SectionKind, SymbolFlags, SymbolKind, SymbolScope,
    write::{Object, Symbol, SymbolSection},
};
use oer_analysis_blobray_application::in_process::{Executable, Limits};
use oer_analysis_blobray_application::library::{Assumptions, register_accesses};
use oer_analysis_blobray_domain::*;
use oer_analysis_riscv_model::*;
use std::process::Command;

/// A relocatable object with one function `entry` of `size` bytes over `code`.
fn object(code: &[u32], size: u64) -> Vec<u8> {
    let bytes: Vec<u8> = code.iter().flat_map(|w| w.to_le_bytes()).collect();
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = obj.add_section(Vec::new(), b".text.entry".to_vec(), SectionKind::Text);
    obj.append_section_data(text, &bytes, 2);
    obj.add_symbol(Symbol {
        name: b"entry".to_vec(),
        value: 0,
        size,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(text),
        flags: SymbolFlags::None,
    });
    obj.write().unwrap()
}

// lui t0,0x20; lw t1,0(t0); andi t2,t1,0xf; andi t1,t1,-16;
// ori t1,t1,3; sw t1,0(t0); lb t2,1(t0); lw t2,0(a0); ret.
const REGISTERS: [u32; 9] = [
    0x000202b7, 0x0002a303, 0x00f37393, 0xff037313, 0x00336313, 0x0062a023, 0x00128383, 0x00052383,
    0x00008067,
];

fn accesses(
    inputs: &[Executable],
    ranges: &[ImageRegion],
) -> Result<(RegisterAccessSummary, Vec<RegisterAccess>)> {
    accesses_with(inputs, ranges, Assumptions::default())
}

fn accesses_with(
    inputs: &[Executable],
    ranges: &[ImageRegion],
    assumptions: Assumptions<'_>,
) -> Result<(RegisterAccessSummary, Vec<RegisterAccess>)> {
    let memory = WorkingMemory::new(64 * 1024 * 1024).unwrap();
    let mut control = Limits::new(DEFAULT_WORK_UNITS, std::time::Duration::from_secs(60));
    let mut records = Vec::new();
    let summary = register_accesses(
        inputs,
        assumptions,
        ranges,
        &oer_analysis_riscv_lift::RiscvDecoder,
        &memory,
        &mut control,
        &mut |record, _| {
            records.push(record.clone());
            Ok(())
        },
    )?;
    Ok((summary, records))
}

fn library() -> Executable {
    let code = object(&REGISTERS, 36);
    Executable::new(support::archive(
        &[(b"first.o", &code), (b"second.o", &code)],
        false,
    ))
}

#[test]
fn every_library_function_reports_its_masked_and_unresolved_accesses() {
    let (summary, records) = accesses(&[library()], &[]).unwrap();
    assert_eq!(summary.functions, 2, "{records:?}");
    assert_eq!((summary.blocked_functions, summary.gaps), (0, 0));
    assert!(summary.unresolved_addresses > 0);
    let observed: Vec<_> = records
        .iter()
        .map(|r| match r {
            RegisterAccess::Observation {
                function,
                address,
                mask,
                ..
            } => (function, *address, *mask),
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert!(
        observed.iter().all(|(function, _, _)| function.input == 0
            && function.name.as_deref() == Some(&b"entry"[..]))
    );
    // Both archive members are analyzed, each under its own occurrence.
    let members: std::collections::BTreeSet<_> =
        observed.iter().map(|(f, _, _)| &f.symbol.object).collect();
    assert_eq!(members.len(), 2);
    for kind in [
        RegisterMaskKind::ReadSelection,
        RegisterMaskKind::WriteReplacement,
    ] {
        assert!(
            observed
                .iter()
                .any(|(_, address, mask)| *address == Some(0x20000)
                    && *mask == Some(RegisterMask { kind, bits: 15 }))
        );
    }
}

#[test]
fn a_store_describes_its_stored_bits_and_a_load_never_does() {
    let (_, records) = accesses(&[library()], &[]).unwrap();
    let mut stores = 0;
    for record in &records {
        let RegisterAccess::Observation {
            fact,
            address,
            stored,
            ..
        } = record
        else {
            continue;
        };
        // A load, or a masked read whose fact is its selecting expression.
        if !matches!(
            fact.as_ref(),
            FunctionRecord::MemoryAccess {
                access: MemoryKind::Store,
                ..
            }
        ) {
            assert!(stored.is_none(), "a read stores nothing: {record:?}");
            continue;
        }
        stores += 1;
        assert_eq!(*address, Some(0x20000));
        // `sw ((lw 0x20000) & ~15 | 3), 0x20000`.
        assert_eq!(
            stored.as_deref(),
            Some(
                &[
                    StoredBits {
                        low: 0,
                        width: 4,
                        source: StoredBitsSource::Constant { value: 3 },
                    },
                    StoredBits {
                        low: 4,
                        width: 28,
                        source: StoredBitsSource::Load {
                            address: Some(0x20000),
                            width: 4,
                            low: 4,
                            same_word: true,
                        },
                    },
                ][..]
            ),
            "{record:?}"
        );
    }
    assert_eq!(stores, 2, "one store per archive member");
}

#[test]
fn ranges_select_resolved_addresses_and_bad_ranges_are_rejected() {
    let inside = [ImageRegion {
        start: 0x20000,
        length: 4,
    }];
    let (summary, records) = accesses(&[library()], &inside).unwrap();
    assert!(records.iter().all(|r| matches!(
        r,
        RegisterAccess::Observation {
            address: None | Some(0x20000 | 0x20001),
            ..
        }
    )));
    let outside = [ImageRegion {
        start: 0x40000,
        length: 4,
    }];
    let (without, _) = accesses(&[library()], &outside).unwrap();
    assert!(without.observations < summary.observations);
    assert_eq!(without.unresolved_addresses, summary.unresolved_addresses);
    for bad in [
        vec![ImageRegion {
            start: 0x20000,
            length: 0,
        }],
        vec![ImageRegion {
            start: u32::MAX,
            length: 2,
        }],
        vec![inside[0]; 257],
    ] {
        let error = accesses(&[library()], &bad).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidRequest, "{error:?}");
    }
}

#[test]
fn unanalyzable_functions_and_foreign_objects_are_reported_not_skipped() {
    let without_size = object(&REGISTERS, 0);
    let inputs = [
        Executable::new(support::archive(
            &[
                (b"unsized.o", &without_size),
                (b"notes.txt", b"not an object"),
            ],
            false,
        )),
        Executable::new(b"neither an archive nor an ELF".to_vec()),
    ];
    let (summary, records) = accesses(&inputs, &[]).unwrap();
    assert_eq!(summary.functions, 0);
    assert!(records.iter().any(|r| matches!(
        r,
        RegisterAccess::Blocked { function, error }
            if function.input == 0 && error.code == ErrorCode::NeedsExtent
    )));
    assert!(records.iter().any(|r| matches!(
        r,
        RegisterAccess::Gap { input: 0, object: Some(_), reason }
            if reason.contains("not a supported ELF32")
    )));
    assert!(records.iter().any(|r| matches!(
        r,
        RegisterAccess::Gap { input: 1, reason, .. } if reason == "unsupported input format"
    )));
    assert_eq!(summary.blocked_functions, 1);
}

#[test]
fn the_command_streams_one_document_of_every_access() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.a");
    let library = library();
    std::fs::write(&path, library.bytes()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["--format", "json", "register-accesses", "--input"])
        .arg(format!("code={}", path.display()))
        .args(["--range", "0x20000:4"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let document: oer_tool_analysis_cli::wire::RegisterAccessDocument =
        serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        document.schema,
        oer_tool_analysis_cli::wire::REGISTER_ACCESSES_SCHEMA
    );
    assert_eq!(document.inputs[0].role, "code");
    assert_eq!(&document.inputs[0].sha256, library.id());
    let (summary, records) = accesses(
        &[library],
        &[ImageRegion {
            start: 0x20000,
            length: 4,
        }],
    )
    .unwrap();
    assert_eq!(document.summary, summary);
    assert_eq!(document.records, records);
    let human = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["register-accesses", "--input"])
        .arg(format!("code={}", path.display()))
        .output()
        .unwrap();
    assert!(human.status.success(), "{human:?}");
    assert!(String::from_utf8_lossy(&human.stdout).starts_with("2 functions"));
}

#[test]
fn the_command_reports_every_record_of_the_named_functions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.a");
    let library = library();
    std::fs::write(&path, library.bytes()).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["--format", "json", "function-records", "--input"])
        .arg(format!("code={}", path.display()))
        .args(["--function", "entry"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let document: oer_tool_analysis_cli::wire::FunctionRecordsDocument =
        serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        document.schema,
        oer_tool_analysis_cli::wire::FUNCTION_RECORDS_SCHEMA
    );
    assert_eq!(&document.inputs[0].sha256, library.id());
    assert!(document.missing.is_empty());
    // One `entry` in each of the two members, both analyzed to the end of
    // their nine instructions, with the register load among their records.
    assert_eq!(document.functions.len(), 2);
    for function in &document.functions {
        let oer_tool_analysis_cli::wire::NamedFunction::Analyzed { records, .. } = function else {
            panic!("`entry` is analyzable: {function:?}");
        };
        assert!(
            records
                .iter()
                .any(|record| matches!(record, FunctionRecord::MemoryAccess { .. }))
        );
    }

    let absent = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["function-records", "--input"])
        .arg(format!("code={}", path.display()))
        .args(["--function", "entry", "--function", "absent"])
        .output()
        .unwrap();
    assert!(!absent.status.success(), "a name no input defines fails");
    let human = String::from_utf8_lossy(&absent.stdout);
    assert!(human.contains("absent: no input defines it"), "{human}");
    assert!(human.contains("entry (input 0)"), "{human}");
}

#[test]
fn the_command_reports_the_accesses_of_one_field() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.a");
    let library = library();
    std::fs::write(&path, library.bytes()).unwrap();
    // `lw t2, 0(a0)` reads field 0 of the entry `a0`; the accesses through
    // `lui t0, 0x20` are absolute and no field.
    let output = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["--format", "json", "field-accesses", "--input"])
        .arg(format!("code={}", path.display()))
        .args(["--offset", "0", "--width", "4"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let document: oer_tool_analysis_cli::wire::FieldAccessesDocument =
        serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        document.schema,
        oer_tool_analysis_cli::wire::FIELD_ACCESSES_SCHEMA
    );
    assert_eq!(&document.inputs[0].sha256, library.id());
    assert!(document.blocked.is_empty());
    // One `entry` in each of the two members.
    assert_eq!(document.functions.len(), 2);
    for function in &document.functions {
        assert_eq!(function.accesses.len(), 1, "{function:?}");
        let access = &function.accesses[0];
        assert_eq!(access.offset, 28);
        assert_eq!(access.width, 4);
        assert_eq!(access.path, [0]);
        assert_eq!(
            access.root,
            oer_tool_analysis_cli::field::FieldRoot::EntryRegister { register: 10 }
        );
    }

    let human = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["field-accesses", "--input"])
        .arg(format!("code={}", path.display()))
        .args(["--offset", "4"])
        .output()
        .unwrap();
    assert!(human.status.success(), "{human:?}");
    let human = String::from_utf8_lossy(&human.stdout);
    assert!(human.contains("0 functions access the field"), "{human}");
}

#[test]
fn filters_select_observations_while_the_summary_counts_the_whole_analysis() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.a");
    std::fs::write(&path, library().bytes()).unwrap();
    let run = |extra: &[&str], json: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_blobray"));
        if json {
            command.args(["--format", "json"]);
        }
        let output = command
            .arg("register-accesses")
            .arg("--input")
            .arg(format!("code={}", path.display()))
            .args(extra)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        output.stdout
    };
    let parse = |bytes: Vec<u8>| -> oer_tool_analysis_cli::wire::RegisterAccessDocument {
        serde_json::from_slice(&bytes).unwrap()
    };
    let all = parse(run(&[], true));
    assert!(all.groups.is_none(), "groups only with --group-by");
    let word = parse(run(
        &["--address", "0x20002", "--group-by", "address"],
        true,
    ));
    assert_eq!(
        word.summary, all.summary,
        "a filter never changes the summary"
    );
    let observations = |document: &oer_tool_analysis_cli::wire::RegisterAccessDocument| {
        document
            .records
            .iter()
            .filter(|record| {
                matches!(
                    record,
                    oer_analysis_blobray_domain::RegisterAccess::Observation { .. }
                )
            })
            .count()
    };
    assert!(observations(&word) > 0 && observations(&word) <= observations(&all));
    assert!(word.records.iter().all(|record| match record {
        oer_analysis_blobray_domain::RegisterAccess::Observation { address, .. } =>
            address.is_some_and(|address| address & !3 == 0x20000),
        _ => true,
    }));
    let groups = word.groups.expect("address groups");
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].word, Some(0x20000));
    let absent = parse(run(
        &["--function", "absent", "--group-by", "function"],
        true,
    ));
    assert_eq!(observations(&absent), 0);
    assert_eq!(absent.groups, Some(Vec::new()));
    let human = String::from_utf8(run(&["--address", "0x20000"], false)).unwrap();
    let mut lines = human.lines();
    assert_eq!(lines.next(), Some("0x00020000"), "{human}");
    let summary = human.lines().last().unwrap();
    assert!(
        summary.contains(" unresolved, ") && summary.ends_with(" indexed)"),
        "{human}"
    );
}

/// `caller`: `li a0, 0x67; li a1, 1; mv a2, a5; call phy_i2c_writeReg; ret`.
fn calling_object() -> Vec<u8> {
    use object::write::{Object, Relocation, Symbol, SymbolSection};
    use object::{
        Architecture, BinaryFormat, Endianness, RelocationFlags, SectionKind, SymbolFlags,
        SymbolKind, SymbolScope,
    };
    let words: [u32; 6] = [
        0x0670_0513, // addi a0, zero, 0x67
        0x0010_0593, // addi a1, zero, 1
        0x0007_8613, // addi a2, a5, 0
        0x0000_0097, // auipc ra, 0
        0x0000_80e7, // jalr ra, 0(ra)
        0x0000_8067, // jalr zero, 0(ra)
    ];
    let code: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = object.add_section(Vec::new(), b".text".to_vec(), SectionKind::Text);
    object.append_section_data(text, &code, 4);
    object.add_symbol(Symbol {
        name: b"caller".to_vec(),
        value: 0,
        size: code.len() as u64,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(text),
        flags: SymbolFlags::None,
    });
    let callee = object.add_symbol(Symbol {
        name: b"phy_i2c_writeReg".to_vec(),
        value: 0,
        size: 0,
        kind: SymbolKind::Unknown,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Undefined,
        flags: SymbolFlags::None,
    });
    object
        .add_relocation(
            text,
            Relocation {
                offset: 12,
                symbol: callee,
                addend: 0,
                flags: RelocationFlags::Elf {
                    r_type: object::elf::R_RISCV_CALL_PLT,
                },
            },
        )
        .unwrap();
    object.write().unwrap()
}

#[test]
fn the_command_reports_the_arguments_of_each_call_site() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("calls.a");
    std::fs::write(
        &path,
        support::archive(&[(b"calls.o", &calling_object())], false),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args([
            "--format",
            "json",
            "call-arguments",
            "--abi",
            "riscv-integer",
            "--input",
        ])
        .arg(format!("code={}", path.display()))
        .args(["--symbol", "phy_i2c_writeReg"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let document: oer_tool_analysis_cli::wire::CallArgumentsDocument =
        serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        document.schema,
        oer_tool_analysis_cli::wire::CALL_ARGUMENTS_SCHEMA
    );
    assert_eq!(document.callers.len(), 1, "{document:?}");
    let sites = &document.callers[0].sites;
    assert_eq!(sites.len(), 1);
    use oer_tool_analysis_cli::call_arguments::ArgumentValue;
    assert_eq!(sites[0].offset, 16, "the jalr of the call pair");
    assert_eq!(
        &sites[0].arguments[..4],
        [
            ArgumentValue::Constant { value: 0x67 },
            ArgumentValue::Constant { value: 1 },
            ArgumentValue::EntryArgument { index: 5 },
            ArgumentValue::EntryArgument { index: 3 },
        ]
    );
    let human = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["call-arguments", "--abi", "riscv-integer", "--input"])
        .arg(format!("code={}", path.display()))
        .args(["--symbol", "phy_i2c_writeReg"])
        .output()
        .unwrap();
    let human = String::from_utf8(human.stdout).unwrap();
    assert!(
        human.starts_with(
            "caller (input 0) +10: phy_i2c_writeReg(a0=0x67, a1=0x1, a2=arg5, a3=arg3,"
        ),
        "{human}"
    );
    assert!(
        human.trim_end().ends_with(
            "1 call sites in 1 functions; 1 partial (by cause: 1 opaque calls), 0 blocked, 0 gaps"
        ),
        "the opaque call leaves the caller's semantics incomplete: {human}"
    );
}

/// `greet`: `lui a0, %hi(.LC0); ret`, with `.LC0` in `.rodata.str1.1`.
fn object_with_strings() -> Vec<u8> {
    use object::write::{Object, Relocation, Symbol, SymbolSection};
    use object::{
        Architecture, BinaryFormat, Endianness, RelocationFlags, SectionKind, SymbolFlags,
        SymbolKind, SymbolScope,
    };
    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = object.add_section(Vec::new(), b".text".to_vec(), SectionKind::Text);
    let code: Vec<u8> = [0x0000_0537_u32, 0x0000_8067]
        .iter()
        .flat_map(|word| word.to_le_bytes())
        .collect();
    object.append_section_data(text, &code, 4);
    let strings = object.add_section(
        Vec::new(),
        b".rodata.str1.1".to_vec(),
        SectionKind::ReadOnlyString,
    );
    object.append_section_data(strings, b"hello\0caf\xc3\xa9\xff\0", 1);
    object.add_symbol(Symbol {
        name: b"greet".to_vec(),
        value: 0,
        size: code.len() as u64,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(text),
        flags: SymbolFlags::None,
    });
    let label = object.add_symbol(Symbol {
        name: b".LC0".to_vec(),
        value: 0,
        size: 0,
        kind: SymbolKind::Data,
        scope: SymbolScope::Compilation,
        weak: false,
        section: SymbolSection::Section(strings),
        flags: SymbolFlags::None,
    });
    object
        .add_relocation(
            text,
            Relocation {
                offset: 0,
                symbol: label,
                addend: 0,
                flags: RelocationFlags::Elf {
                    r_type: object::elf::R_RISCV_HI20,
                },
            },
        )
        .unwrap();
    object.write().unwrap()
}

#[test]
fn symbols_strings_and_referenced_text_come_from_the_captured_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("strings.a");
    let member = object_with_strings();
    std::fs::write(
        &path,
        support::archive(&[(b"dup.o", &member), (b"dup.o", &member)], false),
    )
    .unwrap();
    let run = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_blobray"))
            .args(args)
            .arg(format!("vendorlib={}", path.display()))
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        output.stdout
    };
    let symbols: oer_tool_analysis_cli::wire::SymbolsDocument = serde_json::from_slice(&run(&[
        "--format", "json", "symbols", "--match", "gre*", "--input",
    ]))
    .unwrap();
    assert_eq!(
        symbols.symbols.len(),
        2,
        "both same-named members are listed"
    );
    assert!(
        symbols
            .symbols
            .iter()
            .all(|row| row.citation == "vendorlib[dup.o]::greet"
                && row.kind == "function"
                && row.section.as_deref() == Some(".text"))
    );
    let strings: oer_tool_analysis_cli::wire::StringsDocument =
        serde_json::from_slice(&run(&["--format", "json", "strings", "--input"])).unwrap();
    let texts: Vec<_> = strings
        .strings
        .iter()
        .map(|row| (row.offset, row.text.as_str()))
        .collect();
    assert_eq!(
        texts,
        [
            (0, "hello"),
            (6, "caf\\xc3\\xa9\\xff"),
            (0, "hello"),
            (6, "caf\\xc3\\xa9\\xff")
        ]
    );
    let listing =
        String::from_utf8(run(&["function-records", "--function", "greet", "--input"])).unwrap();
    assert!(listing.contains(".LC0  # \"hello\""), "{listing}");
}

/// `pcrel`: `auipc a0, %pcrel_hi(.LC0+6); addi a0, a0, %pcrel_lo(.Lpcrel_hi0); ret`.
fn object_with_pcrel_string() -> Vec<u8> {
    use object::write::{Object, Relocation, Symbol, SymbolSection};
    use object::{
        Architecture, BinaryFormat, Endianness, RelocationFlags, SectionKind, SymbolFlags,
        SymbolKind, SymbolScope,
    };
    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = object.add_section(Vec::new(), b".text".to_vec(), SectionKind::Text);
    let code: Vec<u8> = [0x0000_0517_u32, 0x0005_0513, 0x0000_8067]
        .iter()
        .flat_map(|word| word.to_le_bytes())
        .collect();
    object.append_section_data(text, &code, 4);
    let strings = object.add_section(
        Vec::new(),
        b".rodata.str1.1".to_vec(),
        SectionKind::ReadOnlyString,
    );
    object.append_section_data(strings, b"hello\0world\0", 1);
    let symbol = |object: &mut Object, name: &[u8], value, size, kind, scope, section| {
        object.add_symbol(Symbol {
            name: name.to_vec(),
            value,
            size,
            kind,
            scope,
            weak: false,
            section: SymbolSection::Section(section),
            flags: SymbolFlags::None,
        })
    };
    symbol(
        &mut object,
        b"pcrel",
        0,
        code.len() as u64,
        SymbolKind::Text,
        SymbolScope::Linkage,
        text,
    );
    let high = symbol(
        &mut object,
        b".Lpcrel_hi0",
        0,
        0,
        SymbolKind::Label,
        SymbolScope::Compilation,
        text,
    );
    let label = symbol(
        &mut object,
        b".LC0",
        0,
        0,
        SymbolKind::Data,
        SymbolScope::Compilation,
        strings,
    );
    for (offset, symbol, addend, r_type) in [
        (0, label, 6, object::elf::R_RISCV_PCREL_HI20),
        (4, high, 0, object::elf::R_RISCV_PCREL_LO12_I),
    ] {
        object
            .add_relocation(
                text,
                Relocation {
                    offset,
                    symbol,
                    addend,
                    flags: RelocationFlags::Elf { r_type },
                },
            )
            .unwrap();
    }
    object.write().unwrap()
}

#[test]
fn a_pcrel_pair_shows_the_text_at_its_high_addend_on_both_instructions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pcrel.a");
    std::fs::write(
        &path,
        support::archive(&[(b"pcrel.o", &object_with_pcrel_string())], false),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args(["function-records", "--function", "pcrel", "--input"])
        .arg(format!("code={}", path.display()))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let listing = String::from_utf8(output.stdout).unwrap();
    let quoted: Vec<_> = listing.lines().filter(|line| line.contains('"')).collect();
    assert_eq!(quoted.len(), 2, "{listing}");
    assert!(
        quoted.iter().all(|line| line.contains("\"world\"")),
        "{listing}"
    );
}

/// `dispatch(a0)`: `li a5, 2; bltu a5, a0, default; lui/addi a5, .Ltable;
/// slli a4, a0, 2; add; lw a5; jalr zero, 0(a5)` with three cases and a
/// default, the table in `.rodata` relocated by `.rela.rodata`. `bounded`
/// keeps the check (a `nop` replaces it otherwise); `relocated` entries of
/// the three carry their `R_RISCV_32`.
fn switch_object(bounded: bool, relocated: u64) -> Vec<u8> {
    let words: [u32; 16] = [
        0x0020_0793,                                     // 0x00 addi a5, zero, 2
        if bounded { 0x02a7_ea63 } else { 0x0000_0013 }, // 0x04 bltu a5, a0, 0x38
        0x0000_07b7,                                     // 0x08 lui a5, %hi(.Ltable)
        0x0007_8793,                                     // 0x0c addi a5, a5, %lo(.Ltable)
        0x0025_1713,                                     // 0x10 slli a4, a0, 2
        0x00f7_0733,                                     // 0x14 add a4, a4, a5
        0x0007_2783,                                     // 0x18 lw a5, 0(a4)
        0x0007_8067,                                     // 0x1c jalr zero, 0(a5)
        0x00a0_0513,
        0x0000_8067, // 0x20 case 0
        0x00b0_0513,
        0x0000_8067, // 0x28 case 1
        0x00c0_0513,
        0x0000_8067, // 0x30 case 2
        0x0000_0513,
        0x0000_8067, // 0x38 default
    ];
    switches_object(&words, &[(0x08, &[0x20, 0x28, 0x30], relocated as usize)])
}

/// `dispatch(a0, a1)`: `switch (a0) { case 0: …; case 1: switch (a1) { … } }`,
/// each dispatch shaped as in `switch_object`, the inner one reached only
/// through the outer table's case 1.
fn nested_switch_object() -> Vec<u8> {
    let words: [u32; 24] = [
        0x0010_0793, // 0x00 addi a5, zero, 1
        0x04a7_ea63, // 0x04 bltu a5, a0, 0x58
        0x0000_07b7, // 0x08 lui a5, %hi(.Ltable0)
        0x0007_8793, // 0x0c addi a5, a5, %lo(.Ltable0)
        0x0025_1713, // 0x10 slli a4, a0, 2
        0x00f7_0733, // 0x14 add a4, a4, a5
        0x0007_2783, // 0x18 lw a5, 0(a4)
        0x0007_8067, // 0x1c jalr zero, 0(a5)
        0x00a0_0513, // 0x20 outer case 0
        0x0000_8067, // 0x24 ret
        0x0010_0793, // 0x28 outer case 1: addi a5, zero, 1
        0x02b7_e663, // 0x2c bltu a5, a1, 0x58
        0x0000_07b7, // 0x30 lui a5, %hi(.Ltable1)
        0x0007_8793, // 0x34 addi a5, a5, %lo(.Ltable1)
        0x0025_9713, // 0x38 slli a4, a1, 2
        0x00f7_0733, // 0x3c add a4, a4, a5
        0x0007_2783, // 0x40 lw a5, 0(a4)
        0x0007_8067, // 0x44 jalr zero, 0(a5)
        0x00b0_0513, // 0x48 inner case 0
        0x0000_8067, // 0x4c ret
        0x00c0_0513, // 0x50 inner case 1
        0x0000_8067, // 0x54 ret
        0x0000_0513, // 0x58 default
        0x0000_8067, // 0x5c ret
    ];
    switches_object(
        &words,
        &[(0x08, &[0x20, 0x28], 2), (0x30, &[0x48, 0x50], 2)],
    )
}

/// `switch_object`'s dispatch whose case 0 sets `a0 = 100` and jumps back to
/// the dispatch block past its bounds check: the index then reaches entry 100.
fn reentrant_switch_object() -> Vec<u8> {
    let words: [u32; 16] = [
        0x0020_0793, // 0x00 addi a5, zero, 2
        0x02a7_ea63, // 0x04 bltu a5, a0, 0x38
        0x0000_07b7, // 0x08 lui a5, %hi(.Ltable0)
        0x0007_8793, // 0x0c addi a5, a5, %lo(.Ltable0)
        0x0025_1713, // 0x10 slli a4, a0, 2
        0x00f7_0733, // 0x14 add a4, a4, a5
        0x0007_2783, // 0x18 lw a5, 0(a4)
        0x0007_8067, // 0x1c jalr zero, 0(a5)
        0x0640_0513, // 0x20 case 0: addi a0, zero, 100
        0xfe5f_f06f, // 0x24 jal zero, 0x08
        0x00b0_0513,
        0x0000_8067, // 0x28 case 1
        0x00c0_0513,
        0x0000_8067, // 0x30 case 2
        0x0000_0513,
        0x0000_8067, // 0x38 default
    ];
    switches_object(&words, &[(0x08, &[0x20, 0x28, 0x30], 3)])
}

/// A relocatable object defining `dispatch` as `words` in `.text`, with one
/// `.Ltable{n}` in `.rodata` per `(lui offset, case labels, relocated)`: the
/// `lui/addi` pair at that offset addresses it and its first `relocated`
/// entries carry their `R_RISCV_32` to the case labels.
fn switches_object(words: &[u32], tables: &[(u64, &[i64], usize)]) -> Vec<u8> {
    use object::write::{Object, Relocation, Symbol, SymbolSection};
    use object::{
        Architecture, BinaryFormat, Endianness, RelocationFlags, SectionKind, SymbolFlags,
        SymbolKind, SymbolScope,
    };
    let code: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = object.add_section(Vec::new(), b".text".to_vec(), SectionKind::Text);
    object.append_section_data(text, &code, 4);
    let rodata = object.add_section(Vec::new(), b".rodata".to_vec(), SectionKind::ReadOnlyData);
    let entries: usize = tables.iter().map(|(_, cases, _)| cases.len()).sum();
    object.append_section_data(rodata, &vec![0; 4 * entries], 4);
    object.add_symbol(Symbol {
        name: b"dispatch".to_vec(),
        value: 0,
        size: code.len() as u64,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(text),
        flags: SymbolFlags::None,
    });
    let text_symbol = object.section_symbol(text);
    let relocate = |object: &mut Object, section, offset, symbol, addend, r_type| {
        object
            .add_relocation(
                section,
                Relocation {
                    offset,
                    symbol,
                    addend,
                    flags: RelocationFlags::Elf { r_type },
                },
            )
            .unwrap();
    };
    let mut at = 0;
    for (n, (lui, cases, relocated)) in tables.iter().enumerate() {
        let table = object.add_symbol(Symbol {
            name: format!(".Ltable{n}").into_bytes(),
            value: at,
            size: 0,
            kind: SymbolKind::Data,
            scope: SymbolScope::Compilation,
            weak: false,
            section: SymbolSection::Section(rodata),
            flags: SymbolFlags::None,
        });
        relocate(&mut object, text, *lui, table, 0, object::elf::R_RISCV_HI20);
        relocate(
            &mut object,
            text,
            lui + 4,
            table,
            0,
            object::elf::R_RISCV_LO12_I,
        );
        for (entry, case) in cases.iter().enumerate().take(*relocated) {
            relocate(
                &mut object,
                rodata,
                at + 4 * entry as u64,
                text_symbol,
                *case,
                object::elf::R_RISCV_32,
            );
        }
        at += 4 * cases.len() as u64;
    }
    object.write().unwrap()
}

/// `orphan`: `addi a0, a0, %pcrel_lo(.Lnone); ret`, whose low half names no
/// `%pcrel_hi` and so no target.
fn orphan_relocation_object() -> Vec<u8> {
    use object::write::{Object, Relocation, Symbol, SymbolSection};
    use object::{
        Architecture, BinaryFormat, Endianness, RelocationFlags, SectionKind, SymbolFlags,
        SymbolKind, SymbolScope,
    };
    let code: Vec<u8> = [0x0005_0513_u32, 0x0000_8067]
        .iter()
        .flat_map(|word| word.to_le_bytes())
        .collect();
    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = object.add_section(Vec::new(), b".text".to_vec(), SectionKind::Text);
    object.append_section_data(text, &code, 4);
    object.add_symbol(Symbol {
        name: b"orphan".to_vec(),
        value: 0,
        size: code.len() as u64,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(text),
        flags: SymbolFlags::None,
    });
    let label = object.add_symbol(Symbol {
        name: b".Lnone".to_vec(),
        value: 4,
        size: 0,
        kind: SymbolKind::Label,
        scope: SymbolScope::Compilation,
        weak: false,
        section: SymbolSection::Section(text),
        flags: SymbolFlags::None,
    });
    object
        .add_relocation(
            text,
            Relocation {
                offset: 0,
                symbol: label,
                addend: 0,
                flags: RelocationFlags::Elf {
                    r_type: object::elf::R_RISCV_PCREL_LO12_I,
                },
            },
        )
        .unwrap();
    object.write().unwrap()
}

#[test]
fn partial_functions_are_counted_once_per_cause() {
    let causes = |name: &[u8], object: Vec<u8>| {
        let library = Executable::new(support::archive(&[(name, &object)], false));
        let (summary, _) = accesses(&[library], &[]).unwrap();
        assert_eq!(summary.partial_functions, 1);
        summary.partial_causes
    };
    let only = |cause: fn(&mut PartialCauses) -> &mut u64| {
        let mut causes = PartialCauses::default();
        *cause(&mut causes) = 1;
        causes
    };
    assert_eq!(
        causes(b"call.o", calling_object()),
        only(|c| &mut c.opaque_calls),
        "an opaque call leaves the values after it unknown, nothing else"
    );
    assert_eq!(
        causes(b"switch.o", switch_object(false, 3)),
        only(|c| &mut c.control_flow),
        "an unexpanded indirect jump is control flow only"
    );
    assert_eq!(
        causes(b"orphan.o", orphan_relocation_object()),
        PartialCauses {
            references: 1,
            // The graph treats an instruction with an unknown relocation as
            // possibly transferring control, so its flow is incomplete too.
            control_flow: 1,
            ..PartialCauses::default()
        },
    );
    // Together, each function counts once in the total and once per cause.
    let library = Executable::new(support::archive(
        &[
            (b"call.o", &calling_object()),
            (b"switch.o", &switch_object(false, 3)),
            (b"orphan.o", &orphan_relocation_object()),
        ],
        false,
    ));
    let (summary, _) = accesses(&[library], &[]).unwrap();
    assert_eq!((summary.functions, summary.partial_functions), (3, 3));
    assert_eq!(
        summary.partial_causes,
        PartialCauses {
            control_flow: 2,
            references: 1,
            opaque_calls: 1,
            ..PartialCauses::default()
        }
    );
}

#[test]
fn a_bounded_relocated_switch_is_followed_and_any_other_stays_a_gap() {
    let dir = tempfile::tempdir().unwrap();
    let records = |name: &str, object: Vec<u8>| {
        let path = dir.path().join(name);
        std::fs::write(&path, support::archive(&[(b"switch.o", &object)], false)).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_blobray"))
            .args([
                "--format",
                "json",
                "function-records",
                "--function",
                "dispatch",
                "--input",
            ])
            .arg(format!("code={}", path.display()))
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let document: oer_tool_analysis_cli::wire::FunctionRecordsDocument =
            serde_json::from_slice(&output.stdout).unwrap();
        match document.functions.into_iter().next().unwrap() {
            oer_tool_analysis_cli::wire::NamedFunction::Analyzed {
                coverage,
                records,
                jump_tables,
                ..
            } => (coverage.control_flow, records, jump_tables),
            other => panic!("{other:?}"),
        }
    };
    let jumps = |records: &[FunctionRecord]| {
        let mut targets: Vec<_> = records
            .iter()
            .filter_map(|record| match record {
                FunctionRecord::Edge {
                    from: 0x1c,
                    target,
                    relation,
                    ..
                } => Some((*relation, *target)),
                _ => None,
            })
            .collect();
        targets.sort_by_key(|(_, target)| *target);
        targets
    };
    let (complete, followed, tables) = records("bounded.a", switch_object(true, 3));
    assert!(complete, "every case is reached");
    assert_eq!(
        tables,
        [oer_analysis_blobray_domain::JumpTable {
            site: 0x1c,
            first_case: Some(0),
            entries: vec![0x20, 0x28, 0x30],
        }]
    );
    assert_eq!(
        jumps(&followed),
        [
            (EdgeKind::Jump, Some(0x20)),
            (EdgeKind::Jump, Some(0x28)),
            (EdgeKind::Jump, Some(0x30)),
        ]
    );
    for (name, object) in [
        ("unbounded.a", switch_object(false, 3)),
        ("unrelocated.a", switch_object(true, 2)),
    ] {
        let (complete, gap, tables) = records(name, object);
        assert!(!complete, "{name}: the dispatch stays a gap");
        assert!(tables.is_empty(), "{name}");
        assert_eq!(jumps(&gap), [(EdgeKind::Indirect, None)], "{name}");
    }
}

#[test]
fn a_table_a_later_pass_no_longer_proves_is_never_reported() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("reentrant.a");
    std::fs::write(
        &path,
        support::archive(&[(b"switch.o", &reentrant_switch_object())], false),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args([
            "--format",
            "json",
            "function-records",
            "--function",
            "dispatch",
            "--input",
        ])
        .arg(format!("code={}", path.display()))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let document: oer_tool_analysis_cli::wire::FunctionRecordsDocument =
        serde_json::from_slice(&output.stdout).unwrap();
    let oer_tool_analysis_cli::wire::NamedFunction::Analyzed {
        coverage,
        records,
        jump_tables,
        ..
    } = document.functions.into_iter().next().unwrap()
    else {
        panic!()
    };
    assert!(
        jump_tables.is_empty(),
        "following the table adds a second way into its dispatch block"
    );
    assert!(!coverage.control_flow, "the dispatch stays a gap");
    assert!(records.iter().any(|record| matches!(
        record,
        FunctionRecord::Edge {
            from: 0x1c,
            target: None,
            relation: EdgeKind::Indirect,
            ..
        }
    )));
}

#[test]
fn a_switch_reached_only_through_another_tables_case_is_followed_too() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested.a");
    std::fs::write(
        &path,
        support::archive(&[(b"switch.o", &nested_switch_object())], false),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args([
            "--format",
            "json",
            "function-records",
            "--function",
            "dispatch",
            "--input",
        ])
        .arg(format!("code={}", path.display()))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let document: oer_tool_analysis_cli::wire::FunctionRecordsDocument =
        serde_json::from_slice(&output.stdout).unwrap();
    let oer_tool_analysis_cli::wire::NamedFunction::Analyzed {
        coverage,
        jump_tables,
        ..
    } = document.functions.into_iter().next().unwrap()
    else {
        panic!()
    };
    let table = |site, entries: &[u64]| oer_analysis_blobray_domain::JumpTable {
        site,
        first_case: Some(0),
        entries: entries.to_vec(),
    };
    assert_eq!(
        jump_tables,
        [table(0x1c, &[0x20, 0x28]), table(0x44, &[0x48, 0x50])],
        "the inner dispatch is found on the pass that follows the outer table"
    );
    assert!(coverage.control_flow, "both dispatches are followed");
}

/// A static image: `dispatch` at 0x1000 indexes its compiler jump table
/// `.LJTI0_0` at 0x2000 by `a0` with no bounds check, as a Rust `async`
/// state machine does: `lui/addi a5, 0x2000; slli a4, a0, 2; add; lw a5;
/// jalr zero, 0(a5)`, cases at 0x1018, 0x1020 and 0x1028. With `relocated`,
/// the table's words keep their `R_RISCV_32` relocations, as an
/// `--emit-relocs` link keeps them.
fn switch_image(relocated: bool) -> Vec<u8> {
    use object::elf;
    use object::write::elf::{FileHeader, ProgramHeader, Rel, SectionHeader, Sym, Writer};
    let words: [u32; 12] = [
        0x0000_27b7, // 0x1000 lui a5, 0x2
        0x0007_8793, // 0x1004 addi a5, a5, 0
        0x0025_1713, // 0x1008 slli a4, a0, 2
        0x00f7_0733, // 0x100c add a4, a4, a5
        0x0007_2783, // 0x1010 lw a5, 0(a4)
        0x0007_8067, // 0x1014 jalr zero, 0(a5)
        0x00a0_0513, // 0x1018 case 0
        0x0000_8067,
        0x00b0_0513, // 0x1020 case 1
        0x0000_8067,
        0x00c0_0513, // 0x1028 case 2
        0x0000_8067,
    ];
    let code: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
    let cases = [0x1018u32, 0x1020, 0x1028];
    let table: Vec<u8> = cases.iter().flat_map(|case| case.to_le_bytes()).collect();
    let mut out = Vec::new();
    let mut w = Writer::new(object::Endianness::Little, false, &mut out);
    w.reserve_file_header();
    w.reserve_program_headers(2);
    let text_name = w.add_section_name(b".text");
    let text = w.reserve_section_index();
    let text_offset = w.reserve(code.len(), 4);
    let rodata_name = w.add_section_name(b".rodata");
    let rodata = w.reserve_section_index();
    let rodata_offset = w.reserve(table.len(), 4);
    w.reserve_null_symbol_index();
    let label_name = w.add_string(b".LJTI0_0");
    w.reserve_symbol_index(None);
    let function_name = w.add_string(b"dispatch");
    let function = w.reserve_symbol_index(None);
    w.reserve_symtab_section_index();
    w.reserve_symtab();
    w.reserve_strtab_section_index();
    w.reserve_strtab();
    let rela_name = w.add_section_name(b".rela.rodata");
    let (rela, rela_offset) = if relocated {
        (
            Some(w.reserve_section_index()),
            w.reserve_relocations(cases.len(), true),
        )
    } else {
        (None, 0)
    };
    w.reserve_shstrtab_section_index();
    w.reserve_shstrtab();
    w.reserve_section_headers();
    w.write_file_header(&FileHeader {
        os_abi: 0,
        abi_version: 0,
        e_type: elf::ET_EXEC,
        e_machine: elf::EM_RISCV,
        e_entry: 0x1000,
        e_flags: 0,
    })
    .unwrap();
    w.write_align_program_headers();
    for (offset, address, size, flags) in [
        (text_offset, 0x1000, code.len(), elf::PF_R | elf::PF_X),
        (rodata_offset, 0x2000, table.len(), elf::PF_R),
    ] {
        w.write_program_header(&ProgramHeader {
            p_type: elf::PT_LOAD,
            p_flags: flags,
            p_offset: offset as u64,
            p_vaddr: address,
            p_paddr: address,
            p_filesz: size as u64,
            p_memsz: size as u64,
            p_align: 4,
        });
    }
    w.write_align(4);
    w.write(&code);
    w.write_align(4);
    w.write(&table);
    w.write_null_symbol();
    w.write_symbol(&Sym {
        name: Some(label_name),
        section: Some(rodata),
        st_info: (elf::STB_LOCAL << 4) | elf::STT_NOTYPE,
        st_other: 0,
        st_shndx: 0,
        st_value: 0x2000,
        st_size: 0,
    });
    w.write_symbol(&Sym {
        name: Some(function_name),
        section: Some(text),
        st_info: (elf::STB_GLOBAL << 4) | elf::STT_FUNC,
        st_other: 0,
        st_shndx: 0,
        st_value: 0x1000,
        st_size: code.len() as u64,
    });
    w.write_strtab();
    if relocated {
        w.write_align_relocation();
        for (entry, case) in cases.iter().enumerate() {
            w.write_relocation(
                true,
                &Rel {
                    r_offset: 0x2000 + 4 * entry as u64,
                    r_sym: function.0,
                    r_type: elf::R_RISCV_32,
                    r_addend: i64::from(case - 0x1000),
                },
            );
        }
    }
    w.write_shstrtab();
    w.write_null_section_header();
    for (name, address, offset, size, flags) in [
        (
            text_name,
            0x1000,
            text_offset,
            code.len(),
            elf::SHF_ALLOC | elf::SHF_EXECINSTR,
        ),
        (
            rodata_name,
            0x2000,
            rodata_offset,
            table.len(),
            elf::SHF_ALLOC,
        ),
    ] {
        w.write_section_header(&SectionHeader {
            name: Some(name),
            sh_type: elf::SHT_PROGBITS,
            sh_flags: u64::from(flags),
            sh_addr: address,
            sh_offset: offset as u64,
            sh_size: size as u64,
            sh_link: 0,
            sh_info: 0,
            sh_addralign: 4,
            sh_entsize: 0,
        });
    }
    w.write_symtab_section_header(2);
    w.write_strtab_section_header();
    if rela.is_some() {
        let symtab = w.symtab_index();
        w.write_relocation_section_header(
            rela_name,
            rodata,
            symtab,
            rela_offset,
            cases.len(),
            true,
        );
    }
    w.write_shstrtab_section_header();
    assert_eq!(w.reserved_len(), w.len());
    out
}

#[test]
fn an_image_follows_its_labelled_table_only_with_the_links_relocations() {
    let dir = tempfile::tempdir().unwrap();
    let records = |name: &str, image: Vec<u8>| {
        let path = dir.path().join(name);
        std::fs::write(&path, image).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_blobray"))
            .args([
                "--format",
                "json",
                "function-records",
                "--function",
                "dispatch",
                "--input",
            ])
            .arg(format!("image={}", path.display()))
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let document: oer_tool_analysis_cli::wire::FunctionRecordsDocument =
            serde_json::from_slice(&output.stdout).unwrap();
        match document.functions.into_iter().next().unwrap() {
            oer_tool_analysis_cli::wire::NamedFunction::Analyzed {
                coverage,
                semantics,
                jump_tables,
                ..
            } => (coverage.control_flow && semantics.complete, jump_tables),
            other => panic!("{other:?}"),
        }
    };
    // No bounds check: the label and the kept relocations delimit the table.
    let (complete, tables) = records("relocated.elf", switch_image(true));
    assert!(
        complete,
        "every case is reached, and the followed jump is no opaque transfer"
    );
    assert_eq!(
        tables,
        [oer_analysis_blobray_domain::JumpTable {
            site: 0x1014,
            first_case: Some(0),
            entries: vec![0x1018, 0x1020, 0x1028],
        }]
    );
    // The same image linked without its relocations proves no table.
    let (complete, tables) = records("stripped.elf", switch_image(false));
    assert!(!complete, "the dispatch stays a gap");
    assert!(tables.is_empty());
}

/// `jal ra, target` at `site`.
fn call(site: u32, target: u32) -> u32 {
    let offset = target.wrapping_sub(site);
    ((offset >> 20 & 1) << 31)
        | ((offset >> 1 & 0x3ff) << 21)
        | ((offset >> 11 & 1) << 20)
        | ((offset >> 12 & 0xff) << 12)
        | (1 << 7)
        | 0x6f
}

#[test]
fn a_call_takes_the_constants_its_callee_returns() {
    const RET: u32 = 0x0000_8067;
    let code = [
        // helper @0x1000: a0 = 0x20101000, or 0x20100894 unless a1 == 0.
        0x2010_1537, // lui a0, 0x20101
        0x0005_8463, // beq a1, zero, +8
        0x8945_0513, // addi a0, a0, -1900
        RET,
        // caller @0x1010: loads through helper's result.
        call(0x1010, 0x1000),
        0x0005_2503, // lw a0, 0(a0)
        RET,
        // loaded @0x101c: returns a word it loads, no constant.
        0x0005_2503, // lw a0, 0(a0)
        RET,
        // other @0x1024: loads through loaded's result.
        call(0x1024, 0x101c),
        0x0005_2503, // lw a0, 0(a0)
        RET,
    ];
    let image = support::executable_with_symbols(
        &code,
        &[
            ("helper", 0x1000, 16),
            ("caller", 0x1010, 12),
            ("loaded", 0x101c, 8),
            ("other", 0x1024, 12),
        ],
    );
    let observed = |call_returns: bool, name: &str| -> Vec<u32> {
        let (_, records) = accesses_with(
            &[Executable::new(image.clone())],
            &[ImageRegion {
                start: 0x2010_0000,
                length: 0x2000,
            }],
            Assumptions {
                abi: None,
                call_returns,
                read_only: &[],
            },
        )
        .unwrap();
        let mut addresses: Vec<u32> = records
            .iter()
            .filter_map(|record| match record {
                RegisterAccess::Observation {
                    function, address, ..
                } if function.name.as_deref() == Some(name.as_bytes()) => *address,
                _ => None,
            })
            .collect();
        addresses.sort_unstable();
        addresses
    };
    assert_eq!(observed(true, "caller"), [0x2010_0894, 0x2010_1000]);
    // A callee with a return that is no constant gives the call nothing.
    assert_eq!(observed(true, "other"), Vec::<u32>::new());
    // Calls take returned constants only when the request assumes them.
    assert_eq!(observed(false, "caller"), Vec::<u32>::new());
}

/// A relocatable object of global functions `(name, offset, size)` over
/// `code`, with relocations `(offset, symbol, type)` against its own or
/// undefined symbols.
fn functions_object(
    code: &[u32],
    functions: &[(&str, u64, u64)],
    relocations: &[(u64, &str, u32)],
) -> Vec<u8> {
    use object::write::Relocation;
    let bytes: Vec<u8> = code.iter().flat_map(|w| w.to_le_bytes()).collect();
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = obj.add_section(Vec::new(), b".text".to_vec(), SectionKind::Text);
    obj.append_section_data(text, &bytes, 4);
    let symbol = |obj: &mut Object, name: &str, value, size, section| {
        obj.add_symbol(Symbol {
            name: name.as_bytes().to_vec(),
            value,
            size,
            kind: SymbolKind::Text,
            scope: SymbolScope::Linkage,
            weak: false,
            section,
            flags: SymbolFlags::None,
        })
    };
    let mut ids = Vec::new();
    for &(name, offset, size) in functions {
        ids.push((
            name,
            symbol(&mut obj, name, offset, size, SymbolSection::Section(text)),
        ));
    }
    for &(offset, name, r_type) in relocations {
        let id = match ids.iter().find(|(known, _)| *known == name) {
            Some(&(_, id)) => id,
            None => {
                let id = symbol(&mut obj, name, 0, 0, SymbolSection::Undefined);
                ids.push((name, id));
                id
            }
        };
        obj.add_relocation(
            text,
            Relocation {
                offset,
                symbol: id,
                addend: 0,
                flags: object::RelocationFlags::Elf { r_type },
            },
        )
        .unwrap();
    }
    obj.write().unwrap()
}

/// The addresses `name`'s observations of `[0x20100000, 0x20102000)` hold
/// in `inputs`, analyzed with returned constants.
fn returned_addresses(inputs: &[Executable], name: &str) -> Vec<u32> {
    let (_, records) = accesses_with(
        inputs,
        &[ImageRegion {
            start: 0x2010_0000,
            length: 0x3000,
        }],
        Assumptions {
            abi: None,
            call_returns: true,
            read_only: &[],
        },
    )
    .unwrap();
    let mut addresses: Vec<u32> = records
        .iter()
        .filter_map(|record| match record {
            RegisterAccess::Observation {
                function, address, ..
            } if function.name.as_deref() == Some(name.as_bytes()) => *address,
            _ => None,
        })
        .collect();
    addresses.sort_unstable();
    addresses
}

#[test]
fn an_object_call_takes_its_global_callees_returned_constants() {
    // helper: lui a0, 0x20101; ret.
    let helper = functions_object(&[0x2010_1537, 0x0000_8067], &[("helper", 0, 8)], &[]);
    // by_jal: jal ra, helper; lw a0, 0(a0); ret.
    // by_call: auipc ra, %hi(helper); jalr ra, %lo(helper)(ra); lw a0, 0(a0); ret.
    let caller = functions_object(
        &[
            0x0000_00ef,
            0x0005_2503,
            0x0000_8067,
            0x0000_0097,
            0x0000_80e7,
            0x0005_2503,
            0x0000_8067,
        ],
        &[("by_jal", 0, 12), ("by_call", 12, 16)],
        &[
            (0, "helper", object::elf::R_RISCV_JAL),
            (12, "helper", object::elf::R_RISCV_CALL_PLT),
        ],
    );
    let library = Executable::new(support::archive(
        &[(b"helper.o", &helper), (b"caller.o", &caller)],
        false,
    ));
    let inputs = [library];
    assert_eq!(returned_addresses(&inputs, "by_jal"), [0x2010_1000]);
    assert_eq!(returned_addresses(&inputs, "by_call"), [0x2010_1000]);
}

#[test]
fn a_symbol_defined_with_other_results_answers_nothing() {
    // helper: lui a0, 0x20101; ret.
    let helper = functions_object(&[0x2010_1537, 0x0000_8067], &[("helper", 0, 8)], &[]);
    // helper: beq a1, zero, elsewhere; lui a0, 0x20101; ret. Its branch
    // out leaves through other code, so it gives no results.
    let branching = functions_object(
        &[0x0005_8063, 0x2010_1537, 0x0000_8067],
        &[("helper", 0, 12)],
        &[(0, "elsewhere", object::elf::R_RISCV_BRANCH)],
    );
    // by_jal: jal ra, helper; lw a0, 0(a0); ret.
    let caller = functions_object(
        &[0x0000_00ef, 0x0005_2503, 0x0000_8067],
        &[("by_jal", 0, 12)],
        &[(0, "helper", object::elf::R_RISCV_JAL)],
    );
    let library = Executable::new(support::archive(
        &[
            (b"helper.o", &helper),
            (b"branching.o", &branching),
            (b"caller.o", &caller),
        ],
        false,
    ));
    assert_eq!(returned_addresses(&[library], "by_jal"), Vec::<u32>::new());
}

#[test]
fn a_callee_that_branches_out_returns_nothing() {
    const RET: u32 = 0x0000_8067;
    let code = [
        // cond @0x1000: bne a1, zero, other; lui a0, 0x20101; ret.
        0x0005_9663,
        0x2010_1537,
        RET,
        // other @0x100c: lui a0, 0x20102; ret.
        0x2010_2537,
        RET,
        // caller @0x1014: jal ra, cond; lw a0, 0(a0); ret.
        call(0x1014, 0x1000),
        0x0005_2503,
        RET,
    ];
    let image = support::executable_with_symbols(
        &code,
        &[
            ("cond", 0x1000, 12),
            ("other", 0x100c, 8),
            ("caller", 0x1014, 12),
        ],
    );
    assert_eq!(
        returned_addresses(&[Executable::new(image)], "caller"),
        Vec::<u32>::new()
    );
}

#[test]
fn the_read_only_windows_are_recorded_and_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("image.elf");
    std::fs::write(&path, support::executable(&[0x0000_8067])).unwrap();
    let run = |window: &str| {
        Command::new(env!("CARGO_BIN_EXE_blobray"))
            .args([
                "--format",
                "json",
                "register-accesses",
                "--read-only",
                window,
            ])
            .arg("--input")
            .arg(format!("image={}", path.display()))
            .output()
            .unwrap()
    };
    let output = run("0x40000000:0x100");
    assert!(output.status.success(), "{output:?}");
    let document: oer_tool_analysis_cli::wire::RegisterAccessDocument =
        serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        document.read_only,
        [ReadOnlyWindow {
            start: 0x4000_0000,
            length: 0x100,
        }]
    );
    // A window past the RV32 address space is refused.
    let refused = run("0xffffff00:0x200");
    assert!(!refused.status.success());
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("exceeds the RV32 address space"),
        "{refused:?}"
    );
}
