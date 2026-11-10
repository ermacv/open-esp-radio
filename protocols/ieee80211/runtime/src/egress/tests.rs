extern crate std;

use std::vec::Vec;

use core::num::NonZeroU32;

use oer_protocols_ieee80211_contracts_port::PhyRate;
use oer_protocols_ieee80211_core_upper_mac::airtime::{AirtimeCandidate, AirtimeStorage};
use oer_protocols_network_port::{Candidate, Class, Egress, Next, PlanStop, Ready};

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
    /// The sleepers: all a candidate reads besides the work.
    type View = u64;

    fn view(&self) -> u64 {
        self.asleep
            .iter()
            .fold(0, |view, peer| view | 1 << (peer % 64))
    }

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
    let (work, held) = policy.first_choice(ready.iter().copied()).unwrap().unwrap();
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
        let (work, held) = policy.first_choice(ready.iter().copied()).unwrap().unwrap();
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
    let chosen = policy.first_choice([data(1, 0)].into_iter()).unwrap();
    assert!(chosen.is_none());
}

#[test]
fn a_full_horizon_waits_for_a_completion() {
    let mut storage = storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    let ready = [data(1, 0)];
    for ticket in 0..2 {
        let (_, held) = policy.first_choice(ready.iter().copied()).unwrap().unwrap();
        policy.accepted(held, ticket).unwrap();
    }
    assert!(
        policy
            .first_choice(ready.iter().copied())
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
            .first_choice(ready.iter().copied())
            .unwrap()
            .is_some()
    );
}

#[test]
fn a_refusal_returns_the_whole_reservation() {
    let mut storage = storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    let ready = [data(1, 0)];
    let (_, held) = policy.first_choice(ready.iter().copied()).unwrap().unwrap();
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
    let (_, held) = policy.first_choice(ready.iter().copied()).unwrap().unwrap();
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
    let (_, held) = policy.first_choice(ready.iter().copied()).unwrap().unwrap();
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
    let (_, held) = policy.first_choice(ready.iter().copied()).unwrap().unwrap();
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
    let (_, held) = policy.first_choice(ready.iter().copied()).unwrap().unwrap();
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
        let (work, held) = policy.first_choice(ready.iter().copied()).unwrap().unwrap();
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
        let (work, held) = policy.first_choice(ready.iter().copied()).unwrap().unwrap();
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
        let (_, held) = policy.first_choice(ready.iter().copied()).unwrap().unwrap();
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
        let (_, held) = policy.first_choice(ready.iter().copied()).unwrap().unwrap();
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
        .first_choice([data(1, 0)].into_iter())
        .unwrap()
        .unwrap();
    policy.accepted(held, 0).unwrap();
    policy.notice(RadioNotice::Left(1)).unwrap();
    let (_, held) = policy
        .first_choice([data(2, 0)].into_iter())
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
    type View = ();

    fn view(&self) {}

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
        let (_, held) = policy.first_choice(works).unwrap().unwrap();
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
        policy.first_choice(ready.iter().copied()),
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
            .first_choice([data(peer, 0)].into_iter())
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
        let (_, held) = policy.first_choice(ready.iter().copied()).unwrap().unwrap();
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
    let (_, held) = policy.first_choice(ready.iter().copied()).unwrap().unwrap();
    policy.accepted(held, 1).unwrap();
    let (_, held) = policy.first_choice(ready.iter().copied()).unwrap().unwrap();
    assert_eq!(policy.accepted(held, 1), Err(AirtimeFault::TicketInUse));
    assert_eq!(policy.in_flight(), 1, "the first frame keeps its place");
    // The refused place's frame was accepted and settled at once: its
    // reservation is free again, the first frame's end still settles, and
    // with both ended the association retires whole.
    let (_, again) = policy.first_choice(ready.iter().copied()).unwrap().unwrap();
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
    let (_, held) = policy.first_choice(ready.iter().copied()).unwrap().unwrap();
    assert_eq!(policy.accepted(held, 2), Err(AirtimeFault::NoPlace));
    assert_eq!(
        policy.balance_micros(1),
        Some(i64::from(QUANTUM) - i64::from(CEILING)),
        "settled at its ceiling, not refunded as unsent"
    );
    // Both reservations are free: two more frames fit.
    for ticket in 0..2 {
        let (_, held) = policy.first_choice(ready.iter().copied()).unwrap().unwrap();
        policy.accepted(held, ticket).unwrap();
    }
}

#[test]
fn reviewer_ticket_collision_does_not_orphan_the_accepted_reservation() {
    let mut storage = storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    let ready = [data(1, 0)];
    let (_, first) = policy.first_choice(ready.into_iter()).unwrap().unwrap();
    policy.accepted(first, 1).unwrap();
    let (_, second) = policy.first_choice(ready.into_iter()).unwrap().unwrap();
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
        .first_choice([data(1, 0)].into_iter())
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

/// Peers are their last octet and a work's least grant its length; a work
/// is told apart by its second octet. `view` moves when a test says.
#[derive(Default)]
struct Lengths {
    view: u64,
}

impl PeerAir for Lengths {
    type Key = u8;
    type View = u64;

    fn view(&self) -> u64 {
        self.view
    }

    fn candidate(&self, ready: &Ready) -> Option<AirtimeCandidate<u8>> {
        Some(AirtimeCandidate {
            key: ready.peer[5],
            minimum_micros: micros(u32::try_from(ready.length).unwrap()),
        })
    }

    fn ceiling(&self, ready: &Ready) -> NonZeroU32 {
        micros(4 * u32::try_from(ready.length).unwrap())
    }

    fn reported(&mut self, _key: u8, _rate: PhyRate) {}
}

type Long<'o> = AirtimeEgress<'o, Lengths, u8, 4, 64>;

/// Why a plan's step stopped, if it did.
fn stop_of<C, R>(next: Next<C, R>) -> Option<PlanStop> {
    match next {
        Next::Stop(stop) => Some(stop),
        Next::Send(..) | Next::Idle => None,
    }
}

fn long_storage() -> AirtimeStorage<u8, 4, 64> {
    AirtimeStorage::new(micros(QUANTUM))
}

/// Work `id` of `class` to `peer`, of `flow`, whose least grant is `length`.
fn sized(id: u8, class: Class, peer: u8, flow: Option<u8>, length: usize) -> Work {
    Work(Ready {
        class,
        length,
        peer: [2, id, 0, 0, 0, peer],
        flow,
        dscp: Default::default(),
    })
}

/// A ready list fixed by `seed`: control and data to three peers, a few flows
/// each, and least grants of three sizes.
fn backlog(seed: u64, length: u8) -> Vec<Work> {
    let mut state = seed;
    (0..length)
        .map(|id| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let pick = (state >> 33) as u8;
            let peer = 1 + pick % 3;
            let size = 100 * (1 + usize::from((pick >> 2) % 3));
            if pick.is_multiple_of(11) {
                sized(id, Class::Control, peer, None, 42)
            } else {
                sized(id, Class::Data, peer, Some((pick >> 4) % 4), size)
            }
        })
        .collect()
}

/// What one frame of a run was: its work, its account and its grant.
type Sent = (u8, u8, u32);

fn take(ready: &mut Vec<Work>, work: Work) {
    let at = ready.iter().position(|left| *left == work).unwrap();
    ready.remove(at);
}

/// Up to `frames` frames, each chosen by a fresh plan's first step over what
/// is left: a choice per frame. Tickets start at 1.
fn by_choice(policy: &mut Long<'_>, mut ready: Vec<Work>, frames: usize) -> Vec<Sent> {
    let mut sent = Vec::new();
    while sent.len() < frames {
        let Some((work, held)) = policy.first_choice(ready.iter().copied()).unwrap() else {
            break;
        };
        let budget = held.reservation.budget_micros().get();
        sent.push((work.0.peer[1], held.reservation.key(), budget));
        policy
            .accepted(held, u8::try_from(sent.len()).unwrap())
            .unwrap();
        take(&mut ready, work);
    }
    sent
}

/// Up to `frames` frames from plans continued while they go on, a new one
/// collected at each stop; the stops, and the collections.
fn by_plan(
    policy: &mut Long<'_>,
    mut ready: Vec<Work>,
    frames: usize,
) -> (Vec<Sent>, Vec<PlanStop>, usize) {
    let mut sent = Vec::new();
    let mut stops = Vec::new();
    let mut collections = 0;
    let mut plan = None;
    while sent.len() < frames {
        let current = match &mut plan {
            Some(current) => current,
            None => {
                collections += 1;
                plan.insert(policy.plan(ready.clone().into_iter()).unwrap())
            }
        };
        match policy.next(current).unwrap() {
            Next::Send(work, held) => {
                let budget = held.reservation.budget_micros().get();
                sent.push((work.0.peer[1], held.reservation.key(), budget));
                policy
                    .accepted(held, u8::try_from(sent.len()).unwrap())
                    .unwrap();
                take(&mut ready, work);
            }
            Next::Idle => break,
            Next::Stop(stop) => {
                stops.push(stop);
                plan = None;
            }
        }
    }
    (sent, stops, collections)
}

/// A plan continued frame by frame sends what a choice per frame sends, with
/// the same account and grant each, and leaves every account's balance as
/// that would: peers by their deficits, control first, a peer's flows in
/// turn, over any backlog. It stops where it cannot know: another account's
/// turn, or its account's largest least grant gone.
#[test]
fn a_continued_plan_sends_what_a_choice_per_frame_sends() {
    let mut continued = 0;
    let mut all_stops = Vec::new();
    for seed in 0..200 {
        let ready = backlog(seed, 24);
        let mut per_frame = long_storage();
        let mut planned = long_storage();
        let mut per_frame = Long::new(&mut per_frame, Lengths::default());
        let mut planned = Long::new(&mut planned, Lengths::default());
        let expected = by_choice(&mut per_frame, ready.clone(), 20);
        let (sent, stops, collections) = by_plan(&mut planned, ready, 20);
        assert_eq!(sent, expected, "seed {seed}: {stops:?}");
        for peer in 1..=3 {
            assert_eq!(
                planned.balance_micros(peer),
                per_frame.balance_micros(peer),
                "seed {seed}"
            );
        }
        continued += sent.len() - collections;
        all_stops.extend(stops);
    }
    assert!(continued > 0, "plans went on past their first frame");
    for stop in [PlanStop::Account, PlanStop::Grant] {
        assert!(all_stops.contains(&stop), "{stop:?} met");
    }
}

/// One peer, flows in turn and control first, at one size: one plan sends a
/// whole quantum, as a choice per frame would.
#[test]
fn one_account_sends_a_quantum_from_one_collection() {
    let ready: Vec<Work> = (0..12)
        .map(|id| match id {
            3 => sized(id, Class::Control, 1, None, 300),
            _ => sized(id, Class::Data, 1, Some(id % 3), 300),
        })
        .collect();
    let mut per_frame = long_storage();
    let mut planned = long_storage();
    let mut per_frame = Long::new(&mut per_frame, Lengths::default());
    let mut planned = Long::new(&mut planned, Lengths::default());
    let expected = by_choice(&mut per_frame, ready.clone(), 8);
    let (sent, stops, collections) = by_plan(&mut planned, ready, 8);
    assert_eq!(sent, expected);
    assert_eq!(sent[0].0, 3, "control first");
    assert_eq!((collections, stops), (1, Vec::new()));
}

/// The port refuses the plan's second frame: its reservation comes back
/// whole, as a choice per frame's would, and dropping the plan returns
/// nothing and costs nothing, as it reserved nothing more.
#[test]
fn a_refusal_in_a_plan_costs_what_it_costs_a_choice_per_frame() {
    let ready: Vec<Work> = (0..6)
        .map(|id| sized(id, Class::Data, 1, Some(id), 300))
        .collect();
    let mut per_frame = long_storage();
    let mut planned = long_storage();
    let mut per_frame = Long::new(&mut per_frame, Lengths::default());
    let mut planned = Long::new(&mut planned, Lengths::default());
    // A choice per frame: the first taken, the second refused.
    let (first, held) = per_frame
        .first_choice(ready.iter().copied())
        .unwrap()
        .unwrap();
    per_frame.accepted(held, 0).unwrap();
    let mut left = ready.clone();
    take(&mut left, first);
    let (_, held) = per_frame
        .first_choice(left.iter().copied())
        .unwrap()
        .unwrap();
    per_frame.refused(held).unwrap();
    // One plan: the same two answers, then dropped.
    let mut plan = planned.plan(ready.iter().copied()).unwrap();
    let Next::Send(work, held) = planned.next(&mut plan).unwrap() else {
        panic!("the plan sends");
    };
    assert_eq!(work, first);
    planned.accepted(held, 0).unwrap();
    let Next::Send(_, held) = planned.next(&mut plan).unwrap() else {
        panic!("the plan goes on");
    };
    planned.refused(held).unwrap();
    drop(plan);
    assert_eq!(planned.balance_micros(1), per_frame.balance_micros(1));
    assert_eq!(
        (planned.unsettled(), planned.in_flight()),
        (1, 1),
        "the accepted frame alone is held"
    );
    // And what comes next is the same either way.
    assert_eq!(
        by_choice(&mut planned, left.clone(), 3),
        by_choice(&mut per_frame, left, 3)
    );
}

/// A plan stops, reserving nothing, when the air's view moves under it.
#[test]
fn a_plan_stops_when_the_air_moves() {
    let ready: Vec<Work> = (0..4)
        .map(|id| sized(id, Class::Data, 1, Some(id), 300))
        .collect();
    let mut storage = long_storage();
    let mut policy = Long::new(&mut storage, Lengths::default());
    let mut plan = policy.plan(ready.into_iter()).unwrap();
    let Next::Send(_, held) = policy.next(&mut plan).unwrap() else {
        panic!("the plan sends");
    };
    policy.accepted(held, 0).unwrap();
    let balance = policy.balance_micros(1);
    policy.air_mut().view += 1;
    assert_eq!(
        stop_of(policy.next(&mut plan).unwrap()),
        Some(PlanStop::Changed)
    );
    assert_eq!(policy.balance_micros(1), balance, "nothing reserved");
    assert_eq!(policy.unsettled(), 1);
}

/// The account's one work of the largest least grant goes: the next
/// largest is not in the plan's count, so it stops, reserving nothing.
#[test]
fn a_plan_stops_when_its_account_s_largest_grant_goes() {
    let ready = [
        sized(0, Class::Data, 1, Some(0), 300),
        sized(1, Class::Data, 1, Some(1), 100),
        sized(2, Class::Data, 1, Some(2), 200),
    ];
    let mut storage = long_storage();
    let mut policy = Long::new(&mut storage, Lengths::default());
    let mut plan = policy.plan(ready.into_iter()).unwrap();
    let Next::Send(work, held) = policy.next(&mut plan).unwrap() else {
        panic!("the plan sends");
    };
    assert_eq!(work, ready[0], "flow 0 first");
    policy.accepted(held, 0).unwrap();
    let balance = policy.balance_micros(1);
    assert_eq!(
        stop_of(policy.next(&mut plan).unwrap()),
        Some(PlanStop::Grant)
    );
    assert_eq!(policy.balance_micros(1), balance, "nothing reserved");
}

/// A plan whose last frame the port did not take goes on no more: the turn
/// of the account's flows is not the one it holds.
#[test]
fn a_plan_whose_frame_was_refused_does_not_go_on() {
    let ready: Vec<Work> = (0..4)
        .map(|id| sized(id, Class::Data, 1, Some(id), 300))
        .collect();
    let mut storage = long_storage();
    let mut policy = Long::new(&mut storage, Lengths::default());
    let mut plan = policy.plan(ready.into_iter()).unwrap();
    let Next::Send(_, held) = policy.next(&mut plan).unwrap() else {
        panic!("the plan sends");
    };
    policy.refused(held).unwrap();
    let balance = policy.balance_micros(1);
    assert_eq!(
        stop_of(policy.next(&mut plan).unwrap()),
        Some(PlanStop::Changed)
    );
    assert_eq!(policy.balance_micros(1), balance);
}

/// Two peers in turn: after one's frame the scheduler serves the other, which
/// the plan does not hold, so it stops with nothing reserved and a new
/// collection serves that peer.
#[test]
fn a_plan_stops_when_the_scheduler_serves_another_account() {
    let ready = [
        sized(0, Class::Data, 1, Some(0), 300),
        sized(1, Class::Data, 2, Some(1), 300),
        sized(2, Class::Data, 1, Some(0), 300),
    ];
    let mut storage = long_storage();
    let mut policy = Long::new(&mut storage, Lengths::default());
    let mut plan = policy.plan(ready.into_iter()).unwrap();
    let Next::Send(work, held) = policy.next(&mut plan).unwrap() else {
        panic!("the plan sends");
    };
    assert_eq!(work, ready[0]);
    policy.accepted(held, 0).unwrap();
    let balances = [policy.balance_micros(1), policy.balance_micros(2)];
    assert_eq!(
        stop_of(policy.next(&mut plan).unwrap()),
        Some(PlanStop::Account)
    );
    assert_eq!(
        [policy.balance_micros(1), policy.balance_micros(2)],
        balances
    );
    let (work, _held) = policy
        .first_choice([ready[1], ready[2]].into_iter())
        .unwrap()
        .unwrap();
    assert_eq!(work, ready[1], "the other peer's turn");
}
