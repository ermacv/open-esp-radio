extern crate std;

use std::vec::Vec;

use core::num::NonZeroU32;

use oer_protocols_ieee80211_contracts_port::PhyRate;
use oer_protocols_ieee80211_core_upper_mac::airtime::{AirtimeCandidate, AirtimeStorage};
use oer_protocols_network_port::{Candidate, Class, Egress, Ready};

use super::{AirtimeEgress, AirtimeFault, Exchange, PeerAir, RadioNotice};

const QUANTUM: u32 = 1_000;
const COST: u32 = 400;
/// The most a frame costs, every retry included.
const CEILING: u32 = 4 * COST;

fn micros(value: u32) -> NonZeroU32 {
    NonZeroU32::new(value).unwrap()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Work(Ready);

impl Candidate for Work {
    fn ready(&self) -> &Ready {
        &self.0
    }
}

fn data(peer: u8, flow: u8) -> Work {
    Work(Ready {
        class: Class::Data,
        length: 100,
        peer: [2, 0, 0, 0, 0, peer],
        flow: Some(flow),
        dscp: Default::default(),
    })
}

fn control(peer: u8) -> Work {
    Work(Ready {
        class: Class::Control,
        length: 42,
        peer: [2, 0, 0, 0, 0, peer],
        flow: None,
        dscp: Default::default(),
    })
}

/// Peers are their last octet; `asleep` may not be served.
#[derive(Default)]
struct Air {
    asleep: Vec<u8>,
    /// What the policy reported of each settled frame's air.
    reports: Vec<(u8, PhyRate)>,
}

impl PeerAir for Air {
    type Key = u8;

    fn candidate(&self, ready: &Ready) -> Option<AirtimeCandidate<u8>> {
        let key = ready.peer[5];
        (!self.asleep.contains(&key)).then(|| AirtimeCandidate {
            key,
            minimum_micros: micros(COST),
        })
    }

    fn ceiling(&self, _ready: &Ready) -> NonZeroU32 {
        micros(CEILING)
    }

    fn reported(&mut self, key: u8, rate: PhyRate) {
        self.reports.push((key, rate));
    }
}

type Policy<'o> = AirtimeEgress<'o, Air, u8, 4, 2>;

/// The place of a test's `round`th frame, each ended before the next: the
/// two places in turn.
fn place(round: u32) -> u8 {
    u8::try_from(round % 2).unwrap()
}

fn storage() -> AirtimeStorage<u8, 4, 2> {
    AirtimeStorage::new(micros(QUANTUM))
}

/// Choose, publish under `ticket` and complete at `charged`.
fn exchange(policy: &mut Policy<'_>, ready: &[Work], ticket: u8, charged: u32) -> Work {
    let (work, held) = policy
        .choose(ready.iter().copied(), &mut ())
        .unwrap()
        .unwrap();
    policy.accepted(held, ticket).unwrap();
    let exchange = Exchange {
        ticket,
        charged: Some(micros(charged)),
        rate: None,
    };
    policy.notice(RadioNotice::Ended(exchange)).unwrap();
    work
}

#[test]
fn peers_at_one_cost_take_equal_turns() {
    let mut storage = storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    let ready = [data(1, 0), data(2, 1)];
    let mut served = [0_u32; 3];
    for round in 0..40 {
        let work = exchange(&mut policy, &ready, place(round), COST);
        served[usize::from(work.0.peer[5])] += 1;
    }
    assert_eq!(served[1], served[2]);
}

#[test]
fn a_peer_that_costs_more_waits_longer() {
    let mut storage = storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    let ready = [data(1, 0), data(2, 1)];
    let mut air = [0_u32; 3];
    for round in 0..40 {
        let ticket = place(round);
        let (work, held) = policy
            .choose(ready.iter().copied(), &mut ())
            .unwrap()
            .unwrap();
        let peer = work.0.peer[5];
        let charged = if peer == 2 { 4 * COST } else { COST };
        air[usize::from(peer)] += charged;
        policy.accepted(held, ticket).unwrap();
        let done = Exchange {
            ticket,
            charged: Some(micros(charged)),
            rate: None,
        };
        policy.notice(RadioNotice::Ended(done)).unwrap();
    }
    assert!(air[1].abs_diff(air[2]) <= 2 * 4 * COST, "{air:?}");
}

#[test]
fn a_peers_flows_take_turns_within_its_share() {
    let mut storage = storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    let ready = [data(1, 0), data(1, 1), data(1, 2)];
    let flows: Vec<_> = (0..6)
        .map(|round| {
            exchange(&mut policy, &ready, place(round), COST)
                .0
                .flow
                .unwrap()
        })
        .collect();
    assert_eq!(flows, [0, 1, 2, 0, 1, 2]);
}

#[test]
fn many_flows_of_one_peer_earn_it_no_more_turns() {
    let mut storage = storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    let ready = [data(1, 0), data(1, 1), data(1, 2), data(2, 3)];
    let mut served = [0_u32; 3];
    for round in 0..40 {
        let work = exchange(&mut policy, &ready, place(round), COST);
        served[usize::from(work.0.peer[5])] += 1;
    }
    assert!(served[1].abs_diff(served[2]) <= 2, "{served:?}");
}

#[test]
fn control_work_goes_first_within_its_peer() {
    let mut storage = storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    let ready = [data(1, 0), control(1)];
    let work = exchange(&mut policy, &ready, 0, COST);
    assert_eq!(work.0.class, Class::Control);
}

#[test]
fn a_sleeping_peer_is_not_chosen_and_earns_nothing() {
    let mut storage = storage();
    let air = Air {
        asleep: [2].into(),
        ..Air::default()
    };
    let mut policy = Policy::new(&mut storage, air);
    let ready = [data(1, 0), data(2, 1)];
    for round in 0..10 {
        assert_eq!(
            exchange(&mut policy, &ready, place(round), COST).0.peer[5],
            1
        );
    }
    assert_eq!(
        policy.balance_micros(2),
        None,
        "no credit, not even an account"
    );
    policy.air_mut().asleep.clear();
    assert_eq!(exchange(&mut policy, &ready, 0, COST).0.peer[5], 2);
}

#[test]
fn nothing_ready_chooses_nothing() {
    let mut storage = storage();
    let air = Air {
        asleep: [1].into(),
        ..Air::default()
    };
    let mut policy = Policy::new(&mut storage, air);
    let chosen = policy.choose([data(1, 0)].into_iter(), &mut ()).unwrap();
    assert!(chosen.is_none());
}

#[test]
fn a_full_horizon_waits_for_a_completion() {
    let mut storage = storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    let ready = [data(1, 0)];
    for ticket in 0..2 {
        let (_, held) = policy
            .choose(ready.iter().copied(), &mut ())
            .unwrap()
            .unwrap();
        policy.accepted(held, ticket).unwrap();
    }
    assert!(
        policy
            .choose(ready.iter().copied(), &mut ())
            .unwrap()
            .is_none()
    );
    assert_eq!(policy.in_flight(), 2);
    let done = Exchange {
        ticket: 0,
        charged: Some(micros(COST)),
        rate: None,
    };
    policy.notice(RadioNotice::Ended(done)).unwrap();
    assert!(
        policy
            .choose(ready.iter().copied(), &mut ())
            .unwrap()
            .is_some()
    );
}

#[test]
fn a_refusal_returns_the_whole_reservation() {
    let mut storage = storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    let ready = [data(1, 0)];
    let (_, held) = policy
        .choose(ready.iter().copied(), &mut ())
        .unwrap()
        .unwrap();
    assert_eq!(policy.balance_micros(1), Some(0), "the balance is reserved");
    policy.refused(held).unwrap();
    assert_eq!(policy.balance_micros(1), Some(i64::from(QUANTUM)));
}

#[test]
fn a_known_cost_replaces_the_reservation() {
    let mut storage = storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    exchange(&mut policy, &[data(1, 0)], 0, COST);
    assert_eq!(policy.balance_micros(1), Some(i64::from(QUANTUM - COST)));
}

#[test]
fn an_unknown_cost_keeps_the_whole_reservation() {
    let mut storage = storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    let ready = [data(1, 0)];
    let (_, held) = policy
        .choose(ready.iter().copied(), &mut ())
        .unwrap()
        .unwrap();
    policy.accepted(held, 1).unwrap();
    let unknown = Exchange {
        ticket: 1,
        charged: None,
        rate: None,
    };
    policy.notice(RadioNotice::Ended(unknown)).unwrap();
    let left = i64::from(QUANTUM) - i64::from(CEILING);
    assert_eq!(policy.balance_micros(1), Some(left));
}

#[test]
fn a_frame_of_unknown_end_is_charged_and_not_refunded() {
    let mut storage = storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    let ready = [data(1, 0)];
    let (_, held) = policy
        .choose(ready.iter().copied(), &mut ())
        .unwrap()
        .unwrap();
    policy.unknown(held).unwrap();
    let left = i64::from(QUANTUM) - i64::from(CEILING);
    assert_eq!(policy.balance_micros(1), Some(left));
    assert_eq!(policy.in_flight(), 0);
}

#[test]
fn a_lost_frame_settles_its_reservation_at_the_ceiling_and_frees_its_place() {
    let mut storage = storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    let ready = [data(1, 0)];
    let (_, held) = policy
        .choose(ready.iter().copied(), &mut ())
        .unwrap()
        .unwrap();
    policy.accepted(held, 1).unwrap();
    policy.notice(RadioNotice::Lost(1)).unwrap();
    let left = i64::from(QUANTUM) - i64::from(CEILING);
    assert_eq!(policy.balance_micros(1), Some(left));
    assert_eq!(policy.in_flight(), 0, "nothing waits for its completion");
    assert_eq!(
        policy.notice(RadioNotice::Lost(1)),
        Err(AirtimeFault::UnknownTicket),
        "settled once"
    );
}

#[test]
fn a_completion_of_no_frame_is_a_fault() {
    let mut storage = storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    let stray = Exchange {
        ticket: 9,
        charged: None,
        rate: None,
    };
    assert_eq!(
        policy.notice(RadioNotice::Ended(stray)),
        Err(AirtimeFault::UnknownTicket)
    );
}

#[test]
fn a_retired_peers_frame_settles_at_its_ceiling_and_its_return_closes_it() {
    let mut storage = storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    let ready = [data(1, 0)];
    let (_, held) = policy
        .choose(ready.iter().copied(), &mut ())
        .unwrap()
        .unwrap();
    policy.accepted(held, 1).unwrap();
    policy.retire(1).unwrap();
    assert_eq!(policy.balance_micros(1), None, "the account ended with it");
    assert_eq!((policy.unsettled(), policy.in_flight()), (0, 1));
    let done = Exchange {
        ticket: 1,
        charged: Some(micros(COST)),
        rate: None,
    };
    policy.notice(RadioNotice::Ended(done)).unwrap();
    assert_eq!(policy.in_flight(), 0, "its return closed the ticket");
}

#[test]
fn a_retired_peer_leaves_room_for_the_next_peers_turns() {
    let mut storage = AirtimeStorage::<u8, 1, 2>::new(micros(QUANTUM));
    let mut policy = AirtimeEgress::<'_, Air, u8, 1, 2>::new(&mut storage, Air::default());
    let go = |policy: &mut AirtimeEgress<'_, Air, u8, 1, 2>, ready: &[Work], ticket| {
        let (work, held) = policy
            .choose(ready.iter().copied(), &mut ())
            .unwrap()
            .unwrap();
        policy.accepted(held, ticket).unwrap();
        let done = Exchange {
            ticket,
            charged: Some(micros(COST)),
            rate: None,
        };
        policy.notice(RadioNotice::Ended(done)).unwrap();
        work.0.flow.unwrap()
    };
    go(&mut policy, &[data(1, 5)], 0);
    policy.retire(1).unwrap();
    let ready = [data(2, 0), data(2, 1)];
    let flows: Vec<_> = (1..5)
        .map(|round| go(&mut policy, &ready, place(round)))
        .collect();
    assert_eq!(flows, [0, 1, 0, 1]);
}

#[test]
fn an_unknown_cost_buys_no_air_when_the_estimate_is_low() {
    let mut storage = storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    let ready = [data(1, 0), data(2, 1)];
    // Each exchange really costs its ceiling, four times the estimate.
    let mut actual = [0_u32; 3];
    for round in 0..1000 {
        let ticket = place(round);
        let (work, held) = policy
            .choose(ready.iter().copied(), &mut ())
            .unwrap()
            .unwrap();
        let peer = work.0.peer[5];
        actual[usize::from(peer)] += CEILING;
        policy.accepted(held, ticket).unwrap();
        let done = Exchange {
            ticket,
            charged: (peer == 2).then(|| micros(CEILING)),
            rate: None,
        };
        policy.notice(RadioNotice::Ended(done)).unwrap();
    }
    assert!(actual[1] <= actual[2] + 2 * CEILING, "{actual:?}");
}

#[test]
fn a_peer_that_left_frees_its_account() {
    let mut storage = AirtimeStorage::<u8, 1, 2>::new(micros(QUANTUM));
    let mut policy = AirtimeEgress::<'_, Air, u8, 1, 2>::new(&mut storage, Air::default());
    let go = |policy: &mut AirtimeEgress<'_, Air, u8, 1, 2>, peer, ticket| {
        let ready = [data(peer, 0)];
        let (_, held) = policy
            .choose(ready.iter().copied(), &mut ())
            .unwrap()
            .unwrap();
        policy.accepted(held, ticket).unwrap();
    };
    go(&mut policy, 1, 0);
    // Its frame is in flight when it leaves: the frame settles at once and
    // the account goes, while the buffer is still the port's.
    policy.notice(RadioNotice::Left(1)).unwrap();
    assert_eq!(policy.balance_micros(1), None);
    go(&mut policy, 2, 1);
    let ended = Exchange {
        ticket: 0,
        charged: Some(micros(CEILING)),
        rate: None,
    };
    policy.notice(RadioNotice::Ended(ended)).unwrap();
    assert_eq!(
        policy.in_flight(),
        1,
        "the next peer has the account's place"
    );
}

/// Five associations in turn, each leaving a frame whose completion is
/// late: every association has its account, though only one fits beside
/// the next, since each departure settles its frames and frees it.
#[test]
fn late_completions_across_many_associations_never_exhaust_the_accounts() {
    const RESERVATIONS: usize = 8;
    let mut storage = AirtimeStorage::<u8, 2, RESERVATIONS>::new(micros(QUANTUM));
    let mut policy =
        AirtimeEgress::<'_, Air, u8, 2, RESERVATIONS>::new(&mut storage, Air::default());
    for peer in 1..=5_u8 {
        let ready = [data(peer, 0)];
        let (_, held) = policy
            .choose(ready.iter().copied(), &mut ())
            .unwrap()
            .unwrap();
        policy.accepted(held, peer).unwrap();
        policy.notice(RadioNotice::Left(peer)).unwrap();
    }
    assert_eq!(policy.unsettled(), 0);
    assert_eq!(policy.in_flight(), 5, "every buffer is still out");
    // The late returns close their tickets, with no account to charge.
    for ticket in 1..=5 {
        let done = Exchange {
            ticket,
            charged: Some(micros(COST)),
            rate: None,
        };
        policy.notice(RadioNotice::Ended(done)).unwrap();
    }
    assert_eq!(policy.in_flight(), 0);
}

/// A frame of a departed association comes back after the next one began:
/// its return closes its ticket, charges neither account and is no stray.
#[test]
fn a_late_return_after_its_account_closed_charges_nothing_twice() {
    let mut storage = storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    let (_, held) = policy
        .choose([data(1, 0)].into_iter(), &mut ())
        .unwrap()
        .unwrap();
    policy.accepted(held, 0).unwrap();
    policy.notice(RadioNotice::Left(1)).unwrap();
    let (_, held) = policy
        .choose([data(2, 0)].into_iter(), &mut ())
        .unwrap()
        .unwrap();
    policy.accepted(held, 1).unwrap();
    let new_account = policy.balance_micros(2);
    let late = Exchange {
        ticket: 0,
        charged: Some(micros(COST)),
        rate: None,
    };
    policy.notice(RadioNotice::Ended(late)).unwrap();
    assert_eq!(policy.balance_micros(1), None, "no account comes back");
    assert_eq!(
        policy.balance_micros(2),
        new_account,
        "the new one untouched"
    );
    assert_eq!(
        policy.notice(RadioNotice::Ended(late)),
        Err(AirtimeFault::UnknownTicket),
        "closed once"
    );
    assert_eq!((policy.unsettled(), policy.in_flight()), (1, 1));
}

/// An [`Air`] that counts the candidates it was asked for.
#[derive(Default)]
struct Counting {
    asked: core::cell::Cell<usize>,
}

impl PeerAir for Counting {
    type Key = u8;

    fn candidate(&self, ready: &Ready) -> Option<AirtimeCandidate<u8>> {
        self.asked.set(self.asked.get() + 1);
        Some(AirtimeCandidate {
            key: ready.peer[5],
            minimum_micros: micros(COST),
        })
    }

    fn ceiling(&self, _ready: &Ready) -> NonZeroU32 {
        micros(CEILING)
    }

    fn reported(&mut self, _key: u8, _rate: PhyRate) {}
}

/// A ready list that counts the works it yields, over every pass.
#[derive(Clone)]
struct Yielded<'r> {
    works: core::slice::Iter<'r, Work>,
    yielded: &'r core::cell::Cell<usize>,
}

impl Iterator for Yielded<'_> {
    type Item = Work;

    fn next(&mut self) -> Option<Work> {
        let work = self.works.next().copied()?;
        self.yielded.set(self.yielded.get() + 1);
        Some(work)
    }
}

/// Each choice reads each ready work once and asks its candidate once,
/// however long the list: what is ready is the network's whole backlog, and
/// a second pass over it doubles each frame's cost.
#[test]
fn a_choice_reads_each_ready_work_once_and_asks_its_candidate_once() {
    let mut storage = AirtimeStorage::<u8, 4, 64>::new(micros(QUANTUM));
    let mut policy =
        AirtimeEgress::<'_, Counting, u8, 4, 64>::new(&mut storage, Counting::default());
    let ready: Vec<Work> = (0..32).map(|flow| data(1, flow)).collect();
    let yielded = core::cell::Cell::new(0);
    for ticket in 0..8 {
        yielded.set(0);
        policy.air().asked.set(0);
        let works = Yielded {
            works: ready.iter(),
            yielded: &yielded,
        };
        let (_, held) = policy.choose(works, &mut ()).unwrap().unwrap();
        policy.accepted(held, ticket).unwrap();
        assert_eq!(yielded.get(), ready.len(), "one pass over the ready work");
        assert_eq!(policy.air().asked.get(), ready.len(), "one candidate each");
    }
}

/// The single pass keeps the choice: a peer's flows still take turns, and
/// control still goes first.
#[test]
fn a_single_pass_serves_the_same_turns_and_control_first() {
    let mut storage = storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    let ready = [data(1, 0), data(1, 1), data(1, 2), control(1), data(2, 0)];
    let first = exchange(&mut policy, &ready, 0, COST);
    assert_eq!(first, control(1), "control first");
    let ready = [data(1, 0), data(1, 1), data(1, 2)];
    let flows: Vec<u8> = (1..7)
        .map(|round| {
            exchange(&mut policy, &ready, place(round), COST)
                .0
                .flow
                .unwrap()
        })
        .collect();
    assert_eq!(flows, [0, 1, 2, 0, 1, 2]);
}

/// More accounts ready than the policy keeps: the scheduler's own answer,
/// as before the single pass.
#[test]
fn more_peers_ready_than_accounts_is_the_scheduler_s_refusal() {
    let mut storage = storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    let ready: Vec<Work> = (1..=5).map(|peer| data(peer, 0)).collect();
    assert!(matches!(
        policy.choose(ready.iter().copied(), &mut ()),
        Err(AirtimeFault::Airtime(
            oer_protocols_ieee80211_core_upper_mac::airtime::AirtimeError::AccountCapacity
        ))
    ));
}

fn rate(mcs: u8) -> PhyRate {
    use oer_protocols_ieee80211_wire::phy::{HtMcs, HtRate, PpduBandwidth};
    PhyRate::Ht(HtRate::new(HtMcs::new(mcs).unwrap(), PpduBandwidth::Mhz20, false).unwrap())
}

/// A settled frame's rate reaches the air under its account's key, after
/// its settlement; a loss, an end without a rate and a frame whose
/// association left say nothing.
#[test]
fn a_settled_frame_reports_its_rate_under_its_account() {
    let mut storage = storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    let send = |policy: &mut Policy<'_>, peer: u8, ticket: u8| {
        let (_, held) = policy
            .choose([data(peer, 0)].into_iter(), &mut ())
            .unwrap()
            .unwrap();
        policy.accepted(held, ticket).unwrap();
    };
    send(&mut policy, 1, 0);
    policy
        .notice(RadioNotice::Ended(Exchange {
            ticket: 0,
            charged: Some(micros(COST)),
            rate: Some(rate(7)),
        }))
        .unwrap();
    send(&mut policy, 2, 1);
    policy
        .notice(RadioNotice::Ended(Exchange {
            ticket: 1,
            charged: Some(micros(COST)),
            rate: None,
        }))
        .unwrap();
    send(&mut policy, 2, 0);
    policy.notice(RadioNotice::Lost(0)).unwrap();
    send(&mut policy, 2, 1);
    policy.notice(RadioNotice::Left(2)).unwrap();
    policy
        .notice(RadioNotice::Ended(Exchange {
            ticket: 1,
            charged: Some(micros(COST)),
            rate: Some(rate(1)),
        }))
        .unwrap();
    assert_eq!(policy.air().reports, [(1, rate(7))]);
}

/// A ticket that counts how often the policy reads its place and compares
/// it: what a frame's acceptance and end cost, whatever else flies.
#[derive(Clone, Copy)]
struct Probe<'c> {
    place: u8,
    reads: &'c core::cell::Cell<usize>,
}

impl From<Probe<'_>> for usize {
    fn from(probe: Probe<'_>) -> usize {
        probe.reads.set(probe.reads.get() + 1);
        usize::from(probe.place)
    }
}

impl PartialEq for Probe<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.reads.set(self.reads.get() + 1);
        self.place == other.place
    }
}

impl Eq for Probe<'_> {}

/// With every place of the station's 63 in flight, ending them in another
/// order than they went: each acceptance and each end reads its ticket's
/// place once and compares no other ticket.
#[test]
fn a_frame_reads_its_own_place_once_at_acceptance_and_once_at_its_end() {
    const PLACES: usize = 63;
    let mut storage = AirtimeStorage::<u8, 1, PLACES>::new(micros(QUANTUM));
    let mut policy =
        AirtimeEgress::<'_, Air, Probe<'_>, 1, PLACES>::new(&mut storage, Air::default());
    let reads = core::cell::Cell::new(0);
    let ticket = |place: usize| Probe {
        place: u8::try_from(place).unwrap(),
        reads: &reads,
    };
    let ready = [data(1, 0)];
    for place in 0..PLACES {
        let (_, held) = policy
            .choose(ready.iter().copied(), &mut ())
            .unwrap()
            .unwrap();
        reads.set(0);
        policy.accepted(held, ticket(place)).unwrap();
        assert_eq!(reads.get(), 1, "acceptance reads one place");
    }
    assert_eq!(policy.in_flight(), PLACES);
    // The last first, then every other one down, then the rest.
    let order = (0..PLACES)
        .rev()
        .step_by(2)
        .chain((0..PLACES).rev().skip(1).step_by(2));
    let mut ended = 0;
    for place in order {
        reads.set(0);
        let done = Exchange {
            ticket: ticket(place),
            charged: Some(micros(COST)),
            rate: None,
        };
        policy.notice(RadioNotice::Ended(done)).unwrap();
        assert_eq!(reads.get(), 1, "an end reads one place");
        ended += 1;
    }
    assert_eq!(ended, PLACES);
    assert_eq!(policy.in_flight(), 0);
}

/// A port that gives a ticket again before its frame's notice, or one past
/// the places, breaks the policy's contract: a fault, not a second frame at
/// one place.
#[test]
fn a_ticket_given_twice_or_past_the_places_is_a_fault() {
    let mut storage = storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    let ready = [data(1, 0)];
    let (_, held) = policy
        .choose(ready.iter().copied(), &mut ())
        .unwrap()
        .unwrap();
    policy.accepted(held, 1).unwrap();
    let (_, held) = policy
        .choose(ready.iter().copied(), &mut ())
        .unwrap()
        .unwrap();
    assert_eq!(policy.accepted(held, 1), Err(AirtimeFault::TicketInUse));
    assert_eq!(policy.in_flight(), 1, "the first frame keeps its place");
    // The refused place's frame was accepted and settled at once: its
    // reservation is free again, the first frame's end still settles, and
    // with both ended the association retires whole.
    let (_, again) = policy
        .choose(ready.iter().copied(), &mut ())
        .unwrap()
        .unwrap();
    policy.refused(again).unwrap();
    let first = Exchange {
        ticket: 1,
        charged: Some(micros(COST)),
        rate: None,
    };
    policy.notice(RadioNotice::Ended(first)).unwrap();
    assert_eq!((policy.in_flight(), policy.unsettled()), (0, 0));
    policy.retire(1).unwrap();
    assert_eq!(policy.balance_micros(1), None, "no reservation pins it");
    let mut storage = self::storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    let (_, held) = policy
        .choose(ready.iter().copied(), &mut ())
        .unwrap()
        .unwrap();
    assert_eq!(policy.accepted(held, 2), Err(AirtimeFault::NoPlace));
    assert_eq!(
        policy.balance_micros(1),
        Some(i64::from(QUANTUM) - i64::from(CEILING)),
        "settled at its ceiling, not refunded as unsent"
    );
    // Both reservations are free: two more frames fit.
    for ticket in 0..2 {
        let (_, held) = policy
            .choose(ready.iter().copied(), &mut ())
            .unwrap()
            .unwrap();
        policy.accepted(held, ticket).unwrap();
    }
}

#[test]
fn reviewer_ticket_collision_does_not_orphan_the_accepted_reservation() {
    let mut storage = storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    let ready = [data(1, 0)];
    let (_, first) = policy.choose(ready.into_iter(), &mut ()).unwrap().unwrap();
    policy.accepted(first, 1).unwrap();
    let (_, second) = policy.choose(ready.into_iter(), &mut ()).unwrap().unwrap();
    assert_eq!(policy.accepted(second, 1), Err(AirtimeFault::TicketInUse));
    // Complete the first physical return after its association leaves.
    policy.notice(RadioNotice::Left(1)).unwrap();
    policy
        .notice(RadioNotice::Ended(Exchange {
            ticket: 1,
            charged: Some(micros(COST)),
            rate: None,
        }))
        .unwrap();
    assert_eq!(policy.in_flight(), 0);
    assert_eq!(policy.unsettled(), 0);
    assert_eq!(
        policy.balance_micros(1),
        None,
        "the fault must not strand the second accepted frame's reservation"
    );
}

#[test]
fn reviewer_an_out_of_range_ticket_does_not_orphan_the_accepted_reservation() {
    let mut storage = storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    let (_, held) = policy
        .choose([data(1, 0)].into_iter(), &mut ())
        .unwrap()
        .unwrap();
    assert_eq!(policy.accepted(held, 2), Err(AirtimeFault::NoPlace));
    policy.notice(RadioNotice::Left(1)).unwrap();
    assert_eq!(policy.in_flight(), 0);
    assert_eq!(policy.unsettled(), 0);
    assert_eq!(
        policy.balance_micros(1),
        None,
        "the fault must not strand the accepted frame's reservation"
    );
}

/// Every event a choice reported, in order.
#[derive(Default)]
struct Weighing(Vec<oer_protocols_network_port::EgressEvent>);

impl oer_portable_events::EventSink<oer_protocols_network_port::EgressEvent> for Weighing {
    fn event(&mut self, event: oer_protocols_network_port::EgressEvent) {
        self.0.push(event);
    }
}

/// A choice tells the candidates it read and the accounts they fall in,
/// once: works whose peer may not go are no candidates, and a peer's flows
/// share its account.
#[test]
fn a_choice_reports_the_candidates_and_the_accounts_it_weighed() {
    use oer_protocols_network_port::EgressEvent;
    let mut storage = storage();
    let air = Air {
        asleep: std::vec![3],
        ..Air::default()
    };
    let mut policy = Policy::new(&mut storage, air);
    let ready = [data(1, 0), data(1, 1), data(2, 0), control(3)];
    let mut seen = Weighing::default();
    let chosen = policy.choose(ready.iter().copied(), &mut seen).unwrap();
    assert!(chosen.is_some());
    assert_eq!(
        seen.0,
        [EgressEvent::Weighed {
            candidates: 3,
            accounts: 2,
            widened: false,
        }]
    );
    // Nothing ready: nothing weighed, and nothing withheld.
    let mut seen = Weighing::default();
    let none: [Work; 0] = [];
    assert!(
        policy
            .choose(none.into_iter(), &mut seen)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        seen.0,
        [EgressEvent::Weighed {
            candidates: 0,
            accounts: 0,
            widened: false,
        }]
    );
}

/// With every reservation out, a choice with work that could go withholds
/// it for capacity and says so; a frame's end makes room again.
#[test]
fn a_choice_with_every_reservation_out_says_it_withheld_for_capacity() {
    use oer_protocols_network_port::{EgressEvent, Withheld};
    let mut storage = storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    let ready = [data(1, 0), data(2, 0)];
    for ticket in 0..2 {
        let (_, held) = policy
            .choose(ready.iter().copied(), &mut ())
            .unwrap()
            .unwrap();
        policy.accepted(held, ticket).unwrap();
    }
    let mut seen = Weighing::default();
    assert!(
        policy
            .choose(ready.iter().copied(), &mut seen)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        seen.0,
        [
            EgressEvent::Weighed {
                candidates: 2,
                accounts: 2,
                widened: false,
            },
            EgressEvent::Withheld(Withheld::Capacity),
        ]
    );
    let done = Exchange {
        ticket: 0,
        charged: Some(micros(COST)),
        rate: None,
    };
    policy.notice(RadioNotice::Ended(done)).unwrap();
    let mut seen = Weighing::default();
    assert!(
        policy
            .choose(ready.iter().copied(), &mut seen)
            .unwrap()
            .is_some()
    );
    assert!(
        !seen
            .0
            .iter()
            .any(|event| matches!(event, EgressEvent::Withheld(_))),
        "{:?}",
        seen.0
    );
}

/// More accounts than one pass keeps widen the choice: it reads the whole
/// list once more for the scheduler, whose refusal of so many accounts ends
/// it, and the candidates it reports are both passes'.
#[test]
fn a_widened_choice_counts_the_candidates_of_every_pass() {
    use oer_protocols_ieee80211_core_upper_mac::airtime::AirtimeError;
    use oer_protocols_network_port::EgressEvent;
    let mut storage = storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    let ready = [data(1, 0), data(2, 0), data(3, 0), data(4, 0), data(5, 0)];
    let mut seen = Weighing::default();
    assert_eq!(
        policy.choose(ready.iter().copied(), &mut seen).err(),
        Some(AirtimeFault::Airtime(AirtimeError::AccountCapacity))
    );
    // Five in the first pass (the fifth overflows), five for the scheduler.
    assert_eq!(
        seen.0,
        [EgressEvent::Weighed {
            candidates: 10,
            accounts: 4,
            widened: true,
        }]
    );
}

/// What a measuring sink hears changes no choice: the same works in the
/// same order with the unit sink and with one that records.
#[test]
fn a_recording_sink_changes_no_choice() {
    let ready = [data(1, 0), data(2, 1), data(2, 2), control(1), data(3, 0)];
    let picks = |record: bool| {
        let mut storage = storage();
        let mut policy = Policy::new(&mut storage, Air::default());
        (0..24)
            .map(|round| {
                let mut seen = Weighing::default();
                let (work, held) = if record {
                    policy.choose(ready.iter().copied(), &mut seen)
                } else {
                    policy.choose(ready.iter().copied(), &mut ())
                }
                .unwrap()
                .unwrap();
                let ticket = place(round);
                policy.accepted(held, ticket).unwrap();
                let done = Exchange {
                    ticket,
                    charged: Some(micros(COST + 100 * round)),
                    rate: None,
                };
                policy.notice(RadioNotice::Ended(done)).unwrap();
                work
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(picks(true), picks(false));
}
