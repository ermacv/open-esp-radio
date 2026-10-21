//! Pinned-toolchain RV32 fixtures through the public register-access API.
use oer_analysis_blobray_application::{
    in_process::Executable,
    library::{Assumptions, register_accesses},
};
use oer_analysis_blobray_domain::*;
use oer_analysis_riscv_model::*;
use std::{collections::BTreeMap, path::Path, process::Command};

fn partition(reads: &ReadUses, width: u8) {
    let used = reads.uses.iter().fold(0, |bits, usage| bits | usage.bits);
    let unknown = reads
        .unknown
        .iter()
        .fold(0, |bits, limit| bits | limit.bits);
    let classes = [used, unknown, reads.preserved, reads.unused];
    let mask = u32::MAX >> (32 - u32::from(width) * 8);
    assert_eq!(classes.iter().fold(0, |a, b| a | b), mask, "{reads:?}");
    for (i, a) in classes.iter().enumerate() {
        for b in classes.iter().skip(i + 1) {
            assert_eq!(a & b, 0, "{reads:?}");
        }
    }
    assert!(reads.uses.iter().all(|usage| usage.bits != 0));
    assert!(reads.unknown.iter().all(|limit| limit.bits != 0));
}

#[allow(clippy::expect_used)] // Fixture setup failures fail this compiler-driven test.
fn fixture(contents: &str) -> Vec<u8> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let temp = tempfile::tempdir_in(root.join("target")).expect("fixture directory");
    let source = temp.path().join("fixture.rs");
    let output = temp.path().join("fixture.o");
    std::fs::write(&source, contents).expect("fixture source");
    let compiled = Command::new("rustc")
        .args([
            "--target",
            "riscv32imac-unknown-none-elf",
            "--crate-type",
            "lib",
            "--emit=obj",
            "--edition=2024",
            "-C",
            "panic=abort",
        ])
        .arg(&source)
        .arg("-o")
        .arg(&output)
        .output()
        .expect("pinned rustc");
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    std::fs::read(output).expect("fixture object")
}

#[test]
fn loads_report_their_sinks_limits_preservation_and_unused_bits() {
    let memory = WorkingMemory::new(64 * 1024 * 1024).expect("working memory");
    let mut by_name: BTreeMap<Vec<u8>, Vec<(u64, ReadUses)>> = BTreeMap::new();
    let mut functions = BTreeMap::new();
    let summary = register_accesses(
        &[Executable::new(fixture(include_str!(
            "fixtures/read_uses.rs"
        )))],
        Assumptions {
            abi: Some(CallAbi::RiscvInteger),
            ..Assumptions::default()
        },
        &[ImageRegion {
            start: 0x2010_7000,
            length: 0x100,
        }],
        &oer_analysis_riscv_lift::RiscvDecoder,
        &memory,
        &mut || Ok(()),
        &mut |observation, _| {
            if let RegisterAccess::Observation {
                function,
                fact,
                reads,
                ..
            } = observation
            {
                functions.insert(
                    function.name.clone().expect("fixture name"),
                    function.clone(),
                );
                if let FunctionRecord::MemoryAccess {
                    access: MemoryKind::Load,
                    width,
                    offset,
                    ..
                } = fact.as_ref()
                {
                    let reads = reads.as_ref().expect("every load has reads");
                    partition(reads, *width);
                    let wire = serde_json::to_value(observation).expect("serialize reads");
                    assert_eq!(
                        serde_json::from_value::<RegisterAccess>(wire)
                            .expect("read uses roundtrip"),
                        *observation
                    );
                    by_name
                        .entry(function.name.clone().expect("named fixture"))
                        .or_default()
                        .push((*offset, reads.clone()));
                } else {
                    assert!(reads.is_none(), "only load observations carry reads");
                }
            }
            Ok(())
        },
    )
    .expect("register-accesses");
    assert_eq!(summary.blocked_functions, 0);
    assert_eq!(memory.used(), 0, "all evaluator admission is released");
    let read = |name: &str| &by_name.get(name.as_bytes()).expect("fixture load")[0].1;
    assert_eq!(read("unused_read").unused, u32::MAX);
    assert_eq!(
        read("branch_read").uses,
        [ReadUse {
            bits: u32::MAX,
            sink: ReadSink::Branch { site: 8 },
            precision: ReadPrecision::Exact
        }]
    );
    assert_eq!(read("preserve_rmw").preserved, !1);
    assert_eq!(read("preserve_rmw").unused, 1);
    assert!(read("preserve_rmw").uses.is_empty());
    assert!(!by_name.contains_key(b"reset_write".as_slice()));
    assert!(!by_name.contains_key(b"nonreset_write".as_slice()));
    let shifted = read("shifted_branch");
    assert_eq!(shifted.unused, !16);
    assert_eq!(
        shifted.uses,
        [ReadUse {
            bits: 16,
            sink: ReadSink::Branch { site: 16 },
            precision: ReadPrecision::Exact
        }]
    );
    assert!(
        read("arithmetic_branch")
            .uses
            .iter()
            .any(|usage| usage.bits == u32::MAX
                && usage.precision == ReadPrecision::May
                && matches!(usage.sink, ReadSink::Branch { .. }))
    );
    for (name, sink) in [
        ("load_address", ReadSink::Address { site: 8 }),
        (
            "call_argument",
            ReadSink::CallArgument {
                site: 12,
                register: 10,
                callee: Some(CallTarget {
                    address: None,
                    function: None,
                    name: Some("external".to_owned()),
                }),
            },
        ),
        ("return_value", ReadSink::Return { register: 10 }),
        (
            "store_other",
            ReadSink::Store {
                site: 8,
                address: Some(0x2010_7004),
                same_word: false,
            },
        ),
    ] {
        assert!(
            read(name).uses.iter().any(|usage| usage.bits == u32::MAX
                && usage.sink == sink
                && usage.precision == ReadPrecision::Exact),
            "{name}: {:?}",
            read(name)
        );
    }
    for name in ["call_named_argument", "tail_named_argument"] {
        let callee = read(name)
            .uses
            .iter()
            .find_map(|usage| match &usage.sink {
                ReadSink::CallArgument {
                    register: 10,
                    callee,
                    ..
                } => callee.as_ref(),
                _ => None,
            })
            .expect("named callee");
        assert_eq!(
            callee.address, None,
            "relocatable offsets are not image addresses"
        );
        assert_eq!(callee.name.as_deref(), Some("named_callee"));
        let function = callee.function.as_ref().expect("defining input function");
        assert_eq!(function.input, 0);
        assert_eq!(function.name.as_deref(), Some(b"named_callee".as_slice()));
        assert_eq!(
            function.symbol.object,
            functions[b"call_named_argument".as_slice()].symbol.object
        );
    }
    assert!(
        read("call_indirect_argument")
            .uses
            .iter()
            .any(|usage| matches!(
                &usage.sink,
                ReadSink::CallArgument {
                    register: 10,
                    callee: None,
                    ..
                }
            )),
        "unresolved register call: {:?}",
        read("call_indirect_argument")
    );
    assert!(read("call_local_jal_argument").uses.iter().any(|usage| matches!(&usage.sink,
        ReadSink::CallArgument { register: 10, callee: Some(CallTarget { address: None, function: Some(function), name }), .. } if name.as_deref() == Some("local_jal_callee") && function.input == 0
    )), "direct CFG target without a relocation: {:?}", read("call_local_jal_argument"));
    assert!(read("call_rom_argument").uses.iter().any(|usage| matches!(&usage.sink,
        ReadSink::CallArgument { register: 10, callee: Some(CallTarget { address: Some(0x40012340), function: None, name }), .. } if name.as_deref() == Some("rom_print")
    )), "resolved register call to ROM: {:?}", read("call_rom_argument"));
    assert!(
        read("indirect_target")
            .uses
            .iter()
            .any(|usage| usage.bits == !1 && usage.sink == ReadSink::IndirectTarget { site: 8 })
    );
    let moved = read("moved_bit");
    assert_eq!(moved.preserved, 0);
    assert_eq!(moved.unused, !1);
    assert!(moved.uses.iter().any(|usage| usage.bits == 1
        && matches!(
            usage.sink,
            ReadSink::Store {
                same_word: true,
                ..
            }
        )));
    assert_eq!(read("inverted_bit").preserved, !1);
    assert!(read("inverted_bit").uses.iter().any(|usage| usage.bits == 1
        && matches!(
            usage.sink,
            ReadSink::Store {
                same_word: true,
                ..
            }
        )));
    for (name, limit) in [
        ("partial_function", ReadLimitKind::PartialFunction),
        ("stack_spill", ReadLimitKind::StackSpill),
        ("unsupported_operation", ReadLimitKind::UnsupportedOperation),
    ] {
        assert_eq!(
            read(name).unknown,
            [ReadLimit {
                bits: u32::MAX,
                limit
            }],
            "{name}: {:?}",
            read(name)
        );
    }
    assert_eq!(read("signed_byte").uses[0].bits, 0xff);
    assert_eq!(read("repeated_loads").unused, u32::MAX);
    assert!(
        by_name[b"repeated_loads".as_slice()][1]
            .1
            .uses
            .iter()
            .any(|usage| matches!(usage.sink, ReadSink::Branch { .. }))
    );
    assert!(
        read("used_and_spilled").unknown.is_empty(),
        "used takes precedence over unknown"
    );
    assert_eq!(read("variable_and").uses[0].bits, 1);
    assert!(
        read("lost_join")
            .uses
            .iter()
            .any(|usage| usage.bits == u32::MAX
                && usage.precision == ReadPrecision::Exact
                && matches!(usage.sink, ReadSink::Branch { site: 16 })),
        "join must retain the load on the path that carries it: {:?}",
        read("lost_join")
    );
    assert!(read("lost_join").unknown.is_empty());
    assert!(
        read("loop_accumulate")
            .uses
            .iter()
            .any(|usage| usage.bits == u32::MAX
                && usage.precision == ReadPrecision::May
                && matches!(usage.sink, ReadSink::Branch { .. })),
        "{:?}",
        read("loop_accumulate")
    );
    for name in ["stack_roundtrip", "saved_across_call"] {
        assert!(
            read(name).uses.iter().any(|usage| usage.bits == u32::MAX
                && usage.precision == ReadPrecision::Exact
                && matches!(usage.sink, ReadSink::Branch { .. })),
            "{name}: {:?}",
            read(name)
        );
        assert!(read(name).unknown.is_empty(), "{name}: {:?}", read(name));
    }
    assert_eq!(
        read("isolated_opaque").unknown,
        [ReadLimit {
            bits: u32::MAX,
            limit: ReadLimitKind::UnsupportedOperation
        }]
    );
    assert_eq!(
        by_name
            .get(b"isolated_opaque".as_slice())
            .expect("two independent loads")
            .get(1)
            .expect("second load")
            .1
            .unused,
        u32::MAX
    );
    assert_eq!(read("dead_before_open_edge").unused, u32::MAX);
    assert!(
        read("dispatch_index")
            .uses
            .iter()
            .any(|usage| usage.bits == 3 && matches!(usage.sink, ReadSink::Dispatch { .. })),
        "{:?}",
        read("dispatch_index")
    );
    assert_eq!(
        read("unknown_stack_byte").unknown,
        [ReadLimit {
            bits: 0xff,
            limit: ReadLimitKind::StackSpill
        }]
    );
    assert_eq!(read("unknown_stack_byte").unused, !0xff);
    assert!(
        read("conflicting_join")
            .uses
            .iter()
            .any(|usage| usage.bits == 0x110
                && usage.precision == ReadPrecision::May
                && matches!(usage.sink, ReadSink::Branch { .. })),
        "{:?}",
        read("conflicting_join")
    );
    assert!(
        read("call_argument")
            .uses
            .iter()
            .all(|usage| !matches!(usage.sink, ReadSink::Return { .. })),
        "call results carry no dependency on the previous argument"
    );
    assert_eq!(oer_tool_analysis_cli::wire::REGISTER_ACCESSES_SCHEMA, 9);
    assert!(
        oer_tool_analysis_cli::mmio::reads(read("call_named_argument")).contains("named_callee")
    );
    assert!(
        oer_tool_analysis_cli::mmio::reads(read("call_rom_argument"))
            .contains("rom_print@0x40012340")
    );
}

#[test]
fn a_named_callee_can_be_defined_by_another_analyzed_input() {
    let memory = WorkingMemory::new(64 << 20).expect("working memory");
    let inputs = [
        Executable::new(fixture(include_str!("fixtures/read_uses.rs"))),
        Executable::new(fixture(
            r#"#![no_std]
core::arch::global_asm!(".section .text.external,\"ax\",@progbits\n.globl external\n.type external,@function\nexternal: ret\n.size external, .-external");
"#,
        )),
    ];
    let inventory =
        oer_analysis_blobray_application::captured::inventory(&inputs[1], &memory, &mut || Ok(()))
            .expect("callee inventory");
    let symbol = inventory
        .objects
        .iter()
        .filter_map(|o| o.elf.as_ref())
        .flat_map(|elf| &elf.symbols)
        .find(|s| s.name.as_deref() == Some(b"external"))
        .expect("callee symbol");
    let expected = CallTarget {
        address: None,
        function: Some(LibraryFunction {
            input: 1,
            symbol: symbol.id.clone(),
            name: symbol.name.clone(),
        }),
        name: Some("external".to_owned()),
    };
    let mut found = false;
    register_accesses(&inputs, Assumptions { abi: Some(CallAbi::RiscvInteger), ..Assumptions::default() }, &[ImageRegion { start: 0x2010_7000, length: 0x100 }], &oer_analysis_riscv_lift::RiscvDecoder, &memory, &mut || Ok(()), &mut |observation, _| {
        if let RegisterAccess::Observation { function, reads: Some(reads), .. } = observation
            && function.name.as_deref() == Some(b"call_argument") {
            found = true;
            assert!(reads.uses.iter().any(|usage| matches!(&usage.sink, ReadSink::CallArgument { callee: Some(callee), .. } if callee == &expected)), "{reads:?}");
        }
        Ok(())
    }).expect("cross-input callee");
    assert!(found);
    assert_eq!(memory.used(), 0, "symbol indexes release their admission");
}
