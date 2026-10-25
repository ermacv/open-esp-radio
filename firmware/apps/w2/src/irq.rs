//! Static ISR owners and DMA allocations, as in W1: the radio handler's part
//! (both MAC banks and the doorbell, bound once) and the line it serves.
use core::sync::atomic::{AtomicU32, Ordering};
use oer_esp32s31_hal::ieee80211::dma::tx_storage::TxDmaStorage;
use oer_esp32s31_ieee80211_driver::{
    irq::{DoorbellAcknowledge, RadioDoorbell, WifiIrqLine, WifiIrqPart, service_wifi_line},
    rx::arena::{WifiRxArena, WifiRxStatic},
};
use oer_esp32s31_ieee80211_mac::tx::TxSlot;
use oer_espressif_executor::{Doorbell, DoorbellAcknowledger, InterruptExecutor};
use oer_interrupt_table::signal::IrqSignal;
use oer_memory::HandlerCell;

pub const N: usize = 32;
pub const B: usize = 1664;
pub const S: usize = 1668;
pub const Q: usize = 32;
pub const TX: usize = 512;
pub type Arena = WifiRxArena<N, B, S, Q, RadioExecutor>;
type Static = WifiRxStatic<N, B, S, Q, RadioExecutor>;
pub type RadioExecutor = InterruptExecutor<2>;

oer_esp32s31_hal::dma_static! {
    pub static RX: Static = zeroed in ".dma.bss.wifi_rx";
    pub static TX_DMA: TxDmaStorage<TX> = zeroed in ".dma.bss.wifi_tx";
}
oer_memory::zeroed_static! {
    pub static LINE: HandlerCell<WifiIrqLine<RadioExecutor>> = zeroed in ".critical.bss.wifi_rx_line";
}
oer_espressif_staged_runtime::placed_static! {
    pub static MAC_SIGNAL: IrqSignal<RadioExecutor> = IrqSignal::new(), in critical_data "w2";
    pub static POWER_SIGNAL: IrqSignal<RadioExecutor> = IrqSignal::new(), in critical_data "w2";
    pub static ERRORS: AtomicU32 = AtomicU32::new(0), in critical_data "w2";
}
pub static TX_SLOT: static_cell::StaticCell<TxSlot<TX>> = static_cell::StaticCell::new();

/// The doorbell's lowering half in the radio handler's part.
pub struct Lower(pub DoorbellAcknowledger<3>);

impl DoorbellAcknowledge for Lower {
    fn acknowledge(&mut self) {
        self.0.acknowledge();
    }
}

/// The radio handler's part: both MAC banks and the doorbell.
pub type WifiPart = WifiIrqPart<Lower>;

/// The radio handler (P3): it serves the line over the part its entry holds;
/// an event before the line is installed is counted.
pub fn on_wifi(part: &mut WifiPart) {
    if !service_wifi_line(&LINE, part) {
        ERRORS.fetch_add(1, Ordering::Relaxed);
    }
}

/// The radio doorbell's ringing half, made from the binding of [`WifiPart`].
pub struct ImageDoorbell(pub Doorbell<3>);

impl RadioDoorbell for ImageDoorbell {
    fn ring(&self) {
        self.0.ring();
    }
}
