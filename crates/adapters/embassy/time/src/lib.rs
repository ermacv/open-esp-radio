#![no_std]
#![forbid(unsafe_code)]

//! The image's monotonic time on the `embassy-time` driver.
//!
//! [`EmbassyClock`] implements [`oer_time::Clock`] and [`oer_time::Timer`]
//! over the one time driver the final image links, and [`now_micros`] reads
//! the same time for owners that take a plain function, such as a trace
//! record stamp. The resolution is the driver's tick; instants
//! convert through `embassy_time::Instant::as_micros`, so any tick rate is
//! accepted.
//!
//! The image's time is the driver's time plus [`CLOCK_EPOCH_OFFSET`]: the
//! driver's zero reads as 2^31 µs, so code at the boundary with 32-bit
//! hardware counters meets the wrap at once.

use oer_time::{CLOCK_EPOCH_OFFSET, Clock, Instant, Timer};

/// The image's monotonic time in microseconds: the driver's time plus
/// [`CLOCK_EPOCH_OFFSET`].
pub fn now_micros() -> u64 {
    image_micros(embassy_time::Instant::now().as_micros())
}

/// The image's microseconds for the driver's. The sum reaches the end of
/// `u64` only after some 584 000 years of uptime; it saturates there so
/// that time never decreases.
const fn image_micros(driver_micros: u64) -> u64 {
    driver_micros.saturating_add(CLOCK_EPOCH_OFFSET.as_micros())
}

/// The driver's microseconds for an image instant, `None` before the
/// driver's epoch: such an instant has already passed.
const fn driver_micros(image: Instant) -> Option<u64> {
    image
        .as_micros()
        .checked_sub(CLOCK_EPOCH_OFFSET.as_micros())
}

/// The `embassy-time` driver as the image's [`Clock`] and [`Timer`].
#[derive(Clone, Copy, Debug, Default)]
pub struct EmbassyClock;

impl Clock for EmbassyClock {
    fn now(&self) -> Instant {
        Instant::from_micros(now_micros())
    }
}

impl Timer for EmbassyClock {
    async fn wait_until(&self, deadline: Instant) {
        // Before the driver's epoch: the deadline has passed.
        let Some(deadline) = driver_micros(deadline) else {
            return;
        };
        match embassy_time::Instant::try_from_micros(deadline) {
            // `embassy_time::Timer` yields once even for a past deadline;
            // the contract completes a reached deadline at once.
            Some(deadline) if deadline <= embassy_time::Instant::now() => {}
            Some(deadline) => embassy_time::Timer::at(deadline).await,
            // Past the driver's tick range: the deadline is never reached.
            None => core::future::pending().await,
        }
    }
}

#[cfg(test)]
mod tests;
