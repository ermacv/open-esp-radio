use super::*;

const MAC: &str = "xx:xx:xx:xx:25:64";

fn access(directory: &Path) -> DeviceAccess {
    DeviceAccess::try_acquire_in(
        &directory.join("locks"),
        &DeviceId::parse(MAC).unwrap(),
        "test",
    )
    .unwrap()
    .unwrap()
}

fn receipt(image: &str) -> Receipt {
    Receipt {
        chip: "chip".into(),
        image: image.into(),
        segments: Vec::new(),
        digest: oer_durable::sha256_bytes(image.as_bytes()),
        by: "test".into(),
        unix_millis: 0,
    }
}

/// A write of `image` that the store committed as `Written`, as [`write`]
/// leaves it once its transport wrote the flash.
fn written<'a>(store: &'a Store, access: &'a DeviceAccess, image: &str) -> Written<'a> {
    let generation = store.begin(access, "test").unwrap();
    let receipt = receipt(image);
    store
        .commit(
            access,
            generation,
            |state| matches!(state, State::Writing { .. }),
            State::Written {
                receipt: receipt.clone(),
            },
        )
        .unwrap();
    Written {
        store,
        access,
        receipt,
        generation,
        pending: Start::Reset,
        _operation: access.operation().unwrap(),
    }
}

#[test]
fn a_confirmation_that_a_later_write_overtook_is_a_stale_write() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::at(directory.path().join("receipts"));
    let access = access(directory.path());
    let a = written(&store, &access, "a");
    let b = written(&store, &access, "b");
    assert_eq!(b.generation(), a.generation() + 1);
    b.started().unwrap();
    let error = a.started().unwrap_err().to_string();
    assert!(error.contains("stale write"), "{error}");
    assert_eq!(store.started(access.id()).unwrap(), Some(receipt("b")));
}

#[test]
fn a_write_whose_flash_a_later_write_began_over_commits_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::at(directory.path().join("receipts"));
    let access = access(directory.path());
    let first = store.begin(&access, "first").unwrap();
    let _second = store.begin(&access, "second").unwrap();
    assert!(
        store
            .commit(
                &access,
                first,
                |state| matches!(state, State::Writing { .. }),
                State::Written {
                    receipt: receipt("a"),
                },
            )
            .is_err()
    );
    assert!(matches!(
        store.state(access.id()).unwrap(),
        Some(State::Writing { by, .. }) if by == "second"
    ));
}

#[test]
fn a_started_write_is_confirmed_once() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::at(directory.path().join("receipts"));
    let access = access(directory.path());
    let a = written(&store, &access, "a");
    let generation = a.generation();
    assert_eq!(a.started().unwrap(), receipt("a"));
    // A confirmation replays nothing: the state is no longer `Written`.
    let replay = Written {
        _operation: access.operation().unwrap(),
        store: &store,
        access: &access,
        receipt: receipt("a"),
        generation,
        pending: Start::Reset,
    };
    assert!(replay.started().is_err());
}

#[test]
fn a_write_replaces_a_receipt_of_another_schema() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::at(directory.path().join("receipts"));
    let access = access(directory.path());
    std::fs::create_dir_all(directory.path().join("receipts")).unwrap();
    std::fs::write(store.path(access.id()), br#"{"schema":1}"#).unwrap();
    assert!(store.state(access.id()).is_err());
    assert_eq!(store.begin(&access, "test").unwrap(), 1);
    assert!(matches!(
        store.state(access.id()).unwrap(),
        Some(State::Writing { .. })
    ));
}
