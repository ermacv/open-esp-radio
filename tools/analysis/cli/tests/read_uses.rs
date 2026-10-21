//! Pinned-toolchain RV32 fixtures through the public register-access API.
use oer_analysis_blobray_application::{
    in_process::Executable,
    library::{Assumptions, register_accesses},
};
use oer_analysis_blobray_domain::*;
use oer_analysis_riscv_model::*;
use std::{collections::BTreeMap, path::Path, process::Command};
#[path = "support/stored_property.rs"]
mod stored_property;

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
    let mut observations = Vec::new();
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
            observations.push(observation.clone());
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
    for (name, replacement) in [
        ("rmw_variable", 0x3e0),
        ("rmw_clear", 0x3e0),
        ("rmw_constant", 0x3e0),
        ("rmw_set", 32),
        ("rmw_sparse", 0x39f),
        ("rmw_right_shift", 0x1f),
        ("rmw_variable_spilled", 0x3e0),
    ] {
        let reads = read(name);
        assert_eq!(reads.preserved, !replacement, "{name}: {reads:?}");
        assert_eq!(reads.unused, replacement, "{name}: {reads:?}");
        assert!(reads.uses.is_empty(), "{name}: {reads:?}");
        assert!(reads.unknown.is_empty(), "{name}: {reads:?}");
        if name == "rmw_variable_spilled" {
            // The CFG forwards known stack bytes; stored expressions do
            // not forward memory and cannot prove this store's RMW shape.
            assert!(!observations.iter().any(|observation| matches!(observation,
                RegisterAccess::Observation { function, mask: Some(RegisterMask { kind: RegisterMaskKind::WriteReplacement, .. }), .. }
                    if function.name.as_deref() == Some(name.as_bytes()))));
            continue;
        }
        let mask = observations
            .iter()
            .find_map(|observation| match observation {
                RegisterAccess::Observation { function, mask, .. }
                    if function.name.as_deref() == Some(name.as_bytes())
                        && mask.is_some_and(|mask| {
                            mask.kind == RegisterMaskKind::WriteReplacement
                        }) =>
                {
                    *mask
                }
                _ => None,
            })
            .expect("RMW replacement mask");
        assert_eq!(mask.bits, replacement, "{name}");
    }
    assert_eq!(read("rmw_join").preserved, !1);
    assert!(read("rmw_join").uses.iter().any(|usage| usage.bits == 1
        && usage.precision == ReadPrecision::May
        && matches!(
            usage.sink,
            ReadSink::Store {
                same_word: true,
                ..
            }
        )));
    assert_eq!(read("rmw_xor").preserved, !1);
    assert!(read("rmw_xor").uses.iter().any(|usage| usage.bits == 1
        && usage.precision == ReadPrecision::Exact
        && matches!(
            usage.sink,
            ReadSink::Store {
                same_word: true,
                ..
            }
        )));
    let two = &by_name[b"rmw_two_loads".as_slice()];
    assert_eq!(two[0].1.preserved, 0);
    assert!(two[0].1.uses.iter().any(|usage| usage.bits == 0x3fff
        && matches!(
            usage.sink,
            ReadSink::Store {
                same_word: true,
                ..
            }
        )));
    assert_eq!(two[1].1.preserved, 0xf0003fff);
    assert!(two[1].1.uses.is_empty());
    // A bit which stored provenance identifies as an identity copy cannot
    // also be consumed by that same store. Other sinks remain independent.
    for observation in &observations {
        let RegisterAccess::Observation {
            function,
            fact,
            stored: Some(stored),
            address,
            ..
        } = observation
        else {
            continue;
        };
        let FunctionRecord::MemoryAccess {
            offset,
            access: MemoryKind::Store,
            ..
        } = fact.as_ref()
        else {
            continue;
        };
        // Public stored runs omit the load-site id. Compare their masks
        // only when the source load is unambiguous; the two-load regression
        // above checks preservation and a moved source bit separately.
        let loads = observations
            .iter()
            .filter(|candidate| {
                matches!(candidate,
            RegisterAccess::Observation { function: f, reads: Some(_), address: a, .. }
                if f == function && a == address)
            })
            .count();
        if loads != 1 {
            continue;
        }
        let preserved = stored_property::identity_bits(stored);
        for read_observation in &observations {
            if let RegisterAccess::Observation {
                function: loaded_function,
                reads: Some(reads),
                address: loaded_address,
                ..
            } = read_observation
                && function == loaded_function
                && address == loaded_address
            {
                for usage in &reads.uses {
                    if matches!(usage.sink, ReadSink::Store { site, same_word: true, .. } if site == *offset)
                    {
                        assert_eq!(
                            usage.bits & preserved,
                            0,
                            "stored/read-flow disagreement: {observation:?}, {reads:?}"
                        );
                    }
                }
            }
        }
    }
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
                && usage.precision
                    == if name == "call_argument" {
                        ReadPrecision::May
                    } else {
                        ReadPrecision::Exact
                    }),
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
core::arch::global_asm!(".section .text.external,\"ax\",@progbits\n.globl external\n.type external,@function\nexternal: bnez a0, 2f\n2: ret\n.size external, .-external");
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

#[test]
fn argument_summaries_propagate_consumption_and_return_positions() {
    for call_returns in [false, true] {
        let memory = WorkingMemory::new(64 << 20).expect("working memory");
        let mut found = BTreeMap::new();
        register_accesses(
            &[Executable::new(fixture(include_str!(
                "fixtures/argument_summaries.rs"
            )))],
            Assumptions {
                abi: Some(CallAbi::RiscvInteger),
                call_returns,
                ..Assumptions::default()
            },
            &[ImageRegion {
                start: 0x2010_7000,
                length: 4,
            }],
            &oer_analysis_riscv_lift::RiscvDecoder,
            &memory,
            &mut || Ok(()),
            &mut |observation, _| {
                if let RegisterAccess::Observation {
                    function,
                    fact,
                    reads: Some(reads),
                    ..
                } = observation
                    && matches!(
                        fact.as_ref(),
                        FunctionRecord::MemoryAccess {
                            access: MemoryKind::Load,
                            ..
                        }
                    )
                {
                    partition(reads, 4);
                    found.insert(
                        String::from_utf8(function.name.clone().expect("name")).expect("utf8"),
                        reads.clone(),
                    );
                }
                Ok(())
            },
        )
        .expect("argument summaries");
        for name in [
            "stale_a3",
            "a3_ignored",
            "a5_overwritten",
            "result_discarded",
        ] {
            let reads = &found[name];
            assert!(
                reads.uses.is_empty() && reads.unknown.is_empty(),
                "{name}: {reads:?}"
            );
            assert_eq!(reads.unused, u32::MAX, "{name}");
        }
        for name in ["result_branch", "tail_result_branch", "high_result_branch"] {
            let reads = &found[name];
            assert_eq!(reads.unused, !16, "{name}: {reads:?}");
            assert_eq!(reads.uses.len(), 1, "{name}: {reads:?}");
            assert!(
                reads.uses.iter().any(|usage| usage.bits == 16
                    && usage.precision == ReadPrecision::Exact
                    && matches!(usage.sink, ReadSink::Branch { .. })),
                "{name}: {reads:?}"
            );
        }
        for (name, bits) in [
            ("a0_consumed", 16),
            ("nested_consumption", 16),
            ("stored_argument", 4),
            ("address_argument", u32::MAX),
        ] {
            let reads = &found[name];
            assert_eq!(reads.unused, !bits, "{name}: {reads:?}");
            assert!(
                reads.uses.iter().any(|usage| usage.bits == bits
                    && usage.precision == ReadPrecision::Exact
                    && matches!(usage.sink, ReadSink::CallArgument { register: 10, .. })),
                "{name}: {reads:?}"
            );
        }
        for name in [
            "recursive_arguments",
            "self_recursive_arguments",
            "partial_arguments",
            "external_arguments",
            "indirect_arguments",
            "tail_unknown_arguments",
        ] {
            let reads = &found[name];
            assert_eq!(reads.unused, 0, "{name}: {reads:?}");
            assert!(
                reads.unknown.is_empty(),
                "May remains use: {name}: {reads:?}"
            );
            assert!(
                reads.uses.iter().any(|usage| usage.bits == u32::MAX
                    && usage.precision == ReadPrecision::May
                    && matches!(usage.sink, ReadSink::CallArgument { .. })),
                "{name}: {reads:?}"
            );
        }
        let may_return = &found["may_result_branch"];
        assert_eq!(may_return.unused, !3, "{may_return:?}");
        assert!(
            may_return.uses.iter().any(|u| u.bits == 3
                && u.precision == ReadPrecision::May
                && matches!(u.sink, ReadSink::Branch { .. })),
            "{may_return:?}"
        );
        for name in ["may_consumption", "may_caller_consumption"] {
            let reads = &found[name];
            assert!(
                reads.uses.iter().all(|u| u.precision == ReadPrecision::May),
                "{name}: {reads:?}"
            );
            assert!(!reads.uses.is_empty(), "{name}");
        }
        assert_eq!(found["identity_store"].preserved, u32::MAX);
        assert!(found["identity_store"].uses.is_empty());
        let inverted = &found["inverted_store"];
        assert_eq!(inverted.preserved, 0);
        assert!(
            inverted.uses.iter().any(|usage| usage.bits == u32::MAX
                && matches!(
                    usage.sink,
                    ReadSink::Store {
                        same_word: true,
                        ..
                    }
                )),
            "{inverted:?}"
        );
        assert_eq!(memory.used(), 0);
    }
}

#[test]
fn finite_rmw_fields_agree_with_read_flow_for_every_byte_and_clearing_form() {
    let mut source =
        String::from("#![no_std]\ncore::arch::global_asm!(r#\"\n.option norvc\n.option norelax\n");
    let mut expected = BTreeMap::new();
    for (a, b) in [(0x80u32, 0xa0u32), (1, 5), (0, 0x55), (0x7f, 0xff)] {
        for byte in 0..4 {
            for reversed in [false, true] {
                for shifts in [false, true] {
                    if shifts && byte != 3 {
                        continue;
                    }
                    let name = format!("finite_{a}_{b}_{byte}_{reversed}_{shifts}");
                    let shift = byte * 8;
                    let mask = 0xffu32 << shift;
                    let clear = if shifts {
                        "slli a4, a4, 8\nsrli a4, a4, 8".to_owned()
                    } else if byte == 0 {
                        "andi a4, a4, -256".to_owned()
                    } else {
                        format!("li t1, {}\nand a4, a4, t1", !mask as i32)
                    };
                    let or = if reversed {
                        "or a4, a3, a4"
                    } else {
                        "or a4, a4, a3"
                    };
                    source.push_str(&format!(
                        r#"
.section .text.{name},"ax",@progbits
.globl {name}
.type {name},@function
{name}:
    beqz a0, 2f
    li a3, {a}
    j 3f
2:  li a3, {b}
3:  lui a5, 0x20107
    lw a4, 0(a5)
    {clear}
    {or}
    sw a4, 0(a5)
    ret
.size {name}, .-{name}
"#,
                        a = (a << shift) as i32,
                        b = (b << shift) as i32,
                    ));
                    expected.insert(name.into_bytes(), (a << shift, b << shift, mask));
                }
            }
        }
    }
    source.push_str("\"#);\n");
    let mut observations = Vec::new();
    let memory = WorkingMemory::new(64 * 1024 * 1024).expect("working memory");
    let summary = register_accesses(
        &[Executable::new(fixture(&source))],
        Assumptions {
            abi: Some(CallAbi::RiscvInteger),
            ..Assumptions::default()
        },
        &[ImageRegion {
            start: 0x2010_7000,
            length: 4,
        }],
        &oer_analysis_riscv_lift::RiscvDecoder,
        &memory,
        &mut || Ok(()),
        &mut |observation, _| {
            observations.push(observation.clone());
            Ok(())
        },
    )
    .expect("finite RMW analysis");
    assert_eq!(summary.blocked_functions, 0);
    assert_eq!(summary.partial_functions, 0);
    for (name, (a, b, replacement)) in expected {
        let mut load = None;
        let mut store = None;
        for observation in &observations {
            if let RegisterAccess::Observation {
                function,
                reads,
                stored,
                mask,
                ..
            } = observation
                && function.name.as_ref() == Some(&name)
            {
                if let Some(reads) = reads {
                    assert!(load.replace(reads).is_none());
                }
                if let Some(stored) = stored {
                    assert!(store.replace((stored, mask)).is_none());
                }
            }
        }
        let reads = load.expect("one source load");
        let (stored, mask) = store.expect("one sink store");
        partition(reads, 4);
        assert!(
            reads.uses.is_empty() && reads.unknown.is_empty(),
            "{name:?}: {reads:?}"
        );
        assert_eq!(reads.preserved, !replacement, "{name:?}");
        assert_eq!(reads.unused, replacement, "{name:?}");
        assert_eq!(
            stored_property::missing_preserved_bits(reads, stored),
            0,
            "{name:?}: {stored:?}"
        );
        assert_eq!(
            *mask,
            Some(RegisterMask {
                kind: RegisterMaskKind::WriteReplacement,
                bits: replacement
            }),
            "{name:?}"
        );
        for bit in 0..32 {
            let run = stored
                .iter()
                .find(|run| bit >= run.low && bit < run.low + run.width)
                .expect("covered bit");
            if replacement & (1 << bit) == 0 {
                assert!(
                    matches!(run.source, StoredBitsSource::Load { same_word: true, low, .. } if low + bit - run.low == bit)
                );
            } else if (a ^ b) & (1 << bit) != 0 {
                assert_eq!(run.source, StoredBitsSource::Unknown, "{name:?}: bit {bit}");
            } else {
                let StoredBitsSource::Constant { value } = run.source else {
                    panic!("{name:?}: bit {bit}: {run:?}")
                };
                assert_eq!(
                    (value >> (bit - run.low)) & 1,
                    (a >> bit) & 1,
                    "{name:?}: bit {bit}"
                );
            }
        }
    }
}
