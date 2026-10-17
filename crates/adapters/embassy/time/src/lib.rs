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

use oer_time::{Clock, Instant, Timer};

/// The image's monotonic time in microseconds since the driver's epoch.
pub fn now_micros() -> u64 {
    embassy_time::Instant::now().as_micros()
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
        match embassy_time::Instant::try_from_micros(deadline.as_micros()) {
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
