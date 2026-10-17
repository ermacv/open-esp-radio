//! Executor-independent platform ports and resources for ESP32-S31 Wi-Fi TX.
//!
//! The ordinary TX owner may await these ports, but this module does not pick
//! an executor, timer driver or entropy peripheral. It also translates the
//! owned PHY calibration profile into the narrow power lookup consumed by the
//! Wi-Fi descriptor path without exposing the vendor parameter image.

use core::pin::Pin;

use oer_esp32s31_ieee80211_mac::{tx::TxSlot, tx::runtime::WifiTxRuntimePolicy};
use oer_esp32s31_phy::PhyTxTargetPowerProfile;

/// State of one finite ESP32-S31 Wi-Fi TX transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiTxProgress {
    /// DMA, acknowledgement or a bounded retry is still in flight.
    Pending,
    /// Hardware no longer owns the TX descriptor or its frame.
    Complete,
}

/// Reason for inspecting one active TX transaction.
///
/// The executor decides how either edge is produced. The transaction owner
/// consumes only this value and therefore remains independent of Embassy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiTxWake {
    /// A coalesced completion, hardware-timeout or collision interrupt fired.
    Interrupt { events: u32 },
    /// The transaction's external deadline expired without a decisive IRQ.
    Deadline,
}

/// Platform entropy input used only for bounded EDCA slot selection.
pub trait WifiTxEntropy {
    fn next_u32(&mut self) -> u32;
}

impl<F: FnMut() -> u32> WifiTxEntropy for F {
    fn next_u32(&mut self) -> u32 {
        self()
    }
}

/// Calibrated MAC power lookup without exposing a vendor parameter image.
pub trait WifiTxPowerProfile {
    fn power_pair(&self, rate_code: u8) -> WifiTxPowerPair;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiTxPowerPair {
    pub primary: i8,
    pub alternate: i8,
}

impl WifiTxPowerProfile for PhyTxTargetPowerProfile {
    fn power_pair(&self, rate_code: u8) -> WifiTxPowerPair {
        let pair = self.pair(rate_code);
        WifiTxPowerPair {
            primary: pair.primary,
            alternate: pair.alternate,
        }
    }
}

/// Finite policy for management and EAPOL frames sent before connected IRQ
/// scheduling owns the ordinary descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlTxConfig {
    /// Executor watchdog for each hardware publication.
    pub completion_timeout: oer_time::Duration,
    /// Cooperative polling interval before the MAC IRQ owner is installed.
    pub poll_interval: oer_time::Duration,
}

/// Resources whose ownership must stay together for every ordinary TX phase.
///
/// Protocol owners add their own finite configuration instead of exposing a
/// growing positional constructor whenever another runtime or hardware port
/// is introduced.
pub struct WifiTxResources<'slot, P, E, T, const BUFFER_SIZE: usize> {
    pub slot: Pin<&'slot mut TxSlot<BUFFER_SIZE>>,
    pub policy: WifiTxRuntimePolicy,
    pub power: P,
    pub entropy: E,
    pub timer: T,
}
