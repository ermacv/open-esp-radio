//! Software-coexistence priorities of the IEEE 802.15.4 MAC.
//!
//! Ported from ESP-IDF `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
//! `components/ieee802154/driver/esp_ieee802154_util.c` (Copyright Espressif
//! Systems (Shanghai) Co Ltd, Apache-2.0) and modified by Vladislav
//! Ermachenkov; see `NOTICE`.
//!
//! In a build with software coexistence the pinned driver publishes the MAC's
//! two PTI fields through libcoexist: `ieee802154_mac_init` sets the ACK PTI
//! to the middle level and the TX/RX PTI to the idle scene, and each
//! operation start switches the TX/RX PTI to its scene
//! (`ieee802154_set_txrx_pti` of `esp_ieee802154_util.c`). Each chip resolves
//! the priority of an operation from its own coexistence table. A build without
//! software coexistence disables both fields instead
//! (`ieee802154_ll_disable_coex`).

/// One four-bit coexistence priority, the shared-table domain every
/// supported chip's MAC PTI field accepts.
pub use oer_espressif_coex::CoexPti;

use oer_espressif_coex::CoexPtiTable;
use oer_ieee802154_port::RadioOperation;

/// The priorities of every operation, resolved from one snapshot of the
/// shared table: the idle scene, immediate and timed transmission and
/// reception, the ACK and a CSL slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154CoexPriorities {
    idle: CoexPti,
    txrx: CoexPti,
    txrx_at: CoexPti,
    ack: CoexPti,
    csl_slot: CoexPti,
}

impl Ieee802154CoexPriorities {
    /// The priorities of every operation of `table`, through the rows of
    /// [`oer_espressif_coex::ieee802154_event`].
    pub const fn resolve(table: &CoexPtiTable) -> Self {
        Self {
            idle: table.ieee802154_pti(RadioOperation::Idle),
            txrx: table.ieee802154_pti(RadioOperation::Tx),
            txrx_at: table.ieee802154_pti(RadioOperation::TxAt),
            ack: table.ieee802154_pti(RadioOperation::Ack),
            csl_slot: table.ieee802154_pti(RadioOperation::CslSlot),
        }
    }

    /// The priority of `operation`.
    pub const fn operation(&self, operation: RadioOperation) -> CoexPti {
        match operation {
            RadioOperation::Idle => self.idle,
            RadioOperation::Tx | RadioOperation::Rx => self.txrx,
            RadioOperation::TxAt | RadioOperation::RxAt => self.txrx_at,
            RadioOperation::Ack => self.ack,
            RadioOperation::CslSlot => self.csl_slot,
        }
    }
}

/// How the MAC takes part in coexistence.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Ieee802154Coexistence {
    /// A build without software coexistence: both PTIs are disabled.
    #[default]
    Disabled,
    /// Software coexistence with these priorities.
    Software(Ieee802154CoexPriorities),
}
