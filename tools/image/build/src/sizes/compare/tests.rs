use super::*;
use oer_image_checks::sizes::{Hole, SectionClass, StackBound, TYPE_MIN_BYTES};

fn summary() -> Summary {
    Summary {
        schema: super::super::SCHEMA,
        type_min_bytes: TYPE_MIN_BYTES,
        image_files: BTreeMap::new(),
        embedded_payloads: Vec::new(),
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
                            class: SectionClass::Code,
                            filler: vec![],
                            nobits: false,
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
        class: SectionClass::Writable,
        filler: vec![],
        nobits: false,
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
        inline_chain: vec![],
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
fn type_frame_and_bound_growth_is_reported_never_stopped() {
    for growth in [64, 65, 4096] {
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
            assert_eq!(
                compare(&a, &b).unwrap().0,
                Verdict::Report,
                "{kind} {growth}"
            );
        }
    }
}
#[test]
fn code_growth_is_reported_never_stopped() {
    for growth in [1024, 1025, 2048] {
        let a = summary();
        let mut b = a.clone();
        runtime(&mut b).sections.get_mut(".text").unwrap().bytes += growth;
        assert_eq!(compare(&a, &b).unwrap().0, Verdict::Report, "{growth}");
    }
}
#[test]
fn sram_section_and_allocated_stack_growth_stops() {
    let a = summary();
    let mut b = a.clone();
    let m = runtime(&mut b);
    m.sections.get_mut(".data").unwrap().bytes += 4;
    m.sections.get_mut(".bss").unwrap().bytes -= 100;
    assert_eq!(compare(&a, &b).unwrap().0, Verdict::Stop);
    for (bytes, expected) in [(8191, Verdict::Report), (8193, Verdict::Stop)] {
        let mut b = a.clone();
        runtime(&mut b)
            .allocated_stacks
            .insert("cpu0".into(), bytes);
        assert_eq!(compare(&a, &b).unwrap().0, expected);
    }
}
#[test]
fn a_new_hole_is_reported_and_an_equal_count_cannot_hide_a_different_one() {
    let a = summary();
    let mut b = a.clone();
    runtime(&mut b).bounds.get_mut("cpu0").unwrap().holes = BTreeSet::from([hole("new")]);
    let (verdict, _, shown) = compare(&a, &b).unwrap();
    assert_eq!(verdict, Verdict::Report);
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
    let (verdict, _, shown) = compare(&a, &b).unwrap();
    assert_eq!(verdict, Verdict::Report);
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
    assert_eq!(compare(&a, &b).unwrap().0, Verdict::Report);
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

#[test]
fn stack_allocation_flags_are_reported_by_the_allocation_rule() {
    let mut a = summary();
    a.provenance.flags.insert(
        "runtime".into(),
        BTreeMap::from([
            ("RUSTFLAGS".into(), "same compiler flags".into()),
            ("OPEN_RADIO_IRQ_STACK_BYTES".into(), "8192".into()),
        ]),
    );
    let mut b = a.clone();
    b.provenance
        .flags
        .get_mut("runtime")
        .unwrap()
        .insert("OPEN_RADIO_IRQ_STACK_BYTES".into(), "16384".into());
    runtime(&mut b)
        .allocated_stacks
        .insert("cpu0".into(), 16384);
    assert_eq!(compare(&a, &b).unwrap().0, Verdict::Stop);
}

fn psram_contents(bytes: u64, executable: bool) -> Section {
    Section {
        memory: "psram".into(),
        class: if executable {
            SectionClass::Code
        } else {
            SectionClass::ReadOnly
        },
        filler: vec![],
        nobits: false,
        executable,
        writable: false,
        address: 0x5000_0000,
        bytes,
    }
}

#[test]
fn linker_padding_growth_cancels_no_code_or_ram_budget() {
    let mut a = summary();
    runtime(&mut a).sections.insert(
        "arbitrary-tail".into(),
        Section {
            class: SectionClass::Filler,
            ..psram_contents(1024, false)
        },
    );
    let mut b = a.clone();
    runtime(&mut b).sections.get_mut(".text").unwrap().bytes -= 466;
    runtime(&mut b)
        .sections
        .get_mut("arbitrary-tail")
        .unwrap()
        .bytes += 688;
    a.image_files.insert("runtime.bin".into(), 65536);
    b.image_files.insert("runtime.bin".into(), 65536);
    let (verdict, code, shown) = compare(&a, &b).unwrap();
    assert_eq!(verdict, Verdict::Report);
    assert_eq!(code, -466);
    assert!(shown.contains("| runtime.bin | 65536 | 65536 | +0 |"));
}

#[test]
fn psram_code_and_read_only_data_share_the_net_code_budget() {
    for executable in [true, false] {
        for (growth, expected) in [
            (652, Verdict::Report),
            (1024, Verdict::Report),
            (1025, Verdict::Report),
        ] {
            let mut a = summary();
            runtime(&mut a)
                .sections
                .insert("contents".into(), psram_contents(2048, executable));
            let mut b = a.clone();
            runtime(&mut b).sections.get_mut("contents").unwrap().bytes += growth;
            let (verdict, code, shown) = compare(&a, &b).unwrap();
            assert_eq!(verdict, expected);
            assert_eq!(code, i128::from(growth));
            assert!(!shown.contains("section `contents` grows"), "{shown}");
        }
    }
    let a = summary();
    let mut b = a.clone();
    runtime(&mut b).sections.get_mut(".text").unwrap().bytes -= 700;
    runtime(&mut b)
        .sections
        .insert(".rodata".into(), psram_contents(800, false));
    assert_eq!(compare(&a, &b).unwrap().1, 100);
}

#[test]
fn executable_and_read_only_sram_are_also_ram() {
    for executable in [true, false] {
        let mut a = summary();
        let mut s = psram_contents(64, executable);
        s.memory = "sram".into();
        runtime(&mut a)
            .sections
            .insert(".critical.rodata".into(), s);
        let mut b = a.clone();
        runtime(&mut b)
            .sections
            .get_mut(".critical.rodata")
            .unwrap()
            .bytes += 4;
        let (verdict, _, shown) = compare(&a, &b).unwrap();
        assert_eq!(verdict, Verdict::Stop);
        assert!(shown.contains("section `.critical.rodata` grows +4 B"));
    }
}

#[test]
fn moving_code_to_sram_has_zero_net_code_but_still_stops_for_ram() {
    let mut a = summary();
    runtime(&mut a).sections.get_mut(".text").unwrap().memory = "psram".into();
    let mut b = a.clone();
    runtime(&mut b).sections.get_mut(".text").unwrap().bytes -= 512;
    let mut hot = psram_contents(512, true);
    hot.memory = "sram".into();
    runtime(&mut b).sections.insert(".hot.text".into(), hot);
    let (verdict, code, shown) = compare(&a, &b).unwrap();
    assert_eq!((verdict, code), (Verdict::Stop, 0));
    assert!(shown.contains("section `.hot.text` grows +512 B"));
    assert!(!shown.contains("STOP: code growth"));
}

#[test]
fn source_hole_identity_ignores_offsets_but_retains_chain_and_multiplicity() {
    let a = summary();
    let mut b = a.clone();
    let moved = Hole {
        offset: 900,
        ..hole("old")
    };
    runtime(&mut b).bounds.get_mut("cpu0").unwrap().holes = BTreeSet::from([moved.clone()]);
    assert!(!compare(&a, &b).unwrap().2.contains("new hole:"));
    let mut changed = b.clone();
    let h = Hole {
        inline_chain: vec!["other::inline".into()],
        ..moved.clone()
    };
    runtime(&mut changed).bounds.get_mut("cpu0").unwrap().holes = BTreeSet::from([h]);
    assert_eq!(compare(&a, &changed).unwrap().0, Verdict::Report);
    runtime(&mut b)
        .bounds
        .get_mut("cpu0")
        .unwrap()
        .holes
        .insert(Hole {
            offset: 1000,
            ..moved
        });
    let (verdict, _, shown) = compare(&a, &b).unwrap();
    assert_eq!(verdict, Verdict::Report);
    assert_eq!(shown.matches("new hole:").count(), 1);
}

#[test]
fn embedded_runtime_is_counted_once_and_own_bootstrap_code_is_counted() {
    let mut a = summary();
    a.elfs.insert(
        "bootstrap".into(),
        Measurements {
            sections: BTreeMap::from([
                (".text".into(), psram_contents(100, true)),
                (".rodata".into(), psram_contents(2048, false)),
            ]),
            ..Measurements::default()
        },
    );
    a.embedded_payloads.push(super::super::EmbeddedPayload {
        source_file: "runtime.bin".into(),
        elf: "bootstrap".into(),
        section: ".rodata".into(),
        address: 0x5000_0000,
        bytes: 2048,
        sha256: "fixture".into(),
    });
    let mut b = a.clone();
    runtime(&mut b).sections.get_mut(".text").unwrap().bytes += 700;
    b.elfs
        .get_mut("bootstrap")
        .unwrap()
        .sections
        .get_mut(".rodata")
        .unwrap()
        .bytes += 700;
    b.embedded_payloads[0].bytes += 700;
    assert_eq!(
        (compare(&a, &b).unwrap().0, compare(&a, &b).unwrap().1),
        (Verdict::Report, 700)
    );
    let mut b = a.clone();
    b.elfs
        .get_mut("bootstrap")
        .unwrap()
        .sections
        .get_mut(".text")
        .unwrap()
        .bytes += 1025;
    assert_eq!(
        (compare(&a, &b).unwrap().0, compare(&a, &b).unwrap().1),
        (Verdict::Report, 1025)
    );
}

#[test]
fn old_or_mixed_schemas_are_errors() {
    let a = summary();
    let mut b = a.clone();
    b.schema = 1;
    assert!(compare(&a, &b).is_err());
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join(super::super::FILE), r#"{"schema":1}"#).unwrap();
    assert!(
        Summary::read(directory.path())
            .unwrap_err()
            .to_string()
            .contains("incompatible sizes schema")
    );
}

#[test]
fn embedded_copy_is_identified_by_bytes_not_section_name() {
    use object::{Architecture, BinaryFormat, Endianness, SectionKind, write::Object};
    let directory = tempfile::tempdir().unwrap();
    let payload = b"a unique runtime payload with a checksum";
    std::fs::write(directory.path().join("runtime.bin"), payload).unwrap();
    std::fs::write(directory.path().join("application.bin"), b"application").unwrap();
    let mut writer = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let section = writer.add_section(
        vec![],
        b".arbitrary.placement".to_vec(),
        SectionKind::ReadOnlyData,
    );
    writer.append_section_data(section, &[0; 17], 1);
    writer.append_section_data(section, payload, 1);
    std::fs::write(
        directory.path().join("bootstrap.elf"),
        writer.write().unwrap(),
    )
    .unwrap();
    let mut s = summary();
    s.contents(directory.path()).unwrap();
    assert_eq!(s.embedded_payloads.len(), 1);
    let embedded = &s.embedded_payloads[0];
    assert_eq!(embedded.section, ".arbitrary.placement");
    assert_eq!(
        (embedded.address, embedded.bytes),
        (17, payload.len() as u64)
    );
    assert_eq!(embedded.sha256, oer_os_durable::sha256_bytes(payload));
    assert_eq!(s.image_files["application.bin"], 11);
    std::fs::write(
        directory.path().join("runtime.bin"),
        b"not the embedded runtime",
    )
    .unwrap();
    assert!(
        s.contents(directory.path())
            .unwrap_err()
            .to_string()
            .contains("no byte-identical copy")
    );
}

#[test]
fn refresh_cannot_write_inside_its_input_tree() {
    let directory = tempfile::tempdir().unwrap();
    let nested = directory.path().join("output");
    assert!(super::super::refresh(directory.path(), &nested, directory.path()).is_err());
    assert!(!nested.exists());
}

#[test]
fn embedded_runtime_alignment_is_not_own_bootstrap_code() {
    let mut a = summary();
    let mut rodata = psram_contents(4096, false);
    rodata.filler.push(oer_image_checks::sizes::Filler {
        address: rodata.address + 2048,
        bytes: 2048,
        contract: "payload alignment".into(),
    });
    a.elfs.insert(
        "bootstrap".into(),
        Measurements {
            sections: BTreeMap::from([(".rodata".into(), rodata)]),
            ..Measurements::default()
        },
    );
    a.embedded_payloads.push(super::super::EmbeddedPayload {
        source_file: "runtime.bin".into(),
        elf: "bootstrap".into(),
        section: ".rodata".into(),
        address: 0x5000_0000,
        bytes: 2048,
        sha256: "fixture".into(),
    });
    let mut b = a.clone();
    runtime(&mut b).sections.get_mut(".text").unwrap().bytes += 700;
    b.embedded_payloads[0].bytes += 700;
    let filler = &mut b
        .elfs
        .get_mut("bootstrap")
        .unwrap()
        .sections
        .get_mut(".rodata")
        .unwrap()
        .filler[0];
    filler.address += 700;
    filler.bytes -= 700;
    assert_eq!(
        (compare(&a, &b).unwrap().0, compare(&a, &b).unwrap().1),
        (Verdict::Report, 700)
    );
}

#[test]
fn refresh_accepts_a_new_output_tree_without_creating_it_on_input_error() {
    let directory = tempfile::tempdir().unwrap();
    let input = directory.path().join("input");
    std::fs::create_dir(&input).unwrap();
    std::fs::write(input.join(super::super::FILE), r#"{"schema":99}"#).unwrap();
    let output = directory.path().join("new/nested/output");
    let error = super::super::refresh(&input, &output, directory.path()).unwrap_err();
    assert!(
        error.to_string().contains("unsupported source schema"),
        "{error}"
    );
    assert!(!output.exists());
}

#[test]
fn an_interrupt_stack_grows_only_as_an_allocated_stack() {
    let mut a = summary();
    runtime(&mut a)
        .allocated_stacks
        .insert("irq hart 0".into(), 4096);
    let mut b = a.clone();
    runtime(&mut b)
        .allocated_stacks
        .insert("irq hart 0".into(), 4112);
    assert_eq!(compare(&a, &b).unwrap().0, Verdict::Stop);
    runtime(&mut b)
        .allocated_stacks
        .insert("irq hart 0".into(), 4080);
    assert_eq!(compare(&a, &b).unwrap().0, Verdict::Report);
}
