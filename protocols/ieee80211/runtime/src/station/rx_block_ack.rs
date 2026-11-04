//! The station's receive Block Ack owner on the port's owners: the upper
//! MAC decides every agreement edge, and the actor carries out each bank
//! and frame transition on its port.
use core::marker::PhantomData;
use oer_protocols_ieee80211_contracts_port::{PortBlockAckSlot, PortTypes};
use oer_protocols_ieee80211_core_roles_sta::epoch::LinkEpoch;
use oer_protocols_ieee80211_core_upper_mac::block_ack::RxBlockAckTypes;

/// The port's owners lent to the receive agreements of phase types `Ty`.
pub struct PortRxBlockAck<'id, Ty>(PhantomData<fn(&'id ()) -> Ty>);

impl<'id, Ty: PortTypes<'id>> RxBlockAckTypes for PortRxBlockAck<'id, Ty> {
    type Slot = PortBlockAckSlot<'id, Ty>;
    type Running = Ty::BlockAck;
    type Frame = Ty::RxBuffer;
    type Meta = Ty::RxMeta;
}

/// Placed receive Block Ack backing an image lends the actor. Bank tokens
/// come from the port's existing shared pool; retained frames are its RX
/// owners. `BANKS` and `FRAMES` are the image's choice.
pub type RxBlockAckStorage<'id, Ty, const BANKS: usize, const FRAMES: usize> =
    oer_protocols_ieee80211_core_upper_mac::block_ack::RxBlockAckStorage<
        PortRxBlockAck<'id, Ty>,
        LinkEpoch<'id, <Ty as PortTypes<'id>>::EpochPath>,
        BANKS,
        FRAMES,
    >;

/// The loans of [`RxBlockAckStorage`] the actor carries.
pub type RxBlockAckView<'id, Ty> =
    oer_protocols_ieee80211_core_upper_mac::block_ack::RxBlockAckView<
        'id,
        PortRxBlockAck<'id, Ty>,
        LinkEpoch<'id, <Ty as PortTypes<'id>>::EpochPath>,
    >;
