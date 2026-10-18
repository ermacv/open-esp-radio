//! AP-epoch modelled service accounting; these are not on-air measurements.
use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum WifiAirtimePeer {
    Group,
    Unicast {
        address: [u8; 6],
        association_id: u16,
        association_epoch: u32,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct WifiAirtimePeerEvidence {
    pub peer: WifiAirtimePeer,
    pub grants: u64,
    pub granted_micros: u64,
    pub settlements: u64,
    pub settled_grants_micros: u64,
    pub charged_micros: u64,
    pub cancellations: u64,
    pub cancelled_grants_micros: u64,
    /// State immediately after this peer's last observed transaction edge.
    /// Other peers' later selections can change its current balance.
    pub balance_after_last_event_micros: i64,
    pub outstanding: u32,
    pub outstanding_micros: u64,
    pub maximum_outstanding: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct WifiAirtimeReport {
    pub peer_records: u8,
    /// Events not stored because the bounded epoch history was full.
    pub dropped_events: u64,
    pub saturated: bool,
}
