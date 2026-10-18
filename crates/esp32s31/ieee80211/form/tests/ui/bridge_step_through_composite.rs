#![no_std]
use oer_esp32s31_ieee80211_form::scope::OwnsDma;
use oer_esp32s31_ieee80211_form::scope::commands::bridge::{Bridged, Executor, ModelHardware};

// A bridge executor steps only through the composite that owns it with its
// hardware: nothing outside can drive its records onto some other DMA owner.
#[cfg(not(ui_reject))]
pub fn step<HW: OwnsDma>(bridged: &mut Bridged<HW, 4, 9>)
where
    HW::Dma: ModelHardware,
{
    let _ = bridged.step();
}

#[cfg(ui_reject)]
pub fn step<D: ModelHardware>(executor: &mut Executor<D, 4, 9>, dma: &mut D) {
    let _ = executor.step(dma); // ui-error: E0624
}
