//! The station's receive Block Ack owner on this service's port: the
//! upper MAC decides every agreement edge, and this binding carries out each
//! bank and frame transition on the running phase.
use super::*;
use oer_ieee80211_port::{PhaseBlockAckSlot, SupportedBlockAck};
use oer_ieee80211_sta::epoch::LinkEpoch;
use oer_ieee80211_upper_mac::block_ack::{RxBlockAckPort, RxBlockAckTypes};

/// The port's owners lent to the receive agreements of phase types `Ty`.
pub struct PortRxBlockAck<'id, Ty>(PhantomData<fn(&'id ()) -> Ty>);

impl<'id, Ty: PhaseTypes<'id>> RxBlockAckTypes for PortRxBlockAck<'id, Ty> {
    type Slot = PhaseBlockAckSlot<'id, Ty>;
    type Running = Ty::BlockAck;
    type Frame = Ty::RxBuffer;
    type Meta = Ty::RxMeta;
}

/// Placed receive Block Ack backing an image lends the service. Bank tokens
/// come from the port's existing shared pool; retained frames are its RX
/// owners. `BANKS` and `FRAMES` are the image's choice.
pub type RxBlockAckStorage<'id, Ty, const BANKS: usize, const FRAMES: usize> =
    oer_ieee80211_upper_mac::block_ack::RxBlockAckStorage<
        PortRxBlockAck<'id, Ty>,
        LinkEpoch<'id, <Ty as PhaseTypes<'id>>::EpochPath>,
        BANKS,
        FRAMES,
    >;

/// The loans of [`RxBlockAckStorage`] the service carries.
pub type RxBlockAckView<'id, Ty> = oer_ieee80211_upper_mac::block_ack::RxBlockAckView<
    'id,
    PortRxBlockAck<'id, Ty>,
    LinkEpoch<'id, <Ty as PhaseTypes<'id>>::EpochPath>,
>;

/// The running phase, carrying out the receive agreements' port calls.
pub(super) struct PhaseBlockAcks<'r, 'p, P>(&'r mut P, PhantomData<&'p ()>);

impl<'r, 'p, P> PhaseBlockAcks<'r, 'p, P> {
    pub(super) fn new(phase: &'r mut P) -> Self {
        Self(phase, PhantomData)
    }
}

impl<'p, 'id: 'p, P> RxBlockAckPort<PortRxBlockAck<'id, P::Types>> for PhaseBlockAcks<'_, 'p, P>
where
    P: LowerMacPhase<'p, 'id>,
{
    type Caps = P::Caps;

    fn add(
        &mut self,
        slot: PhaseBlockAckSlot<'id, P::Types>,
        agreement: SupportedBlockAck<P::Caps>,
    ) -> <P::Types as PhaseTypes<'id>>::BlockAck {
        self.0.station().add_block_ack(slot, agreement)
    }

    fn remove(
        &mut self,
        running: <P::Types as PhaseTypes<'id>>::BlockAck,
    ) -> PhaseBlockAckSlot<'id, P::Types> {
        self.0.station().remove_block_ack(running)
    }

    fn recycle(&mut self, frame: <P::Types as PhaseTypes<'id>>::RxBuffer) {
        self.0.recycle(frame);
    }
}
