//! Association epoch, retransmission schedule and terminal response policy.

use oer_ieee80211_wire::address::{Bssid, OwnAddress};
use oer_ieee80211_wire::sequence::SequenceNumber;
use {
    crate::sequence::StaSequenceCounter,
    oer_ieee80211_wire::{
        security::LinkProtection,
        station::{
            AssociationResponse, StaDisconnect, parse_association_response, parse_sta_disconnect,
        },
    },
};

use oer_time::{Duration, Instant};

use super::STA_RESPONSE_TIMEOUT;

/// Compatibility schedule for Association retransmission inside the vendor
/// one-second state deadline.
///
/// This 160-ms cadence comes from hardware-qualified open STA testing, not
/// from a recovered vendor timer body. It remains explicit so
/// later blob comparison can replace one policy value without changing the
/// executor loop.
pub struct StaAssociationRetrySchedule;

impl StaAssociationRetrySchedule {
    pub const INTERVAL: Duration = Duration::from_millis(160);

    /// When the `ordinal`th request (from one) of an epoch is due, measured
    /// from the epoch's start, while it falls before the epoch's deadline.
    pub const fn offset(ordinal: u16) -> Option<Duration> {
        if ordinal == 0 {
            return None;
        }
        let micros = (ordinal as u64)
            .saturating_sub(1)
            .saturating_mul(Self::INTERVAL.as_micros());
        if micros < STA_RESPONSE_TIMEOUT.as_micros() {
            Some(Duration::from_micros(micros))
        } else {
            None
        }
    }
}

/// One uniquely numbered Association transmission inside a state epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaAssociationAttempt {
    pub ordinal: u16,
    pub sequence_number: SequenceNumber,
    /// When the request was due, from the start of its epoch.
    pub offset: Duration,
}

/// Protocol-level reason why an Association epoch ended unsuccessfully.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaAssociationFailure {
    Timeout,
    /// The complete response/comeback deadline cannot fit monotonic time.
    DeadlineOverflow,
    PeerDisconnect(StaDisconnect),
    Rejected {
        status_code: u16,
    },
    /// The access point refused the association temporarily with a comeback
    /// time the station does not wait for: longer than 5000 TUs, or a second
    /// refusal after the station already came back once.
    ComebackRefused {
        comeback_tu: u32,
    },
    /// A successful response contradicted the exact security mode selected
    /// from the scan record. Treating this as association success would make
    /// the following key/plaintext transition an implicit downgrade.
    SecurityModeMismatch,
}

/// Result of observing a management frame.
#[derive(Debug)]
#[must_use = "retain the active association or deliver its terminal"]
pub enum StaAssociationEvent {
    Irrelevant(StaAssociationRuntime),
    Associated {
        response: AssociationResponse,
        total_received_frames: u32,
    },
    Failed {
        failure: StaAssociationFailure,
        total_received_frames: u32,
    },
}

/// What the association asks of its executor at one instant.
#[derive(Debug)]
#[must_use = "retain the active association or deliver its terminal"]
pub enum StaAssociationPoll {
    Idle(StaAssociationRuntime),
    /// Send this Association Request now.
    Transmit {
        runtime: StaAssociationRuntime,
        attempt: StaAssociationAttempt,
    },
    Failed {
        failure: StaAssociationFailure,
        total_received_frames: u32,
    },
}

/// Allocation-free owner of one ordinary STA Association epoch.
///
/// A target executor polls it with the time, transmits the request it asks
/// for, reports every completed RX descriptor and supplies extracted
/// management frames, and waits until [`Self::next_deadline`] or the next
/// frame. Polling and RX consume this owner: only an active result returns
/// it, so a terminal cannot be polled or delivered twice. This type owns
/// the one-second deadline, retransmission cadence,
/// management sequence consumption and terminal response policy; it does not
/// own timers, DMA or MAC registers.
///
/// SOURCE(esp32s31): complete `libnet80211.a[ieee80211_sta.o]::
/// ieee80211_sta_new_state` Association branch arms the 1,000-ms state timer.
/// The 160-ms retransmission cadence is the hardware-qualified open STA policy
/// and remains isolated in [`StaAssociationRetrySchedule`] pending recovery of
/// the vendor timer body.
#[derive(Debug)]
#[must_use = "retain this association until its terminal or cancel it"]
pub struct StaAssociationRuntime {
    local: OwnAddress,
    bssid: Bssid,
    security: LinkProtection,
    stage: AssociationStage,
    received_frames: u32,
}

#[derive(Debug)]
enum AssociationStage {
    Initial(RequestEpoch),
    /// A complete next epoch, starting only at its scheduled comeback time.
    Comeback(RequestEpoch),
    Returned(RequestEpoch),
}

#[derive(Debug)]
struct RequestEpoch {
    start: Instant,
    deadline: Instant,
    sent: u16,
}

impl RequestEpoch {
    const fn new(start: Instant) -> Result<Self, StaAssociationFailure> {
        let Some(deadline) = start.checked_add(STA_RESPONSE_TIMEOUT) else {
            return Err(StaAssociationFailure::DeadlineOverflow);
        };
        Ok(Self {
            start,
            deadline,
            sent: 0,
        })
    }

    fn next_deadline(&self) -> Instant {
        StaAssociationRetrySchedule::offset(self.sent.saturating_add(1)).map_or(
            self.deadline,
            // Construction checked the full one-second deadline. Every
            // offset is strictly smaller, so this addition cannot saturate.
            |offset| self.start.saturating_add(offset),
        )
    }
}

/// Status of a temporary association refusal: the access point protects the
/// management frames of an association it still holds and first confirms
/// it with an SA Query.
const REJECTED_TEMPORARILY: u16 = 30;
/// The longest comeback time the vendor waits for.
const MAXIMUM_COMEBACK_TU: u32 = 5_000;
/// The vendor comes back this many TUs after the named comeback time.
const COMEBACK_MARGIN_TU: u32 = 100;
const MICROS_PER_TU: u64 = 1_024;

impl StaAssociationRuntime {
    pub const fn new(
        local: OwnAddress,
        bssid: Bssid,
        security: LinkProtection,
        now: Instant,
    ) -> Result<Self, StaAssociationFailure> {
        let epoch = match RequestEpoch::new(now) {
            Ok(epoch) => epoch,
            Err(error) => return Err(error),
        };
        Ok(Self {
            local,
            bssid,
            security,
            stage: AssociationStage::Initial(epoch),
            received_frames: 0,
        })
    }

    /// Advance to `now`: expire the epoch started at construction and its
    /// one-second deadline, or consume a management sequence number exactly
    /// when the retry schedule calls for a new request.
    pub fn poll(mut self, now: Instant, sequence: &mut StaSequenceCounter) -> StaAssociationPoll {
        if let AssociationStage::Comeback(epoch) = self.stage {
            if now < epoch.start {
                self.stage = AssociationStage::Comeback(epoch);
                return StaAssociationPoll::Idle(self);
            }
            self.stage = AssociationStage::Returned(epoch);
        }
        let epoch = match &mut self.stage {
            AssociationStage::Initial(epoch)
            | AssociationStage::Comeback(epoch)
            | AssociationStage::Returned(epoch) => epoch,
        };
        if now >= epoch.deadline {
            return StaAssociationPoll::Failed {
                failure: StaAssociationFailure::Timeout,
                total_received_frames: self.received_frames,
            };
        }
        let ordinal = epoch.sent.saturating_add(1);
        match StaAssociationRetrySchedule::offset(ordinal) {
            Some(offset) if now >= epoch.next_deadline() => {
                epoch.sent = ordinal;
                StaAssociationPoll::Transmit {
                    runtime: self,
                    attempt: StaAssociationAttempt {
                        ordinal,
                        sequence_number: sequence.take(),
                        offset,
                    },
                }
            }
            _ => StaAssociationPoll::Idle(self),
        }
    }

    /// When the association next needs a poll: the end of a comeback wait,
    /// the next scheduled request or the epoch's deadline.
    pub fn next_deadline(&self) -> Instant {
        match &self.stage {
            AssociationStage::Comeback(epoch) => epoch.start,
            AssociationStage::Initial(epoch) | AssociationStage::Returned(epoch) => {
                epoch.next_deadline()
            }
        }
    }

    /// The state deadline remains active while a request waits for a TX
    /// credit. A retry's earlier due time is not a wake source in that wait.
    pub(crate) fn response_deadline(&self) -> Instant {
        match &self.stage {
            AssociationStage::Initial(epoch)
            | AssociationStage::Comeback(epoch)
            | AssociationStage::Returned(epoch) => epoch.deadline,
        }
    }

    /// Account for one completed RX descriptor, including a frame which is
    /// not a valid management input. The station owns this runtime only while
    /// its exchange is active; accounting has no dynamic refusal.
    pub fn observe_received_frame(&mut self) {
        self.received_frames = self.received_frames.saturating_add(1);
    }

    /// Classify one extracted management frame for the selected peer,
    /// received at `now`.
    pub fn observe_management_frame(self, frame: &[u8], now: Instant) -> StaAssociationEvent {
        if let Some(disconnect) = parse_sta_disconnect(frame, self.local, self.bssid) {
            return self.fail(StaAssociationFailure::PeerDisconnect(disconnect));
        }
        let Some(response) = parse_association_response(frame, self.local.get(), self.bssid.get())
        else {
            return StaAssociationEvent::Irrelevant(self);
        };
        if response.status_code == REJECTED_TEMPORARILY
            && let Some(comeback_tu) = response.association_comeback_tu
        {
            return self.come_back(comeback_tu, now);
        }
        if response.status_code != 0 {
            return self.fail(StaAssociationFailure::Rejected {
                status_code: response.status_code,
            });
        }
        if !response.matches_security(self.security) {
            return self.fail(StaAssociationFailure::SecurityModeMismatch);
        }
        StaAssociationEvent::Associated {
            response,
            total_received_frames: self.received_frames,
        }
    }

    pub const fn total_received_frames(&self) -> u32 {
        self.received_frames
    }

    /// Wait out one temporary refusal, then send a fresh Association Request
    /// under a fresh deadline, as the vendor's `sta_recv_assoc` does: it
    /// disarms the association timer and arms `sta_assoc_comeback` for the
    /// comeback time plus 100 TUs. A second refusal, or one longer than
    /// 5000 TUs, ends the association.
    ///
    /// SOURCE(esp32s31): complete pinned `libnet80211.a[ieee80211_sta.o]::
    /// sta_recv_assoc` and `sta_assoc_comeback`.
    fn come_back(mut self, comeback_tu: u32, now: Instant) -> StaAssociationEvent {
        if !matches!(self.stage, AssociationStage::Initial(_)) || comeback_tu > MAXIMUM_COMEBACK_TU
        {
            return self.fail(StaAssociationFailure::ComebackRefused { comeback_tu });
        }
        // The checked comeback is at most 5000 TUs; widening before the
        // fixed margin/product preserves its exact value.
        let wait = Duration::from_micros(
            u64::from(comeback_tu)
                .saturating_add(u64::from(COMEBACK_MARGIN_TU))
                .saturating_mul(MICROS_PER_TU),
        );
        let Some(until) = now.checked_add(wait) else {
            return self.fail(StaAssociationFailure::DeadlineOverflow);
        };
        let epoch = match RequestEpoch::new(until) {
            Ok(epoch) => epoch,
            Err(error) => return self.fail(error),
        };
        self.stage = AssociationStage::Comeback(epoch);
        StaAssociationEvent::Irrelevant(self)
    }

    fn fail(self, failure: StaAssociationFailure) -> StaAssociationEvent {
        StaAssociationEvent::Failed {
            failure,
            total_received_frames: self.received_frames,
        }
    }
}

#[cfg(test)]
mod tests;
