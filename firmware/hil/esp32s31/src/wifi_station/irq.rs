//! Static ISR owners and DMA allocations, as in W1.
use oer_esp32s31_compose::Esp32s31;
use core::sync::atomic::{AtomicU32, Ordering};
use oer_esp32s31_hal::ieee80211::dma::tx_storage::TxDmaStorage;
use oer_esp32s31_ieee80211_driver::{
    irq::{DoorbellAcknowledge, RadioDoorbell, WifiIrqLine, WifiIrqPart, service_wifi_line},
    rx::arena::{WifiRxArena, WifiRxStatic},
};
use oer_esp32s31_ieee80211_mac::tx::TxSlot;
use oer_espressif_executor::{Doorbell, DoorbellAcknowledger, InterruptExecutor};
use oer_espressif_interrupt_table::{
    Bound,
    signal::{FromHandler, FromTask, IrqSignal},
};
use oer_memory::HandlerCell;

pub const N: usize = 32;
pub const B: usize = 1664;
pub const S: usize = B;
pub const Q: usize = 32;
pub const TX: usize = 1664;
pub type RadioExecutor = InterruptExecutor<Esp32s31, 2>;
pub type Arena = WifiRxArena<N, B, S, Q, RadioExecutor>;
type Static = WifiRxStatic<N, B, S, Q, RadioExecutor>;

oer_esp32s31_hal::dma_static! {
    pub static RX: Static = zeroed in ".dma.bss.wifi_rx";
    pub static TX0: TxDmaStorage<TX> = zeroed in ".dma.bss.wifi_station_tx0";
    pub static TX1: TxDmaStorage<TX> = zeroed in ".dma.bss.wifi_station_tx1";
    pub static TX2: TxDmaStorage<TX> = zeroed in ".dma.bss.wifi_station_tx2";
    pub static TX3: TxDmaStorage<TX> = zeroed in ".dma.bss.wifi_station_tx3";
    pub static TX4: TxDmaStorage<TX> = zeroed in ".dma.bss.wifi_station_tx4";
    pub static TX5: TxDmaStorage<TX> = zeroed in ".dma.bss.wifi_station_tx5";
    pub static TX6: TxDmaStorage<TX> = zeroed in ".dma.bss.wifi_station_tx6";
    pub static TX7: TxDmaStorage<TX> = zeroed in ".dma.bss.wifi_station_tx7";
    pub static TX8: TxDmaStorage<TX> = zeroed in ".dma.bss.wifi_station_tx8";
}
oer_memory::zeroed_static! {
    pub static LINE: HandlerCell<WifiIrqLine<RadioExecutor>> = zeroed in ".critical.bss.wifi_rx_line";
}
oer_espressif_staged_runtime::placed_static! {
    pub static MAC_SIGNAL: IrqSignal<RadioExecutor> = IrqSignal::new::<(FromHandler<crate::WifiToken>, FromTask<oer_espressif_executor::Executor<Esp32s31, 0>, crate::ExecutorWake>), crate::RadioToken>(), in critical_data "wifi_station";
    pub static POWER_SIGNAL: IrqSignal<RadioExecutor> = IrqSignal::new::<FromHandler<crate::WifiToken>, crate::RadioToken>(), in critical_data "wifi_station";
    pub static ERRORS: AtomicU32 = AtomicU32::new(0), in critical_data "wifi_station";
}
pub static SLOT0: static_cell::StaticCell<TxSlot<TX>> = static_cell::StaticCell::new();
pub static SLOT1: static_cell::StaticCell<TxSlot<TX>> = static_cell::StaticCell::new();
pub static SLOT2: static_cell::StaticCell<TxSlot<TX>> = static_cell::StaticCell::new();
pub static SLOT3: static_cell::StaticCell<TxSlot<TX>> = static_cell::StaticCell::new();
pub static SLOT4: static_cell::StaticCell<TxSlot<TX>> = static_cell::StaticCell::new();
pub static SLOT5: static_cell::StaticCell<TxSlot<TX>> = static_cell::StaticCell::new();
pub static SLOT6: static_cell::StaticCell<TxSlot<TX>> = static_cell::StaticCell::new();
pub static SLOT7: static_cell::StaticCell<TxSlot<TX>> = static_cell::StaticCell::new();
pub static SLOT8: static_cell::StaticCell<TxSlot<TX>> = static_cell::StaticCell::new();
/// The radio doorbell's lowering half, which the radio handler's part holds.
pub struct Lower(pub DoorbellAcknowledger<Esp32s31, 3>);

impl DoorbellAcknowledge for Lower {
    fn acknowledge(&mut self) {
        self.0.acknowledge();
    }
}

/// The radio handler's part: both MAC interrupt banks and the doorbell.
pub type WifiPart = WifiIrqPart<Lower>;

/// The router's proof that the image binds the radio handler's part.
pub type WifiBound = Bound<crate::WifiToken>;

/// The radio interrupt handler (P3): the doorbell's requests, then the MAC
/// bank, then the WDEVPWR bank, one cause per entry.
pub fn on_wifi(part: &mut WifiPart) {
    if !match service_wifi_line(&LINE, part) {
        Ok(installed) => installed,
        Err(oer_esp32s31_ieee80211_mac::irq::Fault::FatalAssertion { raw }) => {
            oer_esp32s31_compose::wifi_fault::reset(raw)
        }
    } {
        ERRORS.fetch_add(1, Ordering::Relaxed);
    }
}

/// The radio doorbell's ringing half.
pub struct ImageDoorbell<
    E: oer_espressif_interrupt_table::Entry = crate::WifiToken,
    X: oer_espressif_interrupt_table::Entry = crate::RadioToken,
>(pub Doorbell<Esp32s31, 3, E, X>);

impl<E: oer_espressif_interrupt_table::Entry, X: oer_espressif_interrupt_table::Entry> RadioDoorbell
    for ImageDoorbell<E, X>
{
    const HANDLER: oer_espressif_interrupt_table::Context = Doorbell::<Esp32s31, 3, E, X>::HANDLER;
    const EXECUTOR: oer_espressif_interrupt_table::Context = Doorbell::<Esp32s31, 3, E, X>::EXECUTOR;
    fn ring(&self) {
        self.0.ring();
    }
}
