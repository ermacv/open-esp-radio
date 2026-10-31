//! The Wi-Fi interrupt line and the radio executor's signals.
use oer_esp32s31_compose::Esp32s31;
use oer_esp32s31_wifi::{InterruptTypes, Interrupts};
use oer_espressif_executor::InterruptExecutor;
use oer_espressif_interrupt_table::signal::{FromHandler, FromTask, IrqSignal};

type RadioExecutor = InterruptExecutor<Esp32s31, 2>;
oer_memory::zeroed_static! {
    pub(crate) static LINE: Interrupts = zeroed in ".critical.bss.wifi_rx_line";
}
oer_espressif_staged_runtime::placed_static! {
    pub(crate) static MAC_SIGNAL: IrqSignal<RadioExecutor> = IrqSignal::new::<(FromHandler<crate::WifiToken>, FromTask<oer_espressif_executor::Executor<Esp32s31, 0>, crate::ExecutorWake>), crate::RadioToken>(), in critical_data "wifi_station";
    pub(crate) static POWER_SIGNAL: IrqSignal<RadioExecutor> = IrqSignal::new::<FromHandler<crate::WifiToken>, crate::RadioToken>(), in critical_data "wifi_station";
}
pub(crate) type WifiPart = <Interrupts as InterruptTypes>::Part;

/// The radio handler (P3): the doorbell's requests, then the MAC bank, then
/// the WDEVPWR bank, one cause per entry. A MAC fatal assertion is recorded
/// and resets the chip.
pub(crate) fn on_wifi(part: &mut WifiPart) {
    match LINE.service(part) {
        Ok(_installed) => {}
        Err(raw) => oer_esp32s31_compose::wifi_fault::reset(raw),
    }
}
