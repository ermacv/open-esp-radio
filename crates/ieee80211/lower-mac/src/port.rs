//! The portable view of a lower-MAC event.

use oer_ieee80211_port::LifecycleEvent;

use crate::{
    control::VifId,
    rx::RxMeta,
    tx::{TxAttempt, TxCompletion, TxPayload},
};

/// The portable view of one owned event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LowerMacEvent<'a> {
    /// One received MPDU, from its header to the end of its body, without
    /// the FCS.
    Received { frame: &'a [u8], meta: RxMeta },
    /// The terminal event of one attempt. Its buffer is released; the event
    /// owns the attempt's bodies, which
    /// [`LowerMacEvents::into_completed`](crate::LowerMacEvents::into_completed) takes out.
    TxCompleted(TxCompletion),
    /// The terminal event of a lifecycle command.
    Lifecycle(LifecycleEvent),
    /// An event of an extension trait: what it is and which interface it
    /// concerns, so a router hands it to that interface's client without
    /// knowing the extension. Its content is read through the extension's
    /// view.
    Extension { kind: ExtensionKind, vif: VifId },
}

/// The kind of an [`LowerMacEvent::Extension`] event, one per extension
/// event the port's extension traits define.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ExtensionKind {
    /// A target beacon transmission time of the interface's schedule
    Tbtt,
}

/// A single attempt: one MPDU, its header in a backend
/// buffer `B` and its body, if any, the owner `O`.
pub type MpduAttempt<B, O> = TxAttempt<TxPayload<B, O>>;
