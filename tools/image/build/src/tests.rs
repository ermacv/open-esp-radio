use super::*;

#[test]
fn the_repository_directory_is_this_package() {
    assert!(env!("CARGO_MANIFEST_DIR").ends_with("tools/image/build"));
}

#[test]
fn an_application_selects_its_features_alone() {
    let mut application = Application {
        package: "station".into(),
        binary: "station".into(),
        features: vec!["a".into(), "b".into()],
        default_features: false,
    };
    assert_eq!(
        application.feature_arguments(),
        ["--no-default-features", "--features", "a,b"]
    );
    application.features.clear();
    application.default_features = true;
    assert!(application.feature_arguments().is_empty());
}

#[test]
fn an_unchanged_embedded_runtime_keeps_its_timestamp() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("runtime.bin");
    let target = directory.path().join("bootstrap/stage-two-runtime.bin");
    std::fs::write(&source, b"runtime").unwrap();
    replace_if_changed(&source, &target).unwrap();
    let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
    std::fs::File::options()
        .write(true)
        .open(&target)
        .unwrap()
        .set_modified(old)
        .unwrap();
    replace_if_changed(&source, &target).unwrap();
    assert_eq!(std::fs::metadata(&target).unwrap().modified().unwrap(), old);
    std::fs::write(&source, b"changed").unwrap();
    replace_if_changed(&source, &target).unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"changed");
    assert_ne!(std::fs::metadata(&target).unwrap().modified().unwrap(), old);
}
