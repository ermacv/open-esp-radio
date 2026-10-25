//! Ownership boundary for the cold MAC handshake prefix.

use crate::ieee80211::mac::WifiMacColdHal;
use oer_time::{Duration, Timer};

/// The wait between two samples of the ready edge.
pub const MAC_COLD_HANDSHAKE_RESAMPLE: Duration = Duration::from_micros(10);

/// The deadline the apps give the ready edge; the vendor waits forever.
pub const MAC_COLD_HANDSHAKE_DEADLINE: Duration = Duration::from_millis(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacColdStartError {
    HandshakeTimedOut {
        /// Number of not-ready observations before the deadline.
        samples: u32,
        /// Caller-provided deadline, counted from the request.
        deadline: Duration,
    },
    /// The timer cannot express the deadline or a resample wait.
    TimerOverflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacColdStartOutcome {
    /// Number of not-ready observations before the ready edge.
    pub handshake_samples: u32,
    /// Total hardware observations, including the final ready edge.
    pub handshake_observations: u32,
}

pub trait MacColdHandshakeHardware {
    /// Request the cold MAC start.
    fn request_cold_handshake(&mut self);

    /// Sample whether the cold MAC start is ready.
    fn sample_cold_handshake_ready(&mut self) -> bool;
}

impl MacColdHandshakeHardware for WifiMacColdHal<'_> {
    fn request_cold_handshake(&mut self) {
        self.request_handshake();
    }

    fn sample_cold_handshake_ready(&mut self) -> bool {
        self.sample_handshake_ready()
    }
}

/// Request the cold start, sample the ready edge once, then again after each
/// [`MAC_COLD_HANDSHAKE_RESAMPLE`] wait on `timer` until `deadline` has
/// passed.
///
/// One sample always occurs; a not-ready sample at or past the deadline
/// times the handshake out. The vendor's interrupt cleanup after the ready
/// edge (mask and clear every MAC event) is the radio interrupt handler's:
/// it keeps the MAC's banks masked between epochs and clears them when it
/// activates one.
pub async fn run_cold_handshake<H: MacColdHandshakeHardware, T: Timer>(
    hardware: &mut H,
    timer: &T,
    deadline: Duration,
) -> Result<MacColdStartOutcome, MacColdStartError> {
    hardware.request_cold_handshake();
    let end = timer
        .deadline_after(deadline)
        .map_err(|_| MacColdStartError::TimerOverflow)?;
    let mut samples = 0_u32;
    while !hardware.sample_cold_handshake_ready() {
        samples = samples.saturating_add(1);
        if timer.now() >= end {
            return Err(MacColdStartError::HandshakeTimedOut { samples, deadline });
        }
        timer
            .wait_for(MAC_COLD_HANDSHAKE_RESAMPLE)
            .await
            .map_err(|_| MacColdStartError::TimerOverflow)?;
    }
    Ok(MacColdStartOutcome {
        handshake_samples: samples,
        handshake_observations: samples.saturating_add(1),
    })
}
