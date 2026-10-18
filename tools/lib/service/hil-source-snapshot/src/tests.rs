use super::*;

fn git(root: &Path, args: &[&str]) -> oer_tool_base_process::Result<Vec<u8>> {
    oer_tool_base_process::git::output(root, args)
}

/// The objects directory of captures into `output`.
fn objects(output: &Path) -> PathBuf {
    output.join("sources")
}

/// The files `snapshot` holds, by source role and path.
#[cfg(test)]
fn archived(snapshot: &Snapshot) -> Vec<PathBuf> {
    let manifest: Manifest =
        serde_json::from_slice(&fs::read(snapshot.directory.join("manifest.json")).unwrap())
            .unwrap();
    manifest
        .sources
        .iter()
        .flat_map(|source| {
            source
                .files
                .iter()
                .map(move |file| Path::new(&source.name).join(&file.path))
        })
        .collect()
}

/// A snapshot of the build inputs below `inputs`, its objects beside it
/// ([`test_objects_of`]).
pub fn test_snapshot(inputs: &Path) -> (tempfile::TempDir, Snapshot) {
    test_snapshot_into(inputs, None)
}

/// [`test_snapshot`] storing its objects in `objects`, such as the store of
/// the run that binds it.
pub fn test_snapshot_into(inputs: &Path, objects: Option<&Path>) -> (tempfile::TempDir, Snapshot) {
    let root = repository();
    fs::write(root.path().join(".gitignore"), "snapshots/\n").unwrap();
    for relative in [
        "Cargo.lock",
        "hil/targets/chip-a/Cargo.lock",
        "hil/targets/chip-a/Cargo.toml",
        "hil/targets/chip-a/stack.toml",
        "platform/chip-a/stack.toml",
        "platform/chip-a/partitions/applications.csv",
        "platform/chip-a/chip.toml",
    ] {
        let target = root.path().join(relative);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy(inputs.join(relative), target).unwrap();
    }
    git(root.path(), &["add", "."]).unwrap();
    git(root.path(), &["commit", "-qm", "build inputs"]).unwrap();
    let snapshot = capture_roots(
        &[("repository".into(), root.path().to_owned())],
        &[],
        &[],
        &root.path().join("snapshots"),
        &objects.map_or_else(
            || self::objects(&root.path().join("snapshots")),
            Path::to_owned,
        ),
    )
    .unwrap();
    (root, snapshot)
}

fn repository() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.name", "Snapshot Test"],
        vec!["config", "user.email", "snapshot@example.invalid"],
    ] {
        git(dir.path(), &args).unwrap();
    }
    fs::write(dir.path().join("Cargo.toml"), "initial\n").unwrap();
    fs::write(dir.path().join(".gitignore"), "secret.txt\n").unwrap();
    git(dir.path(), &["add", "."]).unwrap();
    git(dir.path(), &["commit", "-qm", "initial"]).unwrap();
    dir
}

#[test]
fn an_untracked_evidence_pin_neither_blocks_nor_enters_a_snapshot() {
    let root = repository();
    let output = tempfile::tempdir().unwrap();
    let target = output.path().join("snapshots");
    fs::create_dir_all(root.path().join("qualification/evidence")).unwrap();
    fs::write(root.path().join("qualification/evidence/archive.toml"), "").unwrap();
    fs::write(root.path().join("new.rs"), "pub fn new() {}\n").unwrap();
    let roots = vec![("repository".into(), root.path().to_owned())];
    let error = capture_roots(&roots, &[], &[], &target, &objects(&target))
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("\n  --source-include new.rs\n"), "{error}");
    assert!(!error.contains("archive.toml"), "{error}");
    let snapshot =
        capture_roots(&roots, &["new.rs".into()], &[], &target, &objects(&target)).unwrap();
    assert!(
        !archived(&snapshot)
            .iter()
            .any(|path| path.starts_with("repository/qualification/evidence"))
    );
}

#[test]
fn untracked_selection_is_explicit_complete_and_does_not_archive_secrets() {
    let root = repository();
    let output = tempfile::tempdir().unwrap();
    let target = output.path().join("snapshots");
    fs::write(root.path().join("new.rs"), "pub fn new() {}\n").unwrap();
    fs::write(root.path().join("secret.txt"), "do not archive").unwrap();
    let roots = vec![("repository".into(), root.path().to_owned())];
    let error = capture_roots(&roots, &[], &[], &target, &objects(&target))
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("repository:new.rs"));
    assert!(!error.contains("secret.txt"));
    assert!(!target.exists());
    let snapshot =
        capture_roots(&roots, &["new.rs".into()], &[], &target, &objects(&target)).unwrap();
    let entries = archived(&snapshot);
    assert!(entries.contains(&PathBuf::from("repository/new.rs")));
    assert!(
        !entries
            .iter()
            .any(|p| p.ends_with("secret.txt") || p.starts_with(".git"))
    );
    for invalid in [
        "../new.rs",
        "unknown:new.rs",
        "Cargo.toml",
        "secret.txt",
        "new.rs/child",
    ] {
        assert!(
            capture_roots(&roots, &[invalid.into()], &[], &target, &objects(&target)).is_err(),
            "{invalid}"
        );
    }
}

#[test]
fn snapshot_is_deterministic_and_tracks_actual_bytes_even_when_git_ignores_changes() {
    let root = repository();
    let output = tempfile::tempdir().unwrap();
    let roots = vec![("repository".into(), root.path().to_owned())];
    let first = capture_roots(&roots, &[], &[], output.path(), &objects(output.path())).unwrap();
    let second = capture_roots(&roots, &[], &[], output.path(), &objects(output.path())).unwrap();
    assert_eq!(first.snapshot_id, second.snapshot_id);
    git(
        root.path(),
        &["update-index", "--assume-unchanged", "Cargo.toml"],
    )
    .unwrap();
    fs::write(root.path().join("Cargo.toml"), "different\n").unwrap();
    let changed = capture_roots(&roots, &[], &[], output.path(), &objects(output.path())).unwrap();
    assert_ne!(first.snapshot_id, changed.snapshot_id);
    // Both versions of the file are objects named by their bytes; the
    // unchanged files are stored once for both snapshots.
    let mut stored = Vec::new();
    for entry in walk(&objects(output.path())) {
        let bytes = fs::read(&entry).unwrap();
        assert_eq!(
            entry.file_name().unwrap().to_str().unwrap(),
            digest(&bytes),
            "an object is named by its bytes"
        );
        stored.push(entry);
    }
    assert_eq!(stored.len(), archived(&first).len() + 1);
}

#[test]
fn override_roles_require_their_own_explicit_files() {
    let root = repository();
    let other = repository();
    let output = tempfile::tempdir().unwrap();
    fs::write(other.path().join("new.rs"), "override\n").unwrap();
    let roots = vec![
        ("repository".into(), root.path().into()),
        ("esp-hal".into(), other.path().into()),
    ];
    assert!(
        capture_roots(
            &roots,
            &["new.rs".into()],
            &[],
            output.path(),
            &objects(output.path())
        )
        .is_err()
    );
    assert!(
        capture_roots(
            &roots,
            &["esp-hal:new.rs".into()],
            &[],
            output.path(),
            &objects(output.path())
        )
        .is_ok()
    );
}

#[test]
fn a_corrupt_object_is_refused_and_a_capture_repairs_it() {
    let root = repository();
    let output = tempfile::tempdir().unwrap();
    let roots = vec![("repository".into(), root.path().to_owned())];
    let snapshot = capture_roots(&roots, &[], &[], output.path(), &objects(output.path())).unwrap();
    let stored = walk(&objects(output.path()));
    let corrupt = &stored[0];
    let length = fs::metadata(corrupt).unwrap().len() as usize;
    fs::write(corrupt, vec![b'x'; length]).unwrap();
    let build = tempfile::tempdir().unwrap();
    assert!(
        materialize(&snapshot.directory, &objects(output.path()), build.path()).is_err(),
        "a corrupt object never reaches a build"
    );
    capture_roots(&roots, &[], &[], output.path(), &objects(output.path())).unwrap();
    let build = tempfile::tempdir().unwrap();
    materialize(&snapshot.directory, &objects(output.path()), build.path()).unwrap();
}

#[test]
fn a_reused_snapshot_directory_with_another_manifest_is_refused() {
    let root = repository();
    let output = tempfile::tempdir().unwrap();
    let roots = vec![("repository".into(), root.path().to_owned())];
    let snapshot = capture_roots(&roots, &[], &[], output.path(), &objects(output.path())).unwrap();
    fs::write(snapshot.directory.join("manifest.json"), b"{}").unwrap();
    assert!(capture_roots(&roots, &[], &[], output.path(), &objects(output.path())).is_err());
    assert_eq!(
        fs::read(snapshot.directory.join("manifest.json")).unwrap(),
        b"{}"
    );
}

/// Every regular file below `directory`, sorted.
#[cfg(test)]
fn walk(directory: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![directory.to_owned()];
    while let Some(path) = pending.pop() {
        for entry in fs::read_dir(&path).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                pending.push(entry.path());
            } else {
                found.push(entry.path());
            }
        }
    }
    found.sort();
    found
}

#[cfg(unix)]
#[test]
fn links_cannot_capture_bytes_outside_the_selected_checkout() {
    let root = repository();
    let output = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink("/etc/passwd", root.path().join("external")).unwrap();
    let roots = vec![("repository".into(), root.path().to_owned())];
    assert!(
        capture_roots(
            &roots,
            &["external".into()],
            &[],
            output.path(),
            &objects(output.path())
        )
        .is_err()
    );
}

#[cfg(unix)]
#[test]
fn agent_guidance_symlinks_do_not_block_frozen_build_inputs() {
    let root = repository();
    let output = tempfile::tempdir().unwrap();
    for (path, text) in [
        ("CLAUDE.md", "root instructions\n"),
        ("hil/CLAUDE.md", "HIL instructions\n"),
        (".claude/skills/example/SKILL.md", "skill instructions\n"),
        ("AGENTS.md.rs", "a real source input\n"),
        (".agents-notes/context.md", "another source input\n"),
    ] {
        let path = root.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    fs::create_dir(root.path().join(".agents")).unwrap();
    for (path, target) in [
        ("AGENTS.md", "CLAUDE.md"),
        ("hil/AGENTS.md", "CLAUDE.md"),
        (".agents/skills", "../.claude/skills"),
    ] {
        std::os::unix::fs::symlink(target, root.path().join(path)).unwrap();
    }
    git(root.path(), &["add", "."]).unwrap();
    git(root.path(), &["commit", "-qm", "agent entry points"]).unwrap();
    // New local agent instructions must not require --source-include either.
    fs::create_dir(root.path().join("tools")).unwrap();
    std::os::unix::fs::symlink("../CLAUDE.md", root.path().join("tools/AGENTS.md")).unwrap();
    fs::write(root.path().join(".agents/local.md"), "local instructions\n").unwrap();
    fs::create_dir_all(root.path().join("tools/.agents/skills/local")).unwrap();
    fs::write(
        root.path().join("tools/.agents/skills/local/SKILL.md"),
        "local workflow\n",
    )
    .unwrap();

    let roots = vec![("repository".into(), root.path().to_owned())];
    let first = capture_roots(&roots, &[], &[], output.path(), &objects(output.path())).unwrap();
    let frozen = FrozenSources::open(first.directory(), &objects(output.path())).unwrap();
    for path in [
        "AGENTS.md",
        "hil/AGENTS.md",
        ".agents",
        "tools/AGENTS.md",
        "tools/.agents",
    ] {
        assert!(fs::symlink_metadata(frozen.repository().join(path)).is_err());
    }
    for path in [
        "CLAUDE.md",
        "hil/CLAUDE.md",
        ".claude/skills/example/SKILL.md",
        "AGENTS.md.rs",
        ".agents-notes/context.md",
    ] {
        assert_eq!(
            fs::read(frozen.repository().join(path)).unwrap(),
            fs::read(root.path().join(path)).unwrap()
        );
    }
    frozen.verify_unchanged().unwrap();

    fs::write(
        root.path().join(".agents/local.md"),
        "changed instructions\n",
    )
    .unwrap();
    let second = capture_roots(&roots, &[], &[], output.path(), &objects(output.path())).unwrap();
    assert_eq!(first.id(), second.id());

    std::os::unix::fs::symlink("Cargo.toml", root.path().join("build-input.toml")).unwrap();
    let error = capture_roots(
        &roots,
        &["build-input.toml".into()],
        &[],
        output.path(),
        &objects(output.path()),
    )
    .err()
    .unwrap()
    .to_string();
    assert!(error.contains("review of symlink source"), "{error}");
}

#[test]
fn materialized_inputs_do_not_follow_later_changes_to_the_live_tree() {
    let root = repository();
    fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"snapshot-input-test\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::create_dir(root.path().join("src")).unwrap();
    fs::write(
        root.path().join("src/lib.rs"),
        "pub const INPUT: u32 = 42;\n",
    )
    .unwrap();
    let output = tempfile::tempdir().unwrap();
    let roots = vec![("repository".into(), root.path().to_owned())];
    let snapshot = capture_roots(
        &roots,
        &["src/lib.rs".into()],
        &[],
        output.path(),
        &objects(output.path()),
    )
    .unwrap();
    fs::write(
        root.path().join("src/lib.rs"),
        "compile_error!(\"live tree is not the snapshot\");\n",
    )
    .unwrap();
    let build = tempfile::tempdir().unwrap();
    materialize(&snapshot.directory, &objects(output.path()), build.path()).unwrap();
    assert_eq!(
        fs::read_to_string(build.path().join("repository/src/lib.rs")).unwrap(),
        "pub const INPUT: u32 = 42;\n"
    );
    use oer_tool_base_process::CommandExt as _;
    let result = std::process::Command::new(oer_tool_base_toolchain::cargo_program())
        .current_dir(build.path().join("repository"))
        .args(["check", "--offline", "--quiet"])
        .env("CARGO_TARGET_DIR", build.path().join("target"))
        .supervised_output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn materialization_rejects_a_modified_manifest_or_a_missing_object_before_building() {
    for change in ["manifest", "object"] {
        let root = repository();
        let output = tempfile::tempdir().unwrap();
        let roots = vec![("repository".into(), root.path().to_owned())];
        let snapshot =
            capture_roots(&roots, &[], &[], output.path(), &objects(output.path())).unwrap();
        if change == "manifest" {
            fs::write(snapshot.directory.join("manifest.json"), "corrupt").unwrap();
        } else {
            fs::remove_file(&walk(&objects(output.path()))[0]).unwrap();
        }
        let build = tempfile::tempdir().unwrap();
        assert!(
            materialize(&snapshot.directory, &objects(output.path()), build.path()).is_err(),
            "{change}"
        );
        if change == "manifest" {
            assert_eq!(fs::read_dir(build.path()).unwrap().count(), 0);
        }
    }
}

#[test]
fn a_build_workspace_is_stable_exclusive_and_replaced_on_reuse() {
    let root = repository();
    let output = tempfile::tempdir().unwrap();
    let roots = vec![("repository".into(), root.path().to_owned())];
    let first = capture_roots(&roots, &[], &[], output.path(), &objects(output.path())).unwrap();
    fs::write(root.path().join("Cargo.toml"), "changed\n").unwrap();
    git(root.path(), &["commit", "-qam", "changed"]).unwrap();
    let second = capture_roots(
        &roots,
        &[],
        &[],
        &output.path().join("second"),
        &objects(output.path()),
    )
    .unwrap();
    let build = tempfile::tempdir().unwrap();
    let workspace = build.path().join("source-build");
    let opened =
        FrozenSources::open_in_workspace(&first.directory, &objects(output.path()), &workspace)
            .unwrap();
    assert_eq!(opened.repository(), workspace.join("repository"));
    fs::write(workspace.join("stale.txt"), "left behind").unwrap();
    let unchanged = workspace.join("repository/.gitignore");
    let unchanged_time = fs::metadata(&unchanged).unwrap().modified().unwrap();
    // A second build waits for the lock; the holder releases it on drop.
    assert!(
        oer_tool_base_process::lock::FileLock::try_acquire(
            &workspace.with_extension("lock"),
            oer_tool_base_process::lock::Mode::Exclusive
        )
        .unwrap()
        .is_none()
    );
    drop(opened);
    let reopened =
        FrozenSources::open_in_workspace(&second.directory, &objects(output.path()), &workspace)
            .unwrap();
    assert_eq!(reopened.repository(), workspace.join("repository"));
    assert!(!workspace.join("stale.txt").exists());
    // Unchanged bytes keep their modification time, so Cargo keeps them fresh.
    assert_eq!(
        fs::metadata(&unchanged).unwrap().modified().unwrap(),
        unchanged_time
    );
    assert_eq!(
        fs::read_to_string(workspace.join("repository/Cargo.toml")).unwrap(),
        "changed\n"
    );
    reopened.verify_unchanged().unwrap();
}

#[test]
fn concurrent_builds_take_free_workspace_slots_and_wait_when_all_are_busy() {
    let root = repository();
    let output = tempfile::tempdir().unwrap();
    let roots = vec![("repository".into(), root.path().to_owned())];
    let snapshot = capture_roots(&roots, &[], &[], output.path(), &objects(output.path())).unwrap();
    let build = tempfile::tempdir().unwrap();
    let base = build.path().join("source-build");
    let held = (0..super::WORKSPACE_SLOTS)
        .map(|slot| {
            let sources = FrozenSources::open_in_free_workspace(
                &snapshot.directory,
                &objects(output.path()),
                &base,
            )
            .unwrap();
            assert_eq!(
                sources.repository(),
                build.path().join(format!("source-build-{slot}/repository"))
            );
            sources
        })
        .collect::<Vec<_>>();
    // Every slot is held: the next build waits until one is released.
    let (sender, receiver) = std::sync::mpsc::channel();
    let waiting = {
        let directory = snapshot.directory.clone();
        let stored = objects(output.path());
        let base = base.clone();
        std::thread::spawn(move || {
            let sources =
                FrozenSources::open_in_free_workspace(&directory, &stored, &base).unwrap();
            sender.send(sources.repository()).unwrap();
        })
    };
    assert!(
        receiver
            .recv_timeout(std::time::Duration::from_millis(300))
            .is_err()
    );
    let mut held = held;
    drop(held.remove(0));
    let reused = receiver
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap();
    assert_eq!(reused, build.path().join("source-build-0/repository"));
    waiting.join().unwrap();
    drop(held);
}

#[test]
fn include_untracked_archives_only_files_inside_its_scopes_and_records_why() {
    let root = repository();
    let output = tempfile::tempdir().unwrap();
    let target = output.path().join("snapshots");
    for file in [
        "crates/driver/src/new.rs",
        "tools/lib/service/hil-runner/src/new.rs",
        "tools/lib/data/hil-scenarios/new.toml",
    ] {
        let path = root.path().join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "new\n").unwrap();
    }
    fs::write(root.path().join("notes.txt"), "scratch\n").unwrap();
    let roots = vec![("repository".into(), root.path().to_owned())];
    let scopes = untracked_scopes(vec![PathBuf::from("crates/driver")]);
    // A file outside every scope still has to be named.
    let error = capture_roots(&roots, &[], &scopes, &target, &objects(&target))
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("repository:notes.txt"), "{error}");
    assert!(!error.contains("new."), "{error}");
    let snapshot = capture_roots(
        &roots,
        &["notes.txt".into()],
        &scopes,
        &target,
        &objects(&target),
    )
    .unwrap();
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(snapshot.directory.join("manifest.json")).unwrap())
            .unwrap();
    assert_eq!(
        manifest["sources"][0]["untracked"],
        serde_json::json!([
            {"path": "crates/driver/src/new.rs", "by": "image-package"},
            {"path": "tools/lib/service/hil-runner/src/new.rs", "by": "hil-host"},
            {"path": "tools/lib/data/hil-scenarios/new.toml", "by": "hil-host"},
            {"path": "notes.txt", "by": "source-include"},
        ])
    );
}

#[test]
fn the_blocked_snapshot_error_names_arguments_ready_to_paste() {
    assert_eq!(
        source_include_arguments(&[
            ("repository", Path::new("a/new.rs")),
            ("esp-hal", Path::new("b.rs")),
        ]),
        "--source-include a/new.rs --source-include esp-hal:b.rs"
    );
}

#[test]
fn a_captured_snapshot_loads_again_from_its_directory() {
    let root = repository();
    let output = tempfile::tempdir().unwrap();
    let roots = vec![("repository".into(), root.path().to_owned())];
    let snapshot = capture_roots(&roots, &[], &[], output.path(), &objects(output.path())).unwrap();
    let loaded = Snapshot::load(snapshot.directory(), &objects(output.path())).unwrap();
    assert_eq!(loaded.directory(), snapshot.directory());
    assert!(Snapshot::load(output.path(), &objects(output.path())).is_err());
}

#[test]
fn a_capture_is_taken_again_while_a_source_changes_under_it() {
    let pause = std::time::Duration::ZERO;
    let mut calls = 0;
    let captured = until_unchanged(5, pause, || {
        calls += 1;
        if calls < 3 {
            Err(ChangedDuringCapture("repository".into()).into())
        } else {
            Ok(calls)
        }
    })
    .unwrap();
    assert_eq!(captured, 3);
    // A source that never settles fails after the last attempt.
    let mut calls = 0;
    let error = until_unchanged(2, pause, || -> crate::Result<()> {
        calls += 1;
        Err(ChangedDuringCapture("repository".into()).into())
    })
    .unwrap_err();
    assert_eq!(calls, 2);
    assert!(error.is::<ChangedDuringCapture>());
    // Any other failure is not retried.
    let mut calls = 0;
    assert!(
        until_unchanged(5, pause, || -> crate::Result<()> {
            calls += 1;
            Err("untracked files".into())
        })
        .is_err()
    );
    assert_eq!(calls, 1);
}

/// A capture of an earlier record schema for the same manifest lies in the
/// store's top level (schema 1 wrote `sources.tar` beside its record); this
/// schema's captures have their own directory and never meet it.
#[test]
fn a_capture_of_an_earlier_schema_does_not_block_a_new_one() {
    let root = repository();
    let output = tempfile::tempdir().unwrap();
    let roots = vec![("repository".into(), root.path().to_owned())];
    let first = capture_roots(&roots, &[], &[], output.path(), &objects(output.path())).unwrap();
    let earlier = output.path().join(first.id());
    fs::create_dir_all(&earlier).unwrap();
    fs::write(earlier.join("snapshot.json"), br#"{"schema":1}"#).unwrap();
    fs::write(earlier.join("sources.tar"), b"archive").unwrap();
    let again = capture_roots(&roots, &[], &[], output.path(), &objects(output.path())).unwrap();
    assert_eq!(again.id(), first.id());
    assert_eq!(
        again.directory(),
        capture_directory(output.path()).join(first.id())
    );
    assert_eq!(fs::read(earlier.join("sources.tar")).unwrap(), b"archive");
}

/// Capturing a tree again marks its capture as in use again, so collection
/// keeps it a while longer.
#[test]
fn capturing_again_refreshes_the_capture_record() {
    let root = repository();
    let output = tempfile::tempdir().unwrap();
    let roots = vec![("repository".into(), root.path().to_owned())];
    let first = capture_roots(&roots, &[], &[], output.path(), &objects(output.path())).unwrap();
    let record = first.directory().join("snapshot.json");
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(7 * 24 * 3600);
    fs::File::options()
        .append(true)
        .open(&record)
        .unwrap()
        .set_modified(old)
        .unwrap();
    capture_roots(&roots, &[], &[], output.path(), &objects(output.path())).unwrap();
    let modified = fs::metadata(&record).unwrap().modified().unwrap();
    assert!(modified.elapsed().unwrap() < std::time::Duration::from_secs(3600));
    // So does a build or run that reads it with `--source-snapshot`.
    fs::File::options()
        .append(true)
        .open(&record)
        .unwrap()
        .set_modified(old)
        .unwrap();
    mark_used(first.directory()).unwrap();
    let modified = fs::metadata(&record).unwrap().modified().unwrap();
    assert!(modified.elapsed().unwrap() < std::time::Duration::from_secs(3600));
}
