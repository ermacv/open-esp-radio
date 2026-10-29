#![allow(clippy::unwrap_used, clippy::indexing_slicing)]
use super::*;

fn target() -> CalibrationCacheTarget {
    serde_json::from_value(serde_json::json!({"identity":[1,2,3],"format":123,"schema":8,"algorithm_revision":1,"profile":1})).unwrap()
}
fn key() -> CacheKey {
    CacheKey::new(
        "esp32s31",
        DeviceId::parse("020000002564").unwrap(),
        target(),
    )
    .unwrap()
}
fn provenance() -> Provenance {
    Provenance {
        image_commit: "1".repeat(40),
        application_sha256: "2".repeat(64),
        run_id: "run-1".into(),
        captured_unix_seconds: 1,
    }
}
fn status(bytes: &[u8]) -> CalibrationStartStatus {
    serde_json::from_value(serde_json::json!({"ticket":7,"state":{"Ready":{
        "decision":"FullRequested", "registration_us":123,
        "capture":{"epoch":1,"lifecycle":"Cold","full_calibration_performed":true,"path":"FullForCache","projection_schema":1,"calibration_words":92,"registers":10,"analog_bytes":8,"channel":1,"radio":"Wifi"},
        "snapshot":{"format":123,"schema":8,"algorithm_revision":1,"profile":1,"length":bytes.len(),"crc32c":calibration_cache_crc32c(bytes)}
    }}})).unwrap()
}
fn download(bytes: &[u8]) -> SnapshotDownload {
    SnapshotDownload::new(11, target(), &status(bytes)).unwrap()
}
fn page(download: &SnapshotDownload, bytes: &[u8]) -> CalibrationSnapshot {
    let request = download.next_request().unwrap();
    let first = usize::from(request.first);
    CalibrationSnapshot {
        ticket: request.ticket,
        first: request.first,
        metadata: download.metadata,
        temperature: CalibrationTemperature {
            value: -12,
            scale: CalibrationTemperatureScale::RawSensor { profile: 4 },
        },
        bytes: bytes[first..first + usize::from(request.count)]
            .try_into()
            .unwrap(),
    }
}
fn full(bytes: &[u8]) -> VerifiedFull {
    let mut download = download(bytes);
    while download.next_request().is_some() {
        download.push(page(&download, bytes)).unwrap();
    }
    download.finish().unwrap()
}

#[test]
fn atomic_promotion_is_private_opaque_and_bound_to_board_and_physical_identity() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path().join("cache"));
    let bytes: Vec<u8> = (0..261).map(|n| n as u8).collect();
    let path = store.promote(&key(), &full(&bytes), provenance()).unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let Loaded::Snapshot(record) = store.load(&key()) else {
        panic!("saved record missing")
    };
    assert_eq!(record.bytes().unwrap(), bytes);
    assert_eq!(record.provenance(), &provenance());
    let mut other = key();
    other.board = DeviceId::parse("020000002565").unwrap();
    assert!(matches!(
        store.load(&other),
        Loaded::Miss(CalibrationCacheMiss::Missing)
    ));
    fs::create_dir_all(store.path(&other).parent().unwrap()).unwrap();
    fs::copy(&path, store.path(&other)).unwrap();
    assert!(matches!(
        store.load(&other),
        Loaded::Miss(CalibrationCacheMiss::Incompatible)
    ));
    other = key();
    other.target.identity.push(4).unwrap();
    assert_ne!(store.path(&other), path);
}

#[test]
fn revisions_control_compatibility_while_age_and_producing_commit_are_provenance() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path().into());
    store.promote(&key(), &full(&[1, 2]), provenance()).unwrap();
    for field in ["schema", "algorithm", "profile", "format"] {
        let mut other = key();
        match field {
            "schema" => other.target.schema += 1,
            "algorithm" => other.target.algorithm_revision += 1,
            "profile" => other.target.profile += 1,
            _ => other.target.format.0 += 1,
        }
        assert!(matches!(
            store.load(&other),
            Loaded::Miss(CalibrationCacheMiss::Incompatible)
        ));
    }
    let mut changed = provenance();
    changed.image_commit = "f".repeat(40);
    changed.captured_unix_seconds = 0;
    store.promote(&key(), &full(&[3]), changed).unwrap();
    assert!(matches!(store.load(&key()), Loaded::Snapshot(_)));
}

#[test]
fn truncated_corrupt_oversized_and_unknown_file_schemas_are_explicit_misses() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path().into());
    assert!(matches!(
        store.load(&key()),
        Loaded::Miss(CalibrationCacheMiss::Missing)
    ));
    let path = store
        .promote(&key(), &full(&[1, 2, 3]), provenance())
        .unwrap();
    let original = fs::read(&path).unwrap();
    for bytes in [b"{".to_vec(), vec![b' '; MAX_FILE_BYTES + 1]] {
        fs::write(&path, bytes).unwrap();
        assert!(matches!(
            store.load(&key()),
            Loaded::Miss(CalibrationCacheMiss::InvalidFile)
        ));
    }
    for (field, value) in [
        ("file_schema", serde_json::json!(2)),
        ("sha256", serde_json::json!("0".repeat(64))),
        ("bytes_hex", serde_json::json!("01020304")),
        ("boot_id", serde_json::json!(0)),
        ("ticket", serde_json::json!(0)),
    ] {
        let mut record: serde_json::Value = serde_json::from_slice(&original).unwrap();
        record[field] = value;
        fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        assert!(
            matches!(
                store.load(&key()),
                Loaded::Miss(CalibrationCacheMiss::InvalidFile)
            ),
            "{field}"
        );
    }
}

#[test]
fn only_successful_full_registration_can_create_a_download() {
    for (path, performed) in [
        (CalibrationPath::PartialFromCache, false),
        (CalibrationPath::FullForCache, false),
        (CalibrationPath::FullUncached, true),
        (CalibrationPath::PartialFromCache, true),
    ] {
        let mut status = status(&[1]);
        if let CalibrationStartState::Ready { capture, .. } = &mut status.state {
            capture.path = path;
            capture.full_calibration_performed = performed;
        }
        assert!(SnapshotDownload::new(1, target(), &status).is_err());
    }
    let mut failed = status(&[1]);
    failed.state = CalibrationStartState::Failed {
        decision: CalibrationCacheDecision::FullRequested,
    };
    assert!(SnapshotDownload::new(1, target(), &failed).is_err());
    assert!(SnapshotDownload::new(0, target(), &status(&[1])).is_err());
}

#[test]
fn incomplete_stale_changed_or_corrupt_export_never_promotes() {
    let bytes = vec![42; 65];
    assert!(download(&bytes).finish().is_err());
    let mut download = download(&bytes);
    for changed in 0..4 {
        let mut bad = page(&download, &bytes);
        match changed {
            0 => bad.ticket += 1,
            1 => bad.first += 1,
            2 => bad.metadata.crc32c ^= 1,
            _ => {
                bad.bytes.pop();
            }
        }
        assert!(download.push(bad).is_err());
        assert_eq!(download.bytes.len(), 0);
    }
    download.push(page(&download, &bytes)).unwrap();
    let mut bad = page(&download, &bytes);
    bad.temperature.value += 1;
    assert!(download.push(bad).is_err());
    let mut bad = page(&download, &bytes);
    bad.bytes[0] ^= 1;
    download.push(bad).unwrap();
    download.push(page(&download, &bytes)).unwrap();
    assert!(download.finish().is_err());
}

#[test]
fn failed_promotion_keeps_last_good_and_per_key_lock_excludes_other_checkouts() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path().into());
    let path = store.promote(&key(), &full(&[1]), provenance()).unwrap();
    let previous = fs::read(&path).unwrap();
    let lock = FileLock::acquire(
        &path.parent().unwrap().join("promotion.lock"),
        Mode::Exclusive,
    )
    .unwrap();
    assert!(store.promote(&key(), &full(&[2]), provenance()).is_err());
    assert_eq!(fs::read(&path).unwrap(), previous);
    drop(lock);
    let mut invalid = provenance();
    invalid.run_id.clear();
    assert!(store.promote(&key(), &full(&[3]), invalid).is_err());
    assert_eq!(fs::read(&path).unwrap(), previous);
    store.promote(&key(), &full(&[4]), provenance()).unwrap();
    let Loaded::Snapshot(record) = store.load(&key()) else {
        panic!("last full missing")
    };
    assert_eq!(record.bytes().unwrap(), [4]);
}

#[test]
fn arbitrary_host_text_cannot_become_a_path_component() {
    for chip in ["", "../esp32s31", "/esp32s31", "ESP32S31", "x/y"] {
        assert!(CacheKey::new(chip, key().board, target()).is_err());
    }
}

#[test]
fn archive_failure_does_not_advance_last_full_and_success_retains_the_exact_record() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::at(dir.path().join("store"));
    let path = store.promote(&key(), &full(&[1]), provenance()).unwrap();
    let previous = fs::read(&path).unwrap();
    let blocked = dir.path().join("blocked");
    fs::write(&blocked, b"file").unwrap();
    assert!(
        store
            .promote_archived(
                &key(),
                &full(&[2]),
                provenance(),
                &blocked.join("artifact.json")
            )
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), previous);
    let artifact = dir.path().join("run/snapshot.json");
    store
        .promote_archived(&key(), &full(&[3]), provenance(), &artifact)
        .unwrap();
    assert_eq!(fs::read(&path).unwrap(), fs::read(&artifact).unwrap());
    assert!(
        store
            .promote_archived(&key(), &full(&[4]), provenance(), &artifact)
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), fs::read(&artifact).unwrap());
}
