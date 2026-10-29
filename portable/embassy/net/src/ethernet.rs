//! Neutral Ethernet device integration; Xarxa packet owners stay in this adapter.

use crate::OwnedNetworkTxFrame;
use oer_network::{EthernetFrame, EthernetFrameView, FrameLengthError, NetDevice, TransmitRefused};

impl EthernetFrame for OwnedNetworkTxFrame<'_> {
    fn view(&self) -> Result<EthernetFrameView<'_>, FrameLengthError> {
        let (header, payload) = self
            .ethernet()
            .split_first_chunk::<{ oer_network::ETHERNET_HEADER_LEN }>()
            .ok_or(FrameLengthError::TooShort)?;
        let [d0, d1, d2, d3, d4, d5, s0, s1, s2, s3, s4, s5, e0, e1] = *header;
        Ok(EthernetFrameView {
            destination: [d0, d1, d2, d3, d4, d5],
            source: [s0, s1, s2, s3, s4, s5],
            ether_type: u16::from_be_bytes([e0, e1]),
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
            Err(reason) => Err(oer_network::TransmitError::InvalidLength(reason)),
        };
        result.map_err(|reason| TransmitRefused {
            frame: self,
            reason,
        })
    }
}

/// Copy a neutral received owner into the stack's final RX pool allocation.
/// Rejection returns the complete owner to the bridge's policy.
pub fn publish<const RX: usize, F: EthernetFrame>(
    publisher: &mut crate::OwnedRxPublisher<'_, RX>,
    frame: F,
) -> Result<(), (F, crate::RxEnqueueError)> {
    let result = frame
        .view()
        .map_err(crate::RxEnqueueError::InvalidLength)
        .and_then(|view| {
            publisher.try_send_parts(view.destination, view.source, view.ether_type, view.payload)
        });
    result.map_err(|reason| (frame, reason))
}

impl crate::OwnedLinkController<'_> {
    /// Follow both the neutral link level and its association identity. If a
    /// complete down/up cycle occurred between bridge polls, advance the
    /// existing packet generation before admitting the new association.
    pub fn follow_device(
        &mut self,
        device: &impl NetDevice,
        previous: &mut oer_network::LinkSnapshot,
    ) {
        let current = device.link();
        if current == *previous {
            return;
        }
        if current.state == oer_network::LinkState::Up
            && previous.state == oer_network::LinkState::Up
            && current.epoch != previous.epoch
        {
            self.set_link_up(false);
        }
        self.set_link_up(current.state == oer_network::LinkState::Up);
        *previous = current;
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_macros, clippy::disallowed_methods)]
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
> {
    radio: crate::OwnedNetworkRunner<'r, X, RX, TX>,
    device: D,
    link: oer_network::LinkSnapshot,
    pending: Option<(u64, OwnedNetworkTxFrame<'r>)>,
}

impl<'r, X: oer_memory::ExecutionContext, const RX: usize, const TX: usize, D: NetDevice>
    EthernetBridge<'r, X, RX, TX, D>
{
    pub fn new(radio: crate::OwnedNetworkRunner<'r, X, RX, TX>, device: D) -> Self {
        Self {
            radio,
            device,
            link: oer_network::LinkSnapshot::DOWN,
            pending: None,
        }
    }
    pub(crate) const fn link(&self) -> oer_network::LinkSnapshot {
        self.link
    }

    pub(crate) fn stop(&mut self) -> Result<(), crate::AccessRefused> {
        self.radio.link_controller().set_link_up(false);
        self.pending = None;
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
            if let Ok(view) = frame.view() {
                received(view);
            }
            if let Err((_owner, reason)) = publish(self.radio.rx_publisher(), frame) {
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
                oer_network::TransmitError::InvalidLength(_) => Ok(true),
                oer_network::TransmitError::QueueFull | oer_network::TransmitError::LinkDown => {
                    self.pending = Some((epoch, refused.frame));
                    // Readiness is only an observation. Rearm after a refusal;
                    // if credit is already back, schedule another bounded turn.
                    Ok(self.device.poll_transmit_ready(cx).is_ready())
                }
            },
        }
    }
}
