//! The read-only baseband receive-information capability of IEEE 802.15.4.
//!
//! The pinned ESP-IDF driver reports its recent RSSI as the low byte of the
//! shared BTBB baseband's current receive information
//! (`ieee802154_get_recent_rssi` over `bt_bb_get_cur_rx_info`), read without
//! a lock from any context. The rest of that baseband is shared with
//! Bluetooth and serialized by the shared radio owner, so this capability
//! carves out only a read of the one register: it has no write accessor and
//! no complete-image read.
//!
//! Limits: read side effects of `CURRENT_RX_INFO` are unproven, and the byte
//! reflects the most recent baseband reception of whichever protocol
//! received it, as it does for the vendor while Bluetooth shares the
//! baseband.

use crate::svd;

/// Read of the shared baseband's current receive information, owned by
/// IEEE 802.15.4.
#[must_use = "dropping the capability loses the baseband RSSI read"]
pub struct Ieee802154BasebandRxInfo {
    registers: svd::BtV3_2Baseband,
}

impl Ieee802154BasebandRxInfo {
    /// The signed RSSI in dBm of the most recent baseband reception.
    pub(crate) fn recent_rssi(&self) -> i8 {
        svd::field_read::observe_ieee802154_recent_rssi(&self.registers).cast_signed()
    }
}

/// Carve the receive-information read out of the shared radio partition.
///
/// # Safety invariant
///
/// The duplicated baseband handle lives in [`Ieee802154BasebandRxInfo`],
/// whose only operation reads `CURRENT_RX_INFO`. No operation of the shared
/// radio owner accesses that read-only register, so the two handles touch
/// disjoint registers, and the capability is never reunited with the
/// partition.
#[allow(
    unsafe_code,
    reason = "duplicating the baseband handle creates the register-disjoint read capability"
)]
pub(crate) fn split(
    shared_radio: svd::peripheral_ownership::SharedRadioPeripherals,
) -> (
    svd::peripheral_ownership::SharedRadioPeripherals,
    Ieee802154BasebandRxInfo,
) {
    // SAFETY: see the invariant.
    let registers = unsafe { svd::BtV3_2Baseband::steal() };
    (shared_radio, Ieee802154BasebandRxInfo { registers })
}
