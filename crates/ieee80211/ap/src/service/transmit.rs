//! An authenticator publication retains the association and response window
//! which created it. RX may advance the handshake before its TX terminal:
//! the old completion must not arm the new message's retry window.

use super::*;

/// One prepared AP EAPOL transmission and its affine terminal context.
///
/// The context is created with the frame, before the caller publishes it. A
/// port consumer moves it into the admitted attempt alongside its ticket and
/// complete MPDU; it never reconstructs it from the current peer-table slot.
#[must_use = "publish the frame with its context or return it as unsent"]
#[derive(Debug)]
pub struct ApEapolTransmit<const N: usize> {
    frame: RsnTxFrame<N>,
    context: ApEapolTxContext,
}

impl<const N: usize> ApEapolTransmit<N> {
    /// The exact EAPOL bytes and retry classification to encode into an MPDU.
    pub const fn frame(&self) -> &RsnTxFrame<N> {
        &self.frame
    }

    /// Transfer the terminal context with the encoded frame's admission.
    pub fn into_parts(self) -> (RsnTxFrame<N>, ApEapolTxContext) {
        (self.frame, self.context)
    }
}

/// The response window of one AP transmission. Neither its identity nor its
/// retry classification can be substituted at terminal delivery.
///
/// ```compile_fail,E0382
/// use oer_ieee80211_ap::ApEapolTxContext;
/// fn duplicate(context: ApEapolTxContext) -> (ApEapolTxContext, ApEapolTxContext) {
///     (context, context)
/// }
/// ```
///
/// ```compile_fail,E0599
/// use oer_ieee80211_ap::ApEapolTxContext;
/// fn clone_context(context: &ApEapolTxContext) -> ApEapolTxContext {
///     ApEapolTxContext::clone(context)
/// }
/// ```
///
/// ```
/// use oer_ieee80211_ap::ApEapolTxContext;
/// fn retain(context: ApEapolTxContext) -> ApEapolTxContext { context }
/// ```
#[must_use = "return the context at this transmission's terminal"]
#[derive(Debug)]
pub struct ApEapolTxContext {
    pub(super) association: ApAssociationIdentity,
    pub(super) transmit: oer_ieee80211_rsn::state::RsnTransmit,
}

/// The protocol consequence of an EAPOL publication's terminal. Resource
/// return happens for every variant, including a retired association/message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApEapolTxCompletion {
    /// RX or peer retirement already ended this exact response window.
    Superseded,
    /// A retransmission retains its already advanced retry alarm.
    WindowRetained,
    /// The initial publication established the current response deadline.
    WindowArmed,
}

impl<S: SppScope> AccessPointService<'_, S> {
    pub(super) fn bind_eapol<const N: usize>(
        &self,
        frame: RsnTxFrame<N>,
        transmit: oer_ieee80211_rsn::state::RsnTransmit,
    ) -> Result<ApEapolTransmit<N>, ApWpa2Error> {
        let peer = self.checked_peer(*frame.peer())?;
        Ok(ApEapolTransmit {
            frame,
            context: ApEapolTxContext {
                association: peer.association_identity(),
                transmit,
            },
        })
    }
}
