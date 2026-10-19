//! Open Authentication attempts, deadline results and retry ownership.

use oer_ieee80211_mac::address::{Bssid, OwnAddress};
use oer_ieee80211_mac::sequence::SequenceNumber;
use oer_ieee80211_mac::station::{
    StaDisconnect, StaSequenceCounter, parse_open_authentication_response, parse_sta_disconnect,
};

use super::STA_RESPONSE_TIMEOUT;

/// Bounded open Authentication attempts retained by the qualified STA path.
pub const STA_AUTHENTICATION_ATTEMPT_LIMIT: u16 = 3;

/// One uniquely numbered Open Authentication transmission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaAuthenticationAttempt {
    pub ordinal: u16,
    pub sequence_number: SequenceNumber,
    pub response_timeout: oer_time::Duration,
}

/// Protocol-level reason why one Authentication attempt ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaAuthenticationFailure {
    Timeout,
    PeerDisconnect(StaDisconnect),
    Rejected { status_code: u16 },
}

/// Result of observing a management frame or expiring an attempt deadline.
#[derive(Debug)]
pub enum StaAuthenticationEvent {
    Irrelevant(StaAuthenticationExchange),
    Authenticated {
        attempt: u16,
        total_received_frames: u32,
    },
    Retry {
        runtime: StaAuthenticationRuntime,
        attempt: u16,
        failure: StaAuthenticationFailure,
        received_frames: u32,
        total_received_frames: u32,
    },
    Failed {
        attempts: u16,
        failure: StaAuthenticationFailure,
        total_received_frames: u32,
    },
}

/// Allocation-free owner of the ordinary Open Authentication retry epoch.
///
/// This type owns protocol policy and state only. A target executor arms RX,
/// submits the returned sequence number, reports every received descriptor,
/// and supplies extracted management frames. It therefore remains independent
/// of Embassy, DMA layout and the ESP32-S31 MAC.
///
/// SOURCE(esp32s31): complete `libnet80211.a[ieee80211_sta.o]::
/// ieee80211_sta_new_state` ordinary Authentication branch arms the 1,000-ms
/// state timer. This runtime owns the open STA policy of at most three
/// authentication attempts, including sequence allocation and timer outcomes.
#[derive(Debug)]
pub struct StaAuthenticationRuntime {
    local: OwnAddress,
    bssid: Bssid,
    attempts_started: u16,
    total_received_frames: u32,
}

/// One active exchange. Only this owner accepts received frames or a timeout;
/// finishing it either consumes the epoch or returns the next retry owner.
#[derive(Debug)]
pub struct StaAuthenticationExchange {
    runtime: StaAuthenticationRuntime,
    received_frames: u32,
}

impl StaAuthenticationRuntime {
    pub const fn new(local: OwnAddress, bssid: Bssid) -> Self {
        Self {
            local,
            bssid,
            attempts_started: 0,
            total_received_frames: 0,
        }
    }

    /// Consume a retry owner and exactly one management sequence. The station
    /// reserves its TX credit first. No active or terminal epoch has this
    /// operation; only `Retry` below can produce another runtime owner.
    pub fn begin_attempt(
        mut self,
        sequence: &mut StaSequenceCounter,
    ) -> (StaAuthenticationExchange, StaAuthenticationAttempt) {
        // Only `new` or a retry below the three-attempt limit creates this
        // owner, so the increment cannot reach the integer bound.
        self.attempts_started = self.attempts_started.saturating_add(1);
        let attempt = StaAuthenticationAttempt {
            ordinal: self.attempts_started,
            sequence_number: sequence.take(),
            response_timeout: STA_RESPONSE_TIMEOUT,
        };
        (
            StaAuthenticationExchange {
                runtime: self,
                received_frames: 0,
            },
            attempt,
        )
    }

    pub const fn total_received_frames(&self) -> u32 {
        self.total_received_frames
    }
}

impl StaAuthenticationExchange {
    /// Account for one received descriptor while this exchange owns RX.
    pub fn observe_received_frame(&mut self) {
        self.received_frames = self.received_frames.saturating_add(1);
    }

    /// Consume the active exchange and classify one management frame.
    /// An irrelevant input returns this same owner unchanged.
    pub fn observe_management_frame(self, frame: &[u8]) -> StaAuthenticationEvent {
        if let Some(disconnect) =
            parse_sta_disconnect(frame, self.runtime.local, self.runtime.bssid)
        {
            return self.finish_retryable(StaAuthenticationFailure::PeerDisconnect(disconnect));
        }
        let Some(response) = parse_open_authentication_response(
            frame,
            self.runtime.local.get(),
            self.runtime.bssid.get(),
        ) else {
            return StaAuthenticationEvent::Irrelevant(self);
        };
        let runtime = self.finish_attempt();
        if response.status_code == 0 {
            StaAuthenticationEvent::Authenticated {
                attempt: runtime.attempts_started,
                total_received_frames: runtime.total_received_frames,
            }
        } else {
            StaAuthenticationEvent::Failed {
                attempts: runtime.attempts_started,
                failure: StaAuthenticationFailure::Rejected {
                    status_code: response.status_code,
                },
                total_received_frames: runtime.total_received_frames,
            }
        }
    }

    /// Expire this attempt once, returning either its retry owner or failure.
    pub fn response_timed_out(self) -> StaAuthenticationEvent {
        self.finish_retryable(StaAuthenticationFailure::Timeout)
    }

    pub const fn total_received_frames(&self) -> u32 {
        self.runtime
            .total_received_frames
            .saturating_add(self.received_frames)
    }
    pub const fn active_received_frames(&self) -> u32 {
        self.received_frames
    }

    fn finish_retryable(self, failure: StaAuthenticationFailure) -> StaAuthenticationEvent {
        let received_frames = self.received_frames;
        let runtime = self.finish_attempt();
        let attempt = runtime.attempts_started;
        let total_received_frames = runtime.total_received_frames;
        if attempt < STA_AUTHENTICATION_ATTEMPT_LIMIT {
            StaAuthenticationEvent::Retry {
                runtime,
                attempt,
                failure,
                received_frames,
                total_received_frames,
            }
        } else {
            StaAuthenticationEvent::Failed {
                attempts: attempt,
                failure,
                total_received_frames,
            }
        }
    }
    fn finish_attempt(self) -> StaAuthenticationRuntime {
        let mut runtime = self.runtime;
        runtime.total_received_frames = runtime
            .total_received_frames
            .saturating_add(self.received_frames);
        runtime
    }
}

#[cfg(test)]
mod tests;
