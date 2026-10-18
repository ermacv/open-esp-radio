//! Waits on the image's monotonic time.

use oer_time::{Duration, Timer};

/// Wait for `duration` on `timer`. A deadline past the timer's range is
/// never reached, so the wait does not end early.
pub(crate) async fn wait_for(timer: &impl Timer, duration: Duration) {
    if timer.wait_for(duration).await.is_err() {
        core::future::pending::<()>().await;
    }
}

/// The wait after a MAC stop request before its first activity readback.
pub(crate) const MAC_STOP_SETTLE: Duration = Duration::from_micros(20);
/// The interval between two MAC activity readbacks.
pub(crate) const MAC_STOP_POLL: Duration = Duration::from_micros(1);
