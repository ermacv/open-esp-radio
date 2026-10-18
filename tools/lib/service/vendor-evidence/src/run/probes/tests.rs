use super::*;
use std::collections::BTreeSet;

/// A temporary repository of two chips: `chip-a` with two probe images,
/// `chip-b` with one, for different instruction sets.
fn repository() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("Cargo.toml"), "[workspace]\n").unwrap();
    for (chip, target, probes) in [
        (
            "chip-a",
            "riscv32imafc-unknown-none-elf",
            &[
                ("rust-artifact", "radio"),
                ("rust-artifact:bluetooth", "bluetooth"),
            ][..],
        ),
        (
            "chip-b",
            "riscv32imac-unknown-none-elf",
            &[("rust-artifact", "radio")][..],
        ),
    ] {
        let profile = directory.path().join("platform").join(chip);
        std::fs::create_dir_all(&profile).unwrap();
        let mut text = format!(
            "schema = 1\nid = \"{chip}\"\nfamily = \"f\"\nrust-target = \"{target}\"\n\
             boot = \"staged\"\nespflash-chip = \"{chip}\"\nrevisions = []\n\
             [properties]\nwifi-bands = []\nbluetooth = []\nieee802154 = false\ncores = 1\n"
        );
        for (role, name) in probes {
            text.push_str(&format!(
                "[[probe]]\nrole = \"{role}\"\npackage = \"oer-{chip}-probe-{name}-elf\"\n"
            ));
        }
        std::fs::write(profile.join("chip.toml"), text).unwrap();
    }
    directory
}

#[test]
fn build_job_override_is_optional_and_rejects_nonpositive_or_nondecimal_values() {
    assert_eq!(parse_jobs(None).unwrap(), None);
    assert_eq!(
        parse_jobs(Some(OsStr::new("12"))).unwrap().unwrap().get(),
        12
    );
    for invalid in [
        "",
        "0",
        "01",
        "-1",
        "+1",
        "1.0",
        " 2",
        "2\n",
        "999999999999999999999999999999999",
    ] {
        assert!(
            parse_jobs(Some(OsStr::new(invalid))).is_err(),
            "accepted {invalid:?}"
        );
    }
}

#[test]
fn every_declared_role_builds_a_distinct_locked_embedded_package_in_the_shared_cache() {
    let directory = repository();
    let context = Checkout::new(directory.path()).unwrap();
    let mut packages = BTreeSet::new();
    let mut outputs = BTreeSet::new();
    let cache = std::path::Path::new("/host/build/cargo");
    let declared = all(&context.root).unwrap();
    assert_eq!(declared.len(), 3);
    for probe in &declared {
        assert!(packages.insert(probe.probe.package.clone()));
        assert!(outputs.insert(output(&context, probe)));
        let target = oer_tool_data_chip_profile::rust_target(&context.root, &probe.chip).unwrap();
        let command = command(&context, probe, &target, cache, None);
        let arguments: Vec<_> = command.get_args().collect();
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["--package", probe.probe.package.as_str()])
        );
        assert!(
            arguments
                .windows(2)
                .any(|pair| pair == ["--target", target.as_str()])
        );
        assert!(arguments.contains(&OsStr::new("--locked")));
        assert!(arguments.contains(&OsStr::new("--release")));
        assert!(!arguments.contains(&OsStr::new("--jobs")));
        assert!(
            command
                .get_envs()
                .any(|(key, value)| key == "CARGO_TARGET_DIR" && value == Some(cache.as_os_str()))
        );
    }
    let command = command(&context, &declared[0], "t", cache, NonZeroUsize::new(3));
    assert!(
        command
            .get_args()
            .collect::<Vec<_>>()
            .windows(2)
            .any(|pair| pair == ["--jobs", "3"])
    );
}

#[test]
fn role_listing_does_not_require_cargo_or_build_outputs() {
    let directory = repository();
    let context = Checkout::new(directory.path()).unwrap();
    run(&context, "chip-a", true).unwrap();
    assert!(!directory.path().join("target").exists());
    assert!(run(&context, "unsupported", true).is_err());
}

#[test]
fn builder_rejects_invalid_jobs_before_execution_and_stops_at_failed_artifact() {
    let directory = repository();
    let context = Checkout::new(directory.path()).unwrap();
    let mut calls = 0;
    assert!(
        build(
            &context,
            "chip-a",
            std::path::Path::new("/cache"),
            Some(OsStr::new("0")),
            |_| {
                calls += 1;
                Ok(())
            }
        )
        .is_err()
    );
    assert_eq!(calls, 0);
    assert!(
        build(
            &context,
            "chip-a",
            std::path::Path::new("/cache"),
            None,
            |_| {
                calls += 1;
                Err("compiled artifact failed".into())
            }
        )
        .is_err()
    );
    assert_eq!(calls, 1);
}

#[test]
fn every_requested_artifact_receives_the_explicit_job_override() {
    let directory = repository();
    let context = Checkout::new(directory.path()).unwrap();
    let mut calls = 0;
    build(
        &context,
        "chip-a",
        std::path::Path::new("/cache"),
        Some(OsStr::new("4")),
        |command| {
            let arguments: Vec<_> = command.get_args().collect();
            assert!(arguments.windows(2).any(|pair| pair == ["--jobs", "4"]));
            assert!(arguments.contains(&OsStr::new("--locked")));
            calls += 1;
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(calls, probes(&context.root, "chip-a").unwrap().len());
}

#[test]
fn each_chip_builds_its_own_workspace_for_its_instruction_set() {
    let directory = repository();
    let context = Checkout::new(directory.path()).unwrap();
    let mut seen = Vec::new();
    build(
        &context,
        "chip-b",
        std::path::Path::new("/cache"),
        None,
        |command| {
            let arguments: Vec<_> = command.get_args().collect();
            assert!(
                arguments
                    .windows(2)
                    .any(|pair| pair == ["--target", "riscv32imac-unknown-none-elf"])
            );
            let manifest = arguments
                .windows(2)
                .find(|pair| pair[0] == "--manifest-path")
                .map(|pair| pair[1].to_owned())
                .unwrap();
            assert!(
                std::path::Path::new(&manifest).ends_with("verification/chip-b/probes/Cargo.toml")
            );
            seen.push(());
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(seen.len(), 1);
    assert!(
        elf(&context, "oer-chip-b-probe-radio-elf")
            .unwrap()
            .ends_with("target/verification/chip-b/oer-chip-b-probe-radio-elf")
    );
}
