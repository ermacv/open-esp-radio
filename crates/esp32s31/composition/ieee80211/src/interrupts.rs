//! Concrete esp-hal ISR bindings and shared Embassy interrupt publication.
//!
//! The supervisor transfers each typed interrupt setup into an epoch borrowing
//! these exact runtimes. Station, paired roles and diagnostics share the same
//! publication owners; role changes do not replace ISR state or route owners.
//! Handler code retains its IRAM placement while the runtime storage keeps its
//! original static lifetime and memory placement.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
#[cfg(feature = "mac-irq-diagnostics")]
use embassy_sync::once_lock::OnceLock;
#[cfg(feature = "mac-irq-diagnostics")]
use oer_esp32s31_ieee80211_mac::irq::{
    EVENT_COLLISION, EVENT_RX_SUCCESS, EVENT_TX_COMPLETE, EVENT_TX_TIMEOUT, IrqSink,
};

use oer_esp32s31_hal::owner::MacInterruptSetup;

use oer_esp32s31_ieee80211_runtime::datapath::irq::{
    EmbassyMacIrqRuntime, EmbassyPowerIrqRuntime, InterruptEpoch,
};

use oer_esp32s31_ieee80211_esp_hal::mac_interrupt_epoch::{
    EspHalMacInterruptRoute, service_mac_interrupt, service_power_interrupt,
};

pub type MacInterruptEpoch =
    InterruptEpoch<'static, EspHalMacInterruptRoute, CriticalSectionRawMutex>;

pub(crate) static IRQ_RUNTIME: EmbassyMacIrqRuntime<CriticalSectionRawMutex> =
    EmbassyMacIrqRuntime::new_with_rx_moderation(
        oer_esp32s31_ieee80211_esp_hal::mac_interrupt_epoch::unmask_active_mac_rx_delivery_interrupts,
    );
static POWER_IRQ_RUNTIME: EmbassyPowerIrqRuntime<CriticalSectionRawMutex> =
    EmbassyPowerIrqRuntime::new();

/// The power interrupt's executor handoff, for the station power agent.
pub(crate) fn power_irq_runtime() -> &'static EmbassyPowerIrqRuntime<CriticalSectionRawMutex> {
    &POWER_IRQ_RUNTIME
}

#[cfg(feature = "mac-irq-diagnostics")]
static MAC_IRQ_OBSERVER: OnceLock<fn(MacIrqObservation)> = OnceLock::new();

/// Value-only hard-IRQ observation exported only by diagnostics builds.
#[cfg(feature = "mac-irq-diagnostics")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacIrqObservation {
    RxEpoch,
    TxEpoch,
    Entry {
        had_status: bool,
        posted_events: u32,
        had_auxiliary_event: bool,
        had_unhandled_event: bool,
    },
}

#[cfg(feature = "mac-irq-diagnostics")]
pub(crate) fn configure_mac_irq_observer(observer: fn(MacIrqObservation)) {
    MAC_IRQ_OBSERVER
        .init(observer)
        .unwrap_or_else(|_| panic!("MAC IRQ observer was configured more than once"));
}

#[cfg(feature = "mac-irq-diagnostics")]
#[inline]
fn observe_mac_irq(observation: MacIrqObservation) {
    if let Some(observer) = MAC_IRQ_OBSERVER.try_get() {
        observer(observation);
    }
}

#[cfg(feature = "mac-irq-diagnostics")]
struct DiagnosticMacIrqSink;

#[cfg(feature = "mac-irq-diagnostics")]
impl IrqSink for DiagnosticMacIrqSink {
    #[inline]
    fn post(&self, pending: u32) {
        if pending & EVENT_RX_SUCCESS != 0 && !IRQ_RUNTIME.rx_signaled() {
            observe_mac_irq(MacIrqObservation::RxEpoch);
        }
        const TX_EVENTS: u32 = EVENT_TX_COMPLETE | EVENT_TX_TIMEOUT | EVENT_COLLISION;
        if pending & TX_EVENTS != 0 && !IRQ_RUNTIME.tx_signaled() {
            observe_mac_irq(MacIrqObservation::TxEpoch);
        }
        IRQ_RUNTIME.publish(pending);
    }

    #[inline]
    fn record_unhandled_event(&self) {
        IRQ_RUNTIME.record_unhandled_event();
    }

    #[inline]
    fn moderate_rx_success(&self) -> bool {
        IrqSink::moderate_rx_success(&IRQ_RUNTIME)
    }
}

/// The interrupt-table handler of `MODEM_WIFI_MAC`.
// The handler must execute while flash access may be unavailable. This is a
// declarative linker placement; it performs no raw memory operation.
#[allow(
    unsafe_code,
    reason = "an interrupt handler runs from SRAM, which only a link section selects"
)]
#[unsafe(link_section = ".rwtext.open_radio_irq")]
pub fn mac_interrupt() {
    #[cfg(feature = "task-poll-telemetry")]
    let core0_cycle_started =
        oer_esp32s31_ieee80211_runtime::diagnostics::core0_rx_cycles::cycle_count();
    #[cfg(feature = "mac-irq-diagnostics")]
    let report = service_mac_interrupt(&DiagnosticMacIrqSink);
    #[cfg(not(feature = "mac-irq-diagnostics"))]
    let _report = service_mac_interrupt(&IRQ_RUNTIME);
    #[cfg(feature = "mac-irq-diagnostics")]
    observe_mac_irq(MacIrqObservation::Entry {
        had_status: report.had_status,
        posted_events: report.posted_events,
        had_auxiliary_event: report.had_auxiliary_event,
        had_unhandled_event: report.had_unhandled_event,
    });
    #[cfg(feature = "task-poll-telemetry")]
    {
        use oer_esp32s31_ieee80211_runtime::diagnostics::core0_rx_cycles::{
            CORE0_RX_CYCLES, cycle_count,
        };

        CORE0_RX_CYCLES.record_mac_irq(cycle_count().wrapping_sub(core0_cycle_started));
    }
}

/// The interrupt-table handler of `MODEM_WIFI_PWR`.
#[allow(
    unsafe_code,
    reason = "an interrupt handler runs from SRAM, which only a link section selects"
)]
#[unsafe(link_section = ".rwtext.open_radio_irq")]
pub fn power_interrupt() {
    let _ = service_power_interrupt(&POWER_IRQ_RUNTIME);
}

/// Construct the reusable interrupt epoch retained by the radio supervisor.
pub(crate) fn mac_interrupt_epoch(setup: MacInterruptSetup) -> MacInterruptEpoch {
    InterruptEpoch::new(
        EspHalMacInterruptRoute::new(),
        setup,
        &IRQ_RUNTIME,
        &POWER_IRQ_RUNTIME,
    )
}
