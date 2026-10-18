//! Grant order and waiting-time estimates over conflicting claims.
//!
//! Requests that conflict are served by their owners' balances
//! ([`crate::balance`]), the earlier request on a tie: a ticket is granted
//! once no holder and no waiting ticket served before it conflicts with it.
//! Tickets that claim disjoint resources proceed in parallel.

use std::time::Duration;

use oer_stand_claims::conflict;

use crate::{
    balance,
    state::{State, Ticket},
};

/// The waiting tickets served before `ticket` that conflict with it, in
/// service order.
fn ahead<'a>(state: &'a State, ticket: &Ticket) -> Vec<&'a Ticket> {
    let mut ahead = state
        .queue
        .iter()
        .filter(|other| {
            other.id != ticket.id
                && balance::before(state, other, ticket)
                && conflict(&other.claims, &ticket.claims)
        })
        .collect::<Vec<_>>();
    ahead.sort_by(|a, b| {
        if balance::before(state, a, b) {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Greater
        }
    });
    ahead
}

/// Whether the ticket `id` can be granted now.
pub(crate) fn grantable(state: &State, id: u64) -> bool {
    let Some(ticket) = state.queue.iter().find(|ticket| ticket.id == id) else {
        return false;
    };
    !state
        .holders
        .iter()
        .any(|holder| conflict(&holder.ticket.claims, &ticket.claims))
        && ahead(state, ticket).is_empty()
}

/// Whether a waiting ticket conflicts with the claims of holder `id`.
pub(crate) fn blocks_waiters(state: &State, id: u64) -> bool {
    state
        .holders
        .iter()
        .find(|holder| holder.ticket.id == id)
        .is_some_and(|holder| {
            state
                .queue
                .iter()
                .any(|ticket| conflict(&ticket.claims, &holder.ticket.claims))
        })
}

/// The owner and balance of the waiting ticket that holder `id` blocks and
/// whose owner's balance exceeds the holder's, when that holder has held for
/// at least `slice`: divisible work then yields at its next boundary.
pub(crate) fn outranked_by(
    state: &State,
    id: u64,
    now_unix: u64,
    slice: Duration,
) -> Option<(String, i64)> {
    let holder = state.holders.iter().find(|holder| holder.ticket.id == id)?;
    if now_unix.saturating_sub(holder.granted_unix) < slice.as_secs() {
        return None;
    }
    let own = balance::of(&state.balances, &holder.ticket.owner);
    state
        .queue
        .iter()
        .filter(|ticket| {
            ticket.owner != holder.ticket.owner && conflict(&ticket.claims, &holder.ticket.claims)
        })
        .map(|ticket| {
            (
                ticket.owner.clone(),
                balance::of(&state.balances, &ticket.owner),
            )
        })
        .filter(|(_, balance)| *balance > own)
        .max_by_key(|(_, balance)| *balance)
}

/// The conflicting tickets served earlier and the expected wait in seconds of
/// every queued ticket. A ticket starts once its conflicting holders end and
/// every conflicting ticket served before it has started and ended, so the
/// wait of a ticket behind one that itself waits includes that wait. A
/// holder that a waiting maintenance ticket conflicts with is preempted and
/// ends within `grace`.
pub(crate) fn expected_starts(state: &State, now: u64, grace: Duration) -> Vec<(u64, usize, u64)> {
    let remaining = |ticket: &Ticket| {
        state
            .holders
            .iter()
            .filter(|holder| conflict(&holder.ticket.claims, &ticket.claims))
            .map(|holder| {
                let end = (holder.granted_unix + holder.ticket.estimate_secs).saturating_sub(now);
                // A waiting maintenance ticket preempts the holder.
                let preempted = state.queue.iter().any(|waiting| {
                    waiting.priority == crate::state::Priority::Maintenance
                        && conflict(&waiting.claims, &holder.ticket.claims)
                });
                if preempted {
                    end.min(grace.as_secs())
                } else {
                    end
                }
            })
            .max()
            .unwrap_or(0)
    };
    // Service order puts every ticket after those served before it, so each
    // start is known when a later ticket needs it.
    let mut order = state.queue.iter().collect::<Vec<_>>();
    order.sort_by(|a, b| {
        if balance::before(state, a, b) {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Greater
        }
    });
    let mut starts = std::collections::BTreeMap::<u64, u64>::new();
    for ticket in order {
        let ahead = ahead(state, ticket);
        let start = ahead
            .iter()
            .map(|earlier| starts.get(&earlier.id).copied().unwrap_or(0) + earlier.estimate_secs)
            .chain([remaining(ticket)])
            .max()
            .unwrap_or(0);
        starts.insert(ticket.id, start);
    }
    state
        .queue
        .iter()
        .map(|ticket| {
            (
                ticket.id,
                ahead(state, ticket).len(),
                starts.get(&ticket.id).copied().unwrap_or(0),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{balance::Balance, process::ProcessIdentity, state::Holder};
    use oer_stand_claims::{AIR, Claim};

    pub(crate) fn ticket(id: u64, estimate_secs: u64, claims: Vec<Claim>) -> Ticket {
        Ticket {
            id,
            owner: format!("owner-{id}"),
            work: format!("work-{id}"),
            estimate_secs,
            process: ProcessIdentity {
                pid: 1,
                start_ticks: 1,
            },
            enqueued_unix: id,
            claims,
            priority: Default::default(),
            job: None,
            run: None,
            unknown: Default::default(),
        }
    }

    fn board(name: &str) -> Vec<Claim> {
        vec![Claim::board(name), Claim::shared(AIR)]
    }

    fn holder(ticket: Ticket) -> Holder {
        Holder {
            ticket,
            token: "t".into(),
            granted_unix: 1000,
            reason: None,
            preempted: None,
            unknown: Default::default(),
        }
    }

    fn balance(state: &mut State, owner: &str, minutes: i64) {
        state.balances.insert(
            owner.into(),
            Balance {
                balance_ms: minutes * 60_000,
                ..Balance::default()
            },
        );
    }

    #[test]
    fn disjoint_requests_proceed_while_conflicting_ones_wait() {
        let state = State {
            holders: vec![holder(ticket(1, 300, board("chip-a")))],
            queue: vec![
                ticket(2, 60, board("chip-a")),
                ticket(3, 60, board("chip-b")),
                ticket(4, 60, vec![Claim::board("chip-b"), Claim::exclusive(AIR)]),
            ],
            ..State::default()
        };
        assert!(!grantable(&state, 2), "chip-a is held");
        assert!(grantable(&state, 3), "chip-b is free");
        assert!(!grantable(&state, 4), "exclusive air waits for the holder");
        assert!(blocks_waiters(&state, 1));
    }

    #[test]
    fn the_owner_with_the_higher_balance_is_served_first() {
        let mut state = State {
            queue: vec![
                ticket(1, 600, board("chip-a")),
                ticket(2, 60, board("chip-a")),
            ],
            ..State::default()
        };
        // Equal balances keep arrival order.
        assert!(grantable(&state, 1));
        assert!(!grantable(&state, 2));
        balance(&mut state, "owner-2", 5);
        assert!(!grantable(&state, 1));
        assert!(grantable(&state, 2));
    }

    #[test]
    fn a_holder_is_outranked_only_after_its_slice_by_a_higher_balance() {
        let mut state = State {
            holders: vec![holder(ticket(1, 300, board("chip-a")))],
            queue: vec![ticket(2, 60, board("chip-a"))],
            ..State::default()
        };
        balance(&mut state, "owner-1", -3);
        balance(&mut state, "owner-2", 4);
        let slice = Duration::from_secs(600);
        assert_eq!(outranked_by(&state, 1, 1000 + 599, slice), None);
        assert_eq!(
            outranked_by(&state, 1, 1000 + 600, slice),
            Some((String::from("owner-2"), 4 * 60_000))
        );
        balance(&mut state, "owner-2", -5);
        assert_eq!(outranked_by(&state, 1, 1000 + 600, slice), None);
    }

    #[test]
    fn expected_starts_count_only_conflicting_work_served_earlier() {
        let state = State {
            holders: vec![holder(ticket(9, 300, board("chip-a")))],
            queue: vec![
                ticket(1, 600, board("chip-a")),
                ticket(2, 60, board("chip-b")),
                ticket(3, 60, board("chip-a")),
            ],
            ..State::default()
        };
        assert_eq!(
            expected_starts(&state, 1100, Duration::from_secs(300)),
            [(1, 0, 200), (2, 0, 0), (3, 1, 800)]
        );
    }

    #[test]
    fn a_request_behind_a_waiting_one_starts_after_it() {
        // Ticket 1 waits for the board the holder keeps for 900 s more;
        // ticket 2 needs only the fixture software ticket 1 shares.
        let software = |mode: fn(&str) -> Claim| vec![mode("fixture-software:linux-bluetooth")];
        let mut first = board("chip-a");
        first.extend(software(|name| Claim::shared(name)));
        let state = State {
            holders: vec![holder(ticket(9, 1000, board("chip-a")))],
            queue: vec![
                ticket(1, 60, first),
                ticket(2, 60, software(|name| Claim::exclusive(name))),
            ],
            ..State::default()
        };
        assert_eq!(
            expected_starts(&state, 1100, Duration::from_secs(300)),
            [(1, 0, 900), (2, 1, 960)]
        );
    }

    #[test]
    fn maintenance_goes_first_and_waits_only_for_the_preemption_grace() {
        let mut maintenance = ticket(2, 60, board("chip-a"));
        maintenance.priority = crate::state::Priority::Maintenance;
        let mut state = State {
            holders: vec![holder(ticket(9, 3000, board("chip-a")))],
            queue: vec![ticket(1, 60, board("chip-a")), maintenance],
            ..State::default()
        };
        balance(&mut state, "owner-1", 30);
        assert_eq!(
            expected_starts(&state, 1100, Duration::from_secs(300)),
            [(1, 1, 360), (2, 0, 300)]
        );
        state.holders.clear();
        assert!(grantable(&state, 2), "maintenance outranks any balance");
        assert!(!grantable(&state, 1));
    }
}
