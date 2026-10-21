use super::*;
use crate::checks::inventory::{DeclaredField, DeclaredRegister, declared_words};
use serde_json::{Value, json};

fn declaration(reset: Option<u64>, fields: u32) -> BTreeMap<u32, DeclaredWord> {
    declared_words(&[DeclaredRegister {
        address: 0x1000,
        width_bits: 32,
        name: "CONTROL".into(),
        template: "CONTROL".into(),
        reset_value: reset,
        reset_mask: None,
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

fn observation(access: &str, address: Value, width: u8) -> Value {
    json!({"kind":"observation", "function":{"input":0,"name":"synthetic", "symbol":{"object":"fixture"}},
        "record":1, "fact":{"kind":"memory-access", "access":access, "width":width,"offset":16},
        "address":address, "mask":null})
}

fn read(uses: Value, unknown: Value, preserved: u32, unused: u32) -> Value {
    let mut observation = observation("load", json!(0x1000), 4);
    observation["reads"] =
        json!({"uses":uses,"unknown":unknown,"preserved":preserved,"unused":unused});
    observation
}

fn constant(value: u32) -> Value {
    let mut observation = observation("store", json!(0x1000), 4);
    observation["stored"] =
        json!([{"low":0,"width":32,"source":{"kind":"constant","value":value}}]);
    observation
}

fn document(records: Vec<Value>) -> Value {
    let count = |kind: &str| {
        records
            .iter()
            .filter(|record| record["kind"] == kind)
            .count()
    };
    let unresolved = records
        .iter()
        .filter(|record| record["kind"] == "observation" && record["address"].is_null())
        .count();
    let indexed = records
        .iter()
        .filter(|record| !record["indexed"].is_null())
        .count();
    json!({"schema":9, "inputs":[{"role":"fixture", "sha256":"synthetic"}],
        "abi":"riscv-integer", "call_returns":true, "read_only":[], "records":records,
        "summary":{"schema":3,"functions":1,"partial_functions":0,"partial_causes":{},
            "blocked_functions":count("blocked"),"gaps":count("gap"),"observations":count("observation"),"unresolved_addresses":unresolved,"indexed_addresses":indexed,
            "ranges":[{"start":0x1000,"length":256}]}})
}

fn check_document(document: Value, declared: &BTreeMap<u32, DeclaredWord>) -> Result<Report> {
    check(
        "esp32s31",
        &document.to_string(),
        &["fixture".into()],
        &[Region {
            name: "synthetic".into(),
            start: 0x1000,
            end: 0x1100,
        }],
        declared,
        &[],
    )
}

fn run(records: Vec<Value>, reset: Option<u64>, fields: u32) -> Report {
    check_document(document(records), &declaration(reset, fields)).expect("valid synthetic corpus")
}

#[test]
fn reset_store_is_inactive_and_nonreset_store_rejects_only_changed_absent_bits() {
    // A nonzero reset ensures that a zero write is not treated as reset.
    let report = run(vec![constant(0xa5)], Some(0xa5), 0);
    assert_eq!(report.violations(), 0);
    assert_eq!(report.words[0].inactive_bits, u32::MAX);
    let report = run(vec![constant(0)], Some(0xa5), 1);
    assert_eq!(report.words[0].used_bits, 0xa5);
    assert_eq!(report.words[0].violation_bits, 0xa4);
    assert_eq!(report.violations(), 1);
}

#[test]
fn rmw_preserves_bits_without_using_the_read_selection() {
    let loaded = read(json!([]), json!([]), !1, 1);
    let mut expression = loaded.clone();
    expression["fact"] = json!({"kind":"expression"});
    expression["mask"] = json!({"kind":"read-selection","bits":4294967294u32});
    let mut stored = constant(1);
    stored["stored"] = json!([
        {"low":0,"width":1,"source":{"kind":"constant","value":1}},
        {"low":1,"width":31,"source":{"kind":"load","address":0x1000,"width":4,"low":1,"same_word":true}}
    ]);
    stored["mask"] = json!({"kind":"write-replacement","bits":1});
    let report = run(vec![loaded, expression, stored], Some(0), 1);
    assert_eq!(report.violations(), 0);
    assert_eq!(report.words[0].used_bits, 1);
    assert_eq!(report.words[0].unknown_bits, 0);
    assert_eq!(report.words[0].inactive_bits, !1);
}

#[test]
fn explicit_mask_is_use_even_when_it_writes_the_reset_value() {
    let mut stored = constant(0);
    stored["mask"] = json!({"kind":"write-replacement","bits":3});
    let report = run(vec![stored], Some(0), 1);
    assert_eq!(report.words[0].violation_bits, 2);
}

#[test]
fn every_read_sink_and_both_precisions_are_use() {
    let sinks = [
        json!({"kind":"branch","site":20}),
        json!({"kind":"dispatch","site":20}),
        json!({"kind":"indirect-target","site":20}),
        json!({"kind":"address","site":20}),
        json!({"kind":"return","register":10}),
        json!({"kind":"store","site":20,"address":0x1004,"same_word":false}),
        json!({"kind":"store","site":20,"address":0x1000,"same_word":true}),
        json!({"kind":"call-argument","site":20,"register":10}),
    ];
    for sink in sinks {
        for precision in ["exact", "may"] {
            let report = run(
                vec![read(
                    json!([{"bits":16,"sink":sink,"precision":precision}]),
                    json!([]),
                    0,
                    !16,
                )],
                None,
                0,
            );
            assert_eq!(report.words[0].violation_bits, 16, "{sink} {precision}");
            assert_eq!(report.words[0].unknown_bits, 0);
        }
    }
}

fn call(
    bits: u32,
    name: Option<&str>,
    precision: ReadPrecision,
) -> oer_analysis_blobray_domain::ReadUse {
    use oer_analysis_blobray_domain::{CallTarget, ReadSink, ReadUse};
    ReadUse {
        bits,
        precision,
        sink: ReadSink::CallArgument {
            site: 20,
            register: 10,
            callee: name.map(|name| CallTarget {
                address: None,
                function: None,
                name: Some(name.into()),
            }),
        },
    }
}

#[test]
fn shared_read_types_roundtrip_and_only_cited_diagnostic_names_are_ignored() {
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
        let typed = ReadUses {
            uses: vec![call(1, name, ReadPrecision::May)],
            unknown: vec![],
            preserved: 0,
            unused: !1,
        };
        let mut record = observation("load", json!(0x1000), 4);
        record["reads"] =
            serde_json::to_value(&typed).expect("serialize producer's shared ReadUses");
        let report = run(vec![record], None, 0);
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
    let regions = [Region {
        name: "synthetic".into(),
        start: 0x1000,
        end: 0x1100,
    }];
    let inputs = ["fixture".into()];
    let declared = declaration(None, 0);
    for (chip, name, expected_use) in [
        ("esp32s31", "phy_log", false),
        ("esp32c5", "phy_log", true),
        ("esp32s31", "wifi_log", false),
        ("esp32c5", "wifi_log", false),
    ] {
        let data = document(vec![read(
            json!([call(1, Some(name), ReadPrecision::Exact)]),
            json!([]),
            0,
            !1,
        )])
        .to_string();
        let report = check(chip, &data, &inputs, &regions, &declared, &[])
            .expect("chip-local synthetic observation");
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
        assert!(check("another-chip", &data, &inputs, &regions, &declared, &[]).is_err());
    }
}

#[test]
fn diagnostic_calls_do_not_hide_behavior_unknown_or_exact_evidence() {
    let diagnostic = call(7, Some("wifi_log"), ReadPrecision::Exact);
    let may = call(3, Some("ets_delay_us"), ReadPrecision::May);
    let exact = json!({"bits":2,"sink":{"kind":"branch","site":24},"precision":"exact"});
    let loaded = read(json!([diagnostic, may, exact]), json!([]), 0, !7);
    let unknown = read(json!([]), json!([{"bits":4,"limit":"work-limit"}]), 0, !4);
    let report = run(vec![loaded.clone(), unknown], Some(0), 0);
    let word = &report.words[0];
    assert_eq!(word.used_bits, 3);
    assert_eq!(word.violation_bits, 3);
    assert_eq!(word.may_only_bits, 1);
    assert_eq!(word.unknown_bits, 4);
    assert_eq!(word.inactive_bits, !7);
    assert!(!word.diagnostics.is_empty());
    assert!(report.render("fixture").contains("MAY ONLY 0x00001000"));
    let report = run(vec![loaded, constant(1)], Some(0), 0);
    assert_eq!(
        report.words[0].may_only_bits, 0,
        "a proven nonreset write is exact use"
    );
}

#[test]
fn call_accounting_unions_sites_and_arguments_before_address_expansion() {
    let mut record = read(
        json!([
            call(3, Some("wifi_log"), ReadPrecision::Exact),
            call(1, Some("wifi_log"), ReadPrecision::Exact),
            call(2, None, ReadPrecision::May)
        ]),
        json!([]),
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
    record["address"] = Value::Null;
    record["indexed"] = json!({"base":0x1000,"stride":4,"count":2});
    let report = run(vec![record.clone()], None, 0);
    assert_eq!(report.words.len(), 2);
    assert_eq!(report.resolved_indexed_reads.call_bits, 2);
    assert_eq!(report.resolved_indexed_reads.load_bits, 32);
    record["indexed"] = Value::Null;
    let report = run(vec![record], None, 0);
    assert_eq!(report.resolved_indexed_reads.loads, 0);
    assert_eq!(report.all_reads.diagnostic_only_bits, 1);
    assert_eq!(report.all_reads.load_bits, 32);
}

#[test]
fn floating_memory_accesses_keep_the_producer_classification_and_unknown_store() {
    let mut loaded = read(
        json!([]),
        json!([{"bits":u32::MAX,"limit":"unsupported-operation"}]),
        0,
        0,
    );
    loaded["fact"]["access"] = json!("float-load");
    let mut stored = observation("float-store", json!(0x1000), 4);
    stored["stored"] = Value::Null;
    let report = run(vec![loaded, stored], None, 0);
    assert_eq!((report.words[0].loads, report.words[0].stores), (1, 1));
    assert_eq!(report.words[0].unknown_bits, u32::MAX);
    assert_eq!(report.violations(), 0);
}

#[test]
fn unused_reads_do_not_establish_use_and_unknown_stays_separate_across_observations() {
    let unused = read(json!([]), json!([]), 0, u32::MAX);
    let unknown = read(json!([]), json!([{"bits":4,"limit":"stack-spill"}]), 0, !4);
    let report = run(vec![unused.clone(), unknown], None, 0);
    assert_eq!(report.violations(), 0);
    assert_eq!(report.words[0].unknown_bits, 4);
    assert_eq!(report.words[0].inactive_bits, !4);
    let rendered = report.render("fixture");
    assert!(rendered.contains("UNKNOWN 0x00001000"));
    assert!(rendered.contains("StackSpill"));
    let branch = read(
        json!([{"bits":4,"sink":{"kind":"branch","site":20},"precision":"exact"}]),
        json!([]),
        0,
        !4,
    );
    let report = run(vec![unused, branch], None, 0);
    assert_eq!(report.words[0].violation_bits, 4);
    assert_eq!(report.words[0].inactive_bits, !4);
}

#[test]
fn every_read_limit_is_retained_even_on_declared_bits() {
    for limit in [
        "partial-function",
        "stack-spill",
        "unsupported-operation",
        "work-limit",
    ] {
        let report = run(
            vec![read(
                json!([]),
                json!([{"bits":u32::MAX,"limit":limit}]),
                0,
                0,
            )],
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
    let stored = observation("store", json!(0x1000), 4);
    let report = run(vec![stored], Some(0), 0);
    assert_eq!(report.words[0].unknown_bits, u32::MAX);
    let mut moved = constant(0);
    moved["stored"] = json!([
        {"low":0,"width":1,"source":{"kind":"load","address":0x1000,"width":4,"low":1,"same_word":true}},
        {"low":1,"width":31,"source":{"kind":"constant","value":0}}
    ]);
    let report = run(vec![moved], Some(0), 0);
    assert_eq!(
        report.words[0].unknown_bits, 1,
        "different bit position is not preservation"
    );
}

#[test]
fn byte_lanes_and_cross_word_accesses_compare_all_accessed_bits() {
    let mut stored = constant(0x100);
    stored["address"] = json!(0x1003);
    stored["fact"]["width"] = json!(2);
    stored["stored"][0]["width"] = json!(16);
    let mut declared = declaration(Some(0), 0);
    declared.insert(
        0x1004,
        DeclaredWord {
            reset_known: u32::MAX,
            ..DeclaredWord::default()
        },
    );
    let report = check_document(document(vec![stored]), &declared).expect("cross-word access");
    assert_eq!(report.words[0].observed_bits, 0xff000000);
    assert_eq!(report.words[0].violation_bits, 0);
    assert_eq!(report.words[1].observed_bits, 0xff);
    assert_eq!(report.words[1].violation_bits, 1);
}

#[test]
fn unknown_reset_mask_bits_are_not_treated_as_zero() {
    let mut declared = declaration(Some(0), 0);
    declared.get_mut(&0x1000).expect("fixture word").reset_known = 1;
    let report = check_document(document(vec![constant(1)]), &declared).expect("partial reset");
    assert_eq!(report.words[0].violation_bits, 1);
    assert_eq!(report.words[0].unknown_bits, !1);
}

#[test]
fn schema_8_missing_reads_and_invalid_bit_partitions_fail_explicitly() {
    let mut old = document(vec![]);
    old["schema"] = json!(8);
    assert!(
        check_document(old, &BTreeMap::new())
            .expect_err("old schema")
            .to_string()
            .contains("schema 9; got 8")
    );
    let missing = document(vec![observation("load", json!(0x1000), 4)]);
    assert!(check_document(missing, &BTreeMap::new()).is_err());
    let overlap = read(json!([]), json!([]), 1, u32::MAX);
    assert!(check_document(document(vec![overlap]), &BTreeMap::new()).is_err());
    let uncovered = read(json!([]), json!([]), 0, 0);
    assert!(check_document(document(vec![uncovered]), &BTreeMap::new()).is_err());
}

#[test]
fn unresolved_addresses_and_indexed_boundaries_are_visible() {
    let mut unresolved = constant(1);
    unresolved["address"] = Value::Null;
    let mut indexed = unresolved.clone();
    indexed["indexed"] = json!({"base":0x1000,"stride":4,"count":null});
    let report = run(vec![unresolved, indexed], Some(0), 0);
    assert_eq!(report.address_limits.len(), 2);
    assert_eq!(report.address_limits[0].compared_addresses, 0);
    assert_eq!(report.address_limits[1].compared_addresses, 1);
    assert_eq!(report.words[0].violation_bits, 1);
    let mut bounded = constant(1);
    bounded["address"] = Value::Null;
    bounded["indexed"] = json!({"base":0x1000,"stride":4,"count":2});
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
fn coverage_and_assumptions_survive_the_report_and_unknown_records_fail() {
    let mut corpus = document(vec![
        json!({"kind":"gap","input":0,"reason":"unselected code"}),
    ]);
    corpus["summary"]["gaps"] = json!(1);
    corpus["summary"]["partial_causes"] = json!({"control_flow":1});
    let report = check_document(corpus, &BTreeMap::new()).expect("gap corpus");
    assert_eq!(report.coverage_gaps.len(), 1);
    let rendered = report.render("fixture");
    assert!(rendered.contains("gaps 1"));
    assert!(rendered.contains("control_flow"));
    assert!(rendered.contains("sha256 synthetic"));
    assert!(rendered.contains("riscv-integer"));
    let invalid = document(vec![json!({"kind":"future"})]);
    assert!(check_document(invalid, &BTreeMap::new()).is_err());
}

#[test]
fn violations_are_errors_after_both_reports_are_written() {
    let report = run(vec![constant(1)], Some(0), 0);
    let directory = tempfile::tempdir().expect("test report directory");
    let error = crate::checks::inventory::run::write_report(directory.path(), "fixture", &report)
        .expect_err("absent-bit use fails the check");
    assert!(error.to_string().contains("vendor uses absent bits"));
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
    let report = run(vec![constant(0)], None, 0);
    crate::checks::inventory::run::write_report(directory.path(), "fixture", &report)
        .expect("unknown alone is not a use violation");
}

#[test]
fn filtered_or_truncated_corpora_cannot_look_complete() {
    let mut corpus = document(vec![constant(0)]);
    corpus["records"] = json!([]);
    let error = check_document(corpus, &declaration(Some(0), 0)).expect_err("missing records");
    assert!(error.to_string().contains("filtered or incomplete corpus"));
}

#[test]
fn used_and_unknown_evidence_from_different_observations_are_both_kept() {
    let branch = read(
        json!([{"bits":1,"sink":{"kind":"branch","site":20},"precision":"may"}]),
        json!([]),
        0,
        !1,
    );
    let mut unknown = read(json!([]), json!([{"bits":1,"limit":"work-limit"}]), 0, !1);
    unknown["function"]["symbol"] = json!({"object":"other-member"});
    let report = run(vec![branch, unknown], None, 0);
    assert_eq!(report.words[0].violation_bits, 1);
    assert_eq!(report.words[0].unknown_bits, 1);
    assert_eq!(report.words[0].inactive_bits, !1);
    assert_eq!(
        report.words[0].unknown[0].symbol,
        json!({"object":"other-member"})
    );
}

#[test]
fn inventory_keeps_access_counts_and_cited_priority_without_turning_citations_into_use() {
    let mut unused = read(json!([]), json!([]), 0, u32::MAX);
    unused["address"] = json!(0x1004);
    unused["function"]["name"] = json!("cited_unused");
    let mut report = run(vec![constant(1), unused], Some(0), 0);
    report.prioritize(&BTreeSet::from([("fixture".into(), "cited_unused".into())]));
    assert_eq!(report.words[0].address, 0x1004);
    assert_eq!(report.words[0].loads, 1);
    assert_eq!(report.words[0].used_bits, 0);
    assert_eq!(report.words[1].stores, 1);
    assert!(report.words[1].whole_word);
    assert_eq!(report.words[1].violation_bits, 1);
}
