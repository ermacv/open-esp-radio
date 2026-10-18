//! Intrusive receive-clock probe for the `diagnostic-rx-clock` image.
//!
//! At the first executor-visible handoff of every 32nd received frame it logs
//! the frame's receive timestamp from the RX-control prefix (a Wi-Fi MAC
//! local-time reading the hardware recorded) beside the monotonic time of the
//! handoff. Paired readings of the MAC local-time counter and the monotonic
//! clock, logged by the image, relate the two, so the frame's age at handoff
//! and the counter's unit and drift follow from the log.

use core::sync::atomic::{AtomicU32, Ordering};

/// Frames handed off since boot; one in [`PERIOD`] is logged.
static FRAMES: AtomicU32 = AtomicU32::new(0);

/// One logged frame per this many handoffs.
const PERIOD: u32 = 32;

/// Log the receive timestamp of `buffer` (the DMA buffer with its RX-control
/// prefix) and the monotonic time `handoff` of its first handoff.
pub(crate) fn observe(buffer: &[u8], handoff: oer_time::Instant) {
    let frame = FRAMES.fetch_add(1, Ordering::Relaxed);
    if frame % PERIOD != 0 {
        return;
    }
    if let Some(timestamp) = oer_esp32s31_ieee80211_mac::rx::decode_rx_local_timestamp(buffer) {
        log::info!(
            "rx_clock_frame frame={frame} frame_ts={timestamp} handoff_us={}",
            handoff.as_micros()
        );
    }
}
