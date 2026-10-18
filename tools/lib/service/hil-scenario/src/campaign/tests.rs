use super::*;
use crate::test_family::{self, TestFamily};

pub(super) fn catalog() -> Catalog<TestFamily> {
    test_family::catalog()
}

#[test]
fn selection_is_exactly_the_requested_scenarios() {
    let catalog = catalog();
    let throughput = catalog.get("throughput").unwrap();
    let silence = catalog.get("silence").unwrap();
    let plan = Plan::create(&catalog, &[silence, throughput], "owned-xarxa").unwrap();
    assert_eq!(plan.requested, ["silence", "throughput"]);
    assert!(
        plan.scenarios
            .iter()
            .all(|entry| entry.reasons == [Reason::Requested])
    );
    assert!(plan.requirements.station_network);
    let roundtrip: Plan = serde_json::from_slice(&serde_json::to_vec(&plan).unwrap()).unwrap();
    assert_eq!(plan, roundtrip);
    assert!(Plan::create(&catalog, &[silence, silence], "owned-xarxa").is_err());
}

#[test]
fn named_check_selection_is_scoped_to_the_scenarios_that_publish_it() {
    let catalog = catalog();
    let selected = catalog
        .all()
        .iter()
        .filter(|scenario| scenario.header.tags.iter().any(|tag| tag == "he20"))
        .collect::<Vec<_>>();
    let plan = Plan::create_for_checks(
        &catalog,
        &selected,
        "owned-xarxa",
        &["udp.rx.maximum-silence".into()],
    )
    .unwrap();
    assert_eq!(plan.scenarios.len(), 1);
    assert_eq!(plan.requested, ["silence"]);
    assert!(
        matches!(&plan.scenarios[0].reasons[1], Reason::ProvidesChecks { checks } if checks == &["udp.rx.maximum-silence"])
    );
    for checks in [
        vec!["not-a-check".into()],
        vec!["udp.rx.maximum-silence".into(), "not-a-check".into()],
    ] {
        assert!(Plan::create_for_checks(&catalog, &selected, "owned-xarxa", &checks).is_err());
    }
}

#[test]
fn procedure_identity_ignores_annotations_but_tracks_execution() {
    let catalog = catalog();
    let mut scenario = catalog.get("boot-smoke").unwrap().clone();
    let original = procedure(&scenario).unwrap();
    scenario.header.description.push_str(" Clarified wording.");
    scenario.header.tags.push("documentation".into());
    assert_eq!(original, procedure(&scenario).unwrap());
    scenario.header.repetitions += 1;
    assert_ne!(original, procedure(&scenario).unwrap());
}
