#![no_std]
#![forbid(unsafe_code)]

//! Trace points of the IEEE 802.15.4 MAC driver.
//!
//! Every event the MAC engine, a chip HAL or a runtime records into
//! [`oer_base_trace`] is defined here with its channel, so the host decodes a
//! drained record with the same type. The types are chip-neutral and name
//! the ESP-IDF driver's vocabulary (`esp_i_dev.c`): its private
//! states, transmit errors and abort reasons. Their encodings use integer
//! operations only, so an interrupt handler may emit them with the FPU off.
//!
//! | index | event                                  | rate                                  |
//! |-------|----------------------------------------|---------------------------------------|
//! | 0     | [`StateChange`]                        | one per driver state change           |
//! | 1     | [`TxOutcome`]                          | one per transmission                  |
//! | 2     | [`RxOutcome`]                          | one per received frame                |
//! | 3     | [`Abort`]                              | one per sampled receive/transmit abort |
//! | 4     | [`Interrupt`]                          | one per MAC interrupt                 |
//! | 5     | [`PowerSequence`], [`DcdcControl`]     | one pair per transmission start       |
//! | 6     | [`Timer`]                              | one per timer arm, stop or overflow   |

use core::fmt;

use oer_base_trace::{Channel, Domain, Event, Kind};

const fn kind(event: u8) -> Kind {
    Kind::new(Domain::Ieee802154, event)
}

const fn channel(index: u8) -> Channel {
    Channel::new(Domain::Ieee802154, index)
}

const fn flag(word: u32) -> Option<bool> {
    match word {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

/// The driver's private state (`i_state_t` without test mode).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum MacState {
    Disable = 0,
    Idle = 1,
    Sleep = 2,
    Rx = 3,
    TxAck = 4,
    TxEnhAck = 5,
    TxCca = 6,
    Tx = 7,
    RxAck = 8,
    Ed = 9,
    Cca = 10,
}

impl MacState {
    const fn from_raw(raw: u32) -> Option<Self> {
        Some(match raw {
            0 => Self::Disable,
            1 => Self::Idle,
            2 => Self::Sleep,
            3 => Self::Rx,
            4 => Self::TxAck,
            5 => Self::TxEnhAck,
            6 => Self::TxCca,
            7 => Self::Tx,
            8 => Self::RxAck,
            9 => Self::Ed,
            10 => Self::Cca,
            _ => return None,
        })
    }
}

/// The driver left `from` for `to`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StateChange {
    pub from: MacState,
    pub to: MacState,
}

impl Event for StateChange {
    const KIND: Kind = kind(1);
    const CHANNEL: Channel = channel(0);

    fn encode(&self) -> [u32; 2] {
        [self.from as u32 | (self.to as u32) << 8, 0]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        if words[0] >> 16 != 0 || words[1] != 0 {
            return None;
        }
        Some(Self {
            from: MacState::from_raw(words[0] & 0xff)?,
            to: MacState::from_raw(words[0] >> 8)?,
        })
    }
}

impl fmt::Display for StateChange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ieee802154 state {:?} -> {:?}", self.from, self.to)
    }
}

/// Why a transmission failed, with its `esp_i_tx_error_t` value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum TxFailure {
    CcaBusy = 1,
    Abort = 2,
    NoAck = 3,
    InvalidAck = 4,
    Coexist = 5,
    Security = 6,
}

impl TxFailure {
    const fn from_raw(raw: u32) -> Option<Self> {
        Some(match raw {
            1 => Self::CcaBusy,
            2 => Self::Abort,
            3 => Self::NoAck,
            4 => Self::InvalidAck,
            5 => Self::Coexist,
            6 => Self::Security,
            _ => return None,
        })
    }
}

/// How a transmission ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxResult {
    /// `esp_i_transmit_done`, with or without a received ACK.
    Done { acked: bool },
    /// `esp_i_transmit_failed`.
    Failed(TxFailure),
}

/// The driver reported a transmission's end to the upper layer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxOutcome {
    /// The PHR frame length of the transmitted frame.
    pub length: u8,
    pub result: TxResult,
}

impl Event for TxOutcome {
    const KIND: Kind = kind(2);
    const CHANNEL: Channel = channel(1);

    fn encode(&self) -> [u32; 2] {
        let result = match self.result {
            TxResult::Done { acked } => acked as u32,
            TxResult::Failed(failure) => 2 | (failure as u32) << 8,
        };
        [self.length as u32, result]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        let length = u8::try_from(words[0]).ok()?;
        let result = match (words[1] & 0xff, words[1] >> 8) {
            (0, 0) => TxResult::Done { acked: false },
            (1, 0) => TxResult::Done { acked: true },
            (2, failure) => TxResult::Failed(TxFailure::from_raw(failure)?),
            _ => return None,
        };
        Some(Self { length, result })
    }
}

impl fmt::Display for TxOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ieee802154 tx len={} ", self.length)?;
        match self.result {
            TxResult::Done { acked: false } => f.write_str("done"),
            TxResult::Done { acked: true } => f.write_str("done acked"),
            TxResult::Failed(failure) => write!(f, "failed {failure:?}"),
        }
    }
}

/// Where a received frame was lost.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum RxDrop {
    /// Every receive-ring slot was held, so the frame landed in the stub
    /// buffer and the driver did not report it.
    RingFull = 0,
    /// The runtime's bounded event queue was full.
    QueueFull = 1,
}

impl RxDrop {
    const fn from_raw(raw: u32) -> Option<Self> {
        Some(match raw {
            0 => Self::RingFull,
            1 => Self::QueueFull,
            _ => return None,
        })
    }
}

/// What became of a received frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxResult {
    /// `esp_i_receive_done` with the ring slot and the frame's
    /// metadata.
    Delivered {
        slot: u8,
        channel: u8,
        rssi: i8,
        lqi: u8,
    },
    Dropped(RxDrop),
}

/// A received frame was delivered or dropped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxOutcome {
    /// The PHR frame length of the received frame.
    pub length: u8,
    pub result: RxResult,
}

impl Event for RxOutcome {
    const KIND: Kind = kind(3);
    const CHANNEL: Channel = channel(2);

    fn encode(&self) -> [u32; 2] {
        let length = self.length as u32;
        match self.result {
            RxResult::Delivered {
                slot,
                channel,
                rssi,
                lqi,
            } => [
                length | (slot as u32) << 16,
                rssi as u8 as u32 | (lqi as u32) << 8 | (channel as u32) << 16,
            ],
            RxResult::Dropped(drop) => [length | 1 << 8 | (drop as u32) << 16, 0],
        }
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        let [word, metadata] = words;
        if word >> 24 != 0 {
            return None;
        }
        let length = word as u8;
        let payload = word >> 16 & 0xff;
        let result = match word >> 8 & 0xff {
            0 if metadata >> 24 == 0 => RxResult::Delivered {
                slot: payload as u8,
                channel: (metadata >> 16) as u8,
                rssi: metadata as u8 as i8,
                lqi: (metadata >> 8) as u8,
            },
            1 if metadata == 0 => RxResult::Dropped(RxDrop::from_raw(payload)?),
            _ => return None,
        };
        Some(Self { length, result })
    }
}

impl fmt::Display for RxOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ieee802154 rx len={} ", self.length)?;
        match self.result {
            RxResult::Delivered {
                slot,
                channel,
                rssi,
                lqi,
            } => write!(
                f,
                "delivered slot={slot} channel={channel} rssi={rssi} lqi={lqi}"
            ),
            RxResult::Dropped(drop) => write!(f, "dropped {drop:?}"),
        }
    }
}

/// A receive-abort reason with its `i_ll_rx_abort_reason_t` value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum RxAbortReason {
    RxStop = 1,
    SfdTimeout = 2,
    CrcError = 3,
    InvalidLength = 4,
    FilterFail = 5,
    NoRss = 6,
    CoexistenceBreak = 7,
    UnexpectedAck = 8,
    RxRestart = 9,
    TxAckTimeout = 16,
    TxAckStop = 17,
    TxAckCoexistenceBreak = 18,
    EnhancedAckSecurityError = 19,
    EdAbort = 24,
    EdStop = 25,
    EdCoexistenceReject = 26,
}

impl RxAbortReason {
    const fn from_raw(raw: u32) -> Option<Self> {
        Some(match raw {
            1 => Self::RxStop,
            2 => Self::SfdTimeout,
            3 => Self::CrcError,
            4 => Self::InvalidLength,
            5 => Self::FilterFail,
            6 => Self::NoRss,
            7 => Self::CoexistenceBreak,
            8 => Self::UnexpectedAck,
            9 => Self::RxRestart,
            16 => Self::TxAckTimeout,
            17 => Self::TxAckStop,
            18 => Self::TxAckCoexistenceBreak,
            19 => Self::EnhancedAckSecurityError,
            24 => Self::EdAbort,
            25 => Self::EdStop,
            26 => Self::EdCoexistenceReject,
            _ => return None,
        })
    }
}

/// A transmit-abort reason with its `i_ll_tx_abort_reason_t` value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum TxAbortReason {
    RxAckStop = 1,
    RxAckSfdTimeout = 2,
    RxAckCrcError = 3,
    RxAckInvalidLength = 4,
    RxAckFilterFail = 5,
    RxAckNoRss = 6,
    RxAckCoexistenceBreak = 7,
    RxAckTypeNotAck = 8,
    RxAckRestart = 9,
    RxAckTimeout = 16,
    TxStop = 17,
    TxCoexistenceBreak = 18,
    TxSecurityError = 19,
    CcaFailed = 24,
    CcaBusy = 25,
}

impl TxAbortReason {
    const fn from_raw(raw: u32) -> Option<Self> {
        Some(match raw {
            1 => Self::RxAckStop,
            2 => Self::RxAckSfdTimeout,
            3 => Self::RxAckCrcError,
            4 => Self::RxAckInvalidLength,
            5 => Self::RxAckFilterFail,
            6 => Self::RxAckNoRss,
            7 => Self::RxAckCoexistenceBreak,
            8 => Self::RxAckTypeNotAck,
            9 => Self::RxAckRestart,
            16 => Self::RxAckTimeout,
            17 => Self::TxStop,
            18 => Self::TxCoexistenceBreak,
            19 => Self::TxSecurityError,
            24 => Self::CcaFailed,
            25 => Self::CcaBusy,
            _ => return None,
        })
    }
}

/// A sampled abort reason; `None` is a value with no reviewed identity,
/// which stops the driver right after.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AbortReason {
    Rx(Option<RxAbortReason>),
    Tx(Option<TxAbortReason>),
}

/// The interrupt handler sampled a receive or transmit abort.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Abort {
    /// The driver state the abort arrived in.
    pub state: MacState,
    pub reason: AbortReason,
}

impl Event for Abort {
    const KIND: Kind = kind(4);
    const CHANNEL: Channel = channel(3);

    fn encode(&self) -> [u32; 2] {
        let (direction, code) = match self.reason {
            AbortReason::Rx(reason) => (0, reason.map_or(0, |reason| reason as u32)),
            AbortReason::Tx(reason) => (1, reason.map_or(0, |reason| reason as u32)),
        };
        [self.state as u32 | direction << 8 | code << 16, 0]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        let [word, zero] = words;
        if zero != 0 || word >> 24 != 0 {
            return None;
        }
        let code = word >> 16;
        let reason = match (word >> 8 & 0xff, code) {
            (0, 0) => AbortReason::Rx(None),
            (0, code) => AbortReason::Rx(Some(RxAbortReason::from_raw(code)?)),
            (1, 0) => AbortReason::Tx(None),
            (1, code) => AbortReason::Tx(Some(TxAbortReason::from_raw(code)?)),
            _ => return None,
        };
        Some(Self {
            state: MacState::from_raw(word & 0xff)?,
            reason,
        })
    }
}

impl fmt::Display for Abort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ieee802154 abort in {:?}: ", self.state)?;
        match self.reason {
            AbortReason::Rx(Some(reason)) => write!(f, "rx {reason:?}"),
            AbortReason::Tx(Some(reason)) => write!(f, "tx {reason:?}"),
            AbortReason::Rx(None) => f.write_str("rx unclassified"),
            AbortReason::Tx(None) => f.write_str("tx unclassified"),
        }
    }
}

/// The MAC events one interrupt sampled, one bit per event in the order of
/// the associated constants.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct InterruptEvents(u16);

impl InterruptEvents {
    pub const NONE: Self = Self(0);
    pub const TX_DONE: Self = Self(1 << 0);
    pub const RX_DONE: Self = Self(1 << 1);
    pub const ACK_TX_DONE: Self = Self(1 << 2);
    pub const ACK_RX_DONE: Self = Self(1 << 3);
    pub const RX_ABORT: Self = Self(1 << 4);
    pub const TX_ABORT: Self = Self(1 << 5);
    pub const ED_DONE: Self = Self(1 << 6);
    pub const TIMER0_OVERFLOW: Self = Self(1 << 7);
    pub const TIMER1_OVERFLOW: Self = Self(1 << 8);
    pub const CLOCK_COUNT_MATCH: Self = Self(1 << 9);
    pub const TX_SFD_DONE: Self = Self(1 << 10);
    pub const RX_SFD_DONE: Self = Self(1 << 11);
    /// The sample also held an event with no reviewed identity, which stops
    /// the driver right after.
    pub const UNCLASSIFIED: Self = Self(1 << 15);

    const VALID: u16 = 0x0fff | Self::UNCLASSIFIED.0;
    const NAMES: [(Self, &'static str); 13] = [
        (Self::TX_DONE, "tx_done"),
        (Self::RX_DONE, "rx_done"),
        (Self::ACK_TX_DONE, "ack_tx_done"),
        (Self::ACK_RX_DONE, "ack_rx_done"),
        (Self::RX_ABORT, "rx_abort"),
        (Self::TX_ABORT, "tx_abort"),
        (Self::ED_DONE, "ed_done"),
        (Self::TIMER0_OVERFLOW, "timer0_overflow"),
        (Self::TIMER1_OVERFLOW, "timer1_overflow"),
        (Self::CLOCK_COUNT_MATCH, "clock_count_match"),
        (Self::TX_SFD_DONE, "tx_sfd_done"),
        (Self::RX_SFD_DONE, "rx_sfd_done"),
        (Self::UNCLASSIFIED, "unclassified"),
    ];

    /// This set with `other`'s events added.
    pub const fn with(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// This set with `other`'s events added when `present`.
    pub const fn with_if(self, other: Self, present: bool) -> Self {
        if present { self.with(other) } else { self }
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    const fn from_raw(raw: u32) -> Option<Self> {
        if raw & !(Self::VALID as u32) != 0 {
            return None;
        }
        Some(Self(raw as u16))
    }
}

impl fmt::Display for InterruptEvents {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for (event, name) in Self::NAMES {
            if self.contains(event) {
                if !first {
                    f.write_str("+")?;
                }
                f.write_str(name)?;
                first = false;
            }
        }
        if first {
            f.write_str("none")?;
        }
        Ok(())
    }
}

/// The MAC interrupt handler ran.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Interrupt {
    /// The driver state at entry.
    pub state: MacState,
    pub events: InterruptEvents,
}

impl Event for Interrupt {
    const KIND: Kind = kind(5);
    const CHANNEL: Channel = channel(4);

    fn encode(&self) -> [u32; 2] {
        [self.events.0 as u32, self.state as u32]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        Some(Self {
            state: MacState::from_raw(words[1])?,
            events: InterruptEvents::from_raw(words[0])?,
        })
    }
}

impl fmt::Display for Interrupt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ieee802154 interrupt in {:?}: {}",
            self.state, self.events
        )
    }
}

/// The MAC's power-sequencing delays when a transmission starts, in the
/// units and widths of the ESP-IDF register fields (`PAON_DELAY`,
/// `TXON_DELAY`, `TXEN_STOP_DLY`, `TXOFF_DELAY`, `RXON_DELAY`,
/// `TXRX_SWITCH_DELAY`, `CONT_RX_DELAY`): 10, 10, 6, 6, 11, 10 and 6 bits.
/// Wider values are truncated to those widths.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PowerSequence {
    pub pa_on: u16,
    pub tx_on: u16,
    pub tx_enable_stop: u8,
    pub tx_off: u8,
    pub rx_on: u16,
    pub txrx_switch: u16,
    pub continuous_rx: u8,
}

impl Event for PowerSequence {
    const KIND: Kind = kind(6);
    const CHANNEL: Channel = channel(5);

    fn encode(&self) -> [u32; 2] {
        [
            (self.pa_on as u32 & 0x3ff)
                | (self.tx_on as u32 & 0x3ff) << 10
                | (self.tx_enable_stop as u32 & 0x3f) << 20
                | (self.tx_off as u32 & 0x3f) << 26,
            (self.rx_on as u32 & 0x7ff)
                | (self.txrx_switch as u32 & 0x3ff) << 11
                | (self.continuous_rx as u32 & 0x3f) << 21,
        ]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        let [transmit, receive] = words;
        if receive >> 27 != 0 {
            return None;
        }
        Some(Self {
            pa_on: (transmit & 0x3ff) as u16,
            tx_on: (transmit >> 10 & 0x3ff) as u16,
            tx_enable_stop: (transmit >> 20 & 0x3f) as u8,
            tx_off: (transmit >> 26) as u8,
            rx_on: (receive & 0x7ff) as u16,
            txrx_switch: (receive >> 11 & 0x3ff) as u16,
            continuous_rx: (receive >> 21) as u8,
        })
    }
}

impl fmt::Display for PowerSequence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ieee802154 power sequence pa_on={} tx_on={} txen_stop={} tx_off={} rx_on={} \
             txrx_switch={} cont_rx={}",
            self.pa_on,
            self.tx_on,
            self.tx_enable_stop,
            self.tx_off,
            self.rx_on,
            self.txrx_switch,
            self.continuous_rx
        )
    }
}

/// The MAC's DC-DC control when a transmission starts (`DCDC_CTRL`), and
/// which of the eight power-sequencing words, from `PAON_DELAY` to
/// `DCDC_CTRL`, had vendor-reserved bits set.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DcdcControl {
    pub pre_raise: u8,
    pub drop: u8,
    pub enabled: bool,
    pub raise_for_tx: bool,
    /// Bit `n` is set when word `n` had a vendor-reserved bit set.
    pub reserved_set: u8,
}

impl DcdcControl {
    /// The [`Self::reserved_set`] of the eight words' vendor-reserved
    /// remainders, in address order.
    pub fn reserved_set_of(reserved: [u32; 8]) -> u8 {
        let mut set = 0;
        for (index, bits) in reserved.into_iter().enumerate() {
            if bits != 0 {
                set |= 1 << index;
            }
        }
        set
    }
}

impl Event for DcdcControl {
    const KIND: Kind = kind(7);
    const CHANNEL: Channel = channel(5);

    fn encode(&self) -> [u32; 2] {
        [
            self.pre_raise as u32
                | (self.drop as u32) << 8
                | (self.enabled as u32) << 16
                | (self.raise_for_tx as u32) << 17,
            self.reserved_set as u32,
        ]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        let [control, reserved] = words;
        if control >> 18 != 0 {
            return None;
        }
        Some(Self {
            pre_raise: control as u8,
            drop: (control >> 8) as u8,
            enabled: flag(control >> 16 & 1)?,
            raise_for_tx: flag(control >> 17)?,
            reserved_set: u8::try_from(reserved).ok()?,
        })
    }
}

impl fmt::Display for DcdcControl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ieee802154 dcdc pre_up={} down={} ctrl_en={} tx_dcdc_up={} reserved_set={:#04x}",
            self.pre_raise,
            self.drop,
            self.enabled as u8,
            self.raise_for_tx as u8,
            self.reserved_set
        )
    }
}

/// One of the MAC's two event timers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum TimerId {
    Timer0 = 0,
    Timer1 = 1,
}

impl TimerId {
    const fn from_raw(raw: u32) -> Option<Self> {
        Some(match raw {
            0 => Self::Timer0,
            1 => Self::Timer1,
            _ => return None,
        })
    }
}

/// The callback a timer carries (`i_timer*_set_callback`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum TimerCallback {
    /// No callback: an armed timer starts an operation through the event
    /// task matrix alone.
    None = 0,
    AckTimeout = 1,
    StartReceiveAt = 2,
    FinishReceiveAt = 3,
}

impl TimerCallback {
    const fn from_raw(raw: u32) -> Option<Self> {
        Some(match raw {
            0 => Self::None,
            1 => Self::AckTimeout,
            2 => Self::StartReceiveAt,
            3 => Self::FinishReceiveAt,
            _ => return None,
        })
    }
}

/// What happened to a timer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimerOp {
    /// Armed to fire at the truncated `esp_timer` microsecond `at`.
    Armed {
        callback: TimerCallback,
        at: u32,
    },
    Stopped,
    /// Overflowed and ran `callback`.
    Fired(TimerCallback),
}

/// A MAC timer was armed, stopped or fired.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Timer {
    pub timer: TimerId,
    pub op: TimerOp,
}

impl Event for Timer {
    const KIND: Kind = kind(8);
    const CHANNEL: Channel = channel(6);

    fn encode(&self) -> [u32; 2] {
        let timer = self.timer as u32;
        match self.op {
            TimerOp::Armed { callback, at } => [timer | (callback as u32) << 16, at],
            TimerOp::Stopped => [timer | 1 << 8, 0],
            TimerOp::Fired(callback) => [timer | 2 << 8 | (callback as u32) << 16, 0],
        }
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        let [word, at] = words;
        if word >> 24 != 0 {
            return None;
        }
        let callback = word >> 16;
        let op = match (word >> 8 & 0xff, callback, at) {
            (0, callback, at) => TimerOp::Armed {
                callback: TimerCallback::from_raw(callback)?,
                at,
            },
            (1, 0, 0) => TimerOp::Stopped,
            (2, callback, 0) => TimerOp::Fired(TimerCallback::from_raw(callback)?),
            _ => return None,
        };
        Some(Self {
            timer: TimerId::from_raw(word & 0xff)?,
            op,
        })
    }
}

impl fmt::Display for Timer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ieee802154 {:?} ", self.timer)?;
        match self.op {
            TimerOp::Armed { callback, at } => write!(f, "armed at {at} for {callback:?}"),
            TimerOp::Stopped => f.write_str("stopped"),
            TimerOp::Fired(callback) => write!(f, "fired {callback:?}"),
        }
    }
}

oer_base_trace::event_set!(
    pub Ieee802154Trace: StateChange,
    TxOutcome,
    RxOutcome,
    Abort,
    Interrupt,
    PowerSequence,
    DcdcControl,
    Timer,
);

#[cfg(test)]
mod tests;
