//! Common Trouble traffic observations; these fields do not establish security.
use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct BluetoothGattEvidence {
    /// Controller public address in HCI byte order after Trouble initialization.
    pub address: Option<[u8; 6]>,
    pub advertising: bool,
    pub connected: bool,
    pub advertising_starts: u32,
    pub connections: u32,
    pub disconnections: u32,
    /// Application value requests processed; the peer must independently
    /// validate ATT responses. These counters alone do not prove delivery.
    pub reads: u32,
    pub writes: u32,
    pub value: u8,
    pub last_disconnect_reason: Option<u8>,
    /// CPU0 task-stack measurement taken with this snapshot; CPU1 is inactive.
    pub cpu0_stack: Option<crate::system::StackWatermark>,
}
