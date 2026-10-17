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
//! the priority of a level from its own coexistence table. A build without
//! software coexistence disables both fields instead
//! (`ieee802154_ll_disable_coex`).

/// One four-bit coexistence priority, the shared-table domain every
/// supported chip's MAC PTI field accepts.
pub use oer_espressif_coex::CoexPti;

/// Operation scene of the TX/RX PTI (`ieee802154_txrx_scene_t`).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Ieee802154CoexScene {
    /// After MAC initialization.
    Idle,
    /// Immediate transmission.
    Tx,
    /// Reception, energy detection and CCA.
    Rx,
    /// Timed transmission.
    TxAt,
    /// Timed reception.
    RxAt,
}

/// The priorities of every scene and of the ACK, resolved from one snapshot
/// of the shared table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154CoexPriorities {
    idle: CoexPti,
    txrx: CoexPti,
    txrx_at: CoexPti,
    ack: CoexPti,
}

impl Ieee802154CoexPriorities {
    /// The priorities of the idle scene, immediate and timed operations and
    /// the ACK, as a chip resolves them from its coexistence table.
    pub const fn new(idle: CoexPti, txrx: CoexPti, txrx_at: CoexPti, ack: CoexPti) -> Self {
        Self {
            idle,
            txrx,
            txrx_at,
            ack,
        }
    }

    /// The TX/RX priority of `scene`.
    pub const fn scene(&self, scene: Ieee802154CoexScene) -> CoexPti {
        match scene {
            Ieee802154CoexScene::Idle => self.idle,
            Ieee802154CoexScene::Tx | Ieee802154CoexScene::Rx => self.txrx,
            Ieee802154CoexScene::TxAt | Ieee802154CoexScene::RxAt => self.txrx_at,
        }
    }

    /// The ACK priority.
    pub const fn ack(&self) -> CoexPti {
        self.ack
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
