//! Probe-image entries (`cfg(oer_probe)`) of the IEEE 802.15.4 coexistence and MAC timing
//! writes, for comparison with the vendor leaves they port
//! (`hal_set_IEEE802154_TXRX_pti`, `hal_set_IEEE802154_ACK_pti` of
//! libcoexist, `ieee802154_txon_delay_set` of libbtbb).
//!
//! Each takes the IEEE 802.15.4 partition of an isolated validation root
//! and performs exactly the production write through the MAC owner.

use crate::coex::CoexPti;
use crate::ieee802154::mac::{self, Ieee802154Pti, Ieee802154TaskOwner};
use crate::root::RadioPartitions;

/// The vendor status of a rejected input: no write.
const REJECTED: u32 = u32::MAX;

/// The IEEE 802.15.4 MAC owner of an isolated validation root.
fn task_owner() -> Ieee802154TaskOwner {
    let (task, _interrupts) = mac::split(RadioPartitions::for_probe().ieee802154.into_mac());
    task
}

/// `hal_set_IEEE802154_TXRX_pti`: replace the TX/RX PTI field, as the
/// low-level `set_txrx_pti` does. Zero, or [`REJECTED`] without a write for
/// a priority outside the four-bit domain.
pub fn set_txrx_pti(pti: u32) -> u32 {
    let Some(pti) = u8::try_from(pti).ok().and_then(CoexPti::new) else {
        return REJECTED;
    };
    task_owner().set_txrx_pti(Ieee802154Pti::from_coex(pti));
    0
}

/// `hal_set_IEEE802154_ACK_pti`: replace the ACK PTI field, as the
/// low-level `set_ack_pti` does. Zero, or [`REJECTED`] without a write.
pub fn set_ack_pti(pti: u32) -> u32 {
    let Some(pti) = u8::try_from(pti).ok().and_then(CoexPti::new) else {
        return REJECTED;
    };
    task_owner().set_ack_pti(Ieee802154Pti::from_coex(pti));
    0
}

/// `ieee802154_txon_delay_set`: the MAC timing delays the foundation
/// writes during MAC initialization. Always zero.
pub fn set_txon_delay() -> u32 {
    task_owner().apply_txon_delay();
    0
}
