use super::*;

fn round_trip<E: Event + Copy + PartialEq + core::fmt::Debug>(event: E) {
    assert_eq!(E::decode(event.encode()), Some(event));
}

#[test]
fn every_station_event_decodes_to_what_it_encoded() {
    for verdict in [
        BeaconVerdict::Published,
        BeaconVerdict::Unextracted,
        BeaconVerdict::Rejected,
        BeaconVerdict::NoMetadata,
    ] {
        for rssi_dbm in [None, Some(-90), Some(0), Some(127), Some(-128)] {
            round_trip(BeaconDispatch {
                verdict,
                rssi_dbm,
                timestamp_tsf_low: 0xdead_beef,
            });
        }
    }
    round_trip(ControlMailbox {
        event: ControlEventKind::PowerSaveData,
        op: MailboxOp::Overflowed,
    });
    round_trip(PowerInputTrace {
        input: PowerInputKind::Tbtt,
        control_event_waiting: true,
    });
    round_trip(RfSleepEntered {
        mac_local_time: u32::MAX,
        monotonic_micros: 0x1234_5678,
    });
    round_trip(RfWoke {
        mac_local_time: 7,
        monotonic_micros: u32::MAX,
    });
    round_trip(NetworkTxPowerTrace {
        offer: true,
        control_event_waiting: false,
    });
    round_trip(BeaconMonitorTrace {
        op: BeaconMonitorOp::Lost,
        deadline_micros_low: 6_000_000,
    });
    for reason in [
        ExitReason::BeaconLoss,
        ExitReason::PeerDeauthentication { reason_code: 7 },
        ExitReason::PeerDisassociation { reason_code: 0 },
        ExitReason::ControlMailboxOverflow,
        ExitReason::ActiveStateRestoreFailed,
        ExitReason::GroupKeyHandshakeFailed,
        ExitReason::SaQueryTimeout,
        ExitReason::JoinTsfUnrepresentable,
    ] {
        round_trip(ControlExit { reason });
    }
    for op in [
        RxSlotOp::Claimed { slot: 0 },
        RxSlotOp::Claimed { slot: 31 },
        RxSlotOp::Published { slot: 255 },
        RxSlotOp::Discarded(RxSlotDiscard::Empty),
        RxSlotOp::Discarded(RxSlotDiscard::TooLong),
        RxSlotOp::Discarded(RxSlotDiscard::Chained),
        RxSlotOp::Discarded(RxSlotDiscard::Exhausted),
    ] {
        round_trip(RxSlotTrace { op, length: 1_700 });
    }
    for direction in [BlockAckDirection::Tx, BlockAckDirection::Rx] {
        round_trip(LinkControlTrace {
            event: LinkEvent::BlockAckOperational {
                direction,
                tid: 7,
                window: 64,
            },
        });
        round_trip(LinkControlTrace {
            event: LinkEvent::BlockAckEnded { direction, tid: 0 },
        });
    }
    for event in [
        LinkEvent::Associated {
            association_id: 2007,
        },
        LinkEvent::KeysInstalled { group_key_id: 1 },
        LinkEvent::GroupKeyRotated { key_id: 2 },
        LinkEvent::BlockAckRejected { tid: 6, status: 37 },
    ] {
        round_trip(LinkControlTrace { event });
    }
    for from in [PowerState::Awake, PowerState::PowerSave, PowerState::Dozing] {
        for to in [PowerState::Awake, PowerState::PowerSave, PowerState::Dozing] {
            round_trip(PowerStateTrace { from, to });
        }
    }
}

#[test]
fn words_no_station_event_encodes_fail_to_decode() {
    assert_eq!(BeaconDispatch::decode([4, 0]), None);
    assert_eq!(BeaconDispatch::decode([1 << 17, 0]), None);
    assert_eq!(ControlMailbox::decode([10, 0]), None);
    assert_eq!(ControlMailbox::decode([0, 3]), None);
    assert_eq!(PowerInputTrace::decode([0, 2]), None);
    assert_eq!(NetworkTxPowerTrace::decode([2, 0]), None);
    assert_eq!(BeaconMonitorTrace::decode([4, 0]), None);
    assert_eq!(ControlExit::decode([0, 1]), None);
    assert_eq!(ControlExit::decode([1, 0x1_0000]), None);
    assert_eq!(ControlExit::decode([7, 1]), None);
    assert_eq!(ControlExit::decode([8, 0]), None);
    assert_eq!(RxSlotTrace::decode([3, 0]), None);
    assert_eq!(RxSlotTrace::decode([2 | (4 << 8), 0]), None);
    assert_eq!(RxSlotTrace::decode([1 << 16, 0]), None);
    assert_eq!(RxSlotTrace::decode([0, 0x1_0000]), None);
    assert_eq!(LinkControlTrace::decode([6, 0]), None);
    assert_eq!(LinkControlTrace::decode([1 << 8, 0]), None);
    assert_eq!(LinkControlTrace::decode([1, 0x100]), None);
    assert_eq!(LinkControlTrace::decode([3 | (2 << 8), 0]), None);
    assert_eq!(LinkControlTrace::decode([4 | (1 << 8), 0]), None);
    assert_eq!(LinkControlTrace::decode([5, 1]), None);
    assert_eq!(LinkControlTrace::decode([1 << 24, 0]), None);
    assert_eq!(PowerStateTrace::decode([3, 0]), None);
    assert_eq!(PowerStateTrace::decode([0, 3]), None);
}

#[test]
fn station_events_have_distinct_kinds_and_channels() {
    let kinds = [
        BeaconDispatch::KIND,
        ControlMailbox::KIND,
        PowerInputTrace::KIND,
        NetworkTxPowerTrace::KIND,
        BeaconMonitorTrace::KIND,
        ControlExit::KIND,
        RxSlotTrace::KIND,
        LinkControlTrace::KIND,
        PowerStateTrace::KIND,
    ];
    let channels = [
        BeaconDispatch::CHANNEL,
        ControlMailbox::CHANNEL,
        PowerInputTrace::CHANNEL,
        NetworkTxPowerTrace::CHANNEL,
        BeaconMonitorTrace::CHANNEL,
        ControlExit::CHANNEL,
        RxSlotTrace::CHANNEL,
        LinkControlTrace::CHANNEL,
        PowerStateTrace::CHANNEL,
    ];
    for (index, kind) in kinds.iter().enumerate() {
        assert_eq!(kind.domain, Domain::Ieee80211);
        assert!(!kinds[..index].contains(kind));
        assert!(!channels[..index].contains(&channels[index]));
    }
}
