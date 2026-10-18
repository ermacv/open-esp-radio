//! Optional, nonblocking allocation recorder. Contention invalidates evidence
//! instead of delaying another core or silently dropping ownership edges.

use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicBool, Ordering},
};
use oer_esp32s31_ieee80211_dma::rx_observation::{RxOwnershipEdge, RxOwnershipObserver};
use oer_hil_esp32s31_telemetry::rx_ownership::{Snapshot, Tracker};

// Size follows the production arena without storing payloads.
const CAPACITY: usize =
    oer_esp32s31_ieee80211_system::resources::profile::ESP32S31_DEFAULT_RX_DESCRIPTOR_COUNT;

pub(super) static RECORDER: Recorder = Recorder {
    busy: AtomicBool::new(false),
    lost: AtomicBool::new(false),
    tracker: UnsafeCell::new(Tracker::new()),
};

pub(super) struct Recorder {
    busy: AtomicBool,
    lost: AtomicBool,
    tracker: UnsafeCell<Tracker<CAPACITY>>,
}

#[allow(
    unsafe_code,
    reason = "try-lock exclusively protects the bounded recorder"
)]
unsafe impl Sync for Recorder {}

impl Recorder {
    fn with<R>(&self, operation: impl FnOnce(&mut Tracker<CAPACITY>) -> R) -> Option<R> {
        if self
            .busy
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            self.lost.store(true, Ordering::Release);
            return None;
        }
        // SAFETY: successful try-lock is exclusive across cores and interrupts.
        // Operations below are finite value updates, never radio calls or I/O.
        #[allow(unsafe_code, reason = "successful atomic try-lock owns the recorder")]
        let result = operation(unsafe { &mut *self.tracker.get() });
        self.busy.store(false, Ordering::Release);
        Some(result)
    }
}

impl RxOwnershipObserver for Recorder {
    fn observe(&self, arena: usize, buffer: usize, edge: RxOwnershipEdge) {
        self.with(|tracker| {
            tracker.observe(
                arena,
                buffer,
                edge,
                embassy_time::Instant::now().as_micros(),
            )
        });
    }
}

pub(super) fn begin() {
    RECORDER.with(Tracker::begin);
}

pub(super) async fn report() -> bool {
    let snapshot = RECORDER.with(|tracker| tracker.snapshot());
    let missing = snapshot.is_none();
    let Snapshot {
        hold,
        returned_to_append,
        carry_hold,
        carry_returned_to_append,
        held,
        returned,
        maximum_held,
        maximum_returned,
        carry_in,
        reclaimed_while_stopped,
        invalid,
    } = snapshot.unwrap_or_default();
    let valid = !missing
        && !invalid
        && !RECORDER.lost.load(Ordering::Acquire)
        && hold.samples != 0
        && returned_to_append.samples != 0;
    crate::console::runtime_log_reliably(format_args!(
        "ORX_OWN valid={} carry={} held={} returned={} peak_held={} peak_returned={} stopped={}",
        valid, carry_in, held, returned, maximum_held, maximum_returned, reclaimed_while_stopped,
    ))
    .await;
    crate::console::runtime_log_reliably(format_args!(
        "ORX_OWN_TIME hold_n={} hold_total_us={} hold_max_us={} append_n={} append_total_us={} append_max_us={}",
        hold.samples,
        hold.total_micros,
        hold.maximum_micros,
        returned_to_append.samples,
        returned_to_append.total_micros,
        returned_to_append.maximum_micros,
    )).await;
    crate::console::runtime_log_reliably(format_args!(
        "ORX_OWN_CARRY hold_n={} hold_total_us={} hold_max_us={} append_n={} append_total_us={} append_max_us={}",
        carry_hold.samples, carry_hold.total_micros, carry_hold.maximum_micros,
        carry_returned_to_append.samples, carry_returned_to_append.total_micros, carry_returned_to_append.maximum_micros,
    )).await;
    valid
}
