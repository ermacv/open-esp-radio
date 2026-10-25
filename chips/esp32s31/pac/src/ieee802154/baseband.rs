//! The read-only baseband receive-information capability of IEEE 802.15.4.
//!
//! The pinned ESP-IDF driver reports its recent RSSI as the low byte of the
//! shared BTBB baseband's current receive information
//! (`ieee802154_get_recent_rssi` over `bt_bb_get_cur_rx_info`), read without
//! a lock from any context. The rest of that baseband is shared with
//! Bluetooth and serialized by the shared radio owner, so this capability
//! is the generated read capability of the one register: it has no write
//! accessor, and the shared radio owner keeps every other register.
//!
//! Limits: read side effects of `CURRENT_RX_INFO` are unproven, so the
//! capability is not `Copy`, and the byte reflects the most recent baseband
//! reception of whichever protocol received it, as it does for the vendor
//! while Bluetooth shares the baseband.

#![forbid(unsafe_code)]

use crate::generated::bt_v3_2_baseband::{BtV3_2BasebandRest, BtV3_2BasebandRxInfo};

/// Read of the shared baseband's current receive information, owned by
/// IEEE 802.15.4.
#[must_use = "dropping the capability loses the baseband RSSI read"]
pub struct Ieee802154BasebandRxInfo {
    registers: BtV3_2BasebandRxInfo,
}

impl Ieee802154BasebandRxInfo {
    /// The signed RSSI in dBm of the most recent baseband reception.
    pub(crate) fn recent_rssi(&self) -> i8 {
        self.registers.read_current_rx_info().rssi().cast_signed()
    }
}

/// Split the receive-information read off the shared baseband: the shared
/// radio owner keeps every other register of it.
pub(crate) fn split(
    baseband: crate::generated::bt_v3_2_baseband::BtV3_2Baseband,
) -> (BtV3_2BasebandRest, Ieee802154BasebandRxInfo) {
    let (rest, registers) = baseband.split_rx_info();
    (rest, Ieee802154BasebandRxInfo { registers })
}
