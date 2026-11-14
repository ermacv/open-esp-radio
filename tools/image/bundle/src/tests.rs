use super::*;

/// A chip with a flash map, whose bundles hold an OTA selection.
fn profile() -> oer_tools_chip_profile::Profile {
    let repository = tempfile::tempdir().unwrap();
    let root = repository.path();
    for (path, text) in oer_chips_family_facts::tracked::FILES {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    oer_tools_chip_profile::Profile::all(root)
        .unwrap()
        .into_iter()
        .find(|profile| profile.facts.flash.is_some())
        .unwrap()
}

/// The build of a package without its default features.
fn measuring() -> Build {
    Build::Package(BuildChoice {
        features: vec!["oer/esp32s31".to_owned()],
        default_features: false,
        instrumentation: Vec::new(),
    })
}

/// A bundle staged for `output` whose application holds `application`.
fn staged(output: &Path, application: &[u8]) -> ImageBundle {
    let profile = profile();
    let staging = staging_directory(output).unwrap();
    remove_directory(&staging).unwrap();
    std::fs::create_dir_all(&staging).unwrap();
    let mut bundle = ImageBundle::new(
        &staging,
        &profile,
        profile.facts.flash.clone().unwrap(),
        measuring(),
    );
    bundle.otadata = true;
    for (file, bytes) in [
        (bundle.bootloader(), &b"boot"[..]),
        (bundle.partitions(), b"table"),
        (bundle.application(), application),
        (bundle.otadata().unwrap(), b"select"),
    ] {
        std::fs::write(file, bytes).unwrap();
    }
    bundle
}

#[test]
fn a_snapshot_holds_the_published_bytes_in_write_order() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("bundle");
    let bundle = staged(&output, b"app").publish(&output).unwrap();
    let snapshot = ImageBundle::load(&output).unwrap().snapshot().unwrap();
    assert_eq!(bundle.directory, output);
    assert_eq!(
        snapshot
            .segments
            .iter()
            .map(|segment| segment.description)
            .collect::<Vec<_>>(),
        [
            "bootloader",
            "partition table",
            "application",
            "OTA selection"
        ]
    );
    assert_eq!(snapshot.segments[2].data, b"app");
    assert_eq!(
        snapshot.segments[2].sha256,
        oer_tools_os_durable::sha256_bytes(b"app")
    );
}

#[test]
fn a_flash_file_changed_after_publication_is_refused() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("bundle");
    let bundle = staged(&output, b"app").publish(&output).unwrap();
    std::fs::write(bundle.application(), b"other").unwrap();
    assert!(ImageBundle::load(&output).is_err());
    assert!(bundle.snapshot().is_err());
}

#[test]
fn a_publication_replaces_the_bundle_whole() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("bundle");
    staged(&output, b"first").publish(&output).unwrap();
    // A build that fails leaves its staging directory unpublished.
    drop(staged(&output, b"half"));
    let snapshot = ImageBundle::load(&output).unwrap().snapshot().unwrap();
    assert_eq!(snapshot.segments[2].data, b"first");
    staged(&output, b"second").publish(&output).unwrap();
    let snapshot = ImageBundle::load(&output).unwrap().snapshot().unwrap();
    assert_eq!(snapshot.segments[2].data, b"second");
    assert!(!staging_directory(&output).unwrap().exists());
}

#[test]
fn only_its_own_staging_directory_publishes() {
    let directory = tempfile::tempdir().unwrap();
    let bundle = staged(&directory.path().join("bundle"), b"app");
    assert!(bundle.publish(&directory.path().join("elsewhere")).is_err());
}

#[test]
fn a_snapshot_materializes_each_segment_as_a_file() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("bundle");
    let snapshot = staged(&output, b"app")
        .publish(&output)
        .unwrap()
        .snapshot()
        .unwrap();
    let private = tempfile::tempdir().unwrap();
    let files = snapshot.materialize(private.path()).unwrap();
    assert_eq!(files.len(), 4);
    assert_eq!(std::fs::read(&files[2].1).unwrap(), b"app");
    assert_eq!(files[2].0, snapshot.segments[2].offset);
}

#[test]
fn purpose_round_trips_and_the_shared_flash_reader_refuses_analysis() {
    let directory = tempfile::tempdir().unwrap();
    for purpose in [Purpose::Flash, Purpose::Analysis] {
        let output = directory.path().join(format!("{purpose:?}"));
        let mut bundle = staged(&output, b"app");
        bundle.purpose = purpose;
        let published = bundle.publish(&output).unwrap();
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(output.join(MANIFEST)).unwrap()).unwrap();
        assert_eq!(manifest["schema"], 9);
        assert_eq!(manifest["purpose"], format!("{purpose:?}"));
        let loaded = ImageBundle::load(&output).unwrap();
        assert_eq!(loaded, published);
        match purpose {
            Purpose::Flash => assert_eq!(loaded.snapshot().unwrap().segments[2].data, b"app"),
            Purpose::Analysis => {
                let error = loaded.snapshot().unwrap_err().to_string();
                assert!(
                    error.contains("purpose Analysis") && error.contains("cannot be flashed"),
                    "{error}"
                );
                // Copies for archived HIL applications keep the restriction.
                let copy = loaded
                    .with_application(&loaded.application(), &directory.path().join("copy"))
                    .unwrap();
                assert_eq!(
                    ImageBundle::load(&copy.directory).unwrap().purpose,
                    Purpose::Analysis
                );
                assert!(copy.snapshot().is_err());
            }
        }
        // Ordinary loading still checks file integrity for either purpose.
        std::fs::write(loaded.application(), b"changed").unwrap();
        assert!(ImageBundle::load(&output).is_err());
    }
}

#[test]
fn schema_six_and_missing_purpose_cannot_default_to_flash() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("bundle");
    staged(&output, b"app").publish(&output).unwrap();
    let path = output.join(MANIFEST);
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    manifest.as_object_mut().unwrap().remove("purpose");
    manifest["schema"] = 6.into();
    std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let error = ImageBundle::load(&output).unwrap_err().to_string();
    assert!(
        error.contains("has schema 6") && error.contains("reads schema 9"),
        "{error}"
    );
    manifest["schema"] = SCHEMA.into();
    std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let error = ImageBundle::load(&output).unwrap_err().to_string();
    assert!(error.contains("missing field `purpose`"), "{error}");
}

#[test]
fn a_bundle_says_what_it_was_built_from_and_one_that_does_not_is_refused() {
    let directory = tempfile::tempdir().unwrap();
    let output = directory.path().join("bundle");
    staged(&output, b"app").publish(&output).unwrap();
    assert_eq!(ImageBundle::load(&output).unwrap().build, measuring());

    let path = output.join(MANIFEST);
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    // A catalog project's bundle says so, with no choice of features.
    manifest["build"] = serde_json::json!("esp-idf");
    std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert_eq!(ImageBundle::load(&output).unwrap().build, Build::EspIdf);

    // A schema-9 bundle without its build is refused, not read as no
    // features: the field has no default.
    manifest.as_object_mut().unwrap().remove("build");
    std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let error = ImageBundle::load(&output).unwrap_err().to_string();
    assert!(error.contains("missing field `build`"), "{error}");

    // So is a bundle of schema 8, written before bundles said it.
    manifest["schema"] = 8.into();
    std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    let error = ImageBundle::load(&output).unwrap_err().to_string();
    assert!(error.contains("has schema 8"), "{error}");
}
