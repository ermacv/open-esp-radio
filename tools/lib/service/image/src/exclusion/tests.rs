use super::*;

fn workspace_fixture() -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let workspace = root.join("application");
    fs::create_dir_all(workspace.join("src")).unwrap();
    fs::write(
        workspace.join("Cargo.toml"),
        "[package]\nname = 'application'\nversion = '0.1.0'\nedition = '2024'\n",
    )
    .unwrap();
    fs::write(workspace.join("src/lib.rs"), "").unwrap();
    fs::write(workspace.join("Cargo.lock"), "version = 4\n").unwrap();
    (directory, workspace)
}

#[test]
fn build_lock_copies_the_committed_catalog_and_owns_its_directory() {
    let (directory, workspace) = workspace_fixture();
    let output = directory.path().join("output");
    let lock = BuildLock::prepare(&workspace, &output).unwrap();
    assert_eq!(lock.path(), output.join("Cargo.lock"));
    assert_eq!(
        fs::read(lock.path()).unwrap(),
        fs::read(workspace.join("Cargo.lock")).unwrap()
    );
    let error = BuildLock::prepare(&workspace, &output)
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("another build owns"), "{error}");
    // Another output directory builds the same workspace concurrently.
    let _other = BuildLock::prepare(&workspace, &directory.path().join("other")).unwrap();
    // dup shares the open file description as inheritance across fork does;
    // releasing the owner must not depend on that descriptor closing.
    let inherited = lock._lease.file().try_clone().unwrap();
    drop(lock);
    BuildLock::prepare(&workspace, &output).unwrap();
    drop(inherited);
}

#[test]
fn cargo_resolves_into_the_private_copy_and_leaves_the_catalog_untouched() {
    let (directory, workspace) = workspace_fixture();
    let committed = fs::read(workspace.join("Cargo.lock")).unwrap();
    let lock = BuildLock::prepare(&workspace, &directory.path().join("output")).unwrap();
    let mut command = Command::new(oer_toolchain::cargo_program());
    command
        .args([
            "metadata",
            "--offline",
            "--format-version",
            "1",
            "--manifest-path",
        ])
        .arg(workspace.join("Cargo.toml"));
    lock.configure(&mut command);
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(workspace.join("Cargo.lock")).unwrap(), committed);
    let resolved = fs::read_to_string(lock.path()).unwrap();
    assert!(resolved.contains("name = \"application\""), "{resolved}");
}

#[test]
fn validation_rejects_any_change_to_the_committed_pins() {
    let (directory, workspace) = workspace_fixture();
    let root = directory.path();
    let catalog =
        |other: &str| format!("version = 4\n[[package]]\nname = 'other'\nversion = '{other}'\n");
    fs::write(workspace.join("Cargo.lock"), catalog("1.0.0")).unwrap();
    let lock = BuildLock::prepare(&workspace, &root.join("output")).unwrap();
    lock.validate().unwrap();
    fs::write(lock.path(), catalog("2.0.0")).unwrap();
    assert!(lock.validate().is_err());
}

#[test]
fn the_compile_cache_is_used_by_one_build_at_a_time() {
    let directory = tempfile::tempdir().unwrap();
    let cache = directory.path().join("nested");
    let first = oer_toolchain::image::lock_compile_cache(&cache).unwrap();
    let path = cache.join("build.lock");
    assert!(
        FileLock::try_acquire(&path, Mode::Shared)
            .unwrap()
            .is_none()
    );
    drop(first);
    let _second = oer_toolchain::image::lock_compile_cache(&cache).unwrap();
    assert!(
        FileLock::try_acquire(&path, Mode::Exclusive)
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_build_slot_is_one_of_the_host_s_slots() {
    let directory = tempfile::tempdir().unwrap();
    let mut held = Vec::new();
    for _ in 0..slots() {
        held.push(slot(directory.path()).unwrap());
    }
    // Every slot is taken: none is free without waiting.
    for index in 0..slots() {
        let path = directory.path().join(format!("{index}.lock"));
        assert!(
            FileLock::try_acquire(&path, Mode::Exclusive)
                .unwrap()
                .is_none()
        );
    }
    held.pop();
    let _freed = slot(directory.path()).unwrap();
}
