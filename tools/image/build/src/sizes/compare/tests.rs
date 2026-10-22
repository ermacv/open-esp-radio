use super::*;
use oer_image_checks::sizes::{Hole, StackBound, TYPE_MIN_BYTES};

fn summary() -> Summary {
    Summary {
        schema: super::super::SCHEMA,
        type_min_bytes: TYPE_MIN_BYTES,
        provenance: Provenance {
            commit: "base".into(),
            dirty: false,
            rustc: "rustc fixture".into(),
            chip: "test".into(),
            package: "image".into(),
            binary: "image".into(),
            target: "rv32".into(),
            profile: "firmware".into(),
            profile_settings: "opt-level=3".into(),
            features: vec![],
            default_features: false,
            purpose: crate::bundle::Purpose::Analysis,
            layout_seed: None,
            configuration_sha256: BTreeMap::new(),
            stack_policy: "stack.toml".into(),
            stack_policy_sources: BTreeMap::new(),
            flags: BTreeMap::new(),
        },
        elfs: BTreeMap::from([(
            "runtime".into(),
            Measurements {
                sections: BTreeMap::from([
                    (
                        ".text".into(),
                        Section {
                            memory: "flash".into(),
                            executable: true,
                            writable: false,
                            address: 0x40000000,
                            bytes: 4096,
                        },
                    ),
                    (".data".into(), ram(100)),
                    (".bss".into(), ram(200)),
                ]),
                allocated_stacks: BTreeMap::from([("cpu0".into(), 8192)]),
                types: BTreeMap::from([("fixture::Large<u32>".into(), 256)]),
                frames: BTreeMap::from([("fixture::poll<u32>".into(), vec![256])]),
                bounds: BTreeMap::from([(
                    "cpu0".into(),
                    StackBound {
                        root: "main".into(),
                        bytes: 2048,
                        complete: false,
                        holes: BTreeSet::from([hole("old")]),
                        assumptions: BTreeSet::new(),
                        maximum_frame: 256,
                    },
                )]),
                ..Measurements::default()
            },
        )]),
    }
}
fn ram(bytes: u64) -> Section {
    Section {
        memory: "sram".into(),
        executable: false,
        writable: true,
        address: 0x20000000,
        bytes,
    }
}
fn hole(name: &str) -> Hole {
    Hole {
        function: name.into(),
        offset: 8,
        reason: "register call".into(),
    }
}
fn runtime(summary: &mut Summary) -> &mut Measurements {
    summary.elfs.get_mut("runtime").unwrap()
}

#[test]
fn identical_sizes_ignore_commit_dirt_and_resource_use() {
    let a = summary();
    let mut b = a.clone();
    b.provenance.commit = "head".into();
    b.provenance.dirty = true;
    runtime(&mut b).resources.work_units = 999;
    assert_eq!(compare(&a, &b).unwrap().0, Verdict::Same);
}
#[test]
fn owner_boundaries_are_inclusive() {
    for (growth, expected) in [(64, Verdict::Report), (65, Verdict::Stop)] {
        for kind in ["type", "frame", "bound", "root maximum frame"] {
            let a = summary();
            let mut b = a.clone();
            let m = runtime(&mut b);
            match kind {
                "type" => *m.types.values_mut().next().unwrap() += growth,
                "frame" => m.frames.values_mut().next().unwrap()[0] += growth,
                "bound" => m.bounds.get_mut("cpu0").unwrap().bytes += growth,
                _ => m.bounds.get_mut("cpu0").unwrap().maximum_frame += growth,
            }
            assert_eq!(compare(&a, &b).unwrap().0, expected, "{kind} {growth}");
        }
    }
    for (growth, expected) in [(1024, Verdict::Report), (1025, Verdict::Stop)] {
        let a = summary();
        let mut b = a.clone();
        runtime(&mut b).sections.get_mut(".text").unwrap().bytes += growth;
        assert_eq!(compare(&a, &b).unwrap().0, expected);
    }
}
#[test]
fn ram_growth_is_checked_per_section_and_stack_changes_in_both_directions() {
    let a = summary();
    let mut b = a.clone();
    let m = runtime(&mut b);
    m.sections.get_mut(".data").unwrap().bytes += 4;
    m.sections.get_mut(".bss").unwrap().bytes -= 100;
    assert_eq!(compare(&a, &b).unwrap().0, Verdict::Stop);
    for bytes in [8191, 8193] {
        let mut b = a.clone();
        runtime(&mut b)
            .allocated_stacks
            .insert("cpu0".into(), bytes);
        assert_eq!(compare(&a, &b).unwrap().0, Verdict::Stop);
    }
}
#[test]
fn equal_bound_and_hole_count_cannot_hide_a_different_hole() {
    let a = summary();
    let mut b = a.clone();
    runtime(&mut b).bounds.get_mut("cpu0").unwrap().holes = BTreeSet::from([hole("new")]);
    let (verdict, _, shown) = compare(&a, &b).unwrap();
    assert_eq!(verdict, Verdict::Stop);
    assert!(shown.contains("new hole: new+0x8"));
}
#[test]
fn incompatible_origins_are_errors() {
    let a = summary();
    for field in [
        "image", "target", "profile", "config", "compiler", "features", "flags",
    ] {
        let mut b = a.clone();
        match field {
            "image" => b.provenance.binary = "other".into(),
            "target" => b.provenance.target = "other".into(),
            "profile" => b.provenance.profile = "other".into(),
            "config" => {
                b.provenance
                    .configuration_sha256
                    .insert("country".into(), Some("different".into()));
            }
            "compiler" => b.provenance.rustc = "other".into(),
            "features" => b.provenance.features.push("different".into()),
            _ => {
                b.provenance.flags.insert(
                    "runtime".into(),
                    BTreeMap::from([("RUSTFLAGS".into(), "different".into())]),
                );
            }
        }
        assert!(compare(&a, &b).is_err(), "{field}");
    }
}
#[test]
fn new_and_removed_functions_and_types_are_named_even_at_zero_size() {
    let a = summary();
    let mut b = a.clone();
    let m = runtime(&mut b);
    m.types.clear();
    m.types.insert("fixture::New<u8>".into(), 256);
    m.frames.clear();
    m.frames.insert("fixture::new<u8>".into(), vec![0]);
    let (_, _, shown) = compare(&a, &b).unwrap();
    for line in [
        "new type (runtime): `fixture::New<u8>`",
        "removed type (runtime): `fixture::Large<u32>`",
        "new function (runtime): `fixture::new<u8>`",
        "removed function (runtime): `fixture::poll<u32>`",
    ] {
        assert!(shown.contains(line), "{shown}");
    }
}
#[test]
fn code_growth_is_accumulated_across_elfs_and_sections() {
    let a = summary();
    let mut b = a.clone();
    let mut second = a.elfs["runtime"].clone();
    second.sections.get_mut(".text").unwrap().bytes += 600;
    let mut a = a;
    a.elfs.insert("bootstrap".into(), a.elfs["runtime"].clone());
    b.elfs.insert("bootstrap".into(), second);
    runtime(&mut b).sections.get_mut(".text").unwrap().bytes += 600;
    assert_eq!(compare(&a, &b).unwrap().0, Verdict::Stop);
}
#[test]
fn directory_comparison_is_read_only_and_reports_every_image() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("base");
    let head = temp.path().join("head");
    for root in [&base, &head] {
        let image = root.join("nested/image");
        std::fs::create_dir_all(&image).unwrap();
        summary().publish(&image).unwrap();
    }
    let result = compare_directories(&base, &head).unwrap();
    assert_eq!(result.verdict, Verdict::Same);
    assert!(result.output.contains("overall: same"));
    assert!(compare_directories(&base, &temp.path().join("missing")).is_err());
}
