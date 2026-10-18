use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use oer_ieee80211_mac::sequence::SequenceNumber;

use super::*;

#[test]
fn mailbox_preserves_owned_agreement_edges_in_order() {
    let resources = RxReorderCommandResources::<NoopRawMutex>::new();
    let (sender, receiver) = resources.split();
    let snapshot = RxBlockAckSnapshot {
        hardware_index: 0,
        interface: oer_esp32s31_ieee80211_mac::MacInterface::Station,
        peer: [2, 0, 0, 0, 0, 1],
        tid: 3,
        starting_sequence: SequenceNumber::new(0x0ffe).unwrap(),
        window: 32,
    };
    let start = RxReorderCommand::Start(snapshot);
    let stop = RxReorderCommand::Stop(snapshot.identity());
    try_send_rx_reorder_command(&sender, start).unwrap();
    try_send_rx_reorder_command(&sender, stop).unwrap();
    let stop_station =
        RxReorderCommand::StopInterface(oer_esp32s31_ieee80211_mac::MacInterface::Station);
    try_send_rx_reorder_command(&sender, stop_station).unwrap();

    assert_eq!(try_receive_rx_reorder_command(&receiver), Some(start));
    assert_eq!(try_receive_rx_reorder_command(&receiver), Some(stop));
    assert_eq!(
        try_receive_rx_reorder_command(&receiver),
        Some(stop_station)
    );
    assert_eq!(try_receive_rx_reorder_command(&receiver), None);
}

#[test]
fn full_mailbox_returns_the_unpublished_command() {
    let resources = RxReorderCommandResources::<NoopRawMutex>::new();
    let (sender, _receiver) = resources.split();
    for hardware_index in 0..RX_REORDER_COMMAND_CAPACITY {
        try_send_rx_reorder_command(
            &sender,
            RxReorderCommand::Stop(RxBlockAckIdentity {
                hardware_index: hardware_index as u8,
                interface: oer_esp32s31_ieee80211_mac::MacInterface::Station,
                peer: [2, 0, 0, 0, 0, 1],
                tid: 0,
            }),
        )
        .unwrap();
    }
    assert_eq!(
        try_send_rx_reorder_command(
            &sender,
            RxReorderCommand::StopInterface(oer_esp32s31_ieee80211_mac::MacInterface::Station,),
        ),
        Err(RxReorderCommandError::Full(
            RxReorderCommand::StopInterface(oer_esp32s31_ieee80211_mac::MacInterface::Station,)
        ))
    );
}

#[test]
fn retained_backing_copies_metadata_and_returns_its_logical_slot() {
    let storage = RxReorderFrameStorage::<16>::new();
    let reservation = storage.try_reserve().unwrap();
    let slot = reservation.slot();
    let bytes = [1, 2, 3, 4, 5];
    let frame = match reservation.copy_from(RxSegment {
        descriptor_address: 0x1000,
        descriptor_word0: 0x2000,
        buffer: &bytes,
        next_descriptor_address: 0x3000,
    }) {
        Ok(frame) => frame,
        Err((error, _reservation)) => panic!("retained copy failed: {error:?}"),
    };
    assert_eq!(storage.available_slots(), RX_REORDER_BACKING_SLOT_COUNT - 1);
    assert_eq!(frame.slot(), slot);
    assert_eq!(frame.segment().as_segment().buffer, bytes);
    assert_eq!(frame.segment().as_segment().descriptor_word0, 0x2000);
    drop(frame);
    assert_eq!(storage.available_slots(), RX_REORDER_BACKING_SLOT_COUNT);
}

#[test]
fn board_profile_selects_the_allocated_reorder_slot_count() {
    let storage = RxReorderFrameStorage::<16, 3>::new();
    assert_eq!(storage.available_slots(), 3);
    let first = storage.try_reserve().unwrap();
    let second = storage.try_reserve().unwrap();
    let third = storage.try_reserve().unwrap();
    assert_eq!(storage.available_slots(), 0);
    assert!(matches!(
        storage.try_reserve(),
        Err(RxReorderStorageError::Exhausted)
    ));
    drop((first, second, third));
    assert_eq!(storage.available_slots(), 3);
}

#[test]
fn unmaterialized_reservation_and_oversize_copy_release_the_slot() {
    let storage = RxReorderFrameStorage::<4>::new();
    drop(storage.try_reserve().unwrap());
    assert_eq!(storage.available_slots(), RX_REORDER_BACKING_SLOT_COUNT);

    let bytes = [0; 5];
    let reservation = storage.try_reserve().unwrap();
    let (error, reservation) = match reservation.copy_from(RxSegment {
        descriptor_address: 0,
        descriptor_word0: 0,
        buffer: &bytes,
        next_descriptor_address: 0,
    }) {
        Ok(_frame) => panic!("oversize retained copy unexpectedly succeeded"),
        Err(failure) => failure,
    };
    assert_eq!(error, RxReorderStorageError::TooLong(5));
    drop(reservation);
    assert_eq!(storage.available_slots(), RX_REORDER_BACKING_SLOT_COUNT);
}
