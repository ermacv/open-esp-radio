use super::*;
use crate::committed::field;

fn review() -> Tolerance {
    Tolerance {
        reason: String::from("reviewed"),
        ..Tolerance::default()
    }
}

#[test]
fn production_within_the_vendor_spread_matches() {
    let field = field("value", 0, 1, 1, 2);
    // The second element's vendor boots span 2, so every element's
    // range widens by 2.
    let vendor = [vec![10, 20], vec![10, 22]];
    let within = [vec![0, 12, 24]];
    let summary = compare_field(&field, Some(&review()), &vendor, &within).unwrap();
    assert_eq!((summary.verdict, summary.margin), (Verdict::Match, 2));
    let beyond = [vec![0, 13, 20]];
    let summary = compare_field(&field, Some(&review()), &vendor, &beyond).unwrap();
    assert_eq!(summary.verdict, Verdict::Diff);
    let unreviewed = compare_field(&field, None, &vendor, &within).unwrap();
    assert_eq!(unreviewed.verdict, Verdict::Incomplete);
}

#[test]
fn masks_compare_only_modeled_bits() {
    let field = field("status", 0, 0, 1, 1);
    let masked = Tolerance {
        mask: Some(vec![0x80]),
        ..review()
    };
    let summary = compare_field(&field, Some(&masked), &[vec![0xa8]], &[vec![0x80]]).unwrap();
    assert_eq!(summary.verdict, Verdict::Match);
    let whole = compare_field(&field, Some(&review()), &[vec![0xa8]], &[vec![0x80]]).unwrap();
    assert_eq!(whole.verdict, Verdict::Diff);
}

#[test]
fn signed_elements_extend_their_sign() {
    let field = field("temperature", 0, 0, 2, 1);
    assert_eq!(element(&[0xfe, 0xff], 0, &field, 0, true).unwrap(), -2);
    assert_eq!(element(&[0xfe, 0xff], 0, &field, 0, false).unwrap(), 0xfffe);
}

#[test]
fn uncovered_ranges_are_the_gaps_between_fields() {
    let fields = [field("a", 2, 0, 2, 1), field("b", 5, 0, 1, 1)];
    assert_eq!(uncovered(7, &fields), [[0, 2], [4, 5], [6, 7]]);
}

#[test]
fn registers_compare_within_their_own_vendor_spread() {
    let image = |name: &str, address| oer_vendor_phy::calibration_capture::space::Register {
        mask: u32::MAX,
        name: name.into(),
        address,
    };
    let registers = [
        image("A.STABLE", 0x10),
        image("A.VARYING", 0x14),
        image("A.CLOCKED_OFF", 0x18),
        image("A.EXCLUDED", 0x1c),
        image("A.MISSED", 0x20),
    ];
    let vendor_boot = |varying| Readings {
        values: BTreeMap::from([(0x10, 7), (0x14, varying), (0x1c, 1), (0x20, 1)]),
        unreadable: [0x18].into(),
    };
    let production = Readings {
        values: BTreeMap::from([(0x10, 7), (0x14, 13), (0x1c, 9)]),
        unreadable: Default::default(),
    };
    let reviews = BTreeMap::from([(
        "A.EXCLUDED".to_owned(),
        Tolerance {
            excluded: true,
            reason: "environment".into(),
            ..Tolerance::default()
        },
    )]);
    let summary = compare_registers(
        &registers,
        &[vendor_boot(10), vendor_boot(11)],
        [&production],
        &reviews,
        None,
    );
    // 13 lies beyond 11 + (11 - 10).
    assert_eq!((summary.compared, summary.matched), (2, 1));
    assert_eq!(summary.differing[0].name, "A.VARYING");
    assert_eq!(summary.unreadable, ["A.CLOCKED_OFF"]);
    assert_eq!(summary.excluded[0].name, "A.EXCLUDED");
    assert_eq!(summary.not_read, ["A.MISSED"]);
    assert_eq!(summary.verdict, Verdict::Diff);
}

#[test]
fn unassigned_lanes_neither_differ_nor_widen_vendor_spread() {
    let image = [Register {
        name: "SPARSE".into(),
        address: 4,
        mask: 0x101,
    }];
    let readings = |value| Readings {
        values: [(4, value)].into(),
        unreadable: Default::default(),
    };
    let vendor = [readings(1), readings(0x8000_0001)];
    let harmless = readings(0x4000_0001);
    let changed = readings(0x101);
    assert_eq!(
        compare_registers(&image, &vendor, [&harmless], &BTreeMap::new(), None).verdict,
        Verdict::Match
    );
    let diff = compare_registers(&image, &vendor, [&changed], &BTreeMap::new(), None);
    assert_eq!(diff.verdict, Verdict::Diff);
    assert_eq!(diff.differing[0].vendor, [1, 1]);
    assert_eq!(diff.differing[0].production, [0x101, 0x101]);
}

#[test]
fn zero_mask_is_explicitly_excluded_and_still_requires_a_read() {
    let image = [Register {
        name: "EMPTY".into(),
        address: 4,
        mask: 0,
    }];
    let readings = Readings {
        values: [(4, 7)].into(),
        unreadable: Default::default(),
    };
    let summary = compare_registers(&image, [&readings], [&readings], &BTreeMap::new(), None);
    assert_eq!(summary.compared, 0);
    assert_eq!(summary.excluded[0].reason, NO_MODELED_BITS);
    assert_eq!(summary.verdict, Verdict::Incomplete);
    let missing = Readings::default();
    assert_eq!(
        compare_registers(&image, [&readings], [&missing], &BTreeMap::new(), None).not_read,
        ["EMPTY"]
    );
}

fn excluded_register_summary(production: &[Readings]) -> RegisterSummary {
    let image = [
        Register {
            mask: u32::MAX,
            name: "STABLE".into(),
            address: 0x10,
        },
        Register {
            mask: u32::MAX,
            name: "EXCLUDED".into(),
            address: 0x14,
        },
    ];
    let vendor = Readings {
        values: [(0x10, 7), (0x14, 4)].into(),
        unreadable: Default::default(),
    };
    let reviews = BTreeMap::from([(
        "EXCLUDED".into(),
        Tolerance {
            excluded: true,
            reason: "measurement result".into(),
            ..Tolerance::default()
        },
    )]);
    compare_registers(&image, [&vendor], production, &reviews, None)
}

#[test]
fn excluded_register_without_a_production_read_is_incomplete() {
    let missing = Readings {
        values: [(0x10, 7)].into(),
        unreadable: Default::default(),
    };
    let read = Readings {
        values: [(0x10, 7), (0x14, 4)].into(),
        unreadable: Default::default(),
    };
    for boots in [vec![missing.clone()], vec![read, missing]] {
        let summary = excluded_register_summary(&boots);
        assert_eq!(summary.verdict, Verdict::Incomplete);
        assert_eq!(summary.not_read, ["EXCLUDED"]);
        assert!(summary.excluded.is_empty());
    }
}

#[test]
fn excluded_unreadable_production_register_is_incomplete() {
    let unreadable = Readings {
        values: [(0x10, 7)].into(),
        unreadable: [0x14].into(),
    };
    let read = Readings {
        values: [(0x10, 7), (0x14, 4)].into(),
        unreadable: Default::default(),
    };
    for boots in [vec![unreadable.clone()], vec![read, unreadable]] {
        let summary = excluded_register_summary(&boots);
        assert_eq!(summary.verdict, Verdict::Incomplete);
        assert_eq!(summary.not_read, ["EXCLUDED"]);
        assert!(summary.excluded.is_empty());
    }
}

#[test]
fn excluded_readable_register_values_do_not_affect_match() {
    let boots = [0, 4, u32::MAX].map(|value| Readings {
        values: [(0x10, 7), (0x14, value)].into(),
        unreadable: Default::default(),
    });
    let summary = excluded_register_summary(&boots);
    assert_eq!(summary.verdict, Verdict::Match);
    assert_eq!((summary.compared, summary.matched), (1, 1));
    assert_eq!(summary.excluded[0].name, "EXCLUDED");
    assert!(summary.not_read.is_empty());
    assert!(summary.differing.is_empty());
}

#[test]
fn an_ieee802154_point_compares_register_state_only() {
    let image = [Register {
        mask: u32::MAX,
        name: "A.X".into(),
        address: 0x10,
    }];
    let readings = |value| Readings {
        values: BTreeMap::from([(0x10, value)]),
        unreadable: Default::default(),
    };
    let vendor = [
        VendorBoot {
            registers: readings(4),
            analog: readings(1),
            ..VendorBoot::default()
        },
        VendorBoot {
            registers: readings(6),
            analog: readings(1),
            ..VendorBoot::default()
        },
    ];
    let production = [ProductionBoot {
        calibration: None,
        registers: readings(8),
        analog: readings(1),
    }];
    let images = Images {
        started_unix_seconds: 1_790_467_200,
        production_image: "boot-smoke".into(),
        ..Images::default()
    };
    let summary = compare(
        Lifecycle::Ieee802154,
        &images,
        &vendor,
        &production,
        None,
        ReviewFile::reviewed().unwrap(),
        (&image, &image),
    )
    .unwrap();
    assert_eq!(summary.verdict, Verdict::Match);
    assert!(summary.fields.is_empty() && summary.excluded.is_empty());
    assert_eq!(summary.date, "2026-09-27");
    assert_eq!(
        (summary.registers.compared, summary.registers.matched),
        (1, 1)
    );
    // The typed result round-trips through its run-bundle form.
    let json = serde_json::to_value(&summary).unwrap();
    assert_eq!(serde_json::from_value::<Summary>(json).unwrap(), summary);
    // A cold point needs every boot's calibration.
    assert!(
        compare(
            Lifecycle::Cold,
            &images,
            &vendor,
            &production,
            None,
            ReviewFile::reviewed().unwrap(),
            (&image, &image)
        )
        .is_err()
    );
}

#[test]
fn sar2_mismatch_fails_restart_while_the_wifi_cold_point_remains_excluded() {
    let registers = [
        Register {
            mask: u32::MAX,
            name: "analog-i2c.69.04".into(),
            address: 0x10,
        },
        Register {
            mask: u32::MAX,
            name: "A.CONTROL".into(),
            address: 0x14,
        },
    ];
    let readings = |value| Readings {
        values: BTreeMap::from([(0x10, value), (0x14, 7)]),
        unreadable: Default::default(),
    };
    let vendor = [readings(0x60)];
    for lifecycle in [Lifecycle::Restart, Lifecycle::Ieee802154Restart] {
        let reviews = ReviewFile::reviewed().unwrap().at(lifecycle).unwrap();
        let mismatch = readings(0x65);
        let summary = compare_registers(&registers, &vendor, [&mismatch], &reviews.registers, None);
        assert_eq!(summary.verdict, Verdict::Diff);
        assert_eq!((summary.compared, summary.matched), (2, 1));
        assert_eq!(summary.differing[0].name, "analog-i2c.69.04");
        assert!(summary.excluded.is_empty());

        let matching = readings(0x60);
        let summary = compare_registers(&registers, &vendor, [&matching], &reviews.registers, None);
        assert_eq!(summary.verdict, Verdict::Match);
        assert_eq!((summary.compared, summary.matched), (2, 2));
    }

    let reviews = ReviewFile::reviewed().unwrap().at(Lifecycle::Cold).unwrap();
    let cold_registration = readings(0x65);
    let summary = compare_registers(
        &registers,
        &vendor,
        [&cold_registration],
        &reviews.registers,
        None,
    );
    assert_eq!(summary.verdict, Verdict::Match);
    assert_eq!((summary.compared, summary.matched), (1, 1));
    assert_eq!(summary.excluded[0].name, "analog-i2c.69.04");
}

#[test]
fn dates_are_utc_days() {
    assert_eq!(utc_date(0), "1970-01-01");
    assert_eq!(utc_date(1_790_467_200), "2026-09-27");
    assert_eq!(utc_date(951_782_400), "2000-02-29");
}

#[test]
fn the_progress_counter_is_not_compared() {
    assert!(fields().iter().all(|f| f.name != TRACKING_PROGRESS_FIELD));
    assert_eq!(
        fields().len(),
        CALIBRATION.len() + PARENT.len() + committed(0).len() - 1
    );
}

#[test]
fn a_review_scoped_to_a_lifecycle_point_applies_only_there() {
    let text = "schema = 4\n\
         [registers.A]\nexcluded = true\nlifecycle = \"cold\"\nreason = \"cold only\"\n\
         [registers.B]\nexcluded = true\nreason = \"always\"\n\
         [[registers.C]]\nexcluded = true\nlifecycle = \"cold\"\nreason = \"cold\"\n\
         [[registers.C]]\nexcluded = true\nlifecycle = \"restart\"\nreason = \"woken\"\n";
    let reviews = || toml::from_str::<ReviewFile>(text).unwrap();
    let restart = reviews().at(Lifecycle::Restart).unwrap();
    assert!(!restart.registers.contains_key("A"));
    assert!(restart.registers.contains_key("B"));
    assert_eq!(restart.registers["C"].reason, "woken");
    assert_eq!(
        reviews().at(Lifecycle::Cold).unwrap().registers["C"].reason,
        "cold"
    );
    let ambiguous =
        "schema = 4\n[[registers.D]]\nreason = \"a\"\n[[registers.D]]\nreason = \"b\"\n";
    assert!(
        toml::from_str::<ReviewFile>(ambiguous)
            .unwrap()
            .at(Lifecycle::Cold)
            .is_err()
    );
}
