//! Diagnostic-only AP observation owner.

use crate::roles::access_point::AccessPointControlObservation;

use oer_esp32s31_ieee80211_ap::{engine::ApEngineObservation, transaction::ApMacObservation};

/// Value-only AP protocol evidence emitted at the terminal owner edge.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AccessPointTerminalObservation {
    pub control: AccessPointControlObservation,
    pub mac: ApMacObservation,
    pub engine: ApEngineObservation,
}

/// Non-owning terminal observer. Implementations receive facts after the AP
/// protocol is quiescent and cannot influence scheduling or hardware state.
pub trait AccessPointTerminalObserver: Sync {
    fn observe(&self, observation: AccessPointTerminalObservation);
}

/// External storage for accumulated AP observations for one role epoch.
///
/// The AP processor borrows this storage exclusively and returns it at the
/// terminal owner edge. The large value therefore never becomes part of the
/// active/parked protocol state machine. Functional RX progress never depends
/// on this value.
#[derive(Default)]
pub struct AccessPointObservationStorage {
    pub(crate) observation: AccessPointControlObservation,
    #[cfg(feature = "diagnostics")]
    pub(crate) next_rx_block_ack_hardware_sample: u64,
}

impl AccessPointObservationStorage {
    #[cfg(feature = "diagnostics")]
    pub(crate) fn reset(&mut self) {
        self.observation = AccessPointControlObservation::default();
        self.next_rx_block_ack_hardware_sample = 8_192;
    }
}
