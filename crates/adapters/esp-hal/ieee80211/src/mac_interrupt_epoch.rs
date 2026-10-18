//! ESP-HAL binding of the single pinned driver/IRQ installation.
//!
//! The platform keeps the WIFI singleton and both CPU route tokens by value.
//! DriverScope owns ActiveEpoch's register banks by value. The sole static
//! address slot contains neither bank and can only be installed through that
//! audited scope. Setup/active/detached transitions do not query a registry.

#![forbid(unsafe_code)]

mod storage;

use crate::{EspHalWifiPlatform, WifiInterruptError};
use oer_esp32s31_ieee80211_form::scope::ScopeSlot;
use oer_esp32s31_ieee80211_mac::irq::{IrqSnapshot, PowerIrqSnapshot};

/// Routing authority retained by value in DriverScope's ScopeRoute.
/// The unique WIFI peripheral and table entries exclude a second installation.
#[must_use = "retain both interrupt route tokens for the entire installation"]
pub struct EspHalMacInterruptRoute {
    platform: EspHalWifiPlatform,
}

impl EspHalMacInterruptRoute {
    pub fn new(platform: EspHalWifiPlatform) -> Self {
        Self { platform }
    }

    /// The scope binding pairs this one slot with this by-value platform.
    /// Its executor mask must also exclude its software-interrupt driver.
    pub fn slot(&self) -> &'static ScopeSlot {
        &storage::INSTALLATION
    }

    pub fn mask(&self) {
        self.platform.disable_interrupts();
    }

    /// Core mismatch is a physical routing failure, not a storage state.
    pub fn enable(&self) -> Result<(), WifiInterruptError> {
        self.platform.enable_interrupts()
    }

    /// Available after ScopeRoute returns from synchronous detachment.
    pub fn into_platform(self) -> EspHalWifiPlatform {
        self.platform
    }
}

/// One bounded read/mask/ack/publication transaction through DriverScope.
#[inline(always)]
pub fn service_mac_interrupt() -> IrqSnapshot {
    storage::INSTALLATION.mac_interrupt()
}

/// Both IRQ banks use the same pinned owner and deregistration proof.
#[inline(always)]
pub fn service_power_interrupt() -> PowerIrqSnapshot {
    storage::INSTALLATION.power_interrupt()
}
