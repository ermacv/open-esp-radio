//! Event-driven WPA2 retransmission state without sleeps or timer polling.

use crate::state::RsnTxMessage;
use oer_portable_time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RsnRetryConfig {
    pub first_interval: Duration,
    pub subsequent_interval: Duration,
    /// Number of retransmissions after the original transmission.
    pub attempts: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnRetryError {
    ZeroFirstInterval,
    ZeroSubsequentInterval,
    ZeroAttempts,
    DeadlineOverflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RsnRetryAlarm {
    pub generation: u32,
    pub deadline: Instant,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnRetryAction {
    Stale,
    Exhausted,
    /// Retransmit `message`. The handshake state, not this schedule, owns
    /// the replay counter: each retransmission takes a new one from it.
    Transmit {
        message: RsnTxMessage,
        next_alarm: RsnRetryAlarm,
    },
}

/// One outstanding finite retransmission schedule.
///
/// The caller programs `RsnRetryAlarm.deadline` into a one-shot hardware
/// or executor alarm. `on_alarm` consumes an interrupt/event edge; it never
/// reads time, sleeps, or loops over missed deadlines.
pub struct RsnRetry {
    config: RsnRetryConfig,
    generation: u32,
    window: Window,
}

/// Whether a message waits for its response.
enum Window {
    Closed,
    Open {
        message: RsnTxMessage,
        attempts_left: u8,
    },
}

impl RsnRetry {
    pub const fn new(config: RsnRetryConfig) -> Result<Self, RsnRetryError> {
        if config.first_interval.as_micros() == 0 {
            return Err(RsnRetryError::ZeroFirstInterval);
        }
        if config.subsequent_interval.as_micros() == 0 {
            return Err(RsnRetryError::ZeroSubsequentInterval);
        }
        if config.attempts == 0 {
            return Err(RsnRetryError::ZeroAttempts);
        }
        Ok(Self {
            config,
            generation: 0,
            window: Window::Closed,
        })
    }

    /// Open the response window of `message`'s original transmission.
    pub fn arm(
        &mut self,
        message: RsnTxMessage,
        now: Instant,
    ) -> Result<RsnRetryAlarm, RsnRetryError> {
        let deadline = now
            .checked_add(self.config.first_interval)
            .ok_or(RsnRetryError::DeadlineOverflow)?;
        self.generation = next_generation(self.generation);
        self.window = Window::Open {
            message,
            attempts_left: self.config.attempts,
        };
        Ok(RsnRetryAlarm {
            generation: self.generation,
            deadline,
        })
    }

    pub fn cancel(&mut self) {
        self.generation = next_generation(self.generation);
        self.window = Window::Closed;
    }

    /// Rebase the first response window to the subsequent interval.
    ///
    /// Authenticator integrations use this after an acknowledged Message 1:
    /// hostapd likewise replaces its short initial EAPOL-Key timeout with the
    /// subsequent timeout once TX status proves that the station received M1.
    pub fn defer_first_after_ack(
        &self,
        now: Instant,
    ) -> Result<Option<RsnRetryAlarm>, RsnRetryError> {
        match self.window {
            Window::Open { attempts_left, .. } if attempts_left == self.config.attempts => {
                Ok(Some(self.alarm_after(now)?))
            }
            Window::Open { .. } | Window::Closed => Ok(None),
        }
    }

    pub fn on_alarm(
        &mut self,
        alarm: RsnRetryAlarm,
        now: Instant,
    ) -> Result<RsnRetryAction, RsnRetryError> {
        if alarm.generation != self.generation {
            return Ok(RsnRetryAction::Stale);
        }
        let Window::Open {
            message,
            attempts_left,
        } = &mut self.window
        else {
            return Ok(RsnRetryAction::Stale);
        };
        let Some(left) = attempts_left.checked_sub(1) else {
            self.cancel();
            return Ok(RsnRetryAction::Exhausted);
        };
        *attempts_left = left;
        let message = *message;
        // Even the last retransmission retains one response window. The next
        // alarm reports explicit exhaustion instead of silently leaving the
        // handshake pending forever after the retry budget is consumed.
        let next_alarm = self.alarm_after(now)?;
        Ok(RsnRetryAction::Transmit {
            message,
            next_alarm,
        })
    }

    pub const fn is_armed(&self) -> bool {
        matches!(self.window, Window::Open { .. })
    }

    pub const fn pending_message(&self) -> Option<RsnTxMessage> {
        match self.window {
            Window::Open { message, .. } => Some(message),
            Window::Closed => None,
        }
    }

    fn alarm_after(&self, now: Instant) -> Result<RsnRetryAlarm, RsnRetryError> {
        Ok(RsnRetryAlarm {
            generation: self.generation,
            deadline: now
                .checked_add(self.config.subsequent_interval)
                .ok_or(RsnRetryError::DeadlineOverflow)?,
        })
    }
}

const fn next_generation(current: u32) -> u32 {
    let next = current.wrapping_add(1);
    if next == 0 { 1 } else { next }
}

#[cfg(test)]
mod tests;
