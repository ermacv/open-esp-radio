//! Value-only observations, reported to the sink each transition's caller
//! lends; that caller owns cumulative telemetry.
use super::*;
use oer_portable_events::report;

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
        events: &mut impl EventSink<AirtimeObservation<K>>,
    ) {
        // The account keeps its outstanding reservations' count and sum.
        report(events, || AirtimeObservation {
            key: account.key,
            action,
            grant_micros,
            charged_micros,
            balance_after_event_micros: account.balance,
            outstanding: account.outstanding,
            outstanding_micros: account.outstanding_micros,
        });
    }
}
