use super::*;
use oer_hil_run_bundle_format::archived::archived_identity;

#[test]
fn remote_provenance_never_retains_https_credentials() {
    assert_eq!(
        sanitize_git_remote(String::from(
            "https://user:secret-token@github.com/owner/repository.git"
        )),
        "https://github.com/owner/repository.git"
    );
    assert_eq!(
        sanitize_git_remote(String::from("git@github.com:owner/repository.git")),
        "git@github.com:owner/repository.git"
    );
}

#[test]
fn source_stability_check_rejects_a_post_capture_change() {
    let root = std::env::temp_dir().join(format!(
        "open-radio-hil-source-stability-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir(&root).unwrap();
    for arguments in [
        &["init", "-q", "-b", "main"][..],
        &["config", "user.name", "HIL Test"],
        &["config", "user.email", "hil@example.invalid"],
        &[
            "remote",
            "add",
            "origin",
            "https://example.invalid/repository.git",
        ],
    ] {
        oer_os_process::git::run(&root, arguments).unwrap();
    }
    fs::write(root.join("tracked.txt"), b"before\n").unwrap();
    oer_os_process::git::run(&root, ["add", "tracked.txt"]).unwrap();
    oer_os_process::git::run(&root, ["commit", "-m", "base"]).unwrap();
    let run = root.join("run");
    fs::create_dir(&run).unwrap();
    let source = capture_source_material(
        "repository",
        &root,
        &run,
        Path::new("source/repository.patch"),
    )
    .unwrap();
    assert_eq!(
        capture_git_source_state(&root)
            .unwrap()
            .unwrap()
            .workspace_sha256,
        source.workspace_sha256
    );
    fs::write(root.join("tracked.txt"), b"after\n").unwrap();
    assert_ne!(
        capture_git_source_state(&root)
            .unwrap()
            .unwrap()
            .workspace_sha256,
        source.workspace_sha256
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn collection_deletes_only_objects_no_archive_links_to() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("target");
    let source = directory.path().join("source");
    fs::create_dir_all(&source).unwrap();
    for name in ["kept", "dropped"] {
        fs::write(source.join(name), name).unwrap();
        archive_content_addressed(
            Source::Built(&source.join(name)),
            &directory.path().join("run").join(name),
            &target,
        )
        .unwrap();
    }
    // The run that archived `dropped` is pruned.
    let dropped = directory.path().join("run/dropped");
    fs::remove_file(&dropped).unwrap();
    let prefix = target.join("objects/sha256/00");
    fs::create_dir_all(&prefix).unwrap();
    fs::write(prefix.join(".firmware-artifact.tmp-1-0"), "partial").unwrap();

    let collected = collect_objects(&target).unwrap();
    assert_eq!(collected.objects, 1);
    assert_eq!(collected.bytes, ("dropped".len() + "partial".len()) as u64);
    assert!(!prefix.join(".firmware-artifact.tmp-1-0").exists());
    assert_eq!(
        fs::read(directory.path().join("run/kept")).unwrap(),
        b"kept"
    );
    // The kept object is reused by a later identical archive.
    archive_content_addressed(
        Source::Built(&source.join("kept")),
        &directory.path().join("again/kept"),
        &target,
    )
    .unwrap();
    assert_eq!(
        collect_objects(&target).unwrap(),
        CollectedObjects::default()
    );
    // An empty store collects nothing.
    assert_eq!(
        collect_objects(&directory.path().join("none")).unwrap(),
        CollectedObjects::default()
    );
}

/// A runtime ELF is kept deflate-compressed in the object store and the
/// bundle; what is recorded of it, and what its one reader returns, is the
/// uncompressed file, and a replay from such a bundle links the same object.
#[test]
fn a_runtime_elf_is_archived_compressed_and_read_back_whole() {
    use std::os::unix::fs::MetadataExt as _;
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("target");
    let built = directory.path().join("build/runtime.elf");
    fs::create_dir_all(built.parent().unwrap()).unwrap();
    let bytes = b"ELF section bytes repeat. ".repeat(4096);
    fs::write(&built, &bytes).unwrap();
    let archived = directory
        .path()
        .join("run/firmware/performance/runtime.elf");
    let identity = archive_content_addressed(Source::Built(&built), &archived, &target).unwrap();
    assert_eq!(identity.size_bytes, bytes.len() as u64);
    assert_eq!(identity.sha256, oer_os_durable::sha256_bytes(&bytes));

    let stored = stored_path(&archived);
    assert_eq!(stored.file_name().unwrap(), "runtime.elf.deflate");
    assert!(!archived.exists());
    assert!(fs::metadata(&stored).unwrap().len() < bytes.len() as u64 / 10);
    assert_eq!(read_archived(&archived).unwrap(), bytes);
    let read = archived_identity(&archived).unwrap();
    assert_eq!(
        (read.size_bytes, read.sha256),
        (identity.size_bytes, identity.sha256.clone())
    );
    let object = target
        .join("objects/sha256")
        .join(&identity.sha256[..2])
        .join(format!("{}.deflate", identity.sha256));
    assert!(object.is_file());

    // A replay archives the same identity and shares the object.
    let replayed = directory
        .path()
        .join("replay/firmware/performance/runtime.elf");
    let again = archive_content_addressed(Source::Archived(&archived), &replayed, &target).unwrap();
    assert_eq!(again.sha256, identity.sha256);
    assert_eq!(fs::metadata(&object).unwrap().nlink(), 3);
    // Other files stay as they are.
    let application = directory
        .path()
        .join("run/firmware/performance/application.bin");
    assert_eq!(stored_path(&application), application);
}

/// A compressed object whose bytes no longer decompress to its identity is
/// refused, not linked into a bundle.
#[test]
fn a_corrupt_compressed_object_is_refused() {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("target");
    let built = directory.path().join("build/runtime.elf");
    fs::create_dir_all(built.parent().unwrap()).unwrap();
    fs::write(&built, b"runtime").unwrap();
    let sha256 = oer_os_durable::sha256_bytes(b"runtime");
    let object = target
        .join("objects/sha256")
        .join(&sha256[..2])
        .join(format!("{sha256}.deflate"));
    fs::create_dir_all(object.parent().unwrap()).unwrap();
    fs::write(&object, b"not deflate of the runtime").unwrap();
    let error = archive_content_addressed(
        Source::Built(&built),
        &directory.path().join("run/runtime.elf"),
        &target,
    )
    .unwrap_err();
    assert!(
        !directory.path().join("run/runtime.elf.deflate").exists(),
        "{error}"
    );
}
