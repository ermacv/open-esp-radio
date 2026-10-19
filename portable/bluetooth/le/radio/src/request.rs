//! Caller requests and long-lived role configuration.

use core::num::NonZeroU64;

use oer_radio_coex::CoexPriority;

use crate::{
    AdvertisingChannels, AdvertisingPdu, DataChannel, LeInstant, LePhy, LeRadio, LeWindow,
    NonZeroRadioDuration, RadioDuration, TestChannel, TestPayloadType, channel::AdvertisingChannel,
};

macro_rules! role_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        #[repr(transparent)]
        pub struct $name(u8);

        impl $name {
            /// Preserve one caller-chosen index.
            pub const fn new(index: u8) -> Self {
                Self(index)
            }

            /// The caller-chosen index.
            pub const fn index(self) -> u8 {
                self.0
            }
        }
    };
}

role_id!(
    /// One configured advertising set.
    AdvertisingSetId
);
role_id!(
    /// One configured scanner.
    ScannerId
);
role_id!(
    /// One open connection.
    ConnectionId
);

/// Requested transmit power in dBm; the backend rounds to what it supports.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TxPower(i8);

impl TxPower {
    /// Request `dbm`.
    pub const fn from_dbm(dbm: i8) -> Self {
        Self(dbm)
    }

    /// The requested power.
    pub const fn dbm(self) -> i8 {
        self.0
    }
}

/// Access Address octets in air order.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AccessAddress(pub [u8; 4]);

/// CRC initialization octets in air order.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CrcInit(pub [u8; 3]);

/// What an advertising set listens for after each of its PDUs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdvertisingReception<'pdu> {
    /// The set only transmits.
    None,
    /// The set answers `SCAN_REQ` with this `SCAN_RSP` and reports every
    /// reception.
    ScanResponse(AdvertisingPdu<'pdu>),
    /// The set reports every reception without answering.
    Report,
}

/// Configuration of one legacy advertising set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdvertisingConfiguration<'pdu> {
    /// The set.
    pub set: AdvertisingSetId,
    /// The advertising PDU.
    pub pdu: AdvertisingPdu<'pdu>,
    /// What the set listens for.
    pub reception: AdvertisingReception<'pdu>,
    /// Transmit power.
    pub tx_power: TxPower,
    /// PHY of the advertising PDUs and their responses.
    pub phy: LePhy,
}

/// How urgently one event needs the shared antenna.
///
/// A radio that shares the antenna with other protocols raises the event's
/// coexistence priority with the level. A Controller derives it from
/// the role's state: an advertising set periodically, a connection when it
/// is new, has missed receptions or runs a control procedure.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, PartialOrd, Ord)]
pub enum CoexistenceLevel {
    /// The role's ordinary priority.
    #[default]
    Baseline,
    /// Above the ordinary priority.
    Elevated,
    /// The highest priority of the role.
    Critical,
}

impl CoexistenceLevel {
    /// The portable priority of this level: `Baseline` is the ordinary
    /// [`CoexPriority::Normal`].
    pub const fn priority(self) -> CoexPriority {
        match self {
            Self::Baseline => CoexPriority::Normal,
            Self::Elevated => CoexPriority::Elevated,
            Self::Critical => CoexPriority::Critical,
        }
    }

    /// The level of a portable priority, or `None` for
    /// [`CoexPriority::Idle`]: an event always runs an operation.
    pub const fn from_priority(priority: CoexPriority) -> Option<Self> {
        match priority {
            CoexPriority::Idle => None,
            CoexPriority::Normal => Some(Self::Baseline),
            CoexPriority::Elevated => Some(Self::Elevated),
            CoexPriority::Critical => Some(Self::Critical),
        }
    }
}

impl From<CoexistenceLevel> for CoexPriority {
    fn from(level: CoexistenceLevel) -> Self {
        level.priority()
    }
}

/// [`CoexPriority::Idle`], which no Bluetooth LE event requests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IdlePriority;

impl TryFrom<CoexPriority> for CoexistenceLevel {
    type Error = IdlePriority;

    fn try_from(priority: CoexPriority) -> Result<Self, Self::Error> {
        Self::from_priority(priority).ok_or(IdlePriority)
    }
}

/// One advertising event of a configured set.
///
/// Channel `i` in index order has its anchor at `anchor + i *
/// channel_spacing`. The backend reserves each channel for one spacing,
/// starting its preparation lead before the channel's anchor, so a spacing
/// covers the lead, the packet and any response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdvertisingEvent {
    /// The set.
    pub set: AdvertisingSetId,
    /// Air anchor of the first channel.
    pub anchor: LeInstant,
    /// Channels used in index order.
    pub channels: AdvertisingChannels,
    /// Time between the anchors of consecutive channels.
    pub channel_spacing: RadioDuration<LeRadio>,
    /// Coexistence urgency of the event.
    pub coexistence: CoexistenceLevel,
}

impl AdvertisingEvent {
    /// The channel and its anchor at `position` in channel order. An absent
    /// position is `Ok(None)`.
    ///
    /// # Errors
    ///
    /// The anchor lies past the radio epoch.
    pub fn channel_anchor(
        &self,
        position: usize,
    ) -> Result<Option<(AdvertisingChannel, LeInstant)>, OutsideEpoch> {
        let Some(channel) = self.channels.iter().nth(position) else {
            return Ok(None);
        };
        // A present position is at most two in the three-channel wire set;
        // an offset past `u64::MAX` cannot lie in the epoch either.
        let anchor = self
            .channel_spacing
            .checked_mul(position as u64)
            .and_then(|offset| self.anchor.checked_add(offset))
            .ok_or(OutsideEpoch)?;
        Ok(Some((channel, anchor)))
    }
}

/// Configuration of one passive scanner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScannerConfiguration {
    /// The scanner.
    pub scanner: ScannerId,
    /// Whether the scanner requests scan responses.
    pub scan_type: ScanType,
    /// Which advertisers the scanner receives from.
    pub filter_policy: ScanFilterPolicy,
    /// Transmit power retained by the scanner profile.
    pub tx_power: TxPower,
    /// PHY listened on.
    pub phy: LePhy,
}

/// Which advertisers a scanner receives from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanFilterPolicy {
    /// Every advertiser.
    AcceptAll,
    /// Only the devices of the filter accept list; the backend filters in
    /// hardware, so other advertisers are neither reported nor scanned.
    AcceptListOnly,
}

/// One device of the filter accept list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AcceptListDevice {
    /// The address is random rather than public.
    pub random: bool,
    /// The address, least significant octet first.
    pub address: [u8; 6],
}

/// One change of the filter accept list. The Controller changes the list only
/// while no role filters against it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AcceptListChange {
    /// Add a device; a device already listed stays listed.
    Add(AcceptListDevice),
    /// Remove a listed device.
    Remove(AcceptListDevice),
    /// Remove every device.
    Clear,
}

/// How a scanner treats scannable advertising.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanType {
    /// Only listen.
    Passive,
    /// Send `SCAN_REQ` to scannable advertisers and receive their `SCAN_RSP`.
    Active,
}

/// One scan window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScanWindow {
    /// The scanner.
    pub scanner: ScannerId,
    /// Channel listened on.
    pub channel: AdvertisingChannel,
    /// Air window of the listening.
    pub window: LeWindow,
}

/// Configuration of one peripheral connection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectionConfiguration {
    /// The connection.
    pub connection: ConnectionId,
    /// Access Address.
    pub access_address: AccessAddress,
    /// CRC initialization.
    pub crc_init: CrcInit,
    /// The on-air start of the connection indication; the first reference
    /// point until a valid reception replaces it.
    pub created_at: LeInstant,
    /// Transmit power.
    pub tx_power: TxPower,
    /// PHY of the connection in both directions.
    pub phy: LePhy,
}

/// How long the peripheral listens for the central in one event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectionEventTiming {
    /// The first event: the transmit window widened by the timing guard on
    /// both sides.
    First {
        /// Transmit window width.
        transmit_window: RadioDuration<LeRadio>,
        /// Timing uncertainty on each side.
        timing_guard: RadioDuration<LeRadio>,
    },
    /// A later event with its complete widened receive wait.
    Recurring {
        /// Total time to wait for the central.
        receive_wait: RadioDuration<LeRadio>,
        /// Window widening on each side of the anchor: the clock drift since
        /// the last received anchor plus the backend's jitter.
        widening: RadioDuration<LeRadio>,
    },
}

/// One connection event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectionEvent {
    /// The connection.
    pub connection: ConnectionId,
    /// Data channel of the event.
    pub channel: DataChannel,
    /// Air window: its start is the earliest anchor, its duration the air
    /// time the event reserves.
    pub window: LeWindow,
    /// Connection interval. A backend may let the event continue past its
    /// air window while it stays within the interval.
    pub interval: RadioDuration<LeRadio>,
    /// Receive wait of the event.
    pub timing: ConnectionEventTiming,
    /// Scheduling priority, 0 to 15.
    pub priority: u8,
    /// Coexistence urgency of the event.
    pub coexistence: CoexistenceLevel,
}

/// One Direct Test Mode transmitter event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TestTransmit<'payload> {
    /// RF channel.
    pub channel: TestChannel,
    /// PHY.
    pub phy: TestPhy,
    /// Air window of the packet.
    pub window: LeWindow,
    /// Transmit power.
    pub tx_power: TxPower,
    /// LE Test packet payload type.
    pub payload_type: TestPayloadType,
    /// Payload.
    pub payload: &'payload [u8],
}

/// One Direct Test Mode receiver event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TestReceive {
    /// RF channel.
    pub channel: TestChannel,
    /// PHY.
    pub phy: TestPhy,
    /// Air window of the listening.
    pub window: LeWindow,
    /// Whether the receiver ran before in the same test.
    pub recurring: bool,
    /// Transmit power retained by the test profile.
    pub tx_power: TxPower,
}

/// PHY of a Direct Test Mode event.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TestPhy {
    /// LE 1M.
    Le1M,
    /// LE 2M.
    Le2M,
    /// LE Coded with S=8 coding.
    LeCodedS8,
    /// LE Coded with S=2 coding; a receiver accepts either coding.
    LeCodedS2,
}

/// The backend's timing, which the planner uses to keep reservations apart.
///
/// A backend reserves `[anchor - preparation_lead, window end)` for every
/// event and refuses an event whose reservation starts less than
/// `admission_guard` after the current time.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RadioTiming {
    /// Time the backend prepares the radio before an anchor.
    pub preparation_lead: RadioDuration<LeRadio>,
    /// Minimum time from a request to the start of its reservation.
    pub admission_guard: RadioDuration<LeRadio>,
    /// How the backend listens around connection anchors.
    pub connection: ConnectionAllowances,
}

impl RadioTiming {
    /// No lead, guard or allowance beyond the one-microsecond boundary guard
    /// every connection event keeps: the timing of a backend that schedules
    /// nothing.
    pub const MINIMAL: Self = Self {
        preparation_lead: ZERO,
        admission_guard: ZERO,
        connection: ConnectionAllowances {
            local_sleep_clock_ppm: 0,
            widening_jitter: ZERO,
            receive_guard: ZERO,
            receive_tail: ZERO,
            boundary_guard: NonZeroRadioDuration::from_micros(NonZeroU64::MIN),
            first_event_guard: ZERO,
            event_length: ZERO,
            first_event_length: ZERO,
        },
    };
}

const ZERO: RadioDuration<LeRadio> = RadioDuration::from_micros(0);

/// The backend's allowances around a connection anchor, which the planner
/// adds to the Link Layer's own window widening.
///
/// For an anchor `A`, a widening `W` (the specification's clock drift plus
/// `widening_jitter`) and an outstanding transmit window `T`, a recurring
/// event listens for `receive_guard + 2W + T + receive_tail` from
/// `A - receive_guard - W` and occupies the air from
/// `A - receive_guard - W - boundary_guard` to `A + W + T + event_length`.
/// The first event listens across `T` widened by `first_event_guard` on each
/// side and occupies the air from `A - first_event_guard - boundary_guard`
/// to `A + T + first_event_length`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ConnectionAllowances {
    /// Worst-case accuracy of the local sleep clock, in parts per million.
    pub local_sleep_clock_ppm: u16,
    /// Timing jitter added to every window widening.
    pub widening_jitter: RadioDuration<LeRadio>,
    /// Fixed listening before the widened anchor of a recurring event.
    pub receive_guard: RadioDuration<LeRadio>,
    /// Listening kept after the widened receive window.
    pub receive_tail: RadioDuration<LeRadio>,
    /// Guard before an event's air window. Every event window includes it,
    /// so none is empty.
    pub boundary_guard: NonZeroRadioDuration<LeRadio>,
    /// Uncertainty on each side of the first event's transmit window.
    pub first_event_guard: RadioDuration<LeRadio>,
    /// Air time a recurring event keeps after its widened anchor window.
    pub event_length: RadioDuration<LeRadio>,
    /// Air time the first event keeps after its transmit window.
    pub first_event_length: RadioDuration<LeRadio>,
}

/// An instant, window or span radio geometry needs lies outside the radio
/// epoch: before its first instant or past its last one. A span longer than
/// the whole epoch cannot lie in it either.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct OutsideEpoch;

impl RadioTiming {
    /// The reservation of a checked air window: the window, starting the
    /// preparation lead earlier.
    ///
    /// # Errors
    ///
    /// The reservation would start before the radio epoch.
    pub const fn reservation(&self, window: LeWindow) -> Result<LeWindow, OutsideEpoch> {
        match window.extended_before(self.preparation_lead) {
            Some(reservation) => Ok(reservation),
            None => Err(OutsideEpoch),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AdvertisingEvent, AdvertisingSetId, CoexPriority, CoexistenceLevel, ConnectionAllowances,
        IdlePriority, RadioTiming,
    };
    use crate::{
        AdvertisingChannel, AdvertisingChannels, LeInstant, LeWindow, NonZeroRadioDuration,
        RadioDuration,
    };
    use core::num::NonZeroU64;

    /// Every level keeps its order as a portable priority and returns from
    /// it; the idle priority has no event level.
    #[test]
    fn coexistence_levels_round_trip_through_portable_priorities() {
        let levels = [
            CoexistenceLevel::Baseline,
            CoexistenceLevel::Elevated,
            CoexistenceLevel::Critical,
        ];
        let priorities = levels.map(CoexPriority::from);
        assert!(priorities.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(priorities.map(CoexistenceLevel::try_from), levels.map(Ok));
        assert_eq!(
            CoexistenceLevel::try_from(CoexPriority::Idle),
            Err(IdlePriority)
        );
        assert_eq!(
            CoexPriority::from(CoexistenceLevel::default()),
            CoexPriority::default()
        );
    }

    #[test]
    fn advertising_channels_follow_one_spacing_apart() {
        let event = AdvertisingEvent {
            set: AdvertisingSetId::new(0),
            anchor: LeInstant::from_micros(1_000),
            channels: AdvertisingChannels::new(false, true, true).unwrap(),
            channel_spacing: RadioDuration::from_micros(400),
            coexistence: super::CoexistenceLevel::Baseline,
        };
        assert_eq!(
            event.channel_anchor(1),
            Ok(Some((
                AdvertisingChannel::Channel39,
                LeInstant::from_micros(1_400)
            )))
        );
        assert_eq!(event.channel_anchor(2), Ok(None));
    }

    #[test]
    fn a_reservation_adds_the_preparation_lead() {
        let timing = RadioTiming {
            preparation_lead: RadioDuration::from_micros(300),
            admission_guard: RadioDuration::from_micros(100),
            connection: ConnectionAllowances {
                local_sleep_clock_ppm: 500,
                widening_jitter: RadioDuration::from_micros(0),
                receive_guard: RadioDuration::from_micros(0),
                receive_tail: RadioDuration::from_micros(0),
                boundary_guard: NonZeroRadioDuration::from_micros(NonZeroU64::MIN),
                first_event_guard: RadioDuration::from_micros(0),
                event_length: RadioDuration::from_micros(0),
                first_event_length: RadioDuration::from_micros(0),
            },
        };
        let air = LeWindow::new(
            LeInstant::from_micros(1_000),
            RadioDuration::from_micros(50),
        )
        .unwrap();
        let reserved = timing.reservation(air).unwrap();
        assert_eq!(reserved.start(), LeInstant::from_micros(700));
        assert_eq!(reserved.end(), LeInstant::from_micros(1_050));
        let early =
            LeWindow::new(LeInstant::from_micros(10), RadioDuration::from_micros(5)).unwrap();
        assert_eq!(timing.reservation(early), Err(super::OutsideEpoch));
    }
}
