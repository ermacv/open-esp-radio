//! The access point's network end: the MSDUs its authorized peers send the
//! network, copied, and the network's frames for the BSS, through the same
//! transmit owner as a station's ([`LocalTx`]): the network writes each
//! frame in one of its transmit buffers and the owner publishes it, with its
//! length and the priority its marking requests, to the runner's queue
//! ([`Outgoing`]); the runner queues it per peer and TID, writes the
//! frame's MPDU in place in that same memory and sends it from there, and
//! gives the memory back ([`TxEnded`]) once the MPDU's terminal came, free
//! again. A frame no buffer or no room takes is refused before it is
//! written, and a stop leaves nothing lent.

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

use crate::station::{LinkView, LocalTx, LocalTxState, Outgoing, SameHart, TxEnded, TxPool};

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

    /// Split the storage into the network's device, its transmit owner
    /// over `pool` and its free buffers `free`, kept in `state`, and the
    /// runner's end. The network holds at most `N` buffers, as the type of
    /// `free` bounds them: the runner's queue back holds `N`, which this
    /// proves at compile time, so every memory the runner gives back finds
    /// room.
    pub fn split<'a, P: TxPool<Memory = M>, const N: usize>(
        &'a mut self,
        pool: P,
        state: &'a mut LocalTxState<P::Slot, N>,
        free: heapless::Vec<P::Slot, N>,
    ) -> (
        ApNetDevice<'a>,
        ApNetTransmit<'a, P, N>,
        ApNetworkEnd<'a, M>,
    ) {
        const {
            if N > AP_TX_DEPTH {
                panic!("the runner's queue back holds every network memory");
            }
        }
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
            LocalTx::new(transmit_sender, returned_receiver, pool, state, free, link),
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

/// The network's transmit owner into the BSS over the pool `P` of `N`
/// buffers: the same owner as a station's, over the access point's link and
/// its queue depth.
pub type ApNetTransmit<'a, P, const N: usize> = LocalTx<'a, P, N, AP_TX_DEPTH, SameHart, ApLink>;

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::panic,
    reason = "tests assert on fixed fixtures"
)]
mod tests {
    extern crate std;

    use super::*;
    use crate::station::PoolRefused;
    use oer_protocols_network_port::TxStorage;

    /// A buffer of one Ethernet header's octets, named by its place.
    struct Byte(u8, [u8; 14]);
    impl TxStorage for Byte {
        fn bytes_mut(&mut self) -> &mut [u8] {
            &mut self.1
        }
    }

    /// A pool whose memory is the buffer's place.
    struct Places;
    impl TxPool for Places {
        type Slot = Byte;
        type Memory = u8;
        fn place(slot: &Byte) -> u8 {
            slot.0
        }
        fn publish(&mut self, slot: Byte, _: usize) -> Result<u8, PoolRefused<Byte>> {
            Ok(slot.0)
        }
        fn slot(&mut self, memory: u8) -> Byte {
            Byte(memory, [0; 14])
        }
    }

    /// The queue back holds every memory of the storage the network was
    /// built over; one past it, which no real network has, comes back whole
    /// to its giver, never kept out of reach.
    #[test]
    fn a_memory_the_queue_back_refuses_comes_back_whole() {
        let storage = std::boxed::Box::leak(std::boxed::Box::new(ApNetworkStorage::<u8>::new()));
        let state = std::boxed::Box::leak(std::boxed::Box::new(LocalTxState::new()));
        let free: heapless::Vec<Byte, AP_TX_DEPTH> = (0..AP_TX_DEPTH)
            .map(|place| Byte(u8::try_from(place).unwrap(), [0; 14]))
            .collect();
        let (_device, _transmit, mut end) = storage.split(Places, state, free);
        for memory in 0..AP_TX_DEPTH {
            assert_eq!(end.give_back(u8::try_from(memory).unwrap()), Ok(()));
        }
        assert_eq!(end.give_back(200), Err(200), "the same memory, whole");
    }

    /// A memory whose drop is counted: one dropped is one lost.
    struct Tracked {
        place: u8,
        drops: std::sync::Arc<core::sync::atomic::AtomicUsize>,
    }
    impl Drop for Tracked {
        fn drop(&mut self) {
            self.drops
                .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        }
    }

    /// A buffer of one Ethernet header's octets around its memory.
    struct Tracking(Tracked, [u8; 14]);
    impl TxStorage for Tracking {
        fn bytes_mut(&mut self) -> &mut [u8] {
            &mut self.1
        }
    }

    /// A pool that publishes a buffer's memory and wraps it again.
    struct Wraps;
    impl TxPool for Wraps {
        type Slot = Tracking;
        type Memory = Tracked;
        fn place(slot: &Tracking) -> u8 {
            slot.0.place
        }
        fn publish(&mut self, slot: Tracking, _: usize) -> Result<Tracked, PoolRefused<Tracking>> {
            Ok(slot.0)
        }
        fn slot(&mut self, memory: Tracked) -> Tracking {
            Tracking(memory, [0; 14])
        }
    }

    /// The network's transmit owner ends (its service retired) while the
    /// runner holds every one of its memories, half of them given back
    /// before the end and half after: every one stays in the queue back,
    /// none refused and none dropped.
    #[test]
    fn a_memory_returned_after_the_networks_end_went_is_not_lost() {
        use oer_protocols_network_port::{FrameMeta, OwnedTransmit};
        let drops = std::sync::Arc::new(core::sync::atomic::AtomicUsize::new(0));
        let storage =
            std::boxed::Box::leak(std::boxed::Box::new(ApNetworkStorage::<Tracked>::new()));
        let state = std::boxed::Box::leak(std::boxed::Box::new(LocalTxState::new()));
        let free: heapless::Vec<Tracking, AP_TX_DEPTH> = (0..AP_TX_DEPTH)
            .map(|place| {
                Tracking(
                    Tracked {
                        place: u8::try_from(place).unwrap(),
                        drops: drops.clone(),
                    },
                    [0; 14],
                )
            })
            .collect();
        let (_device, mut transmit, mut end) = storage.split(Wraps, state, free);
        end.set_link(true);
        let meta = FrameMeta {
            length: 14,
            dscp: None,
        };
        for _ in 0..AP_TX_DEPTH {
            assert!(
                transmit
                    .publish(meta, |_| Ok::<_, core::convert::Infallible>(()))
                    .is_ok()
            );
        }
        transmit.flush();
        let mut held = std::vec::Vec::new();
        while let Some(outgoing) = end.take() {
            held.push(outgoing.memory);
        }
        assert_eq!(held.len(), AP_TX_DEPTH, "the runner holds every memory");
        let after = held.split_off(AP_TX_DEPTH / 2);
        for memory in held {
            assert!(end.give_back(memory).is_ok());
        }
        // The network's service retired: its transmit owner is gone.
        drop(transmit);
        for memory in after {
            assert!(
                end.give_back(memory).is_ok(),
                "a memory finds room after the network's end"
            );
        }
        assert_eq!(
            drops.load(core::sync::atomic::Ordering::Relaxed),
            0,
            "no memory was dropped"
        );
    }
}
