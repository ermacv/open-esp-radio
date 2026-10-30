//! What the station service decided without delivering it, and the sink a
//! composition lends the service for its cores' events too.
//!
//! The service holds the composition's sink `O` and lends it, for one call
//! at a time, to the station, its data machine and its receive agreements.
//! It keeps no count of these events; `()` keeps none either.

use oer_events::EventSink;
use oer_ieee80211_sta::events::StaEvent;
use oer_ieee80211_sta::machine::StaRxError;
use oer_ieee80211_upper_mac::block_ack::RxBlockAckEvent;

/// One decision of the station service.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceEvent {
    /// The port reported that it lost events; the indication carries no
    /// frame count.
    PortEventsLost,
    /// The service dropped one received frame.
    RxFrameDropped(RxDrop),
    /// The service did not deliver one received MSDU.
    RxMsduRefused(RxMsduRefusal),
    /// A station frame left the service unsent; the application heard its
    /// refusal.
    TxUnsent,
    /// An application MSDU exceeded the data machine's frame length and came
    /// back to the application.
    TxMsduTooLong,
    /// The service installed a group key: the station's first or a
    /// replacement. `key_id` is 0..=3.
    GroupKeyInstalled { key_id: u8, replaced: bool },
}

/// Why the service dropped a received frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxDrop {
    /// Receive admission or the authenticated RSN protocol refused it.
    Station(StaRxError),
    /// It arrived while the service drained its poisoned port: no receive
    /// admission ran.
    Draining,
}

/// Why the service did not deliver a received MSDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxMsduRefusal {
    /// It exceeded the receive queue's frame length.
    TooLong,
    /// The receive queue to the application was full.
    QueueFull,
}

/// The sink a composition lends a station service: it takes the events of
/// the service, the station and its receive agreements. `()` takes them all
/// and keeps nothing.
pub trait StationEvents:
    EventSink<ServiceEvent> + EventSink<StaEvent> + EventSink<RxBlockAckEvent>
{
}

impl<S: EventSink<ServiceEvent> + EventSink<StaEvent> + EventSink<RxBlockAckEvent> + ?Sized>
    StationEvents for S
{
}
