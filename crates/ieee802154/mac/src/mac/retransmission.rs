//! Frame retransmission, ported from OpenThread `SubMac::HandleTransmitDone`
//! (`src/core/mac/sub_mac.cpp` at OpenThread
//! `43cc05a9bcf780bd758bad53d897e2d88cf8cb75`, the submodule of ESP-IDF
//! `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`) with the configuration
//! ESP-IDF builds it with: the defaults of `src/core/config/mac.h` (sha256
//! `09637e77eab8a3f90de41b19eff2e0b19a1fd9b61e7b12d0994d4757682e109a`), which
//! ESP-IDF's `openthread-core-esp32x-*-config.h` leave unchanged.
//!
//! ESP-IDF's radio does not retransmit, so `SubMac` retries a frame whose
//! attempt ended without an acknowledgement or without channel access,
//! up to `macMaxFrameRetries` times, each retry acquiring the channel
//! afresh. With `OPENTHREAD_CONFIG_MAC_ADD_DELAY_ON_NO_ACK_ERROR_BEFORE_RETRY`
//! a retry after a missing acknowledgement first waits a random delay whose
//! exponent grows from 0 to 5 per such retry of the frame.

use crate::mac::csma::{SYMBOL_MICROS, UNIT_BACKOFF_PERIOD_SYMBOLS};

/// `OPENTHREAD_CONFIG_MAC_RETX_DELAY_MIN_BACKOFF_EXPONENT`.
pub const RETX_DELAY_MIN_BACKOFF_EXPONENT: u8 = 0;
/// `OPENTHREAD_CONFIG_MAC_RETX_DELAY_MAX_BACKOFF_EXPONENT`.
pub const RETX_DELAY_MAX_BACKOFF_EXPONENT: u8 = 5;

/// How one attempt of a frame failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttemptFailure {
    /// No acknowledgement, or an invalid one (`kErrorNoAck`).
    NoAcknowledgement,
    /// The channel could not be acquired (`kErrorChannelAccessFailure`).
    ChannelAccess,
}

/// How the next attempt of a retried frame starts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetryStart {
    /// Wait [`RetryStart::delay_micros`] first (`kStateDelayBeforeRetx`).
    AfterDelay {
        /// The delay's backoff exponent.
        exponent: u8,
    },
    /// Acquire the channel at once.
    Now,
}

impl RetryStart {
    /// The random delay before the retry in microseconds, drawn from the
    /// uniform word `random` as `SubMac::StartTimerForBackoff` draws it.
    pub const fn delay_micros(self, random: u32) -> u32 {
        match self {
            Self::AfterDelay { exponent } => {
                random % (1 << exponent) * UNIT_BACKOFF_PERIOD_SYMBOLS * SYMBOL_MICROS
            }
            Self::Now => 0,
        }
    }
}

/// The retransmission state of one frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameRetries {
    max_frame_retries: u8,
    retries: u8,
    delay_exponent: u8,
}

impl FrameRetries {
    /// A frame retried at most `max_frame_retries` times (`SubMac::Send`).
    pub const fn new(max_frame_retries: u8) -> Self {
        Self {
            max_frame_retries,
            retries: 0,
            delay_exponent: RETX_DELAY_MIN_BACKOFF_EXPONENT,
        }
    }

    /// The retries made so far.
    pub const fn retries(&self) -> u8 {
        self.retries
    }

    /// Decide after a failed attempt whether the frame is retried, and how
    /// its next attempt starts; `None` ends the transmission.
    pub fn retry(&mut self, failure: AttemptFailure) -> Option<RetryStart> {
        if self.retries >= self.max_frame_retries {
            return None;
        }
        self.retries += 1;
        Some(match failure {
            AttemptFailure::NoAcknowledgement => {
                let exponent = self.delay_exponent;
                self.delay_exponent = if exponent < RETX_DELAY_MAX_BACKOFF_EXPONENT {
                    exponent + 1
                } else {
                    RETX_DELAY_MAX_BACKOFF_EXPONENT
                };
                RetryStart::AfterDelay { exponent }
            }
            AttemptFailure::ChannelAccess => RetryStart::Now,
        })
    }
}

#[cfg(test)]
mod tests;
