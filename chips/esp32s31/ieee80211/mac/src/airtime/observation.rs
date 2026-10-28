//! Value-only observations; optional consumers own cumulative telemetry.
use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AirtimeAction {
    Granted,
    Settled,
    Cancelled,
}

/// One successful transition. Balance includes pending reservations and may
/// change again when later selections advance rounds or remove eligible demand.
/// This is modelled service credit, not measured medium occupancy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AirtimeObservation<K> {
    pub key: K,
    pub action: AirtimeAction,
    pub grant_micros: u32,
    pub charged_micros: u32,
    pub balance_after_event_micros: i64,
    pub outstanding: u32,
    pub outstanding_micros: u64,
}

impl<K: Copy + Eq, const PEERS: usize, const RESERVATIONS: usize>
    AirtimeScheduler<'_, K, PEERS, RESERVATIONS>
{
    pub(super) fn observe(
        &self,
        account: Account<K>,
        action: AirtimeAction,
        grant_micros: u32,
        charged_micros: u32,
    ) {
        let Some(observer) = self.observer else {
            return;
        };
        let mut event = AirtimeObservation {
            key: account.key,
            action,
            grant_micros,
            charged_micros,
            balance_after_event_micros: account.balance,
            outstanding: 0,
            outstanding_micros: 0,
        };
        for pending in self
            .state
            .pending
            .iter()
            .flatten()
            .filter(|p| p.key == account.key)
        {
            // Storage's const capacity bound proves these never saturate:
            // at most u32::MAX reservations, each with a u32 budget.
            event.outstanding = event.outstanding.saturating_add(1);
            event.outstanding_micros = event
                .outstanding_micros
                .saturating_add(u64::from(pending.budget));
        }
        observer(event);
    }
}
