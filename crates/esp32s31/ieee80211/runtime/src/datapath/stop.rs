//! Bounded physical MAC stop before RX ownership is withdrawn at shutdown.
use oer_esp32s31_ieee80211_mac::init::MacRuntimeStopHardware;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StopError {
    DeadlineOverflow,
    TimedOut { active_state: u8 },
}

/// Wait on timer events between explicit activity readbacks. Cancelling this
/// borrow leaves the hardware owner with the caller and never proves it idle.
/// RX must remain published until this function succeeds.
// CAPABILITY: wifi-bounded-wait-owners
pub async fn stop_mac<H: MacRuntimeStopHardware, T: oer_time::Timer>(
    hardware: &mut H,
    timer: &T,
    timeout: oer_time::Duration,
) -> Result<(), StopError> {
    let deadline = timer
        .deadline_after(timeout)
        .map_err(|oer_time::TimeOverflow| StopError::DeadlineOverflow)?;
    hardware.request_mac_runtime_stop();
    loop {
        let now = timer.now();
        let active_state = hardware.mac_runtime_active_state();
        if active_state == 0 {
            return Ok(());
        }
        if now >= deadline {
            return Err(StopError::TimedOut { active_state });
        }
        // MAC idle has no dedicated completion IRQ. This is a bounded,
        // timer-driven observation interval, never an assumed completion delay.
        let next = now
            .checked_add(STOP_POLL_INTERVAL)
            .map_or(deadline, |next| next.min(deadline));
        timer.wait_until(next).await;
    }
}

/// The interval between two activity readbacks of [`stop_mac`].
const STOP_POLL_INTERVAL: oer_time::Duration = oer_time::Duration::from_micros(20);

#[cfg(test)]
mod tests;
