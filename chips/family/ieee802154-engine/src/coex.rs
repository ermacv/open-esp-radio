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
pub use oer_chips_family_coex::CoexPti;

use oer_chips_family_coex::{CoexEventId, CoexPtiTable, ieee802154_events};
use oer_protocols_ieee802154_port::RadioOperation;

/// The shared-table event an IEEE 802.15.4 operation reads.
///
/// The pinned archives' `coex_ieee802154_pti_get` reads the priority of
/// level `n` at event `40 + n`; `ieee802154_mac_init` and
/// `ieee802154_set_txrx_pti` give the idle scene level 4 (event 44),
/// immediate transmission and reception level 3 (event 43), timed
/// transmission and reception level 2 (event 42) and the ACK level 2.
///
/// SOURCE(esp32s31, esp32c5): esp-coex-lib
/// `fa152b3b6b2e3c5e3a57e55fcf4b005a4e28cef9` `coexist_api.o`, whose body is
/// equal in `esp32s31/libcoexist.a` sha256
/// `01c1b4f2b86cbc3024a2b51f3f104ea7937fc721a4ee57b53146fea6608836bb` and
/// `esp32c5/libcoexist.a` sha256
/// (esp-coex-lib `c758e7b56e0fa22177a0539796e1df59978dc322`)
/// `de883d649e4e2e9407a6ab0dc28d5d2aa4cd8d30d0071abfe5a9183348ceb627`;
/// ESP-IDF `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
/// `esp_ieee802154_util.c` (`s_coex_config`).
///
/// `CslSlot`: product choice: the archive names no CSL row; a slot is a
/// timed reception, so it reads the timed-reception event.
pub const fn ieee802154_event(operation: RadioOperation) -> CoexEventId {
    match operation {
        RadioOperation::Idle => ieee802154_events::IDLE,
        RadioOperation::Tx | RadioOperation::Rx => ieee802154_events::IMMEDIATE,
        RadioOperation::TxAt | RadioOperation::RxAt | RadioOperation::Ack => {
            ieee802154_events::TIMED
        }
        RadioOperation::CslSlot => ieee802154_events::TIMED,
    }
}

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
    /// [`ieee802154_event`].
    pub const fn resolve(table: &CoexPtiTable) -> Self {
        Self {
            idle: table.pti(ieee802154_event(RadioOperation::Idle)),
            txrx: table.pti(ieee802154_event(RadioOperation::Tx)),
            txrx_at: table.pti(ieee802154_event(RadioOperation::TxAt)),
            ack: table.pti(ieee802154_event(RadioOperation::Ack)),
            csl_slot: table.pti(ieee802154_event(RadioOperation::CslSlot)),
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

#[cfg(test)]
mod tests;
