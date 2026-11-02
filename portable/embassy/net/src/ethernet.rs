//! Neutral Ethernet device integration; Xarxa packet owners stay in this adapter.

use crate::OwnedNetworkTxFrame;
use oer_protocols_network_port::{
    EthernetFrame, EthernetFrameView, FrameLengthError, FrameStorage, LentFrame, LentParts,
    NetDevice, ReceivedFrame, TransmitError, TransmitRefused, TxStorage,
};
use oer_protocols_network_wire::ethernet::{EtherType, EthernetHeader};

impl EthernetFrame for OwnedNetworkTxFrame<'_> {
    fn view(&self) -> Result<EthernetFrameView<'_>, FrameLengthError> {
        let (header, payload) =
            EthernetHeader::parse(self.ethernet()).map_err(|_| FrameLengthError::TooShort)?;
        Ok(EthernetFrameView {
            destination: header.destination,
            source: header.source,
            ether_type: header.ether_type.value(),
            payload,
        })
    }
}

/// A device whose transmissions take the stack's packet owner itself: the
/// device reads the frame where the stack wrote it, without a copy.
pub trait OwnedTxDevice<'r>: NetDevice<TxFrame = OwnedNetworkTxFrame<'r>> {}
impl<'r, D: NetDevice<TxFrame = OwnedNetworkTxFrame<'r>>> OwnedTxDevice<'r> for D {}

impl<'r> OwnedNetworkTxFrame<'r> {
    /// Transfer this stack owner into a neutral device: on acceptance the
    /// device holds the packet and its software credit; a refusal returns
    /// both together, neither released early.
    pub fn transmit_to(
        self,
        device: &mut impl OwnedTxDevice<'r>,
    ) -> Result<(), TransmitRefused<Self>> {
        if let Err(reason) = self.view() {
            return Err(TransmitRefused {
                frame: self,
                reason: oer_protocols_network_port::TransmitError::InvalidLength(reason),
            });
        }
        device.try_transmit(self)
    }
}

/// A lent frame's storage, started at its Ethernet header: the bytes one
/// packet adopts. Dropping it drops the storage, which returns it to its
/// lender.
pub struct InPlace<S>(S);

impl<S: FrameStorage> xarxa_driver::ExternalPacketStorage for InPlace<S> {
    fn storage_mut(&mut self) -> &mut [u8] {
        self.0.bytes_mut()
    }
}

/// The packet slots of a device's lent frames, placed in a `static` by the
/// image; their count bounds the lent frames the stack holds at once.
pub type LentSlots<S, const N: usize> = xarxa_driver::ExternalPacketSlots<InPlace<S>, N>;

/// Why the bridge copied a frame its device lent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LentCopy {
    /// The bridge has no slots for lent frames.
    NoSlots,
    /// The payload starts too early for an Ethernet header before it.
    NoHeadroom,
    /// The slots refused the packet.
    Refused(xarxa_driver::ExternalPacketRefusal),
}

/// Publish a lent frame in place: write its Ethernet header before its
/// payload and adopt its storage as the packet, without copying the
/// payload. When that is impossible the frame is copied, as a copied
/// device frame is, after `copied` reports why. A storage that broke its
/// stable-bytes contract is refused as `OutsideStorage`. `slots` are the
/// image's `LENT` packet slots, named by type so that their adoption is a
/// direct call.
pub fn publish_lent<const RX: usize, S: FrameStorage, const LENT: usize>(
    publisher: &mut crate::OwnedRxPublisher<'_, RX>,
    slots: Option<&'static LentSlots<S, LENT>>,
    frame: LentFrame<S>,
    copied: &mut impl FnMut(LentCopy),
) -> Result<(), crate::RxEnqueueError> {
    use crate::RxEnqueueError::InvalidLength;
    let outside = InvalidLength(FrameLengthError::OutsideStorage);
    let LentParts {
        mut storage,
        destination,
        source,
        ether_type,
        payload,
    } = frame.into_parts();
    let header = EthernetHeader {
        destination,
        source,
        ether_type: EtherType::new(ether_type),
    }
    .to_bytes();
    let placed = payload
        .start
        .checked_sub(header.len())
        .zip(payload.len().checked_add(header.len()));
    let (Some(slots), Some((start, len))) = (slots, placed) else {
        copied(if slots.is_none() {
            LentCopy::NoSlots
        } else {
            LentCopy::NoHeadroom
        });
        let payload = storage.bytes().get(payload).ok_or(outside)?;
        return publisher.try_send_parts(destination, source, ether_type, payload);
    };
    // The packet begins at the header: the storage starts there, and the
    // payload follows the header in it.
    storage.advance(start).map_err(|_| outside)?;
    let bytes = storage.bytes_mut();
    if bytes.len() < len {
        return Err(outside);
    }
    for (byte, value) in bytes.iter_mut().zip(header) {
        *byte = value;
    }
    match slots.adopt(InPlace(storage), 0, len) {
        Ok(packet) => publisher.try_publish(packet),
        Err(refused) => {
            copied(LentCopy::Refused(refused.reason));
            // The header write left the payload as it was.
            let storage = refused.owner.0;
            let payload = storage.bytes().get(header.len()..len).ok_or(outside)?;
            publisher.try_send_parts(destination, source, ether_type, payload)
        }
    }
}

/// Copy a received frame's view into the stack's final RX pool allocation.
pub fn publish_view<const RX: usize>(
    publisher: &mut crate::OwnedRxPublisher<'_, RX>,
    view: EthernetFrameView<'_>,
) -> Result<(), crate::RxEnqueueError> {
    publisher.try_send_parts(view.destination, view.source, view.ether_type, view.payload)
}

/// A device's transmit memory as a parked packet's storage.
pub struct TxInPlace<S>(S);

impl<S: TxStorage> xarxa_driver::ExternalPacketStorage for TxInPlace<S> {
    fn storage_mut(&mut self) -> &mut [u8] {
        self.0.bytes_mut()
    }
}

/// The packet slots of a device's transmit memory, placed in a `static` by
/// the image; their allocator is the stack's general allocator, so that a
/// packet the stack writes is already in the device's memory.
pub type TxSlots<S, const N: usize> = xarxa_driver::ParkedPacketSlots<TxInPlace<S>, N>;

/// A frame in a device's transmit memory: `len` octets from `start` in
/// [`TxStorage::bytes_mut`].
pub struct TxWritten<S> {
    pub storage: S,
    pub start: usize,
    pub len: usize,
}

/// A frame [`ParkedTx::write`] did not place.
pub enum TxWriteRefused<'r> {
    /// The frame stays the caller's.
    Kept(TransmitRefused<OwnedNetworkTxFrame<'r>>),
    /// The frame was lost on the way: slots that refused a packet they own.
    Lost,
}

/// The network end of a device's transmit memory, on the network's
/// executor: the memory the device lends parks in `slots` for the stack to
/// allocate over, but one spare that a frame from elsewhere (a reply built in
/// a received packet's storage) is copied into.
pub struct ParkedTx<S: TxStorage, const N: usize> {
    slots: &'static TxSlots<S, N>,
    spare: Option<S>,
    /// A frame from elsewhere waited for the spare.
    starved: bool,
}

impl<S: TxStorage, const N: usize> ParkedTx<S, N> {
    /// Over the image's `slots`; the device's memory comes in through
    /// [`Self::give`].
    pub const fn new(slots: &'static TxSlots<S, N>) -> Self {
        Self {
            slots,
            spare: None,
            starved: false,
        }
    }

    /// Whether a frame refused for want of the spare may go now.
    pub fn available(&self) -> bool {
        !self.starved || self.spare.is_some()
    }

    /// Memory free for the network: the spare first, then parked for the
    /// stack's next packets. Memory the slots refuse (more than they hold,
    /// or bytes the packets cannot use) goes back to the device by its drop.
    pub fn give(&mut self, storage: S) {
        if self.spare.is_none() {
            self.spare = Some(storage);
            self.starved = false;
            return;
        }
        if let Err(refused) = self.slots.park(TxInPlace(storage)) {
            drop(refused.owner.0);
        }
    }

    /// The memory `frame` is in: its own when the stack wrote it in these
    /// slots, no copy, or else the spare, the frame copied to its start.
    ///
    /// # Errors
    ///
    /// A frame from elsewhere waits while the spare is out
    /// ([`TransmitError::QueueFull`]); one longer than the spare is refused
    /// ([`TransmitError::InvalidLength`]). Both keep the frame.
    pub fn write<'r>(
        &mut self,
        frame: OwnedNetworkTxFrame<'r>,
    ) -> Result<TxWritten<S>, TxWriteRefused<'r>> {
        if self.slots.owns(frame.packet()) {
            return match self.slots.take(frame.into_packet()) {
                Ok(taken) => Ok(TxWritten {
                    storage: taken.owner.0,
                    start: taken.headroom,
                    len: taken.len,
                }),
                // The slots own the packet, so they take it: one they
                // return ends here.
                Err(packet) => {
                    drop(packet);
                    Err(TxWriteRefused::Lost)
                }
            };
        }
        let Some(mut spare) = self.spare.take() else {
            self.starved = true;
            return Err(TxWriteRefused::Kept(TransmitRefused {
                frame,
                reason: TransmitError::QueueFull,
            }));
        };
        let ethernet = frame.ethernet();
        let len = ethernet.len();
        let Some(bytes) = spare.bytes_mut().get_mut(..len) else {
            self.spare = Some(spare);
            return Err(TxWriteRefused::Kept(TransmitRefused {
                frame,
                reason: TransmitError::InvalidLength(FrameLengthError::TooLong),
            }));
        };
        for (byte, value) in bytes.iter_mut().zip(ethernet) {
            *byte = *value;
        }
        Ok(TxWritten {
            storage: spare,
            start: 0,
            len,
        })
    }
}

impl crate::OwnedLinkController<'_> {
    /// Follow both the neutral link level and its association identity. If a
    /// complete down/up cycle occurred between bridge polls, advance the
    /// existing packet generation before admitting the new association.
    pub fn follow_device(
        &mut self,
        device: &impl NetDevice,
        previous: &mut oer_protocols_network_port::LinkSnapshot,
    ) {
        let current = device.link();
        if current == *previous {
            return;
        }
        if current.state == oer_protocols_network_port::LinkState::Up
            && previous.state == oer_protocols_network_port::LinkState::Up
            && current.epoch != previous.epoch
        {
            self.set_link_up(false);
        }
        self.set_link_up(current.state == oer_protocols_network_port::LinkState::Up);
        *previous = current;
    }
}

#[cfg(test)]
#[allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::arithmetic_side_effects,
    reason = "tests build fixed frame fixtures"
)]
mod tests;

/// Ethernet translation for an image whose existing bridge drains RX, serves
/// one TX per turn. RX, TX publication/capacity and link changes wake it.
/// The adapter owns every packet and retains a refused TX across readiness
/// waits. Observations are callbacks owned by the image, never counters here.
pub struct EthernetBridge<
    'r,
    X: oer_portable_memory::ExecutionContext,
    const RX: usize,
    const TX: usize,
    D: OwnedTxDevice<'r>,
    const LENT: usize,
> {
    radio: crate::OwnedNetworkRunner<'r, X, RX, TX>,
    device: D,
    lent: Option<&'static LentSlots<Lent<D>, LENT>>,
    link: oer_protocols_network_port::LinkSnapshot,
    pending: Option<(u64, OwnedNetworkTxFrame<'r>)>,
}

/// What one bridge turn did with the stack's transmissions, for an image
/// that observes them: the packets the stack's queue held for the device
/// when the turn began, out of its `capacity`, whether a packet waited from
/// an earlier turn, and what became of the turn's packet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxTurn {
    pub queued: usize,
    pub capacity: usize,
    pub waited: bool,
    pub outcome: TxOutcome,
}

/// What became of a bridge turn's packet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxOutcome {
    /// None: the stack published no packet.
    Idle,
    /// The device took the packet.
    Moved,
    /// The packet waits for the device's room, for a later turn.
    Held,
    /// The device refused the packet for its length: it ended here.
    Dropped,
}

/// The storage of a device's lent frames.
pub type Lent<D> = <<D as NetDevice>::RxFrame as ReceivedFrame>::Storage;

impl<
    'r,
    X: oer_portable_memory::ExecutionContext,
    const RX: usize,
    const TX: usize,
    D: OwnedTxDevice<'r>,
    const LENT: usize,
> EthernetBridge<'r, X, RX, TX, D, LENT>
{
    /// `lent`, the image's `LENT` packet slots, adopts the frames the device
    /// lends; without it the bridge copies them.
    pub fn new(
        radio: crate::OwnedNetworkRunner<'r, X, RX, TX>,
        device: D,
        lent: Option<&'static LentSlots<Lent<D>, LENT>>,
    ) -> Self {
        Self {
            radio,
            device,
            lent,
            link: oer_protocols_network_port::LinkSnapshot::DOWN,
            pending: None,
        }
    }
    pub(crate) const fn link(&self) -> oer_protocols_network_port::LinkSnapshot {
        self.link
    }

    pub(crate) fn stop(&mut self) -> Result<(), crate::AccessRefused> {
        self.radio.link_controller().set_link_up(false);
        self.pending = None;
        // A received frame the device still queues may be lent: dropping it
        // returns it to the device before the device's own stop.
        while let Some(frame) = self.device.try_receive() {
            drop(frame);
        }
        while let Some(packet) = self.radio.try_receive_tx()? {
            drop(packet);
        }
        self.reclaim_lent();
        Ok(())
    }

    /// Return to the device every lent frame whose packet the stack
    /// dropped since the last reclaim: the typed slots drop its owner, a
    /// direct call. A packet's drop only released its slot.
    pub(crate) fn reclaim_lent(&self) {
        if let Some(lent) = self.lent {
            lent.reclaim();
        }
    }

    /// One drain/one-TX turn. Always arm link and RX before observing them;
    /// a blocked TX remains owned here while RX and link keep progressing.
    /// No timer or time driver is needed to make an idle bridge run again.
    pub fn poll(
        &mut self,
        cx: &mut core::task::Context<'_>,
        mut received: impl FnMut(EthernetFrameView<'_>),
        mut refused: impl FnMut(crate::RxEnqueueError),
        mut copied: impl FnMut(LentCopy),
        mut transmitted: impl FnMut(TxTurn),
    ) -> core::task::Poll<Result<core::convert::Infallible, crate::AccessRefused>> {
        use core::task::Poll;
        // A lent frame's packet dropped later wakes this task to return it:
        // register before reclaiming, so no release is missed.
        if let Some(lent) = self.lent {
            lent.register_reclaim(cx.waker());
        }
        self.reclaim_lent();
        let _ = self.device.poll_link(cx);
        self.radio
            .link_controller()
            .follow_device(&self.device, &mut self.link);
        if self
            .pending
            .as_ref()
            .is_some_and(|(epoch, _)| *epoch != self.link.epoch)
        {
            self.pending = None;
        }
        // A frame stays with the device while the stack's queue is full:
        // the stack's next take wakes this bridge, and the device's owner
        // holds its frames back meanwhile. Register even when no frame is
        // available.
        while self.radio.rx_publisher().poll_queue_room(cx).is_ready()
            && self.device.poll_receive_ready(cx).is_ready()
        {
            let Some(frame) = self.device.try_receive() else {
                break;
            };
            let published = match frame.into_lent() {
                Ok(lent) => {
                    if let Ok(view) = lent.view() {
                        received(view);
                    }
                    publish_lent(self.radio.rx_publisher(), self.lent, lent, &mut copied)
                }
                Err(copy) => {
                    let publisher = self.radio.rx_publisher();
                    self.device
                        .read_copied(copy, |view| {
                            received(view);
                            publish_view(publisher, view)
                        })
                        .map_err(crate::RxEnqueueError::InvalidLength)
                        .and_then(|published| published)
                }
            };
            if let Err(reason) = published {
                refused(reason);
            }
        }
        match self.poll_transmit_one(cx, &mut transmitted) {
            Err(error) => Poll::Ready(Err(error)),
            Ok(progress) => {
                if progress {
                    // A consumed publication hint or packet needs a fresh turn
                    // to arm the next wait, and yields to other P1 futures.
                    cx.waker().wake_by_ref();
                }
                Poll::Pending
            }
        }
    }

    fn poll_transmit_one(
        &mut self,
        cx: &mut core::task::Context<'_>,
        transmitted: &mut impl FnMut(TxTurn),
    ) -> Result<bool, crate::AccessRefused> {
        use core::{future::Future, task::Poll};
        let waited = self.pending.is_some();
        let queued = self.radio.tx_queue_len()?;
        let mut ended = |outcome, progress| {
            transmitted(TxTurn {
                queued,
                capacity: TX,
                waited,
                outcome,
            });
            Ok(progress)
        };
        let (epoch, packet) = match self.pending.take() {
            Some(pending) => pending,
            None => {
                {
                    let mut publication = core::pin::pin!(self.radio.wait_tx_publication());
                    match publication.as_mut().poll(cx) {
                        Poll::Pending => return ended(TxOutcome::Idle, false),
                        Poll::Ready(result) => result?,
                    }
                }
                let Some(packet) = self.radio.try_receive_tx()? else {
                    // The coalesced publication outlived its last packet.
                    return ended(TxOutcome::Idle, true);
                };
                (self.link.epoch, packet)
            }
        };
        if self.device.poll_transmit_ready(cx).is_pending() {
            self.pending = Some((epoch, packet));
            return ended(TxOutcome::Held, false);
        }
        match packet.transmit_to(&mut self.device) {
            Ok(()) => ended(TxOutcome::Moved, true),
            Err(refused) => match refused.reason {
                oer_protocols_network_port::TransmitError::InvalidLength(_) => {
                    ended(TxOutcome::Dropped, true)
                }
                oer_protocols_network_port::TransmitError::QueueFull
                | oer_protocols_network_port::TransmitError::LinkDown => {
                    self.pending = Some((epoch, refused.frame));
                    // Readiness is only an observation. Rearm after a refusal;
                    // if credit is already back, schedule another bounded turn.
                    let again = self.device.poll_transmit_ready(cx).is_ready();
                    ended(TxOutcome::Held, again)
                }
            },
        }
    }
}
