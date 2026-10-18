//! The receive epoch's statics, the radio interrupt handlers (P3), the
//! radio executor's handler (P2) and the receive runner's task.
//!
//! The arena and the line's cell are zeroed statics in SRAM; the handlers
//! reach the line only through its [`HandlerCell`], which is an ownership
//! state machine, not a lock. The runner binds the arena on P2, where it
//! runs, and sends the application its ends of the epoch.

use core::sync::atomic::{AtomicU32, Ordering};

use esp_hal::interrupt::software::SoftwareInterrupt;
use oer_adapters_embassy_time::EmbassyClock;
use oer_base_memory::{AffineSpscSender, HandlerCell, zeroed::ZeroedStatic};
use oer_esp32s31_hal::owner::{MacInterruptSetup, RadioRuntimeOwner};
use oer_esp32s31_hal::ieee80211::dma::rx_storage::{RxDmaStorage, RxDmaStorageError};
use oer_esp32s31_ieee80211_driver::{
    irq::{
        RadioDoorbell, WifiIrqLine, WifiIrqRoutes, run_doorbell, service_mac_line,
        service_power_line,
    },
    rx::{
        arena::WifiRxArena,
        control::ReceiveStopHandle,
        counters::WifiRxCounters,
        receive::receive,
        sink::CountingAdmission,
        upper::{FrameReceiver, QueueSink, RxFrameEnds},
    },
};
use oer_esp32s31_ieee80211_mac::{
    init::MAC_COLD_RX_INTERRUPT_MASK,
    irq::epoch::{ActiveEpoch, InactiveEpoch, Moderated},
};
use oer_exec_executor::{
    Doorbell, SameLevel,
    poll_timing::{CycleSource, Mcycle},
};

use crate::{
    DoorbellToken, WifiMacToken, WifiPwrToken,
    measure::{self, MAC_LATCH, RING_AT, RING_TO_DOORBELL_MAX},
};

/// RX descriptors.
pub const COUNT: usize = 32;
/// Bytes of one receive buffer.
pub const BUFFER: usize = 1_664;
/// Bytes of one buffer's storage, with the recycle guard.
pub const STORAGE: usize = 1_668;
/// Staging leases.
pub const SLOTS: usize = 32;

type Epoch = ActiveEpoch<Moderated>;
type Arena = WifiRxArena<COUNT, BUFFER, STORAGE, SLOTS>;
/// The arena's DMA storage.
pub type Storage = RxDmaStorage<COUNT, BUFFER, STORAGE>;

oer_esp32s31_hal::dma_static! {
    /// Placement: the receive epoch's arena, descriptors and buffers in the
    /// DMA window.
    static RX_ARENA: Arena = zeroed in ".dma.bss.wifi_rx";
}

oer_base_memory::zeroed_static! {
    /// Placement: the line the three P3 handlers run, in critical SRAM.
    static RX_LINE: HandlerCell<WifiIrqLine<Epoch>> = zeroed in ".critical.bss.wifi_rx_line";
}

/// MAC handler entries without an installed line.
#[unsafe(link_section = ".critical.data.w1")]
pub static UNSERVICED_MAC: AtomicU32 = AtomicU32::new(0);
/// WDEVPWR handler entries without an installed line.
#[unsafe(link_section = ".critical.data.w1")]
pub static UNSERVICED_PWR: AtomicU32 = AtomicU32::new(0);
/// Refused route enables of the MAC and WDEVPWR sources.
#[unsafe(link_section = ".critical.data.w1")]
pub static ROUTE_ERRORS: AtomicU32 = AtomicU32::new(0);

/// The doorbell shares the MAC interrupt's level and core.
pub const BELL: SameLevel<WifiMacToken, DoorbellToken> = SameLevel::new();
/// The WDEVPWR interrupt shares the MAC interrupt's level and core.
const _POWER: SameLevel<WifiMacToken, WifiPwrToken> = SameLevel::new();

/// The MAC interrupt (P3): latch the entry for the MAC-to-poll latency,
/// then run the line.
#[esp_hal::ram]
pub fn wifi_mac() {
    let _latched =
        MAC_LATCH.compare_exchange(0, measure::stamp(), Ordering::AcqRel, Ordering::Relaxed);
    if RX_LINE.with(service_mac_line).is_none() {
        UNSERVICED_MAC.fetch_add(1, Ordering::Relaxed);
    }
}

/// The WDEVPWR interrupt (P3).
#[esp_hal::ram]
pub fn wifi_pwr() {
    if RX_LINE.with(service_power_line).is_none() {
        UNSERVICED_PWR.fetch_add(1, Ordering::Relaxed);
    }
}

/// The radio doorbell (P3): the ring-to-handler latency, then the line's
/// requests.
#[esp_hal::ram]
pub fn doorbell() {
    let rung = RING_AT.swap(0, Ordering::AcqRel);
    if rung != 0 {
        measure::raise(&RING_TO_DOORBELL_MAX, Mcycle.now().wrapping_sub(rung));
    }
    let _ran = run_doorbell(&RX_LINE, oer_exec_executor::acknowledge::<3>);
}

/// The radio executor's software interrupt (P2): each run's cycles.
#[esp_hal::ram]
pub fn radio_executor() {
    let start = Mcycle.now();
    oer_exec_executor::interrupt_handler::<2>();
    measure::record(&measure::SWI2, Mcycle.now().wrapping_sub(start));
}

/// The radio doorbell, its ring stamped.
pub struct ImageDoorbell(pub Doorbell<3>);

impl ImageDoorbell {
    /// The doorbell on `FROM_CPU_INTR3`.
    pub fn new(
        interrupt: SoftwareInterrupt<'static, 3>,
        token: DoorbellToken,
    ) -> Result<Self, oer_exec_esp_hal_irq::Error> {
        Doorbell::<3>::new(interrupt, token, BELL).map(Self)
    }
}

impl RadioDoorbell for ImageDoorbell {
    fn ring(&self) {
        // The handler takes the stamp; a ring before it took the last one
        // keeps the earlier stamp.
        let _stamped =
            RING_AT.compare_exchange(0, measure::stamp(), Ordering::AcqRel, Ordering::Relaxed);
        self.0.ring();
    }
}

/// The routes of the MAC and WDEVPWR sources.
pub struct ImageRoutes {
    pub mac: WifiMacToken,
    pub power: WifiPwrToken,
}

impl WifiIrqRoutes for ImageRoutes {
    fn enable(&mut self) {
        for routed in [
            oer_exec_esp_hal_irq::enable(&self.power),
            oer_exec_esp_hal_irq::enable(&self.mac),
        ] {
            if routed.is_err() {
                ROUTE_ERRORS.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn silence(&mut self) {
        oer_exec_esp_hal_irq::disable(&self.mac);
        oer_exec_esp_hal_irq::disable(&self.power);
    }
}

/// The application's ends of the receive epoch.
pub struct P1Ends {
    pub stop: ReceiveStopHandle<'static>,
    pub receiver: FrameReceiver<'static, SLOTS, BUFFER>,
    pub counters: &'static WifiRxCounters,
    /// The DMA address of descriptor 0, as the bind laid the arena out.
    pub ring_base: u32,
    /// The arena's DMA storage, whose descriptors the hold reads.
    pub storage: &'static Storage,
}

/// What the receive runner sends the application after its bind.
pub enum Bound {
    Ends(P1Ends),
    Refused(RxDmaStorageError),
    /// The arena was already claimed: the task ran twice.
    Claimed,
}

/// The handoff clock of the staged frames.
static CLOCK: EmbassyClock = EmbassyClock;

/// The receive runner on the radio executor (P2): bind the arena, send the
/// application its ends, then receive until the stop and park.
#[embassy_executor::task]
pub async fn rx_task(
    mut registers: RadioRuntimeOwner,
    interrupt_setup: MacInterruptSetup,
    mut routes: ImageRoutes,
    doorbell: ImageDoorbell,
    bound: AffineSpscSender<'static, Bound, 1>,
) {
    let Some(arena) = RX_ARENA.take() else {
        let _full = bound.try_send(Bound::Claimed);
        return;
    };
    let (parts, stop, RxFrameEnds { sender, receiver }) = match Arena::bind(arena) {
        Ok(parts) => parts,
        Err(error) => {
            let _full = bound.try_send(Bound::Refused(error));
            return;
        }
    };
    let counters = parts.counters();
    let _full = bound.try_send(Bound::Ends(P1Ends {
        stop,
        receiver,
        counters,
        ring_base: parts.descriptor_base(),
        storage: parts.rx(),
    }));
    let publisher = QueueSink::new(sender, &CLOCK);
    let admission = CountingAdmission::new(counters);
    let activate = move || {
        InactiveEpoch::new(interrupt_setup).activate_rx_moderated(MAC_COLD_RX_INTERRUPT_MASK)
    };
    let receiving = receive(
        &mut registers,
        activate,
        parts,
        &RX_LINE,
        &mut routes,
        &doorbell,
        &CLOCK,
        &publisher,
        &admission,
    );
    match measure::timed(receiving, &measure::RECEIVE, measure::take_mac_latch).await {}
}
