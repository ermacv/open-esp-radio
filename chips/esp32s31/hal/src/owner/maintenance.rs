//! The stopped Wi-Fi MAC check.
//!
//! Every protocol reaches the shared PHY through the radio domain.
//! Wi-Fi's stopped-MAC check confirms the final-client shutdown boundary: MAC
//! and walker readbacks check that the caller drained active TX and stopped
//! its RX descriptor epoch; they do not retire software DMA leases.

use super::{MacInterruptSetup, RadioRuntimeOwner};
use crate::ieee80211::{
    dma::rx_dma::{RxDma, WifiRxDmaEngine},
    mac::WifiMacHal,
};

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
            let (mac, rx_dma) = registers.split_rx_dma();
            check_stopped(&mut Stopped { mac, rx_dma })
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

/// The MAC and the RX DMA engine whose readbacks prove the stop.
struct Stopped<'a> {
    mac: WifiMacHal<'a>,
    rx_dma: &'a mut WifiRxDmaEngine,
}

impl Observation for Stopped<'_> {
    fn mac_active(&mut self) -> u8 {
        self.mac.channel_active_state()
    }
    fn rx_walker_enabled(&mut self) -> bool {
        self.rx_dma.walker_enabled()
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
