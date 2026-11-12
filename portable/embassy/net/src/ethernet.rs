//! Neutral Ethernet device integration; Xarxa packet owners stay in this adapter.

use crate::OwnedNetworkTxFrame;
use oer_protocols_network_port::{
    BatchEnd, EthernetFrame, EthernetFrameView, FrameLengthError, FrameStorage, Incoming,
    LentFrame, LentParts, NetDevice, TransmitError, TransmitRefused, TxStorage,
};
use oer_protocols_network_wire::ethernet::{EtherType, EthernetHeader};

impl EthernetFrame for OwnedNetworkTxFrame {
    fn view(&mut self) -> Result<EthernetFrameView<'_>, FrameLengthError> {
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
pub enum TxWriteRefused {
    /// The frame stays the caller's.
    Kept(TransmitRefused<OwnedNetworkTxFrame>),
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
}

impl<S: TxStorage, const N: usize> ParkedTx<S, N> {
    /// Over the image's `slots`; the device's memory comes in through
    /// [`Self::give`].
    pub const fn new(slots: &'static TxSlots<S, N>) -> Self {
        Self { slots, spare: None }
    }

    /// Whether any frame may go now: the spare is here for one from
    /// elsewhere.
    pub fn available(&self) -> bool {
        self.spare.is_some()
    }

    /// Memory free for the network: the spare first, then parked for the
    /// stack's next packets. Memory the slots refuse (more than they hold,
    /// or bytes the packets cannot use) goes back to the device by its drop.
    pub fn give(&mut self, storage: S) {
        if self.spare.is_none() {
            self.spare = Some(storage);
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
    pub fn write(&mut self, frame: OwnedNetworkTxFrame) -> Result<TxWritten<S>, TxWriteRefused> {
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

/// Ethernet translation for an image whose existing bridge drains RX: RX
/// and link changes wake it. The stack's sending path owns transmission.
/// Observations are callbacks owned by the image, never counters here.
pub struct EthernetBridge<
    'r,
    X: oer_portable_memory::ExecutionContext,
    const RX: usize,
    D: NetDevice,
    const LENT: usize,
> {
    radio: crate::OwnedNetworkRunner<'r, X, RX>,
    device: D,
    lent: Option<&'static LentSlots<Lent<D>, LENT>>,
    link: oer_protocols_network_port::LinkSnapshot,
}

/// The storage of a device's lent frames.
pub type Lent<D> = <D as NetDevice>::Storage;

impl<'r, X: oer_portable_memory::ExecutionContext, const RX: usize, D: NetDevice, const LENT: usize>
    EthernetBridge<'r, X, RX, D, LENT>
{
    /// `lent`, the image's `LENT` packet slots, adopts the frames the device
    /// lends; without it the bridge copies them.
    pub fn new(
        radio: crate::OwnedNetworkRunner<'r, X, RX>,
        device: D,
        lent: Option<&'static LentSlots<Lent<D>, LENT>>,
    ) -> Self {
        Self {
            radio,
            device,
            lent,
            link: oer_protocols_network_port::LinkSnapshot::DOWN,
        }
    }
    pub(crate) const fn link(&self) -> oer_protocols_network_port::LinkSnapshot {
        self.link
    }

    /// Take the link down and give back every frame the device still
    /// queues, which may be lent: dropping one returns it to the device
    /// before the device's own stop.
    ///
    /// # Errors
    ///
    /// The device's receive queue broke: the frames after the break stay
    /// with the device.
    pub(crate) fn stop(&mut self) -> Result<(), oer_protocols_network_port::ReceiveFault> {
        self.halt();
        loop {
            match self.device.receive(usize::MAX, |frame| drop(frame)).end {
                BatchEnd::Drained => break,
                BatchEnd::Limit => {}
                BatchEnd::Fault(fault) => {
                    self.reclaim_lent();
                    return Err(fault);
                }
            }
        }
        self.reclaim_lent();
        Ok(())
    }

    /// Take the link down without reading the device: what it still queues
    /// stays its own. The stop of a device whose receive queue broke.
    pub(crate) fn halt(&mut self) {
        self.radio.link_controller().set_link_up(false);
    }

    /// Return to the device every lent frame whose packet the stack
    /// dropped since the last reclaim: the typed slots drop its owner, a
    /// direct call. A packet's drop only released its slot.
    pub(crate) fn reclaim_lent(&self) {
        if let Some(lent) = self.lent {
            lent.reclaim();
        }
    }

    /// One turn: follow the link and drain RX. Always arm link and RX before
    /// observing them. No timer or time driver is needed to make an idle
    /// bridge run again.
    ///
    /// # Errors
    ///
    /// The device's receive queue broke: the turn reads no frame after it.
    pub fn poll(
        &mut self,
        cx: &mut core::task::Context<'_>,
        mut received: impl FnMut(EthernetFrameView<'_>),
        mut refused: impl FnMut(crate::RxEnqueueError),
        mut copied: impl FnMut(LentCopy),
    ) -> Result<(), oer_protocols_network_port::ReceiveFault> {
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
        // A frame stays with the device while the stack's queue is full:
        // the stack's next take wakes this bridge, and the device's owner
        // holds its frames back meanwhile. Register even when no frame is
        // available.
        while self.radio.rx_publisher().poll_queue_room(cx).is_ready()
            && self.device.poll_receive_ready(cx).is_ready()
        {
            // As many as the stack's queue has room for, in one batch: a
            // lent frame needs no more, its packet is its own buffer.
            let room = RX.saturating_sub(self.radio.rx_publisher().queue_len());
            let publisher = self.radio.rx_publisher();
            let lent_slots = self.lent;
            let batch = self.device.receive(room.max(1), |incoming| {
                let published = match incoming {
                    Incoming::Lent(cell) => {
                        let Some(lent) = cell.take() else { return };
                        if let Ok(view) = lent.view() {
                            received(view);
                        }
                        publish_lent(publisher, lent_slots, lent, &mut copied)
                    }
                    Incoming::Copied { frame, .. } => frame
                        .map_err(crate::RxEnqueueError::InvalidLength)
                        .and_then(|view| {
                            received(view);
                            publish_view(publisher, view)
                        }),
                };
                if let Err(reason) = published {
                    refused(reason);
                }
            });
            match batch.end {
                BatchEnd::Drained => break,
                BatchEnd::Limit => {}
                BatchEnd::Fault(fault) => return Err(fault),
            }
        }
        Ok(())
    }
}
