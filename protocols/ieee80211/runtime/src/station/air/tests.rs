use super::*;

/// The slowest rate the tests' associations admit.
const FLOOR: Option<PhyRate> = Some(PhyRate::Legacy(
    oer_protocols_ieee80211_wire::phy::LegacyRate::Dsss1M,
));

/// An end of `cost` that never says its rate.
const fn end(cost: TxEstimate) -> TxEnd {
    TxEnd { cost, rate: None }
}
use oer_protocols_network_port::Class;

struct Fixed(u32);

impl FrameCost for Fixed {
    fn least(&self, frame_octets: usize, _rate: PhyRate) -> NonZeroU32 {
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
    link.publish(up(3), FLOOR);
    let first = air.candidate(&ready(1514)).unwrap();
    let small = air.candidate(&ready(64)).unwrap();
    assert_eq!((first.key, small.key), (3, 3), "every peer, one account");
    assert_eq!(
        (first.minimum_micros.get(), small.minimum_micros.get()),
        (1514, 64),
        "each frame's least grant"
    );
    link.publish(up(4), FLOOR);
    assert_eq!(air.candidate(&ready(1514)).unwrap().key, 4);
    assert_eq!(air.ceiling(&ready(1514)).get(), 2_514);
}

#[test]
fn a_buffer_comes_back_only_with_its_end_and_a_new_epoch_ends_the_old_account_first() {
    let mut ends = Completions::<&str, u8, 2>::new();
    assert!(!ends.waiting(up(1)));
    assert_eq!(ends.take_notice(up(1)).map(|(notice, _)| notice), None);
    ends.returned("a", 7, end(TxEstimate::Estimated { micros: 300 }))
        .unwrap();
    ends.returned("b", 8, TxEnd::NONE).unwrap();
    assert_eq!(ends.returned("c", 9, TxEnd::NONE), Err("c"), "full");
    assert!(ends.waiting(up(1)));
    assert_eq!(
        ends.take_notice(up(1)),
        Some((
            RadioNotice::Ended(Exchange {
                ticket: 7,
                charged: NonZeroU32::new(300),
                rate: None
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
                charged: Some(NonZeroU32::MIN),
                rate: None
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
    ends.returned("a", 6, end(TxEstimate::Unknown)).unwrap();
    assert_eq!(ends.take_notice(up(1)), Some((RadioNotice::Lost(5), None)));
    assert_eq!(
        ends.take_notice(up(1)),
        Some((
            RadioNotice::Ended(Exchange {
                ticket: 6,
                charged: None,
                rate: None
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
        link.publish(up(epoch), FLOOR);
        // The previous association's end comes first.
        assert!(handed_on(&mut ends, &mut egress).is_empty());
        let (_, held) = egress
            .choose([Frame(ready(1514))].into_iter())
            .unwrap()
            .expect("every association has its account");
        let ticket = u8::try_from(epoch).unwrap();
        egress.accepted(held, ticket).unwrap();
    }
    link.publish(up(6), FLOOR);
    assert!(handed_on(&mut ends, &mut egress).is_empty());
    assert_eq!((egress.unsettled(), egress.in_flight()), (0, 5));
    // Now every buffer comes back, late.
    for (ticket, name) in (1..=5_u8).zip(names) {
        ends.returned(name, ticket, end(TxEstimate::Estimated { micros: 400 }))
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

/// Two full rates of the same mode, MCS and width, apart in guard interval.
fn ht(mcs: u8, short_gi: bool) -> PhyRate {
    use oer_protocols_ieee80211_wire::phy::{HtMcs, HtRate, PpduBandwidth};
    PhyRate::Ht(HtRate::new(HtMcs::new(mcs).unwrap(), PpduBandwidth::Mhz20, short_gi).unwrap())
}

/// A [`FrameCost`] that says which rate each least grant was asked at.
#[derive(Default)]
struct Asked(core::cell::RefCell<std::vec::Vec<PhyRate>>);

impl FrameCost for &Asked {
    fn least(&self, frame_octets: usize, rate: PhyRate) -> NonZeroU32 {
        self.0.borrow_mut().push(rate);
        NonZeroU32::new(u32::try_from(frame_octets).unwrap()).unwrap()
    }
    fn ceiling(&self, frame_octets: usize) -> NonZeroU32 {
        NonZeroU32::new(u32::try_from(frame_octets).unwrap()).unwrap()
    }
}

/// The least grant goes at the association's last reported full rate: its
/// slowest rate until one reports, each later report as it comes, and none
/// of an earlier association's.
#[test]
fn the_least_grant_goes_at_the_association_s_last_reported_rate() {
    let link = NetworkLink::new();
    link.publish(up(1), FLOOR);
    let asked = Asked::default();
    let mut air = StationAir::new(&link, &asked);
    air.candidate(&ready(1514)).unwrap();
    air.reported(1, ht(7, true));
    air.candidate(&ready(1514)).unwrap();
    // The same epoch and length, another guard interval: its own rate.
    air.reported(1, ht(7, false));
    air.candidate(&ready(1514)).unwrap();
    link.publish(up(2), FLOOR);
    air.candidate(&ready(1514)).unwrap();
    air.reported(2, ht(0, false));
    air.candidate(&ready(1514)).unwrap();
    let floor = FLOOR.unwrap();
    assert_eq!(
        *asked.0.borrow(),
        [floor, ht(7, true), ht(7, false), floor, ht(0, false)]
    );
}

/// The owner's review counterexample, on the rate-aware signature: a cost
/// model whose context changes within one association and length.
struct ReviewerChanging<'a>(&'a core::cell::Cell<u32>);
impl FrameCost for ReviewerChanging<'_> {
    fn least(&self, _: usize, _: PhyRate) -> NonZeroU32 {
        NonZeroU32::new(self.0.get()).unwrap()
    }
    fn ceiling(&self, _: usize) -> NonZeroU32 {
        NonZeroU32::new(10 * self.0.get()).unwrap()
    }
}
#[test]
fn reviewer_a_changed_cost_input_updates_the_grant_within_the_same_association() {
    let link = NetworkLink::new();
    link.publish(up(1), FLOOR);
    let value = core::cell::Cell::new(100);
    let air = StationAir::new(&link, ReviewerChanging(&value));
    assert_eq!(
        air.candidate(&ready(1514)).unwrap().minimum_micros.get(),
        100
    );
    value.set(200);
    assert_eq!(
        air.candidate(&ready(1514)).unwrap().minimum_micros.get(),
        200
    );
}
#[test]
fn reviewer_a_changed_cost_input_updates_the_ceiling_within_the_same_association() {
    let link = NetworkLink::new();
    link.publish(up(1), FLOOR);
    let value = core::cell::Cell::new(100);
    let air = StationAir::new(&link, ReviewerChanging(&value));
    assert_eq!(air.ceiling(&ready(1514)).get(), 1000);
    value.set(200);
    assert_eq!(air.ceiling(&ready(1514)).get(), 2000);
}

/// A returned frame's end hands its rate on with its charge.
#[test]
fn a_returned_frame_s_rate_goes_with_its_end() {
    let mut ends = Completions::<&str, u8, 1>::new();
    let rate = ht(5, true);
    ends.returned(
        "a",
        3,
        TxEnd {
            cost: TxEstimate::Estimated { micros: 90 },
            rate: Some(rate),
        },
    )
    .unwrap();
    assert_eq!(
        ends.take_notice(up(1)),
        Some((
            RadioNotice::Ended(Exchange {
                ticket: 3,
                charged: NonZeroU32::new(90),
                rate: Some(rate),
            }),
            Some("a")
        ))
    );
}

/// A frame of association 1 that completes after association 2 began
/// changes nothing of association 2's rate: its slowest while it has
/// reported none, and its own once it has.
#[test]
fn a_late_completion_of_an_earlier_association_leaves_the_current_rate() {
    let link = NetworkLink::new();
    link.publish(up(1), FLOOR);
    let asked = Asked::default();
    let mut air = StationAir::new(&link, &asked);
    air.reported(1, ht(7, true));
    link.publish(up(2), FLOOR);
    air.reported(1, ht(3, true));
    air.candidate(&ready(1514)).unwrap();
    air.reported(2, ht(5, false));
    air.reported(1, ht(1, false));
    air.candidate(&ready(1514)).unwrap();
    assert_eq!(*asked.0.borrow(), [FLOOR.unwrap(), ht(5, false)]);
}

/// The same through the policy: association 1's frame is still open when
/// association 2 begins, and its completion settles on association 1's
/// account with its rate, which association 2's grants never take.
#[test]
fn a_late_completion_through_the_policy_leaves_the_new_association_s_grant() {
    use crate::egress::AirtimeEgress;
    use oer_protocols_ieee80211_core_upper_mac::airtime::AirtimeStorage;
    use oer_protocols_network_port::Egress;
    let link = NetworkLink::new();
    link.publish(up(1), FLOOR);
    let asked = Asked::default();
    let mut storage = AirtimeStorage::<StationKey, 2, 2>::new(NonZeroU32::new(10_000).unwrap());
    let mut egress =
        AirtimeEgress::<'_, _, u8, 2, 2>::new(&mut storage, StationAir::new(&link, &asked));
    let (_, held) = egress
        .choose([Frame(ready(1514))].into_iter())
        .unwrap()
        .expect("association 1's account");
    egress.accepted(held, 1).unwrap();
    link.publish(up(2), FLOOR);
    egress
        .notice(RadioNotice::Ended(Exchange {
            ticket: 1,
            charged: NonZeroU32::new(500),
            rate: Some(ht(7, true)),
        }))
        .unwrap();
    asked.0.borrow_mut().clear();
    let _chosen = egress
        .choose([Frame(ready(1514))].into_iter())
        .unwrap()
        .expect("association 2's account");
    assert_eq!(
        *asked.0.borrow(),
        [FLOOR.unwrap()],
        "association 2 reported no rate yet"
    );
}

fn legacy(rate: oer_protocols_ieee80211_wire::phy::LegacyRate) -> PhyRate {
    PhyRate::Legacy(rate)
}

/// An OFDM-only association's grants start at its own slowest rate, 6 Mb/s,
/// never at DSSS, until a frame of it reports its rate.
#[test]
fn an_ofdm_only_association_starts_at_its_own_slowest_rate() {
    use oer_protocols_ieee80211_wire::phy::LegacyRate;
    let link = NetworkLink::new();
    link.publish(up(1), Some(legacy(LegacyRate::Ofdm6M)));
    let asked = Asked::default();
    let mut air = StationAir::new(&link, &asked);
    air.candidate(&ready(1514)).unwrap();
    air.reported(1, ht(7, true));
    air.candidate(&ready(1514)).unwrap();
    assert_eq!(*asked.0.borrow(), [legacy(LegacyRate::Ofdm6M), ht(7, true)]);
}

/// Two associations that have reported nothing, at the same length, ask
/// their grants at their own slowest rates: what the cost model sees is the
/// resolved rate, never one `None` for both.
#[test]
fn two_associations_without_reports_ask_at_their_own_slowest_rates() {
    use oer_protocols_ieee80211_wire::phy::LegacyRate;
    let link = NetworkLink::new();
    let asked = Asked::default();
    let air = StationAir::new(&link, &asked);
    link.publish(up(1), Some(legacy(LegacyRate::Dsss1M)));
    air.candidate(&ready(1514)).unwrap();
    link.publish(up(2), Some(legacy(LegacyRate::Ofdm6M)));
    air.candidate(&ready(1514)).unwrap();
    assert_eq!(
        *asked.0.borrow(),
        [legacy(LegacyRate::Dsss1M), legacy(LegacyRate::Ofdm6M)]
    );
}

/// A link up with no rate resolved for its association, which the
/// lifecycle never publishes, offers no account: no frame of it is charged
/// a grant at a rate the association was not given, and the policy chooses
/// nothing for it.
#[test]
fn a_link_up_without_its_association_s_rate_offers_no_account() {
    use crate::egress::AirtimeEgress;
    use oer_protocols_ieee80211_core_upper_mac::airtime::AirtimeStorage;
    use oer_protocols_network_port::Egress;
    let link = NetworkLink::new();
    link.publish(up(1), None);
    let asked = Asked::default();
    let mut storage = AirtimeStorage::<StationKey, 2, 2>::new(NonZeroU32::new(10_000).unwrap());
    let mut egress =
        AirtimeEgress::<'_, _, u8, 2, 2>::new(&mut storage, StationAir::new(&link, &asked));
    assert_eq!(egress.air().candidate(&ready(1514)), None);
    assert!(
        egress
            .choose([Frame(ready(1514))].into_iter())
            .unwrap()
            .is_none()
    );
    assert!(asked.0.borrow().is_empty(), "the cost model is never asked");
    // Once the association's rate is published, its frames go.
    link.publish(up(1), FLOOR);
    assert!(
        egress
            .choose([Frame(ready(1514))].into_iter())
            .unwrap()
            .is_some()
    );
}
