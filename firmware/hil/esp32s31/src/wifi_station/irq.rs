//! Static ISR owners and DMA allocations for the HIL image.
use core::sync::atomic::{AtomicU32, Ordering};
use oer_esp32s31_compose::Esp32s31;
use oer_esp32s31_wifi::{DmaTypes, InterruptTypes, Interrupts};
use oer_espressif_executor::InterruptExecutor;
use oer_espressif_interrupt_table::signal::{FromHandler, FromTask, IrqSignal};

pub type RadioExecutor = InterruptExecutor<Esp32s31, 2>;
type Static = <Interrupts as DmaTypes>::Rx;
type Tx = <Interrupts as DmaTypes>::Tx;
oer_esp32s31_hal::dma_static! {
    pub static RX: Static = zeroed in ".dma.bss.wifi_rx";
    pub static TX0: Tx = zeroed in ".dma.bss.wifi_station_tx0";
    pub static TX1: Tx = zeroed in ".dma.bss.wifi_station_tx1";
    pub static TX2: Tx = zeroed in ".dma.bss.wifi_station_tx2";
    pub static TX3: Tx = zeroed in ".dma.bss.wifi_station_tx3";
    pub static TX4: Tx = zeroed in ".dma.bss.wifi_station_tx4";
    pub static TX5: Tx = zeroed in ".dma.bss.wifi_station_tx5";
    pub static TX6: Tx = zeroed in ".dma.bss.wifi_station_tx6";
    pub static TX7: Tx = zeroed in ".dma.bss.wifi_station_tx7";
    pub static TX8: Tx = zeroed in ".dma.bss.wifi_station_tx8";
}
oer_memory::zeroed_static! {
    pub static LINE: Interrupts = zeroed in ".critical.bss.wifi_rx_line";
}
oer_espressif_staged_runtime::placed_static! {
    pub static MAC_SIGNAL: IrqSignal<RadioExecutor> = IrqSignal::new::<(FromHandler<crate::WifiToken>, FromTask<oer_espressif_executor::Executor<Esp32s31, 0>, crate::ExecutorWake>), crate::RadioToken>(), in critical_data "wifi_station";
    pub static POWER_SIGNAL: IrqSignal<RadioExecutor> = IrqSignal::new::<FromHandler<crate::WifiToken>, crate::RadioToken>(), in critical_data "wifi_station";
    pub static ERRORS: AtomicU32 = AtomicU32::new(0), in critical_data "wifi_station";
}
pub type WifiPart = <Interrupts as InterruptTypes>::Part;
/// The radio interrupt handler (P3): the doorbell's requests, then the MAC
/// bank, then the WDEVPWR bank, one cause per entry.
pub fn on_wifi(part: &mut WifiPart) {
    if !match LINE.service(part) {
        Ok(installed) => installed,
        Err(raw) => oer_esp32s31_compose::wifi_fault::reset(raw),
    } {
        ERRORS.fetch_add(1, Ordering::Relaxed);
    }
}
