extern crate std;

use std::vec::Vec;

use core::num::NonZeroU32;

use oer_protocols_ieee80211_core_upper_mac::airtime::{AirtimeCandidate, AirtimeStorage};
use oer_protocols_network_port::{Candidate, Class, Egress, Ready};

use super::{AirtimeEgress, AirtimeFault, Exchange, PeerAir};

const QUANTUM: u32 = 1_000;
const COST: u32 = 400;

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
}

type Policy<'o> = AirtimeEgress<'o, Air, u32, 4, 2>;

fn storage() -> AirtimeStorage<u8, 4, 2> {
    AirtimeStorage::new(micros(QUANTUM))
}

/// Choose, publish under `ticket` and complete at `charged`.
fn exchange(policy: &mut Policy<'_>, ready: &[Work], ticket: u32, charged: u32) -> Work {
    let (work, held) = policy.choose(ready.iter().copied()).unwrap().unwrap();
    policy.accepted(held, ticket).unwrap();
    let exchange = Exchange {
        ticket,
        charged: Some(micros(charged)),
    };
    policy.completed(exchange).unwrap();
    work
}

#[test]
fn peers_at_one_cost_take_equal_turns() {
    let mut storage = storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    let ready = [data(1, 0), data(2, 1)];
    let mut served = [0_u32; 3];
    for ticket in 0..40 {
        let work = exchange(&mut policy, &ready, ticket, COST);
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
    for ticket in 0..40 {
        let (work, held) = policy.choose(ready.iter().copied()).unwrap().unwrap();
        let peer = work.0.peer[5];
        let charged = if peer == 2 { 4 * COST } else { COST };
        air[usize::from(peer)] += charged;
        policy.accepted(held, ticket).unwrap();
        let done = Exchange {
            ticket,
            charged: Some(micros(charged)),
        };
        policy.completed(done).unwrap();
    }
    assert!(air[1].abs_diff(air[2]) <= 2 * 4 * COST, "{air:?}");
}

#[test]
fn a_peers_flows_take_turns_within_its_share() {
    let mut storage = storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    let ready = [data(1, 0), data(1, 1), data(1, 2)];
    let flows: Vec<_> = (0..6)
        .map(|ticket| exchange(&mut policy, &ready, ticket, COST).0.flow.unwrap())
        .collect();
    assert_eq!(flows, [0, 1, 2, 0, 1, 2]);
}

#[test]
fn many_flows_of_one_peer_earn_it_no_more_turns() {
    let mut storage = storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    let ready = [data(1, 0), data(1, 1), data(1, 2), data(2, 3)];
    let mut served = [0_u32; 3];
    for ticket in 0..40 {
        let work = exchange(&mut policy, &ready, ticket, COST);
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
    let air = Air { asleep: [2].into() };
    let mut policy = Policy::new(&mut storage, air);
    let ready = [data(1, 0), data(2, 1)];
    for ticket in 0..10 {
        assert_eq!(exchange(&mut policy, &ready, ticket, COST).0.peer[5], 1);
    }
    assert_eq!(
        policy.balance_micros(2),
        None,
        "no credit, not even an account"
    );
    policy.air_mut().asleep.clear();
    assert_eq!(exchange(&mut policy, &ready, 10, COST).0.peer[5], 2);
}

#[test]
fn nothing_ready_chooses_nothing() {
    let mut storage = storage();
    let air = Air { asleep: [1].into() };
    let mut policy = Policy::new(&mut storage, air);
    let chosen = policy.choose([data(1, 0)].into_iter()).unwrap();
    assert!(chosen.is_none());
}

#[test]
fn a_full_horizon_waits_for_a_completion() {
    let mut storage = storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    let ready = [data(1, 0)];
    for ticket in 0..2 {
        let (_, held) = policy.choose(ready.iter().copied()).unwrap().unwrap();
        policy.accepted(held, ticket).unwrap();
    }
    assert!(policy.choose(ready.iter().copied()).unwrap().is_none());
    assert_eq!(policy.in_flight(), 2);
    let done = Exchange {
        ticket: 0,
        charged: Some(micros(COST)),
    };
    policy.completed(done).unwrap();
    assert!(policy.choose(ready.iter().copied()).unwrap().is_some());
}

#[test]
fn a_refusal_returns_the_whole_reservation() {
    let mut storage = storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    let ready = [data(1, 0)];
    let (_, held) = policy.choose(ready.iter().copied()).unwrap().unwrap();
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
    let (_, held) = policy.choose(ready.iter().copied()).unwrap().unwrap();
    policy.accepted(held, 7).unwrap();
    let unknown = Exchange {
        ticket: 7,
        charged: None,
    };
    policy.completed(unknown).unwrap();
    assert_eq!(policy.balance_micros(1), Some(0));
}

#[test]
fn a_frame_of_unknown_end_is_charged_and_not_refunded() {
    let mut storage = storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    let ready = [data(1, 0)];
    let (_, held) = policy.choose(ready.iter().copied()).unwrap().unwrap();
    policy.unknown(held).unwrap();
    assert_eq!(policy.balance_micros(1), Some(0));
    assert_eq!(policy.in_flight(), 0);
}

#[test]
fn a_completion_of_no_frame_is_a_fault() {
    let mut storage = storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    let stray = Exchange {
        ticket: 9,
        charged: None,
    };
    assert_eq!(policy.completed(stray), Err(AirtimeFault::UnknownTicket));
}

#[test]
fn a_retired_peers_frame_settles_on_its_old_account() {
    let mut storage = storage();
    let mut policy = Policy::new(&mut storage, Air::default());
    let ready = [data(1, 0)];
    let (_, held) = policy.choose(ready.iter().copied()).unwrap().unwrap();
    policy.accepted(held, 3).unwrap();
    policy.retire(1);
    let done = Exchange {
        ticket: 3,
        charged: Some(micros(COST)),
    };
    policy.completed(done).unwrap();
    assert_eq!(policy.balance_micros(1), None, "the account ended with it");
}

#[test]
fn a_retired_peer_leaves_room_for_the_next_peers_turns() {
    let mut storage = AirtimeStorage::<u8, 1, 2>::new(micros(QUANTUM));
    let mut policy = AirtimeEgress::<'_, Air, u32, 1, 2>::new(&mut storage, Air::default());
    let mut go = |policy: &mut AirtimeEgress<'_, Air, u32, 1, 2>, ready: &[Work], ticket| {
        let (work, held) = policy.choose(ready.iter().copied()).unwrap().unwrap();
        policy.accepted(held, ticket).unwrap();
        let done = Exchange {
            ticket,
            charged: Some(micros(COST)),
        };
        policy.completed(done).unwrap();
        work.0.flow.unwrap()
    };
    go(&mut policy, &[data(1, 5)], 0);
    policy.retire(1);
    let ready = [data(2, 0), data(2, 1)];
    let flows: Vec<_> = (1..5)
        .map(|ticket| go(&mut policy, &ready, ticket))
        .collect();
    assert_eq!(flows, [0, 1, 0, 1]);
}
