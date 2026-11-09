use super::*;
use oer_protocols_network_port::Class;

struct Fixed(u32);

impl FrameCost for Fixed {
    fn least(&self, frame_octets: usize) -> NonZeroU32 {
        NonZeroU32::new(u32::try_from(frame_octets).unwrap()).unwrap()
    }
    fn ceiling(&self, frame_octets: usize) -> NonZeroU32 {
        NonZeroU32::new(self.0 + u32::try_from(frame_octets).unwrap()).unwrap()
    }
}

fn ready(length: usize) -> Ready {
    Ready {
        class: Class::Data,
        length,
        peer: [2, 0, 0, 0, 0, 1],
        flow: Some(0),
        dscp: Default::default(),
    }
}

const fn up(epoch: u64) -> LinkSnapshot {
    LinkSnapshot {
        state: LinkState::Up,
        epoch,
    }
}

#[test]
fn an_association_is_one_account_keyed_by_its_epoch_and_a_down_link_serves_none() {
    let link = NetworkLink::new();
    let air = StationAir::new(&link, Fixed(1_000));
    assert_eq!(air.candidate(&ready(1514)), None, "down");
    link.publish(up(3));
    let first = air.candidate(&ready(1514)).unwrap();
    let small = air.candidate(&ready(64)).unwrap();
    assert_eq!((first.key, small.key), (3, 3), "every peer, one account");
    assert_eq!(
        (first.minimum_micros.get(), small.minimum_micros.get()),
        (1514, 64),
        "each frame's least grant"
    );
    link.publish(up(4));
    assert_eq!(air.candidate(&ready(1514)).unwrap().key, 4);
    assert_eq!(air.ceiling(&ready(1514)).get(), 2_514);
}

#[test]
fn a_buffer_comes_back_only_with_its_end_and_a_new_epoch_ends_the_old_account_first() {
    let mut ends = Completions::<&str, u8, 2>::new();
    assert!(!ends.waiting(up(1)));
    assert_eq!(ends.take_notice(up(1)).map(|(notice, _)| notice), None);
    ends.returned("a", 7, TxEstimate::Estimated { micros: 300 })
        .unwrap();
    ends.returned("b", 8, TxEstimate::NONE).unwrap();
    assert_eq!(ends.returned("c", 9, TxEstimate::NONE), Err("c"), "full");
    assert!(ends.waiting(up(1)));
    assert_eq!(
        ends.take_notice(up(1)),
        Some((
            RadioNotice::Ended(Exchange {
                ticket: 7,
                charged: NonZeroU32::new(300)
            }),
            Some("a")
        ))
    );
    // A new association: its predecessor's account ends before the next
    // frame's end, which still settles on that old account.
    assert!(ends.waiting(up(2)));
    assert_eq!(ends.take_notice(up(2)), Some((RadioNotice::Left(1), None)));
    assert_eq!(
        ends.take_notice(up(2)),
        Some((
            RadioNotice::Ended(Exchange {
                ticket: 8,
                charged: Some(NonZeroU32::MIN)
            }),
            Some("b")
        )),
        "a frame that never reached the air is charged the least"
    );
    assert!(!ends.waiting(up(2)));
}

#[test]
fn a_lost_frame_settles_with_no_buffer_and_an_unknown_cost_is_charged_the_ceiling() {
    let mut ends = Completions::<&str, u8, 2>::new();
    ends.lost(5).unwrap();
    ends.returned("a", 6, TxEstimate::Unknown).unwrap();
    assert_eq!(ends.take_notice(up(1)), Some((RadioNotice::Lost(5), None)));
    assert_eq!(
        ends.take_notice(up(1)),
        Some((
            RadioNotice::Ended(Exchange {
                ticket: 6,
                charged: None
            }),
            Some("a")
        ))
    );
}

#[derive(Clone, Copy)]
struct Frame(Ready);

impl oer_protocols_network_port::Candidate for Frame {
    fn ready(&self) -> &Ready {
        &self.0
    }
}

/// The station's port and egress across associations, as the product runs
/// them: each association leaves a frame whose buffer comes back late.
/// Five associations through two accounts never exhaust them, each late
/// buffer comes back once its end is handed on, and its return charges
/// neither the closed account nor the current one.
#[test]
fn late_returns_across_associations_bring_every_buffer_home_and_charge_once() {
    use crate::egress::AirtimeEgress;
    use oer_protocols_ieee80211_core_upper_mac::airtime::AirtimeStorage;
    use oer_protocols_network_port::Egress;

    const BUFFERS: usize = 8;
    let link = NetworkLink::new();
    let mut storage =
        AirtimeStorage::<StationKey, 2, BUFFERS>::new(NonZeroU32::new(2_000).unwrap());
    let mut egress = AirtimeEgress::<'_, _, u8, 2, BUFFERS>::new(
        &mut storage,
        StationAir::new(&link, Fixed(1_000)),
    );
    let mut ends = Completions::<&'static str, u8, BUFFERS>::new();
    let handed_on = |ends: &mut Completions<&'static str, u8, BUFFERS>,
                     egress: &mut AirtimeEgress<'_, _, u8, 2, BUFFERS>| {
        let mut buffers = std::vec::Vec::new();
        while let Some((notice, buffer)) = ends.take_notice(link.snapshot()) {
            egress.notice(notice).unwrap();
            buffers.extend(buffer);
        }
        buffers
    };
    let names = ["a", "b", "c", "d", "e"];
    for epoch in 1..=5_u64 {
        link.publish(up(epoch));
        // The previous association's end comes first.
        assert!(handed_on(&mut ends, &mut egress).is_empty());
        let (_, held) = egress
            .choose([Frame(ready(1514))].into_iter())
            .unwrap()
            .expect("every association has its account");
        let ticket = u8::try_from(epoch).unwrap();
        egress.accepted(held, ticket).unwrap();
    }
    link.publish(up(6));
    assert!(handed_on(&mut ends, &mut egress).is_empty());
    assert_eq!((egress.unsettled(), egress.in_flight()), (0, 5));
    // Now every buffer comes back, late.
    for (ticket, name) in (1..=5_u8).zip(names) {
        ends.returned(name, ticket, TxEstimate::Estimated { micros: 400 })
            .unwrap();
    }
    assert_eq!(handed_on(&mut ends, &mut egress), names);
    assert_eq!(egress.in_flight(), 0, "every ticket closed once");
    assert_eq!(
        egress.balance_micros(6),
        None,
        "the current one never charged"
    );
}

/// A [`FrameCost`] that counts what it computes.
#[derive(Default)]
struct Counted {
    least: core::cell::Cell<usize>,
    ceiling: core::cell::Cell<usize>,
}

impl FrameCost for &Counted {
    fn least(&self, frame_octets: usize) -> NonZeroU32 {
        self.least.set(self.least.get() + 1);
        NonZeroU32::new(u32::try_from(frame_octets).unwrap()).unwrap()
    }
    fn ceiling(&self, frame_octets: usize) -> NonZeroU32 {
        self.ceiling.set(self.ceiling.get() + 1);
        NonZeroU32::new(10 * u32::try_from(frame_octets).unwrap()).unwrap()
    }
}

/// A backlog of frames of one length costs one computation of each charge:
/// the egress asks each ready frame's least grant at every choice, and the
/// chosen one's ceiling.
#[test]
fn frames_of_one_length_compute_each_charge_once() {
    let link = NetworkLink::new();
    link.publish(up(1));
    let costs = Counted::default();
    let air = StationAir::new(&link, &costs);
    for _ in 0..32 {
        assert_eq!(
            air.candidate(&ready(1514)).unwrap().minimum_micros.get(),
            1514
        );
        assert_eq!(air.ceiling(&ready(1514)).get(), 15_140);
    }
    assert_eq!((costs.least.get(), costs.ceiling.get()), (1, 1));
    // Another length is its own charge, and the first comes back as it was.
    assert_eq!(air.candidate(&ready(64)).unwrap().minimum_micros.get(), 64);
    assert_eq!(
        air.candidate(&ready(1514)).unwrap().minimum_micros.get(),
        1514
    );
    assert_eq!(air.ceiling(&ready(64)).get(), 640);
}
