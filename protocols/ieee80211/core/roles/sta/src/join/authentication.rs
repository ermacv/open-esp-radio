//! Open Authentication attempts, deadline results and retry ownership.

use oer_ieee80211_wire::address::{Bssid, OwnAddress};
use oer_ieee80211_wire::sequence::SequenceNumber;
use oer_ieee80211_wire::status::status_code;
use {
    oer_ieee80211_core_upper_mac::sequence::SequenceCounter,
    oer_ieee80211_wire::station::{
        StaDisconnect, parse_open_authentication_response, parse_sta_disconnect,
    },
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
    },
    Retry {
        runtime: StaAuthenticationRuntime,
        attempt: u16,
        failure: StaAuthenticationFailure,
    },
    Failed {
        attempts: u16,
        failure: StaAuthenticationFailure,
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
}

/// One active exchange. Only this owner accepts received frames or a timeout;
/// finishing it either consumes the epoch or returns the next retry owner.
#[derive(Debug)]
pub struct StaAuthenticationExchange {
    runtime: StaAuthenticationRuntime,
}

impl StaAuthenticationRuntime {
    pub const fn new(local: OwnAddress, bssid: Bssid) -> Self {
        Self {
            local,
            bssid,
            attempts_started: 0,
        }
    }

    /// Consume a retry owner and exactly one management sequence. The station
    /// reserves its TX credit first. No active or terminal epoch has this
    /// operation; only `Retry` below can produce another runtime owner.
    pub fn begin_attempt(
        mut self,
        sequence: &mut SequenceCounter,
    ) -> (StaAuthenticationExchange, StaAuthenticationAttempt) {
        // Only `new` or a retry below the three-attempt limit creates this
        // owner, so the increment cannot reach the integer bound.
        self.attempts_started = self.attempts_started.saturating_add(1);
        let attempt = StaAuthenticationAttempt {
            ordinal: self.attempts_started,
            sequence_number: sequence.take(),
            response_timeout: STA_RESPONSE_TIMEOUT,
        };
        (StaAuthenticationExchange { runtime: self }, attempt)
    }
}

impl StaAuthenticationExchange {
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
        if response.status_code == status_code::SUCCESS {
            StaAuthenticationEvent::Authenticated {
                attempt: runtime.attempts_started,
            }
        } else {
            StaAuthenticationEvent::Failed {
                attempts: runtime.attempts_started,
                failure: StaAuthenticationFailure::Rejected {
                    status_code: response.status_code,
                },
            }
        }
    }

    /// Expire this attempt once, returning either its retry owner or failure.
    pub fn response_timed_out(self) -> StaAuthenticationEvent {
        self.finish_retryable(StaAuthenticationFailure::Timeout)
    }

    fn finish_retryable(self, failure: StaAuthenticationFailure) -> StaAuthenticationEvent {
        let runtime = self.finish_attempt();
        let attempt = runtime.attempts_started;
        if attempt < STA_AUTHENTICATION_ATTEMPT_LIMIT {
            StaAuthenticationEvent::Retry {
                runtime,
                attempt,
                failure,
            }
        } else {
            StaAuthenticationEvent::Failed {
                attempts: attempt,
                failure,
            }
        }
    }
    fn finish_attempt(self) -> StaAuthenticationRuntime {
        self.runtime
    }
}

#[cfg(test)]
mod tests;
