//! Pending-bit rules of the pinned `esp_ieee802154_ack.c`.

use super::{AckPending, AutoPendingMode, PendingTable, PendingTableFull, ack_pending};
use crate::mac::header::{FrameAddress, PhrFrame};

/// 2006 data frame from short source 0x5678.
const DATA_SHORT: [u8; 13] = [
    0x0c, 0x61, 0x98, 0x01, 0x34, 0x12, 0xff, 0xff, 0x78, 0x56, 0xaa, 0x00, 0x00,
];
/// 2006 Data Request from extended source 1..8.
const DATA_REQUEST_EXT: [u8; 19] = [
    18, 0x63, 0xd8, 0x07, 0x34, 0x12, 0x00, 0x00, 1, 2, 3, 4, 5, 6, 7, 8, 0x04, 0, 0,
];
const SHORT: FrameAddress = FrameAddress::Short([0x78, 0x56]);
const EXTENDED: FrameAddress = FrameAddress::Extended([1, 2, 3, 4, 5, 6, 7, 8]);

fn decide(image: &[u8], lookup: bool, mode: AutoPendingMode, table: &PendingTable<4>) -> bool {
    ack_pending(PhrFrame::new(image), lookup, mode, table).pending
}

#[test]
fn each_mode_selects_the_vendor_pending_bit() {
    let mut table = PendingTable::<4>::new();
    table.add(SHORT).unwrap();

    assert!(decide(&DATA_SHORT, false, AutoPendingMode::Disable, &table));
    assert!(decide(&DATA_SHORT, false, AutoPendingMode::Enable, &table));
    assert!(!decide(
        &DATA_REQUEST_EXT,
        true,
        AutoPendingMode::Enhanced,
        &table
    ));
    assert!(!decide(&DATA_SHORT, true, AutoPendingMode::Zigbee, &table));
    assert!(decide(
        &DATA_REQUEST_EXT,
        true,
        AutoPendingMode::Zigbee,
        &table
    ));

    table.add(EXTENDED).unwrap();
    assert!(decide(
        &DATA_REQUEST_EXT,
        false,
        AutoPendingMode::Enable,
        &table
    ));
}

/// A reserved source mode leaves no parseable source and no data request,
/// so no mode sets the pending bit.
#[test]
fn an_unparseable_source_is_never_pending() {
    let table = PendingTable::<4>::new();
    let mut reserved = DATA_REQUEST_EXT;
    reserved[2] = 0x58;
    for mode in [
        AutoPendingMode::Disable,
        AutoPendingMode::Enable,
        AutoPendingMode::Zigbee,
    ] {
        assert!(!decide(&reserved, true, mode, &table));
    }
}

#[test]
fn only_2003_and_2006_frames_reach_the_hardware_ack() {
    let table = PendingTable::<4>::new();
    assert_eq!(
        ack_pending(
            PhrFrame::new(&DATA_SHORT),
            false,
            AutoPendingMode::Disable,
            &table
        ),
        AckPending {
            pending: true,
            set_to_hardware: true
        }
    );
    let mut version_2015 = DATA_SHORT;
    version_2015[2] = 0xa8;
    assert!(
        !ack_pending(
            PhrFrame::new(&version_2015),
            false,
            AutoPendingMode::Disable,
            &table
        )
        .set_to_hardware
    );
}

#[test]
fn the_table_stores_each_address_once_and_reuses_cleared_slots() {
    let mut table = PendingTable::<2>::new();
    table.add(SHORT).unwrap();
    table.add(SHORT).unwrap();
    table.add(FrameAddress::Short([1, 0])).unwrap();
    assert_eq!(
        table.add(FrameAddress::Short([2, 0])),
        Err(PendingTableFull)
    );
    assert!(table.clear(SHORT));
    assert!(!table.clear(SHORT));
    table.add(FrameAddress::Short([2, 0])).unwrap();
    assert!(table.contains(FrameAddress::Short([2, 0])));

    table.add(EXTENDED).unwrap();
    table.reset_short();
    assert!(!table.contains(FrameAddress::Short([1, 0])));
    assert!(table.contains(EXTENDED));
    table.reset_extended();
    assert!(!table.contains(EXTENDED));
}

/// Room for an address: present, or a free slot in its half.
#[test]
fn room_is_a_present_address_or_a_free_slot() {
    let mut table = PendingTable::<1>::new();
    let short = FrameAddress::Short([1, 0]);
    assert!(table.has_room(short));
    table.add(short).unwrap();
    assert!(table.has_room(short), "a present address needs no slot");
    assert!(!table.has_room(FrameAddress::Short([2, 0])));
    assert!(table.has_room(FrameAddress::Extended([1; 8])));
    table.clear(short);
    assert!(table.has_room(FrameAddress::Short([2, 0])));
}
