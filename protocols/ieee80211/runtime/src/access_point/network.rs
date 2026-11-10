//! The access point's network end: the MSDUs its authorized peers send the
//! network stack and the stack's frames for the BSS, each one copied.
//!
//! The runner copies a received MSDU into the receive queue and the stack's
//! sending path copies its frame into the transmit queue, so neither end
//! holds the other's memory: a frame the queue has no room for is refused
//! at once with its owner, and a stop leaves nothing lent.

use core::cell::Cell;
use core::future::Future;
use core::marker::PhantomData;
use core::task::{Context, Poll};

use oer_portable_memory::{WakingSpscQueue, WakingSpscReceiver, WakingSpscSender};
use oer_portable_sync::PriorityWaker;
use oer_protocols_ieee80211_core_roles_sta::data::EthernetFrame;
use oer_protocols_ieee80211_core_roles_sta::data::MsduParts;
use oer_protocols_network_port::{
    EthernetFrameView, FrameLengthError, FrameStorage, LentFrame, LinkSnapshot, LinkState,
    NetDevice, NetTransmit, ReceivedFrame, StorageAdvanceRefused, TransmitError, TransmitRefused,
};

/// The received MSDUs the queue holds for the network.
pub const AP_RX_DEPTH: usize = 4;

/// The network's MSDUs the queue holds for the BSS.
pub const AP_TX_DEPTH: usize = 4;

/// The two queues and the link of one access point's network end, placed
/// by the composition.
pub struct ApNetworkStorage {
    receive: WakingSpscQueue<EthernetFrame, AP_RX_DEPTH>,
    transmit: WakingSpscQueue<EthernetFrame, AP_TX_DEPTH>,
    link: ApLink,
}

impl ApNetworkStorage {
    pub const fn new() -> Self {
        Self {
            receive: WakingSpscQueue::new(),
            transmit: WakingSpscQueue::new(),
            link: ApLink::new(),
        }
    }

    /// Split the storage into the network's device, its transmit half and
    /// the runner's end.
    pub fn split<F>(&mut self) -> (ApNetDevice<'_>, ApNetTransmit<'_, F>, ApNetworkEnd<'_>) {
        let (receive_sender, receive_receiver) = self.receive.split();
        let (transmit_sender, transmit_receiver) = self.transmit.split();
        let link = &self.link;
        (
            ApNetDevice {
                receive: receive_receiver,
                link,
                observed: LinkSnapshot::DOWN,
                marked: 0,
            },
            ApNetTransmit {
                transmit: transmit_sender,
                link,
                frame: PhantomData,
            },
            ApNetworkEnd {
                receive: receive_sender,
                transmit: transmit_receiver,
                link,
            },
        )
    }
}

impl Default for ApNetworkStorage {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether the BSS carries data, and its epoch: up while the access point
/// beacons.
pub struct ApLink {
    snapshot: Cell<LinkSnapshot>,
    waker: PriorityWaker,
}

impl ApLink {
    const fn new() -> Self {
        Self {
            snapshot: Cell::new(LinkSnapshot::DOWN),
            waker: PriorityWaker::new(),
        }
    }

    fn set(&self, state: LinkState) {
        let current = self.snapshot.get();
        if current.state == state {
            return;
        }
        let epoch = match state {
            LinkState::Up => current.epoch.saturating_add(1),
            _ => current.epoch,
        };
        self.snapshot.set(LinkSnapshot { state, epoch });
        self.waker.wake();
    }
}

/// The runner's end: it hands the network received MSDUs and takes the
/// network's frames.
pub struct ApNetworkEnd<'a> {
    receive: WakingSpscSender<'a, EthernetFrame, AP_RX_DEPTH>,
    pub(crate) transmit: WakingSpscReceiver<'a, EthernetFrame, AP_TX_DEPTH>,
    link: &'a ApLink,
}

impl ApNetworkEnd<'_> {
    /// Hand one received MSDU to the network: whether the queue took it.
    pub fn deliver(&self, msdu: MsduParts<'_>) -> bool {
        match EthernetFrame::from_parts(msdu) {
            Ok(frame) => self.receive.try_send(frame).is_ok(),
            Err(_) => false,
        }
    }

    /// The BSS carries data, or no longer does.
    pub fn set_link(&self, up: bool) {
        self.link
            .set(if up { LinkState::Up } else { LinkState::Down });
    }

    /// The next frame of the network, if one waits.
    pub fn take(&self) -> Option<EthernetFrame> {
        self.transmit.try_receive()
    }

    /// Whether a frame of the network waits.
    pub fn waiting(&self) -> bool {
        !self.transmit.is_empty()
    }
}

/// The network stack's device: the link and the received MSDUs.
pub struct ApNetDevice<'a> {
    receive: WakingSpscReceiver<'a, EthernetFrame, AP_RX_DEPTH>,
    link: &'a ApLink,
    /// The link the stack last observed.
    observed: LinkSnapshot,
    /// The copies handed over as marks and not yet read.
    marked: usize,
}

/// A received MSDU the device keeps: [`NetDevice::read_copied`] reads it.
#[derive(Debug)]
pub struct ApCopied(());

/// The storage no lent frame of the access point uses: every received MSDU
/// is a copy.
pub struct NoStorage(core::convert::Infallible);

impl FrameStorage for NoStorage {
    fn bytes(&self) -> &[u8] {
        match self.0 {}
    }
    fn bytes_mut(&mut self) -> &mut [u8] {
        match self.0 {}
    }
    fn advance(&mut self, _: usize) -> Result<(), StorageAdvanceRefused> {
        match self.0 {}
    }
}

impl ReceivedFrame for ApCopied {
    type Storage = NoStorage;
    type Copied = ApCopied;
    fn into_lent(self) -> Result<LentFrame<NoStorage>, ApCopied> {
        Err(self)
    }
    /// The access point's device lends no buffer at all.
    fn copy_reason(_: &ApCopied) -> oer_protocols_network_port::CopyReason {
        oer_protocols_network_port::CopyReason::Format
    }
}

impl NetDevice for ApNetDevice<'_> {
    type RxFrame = ApCopied;
    fn link(&self) -> LinkSnapshot {
        self.link.snapshot.get()
    }
    fn poll_link(&mut self, cx: &mut Context<'_>) -> Poll<LinkSnapshot> {
        self.link.waker.register(cx.waker());
        let link = self.link.snapshot.get();
        if link == self.observed {
            Poll::Pending
        } else {
            self.observed = link;
            Poll::Ready(link)
        }
    }
    fn receive_queue_len(&self) -> usize {
        self.receive.len()
    }
    fn poll_receive_ready(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        let ready = core::pin::pin!(self.receive.ready());
        ready.poll(cx)
    }
    fn try_receive(&mut self) -> Option<ApCopied> {
        // The copy stays in the queue until its mark is read, in order.
        if self.receive.len() <= self.marked {
            return None;
        }
        self.marked = self.marked.saturating_add(1);
        Some(ApCopied(()))
    }
    fn read_copied<R>(
        &mut self,
        _: ApCopied,
        read: impl FnOnce(EthernetFrameView<'_>) -> R,
    ) -> Result<R, FrameLengthError> {
        self.marked = self.marked.saturating_sub(1);
        let frame = self
            .receive
            .try_receive()
            .ok_or(FrameLengthError::MissingCopy)?;
        let MsduParts {
            destination,
            source,
            ether_type,
            payload,
        } = frame.parts();
        Ok(read(EthernetFrameView {
            destination,
            source,
            ether_type,
            payload,
        }))
    }
}

/// The network stack's sending path into the BSS, taking the stack's
/// frames `F`.
pub struct ApNetTransmit<'a, F> {
    transmit: WakingSpscSender<'a, EthernetFrame, AP_TX_DEPTH>,
    link: &'a ApLink,
    frame: PhantomData<fn(F)>,
}

impl<F: oer_protocols_network_port::EthernetFrame> NetTransmit for ApNetTransmit<'_, F> {
    type Frame = F;
    fn has_room(&mut self) -> bool {
        self.transmit.free_capacity() != 0
    }
    fn poll_room(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        let mut permit = core::pin::pin!(self.transmit.ready());
        match permit.as_mut().poll(cx) {
            Poll::Ready(_) => Poll::Ready(()),
            Poll::Pending => Poll::Pending,
        }
    }
    fn try_transmit(&mut self, mut frame: F) -> Result<(), TransmitRefused<F>> {
        if self.link.snapshot.get().state != LinkState::Up {
            return Err(TransmitRefused {
                frame,
                reason: TransmitError::LinkDown,
            });
        }
        let copy = match frame.view() {
            Ok(view) => EthernetFrame::from_parts(MsduParts {
                destination: view.destination,
                source: view.source,
                ether_type: view.ether_type,
                payload: view.payload,
            })
            .map_err(|_| FrameLengthError::TooLong),
            Err(error) => Err(error),
        };
        let copy = match copy {
            Ok(copy) => copy,
            Err(error) => {
                return Err(TransmitRefused {
                    frame,
                    reason: TransmitError::InvalidLength(error),
                });
            }
        };
        match self.transmit.try_send(copy) {
            Ok(()) => Ok(()),
            Err(_) => Err(TransmitRefused {
                frame,
                reason: TransmitError::QueueFull,
            }),
        }
    }
    fn flush(&mut self) {
        self.transmit.flush();
    }
}
