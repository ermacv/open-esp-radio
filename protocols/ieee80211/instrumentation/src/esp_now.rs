//! Events of the ESP-NOW encrypted peer table.

use oer_trace::{Domain, Event, Kind};

use crate::ESP_NOW;
use crate::event::{coded, coded_event};

coded!(
    /// A change of the encrypted peer table, or a refused one.
    pub enum PeerChange {
        Installed = 0,
        Replaced = 1,
        Removed = 2,
        /// A removal was rolled back into its slot.
        RemoveRolledBack = 3,
        /// An install or replacement was refused.
        MutationRejected = 4,
    }
);

coded_event!(
    /// The encrypted peer table changed, or refused a change.
    EspNowPeerTableChanged { change: PeerChange } = 48 on ESP_NOW,
    "esp-now.peer-table"
);

/// Shutdown cleared the table's keys.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowPeersCleared {
    /// The peers whose keys were dropped, saturated at `u32::MAX`.
    pub count: u32,
}

impl Event for EspNowPeersCleared {
    const KIND: Kind = Kind::new(Domain::Ieee80211, 49);
    const CHANNEL: oer_trace::Channel = ESP_NOW;

    fn encode(&self) -> [u32; 2] {
        [self.count, 0]
    }

    fn decode([count, zero]: [u32; 2]) -> Option<Self> {
        (zero == 0).then_some(Self { count })
    }
}

impl core::fmt::Display for EspNowPeersCleared {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "esp-now.peers-cleared {}", self.count)
    }
}

coded!(
    /// The result of a transmit packet-number allocation.
    pub enum PacketNumberOutcome {
        Allocated = 0,
        /// The peer's packet-number space is exhausted.
        Exhausted = 1,
    }
);

coded_event!(
    /// A peer's transmit packet number was allocated, or was not.
    EspNowTxPacketNumber { outcome: PacketNumberOutcome } = 50 on ESP_NOW,
    "esp-now.tx-pn"
);

coded!(
    /// A step of receive replay admission.
    pub enum ReplayOutcome {
        /// A packet number passed the unauthenticated replay check.
        Candidate = 0,
        /// An authenticated candidate advanced the replay frontier.
        Committed = 1,
        /// A packet number was a replay.
        Replayed = 2,
    }
);

coded_event!(
    /// One step of receive replay admission.
    EspNowReplay { outcome: ReplayOutcome } = 51 on ESP_NOW,
    "esp-now.replay"
);
