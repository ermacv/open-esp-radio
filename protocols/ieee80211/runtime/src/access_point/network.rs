//! The access point's network end: the MSDUs its authorized peers send the
//! network stack, copied, and the stack's frames for the BSS, through the
//! same transmit boundary as a station's: the stack's sending path writes
//! its frame into one of the access point's frames ([`ApFrames`]) and hands
//! the frame's name, its length and the priority its marking requests to
//! the runner's queue ([`Outgoing`]); the runner queues it per peer and
//! TID, sends it, and gives the frame back ([`TxEnded`]), free again.
//! A frame no free memory or no room takes is refused at once with its
//! owner, and a stop leaves nothing lent.

use core::cell::Cell;
use core::future::Future;
use core::marker::PhantomData;
use core::task::{Context, Poll};

use oer_portable_memory::{
    ClaimCell, SlotReceiver, SlotSender, WakingSlotQueue, WakingSpscQueue, WakingSpscReceiver,
    WakingSpscSender,
};
use oer_portable_sync::PriorityWaker;
use oer_protocols_ieee80211_core_roles_sta::data::EthernetFrame;
use oer_protocols_ieee80211_core_roles_sta::data::MsduParts;
use oer_protocols_network_port::{
    EthernetFrameView, FrameLengthError, FrameStorage, LentFrame, LinkSnapshot, LinkState,
    NetDevice, ReceivedFrame, StorageAdvanceRefused, TransmitError, TransmitRefused,
};

use crate::station::{DeviceTransmit, LinkView, Outgoing, SameHart, TxEnded, TxRefused, TxSupply};

/// The received MSDUs the queue holds for the network.
pub const AP_RX_DEPTH: usize = 4;

/// The network's frames the access point holds at once: its frames, and
/// the depth of the queues to and from the runner.
pub const AP_TX_DEPTH: usize = 4;

/// The access point's frames the network writes into, one per frame it has
/// out; a frame is named by its place ([`ApMemory`]).
pub struct ApFrames {
    frames: [ClaimCell<EthernetFrame>; AP_TX_DEPTH],
}

/// The name of one of the access point's frames: the network's while it
/// writes it, the runner's from its queue until it gives it back.
#[derive(Debug, Eq, PartialEq)]
pub struct ApMemory(u8);

impl ApFrames {
    const fn new() -> Self {
        Self {
            frames: [const { ClaimCell::new() }; AP_TX_DEPTH],
        }
    }

    /// Run `read` on the frame `memory` names; `None` for no frame of
    /// these.
    pub fn read<R>(&self, memory: &ApMemory, read: impl FnOnce(&EthernetFrame) -> R) -> Option<R> {
        self.frames
            .get(usize::from(memory.0))?
            .lend(|frame| read(frame))
            .ok()
    }
}

/// The two queues and the link of one access point's network end, its
/// frames and the queues between the stack and the runner, placed by the
/// composition.
pub struct ApNetworkStorage {
    receive: WakingSpscQueue<EthernetFrame, AP_RX_DEPTH>,
    transmit: WakingSlotQueue<Option<Outgoing<ApMemory>>, AP_TX_DEPTH>,
    returned: WakingSlotQueue<Option<TxEnded<ApMemory>>, AP_TX_DEPTH>,
    frames: ApFrames,
    installed: bool,
    link: ApLink,
}

impl ApNetworkStorage {
    pub const fn new() -> Self {
        Self {
            receive: WakingSpscQueue::new(),
            transmit: WakingSlotQueue::new(),
            returned: WakingSlotQueue::new(),
            frames: ApFrames::new(),
            installed: false,
            link: ApLink::new(),
        }
    }

    /// Split the storage into the network's device, its transmit half and
    /// the runner's end. Every frame starts free, the network's.
    pub fn split<F: oer_protocols_network_port::EthernetFrame>(
        &mut self,
    ) -> (ApNetDevice<'_>, ApNetTransmit<'_, F>, ApNetworkEnd<'_>) {
        if !self.installed {
            for frame in &self.frames.frames {
                // A fresh storage: every cell is empty.
                let _installed = frame.install(EthernetFrame::empty());
            }
            self.installed = true;
        }
        let (receive_sender, receive_receiver) = self.receive.split();
        let (transmit_sender, transmit_receiver) = self.transmit.split(|| None);
        let (returned_sender, returned_receiver) = self.returned.split(|| None);
        let link = &self.link;
        let frames = &self.frames;
        let mut free = heapless::Vec::new();
        for place in 0..AP_TX_DEPTH {
            // Below AP_TX_DEPTH places: each fits.
            if let Ok(place) = u8::try_from(place) {
                let _fits = free.push(place);
            }
        }
        (
            ApNetDevice {
                receive: receive_receiver,
                link,
                observed: LinkSnapshot::DOWN,
                marked: 0,
            },
            DeviceTransmit::with_link(
                transmit_sender,
                returned_receiver,
                ApSupply {
                    frames,
                    free,
                    refused: false,
                    frame: PhantomData,
                },
                link,
            ),
            ApNetworkEnd {
                receive: receive_sender,
                transmit: transmit_receiver,
                returned: returned_sender,
                frames,
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

impl LinkView for ApLink {
    fn snapshot(&self) -> LinkSnapshot {
        self.snapshot.get()
    }
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

/// The runner's end: it hands the network received MSDUs, takes the
/// network's frames and gives them back once sent.
pub struct ApNetworkEnd<'a> {
    receive: WakingSpscSender<'a, EthernetFrame, AP_RX_DEPTH>,
    pub(crate) transmit: SlotReceiver<'a, Option<Outgoing<ApMemory>>, AP_TX_DEPTH>,
    returned: SlotSender<'a, Option<TxEnded<ApMemory>>, AP_TX_DEPTH>,
    frames: &'a ApFrames,
    link: &'a ApLink,
}

impl<'a> ApNetworkEnd<'a> {
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
    pub fn take(&mut self) -> Option<Outgoing<ApMemory>> {
        self.transmit.try_receive_with(Option::take).ok().flatten()
    }

    /// Whether a frame of the network waits.
    pub fn waiting(&self) -> bool {
        !self.transmit.is_empty()
    }

    /// The frames the network's memory holds.
    pub const fn frames(&self) -> &'a ApFrames {
        self.frames
    }

    /// Give `memory` back to the network: its frame ended, sent or not.
    /// The network has at most [`AP_TX_DEPTH`] frames out, so the queue
    /// back always has room; a broken one keeps the memory quarantined.
    pub fn give_back(&mut self, memory: ApMemory) {
        let mut ended = Some(TxEnded { memory });
        let _sent = self.returned.try_send_with(|slot| {
            *slot = ended.take();
            Ok::<_, core::convert::Infallible>(())
        });
        if let Some(ended) = ended {
            let _quarantined = core::mem::ManuallyDrop::new(ended);
        }
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
/// frames `F`: the same transmit end as a station's, over the access
/// point's frames.
pub type ApNetTransmit<'a, F> =
    DeviceTransmit<'a, F, ApSupply<'a, F>, AP_TX_DEPTH, SameHart, ApLink>;

/// The access point's frames as the network's transmit memory: a frame is
/// copied into a free one.
pub struct ApSupply<'a, F> {
    frames: &'a ApFrames,
    free: heapless::Vec<u8, AP_TX_DEPTH>,
    /// The last write found no free frame.
    refused: bool,
    frame: PhantomData<fn(F)>,
}

impl<F: oer_protocols_network_port::EthernetFrame> TxSupply<F> for ApSupply<'_, F> {
    type Memory = ApMemory;

    fn available(&self) -> bool {
        !self.refused || !self.free.is_empty()
    }

    fn write(&mut self, mut frame: F) -> Result<ApMemory, TxRefused<F>> {
        let Some(place) = self.free.pop() else {
            self.refused = true;
            return Err(TxRefused::Kept(TransmitRefused {
                frame,
                reason: TransmitError::QueueFull,
            }));
        };
        let copied = match frame.view() {
            Ok(view) => self
                .frames
                .frames
                .get(usize::from(place))
                .and_then(|cell| {
                    cell.lend(|slot| {
                        slot.set_parts(MsduParts {
                            destination: view.destination,
                            source: view.source,
                            ether_type: view.ether_type,
                            payload: view.payload,
                        })
                        .map_err(|_| FrameLengthError::TooLong)
                    })
                    .ok()
                })
                .unwrap_or(Err(FrameLengthError::TooLong)),
            Err(error) => Err(error),
        };
        match copied {
            Ok(()) => {
                self.refused = false;
                Ok(ApMemory(place))
            }
            Err(error) => {
                // Nothing was taken: the frame stays free.
                let _fits = self.free.push(place);
                Err(TxRefused::Kept(TransmitRefused {
                    frame,
                    reason: TransmitError::InvalidLength(error),
                }))
            }
        }
    }

    fn give(&mut self, memory: ApMemory) {
        // Every frame is one of these: the vector holds them all.
        let _fits = self.free.push(memory.0);
    }
}
