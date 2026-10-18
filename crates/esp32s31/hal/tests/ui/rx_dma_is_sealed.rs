#![no_std]
use oer_esp32s31_hal::ieee80211::dma::rx_dma::{RxDma, WifiRxDmaEngine};
use oer_esp32s31_hal::owner::RadioRuntimeOwner;

// Code above the HAL drives the walker generically, through the engine the
// running owner lends.
pub fn observe(walker: &mut impl RxDma) -> bool {
    walker.walker_enabled()
}

pub fn engine(owner: &mut RadioRuntimeOwner) -> &mut WifiRxDmaEngine {
    owner.rx_dma()
}

// It cannot stand in for the walker: `RxDma` requires the HAL's private
// `Sealed`, so a type outside the HAL never reaches the walker's registers
// or its descriptor bindings.
#[cfg(ui_reject)]
pub struct Foreign;

#[cfg(ui_reject)]
impl oer_esp32s31_hal::ieee80211::dma::rx_dma::sealed::Sealed for Foreign {} // ui-error: E0603
