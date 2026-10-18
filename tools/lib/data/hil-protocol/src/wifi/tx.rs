//! AP software retention losses, separate from on-air delivery failures.
use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct WifiTxRetentionEvidence {
    pub active_queue_full: u32,
    pub unicast_power_save_full: u32,
    pub group_power_save_full: u32,
}

impl WifiTxRetentionEvidence {
    pub const fn has_drops(self) -> bool {
        self.active_queue_full != 0
            || self.unicast_power_save_full != 0
            || self.group_power_save_full != 0
    }
}
