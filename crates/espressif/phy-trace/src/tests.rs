use super::*;

extern crate std;

use std::format;

fn round_trip<E: Event + Copy + PartialEq + core::fmt::Debug>(event: E) {
    assert_eq!(E::decode(event.encode()), Some(event));
}

const REFUSALS: [Refusal; 15] = [
    Refusal::NotRegistered,
    Refusal::AlreadyRegistered,
    Refusal::TrackingPending,
    Refusal::NoTrackingPending,
    Refusal::Poisoned,
    Refusal::RfClosed,
    Refusal::RfOpen,
    Refusal::EpochMismatch,
    Refusal::ClientsActive,
    Refusal::Clock,
    Refusal::Acquire,
    Refusal::Release,
    Refusal::Time,
    Refusal::ClientAbsent(Client::Bluetooth),
    Refusal::ClientAbsent(Client::Wifi),
];

const FAULT: Fault = Fault {
    stage: FaultStage::Hardware,
    detail: 0xbeef,
};

#[test]
fn every_phy_event_decodes_to_what_it_encoded() {
    round_trip(Registration::Started);
    for path in [
        CalibrationPath::FullUncached,
        CalibrationPath::FullForCache,
        CalibrationPath::FullAfterRejectedCache,
        CalibrationPath::PartialFromCache,
    ] {
        round_trip(Registration::Calibrated(path));
    }
    for refusal in REFUSALS {
        round_trip(Registration::Refused(refusal));
        round_trip(TrackingTick::Unavailable(refusal));
        round_trip(TrackingTick::Refused(refusal));
        round_trip(RfLifecycle {
            operation: RfOperation::Wake,
            result: RfResult::Refused(refusal),
        });
        round_trip(WifiChannel {
            channel_or_frequency: 5180,
            bandwidth: 2,
            result: ChannelResult::Refused(refusal),
        });
    }
    for stage in [
        FaultStage::Transition,
        FaultStage::Hardware,
        FaultStage::EpochMismatch,
        FaultStage::MissingOwner,
        FaultStage::Clock,
    ] {
        let fault = Fault { stage, detail: 7 };
        round_trip(Registration::Failed(fault));
        round_trip(TrackingTick::Failed(fault));
        round_trip(Poison {
            by: PoisonedBy::RfWake,
            fault,
        });
    }
    round_trip(TrackingTick::NotDue);
    round_trip(TrackingTick::Tracked(TrackingProgress {
        wifi_requested: true,
        rfpll_corrected: true,
        calibration_transmit: true,
        calibration_bluetooth_ieee802154: true,
        ..TrackingProgress::default()
    }));
    for operation in [RfOperation::Close, RfOperation::Wake] {
        for result in [
            RfResult::Done,
            RfResult::Recoverable(FAULT),
            RfResult::Failed(FAULT),
        ] {
            round_trip(RfLifecycle { operation, result });
        }
    }
    round_trip(ClientChange {
        client: Client::Ieee802154,
        acquired: true,
        active: Clients::NONE.with(Client::Wifi).with(Client::Ieee802154),
    });
    round_trip(ClientChange {
        client: Client::Bluetooth,
        acquired: false,
        active: Clients::NONE,
    });
    round_trip(TemperatureReferences {
        rfpll: 24,
        calibration: -40,
        transmit: 127,
        power: -128,
        observed: -300,
    });
    for by in [
        PoisonedBy::Registration,
        PoisonedBy::Tracking,
        PoisonedBy::RfClose,
        PoisonedBy::RfWake,
        PoisonedBy::WifiChannel,
    ] {
        round_trip(Poison { by, fault: FAULT });
    }
    for result in [ChannelResult::Done, ChannelResult::Failed(FAULT)] {
        round_trip(WifiChannel {
            channel_or_frequency: 11,
            bandwidth: 1,
            result,
        });
    }
}

#[test]
fn words_no_phy_event_encodes_fail_to_decode() {
    assert_eq!(Registration::decode([0, 1]), None);
    assert_eq!(Registration::decode([1, 4]), None);
    assert_eq!(Registration::decode([4, 0]), None);
    assert_eq!(Registration::decode([2, 17]), None);
    assert_eq!(Registration::decode([2, 13 | 3 << 8]), None);
    assert_eq!(Registration::decode([2, 1 << 8]), None);
    assert_eq!(Registration::decode([3, 6 << 16]), None);
    assert_eq!(TrackingTick::decode([1, 1 << 10]), None);
    assert_eq!(TrackingTick::decode([3, 1]), None);
    assert_eq!(TrackingTick::decode([6, 0]), None);
    assert_eq!(RfLifecycle::decode([2, 0]), None);
    assert_eq!(RfLifecycle::decode([4 << 8, 0]), None);
    assert_eq!(RfLifecycle::decode([0, 1]), None);
    assert_eq!(ClientChange::decode([3, 0]), None);
    assert_eq!(ClientChange::decode([2 << 8, 0]), None);
    assert_eq!(ClientChange::decode([0, 8]), None);
    assert_eq!(TemperatureReferences::decode([0, 1 << 16]), None);
    assert_eq!(Poison::decode([5, 0]), None);
    assert_eq!(WifiChannel::decode([3 << 24, 0]), None);
    assert_eq!(WifiChannel::decode([0, 1]), None);
}

#[test]
fn retired_codes_stay_unassigned() {
    for client in 0..3 {
        assert_eq!(Refusal::from_raw(13 | client << 8), None);
    }
    assert_eq!(Refusal::from_raw(15), None);
    assert_eq!(Refusal::from_raw(16), None);
    assert_eq!(FaultStage::from_raw(2), None);
    assert_eq!(TrackingTick::decode([3, 0]), None);
}

#[test]
fn one_channel_gates_each_phy_event() {
    let channels = [
        Registration::CHANNEL,
        TrackingTick::CHANNEL,
        RfLifecycle::CHANNEL,
        ClientChange::CHANNEL,
        TemperatureReferences::CHANNEL,
        Poison::CHANNEL,
        WifiChannel::CHANNEL,
    ];
    for (index, channel) in channels.iter().enumerate() {
        assert!(channels[index + 1..].iter().all(|other| other != channel));
    }
}

fn snapshot(bus: BusRead) -> PhySnapshot {
    PhySnapshot {
        poison: Poison {
            by: PoisonedBy::Tracking,
            fault: FAULT,
        },
        slot: Slot::Pending,
        clients: Clients::NONE.with(Client::Wifi).with(Client::Bluetooth),
        temperatures: TemperatureReferences {
            rfpll: 24,
            calibration: 23,
            transmit: 25,
            power: 22,
            observed: 26,
        },
        bus,
        platform_clocks: [1, 2, 1, 0, 255, 0, 0, 7],
    }
}

#[test]
fn snapshot_decodes_to_what_it_encoded() {
    for bus in [
        BusRead::DomainOff,
        BusRead::Read(BusState {
            pbus_busy: true,
            analog_i2c_busy: [false, true],
            pbus_results: [1, 2, 3, 4, 5, 6, 7, 8, 9, 0x1ff, 0xffff],
        }),
        BusRead::Read(BusState::default()),
    ] {
        let snapshot = snapshot(bus);
        assert_eq!(PhySnapshot::decode(&snapshot.encode()), Some(snapshot));
    }
}

#[test]
fn snapshot_rejects_words_it_never_encodes() {
    let words = snapshot(BusRead::DomainOff).encode();
    assert_eq!(PhySnapshot::decode(&words[..11]), None, "truncated slot");
    let mut version = words;
    version[0] += 1;
    assert_eq!(PhySnapshot::decode(&version), None);
    let mut unread_with_words = words;
    unread_with_words[6] = 1;
    assert_eq!(PhySnapshot::decode(&unread_with_words), None);
    let mut busy_flags = words;
    busy_flags[5] = 8;
    assert_eq!(PhySnapshot::decode(&busy_flags), None);
    let mut last_pair = words;
    last_pair[11] = 1 << 16;
    assert_eq!(PhySnapshot::decode(&last_pair), None);
    let mut slot = words;
    slot[0] |= 7 << 8;
    assert_eq!(PhySnapshot::decode(&slot), None);
}

#[test]
fn snapshot_names_an_unread_bus() {
    let text = format!("{}", snapshot(BusRead::DomainOff));
    assert!(text.contains("bus not read: domain off"), "{text}");
    assert!(text.contains("clients wifi+bluetooth"), "{text}");
}
