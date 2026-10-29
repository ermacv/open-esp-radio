extern crate std;
use std::format;

use crate::{DESCRIPTORS, LockMismatch, MESSAGES_LOCK, check_lock};

#[test]
fn the_reviewed_lock_lists_exactly_this_crates_descriptors() {
    assert_eq!(check_lock(MESSAGES_LOCK, DESCRIPTORS), Ok(()));
}

#[test]
fn a_changed_key_a_missing_line_and_a_duplicate_are_refused() {
    let line = "lifecycle/reset 29c5dbb4127bbf20 endpoint";
    assert!(MESSAGES_LOCK.contains(line));
    let changed = MESSAGES_LOCK.replace(line, "lifecycle/reset 29c5dbb4127bbf21 endpoint");
    assert_eq!(
        check_lock(&changed, DESCRIPTORS),
        Err(LockMismatch::Stale(
            "lifecycle/reset 29c5dbb4127bbf21 endpoint"
        ))
    );
    let missing = MESSAGES_LOCK.replace(&format!("{line}\n"), "");
    assert_eq!(
        check_lock(&missing, DESCRIPTORS),
        Err(LockMismatch::Missing("lifecycle/reset"))
    );
    let duplicate = format!("{MESSAGES_LOCK}{line}\n");
    assert!(check_lock(&duplicate, DESCRIPTORS).is_err());
    let framing = MESSAGES_LOCK.replace("framing 2", "framing 3");
    assert_eq!(
        check_lock(&framing, DESCRIPTORS),
        Err(LockMismatch::Framing)
    );
}
