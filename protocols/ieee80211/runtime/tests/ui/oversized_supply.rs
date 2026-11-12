#![no_std]
use oer_protocols_ieee80211_core_roles_sta::data::EthernetFrame;
use oer_protocols_ieee80211_runtime::access_point::network::{AP_TX_DEPTH, Holds};
use oer_protocols_ieee80211_runtime::station::{TxRefused, TxSupply};
use oer_protocols_network_port::{TransmitError, TransmitRefused};

/// A supply that can hold one memory more than the runner's queue back.
pub struct Oversized;
impl TxSupply<EthernetFrame> for Oversized {
    type Memory = u8;
    #[cfg(ui_reject)]
    const MEMORIES: usize = AP_TX_DEPTH + 1;
    #[cfg(not(ui_reject))]
    const MEMORIES: usize = AP_TX_DEPTH;
    fn available(&self) -> bool {
        false
    }
    fn write(&mut self, frame: EthernetFrame) -> Result<u8, TxRefused<EthernetFrame>> {
        Err(TxRefused::Kept(TransmitRefused {
            frame,
            reason: TransmitError::QueueFull,
        }))
    }
    fn give(&mut self, _: u8) {}
}

/// The check `ApNetworkStorage::split` makes at its monomorphization.
pub const HOLDS: () = Holds::<EthernetFrame, Oversized>::BACK; // ui-error@outside: E0080: evaluation panicked: the runner's queue back holds every network memory
