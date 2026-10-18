use super::*;

fn round_trip<E: Event + Copy + PartialEq + core::fmt::Debug>(event: E) {
    assert_eq!(E::decode(event.encode()), Some(event));
}

/// Every value of a raw enum, by its discriminants up to the first gap.
fn every<T>(from_raw: fn(u32) -> Option<T>) -> impl Iterator<Item = T> {
    (0..).map_while(from_raw)
}

#[test]
fn every_role_event_decodes_to_what_it_encoded() {
    for kind in every(RequestKind::from_raw) {
        for answer in every(RequestAnswer::from_raw) {
            for lead_micros in [None, Some(0), Some(u32::MAX)] {
                round_trip(RoleRequest {
                    kind,
                    answer,
                    lead_micros,
                });
            }
        }
    }
    assert_eq!(every(RequestKind::from_raw).count(), 14);
    assert_eq!(every(RequestAnswer::from_raw).count(), 11);
    for result in every(EndResult::from_raw) {
        round_trip(RoleEventEnd {
            id: u32::MAX,
            result,
            received: 255,
            discarded: 0,
        });
    }
    for rssi_dbm in [i8::MIN, -40, 0, i8::MAX] {
        round_trip(RolePdu {
            id: 7,
            header: 0xff,
            length: 37,
            rssi_dbm,
        });
    }
}

#[test]
fn every_controller_event_decodes_to_what_it_encoded() {
    for step in every(AdvertiserStepKind::from_raw) {
        round_trip(AdvertiserStep { step, id: u32::MAX });
    }
    for verdict in every(IndicationVerdict::from_raw) {
        round_trip(ConnectionIndication { verdict });
    }
    assert_eq!(every(IndicationVerdict::from_raw).count(), 7);
    round_trip(PeripheralOpened {
        first_anchor_low: u32::MAX,
        interval_units: u16::MAX,
    });
    for op in every(PeripheralEventOp::from_raw) {
        round_trip(PeripheralEvent {
            op,
            counter: u16::MAX,
            id: 3,
        });
    }
    for reason in [None, Some(0), Some(0x08), Some(u8::MAX)] {
        round_trip(PeripheralClosed { reason });
    }
}

#[test]
fn every_runtime_event_decodes_to_what_it_encoded() {
    for op in every(QueueOp::from_raw) {
        round_trip(QueueLoss { op, id: 9 });
    }
    for command in every(LifecycleCommand::from_raw) {
        for step in every(LifecycleStep::from_raw) {
            round_trip(Lifecycle { command, step });
        }
    }
}

#[test]
fn malformed_words_decode_to_nothing() {
    assert_eq!(RoleRequest::decode([14, 0]), None);
    assert_eq!(RoleRequest::decode([1 << 17, 0]), None);
    assert_eq!(RoleEventEnd::decode([0, 1 << 24]), None);
    assert_eq!(RolePdu::decode([0, 1 << 24]), None);
    assert_eq!(ConnectionIndication::decode([0, 1]), None);
    assert_eq!(PeripheralOpened::decode([0, 1 << 16]), None);
    assert_eq!(PeripheralEvent::decode([1 << 24, 0]), None);
    assert_eq!(PeripheralClosed::decode([1, 256]), None);
    assert_eq!(PeripheralClosed::decode([2, 0]), None);
    assert_eq!(Lifecycle::decode([2, 0]), None);
    assert_eq!(Lifecycle::decode([0, 4]), None);
}

#[test]
fn the_events_use_distinct_kinds_and_their_own_channels() {
    let kinds = [
        RoleRequest::KIND,
        RoleEventEnd::KIND,
        RolePdu::KIND,
        AdvertiserStep::KIND,
        ConnectionIndication::KIND,
        PeripheralOpened::KIND,
        PeripheralEvent::KIND,
        PeripheralClosed::KIND,
        QueueLoss::KIND,
        Lifecycle::KIND,
    ];
    let channels = [
        RoleRequest::CHANNEL,
        RoleEventEnd::CHANNEL,
        RolePdu::CHANNEL,
        AdvertiserStep::CHANNEL,
        ConnectionIndication::CHANNEL,
        PeripheralOpened::CHANNEL,
        PeripheralEvent::CHANNEL,
        PeripheralClosed::CHANNEL,
        QueueLoss::CHANNEL,
        Lifecycle::CHANNEL,
    ];
    for (index, kind) in kinds.iter().enumerate() {
        assert_eq!(kind.domain, Domain::Bluetooth);
        assert!(!kinds[..index].contains(kind));
        assert!(!channels[..index].contains(&channels[index]));
    }
}
