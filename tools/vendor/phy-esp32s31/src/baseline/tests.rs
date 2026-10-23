use super::*;
use oer_vendor_phy::calibration_capture::space::Register;

#[test]
fn recipe_fingerprint_ignores_external_tests_and_tracks_production_cache_inputs() {
    let root = tempfile::tempdir().unwrap();
    for input in RECIPE_INPUTS {
        let path = root.path().join(input);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"production recipe").unwrap();
    }
    let before = digest(root.path(), RECIPE_INPUTS).unwrap();
    let test = root
        .path()
        .join("tools/vendor/phy/src/workload/calibration/tests.rs");
    std::fs::create_dir_all(test.parent().unwrap()).unwrap();
    std::fs::write(&test, b"first test body").unwrap();
    assert_eq!(digest(root.path(), RECIPE_INPUTS).unwrap(), before);
    std::fs::write(test, b"changed test body").unwrap();
    assert_eq!(digest(root.path(), RECIPE_INPUTS).unwrap(), before);
    for input in [
        "tools/vendor/phy/src/workload.rs",
        "tools/vendor/phy/src/cache.rs",
    ] {
        let path = root.path().join(input);
        std::fs::write(&path, b"changed production recipe").unwrap();
        assert_ne!(digest(root.path(), RECIPE_INPUTS).unwrap(), before);
        std::fs::write(path, b"production recipe").unwrap();
    }
}

#[test]
fn same_run_bounds_override_only_the_reviewed_bytes_and_recorded_only_is_incomplete() {
    let mut image = images();
    image.analog.extend([
        Register {
            name: "analog-i2c.61.09".into(),
            address: 9,
            mask: 0xff,
        },
        Register {
            name: "analog-i2c.61.0a".into(),
            address: 10,
            mask: 0xff,
        },
    ]);
    let mut fixed = vec![vendor(0, 4); 10];
    let mut paired = fixed.clone();
    for (i, boot) in fixed.iter_mut().enumerate() {
        boot.analog.values.extend([
            (9, if i % 2 == 0 { 2 } else { 26 }),
            (10, if i % 2 == 0 { 2 } else { 26 }),
        ]);
    }
    for (i, boot) in paired.iter_mut().enumerate() {
        boot.analog.values.extend([
            (9, if i % 2 == 0 { 43 } else { 58 }),
            (10, if i % 2 == 0 { 43 } else { 58 }),
        ]);
        // A different CONTROL value for an ordinary register must not replace fixed bounds.
        boot.registers.values.insert(4, 99);
    }
    let reference = generate(signature(), vec![input("s31-a", fixed.clone())], &image).unwrap();
    let control =
        compare::SameRunControl::alternating(Lifecycle::Cold, &identity(), paired, 10).unwrap();
    let mut subject = production(&fixed[0]);
    subject.analog.values.extend([(9, 44), (10, 58)]);
    let subjects = vec![subject.clone(); 10];
    let identity = identity();
    let run = |subjects: &[ProductionBoot], control| {
        reference.check(
            &signature(),
            &image,
            "s31-a",
            0,
            Captures {
                identity: &identity,
                production: subjects,
                control,
            },
        )
    };
    let matched = run(&subjects, Some(&control));
    assert_eq!(matched.verdict, Verdict::Match);
    assert_eq!(matched.comparison.unwrap().analog.same_run_control.len(), 2);
    // A frozen vendor series is not implicitly a CONTROL, even when N matches.
    let frozen = compare::compare(
        Lifecycle::Cold,
        &identity,
        &fixed,
        &subjects,
        None,
        compare::ReviewFile::reviewed().unwrap(),
        (&image.registers, &image.analog),
    )
    .unwrap();
    assert_eq!(frozen.verdict, Verdict::Incomplete);
    let paired = compare::compare(
        Lifecycle::Cold,
        &identity,
        &fixed,
        &subjects,
        Some(&control),
        compare::ReviewFile::reviewed().unwrap(),
        (&image.registers, &image.analog),
    )
    .unwrap();
    assert_eq!(paired.verdict, Verdict::Match);
    let missing = run(&subjects, None);
    assert_eq!(missing.verdict, Verdict::Incomplete);
    assert_eq!(
        missing.comparison.unwrap().analog.same_run_control[0].verdict,
        Verdict::Incomplete
    );
    let mut outside = subjects.clone();
    outside[0].analog.values.insert(9, 74);
    assert_eq!(run(&outside, Some(&control)).verdict, Verdict::Diff);
    let mut changed = subjects;
    changed[0].registers.values.insert(4, 99);
    assert_eq!(run(&changed, Some(&control)).verdict, Verdict::Diff);
}

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
                mask: u32::MAX,
                name: "control".into(),
                address: 4,
            },
            Register {
                mask: u32::MAX,
                name: "PHY_BB_RECOVERED_GAPS.CCA_COUNTER_RESULT_1".into(),
                address: 8,
            },
        ],
        analog: vec![Register {
            mask: u32::MAX,
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
fn recorded_and_live_bounds_accept_only_the_same_boot_whole_initial_pair() {
    let fields = compare::fields();
    let field = fields.iter().find(|f| f.name == "shared-rx-dc").unwrap();
    let mut v = vendor(0, 4);
    let object = v.calibration.as_mut().unwrap();
    for gain in 0..field.count / 2 {
        let pair: [u16; 2] = if gain == 0 { [243, 288] } else { [259, 303] };
        for (i, value) in pair.into_iter().enumerate() {
            let start = field.parameter as usize + gain as usize * 4 + i * 2;
            object[start..start + 2].copy_from_slice(&value.to_le_bytes());
        }
    }
    let boots = vec![v; 10];
    let reference = generate(signature(), vec![input("s31-a", boots.clone())], &images()).unwrap();
    for (pair, verdict) in [
        ([243_u16, 288], Verdict::Match),
        ([243, 303], Verdict::Diff),
        ([259, 288], Verdict::Diff),
        ([240, 280], Verdict::Diff),
    ] {
        let mut p = production(&boots[0]);
        let output = p.calibration.as_mut().unwrap();
        for (i, value) in pair.into_iter().enumerate() {
            let start = field.output as usize + 10 * 4 + i * 2;
            output[start..start + 2].copy_from_slice(&value.to_le_bytes());
        }
        let image = images();
        let live = compare::compare(
            Lifecycle::Cold,
            &identity(),
            &boots,
            std::slice::from_ref(&p),
            None,
            compare::ReviewFile::reviewed().unwrap(),
            (&image.registers, &image.analog),
        )
        .unwrap();
        let recorded = reference.check(
            &signature(),
            &image,
            "s31-a",
            0,
            Captures {
                identity: &identity(),
                production: &[p],
                control: None,
            },
        );
        assert_eq!(recorded.verdict, verdict);
        assert_eq!(recorded.comparison.unwrap(), live);
    }
}

#[test]
fn frozen_bounds_preserve_live_comparison_including_signed_fields_and_counter_exclusion() {
    let (reference, a, b) = fixture();
    reference
        .validate(&signature(), &images(), "s31-a", 0)
        .unwrap();
    assert_eq!(reference.entries["control"].shared, Some(9));
    assert_eq!(reference.entries["common-reference"].shared, None);
    let cca = &reference.entries["PHY_BB_RECOVERED_GAPS.CCA_COUNTER_RESULT_1"];
    assert_eq!(
        cca.excluded.as_deref(),
        Some("hardware measurement result over a window, not calibration")
    );
    assert_eq!(cca.shared, None);
    for (board, boots) in [("s31-a", &a), ("s31-b", &b)] {
        for value in [-9, -3, 0, 2, 7, 14, 20, 25, 30, 31] {
            let mut p = production(&vendor(value, 0));
            for counter in [0, 4, 20, 36, 37, u32::MAX] {
                p.registers.values.insert(8, counter);
                let live = compare::compare(
                    Lifecycle::Cold,
                    &identity(),
                    boots,
                    std::slice::from_ref(&p),
                    None,
                    compare::ReviewFile::reviewed().unwrap(),
                    (&images().registers, &images().analog),
                )
                .unwrap();
                let frozen = reference.check(
                    &signature(),
                    &images(),
                    board,
                    0,
                    Captures {
                        identity: &identity(),
                        production: std::slice::from_ref(&p),
                        control: None,
                    },
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
                Captures {
                    identity: &identity(),
                    production: std::slice::from_ref(&p),
                    control: None
                }
            )
            .verdict,
        Verdict::Match
    );
    assert_eq!(
        reference
            .check(
                &signature(),
                &images(),
                "s31-b",
                0,
                Captures {
                    identity: &identity(),
                    production: &[p],
                    control: None
                }
            )
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
            .check(
                &signature(),
                &images(),
                "s31-a",
                0,
                Captures {
                    identity: &identity(),
                    production: &[p],
                    control: None
                }
            )
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
                Captures {
                    identity: &identity(),
                    production: std::slice::from_ref(&p),
                    control: None
                }
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
                Captures {
                    identity: &identity(),
                    production: std::slice::from_ref(&p),
                    control: None
                }
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
                Captures {
                    identity: &identity(),
                    production: std::slice::from_ref(&p),
                    control: None
                }
            )
            .verdict,
        Verdict::Incomplete
    );
    assert_eq!(
        reference
            .check(
                &signature(),
                &images(),
                "s31-a",
                1,
                Captures {
                    identity: &identity(),
                    production: &[p],
                    control: None
                }
            )
            .verdict,
        Verdict::Incomplete
    );
}

#[test]
fn excluded_counter_still_requires_readable_recorded_production_coverage() {
    let (reference, a, _) = fixture();
    for unreadable in [false, true] {
        let mut p = production(&a[0]);
        p.registers.values.remove(&8);
        if unreadable {
            p.registers.unreadable.insert(8);
        }
        assert_eq!(
            reference
                .check(
                    &signature(),
                    &images(),
                    "s31-a",
                    0,
                    Captures {
                        identity: &identity(),
                        production: &[p],
                        control: None
                    }
                )
                .verdict,
            Verdict::Incomplete
        );
    }
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
                Captures {
                    identity: &identity(),
                    production: &[first, second],
                    control: None
                }
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
                Captures {
                    identity: &identity(),
                    production: &[production(&a[0])],
                    control: None
                }
            )
            .verdict,
        Verdict::Incomplete
    );
}

#[test]
fn recorded_bounds_use_masked_samples_before_computing_spread() {
    let mut images = images();
    images.registers[0].mask = 0xf;
    let boots = (0..10)
        .map(|i| {
            let mut boot = vendor(0, 4);
            boot.registers
                .values
                .insert(4, if i % 2 == 0 { 0x8000_0009 } else { 0x4000_0009 });
            boot
        })
        .collect::<Vec<_>>();
    let reference = generate(
        signature(),
        vec![input("s31-a", boots.clone()), input("s31-b", boots.clone())],
        &images,
    )
    .unwrap();
    assert_eq!(reference.entries["control"].mask, Some(vec![0xf]));
    assert_eq!(reference.entries["control"].shared, Some(9));
    let mut primary = production(&boots[0]);
    primary.registers.values.insert(4, 0x2000_0009);
    let result = reference.check(
        &signature(),
        &images,
        "s31-a",
        0,
        Captures {
            identity: &identity(),
            production: &[primary],
            control: None,
        },
    );
    assert_eq!(result.verdict, Verdict::Match);
}
