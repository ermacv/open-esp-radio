use std::path::PathBuf;

use oer_hil_run_bundle::run::test_support::manifest;
use oer_hil_run_bundle_format::run::Comparison;
use oer_hil_run_bundle_format::run::Failure;
use oer_hil_run_bundle_format::run::FailureKind;
use oer_hil_run_bundle_format::run::Measurement;
use oer_hil_run_bundle_format::run::MeasurementUnit;
use oer_hil_run_bundle_format::run::Outcome;
use oer_hil_run_bundle_format::run::RUN_SCHEMA;
use oer_hil_run_bundle_format::run::RepetitionResult;
use oer_hil_run_bundle_format::run::ScenarioResult;
use oer_hil_run_bundle_format::run::SuiteCounts;
use oer_hil_run_bundle_format::run::SuiteResult;
use oer_hil_schema::image::ImageClass;

use super::{html, junit, views};

fn failed_suite() -> SuiteResult {
    let failure = Failure::new(FailureKind::Scenario, "bad <frame> & timeout");
    let scenarios = vec![ScenarioResult::from_repetitions(
        String::from("udp-rx"),
        ImageClass::SystemWatchdog,
        1,
        vec![RepetitionResult {
            schema: RUN_SCHEMA,
            repetition: 1,
            outcome: Outcome::Failed,
            started_unix_millis: 1,
            duration_millis: 250,
            artifact_directory: PathBuf::from("scenarios/udp-rx/repetition-001"),
            attachments: Vec::new(),
            measurements: vec![
                Measurement::observed("udp.rx.loss", 2, MeasurementUnit::Count)
                    .evaluated(Comparison::AtMost, 0),
            ],
            failure: Some(failure),
        }],
    )];
    SuiteResult {
        schema: RUN_SCHEMA,
        run_id: String::from("run<&>"),
        target: String::from("chip-a"),
        outcome: Outcome::Failed,
        started_unix_millis: 1,
        finished_unix_millis: 251,
        duration_millis: 250,
        counts: SuiteCounts::from_results(&scenarios),
        scenarios,
    }
}

#[test]
fn junit_preserves_failure_and_escapes_xml() {
    let xml = junit(&failed_suite(), &manifest());
    roxmltree::Document::parse(&xml).expect("valid JUnit XML");
    assert!(xml.contains("tests=\"1\" failures=\"1\""));
    assert!(xml.contains("bad &lt;frame&gt; &amp; timeout"));
    assert!(xml.contains("run_id\" value=\"run&lt;&amp;&gt;"));
    assert!(xml.contains("repetition-001"));
    assert!(xml.contains("measurement.udp.rx.loss=2 count"));
}

#[test]
fn html_is_derived_from_the_same_suite_record() {
    let html = html(&failed_suite(), &manifest());
    assert!(html.contains("udp-rx"));
    assert!(html.contains("bad &lt;frame&gt; &amp; timeout"));
    assert!(html.contains("0/1 scenarios passed"));
    assert!(html.contains("udp.rx.loss"));
    assert!(html.contains("&lt;= 0 count"));
}

#[test]
fn the_sealed_views_are_both_projections() {
    let (suite, manifest) = (failed_suite(), manifest());
    let views = views(&suite, &manifest);
    assert_eq!(views.junit, junit(&suite, &manifest));
    assert_eq!(views.html, html(&suite, &manifest));
}
