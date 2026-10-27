use super::{
    tests::{identity, images, input, production, provenance, signature, vendor},
    *,
};
use oer_vendor_phy::calibration_capture::space::Register;

fn captured() -> Vec<VendorBoot> {
    (0..10)
        .map(|i| {
            let mut boot = vendor(0, 100 + i);
            boot.registers.values.insert(4, 0x8000_0009 + (i << 8));
            boot
        })
        .collect()
}
fn current_images() -> RegisterImages {
    let mut images = images();
    images.registers = vec![Register {
        name: "renamed-control".into(),
        address: 4,
        mask: 0xf,
    }];
    images
}
fn historical() -> Reference {
    generate(
        signature(),
        provenance(),
        vec![input("s31-a", captured())],
        &images(),
    )
    .unwrap()
}
fn rebuilt() -> Reference {
    historical()
        .regenerate(
            signature(),
            vec![input("s31-a", captured())],
            &current_images(),
        )
        .unwrap()
}

#[test]
fn archive_rebuild_uses_current_names_and_masks_and_keeps_original_retired_boots() {
    let original = historical();
    let reference = rebuilt();
    assert_eq!(reference.boards, original.boards);
    assert_eq!(
        reference.entries["renamed-control"].boards["s31-a"],
        sample(vec![[9, 9]])
    );
    assert!(!reference.entries.contains_key("control"));
    assert_eq!(reference.provenance.retired_mmio.len(), 1);
    let retired_name = "PHY_BB_RECOVERED_GAPS.CCA_COUNTER_RESULT_1";
    assert_eq!(
        reference.provenance.retired_mmio[retired_name],
        original.entries[retired_name]
    );
    let series = &reference.provenance.retired_series[0];
    assert_eq!(series.source, reference.boards["s31-a"].sources[0]);
    assert_eq!(
        series.boots,
        captured()
            .iter()
            .map(|boot| Readings {
                values: [(8, boot.registers.values[&8])].into(),
                unreadable: BTreeSet::new(),
            })
            .collect::<Vec<_>>()
    );
    reference
        .validate(&signature(), &current_images(), "s31-a", 0)
        .unwrap();
    let repeated = reference
        .clone()
        .regenerate(
            signature(),
            vec![input("s31-a", captured())],
            &current_images(),
        )
        .unwrap();
    assert_eq!(repeated, reference);
}

#[test]
fn active_capture_coverage_remains_exact_with_optional_archived_retired_words() {
    let reference = rebuilt();
    let archived = production(&captured()[0]);
    let mut current = archived.clone();
    current.registers.values.remove(&8);
    for boot in [archived.clone(), current.clone()] {
        assert_eq!(
            reference
                .check(
                    &signature(),
                    &current_images(),
                    "s31-a",
                    0,
                    &identity(),
                    &[boot]
                )
                .verdict,
            Verdict::Match
        );
    }
    let mut missing = current.clone();
    missing.registers.values.remove(&4);
    let mut unknown = current.clone();
    unknown.registers.values.insert(12, 0);
    let mut inconsistent = archived;
    inconsistent.registers.unreadable.insert(8);
    for boot in [missing, unknown, inconsistent] {
        assert_eq!(
            reference
                .check(
                    &signature(),
                    &current_images(),
                    "s31-a",
                    0,
                    &identity(),
                    &[boot]
                )
                .verdict,
            Verdict::Incomplete
        );
    }
    let mut different = current;
    different.registers.values.insert(4, 10);
    assert_eq!(
        reference
            .check(
                &signature(),
                &current_images(),
                "s31-a",
                0,
                &identity(),
                &[different]
            )
            .verdict,
        Verdict::Diff
    );
}

#[test]
fn regeneration_refuses_unknown_missing_or_changed_source_observations() {
    for address in [4, 8] {
        let mut boots = captured();
        boots[0].registers.values.remove(&address);
        assert!(
            historical()
                .regenerate(signature(), vec![input("s31-a", boots)], &current_images())
                .is_err()
        );
    }
    let mut boots = captured();
    boots[0].registers.values.insert(12, 1);
    assert!(
        historical()
            .regenerate(signature(), vec![input("s31-a", boots)], &current_images())
            .is_err()
    );
    let mut source = input("s31-a", captured());
    source.source.observations_sha256 = "another capture".into();
    assert!(
        historical()
            .regenerate(signature(), vec![source], &current_images())
            .is_err()
    );
    let mut boots = captured();
    boots[0].registers.values.insert(8, 999);
    assert!(
        rebuilt()
            .regenerate(signature(), vec![input("s31-a", boots)], &current_images())
            .is_err()
    );
}

#[test]
fn retired_provenance_requires_every_source_and_every_original_boot() {
    let reference = rebuilt();
    let mut missing = reference.clone();
    missing.provenance.retired_series[0].boots[0].values.clear();
    let mut missing_series = reference.clone();
    missing_series.provenance.retired_series.clear();
    let mut overlap = reference.clone();
    overlap
        .provenance
        .retired_mmio
        .values_mut()
        .next()
        .unwrap()
        .address = Some(4);
    for invalid in [missing, missing_series, overlap] {
        assert!(
            invalid
                .validate(&signature(), &current_images(), "s31-a", 0)
                .is_err()
        );
    }
}

#[test]
fn retired_unreadability_is_preserved_for_every_boot() {
    let mut boots = captured();
    for boot in &mut boots {
        boot.registers.values.remove(&8);
        boot.registers.unreadable.insert(8);
    }
    let original = generate(
        signature(),
        provenance(),
        vec![input("s31-a", boots.clone())],
        &images(),
    )
    .unwrap();
    let reference = original
        .regenerate(signature(), vec![input("s31-a", boots)], &current_images())
        .unwrap();
    for boot in &reference.provenance.retired_series[0].boots {
        assert!(boot.values.is_empty());
        assert_eq!(boot.unreadable, [8].into());
    }
}
