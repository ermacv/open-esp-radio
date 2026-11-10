//! What a room knows it holds, through its writer.

extern crate std;

use super::{Held, Slot};

const RUN: Held = Held {
    at: 4,
    length: 8,
    octet: 0x5a,
};

#[test]
fn opening_a_room_hands_its_run_on_and_forgets_it() {
    let mut bytes = [0; 16];
    let mut slot = Slot::new(&mut bytes, Some(RUN));
    assert_eq!(slot.held(), Some(RUN));
    let (room, held) = slot.open();
    assert_eq!(room.len(), 16);
    assert_eq!(held, Some(RUN));
    assert_eq!(
        slot.held(),
        None,
        "a write the writer does not report holds no run"
    );
    slot.hold(Some(RUN));
    assert_eq!(slot.held(), Some(RUN));
}

#[test]
fn a_run_past_the_room_is_none() {
    let mut bytes = [0; 10];
    let past = Held {
        at: 4,
        length: 7,
        octet: 1,
    };
    assert_eq!(Slot::new(&mut bytes, Some(past)).held(), None);
    let mut slot = Slot::new(&mut bytes, None);
    slot.hold(Some(past));
    assert_eq!(slot.held(), None);
    let overflow = Held {
        at: usize::MAX,
        length: 2,
        octet: 1,
    };
    slot.hold(Some(overflow));
    assert_eq!(slot.held(), None);
}

#[test]
fn a_run_covers_only_its_own_octet_inside_it() {
    assert!(RUN.covers(4, 8, 0x5a));
    assert!(RUN.covers(6, 2, 0x5a));
    assert!(!RUN.covers(3, 2, 0x5a), "it starts before the run");
    assert!(!RUN.covers(10, 4, 0x5a), "it ends after the run");
    assert!(!RUN.covers(4, 8, 0x00), "another octet");
    assert!(!RUN.covers(usize::MAX, 2, 0x5a));
}
