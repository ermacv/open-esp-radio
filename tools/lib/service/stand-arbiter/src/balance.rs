//! Owner balances: who has waited more than they used.
//!
//! Every owner has a balance. Each lease it holds charges it the time held;
//! while a request of it waits because of another owner, behind that
//! owner's lease or a request of that owner served before it, it is credited
//! the time waited, once however many requests it has queued. Waiting behind
//! its own lease earns nothing, so an owner that queues more work behind its
//! running lease is still charged for it. Balances decay with a two-hour
//! half-life and are clamped to an hour either way, so old debt fades. Every
//! settlement subtracts the mean over the owners active in the last day, so
//! the sum stays near zero without a jump at each grant.
//! The grantable request whose owner has the highest balance is served
//! first, the earlier one on a tie; divisible work of an owner with a lower
//! balance than a waiter it blocks yields after a minimum slice.

use std::{collections::BTreeMap, time::Duration};

use serde::{Deserialize, Serialize};

use crate::state::State;
use oer_stand_claims::conflict;

/// Balances halve over this time.
pub const HALF_LIFE: Duration = Duration::from_secs(2 * 3600);
/// Balances stay within this much either way.
pub const CAP: Duration = Duration::from_secs(3600);
/// Owners active within this window share the normalization.
pub const ACTIVE_WINDOW: Duration = Duration::from_secs(24 * 3600);
/// Divisible work runs at least this long before it yields to a waiter.
pub const MIN_SLICE: Duration = Duration::from_secs(10 * 60);
/// No lease runs longer than this; its holder is then terminated.
pub const HARD_LIMIT: Duration = Duration::from_secs(3600);

/// One owner's standing.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Balance {
    /// Positive: waited more than used.
    pub balance_ms: i64,
    pub last_active_unix_ms: u64,
    #[serde(flatten)]
    pub unknown: crate::Unknown,
}

/// The owners with a request that waits because of another owner: a
/// conflicting lease of another owner, or a conflicting request of another
/// owner served before it that itself waits. A request ahead that could be
/// granted now is served within a poll and credits nobody behind it, so the
/// credit cannot reorder the queue.
fn waiting_on_others(state: &State) -> std::collections::BTreeSet<String> {
    state
        .queue
        .iter()
        .filter(|ticket| {
            state.holders.iter().any(|holder| {
                holder.ticket.owner != ticket.owner
                    && conflict(&holder.ticket.claims, &ticket.claims)
            }) || state.queue.iter().any(|other| {
                other.owner != ticket.owner
                    && before(state, other, ticket)
                    && conflict(&other.claims, &ticket.claims)
                    && !crate::queue::grantable(state, other.id)
            })
        })
        .map(|ticket| ticket.owner.clone())
        .collect()
}

/// Advance every balance from the state's last settlement to `now_ms`:
/// decay, then credit owners waiting on others and charge holders, then
/// subtract the mean and clamp.
pub(crate) fn settle(state: &mut State, now_ms: u64) {
    let elapsed = now_ms.saturating_sub(state.settled_unix_ms);
    if state.settled_unix_ms == 0 || elapsed == 0 {
        state.settled_unix_ms = state.settled_unix_ms.max(now_ms);
        return;
    }
    // Who waited is decided by the order the interval began with.
    let waiting = waiting_on_others(state);
    let decay = 0.5_f64.powf(elapsed as f64 / HALF_LIFE.as_millis() as f64);
    for balance in state.balances.values_mut() {
        balance.balance_ms = (balance.balance_ms as f64 * decay).round() as i64;
    }
    let elapsed = elapsed as i64;
    for owner in waiting {
        let balance = state.balances.entry(owner).or_default();
        balance.balance_ms += elapsed;
        balance.last_active_unix_ms = now_ms;
    }
    // A preempted lease is charged only up to its preemption.
    for holder in state
        .holders
        .iter()
        .filter(|holder| holder.preempted.is_none())
    {
        let balance = state
            .balances
            .entry(holder.ticket.owner.clone())
            .or_default();
        balance.balance_ms -= elapsed;
        balance.last_active_unix_ms = now_ms;
    }
    normalize(state, now_ms);
    let cap = CAP.as_millis() as i64;
    for balance in state.balances.values_mut() {
        balance.balance_ms = balance.balance_ms.clamp(-cap, cap);
    }
    state.settled_unix_ms = now_ms;
}

/// Subtract the mean balance of the owners active within the window, and
/// forget the others.
fn normalize(state: &mut State, now_ms: u64) {
    let window = ACTIVE_WINDOW.as_millis() as u64;
    state
        .balances
        .retain(|_, balance| now_ms.saturating_sub(balance.last_active_unix_ms) <= window);
    if state.balances.is_empty() {
        return;
    }
    let mean = state
        .balances
        .values()
        .map(|balance| balance.balance_ms)
        .sum::<i64>()
        / state.balances.len() as i64;
    for balance in state.balances.values_mut() {
        balance.balance_ms -= mean;
    }
}

/// The balance of `owner`; zero for an owner without one.
pub(crate) fn of(balances: &BTreeMap<String, Balance>, owner: &str) -> i64 {
    balances.get(owner).map_or(0, |balance| balance.balance_ms)
}

/// Whether ticket `a` is served before ticket `b`: maintenance before
/// ordinary requests, then the higher balance, then the earlier arrival.
pub(crate) fn before(state: &State, a: &crate::state::Ticket, b: &crate::state::Ticket) -> bool {
    use crate::state::Priority;
    match (a.priority, b.priority) {
        (Priority::Maintenance, Priority::Ordinary) => return true,
        (Priority::Ordinary, Priority::Maintenance) => return false,
        (Priority::Maintenance, Priority::Maintenance) => return a.id < b.id,
        (Priority::Ordinary, Priority::Ordinary) => {}
    }
    let (balance_a, balance_b) = (of(&state.balances, &a.owner), of(&state.balances, &b.owner));
    balance_a > balance_b || (balance_a == balance_b && a.id < b.id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        process::ProcessIdentity,
        state::{Holder, Ticket},
    };
    use oer_stand_claims::Claim;

    const MINUTE: u64 = 60_000;

    fn ticket(id: u64, owner: &str) -> Ticket {
        Ticket {
            id,
            owner: owner.into(),
            work: String::from("work"),
            estimate_secs: 60,
            process: ProcessIdentity::current().unwrap(),
            enqueued_unix: 0,
            claims: vec![Claim::board("AA")],
            priority: Default::default(),
            job: None,
            run: None,
            unknown: Default::default(),
        }
    }

    fn holding(owner: &str) -> Holder {
        Holder {
            ticket: ticket(1, owner),
            token: String::from("t"),
            granted_unix: 0,
            reason: None,
            preempted: None,
            unknown: Default::default(),
        }
    }

    fn state_at(now: u64) -> State {
        State {
            settled_unix_ms: now,
            ..State::default()
        }
    }

    fn owners(state: &mut State, entries: &[(&str, i64)]) {
        for (owner, minutes) in entries {
            state.balances.insert(
                (*owner).into(),
                Balance {
                    balance_ms: minutes * MINUTE as i64,
                    last_active_unix_ms: 1,
                    ..Balance::default()
                },
            );
        }
    }

    #[test]
    fn holding_twenty_minutes_while_another_waits_puts_the_waiter_next() {
        let mut state = state_at(1);
        state.holders.push(holding("a"));
        state.queue.push(ticket(2, "b"));
        settle(&mut state, 1 + 20 * MINUTE);
        let (a, b) = (of(&state.balances, "a"), of(&state.balances, "b"));
        assert_eq!((a, b), (-20 * MINUTE as i64, 20 * MINUTE as i64));
        state.holders.clear();
        state.queue.push(ticket(3, "a"));
        assert!(
            before(&state, &state.queue[0], &state.queue[1]),
            "b before a"
        );
    }

    #[test]
    fn waiting_behind_its_own_lease_earns_nothing() {
        let mut state = state_at(1);
        owners(&mut state, &[("a", 0), ("b", 0)]);
        state.holders.push(holding("a"));
        state.queue.push(ticket(2, "a"));
        settle(&mut state, 1 + 10 * MINUTE);
        // The holder is charged its ten minutes; b, idle, gains the mean.
        assert_eq!(of(&state.balances, "a"), -5 * MINUTE as i64);
        assert_eq!(of(&state.balances, "b"), 5 * MINUTE as i64);
    }

    #[test]
    fn waiting_behind_another_owners_request_is_credited() {
        let mut state = state_at(1);
        owners(&mut state, &[("a", 10), ("b", 0), ("c", -10)]);
        // a's request is served first and waits for a board c holds; b's
        // request conflicts only with a's.
        let mut first = ticket(2, "a");
        first.claims = vec![Claim::board("AA"), Claim::board("BB")];
        state.queue.push(first);
        state.queue.push(ticket(3, "b"));
        let mut held = holding("c");
        held.ticket.claims = vec![Claim::board("BB")];
        state.holders.push(held);
        let decayed = |minutes: i64| {
            (minutes as f64 * MINUTE as f64 * 0.5_f64.powf(1.0 / 12.0)).round() as i64
        };
        settle(&mut state, 1 + 10 * MINUTE);
        // a and b are credited, c charged; the mean of +10 minutes is taken
        // from all three.
        let mean = 10 * MINUTE as i64 / 3;
        assert_eq!(
            of(&state.balances, "b"),
            decayed(0) + 10 * MINUTE as i64 - mean
        );
        assert_eq!(
            of(&state.balances, "a"),
            decayed(10) + 10 * MINUTE as i64 - mean
        );
    }

    #[test]
    fn every_settlement_keeps_the_sum_near_zero() {
        let mut state = state_at(1);
        owners(&mut state, &[("a", 0), ("b", 0), ("c", 0), ("d", 0)]);
        state.holders.push(holding("a"));
        for (id, owner) in [(2, "b"), (3, "c"), (4, "d")] {
            state.queue.push(ticket(id, owner));
        }
        for step in 1..=30 {
            settle(&mut state, 1 + step * MINUTE);
            let sum: i64 = state.balances.values().map(|b| b.balance_ms).sum();
            assert!(sum.abs() <= 4, "step {step}: sum {sum}");
        }
    }

    #[test]
    fn balances_decay_with_a_two_hour_half_life_and_are_clamped() {
        let mut state = state_at(1);
        owners(&mut state, &[("a", 40), ("b", -40)]);
        settle(&mut state, 1 + HALF_LIFE.as_millis() as u64);
        assert_eq!(of(&state.balances, "a"), 20 * MINUTE as i64);
        state.holders.push(holding("a"));
        state.queue.push(ticket(2, "b"));
        settle(&mut state, 1 + HALF_LIFE.as_millis() as u64 + 200 * MINUTE);
        assert_eq!(of(&state.balances, "a"), -(CAP.as_millis() as i64));
        assert_eq!(of(&state.balances, "b"), CAP.as_millis() as i64);
    }

    #[test]
    fn an_owner_neither_waiting_nor_holding_is_not_credited() {
        let mut state = state_at(1);
        owners(&mut state, &[("a", 0), ("c", 0), ("idle", 0)]);
        state.holders.push(holding("a"));
        // A queued request on a disjoint board is not blocked.
        let mut free = ticket(2, "c");
        free.claims = vec![Claim::board("BB")];
        state.queue.push(free);
        settle(&mut state, 1 + 10 * MINUTE);
        assert_eq!(of(&state.balances, "c"), of(&state.balances, "idle"));
    }

    #[test]
    fn waiting_behind_a_grantable_request_is_not_credited() {
        let mut state = state_at(1);
        owners(&mut state, &[("head", 0), ("b", 0)]);
        // Nothing holds the board: the head is served at its next poll.
        state.queue.push(ticket(2, "head"));
        state.queue.push(ticket(3, "b"));
        settle(&mut state, 1 + MINUTE);
        assert_eq!(of(&state.balances, "b"), of(&state.balances, "head"));
        assert!(before(&state, &state.queue[0], &state.queue[1]));
    }

    #[test]
    fn owners_idle_for_a_day_leave_the_normalization() {
        let mut state = state_at(1);
        for (owner, active) in [("old", 1), ("new", ACTIVE_WINDOW.as_millis() as u64 + 10)] {
            state.balances.insert(
                owner.into(),
                Balance {
                    balance_ms: 5 * MINUTE as i64,
                    last_active_unix_ms: active,
                    ..Balance::default()
                },
            );
        }
        normalize(&mut state, ACTIVE_WINDOW.as_millis() as u64 + 20);
        assert!(!state.balances.contains_key("old"));
        assert_eq!(of(&state.balances, "new"), 0);
    }
}
