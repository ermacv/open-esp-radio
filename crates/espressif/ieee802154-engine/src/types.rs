//! Chip-neutral values of the MAC low-level interface.
//!
//! These are the semantic values the public ESP-IDF driver's
//! `ieee802154_ll_*` accessors exchange. Each chip's PAC keeps its own
//! register-level types; its HAL converts them to these values, so the engine
//! never sees a register image.

/// One of the four source-confirmed MAC PAN contexts.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154MultipanIndex(u8);

impl Ieee802154MultipanIndex {
    /// Number of PAN contexts.
    pub const COUNT: u8 = 4;
    /// PAN context 0, the only one without Multi-PAN.
    pub const CONTEXT0: Self = Self(0);
    /// PAN context 1.
    pub const CONTEXT1: Self = Self(1);
    /// PAN context 2.
    pub const CONTEXT2: Self = Self(2);
    /// PAN context 3.
    pub const CONTEXT3: Self = Self(3);

    /// The context `value`, or `None` beyond the last one.
    pub const fn new(value: u8) -> Option<Self> {
        if value < Self::COUNT {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The context number.
    pub const fn value(self) -> u8 {
        self.0
    }

    const fn as_usize(self) -> usize {
        self.0 as usize
    }
}

/// Semantic enable state for the four source-confirmed Multi-PAN contexts.
///
/// Hardware bit positions are owned by generated PAC field accessors. This
/// type stores one boolean per context and cannot represent a register image.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154MultipanEnableState([bool; 4]);

impl Ieee802154MultipanEnableState {
    /// No context enabled.
    pub const NONE: Self = Self([false; 4]);
    /// Every context enabled.
    pub const ALL: Self = Self([true; 4]);

    /// Construct an explicit semantic state without exposing register bits.
    pub const fn new(context0: bool, context1: bool, context2: bool, context3: bool) -> Self {
        Self([context0, context1, context2, context3])
    }

    /// The enable state of each context, in index order.
    pub const fn enabled(self) -> [bool; 4] {
        self.0
    }

    /// Whether context `index` is enabled.
    pub const fn contains(self, index: Ieee802154MultipanIndex) -> bool {
        self.0[index.as_usize()]
    }

    /// This state with context `index` enabled.
    pub const fn with(self, index: Ieee802154MultipanIndex) -> Self {
        let mut enabled = self.0;
        enabled[index.as_usize()] = true;
        Self(enabled)
    }

    /// This state with context `index` disabled.
    pub const fn without(self, index: Ieee802154MultipanIndex) -> Self {
        let mut enabled = self.0;
        enabled[index.as_usize()] = false;
        Self(enabled)
    }
}

/// One source-confirmed IEEE 802.15.4 MAC event.
///
/// Register positions belong to each chip's PAC. The enum is the semantic
/// vocabulary consumed by the IRQ state machine; both supported chips name the
/// same twelve events.
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
/// It is suitable for executor-side classification without granting
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
    /// The empty set.
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
    /// Every named event.
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
    /// The events the vendor interrupt handler processes: every named event
    /// except the clock-count match.
    pub const VENDOR_HANDLED: Self = Self {
        clock_count_match: false,
        ..Self::NAMED
    };
    /// The handled events without TIMER0, which the ACK watchdog enables
    /// per transmission.
    pub const HANDLED_BASELINE_NO_TIMER0: Self = Self {
        timer0_overflow: false,
        ..Self::VENDOR_HANDLED
    };

    /// This set with `event` added.
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
        self.tx_done as u8
            + self.rx_done as u8
            + self.ack_tx_done as u8
            + self.ack_rx_done as u8
            + self.rx_abort as u8
            + self.tx_abort as u8
            + self.ed_done as u8
            + self.timer0_overflow as u8
            + self.timer1_overflow as u8
            + self.clock_count_match as u8
            + self.tx_sfd_done as u8
            + self.rx_sfd_done as u8
            > 1
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

impl Ieee802154RxAbortReason {
    /// The reason of `ieee802154_ll_rx_abort_reason_t` value `code`.
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::RxStop),
            2 => Some(Self::SfdTimeout),
            3 => Some(Self::CrcError),
            4 => Some(Self::InvalidLength),
            5 => Some(Self::FilterFail),
            6 => Some(Self::NoRss),
            7 => Some(Self::CoexistenceBreak),
            8 => Some(Self::UnexpectedAck),
            9 => Some(Self::RxRestart),
            16 => Some(Self::TxAckTimeout),
            17 => Some(Self::TxAckStop),
            18 => Some(Self::TxAckCoexistenceBreak),
            19 => Some(Self::EnhancedAckSecurityError),
            24 => Some(Self::EdAbort),
            25 => Some(Self::EdStop),
            26 => Some(Self::EdCoexistenceReject),
            _ => None,
        }
    }
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

impl Ieee802154TxAbortReason {
    /// The reason of `ieee802154_ll_tx_abort_reason_t` value `code`.
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::RxAckStop),
            2 => Some(Self::RxAckSfdTimeout),
            3 => Some(Self::RxAckCrcError),
            4 => Some(Self::RxAckInvalidLength),
            5 => Some(Self::RxAckFilterFail),
            6 => Some(Self::RxAckNoRss),
            7 => Some(Self::RxAckCoexistenceBreak),
            8 => Some(Self::RxAckTypeNotAck),
            9 => Some(Self::RxAckRestart),
            16 => Some(Self::RxAckTimeout),
            17 => Some(Self::TxStop),
            18 => Some(Self::TxCoexistenceBreak),
            19 => Some(Self::TxSecurityError),
            24 => Some(Self::CcaFailed),
            25 => Some(Self::CcaBusy),
            _ => None,
        }
    }
}

/// Semantic classification of one sampled RX-abort reason field.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Ieee802154RxAbortReasonObservation {
    /// The field matched a source-confirmed reason.
    Named(Ieee802154RxAbortReason),
    /// The field value has no reviewed semantic identity.
    Unclassified,
}

impl Ieee802154RxAbortReasonObservation {
    /// Classify a sampled reason field.
    pub const fn from_field(code: u8) -> Self {
        match Ieee802154RxAbortReason::from_code(code) {
            Some(reason) => Self::Named(reason),
            None => Self::Unclassified,
        }
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

impl Ieee802154TxAbortReasonObservation {
    /// Classify a sampled reason field.
    pub const fn from_field(code: u8) -> Self {
        match Ieee802154TxAbortReason::from_code(code) {
            Some(reason) => Self::Named(reason),
            None => Self::Unclassified,
        }
    }
}

/// Copyable semantic `EVENT_ENABLE` or `EVENT_STATUS` observation.
///
/// This type preserves the presence of unnamed physical bits because
/// observations must not erase unexpected hardware state. It cannot be passed
/// to a write; W1C acknowledgement stays with the chip HAL.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154EventObservation {
    events: Ieee802154EventMask,
    has_unclassified: bool,
}

impl Ieee802154EventObservation {
    /// An observation of the named `events`, plus whether the sampled field
    /// also held a bit without a reviewed semantic identity. A chip HAL
    /// builds it from its register field; observations grant no write
    /// authority.
    pub const fn new(events: Ieee802154EventMask, has_unclassified: bool) -> Self {
        Self {
            events,
            has_unclassified,
        }
    }

    /// An observation of exactly the named `events`, as a register model
    /// reports it. Observations grant no write authority.
    pub const fn from_named(events: Ieee802154EventMask) -> Self {
        Self {
            events,
            has_unclassified: false,
        }
    }

    /// Return whether the complete observed event field is clear.
    pub const fn is_clear(self) -> bool {
        self.events.is_empty() && !self.has_unclassified
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
        if self.has_unclassified {
            Err(Ieee802154EventObservationError)
        } else {
            Ok(self.events)
        }
    }
}

/// Opaque three-bit receive-state observation.
///
/// Only the comparison around the publicly identified `RECEIVE_SFD` value is
/// exposed. Zero is intentionally not named `idle` until lifecycle evidence
/// proves that interpretation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154RxStateCode(u8);

impl Ieee802154RxStateCode {
    /// Largest three-bit state code.
    pub const MAX: u8 = 0x07;
    /// The state code the public LL names `RECEIVE_SFD`.
    pub const RECEIVE_SFD: u8 = 1;

    /// Whether the receiver is at `RECEIVE_SFD`.
    pub const fn is_receive_sfd(self) -> bool {
        self.0 == Self::RECEIVE_SFD
    }

    /// Whether the receiver has passed `RECEIVE_SFD`.
    pub const fn is_after_receive_sfd(self) -> bool {
        self.0 > Self::RECEIVE_SFD
    }

    /// Whether the state code is zero.
    pub const fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// Numeric read-only observation for diagnostics.
    pub const fn value(self) -> u8 {
        self.0
    }

    /// A three-bit state observation, as a register model reports it.
    pub const fn new(value: u8) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }
}

/// Energy-detection sample reduction (`ieee802154_ll_ed_sample_mode_t`).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Ieee802154EdSampleMode {
    /// Report the maximum sample.
    Maximum,
    /// Report the average sample.
    Average,
}

/// A receive-abort enable set named by the public LL.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Ieee802154RxAbortEnableSet {
    /// `TX_ACK_TIMEOUT` and `TX_ACK_COEX_BREAK`, as enabled by MAC init.
    RuntimeBaseline,
    /// `IEEE802154_RX_ABORT_ALL`.
    All,
}

/// A transmit-abort enable set named by the public LL.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Ieee802154TxAbortEnableSet {
    /// `RX_ACK_TIMEOUT`, `TX_COEX_BREAK`, `TX_SECURITY_ERROR`, `CCA_FAILED`
    /// and `CCA_BUSY`, as enabled by MAC init.
    RuntimeBaseline,
    /// `IEEE802154_TX_ABORT_ALL`.
    All,
}

/// Complete `RX_STATUS` observation (`ieee802154_ll_get_rx_status`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154RxStatus {
    filter_fail_reason: u8,
    abort_reason: Ieee802154RxAbortReasonObservation,
    state: Ieee802154RxStateCode,
    preamble_match: bool,
    sfd_match: bool,
}

impl Ieee802154RxStatus {
    /// One observation, as a register model reports it. Observations grant
    /// no write authority.
    pub const fn new(
        filter_fail_reason: u8,
        abort_reason: Ieee802154RxAbortReasonObservation,
        state: Ieee802154RxStateCode,
        preamble_match: bool,
        sfd_match: bool,
    ) -> Self {
        Self {
            filter_fail_reason,
            abort_reason,
            state,
            preamble_match,
            sfd_match,
        }
    }

    /// Raw four-bit filter-failure reason; no values are classified.
    pub const fn filter_fail_reason(&self) -> u8 {
        self.filter_fail_reason
    }

    /// Receive-abort reason of the last abort.
    pub const fn abort_reason(&self) -> Ieee802154RxAbortReasonObservation {
        self.abort_reason
    }

    /// Receiver state code.
    pub const fn state(&self) -> Ieee802154RxStateCode {
        self.state
    }

    /// `ieee802154_ll_is_current_rx_frame`: the receiver has passed the SFD
    /// of a frame.
    pub const fn frame_in_progress(&self) -> bool {
        self.state.is_after_receive_sfd()
    }

    /// Whether the preamble matched.
    pub const fn preamble_match(&self) -> bool {
        self.preamble_match
    }

    /// Whether the SFD matched.
    pub const fn sfd_match(&self) -> bool {
        self.sfd_match
    }
}

/// One diagnostic counter of the public common LL (`ieee802154_ll_get_*_cnt`).
///
/// Counters a single chip's LL adds, such as the ESP32-S31 filter and
/// preamble counters, stay with that chip's HAL.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Ieee802154DebugCounter {
    /// SFD timeouts.
    SfdTimeout,
    /// CRC failures.
    CrcError,
    /// Energy-detection aborts.
    EdAbort,
    /// CCA failures.
    CcaFail,
    /// Receive filter failures.
    RxFilterFail,
    /// Receptions without detected RSS.
    NoRssDetect,
    /// Receptions interrupted by coexistence.
    RxAbortCoex,
    /// Receive restarts.
    RxRestart,
    /// ACK transmissions interrupted by coexistence.
    TxAckAbortCoex,
    /// Energy scans interrupted by coexistence.
    EdScanBreakCoex,
    /// ACK receptions interrupted by coexistence.
    RxAckAbortCoex,
    /// ACK reception timeouts.
    RxAckTimeout,
    /// Transmissions interrupted by coexistence.
    TxBreakCoex,
    /// Transmit security failures.
    TxSecurityError,
    /// CCA busy results.
    CcaBusy,
}

/// Modem ETM channel owned by the IEEE 802.15.4 driver.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Ieee802154EtmChannel {
    /// `IEEE802154_ETM_CHANNEL0`, used for timed transmit.
    Channel0,
    /// `IEEE802154_ETM_CHANNEL1`, used for timed receive.
    Channel1,
}

/// Event-to-task route the public driver programs on its ETM channels.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Ieee802154EtmRoute {
    /// Channel zero: `ETM_EVENT_TIMER0_OVERFLOW` to `ETM_TASK_TX_START`.
    Timer0ToTxStart,
    /// Channel zero: `ETM_EVENT_TIMER0_OVERFLOW` to `ETM_TASK_ED_TRIG_TX`,
    /// a clear-channel assessment followed by transmit.
    Timer0ToCcaTx,
    /// Channel one: `ETM_EVENT_TIMER1_OVERFLOW` to `ETM_TASK_RX_START`.
    Timer1ToRxStart,
}

impl Ieee802154EtmRoute {
    /// The channel the public driver programs for this route.
    pub const fn channel(self) -> Ieee802154EtmChannel {
        match self {
            Self::Timer0ToTxStart | Self::Timer0ToCcaTx => Ieee802154EtmChannel::Channel0,
            Self::Timer1ToRxStart => Ieee802154EtmChannel::Channel1,
        }
    }
}

/// Source-confirmed clear-channel-assessment policy.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Ieee802154CcaMode {
    /// Report clear only when no carrier is detected.
    Carrier,
    /// Report clear only when measured energy is below the threshold.
    EnergyDetection,
    /// Report busy when either carrier or excess energy is detected; the
    /// channel is clear only when both checks are clear.
    CarrierOrEnergyDetection,
    /// Report busy only when both carrier and excess energy are detected; the
    /// channel is clear when either check is clear.
    CarrierAndEnergyDetection,
}
