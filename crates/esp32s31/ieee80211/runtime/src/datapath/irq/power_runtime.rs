use core::sync::atomic::{AtomicBool, Ordering};

use embassy_sync::{blocking_mutex::raw::RawMutex, signal::Signal};

use oer_esp32s31_hal::types::MacPowerInterruptObservation;

use oer_esp32s31_ieee80211_mac::irq::PowerIrqSink;

/// Embassy handoff for acknowledged DATAPATHPWR snapshots.
///
/// The PAC decodes the reviewed TSF-timer and station TBTT fields before
/// this boundary. Unknown
/// fields remain one semantic flag and never escape as a register image.
pub struct EmbassyPowerIrqRuntime<M: RawMutex> {
    signal: Signal<M, ()>,
    tsf_timer_0: AtomicBool,
    tsf_timer_1: AtomicBool,
    tsf_timer_2: AtomicBool,
    tsf_timer_3: AtomicBool,
    sta_tbtt: AtomicBool,
    unhandled_event: AtomicBool,
}

impl<M: RawMutex> EmbassyPowerIrqRuntime<M> {
    pub const fn new() -> Self {
        Self {
            signal: Signal::new(),
            tsf_timer_0: AtomicBool::new(false),
            tsf_timer_1: AtomicBool::new(false),
            tsf_timer_2: AtomicBool::new(false),
            tsf_timer_3: AtomicBool::new(false),
            sta_tbtt: AtomicBool::new(false),
            unhandled_event: AtomicBool::new(false),
        }
    }

    #[inline]
    pub fn publish(&self, observation: MacPowerInterruptObservation) {
        self.tsf_timer_0
            .fetch_or(observation.tsf_timer_0(), Ordering::Release);
        self.tsf_timer_1
            .fetch_or(observation.tsf_timer_1(), Ordering::Release);
        self.tsf_timer_2
            .fetch_or(observation.tsf_timer_2(), Ordering::Release);
        self.tsf_timer_3
            .fetch_or(observation.tsf_timer_3(), Ordering::Release);
        self.sta_tbtt
            .fetch_or(observation.sta_tbtt(), Ordering::Release);
        self.unhandled_event
            .fetch_or(observation.has_unhandled_event(), Ordering::Release);
        self.signal.signal(());
    }

    fn take_observation(&self) -> MacPowerInterruptObservation {
        MacPowerInterruptObservation::from_semantic_events(
            self.tsf_timer_0.swap(false, Ordering::Acquire),
            self.tsf_timer_1.swap(false, Ordering::Acquire),
            self.tsf_timer_2.swap(false, Ordering::Acquire),
            self.tsf_timer_3.swap(false, Ordering::Acquire),
            self.sta_tbtt.swap(false, Ordering::Acquire),
            self.unhandled_event.swap(false, Ordering::Acquire),
        )
    }

    /// Wait for and consume the coalesced semantic WDEVPWR causes.
    pub async fn wait(&self) -> MacPowerInterruptObservation {
        self.signal.wait().await;
        self.take_observation()
    }

    /// Consume pending semantic causes without blocking the executor.
    pub fn try_take(&self) -> Option<MacPowerInterruptObservation> {
        self.signal.try_take()?;
        Some(self.take_observation())
    }

    /// Remove one stale power wake and its coalesced semantic causes
    /// after the platform interrupt route has been quiesced.
    pub fn drain_pending(&self) -> MacPowerInterruptObservation {
        let _wake = self.signal.try_take();
        self.take_observation()
    }
}

impl<M: RawMutex> PowerIrqSink for EmbassyPowerIrqRuntime<M> {
    #[inline]
    fn post_power(&self, observation: MacPowerInterruptObservation) {
        self.publish(observation);
    }
}

impl<M: RawMutex> Default for EmbassyPowerIrqRuntime<M> {
    fn default() -> Self {
        Self::new()
    }
}
