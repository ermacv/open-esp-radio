use super::*;
use oer_vendor_phy::calibration_capture::space::Register;

pub(super) fn signature() -> Signature {
    Signature {
        libphy_sha256: "a".repeat(64),
    }
}
pub(super) fn provenance() -> Provenance {
    Provenance {
        vendor_application_sha256: "app".into(),
        idf_revision: "pin".into(),
        scenario_sha256: "scenario".into(),
        model_sha256: "model".into(),
        relation_sha256: "relation".into(),
        review_sha256: "reviews".into(),
        recipe_sha256: "recipe".into(),
        retired_mmio: BTreeMap::new(),
        retired_series: vec![],
    }
}
pub(super) fn images() -> Model {
    Model {
        reviews: compare::fixture_reviews(),
        images: RegisterImages {
            registers: vec![
                Register {
                    mask: u32::MAX,
                    name: "control".into(),
                    address: 4,
                },
                Register {
                    mask: u32::MAX,
                    name: "WIFI_BB.CCA_COUNTER_RESULT_1".into(),
                    address: 8,
                },
            ],
            analog: vec![Register {
                mask: u32::MAX,
                name: "analog".into(),
                address: 1,
            }],
        },
    }
}
pub(super) fn identity() -> Images {
    Images {
        vendor_idf_revision: "pin".into(),
        vendor_application_sha256: "app".into(),
        vendor_project: "calibration".into(),
        production_image: "diagnostic-phy".into(),
        ..Images::default()
    }
}
pub(super) fn vendor(value: i16, counter: u32) -> VendorBoot {
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
pub(super) fn production(vendor: &VendorBoot) -> ProductionBoot {
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
pub(super) fn input(label: &str, boots: Vec<VendorBoot>) -> Input {
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
        provenance(),
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
    let reference = generate(
        signature(),
        provenance(),
        vec![input("s31-a", boots.clone())],
        &images(),
    )
    .unwrap();
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
            compare::fixture_reviews(),
            (&image.images.registers, &image.images.analog),
        )
        .unwrap();
        let recorded = reference.check(&signature(), &image, "s31-a", 0, &identity(), &[p]);
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
    let cca = &reference.entries["WIFI_BB.CCA_COUNTER_RESULT_1"];
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
                    compare::fixture_reviews(),
                    (&images().images.registers, &images().images.analog),
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
    changed.libphy_sha256 = "b".repeat(64);
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
                .check(&signature(), &images(), "s31-a", 0, &identity(), &[p])
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
            provenance(),
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

#[test]
fn recorded_bounds_use_masked_samples_before_computing_spread() {
    let mut images = images();
    images.images.registers[0].mask = 0xf;
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
        provenance(),
        vec![input("s31-a", boots.clone()), input("s31-b", boots.clone())],
        &images,
    )
    .unwrap();
    assert_eq!(reference.entries["control"].mask, Some(vec![0xf]));
    assert_eq!(reference.entries["control"].shared, Some(9));
    let mut primary = production(&boots[0]);
    primary.registers.values.insert(4, 0x2000_0009);
    let result = reference.check(&signature(), &images, "s31-a", 0, &identity(), &[primary]);
    assert_eq!(result.verdict, Verdict::Match);
}

#[test]
fn capture_provenance_does_not_invalidate_the_same_libphy() {
    let (mut reference, a, _) = fixture();
    reference.provenance.idf_revision = "historical-idf".into();
    reference.provenance.vendor_application_sha256 = "historical-app".into();
    reference.provenance.scenario_sha256 = "historical-scenario".into();
    reference.provenance.model_sha256 = "historical-model".into();
    reference.provenance.relation_sha256 = "historical-relation".into();
    reference.provenance.review_sha256 = "historical-review".into();
    reference.provenance.recipe_sha256 = "historical-recipe".into();
    for (value, verdict) in [(-3, Verdict::Match), (100, Verdict::Diff)] {
        let checked = reference.check(
            &signature(),
            &images(),
            "s31-a",
            0,
            &identity(),
            &[production(&vendor(value, 4))],
        );
        assert_eq!(checked.verdict, verdict);
    }
    let mut changed = signature();
    changed.libphy_sha256 = "b".repeat(64);
    let checked = reference.check(
        &changed,
        &images(),
        "s31-a",
        0,
        &identity(),
        &[production(&a[0])],
    );
    assert_eq!(checked.verdict, Verdict::Incomplete);
    assert!(checked.reasons[0].contains("no PHY baseline for libphy"));
    assert!(checked.reasons[0].contains(&changed.libphy_sha256));
}

fn pin_checkout() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (path, contents) in [
        (
            "tools/vendor/phy-esp32s31/src/tolerances.toml",
            compare::TOLERANCES,
        ),
        (
            "chips/esp32s31/knowledge/src/phy_relation.rs",
            include_str!("../../../../../chips/esp32s31/knowledge/src/phy_relation.rs"),
        ),
        (
            "chips/esp32s31/knowledge/src/phy_relation/committed.rs",
            include_str!("../../../../../chips/esp32s31/knowledge/src/phy_relation/committed.rs"),
        ),
    ] {
        let path = dir.path().join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }
    std::fs::create_dir_all(dir.path().join("chips/esp32s31/vendor")).unwrap();
    dir
}
fn write_pins(root: &Path, revision: &str, sha256: &str) {
    std::fs::write(root.join("chips/esp32s31/vendor/artifacts.toml"), format!(
        "[[source]]\nid = 'esp-idf'\nrevision = '{revision}'\n[[artifact]]\nid = 'libphy'\nsha256 = '{sha256}'\n[[artifact]]\nid = 'rom'\nsha256 = '{revision}'\n[[artifact]]\nid = 'esp_phy'\nsha256 = '{revision}'\n"
    )).unwrap();
}

#[test]
fn pin_selection_ignores_idf_rom_and_esp_phy_changes_and_names_missing_libphy() {
    let dir = pin_checkout();
    let key = signature();
    let path = key.path(dir.path());
    let (reference, _, _) = fixture();
    reference.write(&path).unwrap();
    for revision in ["capture-idf", "another-idf"] {
        write_pins(dir.path(), revision, &key.libphy_sha256);
        let selected = Signature::current(dir.path()).unwrap();
        assert_eq!(selected, key);
        assert_eq!(selected.path(dir.path()), path);
        assert_eq!(selected.read_reference(dir.path()).unwrap(), reference);
    }
    let changed_hash = "b".repeat(64);
    write_pins(dir.path(), "another-idf", &changed_hash);
    let selected = Signature::current(dir.path()).unwrap();
    assert_ne!(selected.path(dir.path()), path);
    let checked = Check::incomplete(selected.read_reference(dir.path()).unwrap_err());
    assert_eq!(checked.verdict, Verdict::Incomplete);
    assert!(checked.reasons[0].contains("no PHY baseline for libphy"));
    assert!(checked.reasons[0].contains(&changed_hash));
}

#[test]
fn malformed_libphy_pins_cannot_select_a_reference_path() {
    let dir = pin_checkout();
    for hash in ["", "../escape", "short", &"A".repeat(64), &"z".repeat(64)] {
        write_pins(dir.path(), "idf", hash);
        assert!(Signature::current(dir.path()).is_err());
    }
    write_pins(dir.path(), "idf", &"a".repeat(64));
    let path = dir.path().join("chips/esp32s31/vendor/artifacts.toml");
    let mut pins = std::fs::read_to_string(&path).unwrap();
    pins.push_str(&format!(
        "[[artifact]]\nid = 'libphy'\nsha256 = '{}'\n",
        "b".repeat(64)
    ));
    std::fs::write(&path, pins).unwrap();
    assert!(Signature::current(dir.path()).is_err());
}

#[test]
fn committed_reference_is_valid_for_the_current_libphy_and_every_recorded_board() {
    let root = crate::registers::repository();
    let key = Signature::current(&root).unwrap();
    let reference = key.read_reference(&root).unwrap();
    let images = super::model(&root).unwrap();
    let names = crate::registers::review_names(&root).unwrap();
    for lifecycle in [
        Lifecycle::Cold,
        Lifecycle::Restart,
        Lifecycle::Ieee802154,
        Lifecycle::Ieee802154Restart,
    ] {
        for name in images
            .reviews
            .clone()
            .at(lifecycle)
            .unwrap()
            .registers
            .keys()
        {
            assert!(
                names.contains(name),
                "review does not name a current register: {name}"
            );
        }
    }
    let reviews = images.reviews.clone().at(Lifecycle::Cold).unwrap();
    for register in images.images.registers.iter().chain(&images.images.analog) {
        if let Some(review) = reviews.registers.get(&register.name).filter(|r| r.excluded) {
            assert_eq!(
                reference.entries[&register.name].excluded.as_deref(),
                Some(review.reason.as_str()),
                "lost exclusion: {}",
                register.name
            );
        }
    }
    for (label, board) in &reference.boards {
        reference
            .validate(&key, &images, label, board.revision)
            .unwrap();
        Esp32s31::recorded(&root, label, Lifecycle::Cold).unwrap();
    }
}
