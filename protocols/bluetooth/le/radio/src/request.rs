//! Caller requests and long-lived role configuration.

use core::num::NonZeroU64;

use oer_radio_coex::{AirClass, Operation as CoexOperation, RadioClient};

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

/// The whole 2.4 GHz band the Bluetooth LE channels hop over, as one regulatory
/// span: a domain permits the radio on all of it or not at all. Its width
/// limit applies to one hop.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct LeBand;

impl oer_radio_regulatory::RadioChannel for LeBand {
    fn span(&self) -> oer_radio_regulatory::FrequencySpan {
        match oer_radio_regulatory::FrequencySpan::new(2_400_000, 2_483_500) {
            Some(span) => span,
            None => unreachable!("the band is not empty"),
        }
    }

    /// The widest LE channel (LE 2M): one hop, not the whole band.
    fn bandwidth_khz(&self) -> u32 {
        2_000
    }
}

/// What the regulatory domain allows the radio on the LE band: the most any
/// transmission radiates and whether it may start one. Taken once, when the
/// radio is installed; every [`TxPower`] is checked against it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct LeTxCeiling {
    dbm: i8,
    initiate: oer_radio_regulatory::Initiate,
}

impl LeTxCeiling {
    /// The band `band` under `ceiling`.
    pub const fn new(
        band: oer_radio_regulatory::Permitted<LeBand>,
        ceiling: oer_radio_regulatory::TxCeiling,
    ) -> Self {
        Self {
            dbm: ceiling.dbm(),
            initiate: band.initiate(),
        }
    }

    /// The most any transmission radiates, in dBm.
    pub const fn dbm(self) -> i8 {
        self.dbm
    }

    /// Whether the radio may start a transmission (advertise, send a scan or
    /// connection request) or may only listen.
    pub const fn initiate(self) -> oer_radio_regulatory::Initiate {
        self.initiate
    }
}

/// A requested power above the ceiling.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AboveCeiling {
    /// The request in dBm.
    pub requested_dbm: i8,
    /// The ceiling in dBm.
    pub ceiling_dbm: i8,
}

/// Requested transmit power in dBm, at most the regulatory ceiling; the
/// backend rounds to what it supports.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TxPower(i8);

impl TxPower {
    /// Request `dbm`, when it is within `ceiling`.
    pub const fn within(ceiling: LeTxCeiling, dbm: i8) -> Result<Self, AboveCeiling> {
        if dbm > ceiling.dbm {
            Err(AboveCeiling {
                requested_dbm: dbm,
                ceiling_dbm: ceiling.dbm,
            })
        } else {
            Ok(Self(dbm))
        }
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

/// Why one advertising event asks the shared antenna as it does.
///
/// The Controller names the reason from the set's state; the radio's
/// coexistence owner maps it to a priority. The rule that raises an event is
/// recorded in `chips/esp32s31/notes/bluetooth/le-coexistence-priorities.md`.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum AdvertisingEventKind {
    /// An event of no particular reason.
    #[default]
    Ordinary,
    /// One of the set's starting events, or an event its advertising
    /// interval's period raises.
    Raised,
}

/// Why one connection event asks the shared antenna as it does.
///
/// The Controller names the reason from the link's state; the radio's
/// coexistence owner maps it to a priority. The rule that raises an event is
/// recorded in `chips/esp32s31/notes/bluetooth/le-coexistence-priorities.md`.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ConnectionEventKind {
    /// An event of no particular reason.
    #[default]
    Ordinary,
    /// One of a connection's first events.
    Opening,
    /// An event after more events without a valid reception than the
    /// interval allows.
    MissedReceptions,
    /// An event while a local control procedure runs.
    ControlProcedure,
    /// A central connection's event after repeated failed events. The
    /// Controller decides when failures count as repeated.
    CentralFailures,
}

/// An operation of the LE radio on the shared air.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Operation {
    /// One advertising event.
    AdvertisingEvent(AdvertisingEventKind),
    /// One scan window.
    ScanWindow,
    /// One connection event.
    ConnectionEvent(ConnectionEventKind),
}

impl CoexOperation for Operation {
    const CLIENT: RadioClient = RadioClient::Bluetooth;

    fn class(&self) -> AirClass {
        match self {
            Self::AdvertisingEvent(_) | Self::ScanWindow => AirClass::PtiOnly,
            Self::ConnectionEvent(_) => AirClass::HardDeadline,
        }
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
    /// Why the event asks the air as it does.
    pub coexistence: AdvertisingEventKind,
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
    /// Why the event asks the air as it does.
    pub coexistence: ConnectionEventKind,
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
        AdvertisingEvent, AdvertisingEventKind, AdvertisingSetId, ConnectionAllowances,
        ConnectionEventKind, Operation, RadioTiming,
    };
    use crate::{
        AdvertisingChannel, AdvertisingChannels, LeInstant, LeWindow, NonZeroRadioDuration,
        RadioDuration,
    };
    use core::num::NonZeroU64;

    /// Only a connection event has a hard deadline.
    #[test]
    fn only_connection_events_have_hard_deadlines() {
        use oer_radio_coex::{AirClass, Operation as _};
        assert_eq!(
            Operation::AdvertisingEvent(AdvertisingEventKind::Raised).class(),
            AirClass::PtiOnly
        );
        assert_eq!(Operation::ScanWindow.class(), AirClass::PtiOnly);
        assert_eq!(
            Operation::ConnectionEvent(ConnectionEventKind::CentralFailures).class(),
            AirClass::HardDeadline
        );
    }

    #[test]
    fn advertising_channels_follow_one_spacing_apart() {
        let event = AdvertisingEvent {
            set: AdvertisingSetId::new(0),
            anchor: LeInstant::from_micros(1_000),
            channels: AdvertisingChannels::new(false, true, true).unwrap(),
            channel_spacing: RadioDuration::from_micros(400),
            coexistence: AdvertisingEventKind::Ordinary,
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
