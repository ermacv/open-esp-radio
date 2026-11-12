//! What the station's tasks decided without delivering it, and the sink a
//! composition lends them for their cores' events too.
//!
//! The lifecycle and the datapath actor each hold a composition's sink and
//! lend it, for one call at a time, to the station, the data plane and the
//! receive agreements. Neither keeps a count of these events; `()` keeps
//! none either. A refusal returned to the application
//! (`ServiceOutput::MsduRefused`, `ServiceOutput::TxRefused`) is the
//! application's to count and is no event.

use oer_portable_events::EventSink;
use oer_protocols_ieee80211_core_roles_sta::events::StaEvent;
use oer_protocols_ieee80211_core_roles_sta::machine::StaRxError;
use oer_protocols_ieee80211_core_upper_mac::block_ack::RxBlockAckEvent;

/// One decision of the station's tasks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceEvent {
    /// A received frame was dropped.
    RxFrameDropped(RxDrop),
    /// Some MSDUs of one received frame were not delivered: one event per
    /// frame, after the station's own events of that frame.
    RxMsdusRefused(RxMsdusRefused),
    /// Some MSDUs of one received frame were copied instead of lending its
    /// buffer: one event per frame, beside its refusals.
    RxMsdusCopied(super::RxMsdusCopied),
    /// The network's receive end of the queue ended with frames in it, as
    /// a broken one does: the actor took them back, each once, their
    /// buffers to the ring.
    RxReclaimed { frames: u16 },
    /// A group key was installed: the station's first or a replacement.
    /// `key_id` is 0..=3.
    GroupKeyInstalled { key_id: u8, replaced: bool },
}

/// Why a received frame was dropped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxDrop {
    /// Receive admission or the authenticated RSN protocol refused it.
    Station(StaRxError),
    /// It arrived while the radio stopped: no receive admission ran.
    Draining,
}

/// The MSDUs of one received frame that were not delivered, by reason.
/// Each count saturates at `u16::MAX`; at least one is nonzero.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxMsdusRefused {
    /// MSDUs that exceeded the receive queue's frame length.
    pub too_long: u16,
    /// MSDUs the full receive queue to the application did not take.
    pub queue_full: u16,
    /// MSDUs of a frame whose buffer the port could not lend, its network
    /// holding all the port may lend past the pressure bound
    /// ([`NETWORK_LATE`](crate::datapath::NETWORK_LATE)): pressure on the
    /// station's handoff, before any socket is known. The frame was
    /// admitted and reordered; its buffer went back to the ring.
    pub pressure: u16,
    /// MSDUs of a composition without a network.
    pub no_network: u16,
}

/// The sink a composition lends the station's tasks: it takes their events,
/// the station's and the receive agreements'. `()` takes them all
/// and keeps nothing.
pub trait StationEvents:
    EventSink<ServiceEvent> + EventSink<StaEvent> + EventSink<RxBlockAckEvent>
{
}

impl<S: EventSink<ServiceEvent> + EventSink<StaEvent> + EventSink<RxBlockAckEvent> + ?Sized>
    StationEvents for S
{
}
