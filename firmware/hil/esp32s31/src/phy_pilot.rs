//! Image-selected entries for the shared SAR/PWDET platform binding.
use oer_esp32s31_compose::{Esp32s31, pilot, platform::InterruptTypes};
use oer_espressif_interrupt_table::{Controller, Router};
pub(crate) type Client = pilot::Client<crate::PhyToken>;
oer_espressif_staged_runtime::placed_static! {
    static PILOT: pilot::Storage<crate::PhyToken> = pilot::Storage::new::<crate::RadioToken>(), in critical_data "phy_pilot";
}
pub(crate) fn init(
    parts: oer_esp32s31_hal::peripherals::timg0_alarm::Timg0AlarmParts,
    router: &mut Router<impl Controller>,
    token: crate::PhyToken,
) -> Client {
    PILOT.init(parts, router, token)
}
pub(crate) fn interrupt(part: &mut <Esp32s31 as InterruptTypes>::Phy) {
    PILOT.interrupt(part);
}
