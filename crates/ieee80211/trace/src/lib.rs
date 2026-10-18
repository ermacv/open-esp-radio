#![no_std]
#![forbid(unsafe_code)]

//! Trace points of the connected station and its RX slot pool.
//!
//! Every event the station records into [`oer_trace`] is defined here with
//! its channel, so the host decodes a drained record with the same type. The
//! channels are the ieee80211 domain's indices; one channel gates one event,
//! so the host can leave the high-rate network TX reports off while it
//! records beacons and control.
//!
//! | index | event                   | rate under saturated traffic |
//! |-------|-------------------------|------------------------------|
//! | 0     | [`BeaconDispatch`]      | one per received beacon      |
//! | 1     | [`ControlMailbox`]      | one per control event        |
//! | 2     | [`PowerInputTrace`]     | TBTT, timers and coexistence |
//! | 3     | [`NetworkTxPowerTrace`] | one per network transaction  |
//! | 4     | [`BeaconMonitorTrace`]  | one per beacon or probe      |
//! | 5     | [`ControlExit`]         | one per connected epoch      |
//! | 6     | [`RxSlotTrace`]         | two per received data frame  |
//! | 7     | [`LinkControlTrace`]    | association, keys, Block Ack |
//! | 8     | [`PowerStateTrace`]     | one per power-state change   |

use core::fmt;

use oer_trace::{Channel, Domain, Event, Kind};

const fn kind(event: u8) -> Kind {
    Kind::new(Domain::Ieee80211, event)
}

const fn channel(index: u8) -> Channel {
    Channel::new(Domain::Ieee80211, index)
}

/// What RX dispatch did with a received beacon.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum BeaconVerdict {
    /// Published to control as the associated BSS's beacon.
    Published = 0,
    /// Its management body could not be copied out of the RX segment.
    Unextracted = 1,
    /// Not the associated BSS's beacon, or malformed.
    Rejected = 2,
    /// Its RX metadata was unavailable.
    NoMetadata = 3,
}

impl BeaconVerdict {
    const fn from_raw(raw: u32) -> Option<Self> {
        Some(match raw {
            0 => Self::Published,
            1 => Self::Unextracted,
            2 => Self::Rejected,
            3 => Self::NoMetadata,
            _ => return None,
        })
    }
}

/// One beacon RX dispatch classified.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BeaconDispatch {
    pub verdict: BeaconVerdict,
    /// The beacon's RSSI, when its metadata carried one.
    pub rssi_dbm: Option<i8>,
    /// The low word of the access point TSF in a published beacon.
    pub timestamp_tsf_low: u32,
}

impl Event for BeaconDispatch {
    const KIND: Kind = kind(1);
    const CHANNEL: Channel = channel(0);

    fn encode(&self) -> [u32; 2] {
        let rssi = match self.rssi_dbm {
            Some(rssi) => 0x100 | u32::from(rssi as u8),
            None => 0,
        };
        [(self.verdict as u32) | (rssi << 8), self.timestamp_tsf_low]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        if words[0] >> 17 != 0 {
            return None;
        }
        let rssi = (words[0] >> 8) & 0x1ff;
        Some(Self {
            verdict: BeaconVerdict::from_raw(words[0] & 0xff)?,
            rssi_dbm: (rssi & 0x100 != 0).then_some(rssi as u8 as i8),
            timestamp_tsf_low: words[1],
        })
    }
}

impl fmt::Display for BeaconDispatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "beacon {:?}", self.verdict)?;
        if let Some(rssi) = self.rssi_dbm {
            write!(f, " rssi={rssi}dBm")?;
        }
        write!(f, " tsf_low={}", self.timestamp_tsf_low)
    }
}

/// The kind of a control event, without its payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ControlEventKind {
    Beacon = 0,
    ProbeResponse = 1,
    Trigger = 2,
    Ndpa = 3,
    BlockAck = 4,
    IndividualTwt = 5,
    PeerDisconnect = 6,
    UnprotectedDisconnect = 7,
    SaQuery = 8,
    PowerSaveData = 9,
}

impl ControlEventKind {
    const fn from_raw(raw: u32) -> Option<Self> {
        Some(match raw {
            0 => Self::Beacon,
            1 => Self::ProbeResponse,
            2 => Self::Trigger,
            3 => Self::Ndpa,
            4 => Self::BlockAck,
            5 => Self::IndividualTwt,
            6 => Self::PeerDisconnect,
            7 => Self::UnprotectedDisconnect,
            8 => Self::SaQuery,
            9 => Self::PowerSaveData,
            _ => return None,
        })
    }
}

/// What happened to a control event between RX and the control core.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum MailboxOp {
    /// RX published it to the control mailbox.
    Published = 0,
    /// A full mailbox refused it.
    Overflowed = 1,
    /// The control core applied it.
    Applied = 2,
}

impl MailboxOp {
    const fn from_raw(raw: u32) -> Option<Self> {
        Some(match raw {
            0 => Self::Published,
            1 => Self::Overflowed,
            2 => Self::Applied,
            _ => return None,
        })
    }
}

/// A control event crossing the mailbox.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlMailbox {
    pub event: ControlEventKind,
    pub op: MailboxOp,
}

impl Event for ControlMailbox {
    const KIND: Kind = kind(2);
    const CHANNEL: Channel = channel(1);

    fn encode(&self) -> [u32; 2] {
        [self.event as u32, self.op as u32]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        Some(Self {
            event: ControlEventKind::from_raw(words[0])?,
            op: MailboxOp::from_raw(words[1])?,
        })
    }
}

impl fmt::Display for ControlMailbox {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "control {:?} {:?}", self.event, self.op)
    }
}

/// A power input other than network TX.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PowerInputKind {
    Start = 0,
    Null = 1,
    Tbtt = 2,
    CoexPhase = 3,
    Preemption = 4,
    Timer = 5,
}

impl PowerInputKind {
    const fn from_raw(raw: u32) -> Option<Self> {
        Some(match raw {
            0 => Self::Start,
            1 => Self::Null,
            2 => Self::Tbtt,
            3 => Self::CoexPhase,
            4 => Self::Preemption,
            5 => Self::Timer,
            _ => return None,
        })
    }
}

/// The control core performed a power input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PowerInputTrace {
    pub input: PowerInputKind,
    /// Whether a control event waited, and so takes the next step.
    pub control_event_waiting: bool,
}

impl Event for PowerInputTrace {
    const KIND: Kind = kind(3);
    const CHANNEL: Channel = channel(2);

    fn encode(&self) -> [u32; 2] {
        [self.input as u32, u32::from(self.control_event_waiting)]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        Some(Self {
            input: PowerInputKind::from_raw(words[0])?,
            control_event_waiting: match words[1] {
                0 => false,
                1 => true,
                _ => return None,
            },
        })
    }
}

impl fmt::Display for PowerInputTrace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "power {:?}", self.input)?;
        if self.control_event_waiting {
            f.write_str(" (event waits)")?;
        }
        Ok(())
    }
}

/// The control core gave power management a network TX input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkTxPowerTrace {
    /// A queued frame was offered before it left, rather than reported
    /// after it started.
    pub offer: bool,
    pub control_event_waiting: bool,
}

impl Event for NetworkTxPowerTrace {
    const KIND: Kind = kind(4);
    const CHANNEL: Channel = channel(3);

    fn encode(&self) -> [u32; 2] {
        [u32::from(self.offer), u32::from(self.control_event_waiting)]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        let flag = |word: u32| match word {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        };
        Some(Self {
            offer: flag(words[0])?,
            control_event_waiting: flag(words[1])?,
        })
    }
}

impl fmt::Display for NetworkTxPowerTrace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(if self.offer {
            "power network offer"
        } else {
            "power network report"
        })?;
        if self.control_event_waiting {
            f.write_str(" (event waits)")?;
        }
        Ok(())
    }
}

/// A beacon monitor transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum BeaconMonitorOp {
    /// A beacon refreshed the deadline.
    Refreshed = 0,
    /// The deadline passed and an active probe starts.
    ProbeStarted = 1,
    /// The access point answered a probe.
    ProbeAnswered = 2,
    /// The probes went unanswered; the link is lost.
    Lost = 3,
}

impl BeaconMonitorOp {
    const fn from_raw(raw: u32) -> Option<Self> {
        Some(match raw {
            0 => Self::Refreshed,
            1 => Self::ProbeStarted,
            2 => Self::ProbeAnswered,
            3 => Self::Lost,
            _ => return None,
        })
    }
}

/// The beacon monitor changed state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BeaconMonitorTrace {
    pub op: BeaconMonitorOp,
    /// The low word of the monitor's deadline afterwards, in microseconds.
    pub deadline_micros_low: u32,
}

impl Event for BeaconMonitorTrace {
    const KIND: Kind = kind(5);
    const CHANNEL: Channel = channel(4);

    fn encode(&self) -> [u32; 2] {
        [self.op as u32, self.deadline_micros_low]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        Some(Self {
            op: BeaconMonitorOp::from_raw(words[0])?,
            deadline_micros_low: words[1],
        })
    }
}

impl fmt::Display for BeaconMonitorTrace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "beacon monitor {:?} deadline_low={}us",
            self.op, self.deadline_micros_low
        )
    }
}

/// Why a connected epoch ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExitReason {
    BeaconLoss,
    PeerDeauthentication { reason_code: u16 },
    PeerDisassociation { reason_code: u16 },
    ControlMailboxOverflow,
    ActiveStateRestoreFailed,
    GroupKeyHandshakeFailed,
    SaQueryTimeout,
    JoinTsfUnrepresentable,
}

/// A connected epoch ended; recording freezes a window after it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlExit {
    pub reason: ExitReason,
}

impl ControlExit {
    /// Entries still recorded after the exit: the teardown that follows.
    pub const POST_TRIGGER_ENTRIES: u32 = 64;
}

impl Event for ControlExit {
    const KIND: Kind = kind(6);
    const CHANNEL: Channel = channel(5);

    fn encode(&self) -> [u32; 2] {
        use ExitReason as Reason;
        match self.reason {
            Reason::BeaconLoss => [0, 0],
            Reason::PeerDeauthentication { reason_code } => [1, u32::from(reason_code)],
            Reason::PeerDisassociation { reason_code } => [2, u32::from(reason_code)],
            Reason::ControlMailboxOverflow => [3, 0],
            Reason::ActiveStateRestoreFailed => [4, 0],
            Reason::GroupKeyHandshakeFailed => [5, 0],
            Reason::SaQueryTimeout => [6, 0],
            Reason::JoinTsfUnrepresentable => [7, 0],
        }
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        use ExitReason as Reason;
        let reason_code = u16::try_from(words[1]).ok()?;
        let reason = match (words[0], reason_code) {
            (0, 0) => Reason::BeaconLoss,
            (1, reason_code) => Reason::PeerDeauthentication { reason_code },
            (2, reason_code) => Reason::PeerDisassociation { reason_code },
            (3, 0) => Reason::ControlMailboxOverflow,
            (4, 0) => Reason::ActiveStateRestoreFailed,
            (5, 0) => Reason::GroupKeyHandshakeFailed,
            (6, 0) => Reason::SaQueryTimeout,
            (7, 0) => Reason::JoinTsfUnrepresentable,
            _ => return None,
        };
        Some(Self { reason })
    }
}

impl fmt::Display for ControlExit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "connected exit {:?}", self.reason)
    }
}

/// Why a completed RX DMA unit never reached an upper slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum RxSlotDiscard {
    /// The unit carried no bytes.
    Empty = 0,
    /// The unit exceeded the slot capacity.
    TooLong = 1,
    /// The unit spanned more than one descriptor.
    Chained = 2,
    /// Every upper slot was already claimed.
    Exhausted = 3,
}

impl RxSlotDiscard {
    const fn from_raw(raw: u32) -> Option<Self> {
        Some(match raw {
            0 => Self::Empty,
            1 => Self::TooLong,
            2 => Self::Chained,
            3 => Self::Exhausted,
            _ => return None,
        })
    }
}

/// One step of an RX DMA buffer through the upper slot pool.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxSlotOp {
    /// The buffer moved into the slot without a copy.
    Claimed { slot: u8 },
    /// The slot was republished as an Ethernet frame to the network.
    Published { slot: u8 },
    /// The unit stayed in the ring and returns to DMA.
    Discarded(RxSlotDiscard),
}

/// An RX slot transition; `length` is the MPDU length when claimed or
/// discarded and the Ethernet length when published.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxSlotTrace {
    pub op: RxSlotOp,
    pub length: u16,
}

impl Event for RxSlotTrace {
    const KIND: Kind = kind(7);
    const CHANNEL: Channel = channel(6);

    fn encode(&self) -> [u32; 2] {
        let op = match self.op {
            RxSlotOp::Claimed { slot } => u32::from(slot) << 8,
            RxSlotOp::Published { slot } => 1 | (u32::from(slot) << 8),
            RxSlotOp::Discarded(reason) => 2 | ((reason as u32) << 8),
        };
        [op, u32::from(self.length)]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        if words[0] >> 16 != 0 {
            return None;
        }
        let argument = (words[0] >> 8) as u8;
        let op = match words[0] & 0xff {
            0 => RxSlotOp::Claimed { slot: argument },
            1 => RxSlotOp::Published { slot: argument },
            2 => RxSlotOp::Discarded(RxSlotDiscard::from_raw(u32::from(argument))?),
            _ => return None,
        };
        Some(Self {
            op,
            length: u16::try_from(words[1]).ok()?,
        })
    }
}

impl fmt::Display for RxSlotTrace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.op {
            RxSlotOp::Claimed { slot } => write!(f, "rx slot {slot} claimed")?,
            RxSlotOp::Published { slot } => write!(f, "rx slot {slot} published")?,
            RxSlotOp::Discarded(reason) => write!(f, "rx unit discarded {reason:?}")?,
        }
        write!(f, " len={}", self.length)
    }
}

/// Direction of a Block Ack agreement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum BlockAckDirection {
    /// The station originates the aggregates.
    Tx = 0,
    /// The access point originates them; the station reorders.
    Rx = 1,
}

/// A link-state change of the station's association.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkEvent {
    /// The access point accepted the association.
    Associated { association_id: u16 },
    /// The pairwise and group keys of a handshake were installed.
    KeysInstalled { group_key_id: u8 },
    /// A group key handshake installed a new group key.
    GroupKeyRotated { key_id: u8 },
    /// A Block Ack agreement became operational.
    BlockAckOperational {
        direction: BlockAckDirection,
        tid: u8,
        window: u16,
    },
    /// The peer refused the station's ADDBA request.
    BlockAckRejected { tid: u8, status: u16 },
    /// A DELBA ended an agreement.
    BlockAckEnded {
        direction: BlockAckDirection,
        tid: u8,
    },
}

/// One link-state change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LinkControlTrace {
    pub event: LinkEvent,
}

impl Event for LinkControlTrace {
    const KIND: Kind = kind(8);
    const CHANNEL: Channel = channel(7);

    fn encode(&self) -> [u32; 2] {
        let field =
            |code: u32, direction: u32, tid: u8| code | (direction << 8) | (u32::from(tid) << 16);
        match self.event {
            LinkEvent::Associated { association_id } => [0, u32::from(association_id)],
            LinkEvent::KeysInstalled { group_key_id } => [1, u32::from(group_key_id)],
            LinkEvent::GroupKeyRotated { key_id } => [2, u32::from(key_id)],
            LinkEvent::BlockAckOperational {
                direction,
                tid,
                window,
            } => [field(3, direction as u32, tid), u32::from(window)],
            LinkEvent::BlockAckRejected { tid, status } => [field(4, 0, tid), u32::from(status)],
            LinkEvent::BlockAckEnded { direction, tid } => [field(5, direction as u32, tid), 0],
        }
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        if words[0] >> 24 != 0 {
            return None;
        }
        let code = words[0] & 0xff;
        let direction = match (words[0] >> 8) & 0xff {
            0 => BlockAckDirection::Tx,
            1 => BlockAckDirection::Rx,
            _ => return None,
        };
        let tid = (words[0] >> 16) as u8;
        let plain = words[0] >> 8 == 0;
        let word = u16::try_from(words[1]).ok()?;
        let key = u8::try_from(words[1]).ok();
        let event = match code {
            0 if plain => LinkEvent::Associated {
                association_id: word,
            },
            1 if plain => LinkEvent::KeysInstalled { group_key_id: key? },
            2 if plain => LinkEvent::GroupKeyRotated { key_id: key? },
            3 => LinkEvent::BlockAckOperational {
                direction,
                tid,
                window: word,
            },
            4 if direction == BlockAckDirection::Tx => {
                LinkEvent::BlockAckRejected { tid, status: word }
            }
            5 if words[1] == 0 => LinkEvent::BlockAckEnded { direction, tid },
            _ => return None,
        };
        Some(Self { event })
    }
}

impl fmt::Display for LinkControlTrace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.event {
            LinkEvent::Associated { association_id } => {
                write!(f, "link associated aid={association_id}")
            }
            LinkEvent::KeysInstalled { group_key_id } => {
                write!(f, "link keys installed group_key_id={group_key_id}")
            }
            LinkEvent::GroupKeyRotated { key_id } => {
                write!(f, "link group key rotated key_id={key_id}")
            }
            LinkEvent::BlockAckOperational {
                direction,
                tid,
                window,
            } => write!(
                f,
                "link {direction:?} block ack tid={tid} operational window={window}"
            ),
            LinkEvent::BlockAckRejected { tid, status } => {
                write!(f, "link Tx block ack tid={tid} rejected status={status}")
            }
            LinkEvent::BlockAckEnded { direction, tid } => {
                write!(f, "link {direction:?} block ack tid={tid} ended")
            }
        }
    }
}

/// Power-management state of the station.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PowerState {
    /// Awake with power save not advertised.
    Awake = 0,
    /// Power save advertised; the modem is still awake.
    PowerSave = 1,
    /// Power save advertised and the RF asleep.
    Dozing = 2,
}

impl PowerState {
    const fn from_raw(raw: u32) -> Option<Self> {
        Some(match raw {
            0 => Self::Awake,
            1 => Self::PowerSave,
            2 => Self::Dozing,
            _ => return None,
        })
    }
}

/// The power manager changed state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PowerStateTrace {
    pub from: PowerState,
    pub to: PowerState,
}

impl Event for PowerStateTrace {
    const KIND: Kind = kind(9);
    const CHANNEL: Channel = channel(8);

    fn encode(&self) -> [u32; 2] {
        [self.from as u32, self.to as u32]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        Some(Self {
            from: PowerState::from_raw(words[0])?,
            to: PowerState::from_raw(words[1])?,
        })
    }
}

impl fmt::Display for PowerStateTrace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "power state {:?} -> {:?}", self.from, self.to)
    }
}

/// The station's RF went to sleep: the raw MAC local-time counter and the
/// low 32 bits of monotonic microseconds, read back to back just before.
///
/// With [`RfWoke`], a host tells whether the MAC counter runs through RF
/// sleep: across one sleep, the counter's distance equals the monotonic
/// distance when it runs, is zero when it holds still.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RfSleepEntered {
    pub mac_local_time: u32,
    pub monotonic_micros: u32,
}

/// The station's RF woke: the same pair read just after the wake.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RfWoke {
    pub mac_local_time: u32,
    pub monotonic_micros: u32,
}

impl Event for RfSleepEntered {
    const KIND: Kind = kind(10);
    const CHANNEL: Channel = channel(9);

    fn encode(&self) -> [u32; 2] {
        [self.mac_local_time, self.monotonic_micros]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        Some(Self {
            mac_local_time: words[0],
            monotonic_micros: words[1],
        })
    }
}

impl fmt::Display for RfSleepEntered {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "rf sleep mac={} monotonic={}",
            self.mac_local_time, self.monotonic_micros
        )
    }
}

impl Event for RfWoke {
    const KIND: Kind = kind(11);
    const CHANNEL: Channel = channel(9);

    fn encode(&self) -> [u32; 2] {
        [self.mac_local_time, self.monotonic_micros]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        Some(Self {
            mac_local_time: words[0],
            monotonic_micros: words[1],
        })
    }
}

impl fmt::Display for RfWoke {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "rf wake mac={} monotonic={}",
            self.mac_local_time, self.monotonic_micros
        )
    }
}

oer_trace::event_set!(
    pub StationTrace: BeaconDispatch,
    ControlMailbox,
    PowerInputTrace,
    NetworkTxPowerTrace,
    BeaconMonitorTrace,
    ControlExit,
    RxSlotTrace,
    LinkControlTrace,
    PowerStateTrace,
    RfSleepEntered,
    RfWoke,
);

#[cfg(test)]
mod tests;
