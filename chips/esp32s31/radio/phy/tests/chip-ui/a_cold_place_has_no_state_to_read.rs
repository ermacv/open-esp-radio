#![no_std]
use oer_chips_esp32s31_hal::radio::domain::ChipPhy;
use oer_chips_esp32s31_radio_phy::owner::{PhyHandle, RfCold, RfOpen};

pub fn read(handle: &PhyHandle<'_, ChipPhy>, cold: &RfCold<ChipPhy>, open: &RfOpen<ChipPhy>) {
    let _registered = handle.state(open);
    #[cfg(ui_reject)]
    let _cold = handle.state(cold); // ui-error: E0277
    let _ = cold;
}
