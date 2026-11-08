//! Grep gate for the notification audit in docs/execution-wakes.md.
use std::{collections::BTreeMap, path::Path};

fn visit(directory: &Path, files: &mut Vec<std::path::PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if ![
                "target",
                "tests",
                "examples",
                "vendor",
                "registers",
                "pac-raw",
            ]
            .contains(&name.as_ref())
            {
                visit(&path, files)?;
            }
        } else if path.extension().is_some_and(|extension| extension == "rs")
            && name != "generated.rs"
            && !name.starts_with("test")
        {
            files.push(path);
        }
    }
    Ok(())
}

#[test]
fn raw_atomic_wakers_stay_within_the_reviewed_inventory() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let mut files = Vec::new();
    for directory in ["chips", "portable", "protocols", "oer", "examples", "hil"] {
        visit(&root.join(directory), &mut files).expect("source inventory is readable");
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
        ("chips/esp32s31/app/src/lib.rs", 3),
        ("chips/esp32s31/app/src/wifi/network.rs", 3),
        ("chips/esp32s31/radio/ieee80211/backend/src/port/lent.rs", 3),
        (
            "chips/esp32s31/radio/ieee80211/backend/src/rx/control.rs",
            6,
        ),
        (
            "chips/esp32s31/radio/ieee80211/backend/src/rx/counters/stall.rs",
            3,
        ),
        ("chips/esp32s31/radio/ieee80211/backend/src/rx/upper.rs", 6),
        ("chips/family/app/src/channel.rs", 3),
        ("hil/images/wifi-station/src/udp.rs", 5),
        ("portable/memory/src/lib.rs", 1),
        ("portable/memory/src/slot_queue.rs", 9),
        ("portable/memory/src/waking_spsc.rs", 13),
        ("portable/sync/src/lib.rs", 5),
        ("portable/waker/src/lib.rs", 7),
        ("portable/waker/src/model.rs", 3),
        ("protocols/ieee80211/runtime/src/station/notifies.rs", 3),
        ("protocols/ieee80211/testing/model/src/lib.rs", 1),
        ("protocols/network/runtime/src/local.rs", 3),
        ("protocols/network/runtime/src/returns.rs", 3),
        ("protocols/network/runtime/src/socket.rs", 5),
        ("protocols/network/runtime/src/watch.rs", 3),
        ("protocols/radio/port/src/slot.rs", 3),
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
            include_str!("../../../../chips/esp32s31/radio/ieee80211/backend/src/rx/control.rs"),
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
    ] {
        for field in fields {
            assert!(
                source.contains(field),
                "upward notification must retain {field}"
            );
        }
    }
}
