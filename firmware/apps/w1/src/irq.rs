//! The receive epoch's statics, the radio interrupt handlers (P3), the
//! radio executor's handler (P2) and the receive runner's task.
//!
//! The arena and the line's cell are zeroed statics in SRAM; the handlers
//! reach the line only through its [`HandlerCell`], which is an ownership
//! state machine, not a lock. The runner binds the arena on P2, where it
//! runs, and sends the application its ends of the epoch.

use core::sync::atomic::{AtomicU32, Ordering};

use esp_hal::interrupt::software::SoftwareInterrupt;
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_hal::ieee80211::dma::rx_storage::{RxDmaStorage, RxDmaStorageError};
use oer_esp32s31_hal::owner::{MacInterruptSetup, RadioAccess};
use oer_esp32s31_ieee80211_driver::{
    irq::{
        RadioDoorbell, WifiIrqLine, WifiIrqRoutes, run_doorbell, service_mac_line,
        service_power_line,
    },
    rx::{
        arena::{WifiIrqSignals, WifiRxArena, WifiRxStatic},
        control::ReceiveStopHandle,
        counters::WifiRxCounters,
        receive::receive,
        sink::CountingAdmission,
        upper::{FrameReceiver, QueueSink, RxFrameEnds},
    },
};
use oer_espressif_executor::{
    Doorbell, DoorbellAcknowledger, InterruptExecutor, SameLevel,
    poll_timing::{CycleSource, Mcycle},
};
use oer_interrupt_table::signal::IrqSignal;
use oer_memory::{AffineSpscSender, HandlerCell, zeroed::ZeroedStatic};
use {
    oer_esp32s31_hal::ieee80211::init::MAC_STATION_INTERRUPT_MASK,
    oer_esp32s31_ieee80211_mac::irq::epoch::{ActiveEpoch, InactiveEpoch, Moderated},
};

use crate::{
    DoorbellToken, WifiMacToken, WifiPwrToken, measure,
    measure::{MAC_LATCH, RING_AT, RING_TO_DOORBELL_MAX},
};

/// RX descriptors.
pub const COUNT: usize = 32;
/// Bytes of one receive buffer.
pub const BUFFER: usize = 1_664;
/// Bytes of one buffer's storage, with the recycle guard.
pub const STORAGE: usize = 1_668;
/// Staging leases.
pub const SLOTS: usize = 32;

pub type RadioExecutor = InterruptExecutor<2>;

type Epoch = ActiveEpoch<Moderated>;
type Arena = WifiRxArena<COUNT, BUFFER, STORAGE, SLOTS, RadioExecutor>;
/// The arena and its RX DMA storage, placed together.
type Static = WifiRxStatic<COUNT, BUFFER, STORAGE, SLOTS, RadioExecutor>;
/// The arena's DMA storage.
pub type Storage = RxDmaStorage<COUNT, BUFFER, STORAGE>;

oer_esp32s31_hal::dma_static! {
    /// Placement: the receive epoch's arena, descriptors and buffers in the
    /// DMA window.
    static RX_ARENA: Static = zeroed in ".dma.bss.wifi_rx";
}

oer_memory::zeroed_static! {
    /// Placement: the line the three P3 handlers run, in critical SRAM.
    static RX_LINE: HandlerCell<WifiIrqLine<Epoch, RadioExecutor>> = zeroed in ".critical.bss.wifi_rx_line";
}

oer_espressif_staged_runtime::placed_static! {
    /// MAC handler entries without an installed line.
    pub static UNSERVICED_MAC: AtomicU32 = AtomicU32::new(0), in critical_data "w1";
    /// WDEVPWR handler entries without an installed line.
    pub static UNSERVICED_PWR: AtomicU32 = AtomicU32::new(0), in critical_data "w1";
    /// Refused route enables of the MAC and WDEVPWR sources.
    pub static ROUTE_ERRORS: AtomicU32 = AtomicU32::new(0), in critical_data "w1";
    /// The MAC facts the P3 line raises for the receive runner on the
    /// radio executor (P2).
    static MAC_SIGNAL: IrqSignal<RadioExecutor> = IrqSignal::new(), in critical_data "w1";
    /// The WDEVPWR facts the P3 line raises for the receive runner.
    static POWER_SIGNAL: IrqSignal<RadioExecutor> = IrqSignal::new(), in critical_data "w1";
}

static DOORBELL_ACK: HandlerCell<DoorbellAcknowledger<3>> = HandlerCell::new();

/// The doorbell shares the MAC interrupt's level and core.
pub const BELL: SameLevel<WifiMacToken, DoorbellToken> = SameLevel::new();
/// The WDEVPWR interrupt shares the MAC interrupt's level and core.
const _POWER: SameLevel<WifiMacToken, WifiPwrToken> = SameLevel::new();

/// The MAC interrupt (P3): latch the entry for the MAC-to-poll latency,
/// then run the line.
pub fn wifi_mac() {
    let _latched =
        MAC_LATCH.compare_exchange(0, measure::stamp(), Ordering::AcqRel, Ordering::Relaxed);
    if RX_LINE.with(service_mac_line).is_none() {
        UNSERVICED_MAC.fetch_add(1, Ordering::Relaxed);
    }
}

/// The WDEVPWR interrupt (P3).
pub fn wifi_pwr() {
    if RX_LINE.with(service_power_line).is_none() {
        UNSERVICED_PWR.fetch_add(1, Ordering::Relaxed);
    }
}

/// The radio doorbell (P3): the ring-to-handler latency, then the line's
/// requests.
pub fn doorbell() {
    let rung = RING_AT.swap(0, Ordering::AcqRel);
    if rung != 0 {
        measure::raise(&RING_TO_DOORBELL_MAX, Mcycle.now().wrapping_sub(rung));
    }
    let _acknowledged = DOORBELL_ACK.with(|ack| run_doorbell(&RX_LINE, || ack.acknowledge()));
}

/// The radio executor's software interrupt (P2): each run's cycles.
pub fn radio_executor() {
    let start = Mcycle.now();
    oer_espressif_executor::interrupt_handler::<crate::RadioTasks>();
    measure::record(&measure::SWI2, Mcycle.now().wrapping_sub(start));
}

/// The radio doorbell, its ring stamped.
pub struct ImageDoorbell(pub Doorbell<3>);

impl ImageDoorbell {
    /// The doorbell on `FROM_CPU_INTR3`.
    pub fn new(
        interrupt: SoftwareInterrupt<'static, 3>,
        token: DoorbellToken,
    ) -> Result<Self, oer_espressif_esp_hal_irq::Error> {
        static STORAGE: static_cell::StaticCell<DoorbellAcknowledger<3>> =
            static_cell::StaticCell::new();
        let (bell, ack) = Doorbell::<3>::new(interrupt, token, BELL);
        let ack = STORAGE.init(ack);
        // The route stays silent until its P3 acknowledgement owner is installed.
        if DOORBELL_ACK.install(ack).is_err() {
            panic!("the doorbell acknowledger is installed twice");
        }
        DOORBELL_ACK
            .with(DoorbellAcknowledger::enable)
            .expect("the doorbell owner was just installed")?;
        Ok(Self(bell))
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
            oer_espressif_esp_hal_irq::enable(&self.power),
            oer_espressif_esp_hal_irq::enable(&self.mac),
        ] {
            if routed.is_err() {
                ROUTE_ERRORS.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn silence(&mut self) {
        oer_espressif_esp_hal_irq::disable(&self.mac);
        oer_espressif_esp_hal_irq::disable(&self.power);
    }
}

/// The application's ends of the receive epoch.
pub struct P1Ends {
    pub stop: ReceiveStopHandle<'static>,
    pub receiver: FrameReceiver<'static, SLOTS, BUFFER, RadioExecutor>,
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
pub async fn rx_task(
    mut registers: RadioAccess,
    interrupt_setup: MacInterruptSetup,
    mut routes: ImageRoutes,
    doorbell: ImageDoorbell,
    bound: AffineSpscSender<'static, Bound, 1>,
) {
    let Some(arena) = RX_ARENA.take() else {
        let _full = bound.try_send(Bound::Claimed);
        return;
    };
    let (parts, stop, RxFrameEnds { sender, receiver }) = match Arena::bind(
        arena,
        WifiIrqSignals {
            mac: &MAC_SIGNAL,
            power: &POWER_SIGNAL,
        },
    ) {
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
        InactiveEpoch::new(interrupt_setup).activate_rx_moderated(MAC_STATION_INTERRUPT_MASK)
    };
    let (mut mac, rx_dma) = registers.split_rx_dma();
    let receiving = receive(
        &mut mac,
        rx_dma,
        activate,
        parts,
        &RX_LINE,
        &mut routes,
        &doorbell,
        &CLOCK,
        &publisher,
        &admission,
    );
    match measure::timed(
        core::pin::pin!(receiving),
        &measure::RECEIVE,
        measure::take_mac_latch,
    )
    .await {}
}
