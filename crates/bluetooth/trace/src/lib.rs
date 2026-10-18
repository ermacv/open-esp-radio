#![no_std]
#![forbid(unsafe_code)]

//! Trace points of the Bluetooth LE Controller, the ESP32-S31 radio role
//! and its runtime.
//!
//! Every event they record into [`oer_trace`] is defined here with its
//! channel in `Domain::Bluetooth`, so the host decodes a drained record with
//! the same type. One channel gates one event, so a session can leave the
//! per-PDU reports off while it records the connection's steps. The
//! channels and where each event is emitted are listed in this crate's
//! README.

use core::fmt;

use oer_trace::{Channel, Domain, Event, Kind};

const fn kind(event: u8) -> Kind {
    Kind::new(Domain::Bluetooth, event)
}

const fn channel(index: u8) -> Channel {
    Channel::new(Domain::Bluetooth, index)
}

/// Declare a fieldless enum whose variants encode as their discriminant.
macro_rules! raw_enum {
    ($(#[$meta:meta])* $name:ident { $($(#[$variant_meta:meta])* $variant:ident = $value:literal,)* }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        #[repr(u8)]
        pub enum $name {
            $($(#[$variant_meta])* $variant = $value,)*
        }

        impl $name {
            const fn from_raw(raw: u32) -> Option<Self> {
                match raw {
                    $($value => Some(Self::$variant),)*
                    _ => None,
                }
            }
        }
    };
}

raw_enum!(
    /// The kind of a portable radio request.
    RequestKind {
        ConfigureAdvertising = 0,
        Advertise = 1,
        RemoveAdvertising = 2,
        ConfigureScanner = 3,
        Scan = 4,
        RemoveScanner = 5,
        OpenConnection = 6,
        ConnectionEvent = 7,
        Transmit = 8,
        CloseConnection = 9,
        TestTransmit = 10,
        TestReceive = 11,
        EndTest = 12,
        FilterAcceptList = 13,
    }
);

raw_enum!(
    /// The radio role's answer to a request: accepted, the refusal, or the
    /// radio's fault.
    RequestAnswer {
        Accepted = 0,
        ClockUnavailable = 1,
        EpochExhausted = 2,
        NoInstance = 3,
        Unknown = 4,
        AlreadyConfigured = 5,
        Busy = 6,
        Late = 7,
        ListFull = 8,
        NotListed = 9,
        /// The radio's own structures disagreed: it faulted.
        Faulted = 10,
    }
);

/// The radio role answered one request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RoleRequest {
    pub kind: RequestKind,
    pub answer: RequestAnswer,
    /// Microseconds from the radio time to the start of the request's first
    /// air window; `None` for a request without a window, or one that starts
    /// before the radio time or 2^32 us after it.
    pub lead_micros: Option<u32>,
}

impl Event for RoleRequest {
    const KIND: Kind = kind(1);
    const CHANNEL: Channel = channel(0);

    fn encode(&self) -> [u32; 2] {
        [
            self.kind as u32
                | (self.answer as u32) << 8
                | u32::from(self.lead_micros.is_some()) << 16,
            self.lead_micros.unwrap_or(0),
        ]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        if words[0] >> 17 != 0 {
            return None;
        }
        Some(Self {
            kind: RequestKind::from_raw(words[0] & 0xff)?,
            answer: RequestAnswer::from_raw((words[0] >> 8) & 0xff)?,
            lead_micros: (words[0] & 1 << 16 != 0).then_some(words[1]),
        })
    }
}

impl fmt::Display for RoleRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "request {:?} {:?}", self.kind, self.answer)?;
        if let Some(lead) = self.lead_micros {
            write!(f, " lead={lead}us")?;
        }
        Ok(())
    }
}

raw_enum!(
    /// How a scheduled event ended.
    EndResult {
        /// It ran, with no captured anchor.
        Executed = 0,
        /// It ran, and a packet fixed its anchor.
        ExecutedWithAnchor = 1,
        NotExecuted = 2,
        /// Withdrawn after publication without a completion status.
        Aborted = 3,
    }
);

/// The radio role ended one event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RoleEventEnd {
    pub id: u32,
    pub result: EndResult,
    /// PDUs the event reported, at most 255 counted.
    pub received: u8,
    /// PDUs its receive chain held without a valid reception.
    pub discarded: u8,
}

impl Event for RoleEventEnd {
    const KIND: Kind = kind(2);
    const CHANNEL: Channel = channel(1);

    fn encode(&self) -> [u32; 2] {
        [
            self.id,
            self.result as u32 | u32::from(self.received) << 8 | u32::from(self.discarded) << 16,
        ]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        if words[1] >> 24 != 0 {
            return None;
        }
        Some(Self {
            id: words[0],
            result: EndResult::from_raw(words[1] & 0xff)?,
            received: (words[1] >> 8) as u8,
            discarded: (words[1] >> 16) as u8,
        })
    }
}

impl fmt::Display for RoleEventEnd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "event {} ended {:?} received={} discarded={}",
            self.id, self.result, self.received, self.discarded
        )
    }
}

/// The radio role reported one advertising-channel PDU: an advertising or
/// scan event's reception.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RolePdu {
    pub id: u32,
    /// The PDU header's first octet: its type and address flags.
    pub header: u8,
    /// The payload length from the header.
    pub length: u8,
    pub rssi_dbm: i8,
}

impl Event for RolePdu {
    const KIND: Kind = kind(3);
    const CHANNEL: Channel = channel(2);

    fn encode(&self) -> [u32; 2] {
        [
            self.id,
            u32::from(self.header)
                | u32::from(self.length) << 8
                | u32::from(self.rssi_dbm as u8) << 16,
        ]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        if words[1] >> 24 != 0 {
            return None;
        }
        Some(Self {
            id: words[0],
            header: words[1] as u8,
            length: (words[1] >> 8) as u8,
            rssi_dbm: (words[1] >> 16) as u8 as i8,
        })
    }
}

impl fmt::Display for RolePdu {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "pdu event={} type={:#x} length={} rssi={}dBm",
            self.id,
            self.header & 0x0f,
            self.length,
            self.rssi_dbm
        )
    }
}

raw_enum!(
    /// A step of the Controller's advertising set.
    AdvertiserStepKind {
        /// The set was configured in the radio.
        Configured = 0,
        /// An event was planned and requested.
        EventPlanned = 1,
        /// An event the arbiter could not place was skipped.
        EventSkipped = 2,
        /// An event ended.
        EventEnded = 3,
        /// The set left the radio.
        Removed = 4,
    }
);

/// The Controller's advertising set took one step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdvertiserStep {
    pub step: AdvertiserStepKind,
    /// The event, zero for a step of the set.
    pub id: u32,
}

impl Event for AdvertiserStep {
    const KIND: Kind = kind(4);
    const CHANNEL: Channel = channel(3);

    fn encode(&self) -> [u32; 2] {
        [self.step as u32, self.id]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        Some(Self {
            step: AdvertiserStepKind::from_raw(words[0])?,
            id: words[1],
        })
    }
}

impl fmt::Display for AdvertiserStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "advertiser {:?} event={}", self.step, self.id)
    }
}

raw_enum!(
    /// What the Controller decided about a PDU received in a connectable
    /// advertising event, read as a connection indication.
    IndicationVerdict {
        /// It opens a connection.
        Accepted = 0,
        /// The set no longer runs.
        NotRunning = 1,
        /// The hardware captured no packet start.
        NoCapture = 2,
        /// A `CONNECT_IND` that fails validation.
        Malformed = 3,
        /// The set admits no connection from this initiator.
        NotAdmitted = 4,
        /// It names another advertiser.
        NotAddressed = 5,
        /// It is another PDU type, such as a `SCAN_REQ`.
        OtherPdu = 6,
    }
);

/// The Controller judged one connection indication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectionIndication {
    pub verdict: IndicationVerdict,
}

impl Event for ConnectionIndication {
    const KIND: Kind = kind(5);
    const CHANNEL: Channel = channel(4);

    fn encode(&self) -> [u32; 2] {
        [self.verdict as u32, 0]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        if words[1] != 0 {
            return None;
        }
        Some(Self {
            verdict: IndicationVerdict::from_raw(words[0])?,
        })
    }
}

impl fmt::Display for ConnectionIndication {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "connection indication {:?}", self.verdict)
    }
}

/// The Controller opened the peripheral connection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeripheralOpened {
    /// The low word of the first anchor, in radio microseconds.
    pub first_anchor_low: u32,
    /// The connection interval in 1.25 ms units.
    pub interval_units: u16,
}

impl Event for PeripheralOpened {
    const KIND: Kind = kind(6);
    const CHANNEL: Channel = channel(5);

    fn encode(&self) -> [u32; 2] {
        [self.first_anchor_low, u32::from(self.interval_units)]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        Some(Self {
            first_anchor_low: words[0],
            interval_units: u16::try_from(words[1]).ok()?,
        })
    }
}

impl fmt::Display for PeripheralOpened {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "connection opened anchor_low={} interval={}x1.25ms",
            self.first_anchor_low, self.interval_units
        )
    }
}

raw_enum!(
    /// A step of one connection event.
    PeripheralEventOp {
        /// The Controller requested it.
        Requested = 0,
        /// The radio admitted it.
        Accepted = 1,
        /// The radio refused it: a missed event.
        Refused = 2,
        /// It ended with peer activity.
        Observed = 3,
        /// It ended without peer activity.
        Missed = 4,
    }
);

/// One step of a connection event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeripheralEvent {
    pub op: PeripheralEventOp,
    /// The Link Layer connection event counter.
    pub counter: u16,
    pub id: u32,
}

impl Event for PeripheralEvent {
    const KIND: Kind = kind(7);
    const CHANNEL: Channel = channel(6);

    fn encode(&self) -> [u32; 2] {
        [self.op as u32 | u32::from(self.counter) << 8, self.id]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        if words[0] >> 24 != 0 {
            return None;
        }
        Some(Self {
            op: PeripheralEventOp::from_raw(words[0] & 0xff)?,
            counter: (words[0] >> 8) as u16,
            id: words[1],
        })
    }
}

impl fmt::Display for PeripheralEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "connection event {:?} counter={} event={}",
            self.op, self.counter, self.id
        )
    }
}

/// The Controller closed the peripheral connection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeripheralClosed {
    /// The HCI status reported to the Host; `None` after a Reset, which
    /// reports none.
    pub reason: Option<u8>,
}

impl Event for PeripheralClosed {
    const KIND: Kind = kind(8);
    const CHANNEL: Channel = channel(7);

    fn encode(&self) -> [u32; 2] {
        match self.reason {
            Some(reason) => [1, u32::from(reason)],
            None => [0, 0],
        }
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        match words {
            [0, 0] => Some(Self { reason: None }),
            [1, reason] => Some(Self {
                reason: Some(u8::try_from(reason).ok()?),
            }),
            _ => None,
        }
    }
}

impl fmt::Display for PeripheralClosed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.reason {
            Some(reason) => write!(f, "connection closed reason={reason:#04x}"),
            None => write!(f, "connection closed by reset"),
        }
    }
}

raw_enum!(
    /// What the runtime's outcome queue did with a report.
    QueueOp {
        /// A full queue dropped a report of event `id`.
        ReportDropped = 0,
        /// The consumer took the loss in place of the dropped reports.
        LossReported = 1,
    }
);

/// The runtime's outcome queue lost a report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QueueLoss {
    pub op: QueueOp,
    /// The event of a dropped report; zero for a reported loss.
    pub id: u32,
}

impl Event for QueueLoss {
    const KIND: Kind = kind(9);
    const CHANNEL: Channel = channel(8);

    fn encode(&self) -> [u32; 2] {
        [self.op as u32, self.id]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        Some(Self {
            op: QueueOp::from_raw(words[0])?,
            id: words[1],
        })
    }
}

impl fmt::Display for QueueLoss {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "queue {:?} event={}", self.op, self.id)
    }
}

raw_enum!(
    /// A transition of the port's owner.
    LifecycleCommand {
        Enable = 0,
        Disable = 1,
    }
);

raw_enum!(
    /// How far a transition got. A transition's duration is the distance
    /// between its `Started` record and the record that ended it.
    LifecycleStep {
        /// The transition started.
        Started = 0,
        /// The transition completed.
        Done = 1,
        /// The transition found or left the runtime poisoned.
        Poisoned = 2,
        /// The transition did not end within its bound: it poisoned the
        /// runtime.
        TimedOut = 3,
    }
);

/// The runtime ran one lifecycle step of its port.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Lifecycle {
    pub command: LifecycleCommand,
    pub step: LifecycleStep,
}

impl Event for Lifecycle {
    const KIND: Kind = kind(10);
    const CHANNEL: Channel = channel(9);

    fn encode(&self) -> [u32; 2] {
        [self.command as u32, self.step as u32]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        Some(Self {
            command: LifecycleCommand::from_raw(words[0])?,
            step: LifecycleStep::from_raw(words[1])?,
        })
    }
}

impl fmt::Display for Lifecycle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "lifecycle {:?} {:?}", self.command, self.step)
    }
}

oer_trace::event_set!(
    pub BluetoothTrace: RoleRequest,
    RoleEventEnd,
    RolePdu,
    AdvertiserStep,
    ConnectionIndication,
    PeripheralOpened,
    PeripheralEvent,
    PeripheralClosed,
    QueueLoss,
    Lifecycle,
);

#[cfg(test)]
mod tests;
