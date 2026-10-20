//! Static ISR owners and DMA allocations, as in W1.
use crate::{DoorbellToken, WifiMacToken, WifiPwrToken};
use core::sync::atomic::{AtomicU32, Ordering};
use esp_hal::interrupt::software::SoftwareInterrupt;
use oer_esp32s31_hal::ieee80211::dma::tx_storage::TxDmaStorage;
use oer_esp32s31_ieee80211_driver::{
    irq::{
        RadioDoorbell, WifiIrqLine, WifiIrqRoutes, run_doorbell, service_mac_line,
        service_power_line,
    },
    rx::arena::{WifiRxArena, WifiRxStatic},
};
use oer_esp32s31_ieee80211_mac::{
    irq::epoch::{ActiveEpoch, Moderated},
    tx::TxSlot,
};
use oer_espressif_executor::{Doorbell, DoorbellAcknowledger, InterruptExecutor, SameLevel};
use oer_interrupt_table::signal::IrqSignal;
use oer_memory::HandlerCell;

pub const N: usize = 32;
pub const B: usize = 1664;
pub const S: usize = 1668;
pub const Q: usize = 32;
pub const TX: usize = 512;
pub type Arena = WifiRxArena<N, B, S, Q>;
type Static = WifiRxStatic<N, B, S, Q>;
type Epoch = ActiveEpoch<Moderated>;

oer_esp32s31_hal::dma_static! {
    pub static RX: Static = zeroed in ".dma.bss.wifi_rx";
    pub static TX0: TxDmaStorage<TX> = zeroed in ".dma.bss.conformance_tx0";
    pub static TX1: TxDmaStorage<TX> = zeroed in ".dma.bss.conformance_tx1";
    pub static TX2: TxDmaStorage<TX> = zeroed in ".dma.bss.conformance_tx2";
    pub static TX3: TxDmaStorage<TX> = zeroed in ".dma.bss.conformance_tx3";
    pub static TX4: TxDmaStorage<TX> = zeroed in ".dma.bss.conformance_tx4";
    pub static TX5: TxDmaStorage<TX> = zeroed in ".dma.bss.conformance_tx5";
    pub static TX6: TxDmaStorage<TX> = zeroed in ".dma.bss.conformance_tx6";
    pub static TX7: TxDmaStorage<TX> = zeroed in ".dma.bss.conformance_tx7";
    pub static TX8: TxDmaStorage<TX> = zeroed in ".dma.bss.conformance_tx8";
}
oer_memory::zeroed_static! {
    pub static LINE: HandlerCell<WifiIrqLine<Epoch>> = zeroed in ".critical.bss.wifi_rx_line";
}
oer_espressif_staged_runtime::placed_static! {
    pub static MAC_SIGNAL: IrqSignal = IrqSignal::new(InterruptExecutor::<2>::SIGNALS), in critical_data "lower_mac";
    pub static POWER_SIGNAL: IrqSignal = IrqSignal::new(InterruptExecutor::<2>::SIGNALS), in critical_data "lower_mac";
    pub static ERRORS: AtomicU32 = AtomicU32::new(0), in critical_data "lower_mac";
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
static ACK: HandlerCell<DoorbellAcknowledger<3>> = HandlerCell::new();
const BELL: SameLevel<WifiMacToken, DoorbellToken> = SameLevel::new();

#[cfg(oer_debug_control)]
pub fn hold_completion(hold: bool) {
    use oer_esp32s31_ieee80211_mac::irq::{EVENT_TX_COMPLETE, EVENT_TX_TIMEOUT};
    MAC_SIGNAL.debug_hold(EVENT_TX_COMPLETE | EVENT_TX_TIMEOUT, hold);
}

#[esp_hal::ram]
pub fn wifi_mac() {
    if LINE.with(service_mac_line).is_none() {
        ERRORS.fetch_add(1, Ordering::Relaxed);
    }
}
#[esp_hal::ram]
pub fn wifi_pwr() {
    if LINE.with(service_power_line).is_none() {
        ERRORS.fetch_add(1, Ordering::Relaxed);
    }
}
#[esp_hal::ram]
pub fn doorbell() {
    let _ = ACK.with(|ack| run_doorbell(&LINE, || ack.acknowledge()));
}

pub struct ImageDoorbell(Doorbell<3>);
impl ImageDoorbell {
    pub fn new(interrupt: SoftwareInterrupt<'static, 3>, token: DoorbellToken) -> Self {
        static STORAGE: static_cell::StaticCell<DoorbellAcknowledger<3>> =
            static_cell::StaticCell::new();
        let (bell, ack) = Doorbell::<3>::new(interrupt, token, BELL);
        if ACK.install(STORAGE.init(ack)).is_err() {
            panic!("doorbell installed once");
        }
        ACK.with(DoorbellAcknowledger::enable)
            .expect("installed doorbell")
            .expect("doorbell route");
        Self(bell)
    }
}
impl RadioDoorbell for ImageDoorbell {
    fn ring(&self) {
        self.0.ring();
    }
}
pub struct ImageRoutes {
    pub mac: WifiMacToken,
    pub power: WifiPwrToken,
}
impl WifiIrqRoutes for ImageRoutes {
    fn enable(&mut self) {
        for result in [
            oer_espressif_esp_hal_irq::enable(&self.mac),
            oer_espressif_esp_hal_irq::enable(&self.power),
        ] {
            if result.is_err() {
                ERRORS.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    fn silence(&mut self) {
        oer_espressif_esp_hal_irq::disable(&self.mac);
        oer_espressif_esp_hal_irq::disable(&self.power);
    }
}
