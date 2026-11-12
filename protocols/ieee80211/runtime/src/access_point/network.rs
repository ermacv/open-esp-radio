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
    BatchEnd, CopyReason, EthernetFrameView, FrameStorage, Incoming, LentFrame, LinkSnapshot,
    LinkState, NetDevice, ReceivedBatch, StorageAdvanceRefused,
};

use crate::station::{DeviceTransmit, LinkView, Outgoing, SameHart, TxEnded, TxSupply};

/// The received MSDUs the queue holds for the network.
pub const AP_RX_DEPTH: usize = 4;

/// The depth of the queues to and from the runner, and the network's
/// transmit memories the access point holds at most: a composition gives
/// the network no more transmit memory than this, so the queue back always
/// has room for every memory the runner returns.
pub const AP_TX_DEPTH: usize = 64;

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
    /// the network's transmit memory, which takes the stack's frames `F`,
    /// and the runner's end. The memory is the bounded storage `memories`,
    /// at most `N` of them, which `supply` builds the network's transmit
    /// memory over: the runner's queue back holds `N`, which this proves at
    /// compile time, so every memory the runner gives back finds room.
    pub fn split<F, S: TxSupply<F, Memory = M>, T, const N: usize>(
        &mut self,
        memories: heapless::Vec<T, N>,
        supply: impl FnOnce(heapless::Vec<T, N>) -> S,
    ) -> (
        ApNetDevice<'_>,
        ApNetTransmit<'_, F, S>,
        ApNetworkEnd<'_, M>,
    ) {
        const {
            if N > AP_TX_DEPTH {
                panic!("the runner's queue back holds every network memory");
            }
        }
        let supply = supply(memories);
        let (receive_sender, receive_receiver) = self.receive.split();
        let (transmit_sender, transmit_receiver) = self.transmit.split(|| None);
        let (returned_sender, returned_receiver) = self.returned.split(|| None);
        let link = &self.link;
        (
            ApNetDevice {
                receive: receive_receiver,
                link,
                observed: LinkSnapshot::DOWN,
            },
            DeviceTransmit::with_link(transmit_sender, returned_receiver, supply, link),
            ApNetworkEnd {
                receive: receive_sender,
                transmit: transmit_receiver,
                returned: returned_sender,
                link,
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

    /// Give `memory` back to the network: its frame ended, sent or not.
    /// The queue back holds every network memory ([`ApNetworkStorage::split`]),
    /// so only a broken queue refuses it.
    ///
    /// # Errors
    ///
    /// The queue refused it: the memory comes back whole.
    pub fn give_back(&mut self, memory: M) -> Result<(), M> {
        let mut ended = Some(TxEnded { memory });
        let _sent = self.returned.try_send_with(|slot| {
            *slot = ended.take();
            Ok::<_, core::convert::Infallible>(())
        });
        match ended {
            None => Ok(()),
            Some(ended) => Err(ended.memory),
        }
    }
}

/// The network stack's device: the link and the received MSDUs.
pub struct ApNetDevice<'a> {
    receive: WakingSpscReceiver<'a, EthernetFrame, AP_RX_DEPTH>,
    link: &'a ApLink,
    /// The link the stack last observed.
    observed: LinkSnapshot,
}

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

impl NetDevice for ApNetDevice<'_> {
    type Storage = NoStorage;
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
    /// Every MSDU is the device's own copy, shown in place and released.
    fn receive(
        &mut self,
        limit: usize,
        mut take: impl FnMut(Incoming<'_, LentFrame<NoStorage>>),
    ) -> ReceivedBatch {
        let mut taken = 0_usize;
        let end = loop {
            if taken >= limit {
                break BatchEnd::Limit;
            }
            let Some(frame) = self.receive.try_receive() else {
                break BatchEnd::Drained;
            };
            taken = taken.saturating_add(1);
            let MsduParts {
                destination,
                source,
                ether_type,
                payload,
            } = frame.parts();
            take(Incoming::Copied {
                frame: Ok(EthernetFrameView {
                    destination,
                    source,
                    ether_type,
                    payload,
                }),
                // The access point's device lends no buffer at all.
                reason: CopyReason::Format,
            });
        };
        ReceivedBatch { taken, end }
    }
}

/// The network stack's sending path into the BSS, taking the stack's
/// frames `F` into the network's transmit memory `S`: the same transmit end
/// as a station's.
pub type ApNetTransmit<'a, F, S> = DeviceTransmit<'a, F, S, AP_TX_DEPTH, SameHart, ApLink>;

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "tests assert on fixed fixtures"
)]
mod tests {
    extern crate std;

    use super::*;
    use crate::station::TxRefused;
    use oer_protocols_ieee80211_core_roles_sta::data::EthernetFrame;
    use oer_protocols_network_port::{TransmitError, TransmitRefused};

    /// A supply that copies nothing and holds the memories it was built on.
    struct Held(heapless::Vec<u8, AP_TX_DEPTH>);
    impl TxSupply<EthernetFrame> for Held {
        type Memory = u8;
        fn available(&self) -> bool {
            false
        }
        fn write(&mut self, frame: EthernetFrame) -> Result<u8, TxRefused<EthernetFrame>> {
            Err(TxRefused::Kept(TransmitRefused {
                frame,
                reason: TransmitError::QueueFull,
            }))
        }
        fn give(&mut self, memory: u8) {
            let _held = self.0.push(memory);
        }
    }

    /// The queue back holds every memory of the storage the network was
    /// built over; one past it, which no real network has, comes back whole
    /// to its giver, never kept out of reach.
    #[test]
    fn a_memory_the_queue_back_refuses_comes_back_whole() {
        let storage = std::boxed::Box::leak(std::boxed::Box::new(ApNetworkStorage::<u8>::new()));
        let memories: heapless::Vec<u8, AP_TX_DEPTH> = (0..AP_TX_DEPTH)
            .map(|memory| u8::try_from(memory).unwrap())
            .collect();
        let (_device, _transmit, mut end) =
            storage.split::<EthernetFrame, _, _, AP_TX_DEPTH>(memories, Held);
        for memory in 0..AP_TX_DEPTH {
            assert_eq!(end.give_back(u8::try_from(memory).unwrap()), Ok(()));
        }
        assert_eq!(end.give_back(200), Err(200), "the same memory, whole");
    }
}
