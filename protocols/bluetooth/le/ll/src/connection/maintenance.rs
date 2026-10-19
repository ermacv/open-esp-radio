//! Protocol admission for a budgeted maintenance-induced event pause.
//!
//! This policy accompanies the actual connection owner. It neither grants RF
//! access nor admits elapsed-time budgets. The chip scheduler must separately
//! check fresh time, supervision/procedure margins, physical quiescence and
//! restoration before the unchanged successor anchor.
//!
//! Core 6.2 Vol 6 Part B, §§4.5.5 and 5.5.1 require initial acknowledgement
//! and confirmation of acknowledgement for a pending Instant indication.
//! Peer SN advance supplies the latter confirmation. The maintenance policy
//! protects the Instant event and its predecessor for both supported Instant
//! procedures; the predecessor restriction for Channel Map Update is an OER
//! admission invariant, not an additional Bluetooth requirement.
//! See the [Link Layer specification](https://www.bluetooth.com/wp-content/uploads/Files/Specification/HTML/Core-62/out/en/low-energy-controller/link-layer-specification.html).

use super::acknowledgement::{HeaderFlags, SequenceNumber};
use super::{
    LePeripheralConnectionEventCompleted, LePeripheralConnectionEventDelta,
    LePeripheralConnectionEventPeerActivity, LePeripheralConnectionRecurringEventProvisional,
    LePeripheralConnectionState,
};

/// A deliberate miss is unavailable; ordinary event processing remains legal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SkipBlocked {
    /// A pause must omit events and remain within the event-counter half-range.
    InvalidPause,
    Establishment,
    /// The initial Peripheral packet has not yet been acknowledged (NESN=1).
    InitialAcknowledgement,
    /// A previous intentional pause still needs a successfully serviced event.
    RecoveryEventRequired,
    /// No received peer sequence advance yet proves acknowledgement of the
    /// indication. An elapsed supervision budget is not that proof.
    InstantAcknowledgement,
    /// The omitted range includes an Instant or its immediately preceding event.
    InstantProcedure,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct State {
    initial_acknowledged: bool,
    recovery_required: bool,
    instant_acknowledgement: InstantAcknowledgement,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InstantAcknowledgement {
    HeaderRequired,
    AwaitingPeerSequenceAdvance { indication_sn: SequenceNumber },
    Confirmed,
}

impl State {
    pub(super) const fn new() -> Self {
        Self {
            initial_acknowledged: false,
            recovery_required: false,
            instant_acknowledgement: InstantAcknowledgement::HeaderRequired,
        }
    }

    pub(super) const fn commit_skip(&mut self) {
        self.recovery_required = true;
    }

    pub(super) fn instant_received(&mut self) {
        self.instant_acknowledgement = InstantAcknowledgement::HeaderRequired;
    }

    fn observe_header(&mut self, header: u8, instant_pending: bool) {
        let flags = HeaderFlags::from_octet(header);
        // The Central's NESN moves past zero once it received the
        // Peripheral's first PDU.
        self.initial_acknowledged |= flags.nesn == SequenceNumber::ONE;
        self.recovery_required = false;
        if !instant_pending {
            return;
        }
        let sn = flags.sn;
        self.instant_acknowledgement = match self.instant_acknowledgement {
            InstantAcknowledgement::HeaderRequired => {
                InstantAcknowledgement::AwaitingPeerSequenceAdvance { indication_sn: sn }
            }
            InstantAcknowledgement::AwaitingPeerSequenceAdvance { indication_sn }
                if indication_sn != sn =>
            {
                InstantAcknowledgement::Confirmed
            }
            state => state,
        };
    }
}

impl LePeripheralConnectionEventCompleted {
    /// Observe the header of a valid received Data Channel PDU in this event.
    ///
    /// The backend must validate CRC and, where applicable, MIC and sequence
    /// acceptance before calling this method. It only records completion of
    /// initial acknowledgement and peer sequence advance after an Instant PDU.
    /// Call after dispatching that PDU to `schedule_*_update`. A later SN change
    /// proves the Central received the Peripheral's acknowledgement. Merely
    /// queueing a response or observing retransmission of the same SN does not.
    /// A valid accepted packet releases the maintenance recovery obligation;
    /// raw RX activity alone does not, because it precedes MIC validation.
    /// This does not renew supervision or advance the event counter.
    pub fn observe_valid_packet_header(&mut self, header: u8) {
        if matches!(
            self.peer_activity,
            LePeripheralConnectionEventPeerActivity::Observed
        ) {
            self.connection.maintenance.observe_header(
                header,
                self.connection.pending_connection_update.is_some()
                    || self.connection.pending_channel_map.is_some(),
            );
        }
    }

    /// Release maintenance recovery after a valid plaintext reception in this
    /// event, even when hardware suppresses empty/duplicate payload delivery.
    /// The backend must prove a fresh CRC-valid receive in the completed event;
    /// anchor capture or unchanged receive time alone is insufficient. Never
    /// use this path for encrypted traffic or encryption transitions: those
    /// require packet-level validation through `observe_valid_packet_header`.
    /// This supplies neither initial nor Instant acknowledgement evidence and
    /// does not update supervision, sequence numbers or the connection timeline.
    pub fn observe_valid_plaintext_reception(&mut self) {
        self.connection.maintenance.recovery_required = false;
    }

    /// Check the connection-owned prerequisites for the entire proposed pause.
    ///
    /// The active LL control/encryption owners and actual protocol deadlines
    /// remain separate admission inputs. Dispatch this event's received control
    /// PDUs and validate its packets before inspection. `Ok` alone is never an RF grant.
    pub fn maintenance_skip_eligible(
        &self,
        delta: LePeripheralConnectionEventDelta,
    ) -> Result<(), SkipBlocked> {
        if delta.skipped() == 0 || delta.get() > i16::MAX as u16 {
            return Err(SkipBlocked::InvalidPause);
        }
        if matches!(self.connection.state, LePeripheralConnectionState::Created) {
            return Err(SkipBlocked::Establishment);
        }
        if !self.connection.maintenance.initial_acknowledged {
            return Err(SkipBlocked::InitialAcknowledgement);
        }
        if self.connection.maintenance.recovery_required {
            return Err(SkipBlocked::RecoveryEventRequired);
        }
        // Check every omitted event against both pending procedures, including
        // wrapping counters. The existing predecessor guard remains in force.
        let next = self.connection.event_counter;
        let skipped = delta.skipped();
        let protected = |instant: u16| {
            instant.wrapping_sub(next) < skipped
                || instant.wrapping_sub(1).wrapping_sub(next) < skipped
        };
        if let Some(update) = self.connection.pending_connection_update
            && protected(update.instant)
        {
            return Err(SkipBlocked::InstantProcedure);
        }
        if let Some(update) = self.connection.pending_channel_map
            && protected(update.instant)
        {
            return Err(SkipBlocked::InstantProcedure);
        }
        if (self.connection.pending_connection_update.is_some()
            || self.connection.pending_channel_map.is_some())
            && !matches!(
                self.connection.maintenance.instant_acknowledgement,
                InstantAcknowledgement::Confirmed
            )
        {
            return Err(SkipBlocked::InstantAcknowledgement);
        }
        Ok(())
    }

    /// Preview a deliberate pause on the original connection timeline.
    ///
    /// Cancellation returns this same completion without consuming the skip
    /// budget. Committing the successor requires one subsequently serviced
    /// event before another deliberate pause is available. Ordinary recovery
    /// after unrelated RF loss continues to use `prepare_recurring_event`.
    /// No supervision reference is changed by preview, cancellation or commit.
    #[allow(
        clippy::result_large_err,
        reason = "rejection returns the complete affine LL owner"
    )]
    pub fn prepare_maintenance_event(
        self,
        delta: LePeripheralConnectionEventDelta,
    ) -> Result<LePeripheralConnectionRecurringEventProvisional, (SkipBlocked, Self)> {
        if let Err(error) = self.maintenance_skip_eligible(delta) {
            return Err((error, self));
        }
        let mut successor = self.prepare_recurring_event(delta);
        successor.maintenance_skip = true;
        Ok(successor)
    }
}

#[cfg(test)]
mod tests;
