//! The facade's own trace events: what the composed station saw at the port
//! and in its rate selection. They are the facade's vocabulary on the image's
//! trace observer; the protocol's events reach the image's event sink
//! instead ([`crate::Binding::Sink`]).
//!
//! Each kind keeps its id in the IEEE 802.11 trace domain (the S31 driver's
//! lower-MAC trace uses ids from 100) and its channel.

use oer_trace::{Channel, Domain, Event, Kind};

/// The station activity channel.
const SERVICE: Channel = const {
    match Channel::new(Domain::Ieee80211, 1) {
        Ok(channel) => channel,
        Err(_) => panic!("the station activity channel lies in the IEEE 802.11 range"),
    }
};
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

/// The station service's spans, which [`ServiceSpan`] reports.
pub use oer_ieee80211_runtime::station::ServiceSpan as Span;

/// The start or end of a stretch of the station service's work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServiceSpan {
    pub span: oer_ieee80211_runtime::station::ServiceSpan,
    pub end: bool,
}

impl Event for ServiceSpan {
    const KIND: Kind = Kind::new(Domain::Ieee80211, 29);
    const CHANNEL: Channel = SERVICE;

    fn encode(&self) -> [u32; 2] {
        [u32::from(self.span as u8), u32::from(self.end)]
    }

    fn decode([span, end]: [u32; 2]) -> Option<Self> {
        Some(Self {
            span: oer_ieee80211_runtime::station::ServiceSpan::from_number(
                u8::try_from(span).ok()?,
            )?,
            end: match end {
                0 => false,
                1 => true,
                _ => return None,
            },
        })
    }
}

impl core::fmt::Display for ServiceSpan {
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
    /// The port reported lost events.
    PortEventsLost,
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
    /// MSDUs copied instead of lent, by reason.
    CopiedAggregated,
    CopiedKept,
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
}

impl StationRxKind {
    pub const ALL: [Self; 31] = [
        Self::PortEventsLost,
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
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
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
}

impl Event for StationTxAmpdu {
    const KIND: Kind = Kind::new(Domain::Ieee80211, 36);
    const CHANNEL: Channel = SERVICE;

    fn encode(&self) -> [u32; 2] {
        [
            u32::from_le_bytes([self.members, self.acknowledged, self.retried, self.dropped]),
            u32::from(self.refused) | (u32::from(self.block_ack_request) << 1),
        ]
    }

    fn decode([counts, flags]: [u32; 2]) -> Option<Self> {
        if flags > 0b11 {
            return None;
        }
        let [members, acknowledged, retried, dropped] = counts.to_le_bytes();
        Some(Self {
            members,
            acknowledged,
            retried,
            dropped,
            refused: flags & 1 != 0,
            block_ack_request: flags & 0b10 != 0,
        })
    }
}

impl core::fmt::Display for StationTxAmpdu {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.block_ack_request {
            return write!(f, "station.tx bar");
        }
        write!(
            f,
            "station.tx ampdu {} acked {} retried {} dropped {}{}",
            self.members,
            self.acknowledged,
            self.retried,
            self.dropped,
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

impl<O: oer_trace::Observer> TracedStationEvents<O> {
    #[cfg_attr(
        not(oer_trace),
        allow(unused_variables, reason = "only a trace build reports")
    )]
    fn report(&self, kind: StationRxKind, count: u32) {
        if count != 0 {
            oer_trace::observe!(self.0, StationRx { kind, count });
        }
    }

    #[cfg_attr(
        not(oer_trace),
        allow(unused_variables, reason = "only a trace build reports")
    )]
    fn report_tx(&self, settled: StationTxAmpdu) {
        oer_trace::observe!(self.0, settled);
    }
}

/// The receive decisions of one service event, each to `emit`.
fn service_rx(
    event: oer_ieee80211_runtime::station::ServiceEvent,
    emit: &mut impl FnMut(StationRxKind, u32),
) {
    use oer_ieee80211_runtime::station::ServiceEvent;
    match event {
        ServiceEvent::PortEventsLost => emit(StationRxKind::PortEventsLost, 1),
        ServiceEvent::RxFrameDropped(drop) => emit(dropped(drop), 1),
        ServiceEvent::RxMsdusRefused(refused) => {
            emit(StationRxKind::RefusedTooLong, refused.too_long.into());
            emit(StationRxKind::RefusedQueueFull, refused.queue_full.into());
            emit(StationRxKind::RefusedNoNetwork, refused.no_network.into());
        }
        ServiceEvent::RxMsdusCopied(copied) => {
            emit(StationRxKind::CopiedAggregated, copied.aggregated.into());
            emit(StationRxKind::CopiedKept, copied.kept.into());
        }
        ServiceEvent::GroupKeyInstalled { .. } => {}
    }
}

/// The receive decisions of one receive agreement event, each to `emit`.
fn block_ack_rx(
    event: oer_ieee80211_core_upper_mac::block_ack::RxBlockAckEvent,
    emit: &mut impl FnMut(StationRxKind, u32),
) {
    use oer_ieee80211_contracts_port::RxEvidence;
    use oer_ieee80211_core_upper_mac::block_ack::{ReorderCause, RxBlockAckEvent};
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
    }
}

/// The kind a station receive drop counts as.
const fn dropped(drop: oer_ieee80211_runtime::station::RxDrop) -> StationRxKind {
    use oer_ieee80211_core_roles_sta::machine::StaRxError;
    use oer_ieee80211_core_upper_mac::receive::RxDropReason;
    use oer_ieee80211_runtime::station::RxDrop;
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

impl<O: oer_trace::Observer> oer_events::EventSink<oer_ieee80211_runtime::station::ServiceEvent>
    for TracedStationEvents<O>
{
    const ENABLED: bool = cfg!(oer_trace);

    fn event(&mut self, event: oer_ieee80211_runtime::station::ServiceEvent) {
        service_rx(event, &mut |kind, count| self.report(kind, count));
    }
}

impl<O: oer_trace::Observer>
    oer_events::EventSink<oer_ieee80211_core_upper_mac::block_ack::RxBlockAckEvent>
    for TracedStationEvents<O>
{
    const ENABLED: bool = cfg!(oer_trace);

    fn event(&mut self, event: oer_ieee80211_core_upper_mac::block_ack::RxBlockAckEvent) {
        block_ack_rx(event, &mut |kind, count| self.report(kind, count));
    }
}

impl<O: oer_trace::Observer> oer_events::EventSink<oer_ieee80211_core_roles_sta::events::StaEvent>
    for TracedStationEvents<O>
{
    const ENABLED: bool = cfg!(oer_trace);

    fn event(&mut self, event: oer_ieee80211_core_roles_sta::events::StaEvent) {
        use oer_ieee80211_core_roles_sta::events::StaEvent;
        match event {
            StaEvent::RsnInputDropped(_) => self.report(StationRxKind::RsnInputDropped, 1),
            StaEvent::AggregateSettled(settled) => self.report_tx(StationTxAmpdu {
                members: settled.members,
                acknowledged: settled.acknowledged,
                retried: settled.retried,
                dropped: settled.dropped,
                refused: settled.refused,
                block_ack_request: false,
            }),
            StaEvent::BlockAckRequestSent => self.report_tx(StationTxAmpdu {
                block_ack_request: true,
                ..StationTxAmpdu::default()
            }),
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
        };
        assert_eq!(StationTxAmpdu::decode(settled.encode()), Some(settled));
        let bar = StationTxAmpdu {
            block_ack_request: true,
            ..StationTxAmpdu::default()
        };
        assert_eq!(StationTxAmpdu::decode(bar.encode()), Some(bar));
        assert_eq!(StationTxAmpdu::decode([0, 4]), None);
    }

    #[test]
    fn receive_events_map_to_their_decisions_and_counts() {
        use oer_ieee80211_contracts_port::RxEvidence;
        use oer_ieee80211_core_upper_mac::block_ack::{
            AgreementStop, ReorderCause, RxBlockAckEvent,
        };
        use oer_ieee80211_core_upper_mac::receive::RxDropReason;
        use oer_ieee80211_runtime::station::{RxDrop, RxMsdusCopied, RxMsdusRefused, ServiceEvent};
        let mut seen = heapless::Vec::<(StationRxKind, u32), 16>::new();
        let mut emit = |kind, count| seen.push((kind, count)).unwrap();
        service_rx(
            ServiceEvent::RxFrameDropped(RxDrop::Station(RxDropReason::Replay.into())),
            &mut emit,
        );
        service_rx(
            ServiceEvent::RxMsdusRefused(RxMsdusRefused {
                queue_full: 3,
                ..RxMsdusRefused::default()
            }),
            &mut emit,
        );
        service_rx(
            ServiceEvent::RxMsdusCopied(RxMsdusCopied {
                aggregated: 2,
                kept: 1,
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
        use StationRxKind::*;
        assert_eq!(
            seen.as_slice(),
            [
                (DroppedReplay, 1),
                (RefusedTooLong, 0),
                (RefusedQueueFull, 3),
                (RefusedNoNetwork, 0),
                (CopiedAggregated, 2),
                (CopiedKept, 1),
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

    fn round_trip<E: Event + PartialEq + core::fmt::Debug>(event: E) {
        assert_eq!(E::decode(event.encode()), Some(event));
    }

    #[test]
    fn every_facade_event_decodes_to_itself() {
        round_trip(StationAuthenticated);
        round_trip(StationBeacon { at_millis: 7 });
        round_trip(StationAssociated);
        round_trip(TxIndeterminate);
        round_trip(TxHardwareTimeout);
        round_trip(TxQueueCollision);
        round_trip(ControlDelivered);
        round_trip(TxRateSelected { kbps: None });
        round_trip(TxRateSelected { kbps: Some(65_000) });
        for span in oer_ieee80211_runtime::station::ServiceSpan::ALL {
            round_trip(ServiceSpan { span, end: false });
            round_trip(ServiceSpan { span, end: true });
        }
        assert_eq!(ServiceSpan::decode([0, 2]), None);
        assert_eq!(ServiceSpan::decode([9, 0]), None);
        assert_eq!(StationBeacon::decode([7, 1]), None);
        assert_eq!(TxRateSelected::decode([1, 0]), None);
        for kind in StationRxKind::ALL {
            round_trip(StationRx { kind, count: 9 });
        }
        assert_eq!(
            StationRx::decode([StationRxKind::ALL.len() as u32, 1]),
            None
        );
    }
}
