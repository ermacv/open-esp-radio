//! The access point's network end: the MSDUs its authorized peers send the
//! network stack, copied, and the stack's frames for the BSS, through the
//! same transmit boundary as a station's: the stack's sending path hands
//! the network's transmit memory its frame is in ([`TxSupply`]), with its
//! length and the priority its marking requests, to the runner's queue
//! ([`Outgoing`]); the runner queues it per peer and TID, writes the
//! frame's MPDU in place in that same memory and sends it from there, and
//! gives the memory back ([`TxEnded`]) once the MPDU's terminal came, free
//! again. A frame no memory or no room takes is refused at once with its
//! owner, and a stop leaves nothing lent.

use core::cell::Cell;
use core::future::Future;
use core::task::{Context, Poll};

use oer_portable_memory::{
    SlotReceiver, SlotSender, WakingSlotQueue, WakingSpscQueue, WakingSpscReceiver,
    WakingSpscSender,
};
use oer_portable_sync::PriorityWaker;
use oer_protocols_ieee80211_core_roles_sta::data::EthernetFrame;
use oer_protocols_ieee80211_core_roles_sta::data::MsduParts;
use oer_protocols_network_port::{
    EthernetFrameView, FrameLengthError, FrameStorage, LentFrame, LinkSnapshot, LinkState,
    NetDevice, ReceivedFrame, StorageAdvanceRefused,
};

use crate::station::{DeviceTransmit, LinkView, Outgoing, SameHart, TxEnded, TxSupply};

/// The received MSDUs the queue holds for the network.
pub const AP_RX_DEPTH: usize = 4;

/// The depth of the queues to and from the runner, and the network's
/// transmit memories the access point holds at most: a composition gives
/// the network no more transmit memory than this, so the queue back always
/// has room for every memory the runner returns.
pub const AP_TX_DEPTH: usize = 64;

/// The proof that the runner's queue back holds every memory a supply `S`
/// can hold ([`TxSupply::MEMORIES`]): [`Holds::BACK`] does not compile
/// otherwise. [`ApNetworkStorage::split`] reads it at its monomorphization.
pub struct Holds<F, S>(core::marker::PhantomData<fn(F, S)>);

impl<F, S: TxSupply<F>> Holds<F, S> {
    pub const BACK: () = {
        if S::MEMORIES > AP_TX_DEPTH {
            panic!("the runner's queue back holds every network memory");
        }
    };
}

/// The two queues and the link of one access point's network end, and the
/// queues between the stack and the runner, placed by the composition. `M`
/// is the network's transmit memory.
pub struct ApNetworkStorage<M> {
    receive: WakingSpscQueue<EthernetFrame, AP_RX_DEPTH>,
    transmit: WakingSlotQueue<Option<Outgoing<M>>, AP_TX_DEPTH>,
    returned: WakingSlotQueue<Option<TxEnded<M>>, AP_TX_DEPTH>,
    link: ApLink,
}

impl<M> ApNetworkStorage<M> {
    pub const fn new() -> Self {
        Self {
            receive: WakingSpscQueue::new(),
            transmit: WakingSlotQueue::new(),
            returned: WakingSlotQueue::new(),
            link: ApLink::new(),
        }
    }

    /// Split the storage into the network's device, its transmit half over
    /// the network's transmit memory `supply`, which takes the stack's
    /// frames `F`, and the runner's end.
    /// The runner's queue back holds every memory `supply` can hold
    /// ([`TxSupply::MEMORIES`], its bounded storage's capacity), which this
    /// proves at compile time, so every memory it gives back finds room.
    pub fn split<F, S: TxSupply<F, Memory = M>>(
        &mut self,
        supply: S,
    ) -> (
        ApNetDevice<'_>,
        ApNetTransmit<'_, F, S>,
        ApNetworkEnd<'_, M>,
    ) {
        let () = Holds::<F, S>::BACK;
        let (receive_sender, receive_receiver) = self.receive.split();
        let (transmit_sender, transmit_receiver) = self.transmit.split(|| None);
        let (returned_sender, returned_receiver) = self.returned.split(|| None);
        let link = &self.link;
        (
            ApNetDevice {
                receive: receive_receiver,
                link,
                observed: LinkSnapshot::DOWN,
                marked: 0,
            },
            DeviceTransmit::with_link(transmit_sender, returned_receiver, supply, link),
            ApNetworkEnd {
                receive: receive_sender,
                transmit: transmit_receiver,
                returned: returned_sender,
                link,
                quarantined: 0,
            },
        )
    }
}

impl<M> Default for ApNetworkStorage<M> {
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
/// network's frames in its memory `M` and gives the memory back once its
/// frame ended.
pub struct ApNetworkEnd<'a, M> {
    receive: WakingSpscSender<'a, EthernetFrame, AP_RX_DEPTH>,
    pub(crate) transmit: SlotReceiver<'a, Option<Outgoing<M>>, AP_TX_DEPTH>,
    returned: SlotSender<'a, Option<TxEnded<M>>, AP_TX_DEPTH>,
    link: &'a ApLink,
    /// Memories a broken queue back refused, kept out of reach.
    quarantined: u32,
}

impl<M> ApNetworkEnd<'_, M> {
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
    pub fn take(&mut self) -> Option<Outgoing<M>> {
        self.transmit.try_receive_with(Option::take).ok().flatten()
    }

    /// Whether a frame of the network waits.
    pub fn waiting(&self) -> bool {
        !self.transmit.is_empty()
    }

    /// Memories a broken queue back refused, kept out of reach: the queue
    /// holds every memory ([`ApNetworkStorage::split`]), so only a broken
    /// queue refuses one.
    pub const fn quarantined(&self) -> u32 {
        self.quarantined
    }

    /// Give `memory` back to the network: its frame ended, sent or not.
    /// The queue back holds every network memory, so it has room; a broken
    /// one keeps the memory quarantined and counted ([`Self::quarantined`]).
    pub fn give_back(&mut self, memory: M) {
        let mut ended = Some(TxEnded { memory });
        let _sent = self.returned.try_send_with(|slot| {
            *slot = ended.take();
            Ok::<_, core::convert::Infallible>(())
        });
        if let Some(ended) = ended {
            let _quarantined = core::mem::ManuallyDrop::new(ended);
            self.quarantined = self.quarantined.saturating_add(1);
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
/// frames `F` into the network's transmit memory `S`: the same transmit end
/// as a station's.
pub type ApNetTransmit<'a, F, S> = DeviceTransmit<'a, F, S, AP_TX_DEPTH, SameHart, ApLink>;
