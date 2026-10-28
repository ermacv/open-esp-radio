extern crate std;

use std::{format, vec::Vec};

use oer_trace::{Described, Describer, Domain, Event, EventSet, Noop, Observer, Record};

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
    ];
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
        let (kind, channel, words) = identity(observation);
        assert_eq!(kind.domain, Domain::Ieee80211);
        assert!(kind.event < 100, "{observation:?} uses the driver's ids");
        let (first, count) = Domain::Ieee80211.channels();
        assert!(channel.bit_index() >= first);
        assert!(
            channel.bit_index() < first + count - 1,
            "{observation:?} uses the driver's channel"
        );
        let record = Record {
            tag: 0,
            kind: kind.raw(),
            t_us: 0,
            words,
        };
        let sets: &[Describer] = &[StationEvents::describe];
        let described = format!(
            "{}",
            Described {
                record: &record,
                sets
            }
        );
        assert!(
            !described.starts_with("ieee80211."),
            "{observation:?} is in the event set"
        );
        let name = format!("{observation:?}");
        let variant = std::string::String::from(name.split('(').next().expect("variant"));
        if let Some(previous) = kinds.insert(kind.raw(), variant.clone()) {
            assert_eq!(previous, variant, "kind {kind} is used twice");
        }
    }
    assert_eq!(kinds.len(), 17);
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
}

#[test]
fn malformed_words_do_not_decode() {
    assert_eq!(TokenLost::decode([1, 0]), None);
    assert_eq!(DataMsduLost::decode([6, 0]), None);
    assert_eq!(DataMsduLost::decode([0, 1]), None);
    assert_eq!(RxFrameDropped::decode([0, 9]), None);
    assert_eq!(RxFrameDropped::decode([2, 0]), None);
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
fn events_describe_with_their_names() {
    let event = GroupKeyInstalled {
        key_id: 1,
        replaced: false,
    };
    let record = Record {
        tag: 0,
        kind: GroupKeyInstalled::KIND.raw(),
        t_us: 0,
        words: event.encode(),
    };
    let sets: &[Describer] = &[StationEvents::describe];
    assert_eq!(
        format!(
            "{}",
            Described {
                record: &record,
                sets
            }
        ),
        "service.group-key key_id=1 replaced=false"
    );
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
