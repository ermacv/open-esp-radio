use super::*;

#[test]
fn a_checkout_links_to_the_store_and_refuses_runs_of_its_own() {
    let directory = tempfile::tempdir().unwrap();
    let store = RunStore::at(directory.path().join("store"));
    let first = directory.path().join("first");
    assert_eq!(store.link(&first).unwrap(), Linked::Created);
    assert_eq!(store.link(&first).unwrap(), Linked::Existing);
    assert_eq!(
        RunStore::of_runs(&first.join(CHECKOUT_RUNS))
            .unwrap()
            .root(),
        fs::canonicalize(store.root()).unwrap()
    );

    let empty = directory.path().join("empty");
    fs::create_dir_all(empty.join(CHECKOUT_RUNS)).unwrap();
    assert_eq!(store.link(&empty).unwrap(), Linked::Created);

    let own = directory.path().join("own");
    fs::create_dir_all(own.join(CHECKOUT_RUNS).join("a-run")).unwrap();
    assert!(store.link(&own).is_err());
    assert!(own.join(CHECKOUT_RUNS).join("a-run").is_dir());
}

#[test]
fn notes_sidecars_default_to_empty_and_keep_typed_notes() {
    let directory = tempfile::tempdir().unwrap();
    let store = RunStore::at(directory.path());
    assert!(store.read::<Notes>(Sidecar::Pins).unwrap().is_empty());
    let note = Note {
        by: "wifi".into(),
        reason: "cited".into(),
        unix_millis: 7,
    };
    store
        .note(Sidecar::Pins, "1-a", Some(note.clone()))
        .unwrap();
    store
        .note(Sidecar::Quarantine, "s", Some(note.clone()))
        .unwrap();
    assert_eq!(store.read::<Notes>(Sidecar::Pins).unwrap()["1-a"], note);
    store.note(Sidecar::Pins, "1-a", None).unwrap();
    assert!(store.read::<Notes>(Sidecar::Pins).unwrap().is_empty());
    assert_eq!(store.read::<Notes>(Sidecar::Quarantine).unwrap().len(), 1);
    fs::write(store.sidecar_path(Sidecar::Pins), b"[1]").unwrap();
    assert!(store.read::<Notes>(Sidecar::Pins).is_err());
}

#[test]
fn a_run_id_names_one_directory_of_the_store() {
    let directory = tempfile::tempdir().unwrap();
    let store = RunStore::at(directory.path());
    for id in ["", "..", "a/b", ".hidden"] {
        assert!(store.open(id).is_err(), "{id}");
    }
    assert!(store.open("1-missing").is_err());
    assert!(store.ids_newest_first().unwrap().is_empty());
}

#[test]
fn a_path_below_a_run_belongs_to_that_run() {
    let directory = tempfile::tempdir().unwrap();
    let run = directory.path().join("runs/1-a");
    fs::create_dir_all(run.join("scenarios/s/repetition-001")).unwrap();
    assert_eq!(run_of(&run.join("scenarios/s/repetition-001")), None);
    fs::write(run.join("manifest.json"), b"{}").unwrap();
    assert_eq!(
        run_of(&run.join("scenarios/s/repetition-001")).as_deref(),
        Some("1-a")
    );
}

#[test]
fn concurrent_note_changes_are_all_kept() {
    let directory = tempfile::tempdir().unwrap();
    let store = RunStore::at(directory.path());
    let note = |by: &str| Note {
        by: by.to_owned(),
        reason: String::from("concurrent"),
        unix_millis: 0,
    };
    std::thread::scope(|scope| {
        for writer in 0..8 {
            let store = &store;
            scope.spawn(move || {
                for run in 0..16 {
                    store
                        .note(Sidecar::Pins, &format!("{writer}-{run}"), Some(note("t")))
                        .unwrap();
                }
            });
        }
    });
    let notes = store.read::<Notes>(Sidecar::Pins).unwrap();
    assert_eq!(notes.len(), 8 * 16);
}

/// Pruning reads the pins and deletes under the shared lock: a pin taken
/// meanwhile waits until it is released.
#[test]
fn a_pin_waits_while_the_pins_are_held_shared() {
    use oer_tool_base_process::lock::Mode;
    let directory = tempfile::tempdir().unwrap();
    let store = RunStore::at(directory.path());
    let held = store.lock_notes(Sidecar::Pins, Mode::Shared).unwrap();
    // Another reader shares it.
    drop(store.lock_notes(Sidecar::Pins, Mode::Shared).unwrap());
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        let store = &store;
        scope.spawn(move || {
            store
                .note(
                    Sidecar::Pins,
                    "1-a",
                    Some(Note {
                        by: String::from("hil"),
                        reason: String::from("selected"),
                        unix_millis: 0,
                    }),
                )
                .unwrap();
            done.send(()).unwrap();
        });
        assert!(
            finished
                .recv_timeout(std::time::Duration::from_millis(300))
                .is_err(),
            "the pin landed while the pins were held"
        );
        drop(held);
        finished
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
    });
    assert!(
        store
            .read::<Notes>(Sidecar::Pins)
            .unwrap()
            .contains_key("1-a")
    );
}
