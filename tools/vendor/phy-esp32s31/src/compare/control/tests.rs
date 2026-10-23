use super::*;
use crate::compare::{ReviewFile, compare_registers};

fn image() -> Vec<Register> {
    vec![Register {
        name: "analog-i2c.61.09".into(),
        address: 1,
        mask: 0xff,
    }]
}

fn readings(value: u32) -> Readings {
    Readings {
        values: [(1, value)].into(),
        unreadable: Default::default(),
    }
}

#[test]
fn control_is_required_and_uses_the_existing_spread_with_all_masked_bits() {
    let image = image();
    let reviews = ReviewFile::reviewed().unwrap().at(Lifecycle::Cold).unwrap();
    let fixed = [readings(2), readings(26)];
    let vendor = [readings(43), readings(58)];
    let control = vendor.iter().collect::<Vec<_>>();
    for (value, expected) in [
        (28, Verdict::Match),
        (58, Verdict::Match),
        (73, Verdict::Match),
        (74, Verdict::Diff),
        (27, Verdict::Diff),
        (0x12c, Verdict::Match),
        (0xac, Verdict::Diff),
    ] {
        let subject = readings(value);
        let result = compare_registers(
            &image,
            &fixed,
            [&subject],
            &reviews.registers,
            Some(&control),
        );
        assert_eq!(result.verdict, expected, "value={value}");
        assert_eq!(result.same_run_control[0].vendor, Some([43, 58]));
        assert_eq!(result.same_run_control[0].margin, Some(15));
        assert!(result.excluded.is_empty());
    }
    // Even a subject inside the old fixed bounds cannot substitute for CONTROL.
    let subject = readings(14);
    let result = compare_registers(&image, &fixed, [&subject], &reviews.registers, None);
    assert_eq!(result.verdict, Verdict::Incomplete);
    assert_eq!(result.same_run_control[0].verdict, Verdict::Incomplete);
    assert_eq!(result.same_run_control[0].vendor, None);
    assert!(result.excluded.is_empty());
}

#[test]
fn every_primary_control_and_production_boot_must_read_the_byte() {
    let image = image();
    let reviews = ReviewFile::reviewed().unwrap().at(Lifecycle::Cold).unwrap();
    let good = readings(44);
    let missing = Readings::default();
    let unreadable = Readings {
        unreadable: [1].into(),
        ..Default::default()
    };
    for bad in [&missing, &unreadable] {
        for (control, production) in [
            (vec![&good, bad], vec![&good]),
            (vec![&good], vec![&good, bad]),
        ] {
            let result = compare_registers(
                &image,
                [&good],
                production,
                &reviews.registers,
                Some(&control),
            );
            assert_eq!(result.verdict, Verdict::Incomplete);
        }
    }
}

#[test]
fn smoke_prefixes_and_unequal_series_are_not_complete_control() {
    for (vendors, production, valid) in
        [(1, 1, false), (9, 9, false), (10, 9, false), (10, 10, true)]
    {
        assert_eq!(
            SameRunControl::alternating(
                Lifecycle::Cold,
                &Default::default(),
                vec![VendorBoot::default(); vendors],
                production
            )
            .is_some(),
            valid
        );
    }
}

#[test]
fn control_cannot_be_attached_to_a_different_capture_identity_or_prefix() {
    let identity = Images::default();
    let control = SameRunControl::alternating(
        Lifecycle::Cold,
        &identity,
        vec![VendorBoot::default(); 10],
        10,
    )
    .unwrap();
    assert!(control.belongs_to(&identity, 10));
    assert!(!control.belongs_to(&identity, 9));
    let changed = Images {
        production_application_sha256: "another image".into(),
        ..identity
    };
    assert!(!control.belongs_to(&changed, 10));
}

#[test]
fn reference_rules_cannot_exclude_values_change_masks_or_apply_to_fields() {
    for text in [
        "schema=4\n[registers.byte]\nreference='same-run-control'\nexcluded=true\nreason='review'",
        "schema=4\n[registers.byte]\nreference='same-run-control'\nmask=[63]\nreason='review'",
        "schema=4\n[fields.channel]\nreference='same-run-control'\nreason='review'",
    ] {
        assert!(
            toml::from_str::<ReviewFile>(text)
                .unwrap()
                .at(Lifecycle::Cold)
                .is_err()
        );
    }
    assert!(
        toml::from_str::<ReviewFile>(
            "schema=4\n[registers.byte]\nreference='unknown'\nreason='review'"
        )
        .is_err()
    );
}
