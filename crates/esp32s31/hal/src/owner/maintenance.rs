//! The stopped Wi-Fi MAC check.
//!
//! Every protocol reaches the shared PHY through the radio arbiter lease.
//! Wi-Fi's stopped-MAC check confirms the final-client shutdown boundary: MAC
//! and walker readbacks check that the caller drained active TX and stopped
//! its RX descriptor epoch; they do not retire software DMA leases.

use super::{MacInterruptSetup, RadioRuntimeOwner};
use crate::ieee80211::mac::WifiMacHal;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    MacActive { state: u8 },
    RxWalkerEnabled,
}

/// Original resources retained when admission has made no hardware changes.
pub struct AdmissionFailure {
    pub error: Error,
    pub registers: RadioRuntimeOwner,
    pub interrupts: MacInterruptSetup,
}

impl RadioRuntimeOwner {
    /// Verify the stopped physical boundary without acquiring maintenance or
    /// changing hardware. This is the final-client shutdown admission check.
    pub fn try_confirm_phy_stopped(
        self,
        interrupts: MacInterruptSetup,
    ) -> Result<(Self, MacInterruptSetup), AdmissionFailure> {
        confirm_stopped(self, interrupts, |registers| {
            check_stopped(&mut registers.wifi_mac_hal())
        })
    }
}

fn confirm_stopped(
    mut registers: RadioRuntimeOwner,
    interrupts: MacInterruptSetup,
    check: impl FnOnce(&mut RadioRuntimeOwner) -> Result<(), Error>,
) -> Result<(RadioRuntimeOwner, MacInterruptSetup), AdmissionFailure> {
    if let Err(error) = check(&mut registers) {
        return Err(AdmissionFailure {
            error,
            registers,
            interrupts,
        });
    }
    Ok((registers, interrupts))
}

// Closed readback interface: tests exercise the actual admission/release
// predicate without emulating register encodings or touching host MMIO.
trait Observation {
    fn mac_active(&mut self) -> u8;
    fn rx_walker_enabled(&mut self) -> bool;
}

impl Observation for WifiMacHal<'_> {
    fn mac_active(&mut self) -> u8 {
        self.channel_active_state()
    }
    fn rx_walker_enabled(&mut self) -> bool {
        WifiMacHal::rx_walker_enabled(self)
    }
}

fn check_stopped(hardware: &mut impl Observation) -> Result<(), Error> {
    let state = hardware.mac_active();
    if state != 0 {
        return Err(Error::MacActive { state });
    }
    if hardware.rx_walker_enabled() {
        return Err(Error::RxWalkerEnabled);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
