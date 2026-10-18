#![no_std]
#![forbid(unsafe_code)]

//! Trace points and the post-mortem snapshot of the shared PHY domain.
//!
//! Every event the PHY records into [`oer_base_trace`] is defined here with its
//! channel, so the host decodes a drained record with the same type. The
//! types are chip-neutral: a chip's PHY maps its own errors onto [`Refusal`]
//! and [`Fault`] before it emits. One channel gates one event, so the host
//! can leave the 1 Hz tracking tick off while it records lifecycle edges.
//!
//! | index | event                     | rate                                   |
//! |-------|---------------------------|----------------------------------------|
//! | 0     | [`Registration`]          | one start and one end per cold boot    |
//! | 1     | [`TrackingTick`]          | one per vendor tracking period (1 s)   |
//! | 2     | [`RfLifecycle`]           | one per last-client close or wake      |
//! | 3     | [`ClientChange`]          | one per protocol start or stop         |
//! | 4     | [`TemperatureReferences`] | one per tracking commit                |
//! | 5     | [`Poison`]                | at most one per registration           |
//! | 6     | [`WifiChannel`]           | one per Wi-Fi channel selection        |
//!
//! [`record_poison`] emits [`Poison`], freezes the trace
//! [`Poison::POST_TRIGGER_ENTRIES`] entries later and captures a
//! [`PhySnapshot`], which survives the fail-stop reset that follows.

use core::fmt;

use oer_base_trace::{Channel, Domain, Event, Kind};

const fn kind(event: u8) -> Kind {
    Kind::new(Domain::Phy, event)
}

const fn channel(index: u8) -> Channel {
    Channel::new(Domain::Phy, index)
}

const fn flag(word: u32) -> Option<bool> {
    match word {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    }
}

/// A protocol client of the shared PHY domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Client {
    Wifi = 0,
    Bluetooth = 1,
    Ieee802154 = 2,
}

impl Client {
    const fn from_raw(raw: u32) -> Option<Self> {
        Some(match raw {
            0 => Self::Wifi,
            1 => Self::Bluetooth,
            2 => Self::Ieee802154,
            _ => return None,
        })
    }

    const fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// The set of active clients.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Clients(u8);

impl Clients {
    pub const NONE: Self = Self(0);

    pub const fn with(self, client: Client) -> Self {
        Self(self.0 | client.bit())
    }

    pub const fn contains(self, client: Client) -> bool {
        self.0 & client.bit() != 0
    }

    const fn raw(self) -> u32 {
        self.0 as u32
    }

    const fn from_raw(raw: u32) -> Option<Self> {
        if raw & !0b111 != 0 {
            return None;
        }
        Some(Self(raw as u8))
    }
}

impl fmt::Display for Clients {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for (client, name) in [
            (Client::Wifi, "wifi"),
            (Client::Bluetooth, "bluetooth"),
            (Client::Ieee802154, "ieee802154"),
        ] {
            if self.contains(client) {
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

/// Why the domain refused an operation before any register access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Refusal {
    NotRegistered,
    AlreadyRegistered,
    TrackingPending,
    NoTrackingPending,
    Poisoned,
    RfClosed,
    RfOpen,
    EpochMismatch,
    ClientsActive,
    /// A modem clock module could not change.
    Clock,
    /// The client set rejected an acquisition.
    Acquire,
    /// The client set rejected a release.
    Release,
    /// The tracking clock was rejected.
    Time,
    ClientAbsent(Client),
}

impl Refusal {
    const fn raw(self) -> u32 {
        match self {
            Self::NotRegistered => 0,
            Self::AlreadyRegistered => 1,
            Self::TrackingPending => 2,
            Self::NoTrackingPending => 3,
            Self::Poisoned => 4,
            Self::RfClosed => 5,
            Self::RfOpen => 6,
            Self::EpochMismatch => 7,
            Self::ClientsActive => 8,
            Self::Clock => 9,
            Self::Acquire => 10,
            Self::Release => 11,
            Self::Time => 12,
            // 13, 15 and 16 are retired; never reuse them.
            Self::ClientAbsent(client) => 14 | (client as u32) << 8,
        }
    }

    const fn from_raw(raw: u32) -> Option<Self> {
        let client = raw >> 8;
        Some(match (raw & 0xff, client) {
            (0, 0) => Self::NotRegistered,
            (1, 0) => Self::AlreadyRegistered,
            (2, 0) => Self::TrackingPending,
            (3, 0) => Self::NoTrackingPending,
            (4, 0) => Self::Poisoned,
            (5, 0) => Self::RfClosed,
            (6, 0) => Self::RfOpen,
            (7, 0) => Self::EpochMismatch,
            (8, 0) => Self::ClientsActive,
            (9, 0) => Self::Clock,
            (10, 0) => Self::Acquire,
            (11, 0) => Self::Release,
            (12, 0) => Self::Time,
            (14, client) => match Client::from_raw(client) {
                Some(client) => Self::ClientAbsent(client),
                None => return None,
            },
            _ => return None,
        })
    }
}

/// Where a started hardware operation failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum FaultStage {
    /// A transition rejected a completion or ran out of steps.
    Transition = 0,
    /// A bus or hardware wait failed.
    Hardware = 1,
    // 2 is retired; never reuse it.
    /// The borrow described another registration.
    EpochMismatch = 3,
    /// The completed transition did not return its owner.
    MissingOwner = 4,
    /// A modem clock module did not follow the RF state.
    Clock = 5,
}

impl FaultStage {
    const fn from_raw(raw: u32) -> Option<Self> {
        Some(match raw {
            0 => Self::Transition,
            1 => Self::Hardware,
            3 => Self::EpochMismatch,
            4 => Self::MissingOwner,
            5 => Self::Clock,
            _ => return None,
        })
    }
}

/// A started operation that failed. `detail` is the chip PHY's own
/// discriminant of the failing step, for comparing failures of one image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Fault {
    pub stage: FaultStage,
    pub detail: u16,
}

impl Fault {
    const fn raw(self) -> u32 {
        (self.stage as u32) << 16 | self.detail as u32
    }

    const fn from_raw(raw: u32) -> Option<Self> {
        match FaultStage::from_raw(raw >> 16) {
            Some(stage) => Some(Self {
                stage,
                detail: raw as u16,
            }),
            None => None,
        }
    }
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}#{}", self.stage, self.detail)
    }
}

/// How the cold registration calibrated.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum CalibrationPath {
    /// Full calibration; no persistence was requested.
    FullUncached = 0,
    /// Full calibration producing a fresh cache.
    FullForCache = 1,
    /// A supplied cache was rejected and full calibration replaced it.
    FullAfterRejectedCache = 2,
    /// A valid cache seeded the calibration state.
    PartialFromCache = 3,
}

impl CalibrationPath {
    const fn from_raw(raw: u32) -> Option<Self> {
        Some(match raw {
            0 => Self::FullUncached,
            1 => Self::FullForCache,
            2 => Self::FullAfterRejectedCache,
            3 => Self::PartialFromCache,
            _ => return None,
        })
    }
}

/// A cold registration edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Registration {
    Started,
    Calibrated(CalibrationPath),
    Refused(Refusal),
    Failed(Fault),
}

impl Event for Registration {
    const KIND: Kind = kind(1);
    const CHANNEL: Channel = channel(0);

    fn encode(&self) -> [u32; 2] {
        match *self {
            Self::Started => [0, 0],
            Self::Calibrated(path) => [1, path as u32],
            Self::Refused(refusal) => [2, refusal.raw()],
            Self::Failed(fault) => [3, fault.raw()],
        }
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        Some(match words {
            [0, 0] => Self::Started,
            [1, path] => Self::Calibrated(CalibrationPath::from_raw(path)?),
            [2, refusal] => Self::Refused(Refusal::from_raw(refusal)?),
            [3, fault] => Self::Failed(Fault::from_raw(fault)?),
            _ => return None,
        })
    }
}

impl fmt::Display for Registration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Started => f.write_str("phy registration started"),
            Self::Calibrated(path) => write!(f, "phy registration calibrated {path:?}"),
            Self::Refused(refusal) => write!(f, "phy registration refused {refusal:?}"),
            Self::Failed(fault) => write!(f, "phy registration failed {fault}"),
        }
    }
}

/// What one completed tracking run committed; the vendor's
/// `phy_param_track_tot` progress word.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TrackingProgress {
    /// Clients whose tracking was requested.
    pub wifi_requested: bool,
    pub bluetooth_ieee802154_requested: bool,
    pub inhibited: bool,
    pub rfpll_corrected: bool,
    pub tx_power_wifi: bool,
    pub tx_power_bluetooth_ieee802154: bool,
    pub calibration_common: bool,
    pub calibration_transmit: bool,
    pub calibration_wifi: bool,
    pub calibration_bluetooth_ieee802154: bool,
}

impl TrackingProgress {
    const fn raw(self) -> u32 {
        (self.wifi_requested as u32)
            | (self.bluetooth_ieee802154_requested as u32) << 1
            | (self.inhibited as u32) << 2
            | (self.rfpll_corrected as u32) << 3
            | (self.tx_power_wifi as u32) << 4
            | (self.tx_power_bluetooth_ieee802154 as u32) << 5
            | (self.calibration_common as u32) << 6
            | (self.calibration_transmit as u32) << 7
            | (self.calibration_wifi as u32) << 8
            | (self.calibration_bluetooth_ieee802154 as u32) << 9
    }

    fn from_raw(raw: u32) -> Option<Self> {
        if raw >> 10 != 0 {
            return None;
        }
        let bit = |index: u32| raw >> index & 1 != 0;
        Some(Self {
            wifi_requested: bit(0),
            bluetooth_ieee802154_requested: bit(1),
            inhibited: bit(2),
            rfpll_corrected: bit(3),
            tx_power_wifi: bit(4),
            tx_power_bluetooth_ieee802154: bit(5),
            calibration_common: bit(6),
            calibration_transmit: bit(7),
            calibration_wifi: bit(8),
            calibration_bluetooth_ieee802154: bit(9),
        })
    }
}

/// One periodic tracking tick.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrackingTick {
    NotDue,
    Tracked(TrackingProgress),
    /// The domain cannot track now; nothing changed.
    Unavailable(Refusal),
    Refused(Refusal),
    /// Tracking started and failed; the domain is poisoned.
    Failed(Fault),
}

impl Event for TrackingTick {
    const KIND: Kind = kind(2);
    const CHANNEL: Channel = channel(1);

    fn encode(&self) -> [u32; 2] {
        match *self {
            Self::NotDue => [0, 0],
            Self::Tracked(progress) => [1, progress.raw()],
            Self::Unavailable(refusal) => [2, refusal.raw()],
            // 3 is retired; never reuse it.
            Self::Refused(refusal) => [4, refusal.raw()],
            Self::Failed(fault) => [5, fault.raw()],
        }
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        Some(match words {
            [0, 0] => Self::NotDue,
            [1, progress] => Self::Tracked(TrackingProgress::from_raw(progress)?),
            [2, refusal] => Self::Unavailable(Refusal::from_raw(refusal)?),
            [4, refusal] => Self::Refused(Refusal::from_raw(refusal)?),
            [5, fault] => Self::Failed(Fault::from_raw(fault)?),
            _ => return None,
        })
    }
}

impl fmt::Display for TrackingTick {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotDue => f.write_str("phy tracking not due"),
            Self::Tracked(progress) => write!(f, "phy tracked {progress:?}"),
            Self::Unavailable(refusal) => write!(f, "phy tracking unavailable {refusal:?}"),
            Self::Refused(refusal) => write!(f, "phy tracking refused {refusal:?}"),
            Self::Failed(fault) => write!(f, "phy tracking failed {fault}"),
        }
    }
}

/// An RF power edge of the shared domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum RfOperation {
    /// The last client left and RF closed.
    Close = 0,
    /// RF woke from the retained registration for the next client.
    Wake = 1,
}

/// How an RF edge ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RfResult {
    Done,
    Refused(Refusal),
    /// Preparation failed after every issued transaction completed; RF
    /// stayed as it was.
    Recoverable(Fault),
    /// The domain is poisoned.
    Failed(Fault),
}

/// An RF close or wake.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RfLifecycle {
    pub operation: RfOperation,
    pub result: RfResult,
}

impl Event for RfLifecycle {
    const KIND: Kind = kind(3);
    const CHANNEL: Channel = channel(2);

    fn encode(&self) -> [u32; 2] {
        let (tag, payload) = match self.result {
            RfResult::Done => (0, 0),
            RfResult::Refused(refusal) => (1, refusal.raw()),
            RfResult::Recoverable(fault) => (2, fault.raw()),
            RfResult::Failed(fault) => (3, fault.raw()),
        };
        [self.operation as u32 | tag << 8, payload]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        let operation = match words[0] & 0xff {
            0 => RfOperation::Close,
            1 => RfOperation::Wake,
            _ => return None,
        };
        let result = match (words[0] >> 8, words[1]) {
            (0, 0) => RfResult::Done,
            (1, refusal) => RfResult::Refused(Refusal::from_raw(refusal)?),
            (2, fault) => RfResult::Recoverable(Fault::from_raw(fault)?),
            (3, fault) => RfResult::Failed(Fault::from_raw(fault)?),
            _ => return None,
        };
        Some(Self { operation, result })
    }
}

impl fmt::Display for RfLifecycle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "phy rf {:?} {:?}", self.operation, self.result)
    }
}

/// A client joined or left the shared domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClientChange {
    pub client: Client,
    pub acquired: bool,
    /// The active clients afterwards.
    pub active: Clients,
}

impl Event for ClientChange {
    const KIND: Kind = kind(4);
    const CHANNEL: Channel = channel(3);

    fn encode(&self) -> [u32; 2] {
        [
            self.client as u32 | (self.acquired as u32) << 8,
            self.active.raw(),
        ]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        Some(Self {
            client: Client::from_raw(words[0] & 0xff)?,
            acquired: flag(words[0] >> 8)?,
            active: Clients::from_raw(words[1])?,
        })
    }
}

impl fmt::Display for ClientChange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let op = if self.acquired {
            "acquired"
        } else {
            "released"
        };
        write!(
            f,
            "phy client {:?} {op}, active {}",
            self.client, self.active
        )
    }
}

/// The temperatures, in °C, that calibration and tracking committed as their
/// references.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TemperatureReferences {
    pub rfpll: i8,
    pub calibration: i8,
    pub transmit: i8,
    pub power: i8,
    /// The last observed temperature.
    pub observed: i16,
}

impl TemperatureReferences {
    const fn references(self) -> u32 {
        (self.rfpll as u8 as u32)
            | (self.calibration as u8 as u32) << 8
            | (self.transmit as u8 as u32) << 16
            | (self.power as u8 as u32) << 24
    }

    const fn from_references(word: u32, observed: i16) -> Self {
        Self {
            rfpll: word as u8 as i8,
            calibration: (word >> 8) as u8 as i8,
            transmit: (word >> 16) as u8 as i8,
            power: (word >> 24) as u8 as i8,
            observed,
        }
    }
}

impl Event for TemperatureReferences {
    const KIND: Kind = kind(5);
    const CHANNEL: Channel = channel(4);

    fn encode(&self) -> [u32; 2] {
        [self.references(), self.observed as u16 as u32]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        let observed = u16::try_from(words[1]).ok()? as i16;
        Some(Self::from_references(words[0], observed))
    }
}

impl fmt::Display for TemperatureReferences {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "phy temperature references rfpll={} calibration={} tx={} power={} observed={}",
            self.rfpll, self.calibration, self.transmit, self.power, self.observed
        )
    }
}

/// The operation that poisoned the domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PoisonedBy {
    Registration = 0,
    Tracking = 1,
    RfClose = 2,
    RfWake = 3,
    WifiChannel = 4,
}

impl PoisonedBy {
    const fn from_raw(raw: u32) -> Option<Self> {
        Some(match raw {
            0 => Self::Registration,
            1 => Self::Tracking,
            2 => Self::RfClose,
            3 => Self::RfWake,
            4 => Self::WifiChannel,
            _ => return None,
        })
    }
}

/// The domain was poisoned and requires reset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Poison {
    pub by: PoisonedBy,
    pub fault: Fault,
}

impl Poison {
    /// Entries still recorded after the poison: the teardown that follows.
    pub const POST_TRIGGER_ENTRIES: u32 = 16;
}

impl Event for Poison {
    const KIND: Kind = kind(6);
    const CHANNEL: Channel = channel(5);

    fn encode(&self) -> [u32; 2] {
        [self.by as u32, self.fault.raw()]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        Some(Self {
            by: PoisonedBy::from_raw(words[0])?,
            fault: Fault::from_raw(words[1])?,
        })
    }
}

impl fmt::Display for Poison {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "phy poisoned by {:?}: {}", self.by, self.fault)
    }
}

/// How a Wi-Fi channel selection ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelResult {
    Done,
    Refused(Refusal),
    Failed(Fault),
}

/// The shared domain tuned to a Wi-Fi channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiChannel {
    /// The primary channel number, or a frequency in MHz, as requested.
    pub channel_or_frequency: u16,
    /// The vendor channel-bandwidth code.
    pub bandwidth: u8,
    pub result: ChannelResult,
}

impl Event for WifiChannel {
    const KIND: Kind = kind(7);
    const CHANNEL: Channel = channel(6);

    fn encode(&self) -> [u32; 2] {
        let (tag, payload) = match self.result {
            ChannelResult::Done => (0, 0),
            ChannelResult::Refused(refusal) => (1, refusal.raw()),
            ChannelResult::Failed(fault) => (2, fault.raw()),
        };
        [
            self.channel_or_frequency as u32 | (self.bandwidth as u32) << 16 | tag << 24,
            payload,
        ]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        let result = match (words[0] >> 24, words[1]) {
            (0, 0) => ChannelResult::Done,
            (1, refusal) => ChannelResult::Refused(Refusal::from_raw(refusal)?),
            (2, fault) => ChannelResult::Failed(Fault::from_raw(fault)?),
            _ => return None,
        };
        Some(Self {
            channel_or_frequency: words[0] as u16,
            bandwidth: (words[0] >> 16) as u8,
            result,
        })
    }
}

impl fmt::Display for WifiChannel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "phy wifi channel {} bw={} {:?}",
            self.channel_or_frequency, self.bandwidth, self.result
        )
    }
}

oer_base_trace::event_set!(
    pub PhyTrace: Registration,
    TrackingTick,
    RfLifecycle,
    ClientChange,
    TemperatureReferences,
    Poison,
    WifiChannel,
);

/// The state slot of the shared domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Slot {
    Empty = 0,
    Registered = 1,
    Pending = 2,
    RfClosed = 3,
    Poisoned = 4,
}

impl Slot {
    const fn from_raw(raw: u32) -> Option<Self> {
        Some(match raw {
            0 => Self::Empty,
            1 => Self::Registered,
            2 => Self::Pending,
            3 => Self::RfClosed,
            4 => Self::Poisoned,
            _ => return None,
        })
    }
}

/// The PHY bus state at the failure: whether the PBus and the two
/// analog-I2C hosts were busy, and the PBus result windows in the order of
/// the vendor `phy_pbus_rd` tables (selectors 0 to 4 on path 1 and on the
/// other path, then selector 5).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BusState {
    pub pbus_busy: bool,
    pub analog_i2c_busy: [bool; 2],
    pub pbus_results: [u16; 11],
}

/// The bus state, when its clock and power domain was on. A gated
/// peripheral is not read: a read of it could hang the failure path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BusRead {
    DomainOff,
    Read(BusState),
}

/// The PHY state a post-mortem needs after a poison.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhySnapshot {
    pub poison: Poison,
    /// The slot the failing operation started from.
    pub slot: Slot,
    pub clients: Clients,
    pub temperatures: TemperatureReferences,
    pub bus: BusRead,
    /// References the radio held to each platform-owned clock, indexed as
    /// the chip's platform clocks (ESP32-S31: 160 MHz PLL, analog-I2C
    /// master, MPLL, modem coexistence, modem low-power timer), saturating.
    pub platform_clocks: [u8; PLATFORM_CLOCKS],
}

/// Platform clock slots a [`PhySnapshot`] carries.
pub const PLATFORM_CLOCKS: usize = 8;

const SNAPSHOT_VERSION: u32 = 2;
const SNAPSHOT_WORDS: usize = 14;

impl PhySnapshot {
    /// The capture point of the snapshot.
    pub const POINT: Kind = kind(0x80);

    /// The snapshot as slot words.
    pub fn encode(&self) -> [u32; SNAPSHOT_WORDS] {
        let (read, bus) = match self.bus {
            BusRead::DomainOff => (
                0,
                BusState {
                    pbus_busy: false,
                    analog_i2c_busy: [false; 2],
                    pbus_results: [0; 11],
                },
            ),
            BusRead::Read(bus) => (1, bus),
        };
        let results = bus.pbus_results;
        let pair = |low: usize, high: u16| results[low] as u32 | (high as u32) << 16;
        [
            SNAPSHOT_VERSION | (self.slot as u32) << 8 | self.clients.raw() << 16 | read << 24,
            self.poison.by as u32,
            self.poison.fault.raw(),
            self.temperatures.references(),
            self.temperatures.observed as u16 as u32,
            bus.pbus_busy as u32
                | (bus.analog_i2c_busy[0] as u32) << 1
                | (bus.analog_i2c_busy[1] as u32) << 2,
            pair(0, results[1]),
            pair(2, results[3]),
            pair(4, results[5]),
            pair(6, results[7]),
            pair(8, results[9]),
            pair(10, 0),
            u32::from_le_bytes([
                self.platform_clocks[0],
                self.platform_clocks[1],
                self.platform_clocks[2],
                self.platform_clocks[3],
            ]),
            u32::from_le_bytes([
                self.platform_clocks[4],
                self.platform_clocks[5],
                self.platform_clocks[6],
                self.platform_clocks[7],
            ]),
        ]
    }

    /// `None` for words this type never encodes, including a truncated slot.
    pub fn decode(words: &[u32]) -> Option<Self> {
        let words: &[u32; SNAPSHOT_WORDS] = words.try_into().ok()?;
        let header = words[0];
        if header & 0xff != SNAPSHOT_VERSION || header >> 25 != 0 {
            return None;
        }
        if words[5] >> 3 != 0 || words[11] >> 16 != 0 {
            return None;
        }
        let mut pbus_results = [0; 11];
        for (index, result) in pbus_results.iter_mut().enumerate() {
            *result = (words[6 + index / 2] >> (16 * (index % 2))) as u16;
        }
        let bus = BusState {
            pbus_busy: words[5] & 1 != 0,
            analog_i2c_busy: [words[5] & 2 != 0, words[5] & 4 != 0],
            pbus_results,
        };
        let bus = match header >> 24 {
            0 if bus == BusState::default() => BusRead::DomainOff,
            1 => BusRead::Read(bus),
            _ => return None,
        };
        let mut platform_clocks = [0; PLATFORM_CLOCKS];
        platform_clocks[..4].copy_from_slice(&words[12].to_le_bytes());
        platform_clocks[4..].copy_from_slice(&words[13].to_le_bytes());
        Some(Self {
            poison: Poison {
                by: PoisonedBy::from_raw(words[1])?,
                fault: Fault::from_raw(words[2])?,
            },
            slot: Slot::from_raw(header >> 8 & 0xff)?,
            clients: Clients::from_raw(header >> 16 & 0xff)?,
            temperatures: TemperatureReferences::from_references(
                words[3],
                u16::try_from(words[4]).ok()? as i16,
            ),
            bus,
            platform_clocks,
        })
    }
}

impl fmt::Display for PhySnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} from {:?}, clients {}; {}",
            self.poison, self.slot, self.clients, self.temperatures
        )?;
        match self.bus {
            BusRead::DomainOff => f.write_str("; bus not read: domain off")?,
            BusRead::Read(bus) => write!(
                f,
                "; pbus busy={} results={:03x?}; analog i2c busy={:?}",
                bus.pbus_busy, bus.pbus_results, bus.analog_i2c_busy
            )?,
        }
        write!(f, "; platform clocks held {:?}", self.platform_clocks)
    }
}

/// Record a poison: emit it, freeze the trace
/// [`Poison::POST_TRIGGER_ENTRIES`] entries later and capture `snapshot`.
/// Takes no lock and does not wait, so it may run where the poison is
/// raised.
#[inline(always)]
pub fn record_poison(snapshot: &PhySnapshot) {
    oer_base_trace::emit(&snapshot.poison);
    oer_base_trace::freeze(Poison::KIND, Poison::POST_TRIGGER_ENTRIES);
    oer_base_trace::capture(PhySnapshot::POINT, |writer| {
        writer.extend(snapshot.encode());
    });
}

#[cfg(test)]
mod tests;
