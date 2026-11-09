//! The facade's own trace events: what the composed station saw at the port,
//! its datapath actor's spans and its rate selection. They are the facade's vocabulary on the image's
//! trace observer; the protocol's events reach the image's event sink
//! instead ([`crate::Binding::Sink`]).
//!
//! Each kind keeps its id in the IEEE 802.11 trace domain (the S31 driver's
//! lower-MAC trace uses ids from 100) and its channel.

use oer_portable_trace::{Channel, Domain, Event, Kind};
pub use oer_protocols_ieee80211_core_roles_sta::events::{ConnectStep, ScanHeard, ScanProbeEnd};

/// The station activity channel.
const SERVICE: Channel = const {
    match Channel::new(Domain::Ieee80211, 1) {
        Ok(channel) => channel,
        Err(_) => panic!("the station activity channel lies in the IEEE 802.11 range"),
    }
};
/// A connect's step at a wrapping microsecond.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StationConnectStep {
    pub at_micros: u32,
    pub step: ConnectStep,
    /// The channel, 0 for none.
    pub channel: u8,
}

impl Event for StationConnectStep {
    const KIND: Kind = Kind::new(Domain::Ieee80211, 41);
    const CHANNEL: Channel = SERVICE;

    fn encode(&self) -> [u32; 2] {
        [
            self.at_micros,
            u32::from_le_bytes([self.step as u8, self.channel, 0, 0]),
        ]
    }

    fn decode([at_micros, word]: [u32; 2]) -> Option<Self> {
        let [step, channel, 0, 0] = word.to_le_bytes() else {
            return None;
        };
        Some(Self {
            at_micros,
            step: ConnectStep::ALL.get(usize::from(step)).copied()?,
            channel,
        })
    }
}

impl core::fmt::Display for StationConnectStep {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "station.connect at_micros={} step={:?} channel={}",
            self.at_micros, self.step, self.channel
        )
    }
}

/// A frame a scan heard, at a wrapping microsecond.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StationScanHeard {
    pub at_micros: u32,
    pub heard: ScanHeard,
}

impl Event for StationScanHeard {
    const KIND: Kind = Kind::new(Domain::Ieee80211, 42);
    const CHANNEL: Channel = SERVICE;

    fn encode(&self) -> [u32; 2] {
        let ScanHeard {
            own,
            probe_response,
            to_us,
            rssi,
        } = self.heard;
        [
            self.at_micros,
            u32::from_le_bytes([
                u8::from(own) | u8::from(to_us) << 1,
                u8::from(probe_response),
                u8::from(rssi.is_some()),
                rssi.unwrap_or(0).to_le_bytes()[0],
            ]),
        ]
    }

    fn decode([at_micros, word]: [u32; 2]) -> Option<Self> {
        let flag = |byte| match byte {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        };
        let [flags, probe_response, known, rssi] = word.to_le_bytes();
        let known = flag(known)?;
        if flags > 3 {
            return None;
        }
        Some(Self {
            at_micros,
            heard: ScanHeard {
                own: flags & 1 != 0,
                to_us: flags & 2 != 0,
                probe_response: flag(probe_response)?,
                rssi: known.then_some(i8::from_le_bytes([rssi])),
            },
        })
    }
}

impl core::fmt::Display for StationScanHeard {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "station.scan_heard at_micros={} {:?}",
            self.at_micros, self.heard
        )
    }
}

/// A scan's probe built, or ended, at a wrapping microsecond.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StationScanProbe {
    pub at_micros: u32,
    pub step: ScanProbeStep,
}

/// What happened to a scan's probe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanProbeStep {
    Built,
    Transmitted,
    NotTransmitted,
    Unknown,
}

impl ScanProbeStep {
    const fn word(self) -> u32 {
        match self {
            Self::Built => 0,
            Self::Transmitted => 1,
            Self::NotTransmitted => 2,
            Self::Unknown => 3,
        }
    }

    const fn from_word(word: u32) -> Option<Self> {
        match word {
            0 => Some(Self::Built),
            1 => Some(Self::Transmitted),
            2 => Some(Self::NotTransmitted),
            3 => Some(Self::Unknown),
            _ => None,
        }
    }
}

impl Event for StationScanProbe {
    const KIND: Kind = Kind::new(Domain::Ieee80211, 43);
    const CHANNEL: Channel = SERVICE;

    fn encode(&self) -> [u32; 2] {
        [self.at_micros, self.step.word()]
    }

    fn decode([at_micros, word]: [u32; 2]) -> Option<Self> {
        match ScanProbeStep::from_word(word) {
            Some(step) => Some(Self { at_micros, step }),
            None => None,
        }
    }
}

impl core::fmt::Display for StationScanProbe {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "station.scan_probe at_micros={} step={:?}",
            self.at_micros, self.step
        )
    }
}

/// The rate selection channel.
const AGGREGATION: Channel = const {
    match Channel::new(Domain::Ieee80211, 4) {
        Ok(channel) => channel,
        Err(_) => panic!("the rate selection channel lies in the IEEE 802.11 range"),
    }
};

/// An event without fields.
macro_rules! unit_event {
    ($(#[$meta:meta])* $name:ident = $id:literal on $channel:path, $text:literal) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub struct $name;

        impl Event for $name {
            const KIND: Kind = Kind::new(Domain::Ieee80211, $id);
            const CHANNEL: Channel = $channel;

            fn encode(&self) -> [u32; 2] {
                [0, 0]
            }

            fn decode(words: [u32; 2]) -> Option<Self> {
                (words == [0, 0]).then_some(Self)
            }
        }

        impl core::fmt::Display for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str($text)
            }
        }
    };
}

unit_event!(
    /// A successful open authentication response addressed to the local station.
    StationAuthenticated = 22 on SERVICE,
    "station.authenticated"
);

/// A beacon from the configured target, with wrapping monotonic milliseconds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StationBeacon {
    pub at_millis: u32,
}

impl Event for StationBeacon {
    const KIND: Kind = Kind::new(Domain::Ieee80211, 23);
    const CHANNEL: Channel = SERVICE;

    fn encode(&self) -> [u32; 2] {
        [self.at_millis, 0]
    }

    fn decode([at_millis, reserved]: [u32; 2]) -> Option<Self> {
        (reserved == 0).then_some(Self { at_millis })
    }
}

impl core::fmt::Display for StationBeacon {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "station.beacon at_millis={}", self.at_millis)
    }
}

unit_event!(
    /// Rate selection accepted the associated peer's capabilities.
    StationAssociated = 24 on SERVICE,
    "station.associated"
);
unit_event!(
    /// A single-TX terminal had an unknown outcome, of any cause.
    TxIndeterminate = 25 on SERVICE,
    "station.tx-indeterminate"
);
unit_event!(
    /// Rate selection received a single-TX hardware timeout.
    TxHardwareTimeout = 26 on SERVICE,
    "station.tx-hardware-timeout"
);
unit_event!(
    /// One EDCA queue had a sampled collision pending, not a retry count.
    TxQueueCollision = 27 on SERVICE,
    "station.tx-queue-collision"
);
unit_event!(
    /// A control frame completed with delivery confirmed.
    ControlDelivered = 28 on SERVICE,
    "station.control-delivered"
);

/// First rate of the data retry plan after association or a learned
/// terminal. `None` means that the selection has no admissible data plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxRateSelected {
    pub kbps: Option<u32>,
}

impl Event for TxRateSelected {
    const KIND: Kind = Kind::new(Domain::Ieee80211, 40);
    const CHANNEL: Channel = AGGREGATION;

    fn encode(&self) -> [u32; 2] {
        [self.kbps.unwrap_or(0), u32::from(self.kbps.is_some())]
    }

    fn decode([kbps, known]: [u32; 2]) -> Option<Self> {
        match known {
            0 if kbps == 0 => Some(Self { kbps: None }),
            1 => Some(Self { kbps: Some(kbps) }),
            _ => None,
        }
    }
}

impl core::fmt::Display for TxRateSelected {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "TxRateSelected {self:?}")
    }
}

/// One PPDU's rate as the link histograms class it: its HT MCS, width and
/// guard interval, or only its format.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RateClass {
    /// A DSSS, CCK or OFDM rate.
    NonHt,
    /// An HT rate.
    Ht {
        /// The MCS index, 0 to 31.
        mcs: u8,
        width40: bool,
        short_gi: bool,
    },
    /// An HE rate.
    He,
    /// No rate: the frame's metadata or the outcome carried none.
    Unknown,
}

impl RateClass {
    /// The class of `rate`, `Unknown` without one.
    pub const fn of(rate: Option<oer_protocols_ieee80211_wire::phy::PhyRate>) -> Self {
        use oer_protocols_ieee80211_wire::phy::{PhyRate, PpduBandwidth};
        match rate {
            Some(PhyRate::Legacy(_)) => Self::NonHt,
            Some(PhyRate::Ht(rate)) => Self::Ht {
                mcs: rate.mcs().index(),
                width40: matches!(rate.bandwidth(), PpduBandwidth::Mhz40),
                short_gi: rate.short_gi(),
            },
            Some(PhyRate::He(_)) => Self::He,
            None => Self::Unknown,
        }
    }

    /// The class in one word: the format in bits 0..8, an HT rate's MCS in
    /// bits 8..16, its width in bit 16 and its guard interval in bit 17.
    const fn encode(self) -> u32 {
        match self {
            Self::Unknown => 0,
            Self::NonHt => 1,
            Self::Ht {
                mcs,
                width40,
                short_gi,
            } => 2 | ((mcs as u32) << 8) | ((width40 as u32) << 16) | ((short_gi as u32) << 17),
            Self::He => 3,
        }
    }

    /// The class of one word [`Self::encode`] wrote; `None` for any other.
    const fn decode(word: u32) -> Option<Self> {
        let mcs = (word >> 8) as u8;
        match word {
            0 => Some(Self::Unknown),
            1 => Some(Self::NonHt),
            3 => Some(Self::He),
            _ if word & 0xff == 2 && word >> 18 == 0 && mcs <= 31 => Some(Self::Ht {
                mcs,
                width40: word & (1 << 16) != 0,
                short_gi: word & (1 << 17) != 0,
            }),
            _ => None,
        }
    }
}

/// Whether a received MPDU came in an A-MPDU, as its metadata tells.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Containment {
    Unknown,
    Single,
    InAmpdu,
}

impl Containment {
    /// The containment `evidence` establishes, hardware-observed or
    /// protocol-validated alike.
    pub const fn of(evidence: oer_protocols_ieee80211_contracts_port::RxEvidence<bool>) -> Self {
        use oer_protocols_ieee80211_contracts_port::RxEvidence;
        match evidence {
            RxEvidence::HardwareObserved(true) | RxEvidence::ProtocolValidated(true) => {
                Self::InAmpdu
            }
            RxEvidence::HardwareObserved(false) | RxEvidence::ProtocolValidated(false) => {
                Self::Single
            }
            RxEvidence::Unavailable => Self::Unknown,
        }
    }

    const fn decode(word: u32) -> Option<Self> {
        match word {
            0 => Some(Self::Unknown),
            1 => Some(Self::Single),
            2 => Some(Self::InAmpdu),
            _ => None,
        }
    }
}

/// A data MPDU addressed to the station's own address reached the port:
/// its PPDU's rate and whether it came in an A-MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxData {
    pub rate: RateClass,
    pub containment: Containment,
}

impl Event for RxData {
    const KIND: Kind = Kind::new(Domain::Ieee80211, 31);
    const CHANNEL: Channel = SERVICE;

    fn encode(&self) -> [u32; 2] {
        [self.rate.encode(), self.containment as u32]
    }

    fn decode([rate, containment]: [u32; 2]) -> Option<Self> {
        Some(Self {
            rate: RateClass::decode(rate)?,
            containment: Containment::decode(containment)?,
        })
    }
}

impl core::fmt::Display for RxData {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "station.rx-data {:?} {:?}", self.rate, self.containment)
    }
}

/// How a single data MPDU's transmission ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum TxEnd {
    /// Its response arrived, or it solicited none.
    Delivered,
    /// Its plan exhausted the ACK budget.
    AckTimeout,
    /// Another known end: a protection timeout, the contention limit or a
    /// coexistence denial.
    Other,
    /// It expired, was withdrawn or ended without a known outcome; no
    /// transmission is reported.
    Unsent,
}

impl TxEnd {
    const ALL: [Self; 4] = [Self::Delivered, Self::AckTimeout, Self::Other, Self::Unsent];
}

/// A single data MPDU's transmission ended: the rate of its last
/// transmission and how many transmissions it took (none when it reports
/// none).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxData {
    pub rate: RateClass,
    pub transmissions: u16,
    pub end: TxEnd,
}

impl TxData {
    /// The end of a data MPDU's single-TX `outcome`.
    pub fn of<A, D>(outcome: &oer_protocols_ieee80211_contracts_port::SingleOutcome<A, D>) -> Self {
        use oer_protocols_ieee80211_contracts_port::{SingleEnd, TxOutcome};
        let (air, end) = match outcome {
            TxOutcome::Ended(SingleEnd::Delivered { air, .. }) => (Some(*air), TxEnd::Delivered),
            TxOutcome::Ended(SingleEnd::AckTimeout(air)) => (Some(*air), TxEnd::AckTimeout),
            TxOutcome::Ended(
                SingleEnd::CtsTimeout(air)
                | SingleEnd::ContentionLimit(air)
                | SingleEnd::AirDenied { air, .. },
            ) => (*air, TxEnd::Other),
            TxOutcome::Expired { .. } | TxOutcome::Withdrawn(_) | TxOutcome::Indeterminate(_) => {
                (None, TxEnd::Unsent)
            }
        };
        Self {
            rate: RateClass::of(air.map(|air| air.rate)),
            transmissions: air.map_or(0, |air| air.transmissions.get()),
            end,
        }
    }
}

impl Event for TxData {
    const KIND: Kind = Kind::new(Domain::Ieee80211, 32);
    const CHANNEL: Channel = AGGREGATION;

    fn encode(&self) -> [u32; 2] {
        [
            self.rate.encode(),
            u32::from(self.transmissions) | ((self.end as u32) << 16),
        ]
    }

    fn decode([rate, word]: [u32; 2]) -> Option<Self> {
        Some(Self {
            rate: RateClass::decode(rate)?,
            transmissions: word as u16,
            end: *TxEnd::ALL.get(usize::try_from(word >> 16).ok()?)?,
        })
    }
}

impl core::fmt::Display for TxData {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "station.tx-data {:?} transmissions={} {:?}",
            self.rate, self.transmissions, self.end
        )
    }
}

/// The datapath actor's spans, which [`ActorSpan`] reports.
pub use oer_protocols_ieee80211_runtime::datapath::ActorSpan as Span;

/// The start or end of a stretch of the datapath actor's work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ActorSpan {
    pub span: Span,
    pub end: bool,
}

impl Event for ActorSpan {
    const KIND: Kind = Kind::new(Domain::Ieee80211, 29);
    const CHANNEL: Channel = SERVICE;

    fn encode(&self) -> [u32; 2] {
        [u32::from(self.span as u8), u32::from(self.end)]
    }

    fn decode([span, end]: [u32; 2]) -> Option<Self> {
        Some(Self {
            span: Span::from_number(u8::try_from(span).ok()?)?,
            end: match end {
                0 => false,
                1 => true,
                _ => return None,
            },
        })
    }
}

impl core::fmt::Display for ActorSpan {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let edge = if self.end { "end" } else { "start" };
        write!(f, "station.span {:?} {edge}", self.span)
    }
}

/// What the station service decided about received frames: the kind of
/// one decision, which [`StationRx`] counts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum StationRxKind {
    /// The station dropped a frame, by its receive drop reason, a security
    /// refusal or the drain of a poisoned port.
    DroppedHeader,
    DroppedFrameKind,
    DroppedAddress,
    DroppedFragmented,
    DroppedProtection,
    DroppedIntegrity,
    DroppedReplay,
    DroppedPayload,
    DroppedNotConnected,
    DroppedSecurity,
    DroppedDraining,
    /// MSDUs not delivered, by reason.
    RefusedTooLong,
    RefusedQueueFull,
    RefusedNoNetwork,
    /// MSDUs copied instead of lent, by reason: by form, an A-MSDU's or
    /// one that shared its frame with the station's EAPOL; by pressure, one
    /// whose buffer the backend kept.
    CopiedAggregated,
    CopiedKept,
    CopiedShared,
    /// An MPDU the receive agreements admitted, in an A-MPDU, alone, or
    /// without evidence of either.
    MpduInAmpdu,
    MpduSingle,
    MpduUnknown,
    /// MPDUs the reorder released, by the decision that released them.
    ReleasedInOrder,
    ReleasedByTimeout,
    ReleasedByBar,
    ReleasedByStop,
    /// Sequences the reorder skipped without an MPDU.
    MissingByTimeout,
    MissingOther,
    /// A buffered duplicate, or other rejected reorder input.
    Duplicate,
    Rejected,
    /// A receive agreement started (its count is its window) or ended.
    AgreementStarted,
    AgreementStopped,
    /// A connected RSN input the station dropped.
    RsnInputDropped,
    /// MSDUs not delivered for pressure on the station's handoff: the
    /// network held all the port may lend past the pressure bound.
    RefusedPressure,
}

impl StationRxKind {
    pub const ALL: [Self; 32] = [
        Self::DroppedHeader,
        Self::DroppedFrameKind,
        Self::DroppedAddress,
        Self::DroppedFragmented,
        Self::DroppedProtection,
        Self::DroppedIntegrity,
        Self::DroppedReplay,
        Self::DroppedPayload,
        Self::DroppedNotConnected,
        Self::DroppedSecurity,
        Self::DroppedDraining,
        Self::RefusedTooLong,
        Self::RefusedQueueFull,
        Self::RefusedNoNetwork,
        Self::CopiedAggregated,
        Self::CopiedKept,
        Self::CopiedShared,
        Self::MpduInAmpdu,
        Self::MpduSingle,
        Self::MpduUnknown,
        Self::ReleasedInOrder,
        Self::ReleasedByTimeout,
        Self::ReleasedByBar,
        Self::ReleasedByStop,
        Self::MissingByTimeout,
        Self::MissingOther,
        Self::Duplicate,
        Self::Rejected,
        Self::AgreementStarted,
        Self::AgreementStopped,
        Self::RsnInputDropped,
        Self::RefusedPressure,
    ];

    fn from_raw(raw: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| *kind as u32 == raw)
    }
}

/// One receive decision of the station service and its count: MSDUs for
/// refusals and copies, released MPDUs or skipped sequences for the
/// reorder, the window for a started agreement, one otherwise.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StationRx {
    pub kind: StationRxKind,
    pub count: u32,
}

impl Event for StationRx {
    const KIND: Kind = Kind::new(Domain::Ieee80211, 30);
    const CHANNEL: Channel = SERVICE;

    fn encode(&self) -> [u32; 2] {
        [self.kind as u32, self.count]
    }

    fn decode([kind, count]: [u32; 2]) -> Option<Self> {
        Some(Self {
            kind: StationRxKind::from_raw(kind)?,
            count,
        })
    }
}

impl core::fmt::Display for StationRx {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "station.rx {:?} {}", self.kind, self.count)
    }
}

/// What the station's transmit agreements settled: one data aggregate's
/// members by outcome, or one BlockAckReq the station sent
/// (`members` zero and `block_ack_request` set).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StationTxAmpdu {
    /// The aggregate's members: the subframes it went with.
    pub members: u8,
    /// Members the BlockAck acknowledged.
    pub acknowledged: u8,
    /// Members back in their queue for a later aggregate.
    pub retried: u8,
    /// Members dropped.
    pub dropped: u8,
    /// Whether the aggregate was refused before the port.
    pub refused: bool,
    /// One BlockAckReq the station sent, instead of an aggregate.
    pub block_ack_request: bool,
    /// The rate the aggregate's last transmission went at.
    pub rate: RateClass,
}

impl Default for StationTxAmpdu {
    fn default() -> Self {
        Self {
            members: 0,
            acknowledged: 0,
            retried: 0,
            dropped: 0,
            refused: false,
            block_ack_request: false,
            rate: RateClass::Unknown,
        }
    }
}

impl Event for StationTxAmpdu {
    const KIND: Kind = Kind::new(Domain::Ieee80211, 33);
    const CHANNEL: Channel = SERVICE;

    fn encode(&self) -> [u32; 2] {
        [
            u32::from_le_bytes([self.members, self.acknowledged, self.retried, self.dropped]),
            // The rate's word takes 18 bits above the two flags.
            u32::from(self.refused)
                | (u32::from(self.block_ack_request) << 1)
                | (self.rate.encode() << 2),
        ]
    }

    fn decode([counts, word]: [u32; 2]) -> Option<Self> {
        let [members, acknowledged, retried, dropped] = counts.to_le_bytes();
        Some(Self {
            members,
            acknowledged,
            retried,
            dropped,
            refused: word & 1 != 0,
            block_ack_request: word & 0b10 != 0,
            rate: RateClass::decode(word >> 2)?,
        })
    }
}

/// One ADDBA Request the station's receive agreements took: its TID, the
/// Buffer Size the access point asked for as received (zero asks the
/// station to choose) and what became of it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StationAddba {
    pub tid: u8,
    pub requested: u16,
    pub outcome: AddbaOutcome,
}

/// What became of one ADDBA Request, as the recipient names it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddbaOutcome {
    /// A new agreement started with `window`.
    Accepted { window: u16 },
    /// The running agreement's own request again, answered with its
    /// `window`.
    Repeated { window: u16 },
    /// Refused with this IEEE 802.11 status code.
    Refused { status: u16 },
    /// No reply place or bank remained: the request went unanswered.
    Unanswered,
}

impl StationAddba {
    fn of(
        tid: u8,
        requested: u16,
        outcome: oer_protocols_ieee80211_core_upper_mac::block_ack::AddbaOutcome,
    ) -> Self {
        use oer_protocols_ieee80211_core_upper_mac::block_ack::AddbaOutcome as Recipient;
        Self {
            tid,
            requested,
            outcome: match outcome {
                Recipient::Accepted { window } => AddbaOutcome::Accepted { window },
                Recipient::Repeated { window } => AddbaOutcome::Repeated { window },
                Recipient::Refused(status) => AddbaOutcome::Refused {
                    status: status as u16,
                },
                Recipient::Unanswered => AddbaOutcome::Unanswered,
            },
        }
    }
}

impl Event for StationAddba {
    const KIND: Kind = Kind::new(Domain::Ieee80211, 44);
    const CHANNEL: Channel = SERVICE;

    fn encode(&self) -> [u32; 2] {
        let (code, value) = match self.outcome {
            AddbaOutcome::Accepted { window } => (0, window),
            AddbaOutcome::Repeated { window } => (1, window),
            AddbaOutcome::Refused { status } => (2, status),
            AddbaOutcome::Unanswered => (3, 0),
        };
        let [low, high] = self.requested.to_le_bytes();
        [
            u32::from_le_bytes([self.tid, code, low, high]),
            u32::from(value),
        ]
    }

    fn decode([word, value]: [u32; 2]) -> Option<Self> {
        let [tid, code, low, high] = word.to_le_bytes();
        let value = u16::try_from(value).ok()?;
        let outcome = match code {
            0 => AddbaOutcome::Accepted { window: value },
            1 => AddbaOutcome::Repeated { window: value },
            2 => AddbaOutcome::Refused { status: value },
            3 if value == 0 => AddbaOutcome::Unanswered,
            _ => return None,
        };
        Some(Self {
            tid,
            requested: u16::from_le_bytes([low, high]),
            outcome,
        })
    }
}

impl core::fmt::Display for StationAddba {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "station.addba tid {} asked {} {:?}",
            self.tid, self.requested, self.outcome
        )
    }
}

/// One answered first transmission of an aggregate: its subframes and the
/// positions the BlockAck left unacknowledged, bit 0 the first on air.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StationTxHoles {
    pub members: u8,
    pub holes: u32,
}

impl Event for StationTxHoles {
    const KIND: Kind = Kind::new(Domain::Ieee80211, 34);
    const CHANNEL: Channel = SERVICE;

    fn encode(&self) -> [u32; 2] {
        [self.holes, u32::from(self.members)]
    }

    fn decode([holes, members]: [u32; 2]) -> Option<Self> {
        Some(Self {
            members: u8::try_from(members).ok()?,
            holes,
        })
    }
}

impl core::fmt::Display for StationTxHoles {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "station.tx {} holes {:#x}", self.members, self.holes)
    }
}

impl core::fmt::Display for StationTxAmpdu {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.block_ack_request {
            return write!(f, "station.tx bar");
        }
        write!(
            f,
            "station.tx ampdu {} acked {} retried {} dropped {} {:?}{}",
            self.members,
            self.acknowledged,
            self.retried,
            self.dropped,
            self.rate,
            if self.refused { " refused" } else { "" }
        )
    }
}

/// The station's event sink of a trace build: it reports each receive
/// decision of the service, the station and its receive agreements as
/// [`StationRx`], and each data aggregate's settlement and BlockAckReq as
/// [`StationTxAmpdu`], to the image's observer `O` and keeps nothing. Without
/// `oer_trace` it is disabled, so the service builds no event.
#[derive(Clone, Copy, Debug, Default)]
pub struct TracedStationEvents<O>(pub O);

impl<O: oer_portable_trace::Observer> TracedStationEvents<O> {
    #[cfg_attr(
        not(oer_trace),
        allow(unused_variables, reason = "only a trace build reports")
    )]
    fn report(&self, kind: StationRxKind, count: u32) {
        if count != 0 {
            oer_portable_trace::observe!(self.0, StationRx { kind, count });
        }
    }

    #[cfg_attr(
        not(oer_trace),
        allow(unused_variables, reason = "only a trace build reports")
    )]
    fn report_tx(&self, settled: StationTxAmpdu) {
        oer_portable_trace::observe!(self.0, settled);
    }
}

/// `at`'s low 32 bits of microseconds.
#[cfg_attr(
    not(oer_trace),
    allow(dead_code, reason = "only a trace build reports")
)]
fn wrapped_micros(at: oer_portable_time::Instant) -> u32 {
    let [a, b, c, d, ..] = at.as_micros().to_le_bytes();
    u32::from_le_bytes([a, b, c, d])
}

/// The receive decisions of one service event, each to `emit`.
fn service_rx(
    event: oer_protocols_ieee80211_runtime::station::ServiceEvent,
    emit: &mut impl FnMut(StationRxKind, u32),
) {
    use oer_protocols_ieee80211_runtime::station::ServiceEvent;
    match event {
        ServiceEvent::RxFrameDropped(drop) => emit(dropped(drop), 1),
        ServiceEvent::RxMsdusRefused(refused) => {
            emit(StationRxKind::RefusedTooLong, refused.too_long.into());
            emit(StationRxKind::RefusedQueueFull, refused.queue_full.into());
            emit(StationRxKind::RefusedPressure, refused.pressure.into());
            emit(StationRxKind::RefusedNoNetwork, refused.no_network.into());
        }
        ServiceEvent::RxMsdusCopied(copied) => {
            emit(StationRxKind::CopiedAggregated, copied.aggregated.into());
            emit(StationRxKind::CopiedKept, copied.kept.into());
            emit(StationRxKind::CopiedShared, copied.shared.into());
        }
        ServiceEvent::GroupKeyInstalled { .. } => {}
    }
}

/// The receive decisions of one receive agreement event, each to `emit`.
fn block_ack_rx(
    event: oer_protocols_ieee80211_core_upper_mac::block_ack::RxBlockAckEvent,
    emit: &mut impl FnMut(StationRxKind, u32),
) {
    use oer_protocols_ieee80211_contracts_port::RxEvidence;
    use oer_protocols_ieee80211_core_upper_mac::block_ack::{ReorderCause, RxBlockAckEvent};
    match event {
        RxBlockAckEvent::Contained { ampdu, .. } => emit(
            match ampdu {
                RxEvidence::HardwareObserved(true) | RxEvidence::ProtocolValidated(true) => {
                    StationRxKind::MpduInAmpdu
                }
                RxEvidence::HardwareObserved(false) | RxEvidence::ProtocolValidated(false) => {
                    StationRxKind::MpduSingle
                }
                RxEvidence::Unavailable => StationRxKind::MpduUnknown,
            },
            1,
        ),
        RxBlockAckEvent::Reordered {
            cause,
            released,
            missing,
            ..
        } => {
            let released = u32::from(released);
            let ((kind, count), missing_kind) = match cause {
                ReorderCause::Receive => (
                    (StationRxKind::ReleasedInOrder, released),
                    StationRxKind::MissingOther,
                ),
                ReorderCause::Timeout => (
                    (StationRxKind::ReleasedByTimeout, released),
                    StationRxKind::MissingByTimeout,
                ),
                ReorderCause::Bar => (
                    (StationRxKind::ReleasedByBar, released),
                    StationRxKind::MissingOther,
                ),
                ReorderCause::Stop | ReorderCause::Clear => (
                    (StationRxKind::ReleasedByStop, released),
                    StationRxKind::MissingOther,
                ),
                // A rejected decision is one MPDU, whatever it released.
                ReorderCause::Duplicate => {
                    ((StationRxKind::Duplicate, 1), StationRxKind::MissingOther)
                }
                ReorderCause::Rejected => {
                    ((StationRxKind::Rejected, 1), StationRxKind::MissingOther)
                }
            };
            emit(kind, count);
            emit(missing_kind, missing.into());
        }
        RxBlockAckEvent::Started { window, .. } => {
            emit(StationRxKind::AgreementStarted, window.into());
        }
        RxBlockAckEvent::Stopped { .. } => emit(StationRxKind::AgreementStopped, 1),
        // A request is a [`StationAddba`], not a receive decision.
        RxBlockAckEvent::Requested { .. } => {}
    }
}

/// The kind a station receive drop counts as.
const fn dropped(drop: oer_protocols_ieee80211_runtime::station::RxDrop) -> StationRxKind {
    use oer_protocols_ieee80211_core_roles_sta::machine::StaRxError;
    use oer_protocols_ieee80211_core_upper_mac::receive::RxDropReason;
    use oer_protocols_ieee80211_runtime::station::RxDrop;
    match drop {
        RxDrop::Station(StaRxError::Frame(reason)) => match reason {
            RxDropReason::Header => StationRxKind::DroppedHeader,
            RxDropReason::FrameKind => StationRxKind::DroppedFrameKind,
            RxDropReason::Address => StationRxKind::DroppedAddress,
            RxDropReason::Fragmented => StationRxKind::DroppedFragmented,
            RxDropReason::Protection => StationRxKind::DroppedProtection,
            RxDropReason::Integrity => StationRxKind::DroppedIntegrity,
            RxDropReason::Replay => StationRxKind::DroppedReplay,
            RxDropReason::Payload => StationRxKind::DroppedPayload,
            RxDropReason::NotConnected => StationRxKind::DroppedNotConnected,
        },
        RxDrop::Station(StaRxError::Security(_)) => StationRxKind::DroppedSecurity,
        RxDrop::Draining => StationRxKind::DroppedDraining,
    }
}

impl<O: oer_portable_trace::Observer>
    oer_portable_events::EventSink<oer_protocols_ieee80211_runtime::station::ServiceEvent>
    for TracedStationEvents<O>
{
    const ENABLED: bool = cfg!(oer_trace);

    fn event(&mut self, event: oer_protocols_ieee80211_runtime::station::ServiceEvent) {
        service_rx(event, &mut |kind, count| self.report(kind, count));
    }
}

impl<O: oer_portable_trace::Observer>
    oer_portable_events::EventSink<
        oer_protocols_ieee80211_core_upper_mac::block_ack::RxBlockAckEvent,
    > for TracedStationEvents<O>
{
    const ENABLED: bool = cfg!(oer_trace);

    fn event(&mut self, event: oer_protocols_ieee80211_core_upper_mac::block_ack::RxBlockAckEvent) {
        use oer_protocols_ieee80211_core_upper_mac::block_ack::RxBlockAckEvent;
        if let RxBlockAckEvent::Requested {
            tid,
            requested,
            outcome,
        } = event
        {
            oer_portable_trace::observe!(self.0, StationAddba::of(tid, requested, outcome));
        }
        block_ack_rx(event, &mut |kind, count| self.report(kind, count));
    }
}

impl<O: oer_portable_trace::Observer>
    oer_portable_events::EventSink<oer_protocols_ieee80211_core_roles_sta::events::StaEvent>
    for TracedStationEvents<O>
{
    const ENABLED: bool = cfg!(oer_trace);

    fn event(&mut self, event: oer_protocols_ieee80211_core_roles_sta::events::StaEvent) {
        use oer_protocols_ieee80211_core_roles_sta::events::StaEvent;
        match event {
            StaEvent::RsnInputDropped(_) => self.report(StationRxKind::RsnInputDropped, 1),
            StaEvent::AggregateSettled(settled) => {
                if let Some(holes) = settled.holes {
                    oer_portable_trace::observe!(
                        self.0,
                        StationTxHoles {
                            members: settled.members,
                            holes,
                        }
                    );
                }
                self.report_tx(StationTxAmpdu {
                    members: settled.members,
                    acknowledged: settled.acknowledged,
                    retried: settled.retried,
                    dropped: settled.dropped,
                    refused: settled.refused,
                    block_ack_request: false,
                    rate: RateClass::of(settled.rate),
                });
            }
            StaEvent::BlockAckRequestSent => self.report_tx(StationTxAmpdu {
                block_ack_request: true,
                ..StationTxAmpdu::default()
            }),
            StaEvent::ConnectStep { at, step, channel } => {
                oer_portable_trace::observe!(
                    self.0,
                    StationConnectStep {
                        at_micros: wrapped_micros(at),
                        step,
                        channel: channel.unwrap_or(0),
                    }
                );
            }
            StaEvent::ScanProbeBuilt { at } => {
                oer_portable_trace::observe!(
                    self.0,
                    StationScanProbe {
                        at_micros: wrapped_micros(at),
                        step: ScanProbeStep::Built,
                    }
                );
            }
            StaEvent::ScanProbeEnded { at, end } => {
                oer_portable_trace::observe!(
                    self.0,
                    StationScanProbe {
                        at_micros: wrapped_micros(at),
                        step: match end {
                            ScanProbeEnd::Transmitted => ScanProbeStep::Transmitted,
                            ScanProbeEnd::NotTransmitted => ScanProbeStep::NotTransmitted,
                            ScanProbeEnd::Unknown => ScanProbeStep::Unknown,
                        },
                    }
                );
            }
            StaEvent::ScanHeard { at, heard } => {
                oer_portable_trace::observe!(
                    self.0,
                    StationScanHeard {
                        at_micros: wrapped_micros(at),
                        heard,
                    }
                );
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transmit_settlement_round_trips_and_refuses_unknown_flags() {
        let settled = StationTxAmpdu {
            members: 32,
            acknowledged: 29,
            retried: 2,
            dropped: 1,
            refused: false,
            block_ack_request: false,
            rate: RateClass::Ht {
                mcs: 7,
                width40: true,
                short_gi: true,
            },
        };
        assert_eq!(StationTxAmpdu::decode(settled.encode()), Some(settled));
        let bar = StationTxAmpdu {
            block_ack_request: true,
            ..StationTxAmpdu::default()
        };
        assert_eq!(StationTxAmpdu::decode(bar.encode()), Some(bar));
        // A word above the flags that names no rate is refused.
        assert_eq!(StationTxAmpdu::decode([0, 5 << 2]), None);
    }

    #[test]
    fn a_request_record_round_trips_every_outcome_and_refuses_unknown_words() {
        use oer_protocols_ieee80211_core_upper_mac::block_ack::{
            AddbaOutcome as Recipient, AddbaStatus,
        };
        for (requested, recipient, outcome) in [
            (
                0,
                Recipient::Accepted { window: 16 },
                AddbaOutcome::Accepted { window: 16 },
            ),
            (
                64,
                Recipient::Repeated { window: 1 },
                AddbaOutcome::Repeated { window: 1 },
            ),
            (
                1023,
                Recipient::Refused(AddbaStatus::InvalidParameters),
                AddbaOutcome::Refused {
                    status: oer_protocols_ieee80211_wire::status::status_code::INVALID_PARAMETERS,
                },
            ),
            (8, Recipient::Unanswered, AddbaOutcome::Unanswered),
        ] {
            let record = StationAddba::of(15, requested, recipient);
            assert_eq!(
                record,
                StationAddba {
                    tid: 15,
                    requested,
                    outcome
                }
            );
            assert_eq!(StationAddba::decode(record.encode()), Some(record));
        }
        assert_eq!(StationAddba::decode([4 << 8, 0]), None);
        assert_eq!(StationAddba::decode([3 << 8, 1]), None);
        assert_eq!(StationAddba::decode([0, 1 << 16]), None);
    }

    #[test]
    fn every_facade_event_has_its_own_kind() {
        let kinds = [
            StationAuthenticated::KIND,
            StationBeacon::KIND,
            StationAssociated::KIND,
            TxIndeterminate::KIND,
            TxHardwareTimeout::KIND,
            TxQueueCollision::KIND,
            ControlDelivered::KIND,
            ActorSpan::KIND,
            StationRx::KIND,
            RxData::KIND,
            TxData::KIND,
            StationTxAmpdu::KIND,
            StationTxHoles::KIND,
            StationAddba::KIND,
            TxRateSelected::KIND,
        ]
        .map(Kind::raw);
        for (at, kind) in kinds.iter().enumerate() {
            assert!(!kinds[at + 1..].contains(kind), "kind {kind} repeats");
        }
    }

    #[test]
    fn receive_events_map_to_their_decisions_and_counts() {
        use oer_protocols_ieee80211_contracts_port::RxEvidence;
        use oer_protocols_ieee80211_core_upper_mac::block_ack::{
            AgreementStop, ReorderCause, RxBlockAckEvent,
        };
        use oer_protocols_ieee80211_core_upper_mac::receive::RxDropReason;
        use oer_protocols_ieee80211_runtime::station::{
            RxDrop, RxMsdusCopied, RxMsdusRefused, ServiceEvent,
        };
        let mut seen = heapless::Vec::<(StationRxKind, u32), 16>::new();
        let mut emit = |kind, count| seen.push((kind, count)).unwrap();
        service_rx(
            ServiceEvent::RxFrameDropped(RxDrop::Station(RxDropReason::Replay.into())),
            &mut emit,
        );
        service_rx(
            ServiceEvent::RxMsdusRefused(RxMsdusRefused {
                queue_full: 3,
                pressure: 2,
                ..RxMsdusRefused::default()
            }),
            &mut emit,
        );
        service_rx(
            ServiceEvent::RxMsdusCopied(RxMsdusCopied {
                aggregated: 2,
                kept: 1,
                shared: 4,
            }),
            &mut emit,
        );
        block_ack_rx(
            RxBlockAckEvent::Contained {
                ampdu: RxEvidence::HardwareObserved(true),
                s_mpdu: RxEvidence::Unavailable,
                bytes: Some(1500),
            },
            &mut emit,
        );
        block_ack_rx(
            RxBlockAckEvent::Reordered {
                tid: 0,
                cause: ReorderCause::Timeout,
                held: 0,
                released: 5,
                missing: 2,
            },
            &mut emit,
        );
        block_ack_rx(
            RxBlockAckEvent::Reordered {
                tid: 0,
                cause: ReorderCause::Duplicate,
                held: 4,
                released: 0,
                missing: 0,
            },
            &mut emit,
        );
        block_ack_rx(RxBlockAckEvent::Started { tid: 0, window: 16 }, &mut emit);
        block_ack_rx(
            RxBlockAckEvent::Stopped {
                tid: 0,
                reason: AgreementStop::Peer,
            },
            &mut emit,
        );
        // A request is no receive decision.
        block_ack_rx(
            RxBlockAckEvent::Requested {
                tid: 0,
                requested: 0,
                outcome:
                    oer_protocols_ieee80211_core_upper_mac::block_ack::AddbaOutcome::Unanswered,
            },
            &mut emit,
        );
        use StationRxKind::*;
        assert_eq!(
            seen.as_slice(),
            [
                (DroppedReplay, 1),
                (RefusedTooLong, 0),
                (RefusedQueueFull, 3),
                (RefusedPressure, 2),
                (RefusedNoNetwork, 0),
                (CopiedAggregated, 2),
                (CopiedKept, 1),
                (CopiedShared, 4),
                (MpduInAmpdu, 1),
                (ReleasedByTimeout, 5),
                (MissingByTimeout, 2),
                (Duplicate, 1),
                (MissingOther, 0),
                (AgreementStarted, 16),
                (AgreementStopped, 1),
            ]
        );
    }

    #[test]
    fn a_rate_and_a_transmission_end_class_as_the_histograms_count_them() {
        use oer_protocols_ieee80211_contracts_port::{
            Air, IndeterminateCause, SingleEnd, SingleOutcome, TxOutcome,
        };
        use oer_protocols_ieee80211_wire::phy::{
            HtMcs, HtRate, LegacyRate, PhyRate, PpduBandwidth,
        };
        let ht40 =
            PhyRate::Ht(HtRate::new(HtMcs::new(7).unwrap(), PpduBandwidth::Mhz40, true).unwrap());
        assert_eq!(
            RateClass::of(Some(ht40)),
            RateClass::Ht {
                mcs: 7,
                width40: true,
                short_gi: true
            }
        );
        assert_eq!(
            RateClass::of(Some(PhyRate::Legacy(LegacyRate::Ofdm54M))),
            RateClass::NonHt
        );
        assert_eq!(RateClass::of(None), RateClass::Unknown);
        let air = Air {
            transmissions: core::num::NonZeroU16::new(3).unwrap(),
            rate: ht40,
        };
        let timeout: SingleOutcome<(), ()> = TxOutcome::Ended(SingleEnd::AckTimeout(air));
        assert_eq!(
            TxData::of(&timeout),
            TxData {
                rate: RateClass::of(Some(ht40)),
                transmissions: 3,
                end: TxEnd::AckTimeout,
            }
        );
        let denied: SingleOutcome<(), ()> = TxOutcome::Ended(SingleEnd::AirDenied {
            denial: (),
            air: None,
        });
        assert_eq!(
            TxData::of(&denied),
            TxData {
                rate: RateClass::Unknown,
                transmissions: 0,
                end: TxEnd::Other,
            }
        );
        let unknown: SingleOutcome<(), ()> =
            TxOutcome::Indeterminate(IndeterminateCause::HardwareTimeout);
        assert_eq!(TxData::of(&unknown).end, TxEnd::Unsent);
        assert_eq!(
            Containment::of(
                oer_protocols_ieee80211_contracts_port::RxEvidence::ProtocolValidated(true)
            ),
            Containment::InAmpdu
        );
    }

    fn round_trip<E: Event + PartialEq + core::fmt::Debug>(event: E) {
        assert_eq!(E::decode(event.encode()), Some(event));
    }

    #[test]
    fn every_facade_event_decodes_to_itself() {
        round_trip(StationAuthenticated);
        round_trip(StationBeacon { at_millis: 7 });
        for step in ConnectStep::ALL {
            round_trip(StationConnectStep {
                at_micros: 9,
                step,
                channel: 13,
            });
        }
        for step in [
            ScanProbeStep::Built,
            ScanProbeStep::Transmitted,
            ScanProbeStep::NotTransmitted,
            ScanProbeStep::Unknown,
        ] {
            round_trip(StationScanProbe {
                at_micros: 13,
                step,
            });
        }
        for rssi in [None, Some(-61), Some(0)] {
            round_trip(StationScanHeard {
                at_micros: 11,
                heard: ScanHeard {
                    own: true,
                    probe_response: false,
                    to_us: true,
                    rssi,
                },
            });
        }
        round_trip(StationAssociated);
        round_trip(TxIndeterminate);
        round_trip(TxHardwareTimeout);
        round_trip(TxQueueCollision);
        round_trip(ControlDelivered);
        round_trip(TxRateSelected { kbps: None });
        round_trip(TxRateSelected { kbps: Some(65_000) });
        assert_eq!(StationBeacon::decode([7, 1]), None);
        assert_eq!(TxRateSelected::decode([1, 0]), None);
        for span in Span::ALL {
            round_trip(ActorSpan { span, end: false });
            round_trip(ActorSpan { span, end: true });
        }
        assert_eq!(ActorSpan::decode([0, 2]), None, "an unknown edge");
        assert_eq!(
            ActorSpan::decode([Span::ALL.len() as u32, 0]),
            None,
            "an unknown span"
        );
        for kind in StationRxKind::ALL {
            round_trip(StationRx { kind, count: 9 });
        }
        let rates = [
            RateClass::Unknown,
            RateClass::NonHt,
            RateClass::He,
            RateClass::Ht {
                mcs: 7,
                width40: true,
                short_gi: true,
            },
            RateClass::Ht {
                mcs: 31,
                width40: false,
                short_gi: false,
            },
        ];
        for rate in rates {
            for containment in [
                Containment::Unknown,
                Containment::Single,
                Containment::InAmpdu,
            ] {
                round_trip(RxData { rate, containment });
            }
            for end in TxEnd::ALL {
                round_trip(TxData {
                    rate,
                    transmissions: u16::MAX,
                    end,
                });
            }
        }
        assert_eq!(RxData::decode([4, 0]), None, "an unknown format");
        assert_eq!(RxData::decode([2 | (32 << 8), 0]), None, "MCS 32");
        assert_eq!(RxData::decode([2 | (1 << 18), 0]), None, "an unknown bit");
        assert_eq!(RxData::decode([1, 3]), None);
        assert_eq!(TxData::decode([1, 4 << 16]), None);
        assert_eq!(
            StationRx::decode([StationRxKind::ALL.len() as u32, 1]),
            None
        );
    }
}

/// The shared RF actor's periodic tracking ticks: the observer the image
/// gives [`crate::RadioResources::start`] receives them.
#[cfg(target_arch = "riscv32")]
pub use oer_chips_esp32s31_radio_system::observation::{TickRan, TrackingBranches, TrackingTick};
