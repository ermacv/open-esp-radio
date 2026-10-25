use super::*;
use crate::checks::inventory::{DeclaredField, DeclaredRegister, declared_words};
use oer_analysis_blobray_domain::{
    CallTarget, ImageRegion, LibraryFunction, PartialCauses, ReadLimit, ReadLimitKind, ReadSink,
    ReadUse,
};
use oer_analysis_riscv_model::{
    AbstractValue, Error, ErrorCode, ObjectId, ObjectLocation, SymbolId, SymbolTableKind,
};
use serde_json::Value;

fn declaration(reset: Option<u64>, fields: u32) -> BTreeMap<u32, DeclaredWord> {
    declared_words(&[DeclaredRegister {
        address: 0x1000,
        width_bits: 32,
        name: "CONTROL".into(),
        template: "CONTROL".into(),
        reset_value: reset,
        reset_mask: None,
        reserved: None,
        fields: (0..32)
            .filter(|bit| fields & (1 << bit) != 0)
            .map(|offset| DeclaredField {
                name: format!("BIT{offset}"),
                offset,
                width: 1,
            })
            .collect(),
    }])
}

fn symbol(member: &[u8]) -> SymbolId {
    SymbolId {
        object: ObjectId {
            artifact: ArtifactId::of_bytes(member),
            location: ObjectLocation::Standalone,
        },
        table: SymbolTableKind::Static,
        table_section: 1,
        index: 1,
    }
}

fn observation(access: MemoryKind, address: Option<u32>, width: u8) -> RegisterAccess {
    RegisterAccess::Observation {
        function: LibraryFunction {
            input: 0,
            name: Some(b"synthetic".to_vec()),
            symbol: symbol(b"fixture"),
        },
        record: 1,
        fact: Box::new(FunctionRecord::MemoryAccess {
            access,
            width,
            offset: 16,
            address: AbstractValue::Unknown,
            value: None,
            relocation: None,
        }),
        address,
        alternative: None,
        indexed: None,
        mask: None,
        stored: None,
        reads: None,
    }
}

fn read(
    uses: Vec<ReadUse>,
    unknown: Vec<ReadLimit>,
    preserved: u32,
    unused: u32,
) -> RegisterAccess {
    let mut observation = observation(MemoryKind::Load, Some(0x1000), 4);
    if let RegisterAccess::Observation { reads, .. } = &mut observation {
        *reads = Some(ReadUses {
            uses,
            unknown,
            preserved,
            unused,
        });
    }
    observation
}

fn constant(value: u32) -> RegisterAccess {
    let mut observation = observation(MemoryKind::Store, Some(0x1000), 4);
    if let RegisterAccess::Observation { stored, .. } = &mut observation {
        *stored = Some(vec![StoredBits {
            low: 0,
            width: 32,
            source: StoredBitsSource::Constant { value },
        }]);
    }
    observation
}

fn located(
    mut observation: RegisterAccess,
    at: Option<u32>,
    progression: Option<IndexedAddress>,
) -> RegisterAccess {
    if let RegisterAccess::Observation {
        address, indexed, ..
    } = &mut observation
    {
        *address = at;
        *indexed = progression;
    }
    observation
}

fn branch(bits: u32, precision: ReadPrecision) -> ReadUse {
    ReadUse {
        bits,
        precision,
        sink: ReadSink::Branch { site: 20 },
    }
}

fn limit(bits: u32, limit: ReadLimitKind) -> ReadLimit {
    ReadLimit { bits, limit }
}

fn inputs() -> [Input; 1] {
    [Input {
        role: "fixture".into(),
        sha256: ArtifactId::of_bytes(b"synthetic"),
    }]
}

fn assumptions() -> Assumptions<'static> {
    Assumptions {
        functions: &[],
        symbolic_stores: false,
        pointer_chains: false,
        abi: Some(CallAbi::RiscvInteger),
        call_summaries: true,
        signatures: None,
        read_only: &[],
        call_results: &[],
    }
}

fn regions() -> [Region; 1] {
    [Region {
        name: "synthetic".into(),
        start: 0x1000,
        end: 0x1100,
    }]
}

fn summary(records: &[RegisterAccess]) -> RegisterAccessSummary {
    let mut summary = RegisterAccessSummary {
        schema: 4,
        flow_local_budget_functions: 0,
        summary_outcomes: Default::default(),
        rom_signature_summary_unavailable_reasons: Default::default(),
        signature_audit_incomplete_reasons: Default::default(),
        contract_narrowed_bits: 0,
        contract_may_narrowed_bits: 0,
        signature_conflicts: Vec::new(),
        rom_signature_diagnostics: Vec::new(),
        rom_signature_diagnostic_count: 0,
        rom_signature_summary_unavailable: Vec::new(),
        signature_audit_incomplete: Vec::new(),
        functions: 1,
        partial_functions: 0,
        partial_causes: PartialCauses::default(),
        blocked_functions: 0,
        gaps: 0,
        observations: 0,
        unresolved_addresses: 0,
        indexed_addresses: 0,
        summary_failure_results: Default::default(),
        ranges: vec![ImageRegion {
            start: 0x1000,
            length: 256,
        }],
    };
    for record in records {
        match record {
            RegisterAccess::Observation {
                address, indexed, ..
            } => {
                summary.observations += 1;
                summary.unresolved_addresses += u64::from(address.is_none());
                summary.indexed_addresses += u64::from(indexed.is_some());
            }
            RegisterAccess::CallResult { .. } => summary.observations += 1,
            RegisterAccess::Blocked { .. } => summary.blocked_functions += 1,
            RegisterAccess::Gap { .. } => summary.gaps += 1,
        }
    }
    summary
}

fn check_records(
    records: &[RegisterAccess],
    declared: &BTreeMap<u32, DeclaredWord>,
) -> Result<Report> {
    check_on_chip("esp32s31", records, declared)
}

fn check_on_chip(
    chip: &str,
    records: &[RegisterAccess],
    declared: &BTreeMap<u32, DeclaredWord>,
) -> Result<Report> {
    let (inputs, regions) = (inputs(), regions());
    let mut check = Accumulator::new(chip, &inputs, assumptions(), &regions, declared, &[])?;
    records
        .iter()
        .try_for_each(|record| check.observe(record))?;
    Ok(check.finish(summary(records)))
}

fn run(records: Vec<RegisterAccess>, reset: Option<u64>, fields: u32) -> Report {
    check_records(&records, &declaration(reset, fields)).expect("valid synthetic corpus")
}

#[test]
fn reserved_and_absent_uses_are_distinct_errors_with_cited_masks() {
    let mut declared = declaration(Some(0), 1);
    declared
        .get_mut(&0x1000)
        .unwrap()
        .reserved
        .insert("vendor-header".into(), 2);
    let report = check_records(&[constant(6)], &declared).unwrap();
    let word = &report.words[0];
    assert_eq!(word.reserved_bits, 2);
    assert_eq!(word.reserved_violation_bits, 2);
    assert_eq!(word.absent_violation_bits, 4);
    assert_eq!(word.violation_bits, 6);
    let text = report.render("fixture");
    assert!(text.contains("reserved-used 0x00000002; absent-used 0x00000004"));
    assert!(text.contains("RESERVED SOURCE vendor-header bits 0x00000002"));
    let preserved = read(vec![], vec![], u32::MAX, 0);
    let clean = check_records(&[preserved, constant(0)], &declared).unwrap();
    assert_eq!(clean.violations(), 0);
    assert_eq!(clean.words[0].reserved_bits, 2);
}

#[test]
fn the_reproduce_command_names_the_profile_and_the_chips_signatures() {
    let report = check_records(&[], &declaration(None, 1)).unwrap();
    let text = report.render("fixture");
    let command = text
        .lines()
        .find(|line| line.contains("Reproduce JSON"))
        .unwrap();
    assert!(
        command.contains(" --analysis-profile register-inventory --signatures chips/fixture/vendor/facts/idf-function-signatures.toml "),
        "{command}"
    );
    assert!(!command.contains("--timeout-secs"), "{command}");
}

#[test]
fn reserved_branch_reads_fail_and_reserved_unknown_remains_separate() {
    let mut declared = declaration(None, 1);
    declared
        .get_mut(&0x1000)
        .unwrap()
        .reserved
        .insert("vendor-header".into(), 6);
    let observation = read(
        vec![branch(2, ReadPrecision::Exact)],
        vec![limit(4, ReadLimitKind::PartialFunction)],
        0,
        !6u32,
    );
    let report = check_records(&[observation], &declared).unwrap();
    assert_eq!(report.words[0].reserved_violation_bits, 2);
    assert_eq!(report.words[0].absent_violation_bits, 0);
    assert_eq!(report.words[0].unknown_bits, 4);
}

#[test]
fn reset_store_is_inactive_and_nonreset_store_rejects_only_changed_absent_bits() {
    let report = run(vec![constant(0xa5)], Some(0xa5), 0);
    assert_eq!(report.violations(), 0);
    assert_eq!(report.words[0].inactive_bits, u32::MAX);
    let report = run(vec![constant(0)], Some(0xa5), 1);
    assert_eq!(report.words[0].used_bits, 0xa5);
    assert_eq!(report.words[0].violation_bits, 0xa4);
    assert_eq!(report.violations(), 1);
}

#[test]
fn rmw_preserves_bits_without_counting_them_as_used() {
    let loaded = read(vec![], vec![], !1, 1);
    let mut stored = constant(1);
    if let RegisterAccess::Observation { stored, mask, .. } = &mut stored {
        *stored = Some(vec![
            StoredBits {
                low: 0,
                width: 1,
                source: StoredBitsSource::Constant { value: 1 },
            },
            StoredBits {
                low: 1,
                width: 31,
                source: StoredBitsSource::Load {
                    address: Some(0x1000),
                    width: 4,
                    low: 1,
                    same_word: true,
                },
            },
        ]);
        *mask = Some(RegisterMask {
            kind: RegisterMaskKind::WriteReplacement,
            bits: 1,
        });
    }
    let report = run(vec![loaded, stored], Some(0), 1);
    assert_eq!(report.violations(), 0);
    assert_eq!(report.words[0].used_bits, 1);
    assert_eq!(report.words[0].unknown_bits, 0);
    assert_eq!(report.words[0].inactive_bits, !1);
}

#[test]
fn explicit_mask_is_use_even_when_it_writes_the_reset_value() {
    let mut stored = constant(0);
    if let RegisterAccess::Observation { mask, .. } = &mut stored {
        *mask = Some(RegisterMask {
            kind: RegisterMaskKind::WriteReplacement,
            bits: 3,
        });
    }
    let report = run(vec![stored], Some(0), 1);
    assert_eq!(report.words[0].violation_bits, 2);
}

#[test]
fn every_read_sink_and_both_precisions_are_use() {
    let sinks = [
        ReadSink::Branch { site: 20 },
        ReadSink::Dispatch { site: 20 },
        ReadSink::IndirectTarget { site: 20 },
        ReadSink::Address { site: 20 },
        ReadSink::Return { register: 10 },
        ReadSink::Store {
            site: 20,
            address: Some(0x1004),
            same_word: false,
        },
        ReadSink::Store {
            site: 20,
            address: Some(0x1000),
            same_word: true,
        },
        ReadSink::CallArgument {
            site: 20,
            register: 10,
            callee: None,
            shift: None,
        },
    ];
    for sink in sinks {
        for precision in [ReadPrecision::Exact, ReadPrecision::May] {
            let report = run(
                vec![read(
                    vec![ReadUse {
                        bits: 16,
                        sink: sink.clone(),
                        precision,
                    }],
                    vec![],
                    0,
                    !16,
                )],
                None,
                0,
            );
            assert_eq!(report.words[0].violation_bits, 16, "{sink:?} {precision:?}");
            assert_eq!(report.words[0].unknown_bits, 0);
        }
    }
}

fn call(bits: u32, name: Option<&str>, precision: ReadPrecision) -> ReadUse {
    ReadUse {
        bits,
        precision,
        sink: ReadSink::CallArgument {
            site: 20,
            register: 10,
            shift: None,
            callee: name.map(|name| CallTarget {
                address: None,
                function: None,
                name: Some(name.into()),
            }),
        },
    }
}

#[test]
fn only_cited_diagnostic_names_are_ignored() {
    for (name, expected_use) in [
        (Some("wifi_log"), false),
        (Some("phy_printf"), false),
        (Some("wr_btdm_compressed_assert_x2"), false),
        (Some("r_ble_log_internal_x2"), false),
        (Some("ets_delay_us"), true),
        (Some("wifi_log_set_level"), true),
        (Some("unknown_printf_wrapper"), true),
        (None, true),
    ] {
        let report = run(
            vec![read(vec![call(1, name, ReadPrecision::May)], vec![], 0, !1)],
            None,
            0,
        );
        assert_eq!(report.words[0].used_bits != 0, expected_use, "{name:?}");
        assert_eq!(report.words[0].diagnostic_bits, u32::from(!expected_use));
        assert_eq!(report.words[0].inactive_bits & 1 != 0, !expected_use);
        assert_eq!(
            report.all_reads.diagnostic_only_bits,
            u64::from(!expected_use)
        );
    }
}

#[test]
fn diagnostics_require_evidence_from_the_selected_chip() {
    let declared = declaration(None, 0);
    for (chip, name, expected_use) in [
        ("esp32s31", "phy_log", false),
        ("esp32s31", "wifi_log", false),
    ] {
        let records = [read(
            vec![call(1, Some(name), ReadPrecision::Exact)],
            vec![],
            0,
            !1,
        )];
        let report =
            check_on_chip(chip, &records, &declared).expect("chip-local synthetic observation");
        assert_eq!(
            report.words[0].used_bits != 0,
            expected_use,
            "{chip} {name}"
        );
        let source = report.all_reads.callees[0].diagnostic_source;
        if expected_use {
            assert!(source.is_none());
        } else {
            assert!(
                source
                    .expect("cited diagnostic")
                    .starts_with(&format!("pin:{chip}/"))
            );
        }
        assert!(check_on_chip("another-chip", &records, &declared).is_err());
    }
}

#[test]
fn diagnostic_calls_do_not_hide_behavior_unknown_or_exact_evidence() {
    let diagnostic = call(7, Some("wifi_log"), ReadPrecision::Exact);
    let may = call(3, Some("ets_delay_us"), ReadPrecision::May);
    let mut exact = branch(2, ReadPrecision::Exact);
    exact.sink = ReadSink::Branch { site: 24 };
    let loaded = read(vec![diagnostic, may, exact], vec![], 0, !7);
    let unknown = read(vec![], vec![limit(4, ReadLimitKind::WorkLimit)], 0, !4);
    let report = run(vec![loaded.clone(), unknown], Some(0), 0);
    let word = &report.words[0];
    assert_eq!(word.used_bits, 3);
    assert_eq!(word.violation_bits, 3);
    assert_eq!(word.may_only_bits, 1);
    assert_eq!(word.unknown_bits, 4);
    assert_eq!(word.inactive_bits, !7);
    assert!(!word.diagnostics.is_empty());
    assert!(report.render("fixture").contains("MAY ONLY 0x00001000"));
    assert_eq!(
        run(vec![loaded, constant(1)], Some(0), 0).words[0].may_only_bits,
        0,
        "a proven nonreset write is exact use"
    );
}

#[test]
fn call_accounting_unions_sites_and_arguments_before_address_expansion() {
    let record = read(
        vec![
            call(3, Some("wifi_log"), ReadPrecision::Exact),
            call(1, Some("wifi_log"), ReadPrecision::Exact),
            call(2, None, ReadPrecision::May),
        ],
        vec![],
        0,
        !3,
    );
    let report = run(vec![record.clone()], None, 0);
    let accounting = &report.resolved_indexed_reads;
    assert_eq!(accounting.call_bits, 2);
    assert_eq!(accounting.diagnostic_call_bits, 2);
    assert_eq!(accounting.diagnostic_only_bits, 1);
    let logger = accounting
        .callees
        .iter()
        .find(|c| c.diagnostic_source.is_some())
        .expect("logger group");
    assert_eq!((logger.bits, logger.exclusive_bits), (2, 1));
    let record = located(
        record,
        None,
        Some(IndexedAddress {
            base: 0x1000,
            stride: 4,
            count: Some(2),
        }),
    );
    let report = run(vec![record.clone()], None, 0);
    assert_eq!(report.words.len(), 2);
    assert_eq!(report.resolved_indexed_reads.call_bits, 2);
    assert_eq!(report.resolved_indexed_reads.load_bits, 32);
    let report = run(vec![located(record, None, None)], None, 0);
    assert_eq!(report.resolved_indexed_reads.loads, 0);
    assert_eq!(report.all_reads.diagnostic_only_bits, 1);
    assert_eq!(report.all_reads.load_bits, 32);
}

#[test]
fn floating_memory_accesses_keep_the_producer_classification_and_unknown_store() {
    let mut loaded = read(
        vec![],
        vec![limit(u32::MAX, ReadLimitKind::UnsupportedOperation)],
        0,
        0,
    );
    if let RegisterAccess::Observation { fact, .. } = &mut loaded
        && let FunctionRecord::MemoryAccess { access, .. } = fact.as_mut()
    {
        *access = MemoryKind::FloatLoad;
    }
    let stored = observation(MemoryKind::FloatStore, Some(0x1000), 4);
    let report = run(vec![loaded, stored], None, 0);
    assert_eq!((report.words[0].loads, report.words[0].stores), (1, 1));
    assert_eq!(report.words[0].unknown_bits, u32::MAX);
    assert_eq!(report.violations(), 0);
}

#[test]
fn unused_reads_do_not_establish_use_and_unknown_stays_separate_across_observations() {
    let unused = read(vec![], vec![], 0, u32::MAX);
    let unknown = read(vec![], vec![limit(4, ReadLimitKind::StackSpill)], 0, !4);
    let report = run(vec![unused.clone(), unknown], None, 0);
    assert_eq!(report.violations(), 0);
    assert_eq!(report.words[0].unknown_bits, 4);
    assert_eq!(report.words[0].inactive_bits, !4);
    let rendered = report.render("fixture");
    assert!(rendered.contains("UNKNOWN 0x00001000"));
    assert!(rendered.contains("StackSpill"));
    let report = run(
        vec![
            unused,
            read(vec![branch(4, ReadPrecision::Exact)], vec![], 0, !4),
        ],
        None,
        0,
    );
    assert_eq!(report.words[0].violation_bits, 4);
    assert_eq!(report.words[0].inactive_bits, !4);
}

#[test]
fn every_read_limit_is_retained_even_on_declared_bits() {
    for cause in [
        ReadLimitKind::PartialFunction,
        ReadLimitKind::StackSpill,
        ReadLimitKind::UnsupportedOperation,
        ReadLimitKind::WorkLimit,
    ] {
        let report = run(
            vec![read(vec![], vec![limit(u32::MAX, cause)], 0, 0)],
            None,
            u32::MAX,
        );
        assert_eq!(report.violations(), 0);
        assert_eq!(report.words[0].unknown_bits, u32::MAX);
        assert_eq!(report.words[0].inactive_bits, 0);
    }
}

#[test]
fn missing_reset_or_unresolved_store_is_unknown_instead_of_opaque_evidence() {
    let report = run(vec![constant(0)], None, 0);
    assert_eq!(report.violations(), 0);
    assert_eq!(report.words[0].unknown_bits, u32::MAX);
    assert_eq!(
        run(
            vec![observation(MemoryKind::Store, Some(0x1000), 4)],
            Some(0),
            0
        )
        .words[0]
            .unknown_bits,
        u32::MAX
    );
    let mut moved = constant(0);
    if let RegisterAccess::Observation { stored, .. } = &mut moved {
        *stored = Some(vec![
            StoredBits {
                low: 0,
                width: 1,
                source: StoredBitsSource::Load {
                    address: Some(0x1000),
                    width: 4,
                    low: 1,
                    same_word: true,
                },
            },
            StoredBits {
                low: 1,
                width: 31,
                source: StoredBitsSource::Constant { value: 0 },
            },
        ]);
    }
    assert_eq!(
        run(vec![moved], Some(0), 0).words[0].unknown_bits,
        1,
        "different bit position is not preservation"
    );
}

#[test]
fn byte_lanes_and_cross_word_accesses_compare_all_accessed_bits() {
    let mut stored = located(constant(0x100), Some(0x1003), None);
    if let RegisterAccess::Observation {
        fact,
        stored: Some(stored),
        ..
    } = &mut stored
    {
        if let FunctionRecord::MemoryAccess { width, .. } = fact.as_mut() {
            *width = 2;
        }
        stored[0].width = 16;
    }
    let mut declared = declaration(Some(0), 0);
    declared.insert(
        0x1004,
        DeclaredWord {
            reset_known: u32::MAX,
            ..DeclaredWord::default()
        },
    );
    let report = check_records(&[stored], &declared).expect("cross-word access");
    assert_eq!(report.words[0].observed_bits, 0xff000000);
    assert_eq!(report.words[0].violation_bits, 0);
    assert_eq!(report.words[1].observed_bits, 0xff);
    assert_eq!(report.words[1].violation_bits, 1);
}

#[test]
fn unknown_reset_mask_bits_are_not_treated_as_zero() {
    let mut declared = declaration(Some(0), 0);
    declared.get_mut(&0x1000).expect("fixture word").reset_known = 1;
    let report = check_records(&[constant(1)], &declared).expect("partial reset");
    assert_eq!(report.words[0].violation_bits, 1);
    assert_eq!(report.words[0].unknown_bits, !1);
}

#[test]
fn missing_reads_and_invalid_bit_partitions_fail_explicitly() {
    let missing = observation(MemoryKind::Load, Some(0x1000), 4);
    assert!(check_records(&[missing], &BTreeMap::new()).is_err());
    assert!(check_records(&[read(vec![], vec![], 1, u32::MAX)], &BTreeMap::new()).is_err());
    assert!(check_records(&[read(vec![], vec![], 0, 0)], &BTreeMap::new()).is_err());
}

#[test]
fn unresolved_addresses_and_indexed_boundaries_are_visible() {
    let unresolved = located(constant(1), None, None);
    let indexed = located(
        unresolved.clone(),
        None,
        Some(IndexedAddress {
            base: 0x1000,
            stride: 4,
            count: None,
        }),
    );
    let report = run(vec![unresolved, indexed], Some(0), 0);
    assert_eq!(report.address_limits.len(), 2);
    assert_eq!(report.address_limits[0].compared_addresses, 0);
    assert_eq!(report.address_limits[1].compared_addresses, 1);
    assert_eq!(report.words[0].violation_bits, 1);
    let bounded = located(
        constant(1),
        None,
        Some(IndexedAddress {
            base: 0x1000,
            stride: 4,
            count: Some(2),
        }),
    );
    let report = run(vec![bounded], Some(0), 0);
    assert!(report.address_limits.is_empty());
    assert_eq!(
        report.words.len(),
        2,
        "small bounded arrays include undeclared words"
    );
    assert_eq!(
        report.words[1].unknown_bits,
        u32::MAX,
        "unmodeled word has no known reset"
    );
}

#[test]
fn coverage_and_assumptions_survive_the_report() {
    let records = [
        RegisterAccess::Gap {
            input: 0,
            object: None,
            reason: "unselected code".into(),
        },
        RegisterAccess::Blocked {
            function: LibraryFunction {
                input: 0,
                name: Some(b"blocked".to_vec()),
                symbol: symbol(b"fixture"),
            },
            error: Error::new(ErrorCode::Unavailable, "unavailable"),
        },
    ];
    let (inputs, regions, declared) = (inputs(), regions(), BTreeMap::new());
    let readonly = [ReadOnlyWindow {
        start: 0x8000,
        length: 0x1000,
    }];
    let assumptions = Assumptions {
        read_only: &readonly,
        ..assumptions()
    };
    let mut check =
        Accumulator::new("esp32s31", &inputs, assumptions, &regions, &declared, &[]).unwrap();
    for record in &records {
        check.observe(record).unwrap();
    }
    let mut summary = summary(&records);
    summary.partial_causes.control_flow = 1;
    summary.partial_causes.expression_limits = 2;
    let report = check.finish(summary);
    assert_eq!(report.coverage_gaps, records);
    assert_eq!(report.corpus.abi, assumptions.abi);
    assert_eq!(report.corpus.call_summaries, assumptions.call_summaries);
    assert_eq!(report.corpus.read_only, readonly);
    let rendered = report.render("fixture");
    assert!(rendered.contains("gaps 1"));
    assert!(rendered.contains("control_flow"));
    assert!(rendered.contains("\"expression_limits\":2"));
    let encoded = serde_json::to_value(&report).unwrap();
    assert_eq!(
        encoded["corpus"]["summary"]["partial_causes"]["expression_limits"],
        2
    );
    assert!(rendered.contains(&format!("sha256 {}", inputs[0].sha256)));
    assert!(rendered.contains("RiscvInteger"));
    assert!(rendered.contains("cargo blobray --format json register-accesses"));
    assert!(rendered.contains("--input fixture=pin:fixture/fixture"));
    assert!(rendered.contains("--range 0x1000:0x100"));
    assert!(rendered.contains("--read-only 0x8000:0x1000"));
}

#[test]
fn violations_are_errors_after_both_reports_are_written() {
    let report = run(vec![constant(1)], Some(0), 0);
    let directory = tempfile::tempdir().expect("test report directory");
    let error = crate::checks::inventory::run::write_report(
        directory.path(),
        "fixture",
        &report,
        &resources(),
    )
    .expect_err("absent-bit use fails the check");
    assert!(
        error
            .to_string()
            .contains("vendor uses reserved or absent bits")
    );
    assert!(
        std::fs::read_to_string(directory.path().join("report.txt"))
            .expect("text report")
            .contains("VIOLATION")
    );
    let json: Value = serde_json::from_str(
        &std::fs::read_to_string(directory.path().join("report.json")).expect("JSON report"),
    )
    .expect("valid report JSON");
    assert_eq!(json["check"]["words"][0]["violation_bits"], 1);
    crate::checks::inventory::run::write_report(
        directory.path(),
        "fixture",
        &run(vec![constant(0)], None, 0),
        &resources(),
    )
    .expect("unknown alone is not a use violation");
}

#[test]
fn used_and_unknown_evidence_from_different_observations_are_both_kept() {
    let branch = read(vec![branch(1, ReadPrecision::May)], vec![], 0, !1);
    let mut unknown = read(vec![], vec![limit(1, ReadLimitKind::WorkLimit)], 0, !1);
    let other = symbol(b"other-member");
    if let RegisterAccess::Observation { function, .. } = &mut unknown {
        function.symbol = other.clone();
    }
    let report = run(vec![branch, unknown], None, 0);
    assert_eq!(report.words[0].violation_bits, 1);
    assert_eq!(report.words[0].unknown_bits, 1);
    assert_eq!(report.words[0].inactive_bits, !1);
    assert_eq!(
        report.words[0].unknown[0].symbol,
        serde_json::to_value(other).unwrap()
    );
}

#[test]
fn inventory_keeps_access_counts_and_cited_priority_without_turning_citations_into_use() {
    let mut unused = located(read(vec![], vec![], 0, u32::MAX), Some(0x1004), None);
    if let RegisterAccess::Observation { function, .. } = &mut unused {
        function.name = Some(b"cited_unused".to_vec());
    }
    let mut report = run(vec![constant(1), unused], Some(0), 0);
    report.prioritize(&BTreeSet::from([("fixture".into(), "cited_unused".into())]));
    assert_eq!(report.words[0].address, 0x1004);
    assert_eq!(report.words[0].loads, 1);
    assert_eq!(report.words[0].used_bits, 0);
    assert_eq!(report.words[1].stores, 1);
    assert!(report.words[1].whole_word);
    assert_eq!(report.words[1].violation_bits, 1);
}

#[test]
fn records_received_one_at_a_time_match_the_same_ordered_list() {
    let mut second = read(
        vec![call(3, Some("wifi_log"), ReadPrecision::May)],
        vec![],
        0,
        !3,
    );
    if let RegisterAccess::Observation { function, .. } = &mut second {
        function.input = 1;
    }
    let records = [
        constant(1),
        second,
        located(constant(2), None, None),
        constant(1),
    ];
    let inputs = [
        inputs()[0].clone(),
        Input {
            role: "second".into(),
            sha256: ArtifactId::of_bytes(b"second"),
        },
    ];
    let (regions, declared) = (regions(), declaration(Some(0), 0));
    let mut stream =
        Accumulator::new("esp32s31", &inputs, assumptions(), &regions, &declared, &[]).unwrap();
    for record in &records {
        stream.observe(record).unwrap();
    }
    let mut list =
        Accumulator::new("esp32s31", &inputs, assumptions(), &regions, &declared, &[]).unwrap();
    records
        .iter()
        .try_for_each(|record| list.observe(record))
        .unwrap();
    let streamed = stream.finish(summary(&records));
    let listed = list.finish(summary(&records));
    assert_eq!(
        serde_json::to_value(&streamed).unwrap(),
        serde_json::to_value(listed).unwrap()
    );
    assert_eq!(streamed.corpus.inputs[0].role, "fixture");
    assert_eq!(streamed.corpus.inputs[1].role, "second");
    assert_eq!(streamed.address_limits[0].function.0, "fixture");
    assert_eq!(streamed.all_reads.loads, 1);
}

#[test]
fn signature_conflicts_fail_only_after_complete_reports_are_written() {
    use oer_analysis_blobray_domain::{SignatureConflict, SignatureOrigin};
    let mut report = run(vec![constant(0)], None, 0);
    assert_eq!(report.violations(), 0);
    report.corpus.summary.signature_conflicts = [10, 11]
        .map(|register| SignatureConflict {
            function: LibraryFunction {
                input: 0,
                symbol: symbol(b"fixture"),
                name: Some(b"synthetic".to_vec()),
            },
            signature: "synthetic".into(),
            declaration: Some("synthetic.h:1".into()),
            origin: SignatureOrigin::Argument { register },
            usage: ReadUse {
                bits: 1,
                precision: ReadPrecision::Exact,
                sink: ReadSink::Branch { site: 16 },
            },
        })
        .to_vec();
    let directory = tempfile::tempdir().unwrap();
    let error = crate::checks::inventory::run::write_report(
        directory.path(),
        "fixture",
        &report,
        &resources(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("2 signature conflicts"));
    let json: Value =
        serde_json::from_slice(&std::fs::read(directory.path().join("report.json")).unwrap())
            .unwrap();
    assert_eq!(
        json["check"]["corpus"]["summary"]["signature_conflicts"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let text = std::fs::read_to_string(directory.path().join("report.txt")).unwrap();
    assert!(text.contains("SIGNATURE CONFLICT function synthetic"));
    assert!(text.contains("entry a0") && text.contains("entry a1"));
}

#[test]
fn rom_header_diagnostics_do_not_fail_the_inventory() {
    use oer_analysis_blobray_domain::RomSignatureDiagnostic;
    let mut report = run(vec![constant(0)], None, 0);
    report.corpus.summary.rom_signature_diagnostic_count = 1;
    report
        .corpus
        .summary
        .rom_signature_diagnostics
        .push(RomSignatureDiagnostic {
            function: LibraryFunction {
                input: 0,
                symbol: symbol(b"fixture"),
                name: Some(b"synthetic_rom".to_vec()),
            },
            signature: "synthetic_rom".into(),
            declaration: "synthetic.h:1".into(),
            register: 12,
            usage: ReadUse {
                bits: 1,
                precision: ReadPrecision::Exact,
                sink: ReadSink::Branch { site: 16 },
            },
        });
    report
        .corpus
        .summary
        .rom_signature_summary_unavailable
        .push(oer_analysis_blobray_domain::SignatureAuditIncomplete {
            function: LibraryFunction {
                input: 0,
                symbol: symbol(b"fixture"),
                name: Some(b"large_synthetic_rom".to_vec()),
            },
            reason: oer_analysis_riscv_model::Error::new(
                oer_analysis_riscv_model::ErrorCode::ResourceLimited,
                "synthetic ROM diagnostic limit",
            ),
        });
    let directory = tempfile::tempdir().unwrap();
    crate::checks::inventory::run::write_report(directory.path(), "fixture", &report, &resources())
        .unwrap();
    let json: Value =
        serde_json::from_slice(&std::fs::read(directory.path().join("report.json")).unwrap())
            .unwrap();
    assert_eq!(
        json["check"]["corpus"]["summary"]["rom_signature_diagnostic_count"],
        1
    );
    assert_eq!(
        json["check"]["corpus"]["summary"]["rom_signature_diagnostics"][0]["register"],
        12
    );
    let text = std::fs::read_to_string(directory.path().join("report.txt")).unwrap();
    assert!(text.contains("ROM/HEADER synthetic_rom"));
    assert!(text.contains("Exact"));
    assert!(text.contains("UNAVAILABLE ROM SUMMARY large_synthetic_rom"));
}

fn resources() -> oer_analysis_riscv_model::AnalysisResources {
    use oer_analysis_riscv_model::{AnalysisProfile, AnalysisResources};
    AnalysisResources::new(
        AnalysisProfile::RegisterInventory,
        AnalysisProfile::RegisterInventory.limits(),
        Default::default(),
        Default::default(),
    )
}

#[test]
fn independent_input_limits_are_summed_and_retained_in_inventory_json() {
    use oer_analysis_riscv_model::{
        AnalysisLimited, FunctionLimits, LimitReason, LocalBudgetExceeded, LocalBudgetScope,
        SummaryOutcome,
    };
    let reason = LimitReason::LocalBudget(LocalBudgetScope::ReadFlow);
    let mut total = AnalysisLimited::default();
    for events in [2, 3] {
        let mut function = FunctionLimits::default();
        for _ in 0..events {
            function.outcome(&SummaryOutcome::<()>::LocalBudgetExceeded(
                LocalBudgetExceeded {
                    scope: LocalBudgetScope::ReadFlow,
                    limit: 100000,
                },
            ));
        }
        for _ in 0..events + 1 {
            function.result(&[reason]);
        }
        let mut input = AnalysisLimited::default();
        input.function(&function);
        total.merge(&input);
    }
    let mut resources = resources();
    resources.limited = Box::new(total);
    let directory = tempfile::tempdir().unwrap();
    crate::checks::inventory::run::write_report(
        directory.path(),
        "fixture",
        &run(vec![], None, 0),
        &resources,
    )
    .unwrap();
    let json: Value =
        serde_json::from_slice(&std::fs::read(directory.path().join("report.json")).unwrap())
            .unwrap();
    assert_eq!(json["schema"], 1);
    let limited = &json["analysis_resources"]["limited"];
    assert_eq!(limited["functions"], 2);
    assert_eq!(limited["results"], 7);
    assert_eq!(limited["events_by_reason"][0]["events"], 5);
    assert_eq!(limited["events_by_reason"][0]["functions"], 2);
    assert_eq!(limited["events_by_reason"][0]["results"], 7);
}
