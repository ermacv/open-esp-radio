//! The receive epoch's statics, the radio interrupt handlers (P3), the
//! radio executor's handler (P2) and the receive runner's task.
//!
//! The arena and the line's cell are zeroed statics in SRAM; the handlers
//! reach the line only through its [`HandlerCell`], which is an ownership
//! state machine, not a lock. The runner binds the arena on P2, where it
//! runs, and sends the application its ends of the epoch.

use oer_esp32s31_compose::Esp32s31;
use core::sync::atomic::{AtomicU32, Ordering};

use oer_embassy_time::EmbassyClock;
use oer_esp32s31_hal::ieee80211::dma::rx_storage::{RxDmaStorage, RxDmaStorageError};
use oer_esp32s31_hal::owner::{MacInterruptRegisters, MacPowerInterruptRegisters, RadioAccess};
use oer_esp32s31_hal::platform::{FromCpu2Irq, interrupt_controller};
use oer_esp32s31_ieee80211_driver::{
    irq::{
        DoorbellAcknowledge, RadioDoorbell, WifiIrqLine, WifiIrqPart, WifiIrqRequests,
        service_wifi_line,
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
    Doorbell, DoorbellAcknowledger, InterruptExecutor,
    poll_timing::{CycleSource, Mcycle},
};
use oer_espressif_interrupt_table::InterruptSource;
use oer_espressif_interrupt_table::{
    Bound as Binding,
    signal::{FromHandler, FromTask, IrqSignal},
};
use oer_memory::{AffineSpscSender, HandlerCell};

use crate::{
    measure,
    measure::{MAC_LATCH, RING_AT, RING_TO_DOORBELL_MAX},
};

/// RX descriptors.
pub const COUNT: usize = 32;
/// Bytes of one receive buffer.
pub const BUFFER: usize = 1_664;
/// Backing bytes of one buffer; its leading guard is inside `BUFFER`.
pub const STORAGE: usize = BUFFER;
/// Staging leases.
pub const SLOTS: usize = 32;

pub type RadioExecutor = InterruptExecutor<Esp32s31, 2>;

type Arena = WifiRxArena<COUNT, BUFFER, STORAGE, SLOTS, RadioExecutor>;
/// The arena and its RX DMA storage, placed together.
type Static = WifiRxStatic<COUNT, BUFFER, STORAGE, SLOTS, RadioExecutor>;
/// The arena's DMA storage.
pub type Storage = RxDmaStorage<COUNT, BUFFER, STORAGE>;

oer_esp32s31_hal::memory::dma_static! {
    /// Placement: the receive epoch's arena, descriptors and buffers in the
    /// DMA window.
    static RX_ARENA: Static = zeroed in ".dma.bss.wifi_rx";
}

oer_memory::zeroed_static! {
    /// Placement: the line the radio handler runs, in critical SRAM.
    static RX_LINE: HandlerCell<WifiIrqLine<RadioExecutor>> = zeroed in ".critical.bss.wifi_rx_line";
}

oer_espressif_staged_runtime::placed_static! {
    /// Radio handler entries for a pending MAC event without an installed
    /// line.
    pub static UNSERVICED_MAC: AtomicU32 = AtomicU32::new(0), in critical_data "w1";
    /// Radio handler entries for a pending WDEVPWR event without an
    /// installed line.
    pub static UNSERVICED_PWR: AtomicU32 = AtomicU32::new(0), in critical_data "w1";
    /// The MAC facts the P3 line raises for the receive runner on the
    /// radio executor (P2).
    static MAC_SIGNAL: IrqSignal<RadioExecutor> = IrqSignal::new::<(FromHandler<crate::WifiToken>, FromTask<oer_espressif_executor::Executor<Esp32s31, 0>, crate::WakeToken>), crate::RadioToken>(), in critical_data "w1";
    /// The WDEVPWR facts the P3 line raises for the receive runner.
    static POWER_SIGNAL: IrqSignal<RadioExecutor> = IrqSignal::new::<FromHandler<crate::WifiToken>, crate::RadioToken>(), in critical_data "w1";
}

/// The doorbell's lowering half, which the radio handler's part holds.
pub struct Lower(pub DoorbellAcknowledger<Esp32s31, 3>);

impl DoorbellAcknowledge for Lower {
    fn acknowledge(&mut self) {
        self.0.acknowledge();
    }
}

/// The radio handler's part: both Wi-Fi interrupt banks, the doorbell's
/// part and its lowering half.
pub type WifiPart = WifiIrqPart<Lower>;

/// The radio interrupt (P3), which the MAC, WDEVPWR and doorbell sources
/// share: the ring-to-handler latency of a doorbell entry or the latch of
/// a bank entry for the MAC-to-poll latency, then the line.
pub fn on_wifi(part: &mut WifiPart) {
    let rung = RING_AT.swap(0, Ordering::AcqRel);
    if rung != 0 {
        measure::raise(&RING_TO_DOORBELL_MAX, Mcycle.now().wrapping_sub(rung));
    }
    let hart = oer_esp32s31_hal::platform::Hart::constant::<
        { <crate::WifiToken as oer_espressif_interrupt_table::Entry>::HART as usize },
    >();
    let mac = interrupt_controller::is_pending(hart, MacInterruptRegisters::SOURCE);
    let power = interrupt_controller::is_pending(hart, MacPowerInterruptRegisters::SOURCE);
    if mac {
        let _latched =
            MAC_LATCH.compare_exchange(0, measure::stamp(), Ordering::AcqRel, Ordering::Relaxed);
    }
    if !match service_wifi_line(&RX_LINE, part) {
        Ok(installed) => installed,
        Err(oer_esp32s31_ieee80211_mac::irq::Fault::FatalAssertion { raw }) => {
            oer_esp32s31_compose::wifi_fault::reset(raw)
        }
    } {
        if mac {
            UNSERVICED_MAC.fetch_add(1, Ordering::Relaxed);
        } else if power {
            UNSERVICED_PWR.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// The radio executor's software interrupt (P2): each run's cycles.
pub fn radio_executor(part: &mut FromCpu2Irq) {
    let start = Mcycle.now();
    oer_espressif_executor::interrupt_handler::<crate::RadioTasks, FromCpu2Irq>(part);
    measure::record(&measure::SWI2, Mcycle.now().wrapping_sub(start));
}

/// The radio doorbell, its ring stamped.
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
        // The handler takes the stamp; a ring before it took the last one
        // keeps the earlier stamp.
        let _stamped =
            RING_AT.compare_exchange(0, measure::stamp(), Ordering::AcqRel, Ordering::Relaxed);
        self.0.ring();
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
    /// The line's requests, with the MAC interrupt enable image it read.
    pub requests: &'static WifiIrqRequests,
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
    bound_part: Binding<crate::WifiToken>,
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
        requests: parts.requests(),
    }));
    let publisher = QueueSink::new(sender, &CLOCK);
    let admission = CountingAdmission::new(counters);
    let (mut mac, rx_dma) = registers.split_rx_dma();
    let receiving = receive(
        &mut mac,
        rx_dma,
        &bound_part,
        parts,
        &RX_LINE,
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
