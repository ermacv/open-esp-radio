use super::*;
use oer_vendor_phy::calibration_capture::space::Register;

fn signature() -> Signature {
    Signature {
        idf_revision: "pin".into(),
        scenario_sha256: "scenario".into(),
        model_sha256: "model".into(),
        relation_sha256: "relation".into(),
        review_sha256: "reviews".into(),
        recipe_sha256: "recipe".into(),
    }
}
fn images() -> RegisterImages {
    RegisterImages {
        registers: vec![
            Register {
                name: "control".into(),
                address: 4,
            },
            Register {
                name: "PHY_BB_RECOVERED_GAPS.WORD_0060_OPAQUE".into(),
                address: 8,
            },
        ],
        analog: vec![Register {
            name: "analog".into(),
            address: 1,
        }],
    }
}
fn identity() -> Images {
    Images {
        vendor_idf_revision: "pin".into(),
        vendor_application_sha256: "app".into(),
        vendor_project: "calibration".into(),
        production_image: "diagnostic-phy".into(),
        ..Images::default()
    }
}
fn vendor(value: i16, counter: u32) -> VendorBoot {
    let fields = compare::fields();
    let len = fields
        .iter()
        .map(|f| f.parameter as usize + usize::from(f.width) * f.count as usize)
        .max()
        .unwrap();
    let mut calibration = vec![0; len];
    let field = fields
        .iter()
        .find(|f| f.name == "common-reference")
        .unwrap();
    let bytes = value.to_le_bytes();
    calibration[field.parameter as usize..field.parameter as usize + 2].copy_from_slice(&bytes);
    VendorBoot {
        calibration: Some(calibration),
        registers: Readings {
            values: [(4, 9), (8, counter)].into(),
            unreadable: Default::default(),
        },
        analog: Readings {
            values: [(1, 7)].into(),
            unreadable: Default::default(),
        },
        ..VendorBoot::default()
    }
}
fn production(vendor: &VendorBoot) -> ProductionBoot {
    let fields = compare::fields();
    let len = fields
        .iter()
        .map(|f| f.output as usize + usize::from(f.width) * f.count as usize)
        .max()
        .unwrap();
    let mut bytes = vec![0; len];
    for field in fields {
        let size = usize::from(field.width) * field.count as usize;
        bytes[field.output as usize..field.output as usize + size].copy_from_slice(
            &vendor.calibration.as_ref().unwrap()
                [field.parameter as usize..field.parameter as usize + size],
        );
    }
    ProductionBoot {
        calibration: Some(bytes),
        registers: vendor.registers.clone(),
        analog: vendor.analog.clone(),
    }
}
fn input(label: &str, boots: Vec<VendorBoot>) -> Input {
    Input {
        label: label.into(),
        revision: 0,
        source: Source {
            run_id: label.into(),
            scenario: SCENARIO.into(),
            repetition: 1,
            repository_commit: "commit".into(),
            repository_dirty: false,
            date: "2026-10-20".into(),
            samples: boots.len(),
            observations_sha256: "digest".into(),
        },
        identity: identity(),
        channel: 1,
        boots,
    }
}
fn fixture() -> (Reference, Vec<VendorBoot>, Vec<VendorBoot>) {
    let a = (0..10)
        .map(|i| {
            vendor(
                if i % 2 == 0 { -3 } else { 2 },
                if i % 2 == 0 { 4 } else { 20 },
            )
        })
        .collect::<Vec<_>>();
    let b = (0..10)
        .map(|i| vendor(if i % 2 == 0 { 20 } else { 25 }, 4))
        .collect::<Vec<_>>();
    let reference = generate(
        signature(),
        vec![input("s31-a", a.clone()), input("s31-b", b.clone())],
        &images(),
    )
    .unwrap();
    (reference, a, b)
}
#[test]
fn frozen_bounds_preserve_live_comparison_including_signed_fields_and_counter_spread() {
    let (reference, a, b) = fixture();
    reference
        .validate(&signature(), &images(), "s31-a", 0)
        .unwrap();
    assert_eq!(reference.entries["control"].shared, Some(9));
    assert_eq!(reference.entries["common-reference"].shared, None);
    let cca = &reference.entries["PHY_BB_RECOVERED_GAPS.WORD_0060_OPAQUE"];
    assert_eq!(cca.excluded, None);
    assert_eq!(cca.shared, None);
    for (board, boots) in [("s31-a", &a), ("s31-b", &b)] {
        for value in [-9, -3, 0, 2, 7, 14, 20, 25, 30, 31] {
            let mut p = production(&vendor(value, 0));
            for counter in [0, 4, 20, 36, 37] {
                p.registers.values.insert(8, counter);
                let live = compare::compare(
                    Lifecycle::Cold,
                    &identity(),
                    boots,
                    std::slice::from_ref(&p),
                    compare::ReviewFile::reviewed().unwrap(),
                    (&images().registers, &images().analog),
                )
                .unwrap();
                let frozen = reference.check(
                    &signature(),
                    &images(),
                    board,
                    0,
                    &identity(),
                    std::slice::from_ref(&p),
                );
                assert_eq!(
                    frozen.comparison.unwrap(),
                    live,
                    "{board} signed={value} counter={counter}"
                );
            }
        }
    }
}
#[test]
fn the_selected_board_never_uses_the_other_boards_range() {
    let (reference, a, _) = fixture();
    let p = production(&a[0]);
    assert_eq!(
        reference
            .check(
                &signature(),
                &images(),
                "s31-a",
                0,
                &identity(),
                std::slice::from_ref(&p)
            )
            .verdict,
        Verdict::Match
    );
    assert_eq!(
        reference
            .check(&signature(), &images(), "s31-b", 0, &identity(), &[p])
            .verdict,
        Verdict::Diff
    );
}
#[test]
fn stale_or_missing_coverage_is_incomplete_even_when_another_value_differs() {
    let (reference, a, _) = fixture();
    let mut p = production(&a[0]);
    p.registers.values.insert(4, 100);
    p.analog.values.clear();
    assert_eq!(
        reference
            .check(&signature(), &images(), "s31-a", 0, &identity(), &[p])
            .verdict,
        Verdict::Incomplete
    );
    let p = production(&a[0]);
    assert_eq!(
        reference
            .check(
                &signature(),
                &images(),
                "other",
                0,
                &identity(),
                std::slice::from_ref(&p)
            )
            .verdict,
        Verdict::Incomplete
    );
    let mut changed = signature();
    changed.recipe_sha256.push('x');
    assert_eq!(
        reference
            .check(
                &changed,
                &images(),
                "s31-a",
                0,
                &identity(),
                std::slice::from_ref(&p)
            )
            .verdict,
        Verdict::Incomplete
    );
    let mut incomplete = reference.clone();
    incomplete.entries.remove("analog");
    assert_eq!(
        incomplete
            .check(
                &signature(),
                &images(),
                "s31-a",
                0,
                &identity(),
                std::slice::from_ref(&p)
            )
            .verdict,
        Verdict::Incomplete
    );
    assert_eq!(
        reference
            .check(&signature(), &images(), "s31-a", 1, &identity(), &[p])
            .verdict,
        Verdict::Incomplete
    );
}
#[test]
fn inconsistent_shared_values_and_vendor_discovery_gaps_cannot_be_published() {
    let (mut reference, mut a, b) = fixture();
    reference
        .entries
        .get_mut("common-reference")
        .unwrap()
        .shared = Some(0);
    assert!(
        reference
            .validate(&signature(), &images(), "s31-a", 0)
            .is_err()
    );
    a[1].registers.values.remove(&4);
    assert!(
        generate(
            signature(),
            vec![input("s31-a", a), input("s31-b", b)],
            &images()
        )
        .is_err()
    );
}
#[test]
fn missing_one_production_boot_is_not_hidden_by_another_boots_registers() {
    let (reference, a, _) = fixture();
    let first = production(&a[0]);
    let mut second = production(&a[1]);
    second.registers.values.remove(&4);
    assert_eq!(
        reference
            .check(
                &signature(),
                &images(),
                "s31-a",
                0,
                &identity(),
                &[first, second]
            )
            .verdict,
        Verdict::Incomplete
    );
}
#[test]
fn reference_serialization_is_deterministic_and_round_trips_in_own_directory() {
    let (reference, _, _) = fixture();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("baseline.json");
    reference.write(&path).unwrap();
    let first = std::fs::read(&path).unwrap();
    Reference::read(&path).unwrap().write(&path).unwrap();
    assert_eq!(first, std::fs::read(path).unwrap());
    assert!(!String::from_utf8(first).unwrap().contains("mac"));
}

#[test]
fn out_of_width_external_bounds_are_incomplete_without_arithmetic_overflow() {
    let (mut reference, a, _) = fixture();
    reference
        .entries
        .get_mut("common-reference")
        .unwrap()
        .boards
        .insert(
            "s31-a".into(),
            Sample::Read {
                ranges: vec![[i64::MIN, i64::MAX]],
                margin: 0,
            },
        );
    assert_eq!(
        reference
            .check(
                &signature(),
                &images(),
                "s31-a",
                0,
                &identity(),
                &[production(&a[0])]
            )
            .verdict,
        Verdict::Incomplete
    );
}
