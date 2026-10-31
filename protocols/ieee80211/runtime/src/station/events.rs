//! What the station service decided without delivering it, and the sink a
//! composition lends the service for its cores' events too.
//!
//! The service holds the composition's sink `O` and lends it, for one call
//! at a time, to the station, its data machine and its receive agreements.
//! It keeps no count of these events; `()` keeps none either. A refusal
//! the service returns to the application (`ServiceOutput::MsduRefused`,
//! `ServiceOutput::TxRefused`) is the application's to count and is no event.

use oer_events::EventSink;
use oer_ieee80211_core_roles_sta::events::StaEvent;
use oer_ieee80211_core_roles_sta::machine::StaRxError;
use oer_ieee80211_core_upper_mac::block_ack::RxBlockAckEvent;

/// One decision of the station service.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceEvent {
    /// The port reported that it lost events; the indication carries no
    /// frame count.
    PortEventsLost,
    /// The service dropped one received frame.
    RxFrameDropped(RxDrop),
    /// The service did not deliver some MSDUs of one received frame: one
    /// event per frame, after the station's own events of that frame.
    RxMsdusRefused(RxMsdusRefused),
    /// The service copied some MSDUs of one received frame instead of
    /// lending its buffer: one event per frame, beside its refusals.
    RxMsdusCopied(super::RxMsdusCopied),
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

/// The MSDUs of one received frame the service did not deliver, by reason.
/// Each count saturates at `u16::MAX`; at least one is nonzero.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxMsdusRefused {
    /// MSDUs that exceeded the receive queue's frame length.
    pub too_long: u16,
    /// MSDUs the full receive queue to the application did not take.
    pub queue_full: u16,
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
