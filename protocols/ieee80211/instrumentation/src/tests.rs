extern crate std;

use std::{format, vec::Vec};

use oer_trace::{Domain, Event, Noop, Observer};

use super::*;

fn every_event() -> Vec<Observation> {
    let mut events = std::vec![
        Observation::TokenLost(TokenLost),
        Observation::PortEventsLost(PortEventsLost),
        Observation::TxUnsent(TxUnsent),
        Observation::TxMsduTooLong(TxMsduTooLong),
        Observation::GroupKeyInstalled(GroupKeyInstalled {
            key_id: 3,
            replaced: true,
        }),
        Observation::ArpAddresses(ArpAddresses {
            sender: [192, 168, 1, 2],
            target: [192, 168, 1, 1],
        }),
        Observation::ArpTransmitted(ArpTransmitted {
            operation: ArpOperation::Reply,
            sequence: 4095,
            packet_number: 0xffff_ffff_ffff,
        }),
        Observation::EspNowPeersCleared(EspNowPeersCleared { count: 7 }),
        Observation::RxFrameDropped(RxFrameDropped {
            reason: RxDrop::Draining,
        }),
    ];
    events.extend([
        Observation::StationAuthenticated(StationAuthenticated),
        Observation::StationBeacon(StationBeacon { at_millis: 0 }),
        Observation::StationBeacon(StationBeacon {
            at_millis: u32::MAX,
        }),
        Observation::StationAssociated(StationAssociated),
        Observation::TxIndeterminate(TxIndeterminate),
        Observation::TxHardwareTimeout(TxHardwareTimeout),
        Observation::TxQueueCollision(TxQueueCollision),
        Observation::ControlDelivered(ControlDelivered),
    ]);
    for code in 0..5 {
        let evidence = Evidence::of_code(code).unwrap();
        events.push(Observation::RxContainment(RxContainment {
            ampdu: evidence,
            s_mpdu: evidence,
            bytes: Some(u32::MAX),
        }));
    }
    events.extend([
        Observation::RxContainment(RxContainment {
            ampdu: Evidence::Unavailable,
            s_mpdu: Evidence::Unavailable,
            bytes: None,
        }),
        Observation::RxBaStarted(RxBaStarted { tid: 7, window: 64 }),
        Observation::TxRateSelected(TxRateSelected { kbps: Some(65_000) }),
        Observation::TxRateSelected(TxRateSelected { kbps: None }),
    ]);
    for code in 0..7 {
        events.push(Observation::RxReorder(RxReorder {
            tid: 7,
            cause: ReorderCause::of_code(code).unwrap(),
            held: 1,
            released: 8,
            missing: 2047,
        }));
    }
    for code in 0..5 {
        events.push(Observation::RxBaStopped(RxBaStopped {
            tid: 7,
            reason: BaStop::of_code(code).unwrap(),
        }));
    }
    for code in 0..6 {
        events.push(Observation::DataMsduLost(DataMsduLost {
            reason: DataLoss::of_code(code).expect("data loss"),
        }));
    }
    for code in 0..4 {
        events.push(Observation::RsnInputDropped(RsnInputDropped {
            cause: RsnDrop::of_code(code).expect("RSN drop"),
        }));
        events.push(Observation::ScanBssObserved(ScanBssObserved {
            outcome: ScanOutcome::of_code(code).expect("scan outcome"),
        }));
    }
    for code in 0..9 {
        events.push(Observation::RxFrameDropped(RxFrameDropped {
            reason: RxDrop::Frame(FrameDrop::of_code(code).expect("frame drop")),
        }));
    }
    for code in 0..3 {
        events.push(Observation::RxFrameDropped(RxFrameDropped {
            reason: RxDrop::Security(SecurityDrop::of_code(code).expect("security drop")),
        }));
        events.push(Observation::EspNowReplay(EspNowReplay {
            outcome: ReplayOutcome::of_code(code).expect("replay"),
        }));
    }
    for code in 0..2 {
        events.push(Observation::RxMsduRefused(RxMsduRefused {
            reason: RxMsduRefusal::of_code(code).expect("refusal"),
        }));
        events.push(Observation::EspNowTxPacketNumber(EspNowTxPacketNumber {
            outcome: PacketNumberOutcome::of_code(code).expect("PN outcome"),
        }));
    }
    for code in 0..5 {
        events.push(Observation::EspNowPeerTableChanged(
            EspNowPeerTableChanged {
                change: PeerChange::of_code(code).expect("peer change"),
            },
        ));
    }
    for operation in [ArpOperation::Request, ArpOperation::Reply] {
        for group_addressed in [false, true] {
            for key_id in 0..4 {
                events.push(Observation::ArpReceived(ArpReceived {
                    operation,
                    group_addressed,
                    key_id,
                    sequence: 1234,
                    packet_number: 0x0102_0304_0506,
                }));
            }
        }
    }
    events
}

/// The kind, channel and words of an observation's event.
fn identity(observation: Observation) -> (oer_trace::Kind, oer_trace::Channel, [u32; 2]) {
    struct Capture(core::cell::Cell<Option<(oer_trace::Kind, oer_trace::Channel, [u32; 2])>>);
    impl Observer for Capture {
        fn event<E: Event + Copy + 'static>(&self, event: E) {
            self.0.set(Some((E::KIND, E::CHANNEL, event.encode())));
        }
    }
    let capture = Capture(core::cell::Cell::new(None));
    macro_rules! forward {
        ($($variant:ident),+) => {
            match observation {
                $(Observation::$variant(event) => capture.event(event),)+
            }
        };
    }
    forward!(
        RxContainment,
        RxReorder,
        RxBaStarted,
        RxBaStopped,
        TxRateSelected,
        TokenLost,
        DataMsduLost,
        RsnInputDropped,
        ScanBssObserved,
        PortEventsLost,
        RxFrameDropped,
        RxMsduRefused,
        TxUnsent,
        TxMsduTooLong,
        GroupKeyInstalled,
        StationAuthenticated,
        StationBeacon,
        StationAssociated,
        TxIndeterminate,
        TxHardwareTimeout,
        TxQueueCollision,
        ControlDelivered,
        ArpAddresses,
        ArpTransmitted,
        ArpReceived,
        EspNowPeerTableChanged,
        EspNowPeersCleared,
        EspNowTxPacketNumber,
        EspNowReplay
    );
    capture.0.get().expect("forwarded")
}

#[test]
fn every_event_decodes_to_itself_and_kinds_are_unique_below_the_driver_range() {
    let mut kinds = std::collections::BTreeMap::new();
    for observation in every_event() {
        let (kind, channel, _words) = identity(observation);
        assert_eq!(kind.domain, Domain::Ieee80211);
        assert!(kind.event < 100, "{observation:?} uses the driver's ids");
        let (first, count) = Domain::Ieee80211.channels();
        assert!(channel.bit_index() >= first);
        assert!(
            channel.bit_index() < first + count - 1,
            "{observation:?} uses the driver's channel"
        );
        let name = format!("{observation:?}");
        let variant = std::string::String::from(name.split('(').next().expect("variant"));
        if let Some(previous) = kinds.insert(kind.raw(), variant.clone()) {
            assert_eq!(previous, variant, "kind {kind} is used twice");
        }
    }
    assert_eq!(kinds.len(), 29);
}

#[test]
fn observation_of_an_event_is_that_event_and_others_are_none() {
    for observation in every_event() {
        let (kind, _, words) = identity(observation);
        // An event type with the same kind and words, as a collector sees it.
        macro_rules! check {
            ($($variant:ident),+) => {
                match observation {
                    $(Observation::$variant(event) => {
                        assert_eq!(Observation::of(&event), Some(observation));
                        assert_eq!(<$variant as Event>::KIND, kind);
                        assert_eq!(event.encode(), words);
                    })+
                }
            };
        }
        check!(
            RxContainment,
            RxReorder,
            RxBaStarted,
            RxBaStopped,
            TxRateSelected,
            TokenLost,
            DataMsduLost,
            RsnInputDropped,
            ScanBssObserved,
            PortEventsLost,
            RxFrameDropped,
            RxMsduRefused,
            TxUnsent,
            TxMsduTooLong,
            GroupKeyInstalled,
            StationAuthenticated,
            StationBeacon,
            StationAssociated,
            TxIndeterminate,
            TxHardwareTimeout,
            TxQueueCollision,
            ControlDelivered,
            ArpAddresses,
            ArpTransmitted,
            ArpReceived,
            EspNowPeerTableChanged,
            EspNowPeersCleared,
            EspNowTxPacketNumber,
            EspNowReplay
        );
    }
    struct Foreign;
    impl Event for Foreign {
        const KIND: oer_trace::Kind = oer_trace::Kind::new(Domain::Phy, 1);
        const CHANNEL: oer_trace::Channel = match oer_trace::Channel::new(Domain::Phy, 0) {
            Ok(channel) => channel,
            Err(_) => panic!("channel"),
        };
        fn encode(&self) -> [u32; 2] {
            [0, 0]
        }
        fn decode(_: [u32; 2]) -> Option<Self> {
            Some(Self)
        }
    }
    assert_eq!(Observation::of(&Foreign), None);
    assert_eq!(StationActivity::of(&Foreign), None);
}

#[test]
fn station_activity_and_service_events_share_the_collector_in_order() {
    let recorder = Recorder::<3>::new();
    let observer = &recorder;
    oer_trace::observe!(observer, StationAssociated);
    oer_trace::observe!(observer, TxUnsent);
    oer_trace::observe!(observer, TxHardwareTimeout);
    let events: Vec<_> = recorder.observations().collect();
    if cfg!(oer_trace) {
        assert_eq!(
            events,
            [
                Observation::StationAssociated(StationAssociated),
                Observation::TxUnsent(TxUnsent),
                Observation::TxHardwareTimeout(TxHardwareTimeout),
            ]
        );
    } else {
        assert!(events.is_empty());
    }
    assert!(!recorder.overflowed());
}

#[test]
fn activity_projection_preserves_each_outcome_and_wrapping_beacon_time() {
    macro_rules! check {
        ($event:expr, $activity:expr) => {
            assert_eq!(StationActivity::of(&$event), Some($activity));
        };
    }
    check!(StationAuthenticated, StationActivity::Authenticated);
    check!(StationAssociated, StationActivity::Associated);
    check!(TxIndeterminate, StationActivity::Indeterminate);
    check!(TxHardwareTimeout, StationActivity::HardwareTimeout);
    check!(TxQueueCollision, StationActivity::QueueCollision);
    check!(ControlDelivered, StationActivity::ControlDelivered);
    for at_millis in [0, 1, u32::MAX] {
        check!(
            StationBeacon { at_millis },
            StationActivity::Beacon(at_millis)
        );
    }
    assert_eq!(StationActivity::of(&TxUnsent), None);
}

#[test]
fn capture_counts_activity_independently_when_history_is_filtered_or_full() {
    use oer_hil_agent::capture::{Reader, collector};
    use oer_hil_protocol::capture::{Error, Identity, PageRequest};
    use oer_time::Clock as _;

    struct Clock<'a>(&'a oer_time_virtual::VirtualClock);
    impl collector::Clock for Clock<'_> {
        fn micros(&self) -> u64 {
            self.0.now().as_micros()
        }
    }
    let clock = oer_time_virtual::VirtualClock::new();
    let mut capture = collector::Collector::<_, 0, 7>::new(Clock(&clock));
    for (index, mask) in [0, SERVICE.mask()].into_iter().enumerate() {
        let identity = Identity {
            capture: index as u32 + 1,
            epoch: 11,
        };
        capture.begin(identity, mask).unwrap();
        let observer = &capture;
        for _ in 0..2 {
            oer_trace::observe!(observer, StationAuthenticated);
            oer_trace::observe!(observer, StationBeacon { at_millis: 0 });
            oer_trace::observe!(observer, StationAssociated);
            oer_trace::observe!(observer, TxIndeterminate);
            oer_trace::observe!(observer, TxHardwareTimeout);
            oer_trace::observe!(observer, TxQueueCollision);
            oer_trace::observe!(observer, ControlDelivered);
        }
        let description = capture.freeze().unwrap();
        assert_eq!(description.records, 0);
        if cfg!(oer_trace) {
            let counters = capture
                .counters(PageRequest {
                    identity,
                    first: 0,
                    count: 7,
                })
                .unwrap();
            assert_eq!(description.counters, 7);
            assert_eq!(counters.counters.len(), 7);
            assert!(
                counters
                    .counters
                    .iter()
                    .all(|counter| counter.value == 2 && !counter.saturated)
            );
            let kinds: std::collections::BTreeSet<_> = counters
                .counters
                .iter()
                .map(|counter| counter.kind)
                .collect();
            assert_eq!(
                kinds,
                [
                    StationAuthenticated::KIND,
                    StationBeacon::KIND,
                    StationAssociated::KIND,
                    TxIndeterminate::KIND,
                    TxHardwareTimeout::KIND,
                    TxQueueCollision::KIND,
                    ControlDelivered::KIND
                ]
                .map(|kind| kind.raw())
                .into_iter()
                .collect()
            );
            assert_eq!(
                description.failure,
                (mask != 0).then_some(Error::RecordsFull)
            );
            assert_eq!(description.dropped, if mask == 0 { 0 } else { 14 });
        } else {
            assert_eq!(description.counters, 0);
            assert_eq!(description.failure, None);
        }
        capture.release(identity).unwrap();
    }
}

#[test]
fn malformed_words_do_not_decode() {
    assert_eq!(StationBeacon::decode([0, 1]), None);
    assert_eq!(StationAuthenticated::decode([1, 0]), None);
    assert_eq!(StationAssociated::decode([1, 0]), None);
    assert_eq!(TxIndeterminate::decode([0, 1]), None);
    assert_eq!(TxHardwareTimeout::decode([0, 1]), None);
    assert_eq!(TxQueueCollision::decode([0, 1]), None);
    assert_eq!(ControlDelivered::decode([0, 1]), None);
    assert_eq!(TokenLost::decode([1, 0]), None);
    assert_eq!(DataMsduLost::decode([6, 0]), None);
    assert_eq!(DataMsduLost::decode([0, 1]), None);
    assert_eq!(RxFrameDropped::decode([0, 9]), None);
    assert_eq!(RxFrameDropped::decode([2, 1]), None);
    assert_eq!(RxFrameDropped::decode([3, 0]), None);
    assert_eq!(GroupKeyInstalled::decode([4, 0]), None);
    assert_eq!(GroupKeyInstalled::decode([0, 2]), None);
    assert_eq!(ArpTransmitted::decode([2 * 4096, 0]), None);
    assert_eq!(EspNowPeersCleared::decode([1, 1]), None);
}

#[test]
fn frame_drop_codes_follow_the_upper_mac_order() {
    let order = [
        FrameDrop::Header,
        FrameDrop::FrameKind,
        FrameDrop::Address,
        FrameDrop::Fragmented,
        FrameDrop::Protection,
        FrameDrop::Integrity,
        FrameDrop::Replay,
        FrameDrop::Payload,
        FrameDrop::NotConnected,
    ];
    for (code, reason) in order.into_iter().enumerate() {
        assert_eq!(reason.code(), code as u32);
    }
}

#[test]
fn arp_parse_takes_ethernet_ipv4_requests_and_replies_only() {
    let mut payload = [0u8; 28];
    payload[..8].copy_from_slice(&[0, 1, 8, 0, 6, 4, 0, 1]);
    payload[14..18].copy_from_slice(&[10, 0, 0, 2]);
    payload[24..28].copy_from_slice(&[10, 0, 0, 1]);
    assert_eq!(
        ArpAddresses::parse(&payload),
        Some((
            ArpOperation::Request,
            ArpAddresses {
                sender: [10, 0, 0, 2],
                target: [10, 0, 0, 1],
            }
        ))
    );
    payload[7] = 2;
    assert_eq!(
        ArpAddresses::parse(&payload).map(|(op, _)| op),
        Some(ArpOperation::Reply)
    );
    for end in 0..28 {
        assert_eq!(ArpAddresses::parse(&payload[..end]), None);
    }
    for (index, value) in [(1, 6), (3, 0xdd), (4, 8), (5, 16), (7, 3)] {
        let mut other = payload;
        other[index] = value;
        assert_eq!(ArpAddresses::parse(&other), None, "byte {index} = {value}");
    }
}

#[test]
fn the_recorder_keeps_observations_only_with_the_trace_cfg() {
    let recorder = Recorder::<2>::new();
    let observer = &recorder;
    oer_trace::observe!(observer, TokenLost);
    oer_trace::observe!(observer, TxUnsent);
    oer_trace::observe!(observer, PortEventsLost);
    let kept: Vec<_> = recorder.observations().collect();
    if cfg!(oer_trace) {
        assert_eq!(
            kept,
            [
                Observation::TokenLost(TokenLost),
                Observation::TxUnsent(TxUnsent)
            ]
        );
        assert!(recorder.overflowed());
    } else {
        assert!(kept.is_empty());
        assert!(!recorder.overflowed());
    }
    Noop.event(TokenLost);
}

#[test]
fn aggregation_encodings_refuse_malformed_words() {
    assert_eq!(RxContainment::decode([5, 0]), None);
    assert_eq!(RxContainment::decode([0, 1]), None);
    assert_eq!(RxContainment::decode([1 << 17, 0]), None);
    assert_eq!(RxReorder::decode([7 << 8, 0]), None);
    assert_eq!(RxReorder::decode([0, 65536]), None);
    assert_eq!(RxBaStarted::decode([256, 1]), None);
    assert_eq!(RxBaStopped::decode([0, 5]), None);
    assert_eq!(TxRateSelected::decode([1, 0]), None);
    assert_eq!(TxRateSelected::decode([1, 2]), None);
}
