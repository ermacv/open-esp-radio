//! Grep gate for the notification audit in docs/execution-wakes.md.
use std::{collections::BTreeMap, path::Path};

fn visit(directory: &Path, files: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(directory).expect("source directory is readable") {
        let path = entry.expect("source entry is readable").path();
        let name = path
            .file_name()
            .expect("a source path has a name")
            .to_str()
            .expect("UTF-8 source name");
        if path.is_dir() {
            if ![
                "target",
                "tests",
                "examples",
                "vendor",
                "registers",
                "pac-raw",
            ]
            .contains(&name)
            {
                visit(&path, files);
            }
        } else if path.extension().is_some_and(|extension| extension == "rs")
            && name != "generated.rs"
            && !name.starts_with("test")
        {
            files.push(path);
        }
    }
}

#[test]
fn raw_atomic_wakers_stay_within_the_reviewed_inventory() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let mut files = Vec::new();
    for directory in ["chips", "portable", "protocols", "firmware"] {
        visit(&root.join(directory), &mut files);
    }
    let actual = files
        .into_iter()
        .filter_map(|path| {
            let text = std::fs::read_to_string(&path).expect("source is readable");
            let count = text.matches("AtomicWaker").count() + text.matches("atomic_waker").count();
            (count != 0).then(|| {
                (
                    path.strip_prefix(&root)
                        .expect("source is below root")
                        .to_string_lossy()
                        .into_owned(),
                    count,
                )
            })
        })
        .collect::<BTreeMap<_, _>>();
    // Each exception has a direction and a reason in the audit. Counting uses
    // also catches another raw field added to a previously reviewed file.
    let expected = [
        ("chips/esp32s31/ieee80211/driver/src/port.rs", 5),
        ("chips/esp32s31/ieee80211/driver/src/rx/control.rs", 7),
        ("chips/esp32s31/ieee80211/driver/src/rx/upper.rs", 7),
        ("portable/embassy/net/src/owned.rs", 5),
        ("portable/hil-agent/src/console/writer.rs", 4),
        ("portable/sync/src/lib.rs", 6),
        (
            "protocols/ieee80211/runtime/src/station_service/msdu_queue.rs",
            13,
        ),
        ("protocols/radio/port/src/slot.rs", 4),
    ]
    .into_iter()
    .map(|(path, count)| (String::from(path), count))
    .collect::<BTreeMap<_, _>>();
    assert_eq!(
        actual, expected,
        "review raw waker additions against docs/execution-wakes.md"
    );
}

#[test]
fn upward_notification_fields_use_the_shared_primitive() {
    for (source, fields) in [
        (
            include_str!("../../../../chips/esp32s31/ieee80211/driver/src/port.rs"),
            &["driver: PriorityWaker,"][..],
        ),
        (
            include_str!("../../../../chips/esp32s31/ieee80211/driver/src/rx/control.rs"),
            &["runner: PriorityWaker,"][..],
        ),
        (
            include_str!("../../../../protocols/radio/port/src/slot.rs"),
            &["p2: PriorityWaker,"][..],
        ),
        (
            include_str!("../../../../portable/memory/src/request_slot.rs"),
            &["waker: PriorityWaker,", "client: PriorityWaker,"][..],
        ),
        (
            include_str!("../../../../firmware/apps/w1/src/measure.rs"),
            &["static PHASE_WAKER: PriorityWaker = PriorityWaker::new()"][..],
        ),
    ] {
        for field in fields {
            assert!(
                source.contains(field),
                "upward notification must retain {field}"
            );
        }
    }
}
