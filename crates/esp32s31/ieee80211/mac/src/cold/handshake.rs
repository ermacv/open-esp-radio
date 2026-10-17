//! Ownership boundary for the finite cold MAC handshake prefix.

use oer_esp32s31_hal::ieee80211::mac::WifiMacColdHal;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacColdStartError {
    HandshakeTimedOut {
        /// Number of not-ready observations consumed before stopping.
        samples: u32,
        /// Caller-provided finite not-ready observation limit.
        sample_limit: u32,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacColdStartOutcome {
    /// Number of not-ready observations before the ready edge.
    pub handshake_samples: u32,
    /// Total hardware observations, including the final ready edge.
    pub handshake_observations: u32,
}

pub trait MacColdHandshakeHardware {
    fn begin_cold_handshake(
        &mut self,
        sample_limit: u32,
    ) -> Result<MacColdStartOutcome, MacColdStartError>;
}

impl MacColdHandshakeHardware for WifiMacColdHal<'_> {
    fn begin_cold_handshake(
        &mut self,
        sample_limit: u32,
    ) -> Result<MacColdStartOutcome, MacColdStartError> {
        self.begin_handshake(sample_limit)
            .map(|outcome| MacColdStartOutcome {
                handshake_samples: outcome.samples,
                handshake_observations: outcome.observations,
            })
            .map_err(|timeout| MacColdStartError::HandshakeTimedOut {
                samples: timeout.samples,
                sample_limit: timeout.sample_limit,
            })
    }
}
