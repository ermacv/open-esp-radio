//! Neutral Ethernet device integration; Xarxa packet owners stay in this adapter.

use crate::OwnedNetworkTxFrame;
use oer_network_port::{
    EthernetFrame, EthernetFrameView, FrameLengthError, FrameStorage, LentFrame, LentParts,
    NetDevice, ReceivedFrame, TransmitRefused,
};
use oer_network_wire::ethernet::{EtherType, EthernetHeader};

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

/// A projection borrows the packet until admission and its observation finish.
/// The current neutral TX contract copies into the device's bounded owner.
struct FrameProjection<'a>(EthernetFrameView<'a>);
impl EthernetFrame for FrameProjection<'_> {
    fn view(&self) -> Result<EthernetFrameView<'_>, FrameLengthError> {
        Ok(self.0)
    }
}

impl OwnedNetworkTxFrame<'_> {
    /// Transfer this stack owner into a neutral device. Refusal preserves its
    /// packet and software credit together; neither is returned early. The
    /// accepted callback runs only after admission, while that packet and
    /// credit remain retained, matching the copying bridge's observation point.
    pub fn transmit_to(
        self,
        device: &mut impl NetDevice,
        accepted: impl FnOnce(EthernetFrameView<'_>),
    ) -> Result<(), TransmitRefused<Self>> {
        let result = match self.view() {
            Ok(view) => match device.try_transmit(FrameProjection(view)) {
                Ok(()) => {
                    accepted(view);
                    Ok(())
                }
                Err(refused) => Err(refused.reason),
            },
            Err(reason) => Err(oer_network_port::TransmitError::InvalidLength(reason)),
        };
        result.map_err(|reason| TransmitRefused {
            frame: self,
            reason,
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
/// stable-bytes contract is refused as `OutsideStorage`.
pub fn publish_lent<const RX: usize, S: FrameStorage, const N: usize>(
    publisher: &mut crate::OwnedRxPublisher<'_, RX>,
    slots: Option<&'static LentSlots<S, N>>,
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

impl crate::OwnedLinkController<'_> {
    /// Follow both the neutral link level and its association identity. If a
    /// complete down/up cycle occurred between bridge polls, advance the
    /// existing packet generation before admitting the new association.
    pub fn follow_device(
        &mut self,
        device: &impl NetDevice,
        previous: &mut oer_network_port::LinkSnapshot,
    ) {
        let current = device.link();
        if current == *previous {
            return;
        }
        if current.state == oer_network_port::LinkState::Up
            && previous.state == oer_network_port::LinkState::Up
            && current.epoch != previous.epoch
        {
            self.set_link_up(false);
        }
        self.set_link_up(current.state == oer_network_port::LinkState::Up);
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
    X: oer_memory::ExecutionContext,
    const RX: usize,
    const TX: usize,
    D: NetDevice,
    const LENT: usize = 0,
> {
    radio: crate::OwnedNetworkRunner<'r, X, RX, TX>,
    device: D,
    lent: Option<&'static LentSlots<Lent<D>, LENT>>,
    link: oer_network_port::LinkSnapshot,
    pending: Option<(u64, OwnedNetworkTxFrame<'r>)>,
}

/// The storage of a device's lent frames.
pub type Lent<D> = <<D as NetDevice>::RxFrame as ReceivedFrame>::Storage;

impl<
    'r,
    X: oer_memory::ExecutionContext,
    const RX: usize,
    const TX: usize,
    D: NetDevice,
    const LENT: usize,
> EthernetBridge<'r, X, RX, TX, D, LENT>
{
    /// `lent` adopts the frames the device lends; without it the bridge
    /// copies them.
    pub fn new(
        radio: crate::OwnedNetworkRunner<'r, X, RX, TX>,
        device: D,
        lent: Option<&'static LentSlots<Lent<D>, LENT>>,
    ) -> Self {
        Self {
            radio,
            device,
            lent,
            link: oer_network_port::LinkSnapshot::DOWN,
            pending: None,
        }
    }
    pub(crate) const fn link(&self) -> oer_network_port::LinkSnapshot {
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
        Ok(())
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
        transmitted: impl FnMut(EthernetFrameView<'_>),
    ) -> core::task::Poll<Result<core::convert::Infallible, crate::AccessRefused>> {
        use core::task::Poll;
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
        // Preserve this bridge's existing drain-and-report-refusal RX policy.
        // Register even when no frame is available.
        while self.device.poll_receive_ready(cx).is_ready() {
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
        match self.poll_transmit_one(cx, transmitted) {
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
        mut observe: impl FnMut(EthernetFrameView<'_>),
    ) -> Result<bool, crate::AccessRefused> {
        use core::{future::Future, task::Poll};
        let (epoch, packet) = match self.pending.take() {
            Some(pending) => pending,
            None => {
                {
                    let mut publication = core::pin::pin!(self.radio.wait_tx_publication());
                    match publication.as_mut().poll(cx) {
                        Poll::Pending => return Ok(false),
                        Poll::Ready(result) => result?,
                    }
                }
                let Some(packet) = self.radio.try_receive_tx()? else {
                    // The coalesced publication outlived its last packet.
                    return Ok(true);
                };
                (self.link.epoch, packet)
            }
        };
        if self.device.poll_transmit_ready(cx).is_pending() {
            self.pending = Some((epoch, packet));
            return Ok(false);
        }
        match packet.transmit_to(&mut self.device, &mut observe) {
            Ok(()) => Ok(true),
            Err(refused) => match refused.reason {
                oer_network_port::TransmitError::InvalidLength(_) => Ok(true),
                oer_network_port::TransmitError::QueueFull
                | oer_network_port::TransmitError::LinkDown => {
                    self.pending = Some((epoch, refused.frame));
                    // Readiness is only an observation. Rearm after a refusal;
                    // if credit is already back, schedule another bounded turn.
                    Ok(self.device.poll_transmit_ready(cx).is_ready())
                }
            },
        }
    }
}
