// SPDX-License-Identifier: Apache-2.0
//! Events, abort reasons and their enable and observation vocabularies.

/// One source-confirmed IEEE 802.15.4 MAC event.
///
/// Register positions remain an implementation detail of this PAC type. The
/// enum itself is the semantic vocabulary consumed by the IRQ state machine.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Ieee802154Event {
    /// A transmission completed.
    TxDone,
    /// A reception completed.
    RxDone,
    /// Automatic ACK transmission completed.
    AckTxDone,
    /// ACK reception completed.
    AckRxDone,
    /// Receive processing aborted.
    RxAbort,
    /// Transmit processing aborted.
    TxAbort,
    /// Energy detection completed.
    EdDone,
    /// TIMER0 overflowed.
    Timer0Overflow,
    /// TIMER1 overflowed.
    Timer1Overflow,
    /// The MAC clock counter matched its configured value.
    ClockCountMatch,
    /// Transmission SFD processing completed.
    TxSfdDone,
    /// Reception SFD processing completed.
    RxSfdDone,
}

impl Ieee802154Event {
    /// Return this event as a validated semantic event set.
    pub const fn mask(self) -> Ieee802154EventMask {
        Ieee802154EventMask::NONE.with(self)
    }
}

/// A sampled event field contained at least one event without a reviewed
/// semantic identity.
///
/// The physical field image remains private to [`Ieee802154EventObservation`].
/// This error deliberately exposes no raw positions while still forcing every
/// consumer to reject an unclassified sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154EventObservationError;

/// A semantic set containing only source-confirmed MAC events.
///
/// Unlike [`Ieee802154EventEnableState`], this value is not accepted by any PAC
/// writer. It is suitable for executor-side classification without granting
/// event-enable authority.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154EventMask {
    tx_done: bool,
    rx_done: bool,
    ack_tx_done: bool,
    ack_rx_done: bool,
    rx_abort: bool,
    tx_abort: bool,
    ed_done: bool,
    timer0_overflow: bool,
    timer1_overflow: bool,
    clock_count_match: bool,
    tx_sfd_done: bool,
    rx_sfd_done: bool,
}

impl Ieee802154EventMask {
    pub const NONE: Self = Self {
        tx_done: false,
        rx_done: false,
        ack_tx_done: false,
        ack_rx_done: false,
        rx_abort: false,
        tx_abort: false,
        ed_done: false,
        timer0_overflow: false,
        timer1_overflow: false,
        clock_count_match: false,
        tx_sfd_done: false,
        rx_sfd_done: false,
    };
    pub const NAMED: Self = Self {
        tx_done: true,
        rx_done: true,
        ack_tx_done: true,
        ack_rx_done: true,
        rx_abort: true,
        tx_abort: true,
        ed_done: true,
        timer0_overflow: true,
        timer1_overflow: true,
        clock_count_match: true,
        tx_sfd_done: true,
        rx_sfd_done: true,
    };
    pub const VENDOR_HANDLED: Self = Self {
        clock_count_match: false,
        ..Self::NAMED
    };
    pub const HANDLED_BASELINE_NO_TIMER0: Self = Self {
        timer0_overflow: false,
        ..Self::VENDOR_HANDLED
    };

    /// Add `event` to the set.
    pub const fn with(mut self, event: Ieee802154Event) -> Self {
        match event {
            Ieee802154Event::TxDone => self.tx_done = true,
            Ieee802154Event::RxDone => self.rx_done = true,
            Ieee802154Event::AckTxDone => self.ack_tx_done = true,
            Ieee802154Event::AckRxDone => self.ack_rx_done = true,
            Ieee802154Event::RxAbort => self.rx_abort = true,
            Ieee802154Event::TxAbort => self.tx_abort = true,
            Ieee802154Event::EdDone => self.ed_done = true,
            Ieee802154Event::Timer0Overflow => self.timer0_overflow = true,
            Ieee802154Event::Timer1Overflow => self.timer1_overflow = true,
            Ieee802154Event::ClockCountMatch => self.clock_count_match = true,
            Ieee802154Event::TxSfdDone => self.tx_sfd_done = true,
            Ieee802154Event::RxSfdDone => self.rx_sfd_done = true,
        }
        self
    }

    const fn same_as(self, other: Self) -> bool {
        self.tx_done == other.tx_done
            && self.rx_done == other.rx_done
            && self.ack_tx_done == other.ack_tx_done
            && self.ack_rx_done == other.ack_rx_done
            && self.rx_abort == other.rx_abort
            && self.tx_abort == other.tx_abort
            && self.ed_done == other.ed_done
            && self.timer0_overflow == other.timer0_overflow
            && self.timer1_overflow == other.timer1_overflow
            && self.clock_count_match == other.clock_count_match
            && self.tx_sfd_done == other.tx_sfd_done
            && self.rx_sfd_done == other.rx_sfd_done
    }

    /// Return whether the semantic event set is empty.
    pub const fn is_empty(self) -> bool {
        self.same_as(Self::NONE)
    }

    /// Return whether this set contains `event`.
    pub const fn contains(self, event: Ieee802154Event) -> bool {
        match event {
            Ieee802154Event::TxDone => self.tx_done,
            Ieee802154Event::RxDone => self.rx_done,
            Ieee802154Event::AckTxDone => self.ack_tx_done,
            Ieee802154Event::AckRxDone => self.ack_rx_done,
            Ieee802154Event::RxAbort => self.rx_abort,
            Ieee802154Event::TxAbort => self.tx_abort,
            Ieee802154Event::EdDone => self.ed_done,
            Ieee802154Event::Timer0Overflow => self.timer0_overflow,
            Ieee802154Event::Timer1Overflow => self.timer1_overflow,
            Ieee802154Event::ClockCountMatch => self.clock_count_match,
            Ieee802154Event::TxSfdDone => self.tx_sfd_done,
            Ieee802154Event::RxSfdDone => self.rx_sfd_done,
        }
    }

    /// Combine two already classified event sets.
    pub const fn union(self, other: Self) -> Self {
        Self {
            tx_done: self.tx_done || other.tx_done,
            rx_done: self.rx_done || other.rx_done,
            ack_tx_done: self.ack_tx_done || other.ack_tx_done,
            ack_rx_done: self.ack_rx_done || other.ack_rx_done,
            rx_abort: self.rx_abort || other.rx_abort,
            tx_abort: self.tx_abort || other.tx_abort,
            ed_done: self.ed_done || other.ed_done,
            timer0_overflow: self.timer0_overflow || other.timer0_overflow,
            timer1_overflow: self.timer1_overflow || other.timer1_overflow,
            clock_count_match: self.clock_count_match || other.clock_count_match,
            tx_sfd_done: self.tx_sfd_done || other.tx_sfd_done,
            rx_sfd_done: self.rx_sfd_done || other.rx_sfd_done,
        }
    }

    /// Return events present in `self` but absent from `allowed`.
    pub const fn difference(self, allowed: Self) -> Self {
        Self {
            tx_done: self.tx_done && !allowed.tx_done,
            rx_done: self.rx_done && !allowed.rx_done,
            ack_tx_done: self.ack_tx_done && !allowed.ack_tx_done,
            ack_rx_done: self.ack_rx_done && !allowed.ack_rx_done,
            rx_abort: self.rx_abort && !allowed.rx_abort,
            tx_abort: self.tx_abort && !allowed.tx_abort,
            ed_done: self.ed_done && !allowed.ed_done,
            timer0_overflow: self.timer0_overflow && !allowed.timer0_overflow,
            timer1_overflow: self.timer1_overflow && !allowed.timer1_overflow,
            clock_count_match: self.clock_count_match && !allowed.clock_count_match,
            tx_sfd_done: self.tx_sfd_done && !allowed.tx_sfd_done,
            rx_sfd_done: self.rx_sfd_done && !allowed.rx_sfd_done,
        }
    }

    /// Return events present in both semantic sets.
    pub const fn intersection(self, other: Self) -> Self {
        Self {
            tx_done: self.tx_done && other.tx_done,
            rx_done: self.rx_done && other.rx_done,
            ack_tx_done: self.ack_tx_done && other.ack_tx_done,
            ack_rx_done: self.ack_rx_done && other.ack_rx_done,
            rx_abort: self.rx_abort && other.rx_abort,
            tx_abort: self.tx_abort && other.tx_abort,
            ed_done: self.ed_done && other.ed_done,
            timer0_overflow: self.timer0_overflow && other.timer0_overflow,
            timer1_overflow: self.timer1_overflow && other.timer1_overflow,
            clock_count_match: self.clock_count_match && other.clock_count_match,
            tx_sfd_done: self.tx_sfd_done && other.tx_sfd_done,
            rx_sfd_done: self.rx_sfd_done && other.rx_sfd_done,
        }
    }

    /// Return whether the set contains more than one semantic event.
    pub const fn has_multiple(self) -> bool {
        let mut remaining: &[bool] = &[
            self.tx_done,
            self.rx_done,
            self.ack_tx_done,
            self.ack_rx_done,
            self.rx_abort,
            self.tx_abort,
            self.ed_done,
            self.timer0_overflow,
            self.timer1_overflow,
            self.clock_count_match,
            self.tx_sfd_done,
            self.rx_sfd_done,
        ];
        let mut seen = false;
        while let Some((&present, rest)) = remaining.split_first() {
            if present {
                if seen {
                    return true;
                }
                seen = true;
            }
            remaining = rest;
        }
        false
    }

    /// Collapse this named set into the closed diagnostic vocabulary.
    pub const fn state(self) -> Ieee802154ObservedEventState {
        Ieee802154ObservedEventState::from_mask(self)
    }
}

impl From<Ieee802154Event> for Ieee802154EventMask {
    fn from(event: Ieee802154Event) -> Self {
        event.mask()
    }
}

/// Closed semantic summary of one complete MAC event observation.
///
/// Register positions remain private to the PAC. The two validation probes and
/// production ED diagnostics need only these exact relations; every other
/// source-confirmed combination is retained as `UnexpectedNamed`, while an
/// unnamed physical event remains fail-closed as `Unclassified`.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Ieee802154ObservedEventState {
    #[default]
    Clear,
    Timer0Only,
    Timer1Only,
    Timer0AndTimer1,
    EdDoneOnly,
    EdDoneAndTimer0,
    RxAbortOnly,
    RxAbortWithOther,
    EdDoneWithOther,
    EdDoneAndRxAbortWithOther,
    UnexpectedNamed,
    Unclassified,
}

impl Ieee802154ObservedEventState {
    const fn from_mask(events: Ieee802154EventMask) -> Self {
        if events.is_empty() {
            Self::Clear
        } else if events.same_as(Ieee802154Event::Timer0Overflow.mask()) {
            Self::Timer0Only
        } else if events.same_as(Ieee802154Event::Timer1Overflow.mask()) {
            Self::Timer1Only
        } else if events.same_as(
            Ieee802154Event::Timer0Overflow
                .mask()
                .union(Ieee802154Event::Timer1Overflow.mask()),
        ) {
            Self::Timer0AndTimer1
        } else if events.same_as(Ieee802154Event::EdDone.mask()) {
            Self::EdDoneOnly
        } else if events.same_as(
            Ieee802154Event::EdDone
                .mask()
                .union(Ieee802154Event::Timer0Overflow.mask()),
        ) {
            Self::EdDoneAndTimer0
        } else if events.same_as(Ieee802154Event::RxAbort.mask()) {
            Self::RxAbortOnly
        } else if events.contains(Ieee802154Event::EdDone)
            && events.contains(Ieee802154Event::RxAbort)
        {
            Self::EdDoneAndRxAbortWithOther
        } else if events.contains(Ieee802154Event::RxAbort) {
            Self::RxAbortWithOther
        } else if events.contains(Ieee802154Event::EdDone) {
            Self::EdDoneWithOther
        } else {
            Self::UnexpectedNamed
        }
    }

    /// Return whether the observation is clear.
    pub const fn is_clear(self) -> bool {
        matches!(self, Self::Clear)
    }

    /// Return whether TIMER0 is part of this classified observation.
    pub const fn has_timer0(self) -> bool {
        matches!(
            self,
            Self::Timer0Only | Self::Timer0AndTimer1 | Self::EdDoneAndTimer0
        )
    }

    /// Return whether TIMER1 is part of this classified observation.
    pub const fn has_timer1(self) -> bool {
        matches!(self, Self::Timer1Only | Self::Timer0AndTimer1)
    }

    /// Return whether ED-DONE is part of this classified observation.
    pub const fn has_ed_done(self) -> bool {
        matches!(
            self,
            Self::EdDoneOnly
                | Self::EdDoneAndTimer0
                | Self::EdDoneWithOther
                | Self::EdDoneAndRxAbortWithOther
        )
    }

    /// Return whether RX-ABORT is part of this classified observation.
    pub const fn has_rx_abort(self) -> bool {
        matches!(
            self,
            Self::RxAbortOnly | Self::RxAbortWithOther | Self::EdDoneAndRxAbortWithOther
        )
    }

    /// Return whether this is exactly the classified RX-abort observation.
    pub const fn is_rx_abort_only(self) -> bool {
        matches!(self, Self::RxAbortOnly)
    }

    /// Combine observations without exposing their physical encoding.
    pub fn union(self, other: Self) -> Self {
        use Ieee802154ObservedEventState as State;
        if matches!(self, State::Unclassified) || matches!(other, State::Unclassified) {
            return State::Unclassified;
        }
        if self == State::Clear {
            return other;
        }
        if other == State::Clear || self == other {
            return self;
        }

        let ed_done = self.has_ed_done() || other.has_ed_done();
        let rx_abort = self.has_rx_abort() || other.has_rx_abort();
        let timer0 = self.has_timer0() || other.has_timer0();
        let timer1 = self.has_timer1() || other.has_timer1();
        let has_opaque_other = matches!(
            self,
            State::RxAbortWithOther
                | State::EdDoneWithOther
                | State::EdDoneAndRxAbortWithOther
                | State::UnexpectedNamed
        ) || matches!(
            other,
            State::RxAbortWithOther
                | State::EdDoneWithOther
                | State::EdDoneAndRxAbortWithOther
                | State::UnexpectedNamed
        );

        if ed_done && rx_abort {
            State::EdDoneAndRxAbortWithOther
        } else if has_opaque_other && rx_abort {
            State::RxAbortWithOther
        } else if has_opaque_other && ed_done {
            State::EdDoneWithOther
        } else if has_opaque_other {
            State::UnexpectedNamed
        } else if ed_done && timer0 && !timer1 {
            State::EdDoneAndTimer0
        } else if rx_abort && !ed_done && !timer0 && !timer1 {
            State::RxAbortOnly
        } else if rx_abort {
            State::RxAbortWithOther
        } else if ed_done && !timer0 && !timer1 {
            State::EdDoneOnly
        } else if ed_done {
            State::EdDoneWithOther
        } else if timer0 && timer1 {
            State::Timer0AndTimer1
        } else if timer0 {
            State::Timer0Only
        } else if timer1 {
            State::Timer1Only
        } else {
            State::UnexpectedNamed
        }
    }
}

/// Semantic readback of the validation-owned event-enable field.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Ieee802154ValidationEventEnableState {
    #[default]
    AllMasked,
    TimerPairOnly,
    EdDoneTimer0RxAbortOnly,
    Unexpected,
}

/// Semantic readback of the fixed validation ED duration.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Ieee802154ValidationEdDurationState {
    ValidationEight,
    #[default]
    Other,
}

impl Ieee802154ValidationEdDurationState {
    #[doc(hidden)]
    pub const fn from_field(value: u32) -> Self {
        if value == 8 {
            Self::ValidationEight
        } else {
            Self::Other
        }
    }
}

/// One source-confirmed receive-abort reason.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Ieee802154RxAbortReason {
    /// Receive stop command.
    RxStop,
    /// SFD timeout.
    SfdTimeout,
    /// CRC failure.
    CrcError,
    /// Invalid frame length.
    InvalidLength,
    /// Address or filter rejection.
    FilterFail,
    /// RSS was not detected.
    NoRss,
    /// Coexistence interrupted reception.
    CoexistenceBreak,
    /// An ACK was received unexpectedly.
    UnexpectedAck,
    /// Receive processing restarted.
    RxRestart,
    /// ACK transmission timed out.
    TxAckTimeout,
    /// ACK transmission was stopped.
    TxAckStop,
    /// Coexistence interrupted ACK transmission.
    TxAckCoexistenceBreak,
    /// Enhanced-ACK security processing failed.
    EnhancedAckSecurityError,
    /// Energy detection was aborted.
    EdAbort,
    /// Energy detection was stopped.
    EdStop,
    /// Coexistence rejected energy detection.
    EdCoexistenceReject,
}

/// One source-confirmed transmit-abort reason.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Ieee802154TxAbortReason {
    /// ACK reception was stopped.
    RxAckStop,
    /// ACK SFD timed out.
    RxAckSfdTimeout,
    /// The received ACK had a CRC failure.
    RxAckCrcError,
    /// The received ACK had an invalid length.
    RxAckInvalidLength,
    /// The received ACK failed filtering.
    RxAckFilterFail,
    /// RSS was not detected for the ACK.
    RxAckNoRss,
    /// Coexistence interrupted ACK reception.
    RxAckCoexistenceBreak,
    /// The received frame was not an ACK.
    RxAckTypeNotAck,
    /// ACK receive processing restarted.
    RxAckRestart,
    /// ACK reception timed out.
    RxAckTimeout,
    /// Transmission was stopped.
    TxStop,
    /// Coexistence interrupted transmission.
    TxCoexistenceBreak,
    /// Transmission security processing failed.
    TxSecurityError,
    /// CCA failed.
    CcaFailed,
    /// CCA observed a busy channel.
    CcaBusy,
}

/// Semantic classification of one sampled RX-abort reason field.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Ieee802154RxAbortReasonObservation {
    /// The field matched a source-confirmed reason.
    Named(Ieee802154RxAbortReason),
    /// The field value has no reviewed semantic identity.
    Unclassified,
}

impl From<Ieee802154RxAbortReason> for Ieee802154RxAbortReasonObservation {
    fn from(reason: Ieee802154RxAbortReason) -> Self {
        Self::Named(reason)
    }
}

/// Semantic classification of one sampled TX-abort reason field.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Ieee802154TxAbortReasonObservation {
    /// The field matched a source-confirmed reason.
    Named(Ieee802154TxAbortReason),
    /// The field value has no reviewed semantic identity.
    Unclassified,
}

impl From<Ieee802154TxAbortReason> for Ieee802154TxAbortReasonObservation {
    fn from(reason: Ieee802154TxAbortReason) -> Self {
        Self::Named(reason)
    }
}

/// Closed semantic `EVENT_ENABLE` states accepted by finite polled operations.
///
/// Register geometry and physical images remain exclusively in generated PAC
/// accessors. No integer conversion exists in either direction.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Ieee802154EventEnableState {
    #[default]
    AllMasked,
    EdOperation,
}

/// Closed semantic `RX_ABORT_ENABLE` states accepted by finite ED/CCA work.
///
/// The runtime interrupt baseline is owned by the complete activation
/// transaction and is intentionally not constructible through this API.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Ieee802154RxAbortEnableState {
    #[default]
    AllMasked,
    EdOperationReasons,
}

/// One closed production plan for activating the IEEE 802.15.4 IRQ owner.
///
/// Register images are selected only by generated accessors inside the raw PAC
/// owner; this marker grants no field or integer authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[doc(hidden)]
pub struct Ieee802154InterruptActivationPlan;

impl Ieee802154InterruptActivationPlan {
    pub const SOURCE_CONFIRMED_BASELINE: Self = Self;
}

/// Semantic readback of the event-enable field owned by a polled ED/CCA
/// operation.
///
/// `Unexpected` deliberately combines every other image of the field. It
/// never projects an unexpected image into a writable mask.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154OperationEventEnableObservation {
    /// Every event is masked.
    AllMasked,
    /// Exactly `ED_DONE` and `RX_ABORT` are enabled.
    EdDoneAndRxAbortOnly,
    /// A required event is missing or at least one other event is enabled.
    Unexpected,
}

/// Semantic readback of the receive-abort-enable field owned by a polled
/// ED/CCA operation.
///
/// The observation has no conversion to [`Ieee802154RxAbortEnableState`], so
/// unexpected hardware state cannot accidentally become a writable image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154OperationRxAbortEnableObservation {
    /// Every receive-abort reason is masked.
    AllMasked,
    /// Exactly the three ED-operation reasons are enabled.
    EdOperationReasonsOnly,
    /// A required reason is missing or at least one other reason is enabled.
    Unexpected,
}

/// Copyable semantic `EVENT_ENABLE` or `EVENT_STATUS` observation.
///
/// Unlike [`Ieee802154EventEnableState`], this type preserves unnamed physical
/// lanes separately because observations must not erase unexpected hardware
/// state. Construction grants no write authority. W1C acknowledgement consumes
/// a separate affine snapshot.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154EventObservation {
    events: Ieee802154EventMask,
    // SOURCE: chips/esp32s31/registers/types/ieee802154.toml, EVENT_STATUS.
    // Neither lane is assigned a hardware meaning.
    events_7: bool,
    events_13: bool,
}

impl Ieee802154EventObservation {
    /// Combine named events with the individual observations of lanes 7 and 13.
    /// A chip without a lane supplies `false` for that lane.
    ///
    /// The lanes are retained independently without assigning either a meaning.
    /// Constructing this value grants no acknowledgement or write authority.
    pub const fn from_parts(events: Ieee802154EventMask, lane_7: bool, lane_13: bool) -> Self {
        Self {
            events,
            events_7: lane_7,
            events_13: lane_13,
        }
    }

    /// An observation of exactly the named `events`, as a register model
    /// reports it. Observations grant no write authority.
    pub const fn from_named(events: Ieee802154EventMask) -> Self {
        Self::from_parts(events, false, false)
    }

    /// Return whether the complete observed event field is clear.
    pub const fn is_clear(self) -> bool {
        match self.classification() {
            Ok(named) => named.is_empty(),
            Err(_) => false,
        }
    }

    /// Observed presence of unnamed lane 7.
    pub const fn events_7(self) -> bool {
        self.events_7
    }

    /// Observed presence of unnamed lane 13.
    pub const fn events_13(self) -> bool {
        self.events_13
    }

    /// Return whether every named event in `required` was observed.
    pub const fn contains(self, required: Ieee802154Event) -> bool {
        self.events.contains(required)
    }

    /// Classify the complete observation as a semantic event set.
    ///
    /// Any unnamed physical event produces an opaque error. Neither branch
    /// exposes the underlying register image, and the named set is not write
    /// authority.
    pub const fn classification(
        self,
    ) -> Result<Ieee802154EventMask, Ieee802154EventObservationError> {
        if self.events_7 || self.events_13 {
            Err(Ieee802154EventObservationError)
        } else {
            Ok(self.events)
        }
    }

    /// Collapse the complete observation into the closed semantic vocabulary.
    pub const fn state(self) -> Ieee802154ObservedEventState {
        match self.classification() {
            Ok(events) => Ieee802154ObservedEventState::from_mask(events),
            Err(_) => Ieee802154ObservedEventState::Unclassified,
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    reason = "tests assert behavior and compare with reference arithmetic"
)]
mod tests;
