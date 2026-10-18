use super::*;

/// A chip with a flash map, whose bundles hold an OTA selection.
fn profile() -> oer_chip_profile::Profile {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    oer_chip_profile::Profile::all(&root)
        .unwrap()
        .into_iter()
        .find(|profile| profile.flash.is_some())
        .unwrap()
}

/// A bundle staged for `output` whose application holds `application`.
fn staged(output: &Path, application: &[u8]) -> ImageBundle {
    let profile = profile();
    let staging = staging_directory(output).unwrap();
    remove_directory(&staging).unwrap();
    std::fs::create_dir_all(&staging).unwrap();
    let mut bundle = ImageBundle::new(&staging, &profile, profile.flash.clone().unwrap());
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
        oer_durable::sha256_bytes(b"app")
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
