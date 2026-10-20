//! Timer-woken TX detector readiness while the caller retains its PHY owner.
use oer_time::{Duration, Timer};

// MEASUREMENT: s31-a run 1792521908966-002e2465, 21,512 conversions,
// 100 forced tracking ticks, post-2-us-settle ready observations <= 20 us;
// none censored by the 100-ms acquisition watchdog. Keep twice that maximum.
pub(crate) const READY_LIMIT: Duration = Duration::from_micros(40);
const SAMPLE_INTERVAL: Duration = Duration::from_micros(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Readiness {
    pub elapsed_micros: u64,
    pub observations: u16,
    pub ready: bool,
}

/// SOURCE(esp32s31): complete rev0 ROM `phy_pwdet_tone_start` samples all
/// three SAR-ready bits after its 1/2-us tone/trigger settles. It does not
/// provide a readiness deadline. The Rust owner keeps both settling edges,
/// and this phase retains its mutable physical borrow across timer waits.
/// Readiness observed exactly at the limit succeeds; a late observation fails.
pub(crate) async fn wait(
    timer: &impl Timer,
    limit: Duration,
    mut sample: impl FnMut() -> bool,
) -> Readiness {
    let started = timer.now();
    let Some(deadline) = started.checked_add(limit) else {
        return Readiness {
            elapsed_micros: 0,
            observations: 0,
            ready: false,
        };
    };
    let mut observations = 0_u16;
    loop {
        let ready = sample();
        let now = timer.now();
        observations = observations.saturating_add(1);
        let elapsed_micros = now.saturating_duration_since(started).as_micros();
        if ready || now >= deadline {
            return Readiness {
                elapsed_micros,
                observations,
                ready: ready && now <= deadline,
            };
        }
        timer
            .wait_until(now.saturating_add(SAMPLE_INTERVAL).min(deadline))
            .await;
    }
}

#[cfg(test)]
mod tests;
