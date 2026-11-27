//! The places of the radio's chains: a queue's and a long chain's, each the
//! contract's sum of the states it may be in. A holder lends a place by
//! `&mut`; the place answers the proof of its state ([`QueuePlace::state`],
//! [`LongPlace::state`]), and a transition is the proof's method, which
//! calls the port's sealed primitive and writes the place's next state. No
//! holder takes a right out of a place or puts one back: the contract,
//! which alone builds a right, writes the next state from the last.

use core::marker::PhantomData;

use oer_portable_time::Instant;
use oer_protocols_ieee80211_wire::qos::{Tid, WmmAccessCategory};

use super::{
    AggregatePpdu, ArenaOutcome, Brand, Complete, DataPort, Ended, FrameSet, HEADROOM, HeaderLen,
    Layout, LongChain, LongIn, QUEUES, Queue, Sealed, TxFacts,
};
use crate::{GateOpen, InterfaceRole, MpduLength, PortRate, RetryPlan, SingleOutcome};

/// How a queue's chain of one goes: its one frame as one MPDU, of a kind
/// that names the response it solicits. A chain of one is never an
/// aggregate.
pub enum OnePpdu<'id, K> {
    /// Acknowledged, by this plan: data, or a frame a role writes to an
    /// individual receiver (a directed Probe Request, EAPOL).
    Acked(RetryPlan<PortRate<'id, K>>),
    /// Individually addressed, soliciting no ACK and saying so, at a rate
    /// that carries one without it.
    NoAck {
        /// Its checked unacknowledged header and its MPDU's length.
        frame: crate::SizedFrame<crate::NoAckHeader, K>,
        /// The rate.
        rate: crate::SupportedNoAck<'id, K>,
    },
    /// Group-addressed, soliciting no response.
    Group {
        /// Its checked group receiver, header and MPDU length.
        frame: crate::SizedFrame<crate::HeaderSpec<oer_protocols_ieee802_wire::GroupAddress>, K>,
        /// The rate.
        rate: PortRate<'id, K>,
    },
    /// A BlockAckReq, soliciting a BlockAck, at a non-HT rate.
    BlockAckRequest {
        /// The peer asked to return its Block Ack state.
        receiver: oer_protocols_ieee802_wire::IndividualAddress,
        /// The address of the requesting transmitter.
        transmitter: oer_protocols_ieee802_wire::Eui48,
        /// The traffic identifier of the agreement.
        tid: Tid,
        /// The first sequence number requested in the window.
        starting_sequence: oer_protocols_ieee80211_wire::sequence::SequenceNumber,
        /// The non-HT rates and retry budget of the request.
        plan: RetryPlan<crate::LegacyPortRate<'id, K>>,
    },
}

/// How a long chain goes: its first frame as one MPDU, or an aggregate.
pub enum LongPpdu<'id, K> {
    /// The chain's first frame as one MPDU, acknowledged, by this plan.
    Single(RetryPlan<PortRate<'id, K>>),
    /// The longest prefix the rate admits as an A-MPDU; if that prefix is
    /// one frame, that frame as one MPDU by its `alone`.
    Aggregate(AggregatePpdu<'id, K>),
}

impl<K> Clone for OnePpdu<'_, K> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<K> Copy for OnePpdu<'_, K> {}
impl<K> Clone for LongPpdu<'_, K> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<K> Copy for LongPpdu<'_, K> {}

/// The state of a queue's place.
#[derive(Debug)]
enum QueueState<T> {
    /// Free, its chain of one idle: the queue is here.
    Free,
    /// Its chain of one on the air, with what its holder keeps of it.
    One(T),
    /// Carried by a long chain on the air, which holds the queue.
    Carried,
}

/// A queue's place, of the queue of its category: free with its queue, its
/// chain of one on the air (with `T`, what its holder keeps of that frame),
/// or carried by a long chain on the air. A holder has no way to the queue
/// itself but through the place's transitions
/// (`tests/ui/a_queue_stays_in_its_place.rs`).
#[derive(Debug)]
pub struct QueuePlace<'id, T = ()> {
    category: WmmAccessCategory,
    state: QueueState<T>,
    brand: Brand<'id>,
}

/// What a queue's place is now, read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueueIs {
    /// Free, its chain of one idle.
    Free,
    /// Its chain of one on the air.
    One,
    /// Carried by a long chain on the air.
    Carried,
}

/// The proof of a queue's place's state, borrowing the place: its
/// transitions are the proof's.
#[derive(Debug)]
pub enum QueueNow<'p, 'id, T> {
    /// Free: its chain of one may go, or a long chain may take it.
    Free(FreeQueue<'p, 'id, T>),
    /// Its chain of one is on the air.
    One(OneOnAir<'p, 'id, T>),
    /// Carried: the long chain on the air gives it back at its settlement.
    Carried,
}

/// A free queue's place.
#[derive(Debug)]
pub struct FreeQueue<'p, 'id, T> {
    place: &'p mut QueuePlace<'id, T>,
}

/// A queue's place whose chain of one is on the air, with what its holder
/// keeps of that frame.
#[derive(Debug)]
pub struct OneOnAir<'p, 'id, T> {
    place: &'p mut QueuePlace<'id, T>,
    data: T,
}

impl<'id, T: Copy> QueuePlace<'id, T> {
    const fn free(category: WmmAccessCategory) -> Self {
        Self {
            category,
            state: QueueState::Free,
            brand: PhantomData,
        }
    }

    /// The queue's category.
    pub const fn category(&self) -> WmmAccessCategory {
        self.category
    }

    /// What the place is now.
    pub const fn now(&self) -> QueueIs {
        match self.state {
            QueueState::Free => QueueIs::Free,
            QueueState::One(_) => QueueIs::One,
            QueueState::Carried => QueueIs::Carried,
        }
    }

    /// What its holder keeps of its chain of one on the air.
    pub const fn one(&self) -> Option<&T> {
        match &self.state {
            QueueState::One(data) => Some(data),
            QueueState::Free | QueueState::Carried => None,
        }
    }

    /// The proof of the place's state.
    pub const fn state(&mut self) -> QueueNow<'_, 'id, T> {
        match self.state {
            QueueState::Free => QueueNow::Free(FreeQueue { place: self }),
            QueueState::One(data) => QueueNow::One(OneOnAir { place: self, data }),
            QueueState::Carried => QueueNow::Carried,
        }
    }

    /// The queue, as the contract rebuilds it for a primitive.
    const fn queue(&self) -> Queue<'id> {
        Queue {
            category: self.category,
            brand: PhantomData,
        }
    }
}

/// The one frame of a queue's chain of one: for `link` of its interface and
/// `tid`, its body by its handle, and its header, written into the headroom
/// the port lends.
pub struct OneFrame<'l, L, M, H> {
    /// The link it is sent on, of its interface.
    pub link: &'l L,
    /// Its traffic identifier.
    pub tid: Tid,
    /// Its body, by its memory's handle.
    pub body: M,
    /// Its header's writer.
    pub header: H,
}

impl<'p, 'id, T: Copy> FreeQueue<'p, 'id, T> {
    /// The queue's category.
    pub const fn category(&self) -> WmmAccessCategory {
        self.place.category
    }

    /// Send `frame` in the queue's chain of one, published as `how`;
    /// `data` is what the holder keeps of it until its end. A free chain of
    /// one takes its one frame: nothing is refused.
    pub fn send<I: InterfaceRole, P: DataPort<'id>, H>(
        self,
        port: &mut P,
        frame: OneFrame<'_, P::Link<I>, P::Memory, H>,
        how: OnePpdu<'id, P::Caps>,
        gate: &GateOpen<'id>,
        now: Instant,
        data: T,
    ) -> OneOnAir<'p, 'id, T>
    where
        H: FnOnce(&mut [u8; HEADROOM]) -> HeaderLen,
    {
        let OneFrame {
            link,
            tid,
            body,
            header,
        } = frame;
        let queue = self.place.queue();
        port.open_chain(Sealed::new(), &queue, link, tid);
        port.write_frame(Sealed::new(), &queue, body, header);
        port.publish_one(Sealed::new(), &queue, how, gate, now);
        self.place.state = QueueState::One(data);
        OneOnAir {
            place: self.place,
            data,
        }
    }
}

impl<'p, 'id, T: Copy> OneOnAir<'p, 'id, T> {
    /// The queue's category.
    pub const fn category(&self) -> WmmAccessCategory {
        self.place.category
    }

    /// What the holder keeps of the frame.
    pub const fn data(&self) -> &T {
        &self.data
    }

    /// The frame's end, read in place, once it ended: pulled.
    pub fn ended<'a, P: DataPort<'id>>(
        &self,
        port: &'a mut P,
        facts: TxFacts,
        now: Instant,
    ) -> Option<&'a Ended<SingleOutcome<P::Ack, P::Denial>>> {
        port.ended_one(Sealed::new(), &self.place.queue(), facts, now)
    }

    /// Settle the ended frame: its body goes home and the queue is free
    /// again. What the holder kept of it comes back.
    pub fn settle<P: DataPort<'id>>(
        self,
        port: &mut P,
        home: &mut impl FnMut(P::Memory),
    ) -> (FreeQueue<'p, 'id, T>, T) {
        let queue = self.place.queue();
        port.settle_one(Sealed::new(), &queue, home);
        self.place.state = QueueState::Free;
        (FreeQueue { place: self.place }, self.data)
    }
}

/// The four queues' places, a category each, reached by its category
/// through a total match.
#[derive(Debug)]
pub struct QueuePlaces<'id, T = ()> {
    /// In `WmmAccessCategory::select`'s order.
    places: [QueuePlace<'id, T>; 4],
}

impl<'id, T: Copy> QueuePlaces<'id, T> {
    /// The places of the radio's four queues, each free. The four rights
    /// are the radio's four, one a category: the installation has no other.
    pub const fn new(queues: [Queue<'id>; 4]) -> Self {
        let [_, _, _, _] = queues;
        Self {
            places: [
                QueuePlace::free(WmmAccessCategory::BestEffort),
                QueuePlace::free(WmmAccessCategory::Background),
                QueuePlace::free(WmmAccessCategory::Video),
                QueuePlace::free(WmmAccessCategory::Voice),
            ],
        }
    }

    /// The place of the queue of `category`.
    pub const fn at(&self, category: WmmAccessCategory) -> &QueuePlace<'id, T> {
        category.select(&self.places)
    }

    /// The same, lent.
    pub const fn at_mut(&mut self, category: WmmAccessCategory) -> &mut QueuePlace<'id, T> {
        category.select_mut(&mut self.places)
    }

    /// Each place.
    pub fn each(&self) -> impl Iterator<Item = &QueuePlace<'id, T>> {
        self.places.iter()
    }

    /// The four queues back, in [`QUEUES`]' order, when every place is
    /// free: the data's stop. The places, while one is not: the owner stays
    /// where it was.
    ///
    /// # Errors
    ///
    /// A place's chain of one is on the air, or its queue carried.
    pub fn into_queues(self) -> Result<[Queue<'id>; 4], Self> {
        if self
            .places
            .iter()
            .all(|place| matches!(place.state, QueueState::Free))
        {
            Ok(QUEUES.map(|category| Queue {
                category,
                brand: PhantomData,
            }))
        } else {
            Err(self)
        }
    }
}

/// The state of a long chain's place.
#[derive(Clone, Copy, Debug)]
enum LongState {
    /// It holds nothing.
    Idle,
    /// Open, holding `frames`.
    Open { frames: u8 },
    /// On the air on the queue of `category`, which it carries: its first
    /// `sent` frames went.
    OnAir {
        sent: u8,
        category: WmmAccessCategory,
    },
}

/// A long chain's place, of the marker `M` of its layout's set: idle, open
/// with its frames, or on the air with the queue it carries.
pub struct LongPlace<'id, M, const SLOTS: usize> {
    state: LongState,
    brand: Brand<'id>,
    marker: PhantomData<fn() -> M>,
}

impl<M, const SLOTS: usize> core::fmt::Debug for LongPlace<'_, M, SLOTS> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("LongPlace")
            .field("marker", &core::any::type_name::<M>())
            .field("state", &self.state)
            .finish()
    }
}

/// What a long chain's place is now, read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LongIs {
    /// It holds nothing.
    Idle,
    /// Open, holding this many frames.
    Open(u8),
    /// On the air: its prefix that went, and its queue.
    OnAir(u8, WmmAccessCategory),
}

/// The proof of a long chain's place's state, borrowing the place.
pub enum LongNow<'p, 'id, M, const SLOTS: usize> {
    /// Idle.
    Idle(IdleLong<'p, 'id, M, SLOTS>),
    /// Open.
    Open(OpenLong<'p, 'id, M, SLOTS>),
    /// On the air.
    OnAir(LongOnAir<'p, 'id, M, SLOTS>),
}

/// An idle long chain's place.
pub struct IdleLong<'p, 'id, M, const SLOTS: usize> {
    place: &'p mut LongPlace<'id, M, SLOTS>,
}

/// An open long chain's place, with its frames.
pub struct OpenLong<'p, 'id, M, const SLOTS: usize> {
    place: &'p mut LongPlace<'id, M, SLOTS>,
    frames: u8,
}

/// A long chain's place on the air.
pub struct LongOnAir<'p, 'id, M, const SLOTS: usize> {
    place: &'p mut LongPlace<'id, M, SLOTS>,
    sent: u8,
    category: WmmAccessCategory,
}

/// What a long chain's settlement leaves.
pub enum LongSettled<'p, 'id, M, const SLOTS: usize> {
    /// The kept frames stay in their places for the retry.
    Kept(OpenLong<'p, 'id, M, SLOTS>),
    /// The chain holds nothing.
    Idle(IdleLong<'p, 'id, M, SLOTS>),
}

impl<'id, M, const SLOTS: usize> LongPlace<'id, M, SLOTS> {
    /// The place of the idle long chain `chain`.
    pub const fn new(chain: LongChain<'id, M, SLOTS>) -> Self {
        let LongChain { brand, marker } = chain;
        Self {
            state: LongState::Idle,
            brand,
            marker,
        }
    }

    /// What the place is now.
    pub const fn now(&self) -> LongIs {
        match self.state {
            LongState::Idle => LongIs::Idle,
            LongState::Open { frames } => LongIs::Open(frames),
            LongState::OnAir { sent, category } => LongIs::OnAir(sent, category),
        }
    }

    /// The proof of the place's state.
    pub const fn state(&mut self) -> LongNow<'_, 'id, M, SLOTS> {
        match self.state {
            LongState::Idle => LongNow::Idle(IdleLong { place: self }),
            LongState::Open { frames } => LongNow::Open(OpenLong {
                place: self,
                frames,
            }),
            LongState::OnAir { sent, category } => LongNow::OnAir(LongOnAir {
                place: self,
                sent,
                category,
            }),
        }
    }

    /// The idle long chain back, when its place is idle: the data's stop.
    /// The place, while it is not: the owner stays where it was.
    ///
    /// # Errors
    ///
    /// The chain is open or on the air.
    pub const fn into_idle(self) -> Result<LongChain<'id, M, SLOTS>, Self> {
        match self.state {
            LongState::Idle => Ok(LongChain {
                brand: PhantomData,
                marker: PhantomData,
            }),
            LongState::Open { .. } | LongState::OnAir { .. } => Err(self),
        }
    }

    /// The chain, as the contract rebuilds it for a primitive.
    const fn chain(&self) -> LongChain<'id, M, SLOTS> {
        LongChain {
            brand: PhantomData,
            marker: PhantomData,
        }
    }
}

impl<'p, 'id, M, const SLOTS: usize> IdleLong<'p, 'id, M, SLOTS> {
    /// Open the chain for `link` of the interface `I` and `tid`.
    pub fn open<I: InterfaceRole, P: DataPort<'id>>(
        self,
        port: &mut P,
        link: &P::Link<I>,
        tid: Tid,
    ) -> OpenLong<'p, 'id, M, SLOTS>
    where
        M: LongIn<<P::Layout as Layout>::Longs>,
    {
        port.open_chain(Sealed::new(), &self.place.chain(), link, tid);
        self.place.state = LongState::Open { frames: 0 };
        OpenLong {
            place: self.place,
            frames: 0,
        }
    }
}

/// A frame's place in an open long chain, borrowed for one write.
#[must_use = "a place is written once"]
pub struct FramePlace<'a, 'p, 'id, P, M, const SLOTS: usize> {
    port: &'a mut P,
    open: &'a mut OpenLong<'p, 'id, M, SLOTS>,
}

impl<'p, 'id, M, const SLOTS: usize> OpenLong<'p, 'id, M, SLOTS> {
    /// The frames it holds.
    pub const fn frames(&self) -> u8 {
        self.frames
    }

    /// The next frame's place of `length`, or [`Complete`] when its
    /// descriptors are full.
    ///
    /// # Errors
    ///
    /// The chain's descriptors are full.
    pub fn place<'a, P: DataPort<'id>>(
        &'a mut self,
        port: &'a mut P,
        length: MpduLength<P::Caps>,
    ) -> Result<FramePlace<'a, 'p, 'id, P, M, SLOTS>, Complete>
    where
        M: LongIn<<P::Layout as Layout>::Longs>,
    {
        port.place_frame(Sealed::new(), &self.place.chain(), self.frames, length)?;
        Ok(FramePlace { port, open: self })
    }

    /// Withdraw its frames: each body goes home, and the chain is idle.
    pub fn withdraw<P: DataPort<'id>>(
        self,
        port: &mut P,
        home: &mut impl FnMut(P::Memory),
    ) -> IdleLong<'p, 'id, M, SLOTS>
    where
        M: LongIn<<P::Layout as Layout>::Longs>,
    {
        port.withdraw_chain(Sealed::new(), &self.place.chain(), home);
        self.place.state = LongState::Idle;
        IdleLong { place: self.place }
    }

    /// Publish the chain on the free queue `queue` as `how` says: a single
    /// MPDU its first frame; an aggregate the longest prefix of its frames
    /// the rate admits, one frame at least. The others stay in their places
    /// with no attempt spent. The queue's place is carried until the
    /// settlement.
    pub fn publish<T: Copy, P: DataPort<'id>>(
        self,
        queue: FreeQueue<'_, 'id, T>,
        port: &mut P,
        how: LongPpdu<'id, P::Caps>,
        gate: &GateOpen<'id>,
        now: Instant,
    ) -> LongOnAir<'p, 'id, M, SLOTS>
    where
        M: LongIn<<P::Layout as Layout>::Longs>,
    {
        let category = queue.place.category;
        let sent = port.publish_long(
            Sealed::new(),
            &self.place.chain(),
            &queue.place.queue(),
            how,
            gate,
            now,
        );
        queue.place.state = QueueState::Carried;
        self.place.state = LongState::OnAir { sent, category };
        LongOnAir {
            place: self.place,
            sent,
            category,
        }
    }
}

impl<'id, P: DataPort<'id>, M: LongIn<<P::Layout as Layout>::Longs>, const SLOTS: usize>
    FramePlace<'_, '_, 'id, P, M, SLOTS>
{
    /// Write the frame: the body by its handle, in place, the header once
    /// into the headroom the port lends.
    pub fn write(self, body: P::Memory, header: impl FnOnce(&mut [u8; HEADROOM]) -> HeaderLen) {
        let Self { port, open } = self;
        port.write_frame(Sealed::new(), &open.place.chain(), body, header);
        open.frames = open.frames.saturating_add(1);
        open.place.state = LongState::Open {
            frames: open.frames,
        };
    }
}

impl<'p, 'id, M, const SLOTS: usize> LongOnAir<'p, 'id, M, SLOTS> {
    /// The queue it is on.
    pub const fn category(&self) -> WmmAccessCategory {
        self.category
    }

    /// The frames of the prefix that went; the others stay in their places
    /// with no attempt spent.
    pub const fn sent(&self) -> FrameSet {
        FrameSet::prefix(self.sent)
    }

    /// How many went: what went is always a prefix.
    pub const fn went(&self) -> u8 {
        self.sent
    }

    /// The chain's end, read in place, once it ended: pulled.
    pub fn ended<'a, P: DataPort<'id>>(
        &self,
        port: &'a mut P,
        facts: TxFacts,
        now: Instant,
    ) -> Option<&'a Ended<ArenaOutcome<P::Ack, P::BlockAck, P::Denial>>>
    where
        M: LongIn<<P::Layout as Layout>::Longs>,
    {
        port.ended_long(Sealed::new(), &self.place.chain(), self.sent, facts, now)
    }

    /// Settle the ended chain: the frames of `keep` stay in their places
    /// for a retry, every other body goes home, and its queue's place is
    /// free again.
    pub fn settle<T: Copy, P: DataPort<'id>>(
        self,
        port: &mut P,
        keep: FrameSet,
        home: &mut impl FnMut(P::Memory),
        queues: &mut QueuePlaces<'id, T>,
    ) -> LongSettled<'p, 'id, M, SLOTS>
    where
        M: LongIn<<P::Layout as Layout>::Longs>,
    {
        let kept = port.settle_chain(Sealed::new(), &self.place.chain(), self.sent, keep, home);
        // The place of the queue it carried: the contract knows its
        // category and that it is carried.
        queues.at_mut(self.category).state = QueueState::Free;
        if kept == 0 {
            self.place.state = LongState::Idle;
            LongSettled::Idle(IdleLong { place: self.place })
        } else {
            self.place.state = LongState::Open { frames: kept };
            LongSettled::Kept(OpenLong {
                place: self.place,
                frames: kept,
            })
        }
    }
}
