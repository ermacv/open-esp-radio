//! A socket: what an application holds, and the lanes to the service.

use core::future::poll_fn;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use core::task::Poll;

use embassy_futures::select::{select, select3};
use oer_portable_memory::{AtomicWaker, WakingSpscQueue, WakingSpscReceiver, WakingSpscSender};
use oer_protocols_network_core::{Block, Datagram, Eviction, Outcome, Pool, Remote, SendRefusal};

/// The answers a socket can be owed at once: a block, a close and a stop.
const ANSWERS: usize = 3;

/// What a socket asks of the service.
pub(crate) enum Request<'m, B, const SIZE: usize> {
    /// A free block; a socket has one such request at a time.
    Reserve,
    /// A block back.
    Release(Block<'m, B, SIZE>),
    /// A datagram, under its credit's tag.
    Send {
        tag: u8,
        remote: Remote,
        block: Block<'m, B, SIZE>,
    },
    /// End the socket.
    Close,
}

/// What the service tells a socket.
pub(crate) enum Answer<'a, 'm, B, const SIZE: usize, const DEPTH: usize> {
    Reserved(Block<'m, B, SIZE>),
    /// The socket's close finished.
    Closed,
    /// The service stopped. What the request lane still holds, it never saw:
    /// the lane comes back.
    Stopped(WakingSpscReceiver<'a, Request<'m, B, SIZE>, DEPTH>),
}

/// How a datagram ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Sent {
    /// The core refused it; its block went back to the pool.
    Refused(SendRefusal),
    /// The core's outcome.
    Ended(Outcome),
}

/// Every [`Sent`], at the word [`word_of`] gives it, less [`FIRST_RESULT`].
const RESULTS: [Sent; 12] = [
    Sent::Refused(SendRefusal::Stale),
    Sent::Refused(SendRefusal::NoPlace),
    Sent::Refused(SendRefusal::TooLong),
    Sent::Refused(SendRefusal::NoRoute),
    Sent::Refused(SendRefusal::NeighborsFull),
    Sent::Ended(Outcome::Accepted),
    Sent::Ended(Outcome::Unreachable),
    Sent::Ended(Outcome::Closed),
    Sent::Ended(Outcome::SocketClosed),
    Sent::Ended(Outcome::Unknown),
    Sent::Ended(Outcome::Retired),
    Sent::Ended(Outcome::Evicted(Eviction::MemoryPressure)),
];

/// A place's command runs, and its observer may wait.
const WAITING: u32 = 0;
/// A place's observer has gone.
const GONE: u32 = 1;
/// The first word holding a result.
const FIRST_RESULT: u32 = 2;

/// The word of `sent`: exhaustive, so a new outcome does not compile unnamed.
const fn word_of(sent: Sent) -> u32 {
    match sent {
        Sent::Refused(SendRefusal::Stale) => 2,
        Sent::Refused(SendRefusal::NoPlace) => 3,
        Sent::Refused(SendRefusal::TooLong) => 4,
        Sent::Refused(SendRefusal::NoRoute) => 5,
        Sent::Refused(SendRefusal::NeighborsFull) => 6,
        Sent::Ended(Outcome::Accepted) => 7,
        Sent::Ended(Outcome::Unreachable) => 8,
        Sent::Ended(Outcome::Closed) => 9,
        Sent::Ended(Outcome::SocketClosed) => 10,
        Sent::Ended(Outcome::Unknown) => 11,
        Sent::Ended(Outcome::Retired) => 12,
        Sent::Ended(Outcome::Evicted(Eviction::MemoryPressure)) => 13,
    }
}

/// A credit's own place for its one result.
pub(crate) struct Place {
    word: AtomicU32,
    waker: AtomicWaker,
}

impl Place {
    const fn new() -> Self {
        Self {
            word: AtomicU32::new(WAITING),
            waker: AtomicWaker::new(),
        }
    }

    /// The command ended: its observer hears, or, gone, its credit frees.
    pub(crate) fn end(&self, sent: Sent, credits: &Credits, tag: u8) {
        if self.word.swap(word_of(sent), Ordering::AcqRel) == GONE {
            credits.give(tag);
        } else {
            self.waker.wake();
        }
    }

    pub(crate) fn wake(&self) {
        self.waker.wake();
    }
}

/// What a socket and its observers share with the service.
pub(crate) struct Credits {
    /// Tags free for a command.
    free: AtomicU32,
    /// The waiter for a free tag.
    waker: AtomicWaker,
    /// The service stopped, after writing every result it knows.
    stopped: AtomicBool,
}

impl Credits {
    /// Free `tag`, and tell who waits for one.
    fn give(&self, tag: u8) {
        self.free.fetch_or(tag_bit(tag), Ordering::AcqRel);
        self.waker.wake();
    }

    /// Wait until the service has ended.
    async fn stopped(&self) {
        poll_fn(|cx| {
            self.waker.register(cx.waker());
            if self.stopped.load(Ordering::Acquire) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
    }
}

/// Says the service ended, however it does: at its end, dropped before it
/// ran, or with its task cancelled. Everything the service wrote comes first.
pub(crate) struct Ending<'a, const CREDITS: usize> {
    credits: &'a Credits,
    places: &'a [Place; CREDITS],
}

impl<const CREDITS: usize> Drop for Ending<'_, CREDITS> {
    fn drop(&mut self) {
        self.credits.stopped.store(true, Ordering::Release);
        self.credits.waker.wake();
        for place in self.places {
            place.wake();
        }
    }
}

/// The socket closed, or its service stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SocketClosed;

/// Why a socket did not take a block.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Refusal {
    /// For now: [`Socket::credit`] or [`Socket::room`] say when.
    Busy,
    /// For good; the block goes to the pool's holder.
    Closed,
}

/// A block a socket did not take, back to its caller.
#[derive(Debug)]
pub struct Refused<'m, B, const SIZE: usize> {
    pub reason: Refusal,
    pub block: Block<'m, B, SIZE>,
}

/// The storage of one socket's lanes and its credits' places.
pub struct SocketLanes<
    'a,
    'm,
    B,
    const SIZE: usize,
    const DEPTH: usize,
    const RECEIVED: usize,
    const CREDITS: usize,
> {
    requests: WakingSpscQueue<Request<'m, B, SIZE>, DEPTH>,
    answers: WakingSpscQueue<Answer<'a, 'm, B, SIZE, DEPTH>, ANSWERS>,
    datagrams: WakingSpscQueue<Datagram<'m, B, SIZE>, RECEIVED>,
    /// A dropped socket's last message, used once.
    abandon: WakingSpscQueue<Inbound<'a, 'm, B, SIZE, DEPTH, RECEIVED>, 1>,
    credits: Credits,
    places: [Place; CREDITS],
}

impl<B, const SIZE: usize, const DEPTH: usize, const RECEIVED: usize, const CREDITS: usize>
    SocketLanes<'_, '_, B, SIZE, DEPTH, RECEIVED, CREDITS>
{
    #[must_use]
    pub const fn new() -> Self {
        Self {
            requests: WakingSpscQueue::new(),
            answers: WakingSpscQueue::new(),
            datagrams: WakingSpscQueue::new(),
            abandon: WakingSpscQueue::new(),
            credits: Credits {
                free: AtomicU32::new(tag_mask(const { CREDITS as u8 })),
                waker: AtomicWaker::new(),
                stopped: AtomicBool::new(false),
            },
            places: [const { Place::new() }; CREDITS],
        }
    }
}

impl<B, const SIZE: usize, const DEPTH: usize, const RECEIVED: usize, const CREDITS: usize> Default
    for SocketLanes<'_, '_, B, SIZE, DEPTH, RECEIVED, CREDITS>
{
    fn default() -> Self {
        Self::new()
    }
}

/// A socket's lanes in two ends.
pub(crate) type Lanes<
    'a,
    'm,
    B,
    const SIZE: usize,
    const DEPTH: usize,
    const RECEIVED: usize,
    const CREDITS: usize,
> = (
    Socket<'a, 'm, B, SIZE, DEPTH, RECEIVED, CREDITS>,
    ServiceEnd<'a, 'm, B, SIZE, DEPTH, RECEIVED, CREDITS>,
);

/// Split `storage` into the application's socket and the service's end. Only
/// a service binding a socket does this, so no socket exists before its service.
pub(crate) fn lanes<
    'a,
    'm,
    B,
    const SIZE: usize,
    const DEPTH: usize,
    const RECEIVED: usize,
    const CREDITS: usize,
>(
    storage: &'a mut SocketLanes<'a, 'm, B, SIZE, DEPTH, RECEIVED, CREDITS>,
) -> Lanes<'a, 'm, B, SIZE, DEPTH, RECEIVED, CREDITS> {
    const {
        if CREDITS == 0 || CREDITS > 32 {
            panic!("a socket's tags are the bits of one word");
        }
    };
    let SocketLanes {
        requests,
        answers,
        datagrams,
        abandon,
        credits,
        places,
    } = storage;
    let (request_sender, request_receiver) = requests.split();
    let (answer_sender, answer_receiver) = answers.split();
    let (datagram_sender, datagram_receiver) = datagrams.split();
    let (abandon_sender, abandon_receiver) = abandon.split();
    let (credits, places) = (&*credits, &*places);
    (
        Socket {
            requests: request_sender,
            inbound: Some(Inbound {
                answers: answer_receiver,
                datagrams: datagram_receiver,
                reserved: None,
                stopped: None,
            }),
            abandon: abandon_sender,
            credits,
            places,
            reserving: false,
            closing: false,
            closed: false,
        },
        ServiceEnd {
            requests: request_receiver,
            answers: answer_sender,
            datagrams: datagram_sender,
            abandon: abandon_receiver,
            credits,
            places,
            ending: Ending { credits, places },
        },
    )
}

/// The first `count` tags, of at most 32.
const fn tag_mask(count: u8) -> u32 {
    match 1_u32.checked_shl(count as u32) {
        Some(bit) => bit.wrapping_sub(1),
        None => u32::MAX,
    }
}

const fn tag_bit(tag: u8) -> u32 {
    match 1_u32.checked_shl(tag as u32) {
        Some(bit) => bit,
        None => 0,
    }
}

/// The service's end of one socket's lanes.
pub(crate) struct ServiceEnd<
    'a,
    'm,
    B,
    const SIZE: usize,
    const DEPTH: usize,
    const RECEIVED: usize,
    const CREDITS: usize,
> {
    pub(crate) requests: WakingSpscReceiver<'a, Request<'m, B, SIZE>, DEPTH>,
    pub(crate) answers: WakingSpscSender<'a, Answer<'a, 'm, B, SIZE, DEPTH>, ANSWERS>,
    pub(crate) datagrams: WakingSpscSender<'a, Datagram<'m, B, SIZE>, RECEIVED>,
    pub(crate) abandon: WakingSpscReceiver<'a, Inbound<'a, 'm, B, SIZE, DEPTH, RECEIVED>, 1>,
    pub(crate) credits: &'a Credits,
    pub(crate) places: &'a [Place; CREDITS],
    pub(crate) ending: Ending<'a, CREDITS>,
}

/// The right to a datagram's one result, which needs no socket. Reading
/// consumes it, so a result is read once and a credit's next command is out of
/// reach. Dropping it gives the result up; the command still runs to its end,
/// and its credit frees then.
#[must_use = "dropping an observer gives up the datagram's result"]
pub struct Observer<'a> {
    place: &'a Place,
    credits: &'a Credits,
    tag: u8,
}

impl Observer<'_> {
    fn read(&self) -> Option<Sent> {
        // In this order: the service writes every result before it stops.
        let stopped = self.credits.stopped.load(Ordering::Acquire);
        match self.place.word.load(Ordering::Acquire) {
            // The service stopped without seeing this command.
            WAITING => stopped.then_some(Sent::Ended(Outcome::Retired)),
            word => RESULTS
                .get(word.wrapping_sub(FIRST_RESULT) as usize)
                .copied()
                .or(Some(Sent::Ended(Outcome::Unknown))),
        }
    }

    /// Wait until [`Self::take`] has the result. A wait may be cancelled and
    /// begun again.
    pub async fn finished(&mut self) {
        poll_fn(|cx| {
            self.place.waker.register(cx.waker());
            if self.read().is_some() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
    }

    /// The result.
    ///
    /// # Errors
    /// The command still runs: the observer comes back.
    pub fn take(self) -> Result<Sent, Self> {
        self.read().ok_or(self)
    }
}

impl core::fmt::Debug for Observer<'_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_tuple("Observer").field(&self.tag).finish()
    }
}

impl Drop for Observer<'_> {
    fn drop(&mut self) {
        if self.place.word.swap(GONE, Ordering::AcqRel) != WAITING {
            self.credits.give(self.tag);
        }
    }
}

/// What a socket receives on, and the blocks in it: a dropped socket's last
/// message hands all this over, so every block keeps an owner.
pub(crate) struct Inbound<'a, 'm, B, const SIZE: usize, const DEPTH: usize, const RECEIVED: usize> {
    answers: WakingSpscReceiver<'a, Answer<'a, 'm, B, SIZE, DEPTH>, ANSWERS>,
    datagrams: WakingSpscReceiver<'a, Datagram<'m, B, SIZE>, RECEIVED>,
    reserved: Option<Block<'m, B, SIZE>>,
    /// The request lane, after a stop.
    stopped: Option<WakingSpscReceiver<'a, Request<'m, B, SIZE>, DEPTH>>,
}

/// Give what `requests` holds to `pool`, and name the commands' tags.
pub(crate) fn drain_requests<'m, B, const SIZE: usize, const DEPTH: usize, const BLOCKS: usize>(
    requests: &WakingSpscReceiver<'_, Request<'m, B, SIZE>, DEPTH>,
    pool: &mut Pool<'m, B, SIZE, BLOCKS>,
    mut command: impl FnMut(u8),
) {
    while let Some(request) = requests.try_receive() {
        match request {
            Request::Send { tag, block, .. } => {
                pool.give(block);
                command(tag);
            }
            Request::Release(block) => pool.give(block),
            Request::Reserve | Request::Close => {}
        }
    }
}

impl<'m, B, const SIZE: usize, const DEPTH: usize, const RECEIVED: usize>
    Inbound<'_, 'm, B, SIZE, DEPTH, RECEIVED>
{
    /// Give every block in here to `pool`. The commands of a lane that came
    /// after a stop end by that stop.
    pub(crate) fn drain<const BLOCKS: usize>(mut self, pool: &mut Pool<'m, B, SIZE, BLOCKS>) {
        if let Some(block) = self.reserved.take() {
            pool.give(block);
        }
        while let Some(answer) = self.answers.try_receive() {
            match answer {
                Answer::Reserved(block) => pool.give(block),
                Answer::Stopped(requests) => drain_requests(&requests, pool, |_| {}),
                Answer::Closed => {}
            }
        }
        if let Some(requests) = &self.stopped {
            drain_requests(requests, pool, |_| {});
        }
        while let Some(datagram) = self.datagrams.try_receive() {
            pool.give(datagram.block);
        }
    }
}

/// What a stop leaves of a socket: if the socket is dropped later, this
/// gives the pool's holder what it held.
pub struct LateSocket<'a, 'm, B, const SIZE: usize, const DEPTH: usize, const RECEIVED: usize> {
    pub(crate) abandon: WakingSpscReceiver<'a, Inbound<'a, 'm, B, SIZE, DEPTH, RECEIVED>, 1>,
}

impl<'m, B, const SIZE: usize, const DEPTH: usize, const RECEIVED: usize>
    LateSocket<'_, 'm, B, SIZE, DEPTH, RECEIVED>
{
    /// Whether the socket was dropped: if so, its blocks are in `pool` now.
    pub fn collect<const BLOCKS: usize>(&mut self, pool: &mut Pool<'m, B, SIZE, BLOCKS>) -> bool {
        match self.abandon.try_receive() {
            Some(inbound) => {
                inbound.drain(pool);
                true
            }
            None => false,
        }
    }
}

/// What an application holds of a socket. Dropping it aborts the socket
/// without waiting: its accepted commands still reach their results, and its
/// lanes' blocks go back to the pool. A block in the application's hands stays
/// there, for the pool's holder.
pub struct Socket<
    'a,
    'm,
    B,
    const SIZE: usize,
    const DEPTH: usize,
    const RECEIVED: usize,
    const CREDITS: usize,
> {
    requests: WakingSpscSender<'a, Request<'m, B, SIZE>, DEPTH>,
    /// `None` only inside `drop`.
    inbound: Option<Inbound<'a, 'm, B, SIZE, DEPTH, RECEIVED>>,
    abandon: WakingSpscSender<'a, Inbound<'a, 'm, B, SIZE, DEPTH, RECEIVED>, 1>,
    credits: &'a Credits,
    places: &'a [Place; CREDITS],
    /// A reservation is out.
    reserving: bool,
    /// The socket asked to close: no command follows.
    closing: bool,
    /// The socket's close finished, or the service stopped.
    closed: bool,
}

impl<B, const SIZE: usize, const DEPTH: usize, const RECEIVED: usize, const CREDITS: usize> Drop
    for Socket<'_, '_, B, SIZE, DEPTH, RECEIVED, CREDITS>
{
    fn drop(&mut self) {
        if let Some(inbound) = self.inbound.take() {
            // This lane's one use, so it has room.
            let _first_use = self.abandon.try_send(inbound);
        }
    }
}

impl<'a, 'm, B, const SIZE: usize, const DEPTH: usize, const RECEIVED: usize, const CREDITS: usize>
    Socket<'a, 'm, B, SIZE, DEPTH, RECEIVED, CREDITS>
{
    /// Take what the service said.
    fn take_answers(&mut self) {
        let Some(inbound) = &mut self.inbound else {
            return;
        };
        while let Some(answer) = inbound.answers.try_receive() {
            match answer {
                Answer::Reserved(block) => {
                    self.reserving = false;
                    inbound.reserved = Some(block);
                }
                Answer::Closed => self.closed = true,
                Answer::Stopped(requests) => {
                    self.closed = true;
                    inbound.stopped = Some(requests);
                }
            }
        }
        // After the lane: a service's last answer comes before this word.
        if self.credits.stopped.load(Ordering::Acquire) {
            self.closed = true;
        }
    }

    /// Wait for the service's next word, or its end.
    async fn answered(&mut self) {
        if let Some(inbound) = &mut self.inbound {
            let _either = select(inbound.answers.ready(), self.credits.stopped()).await;
        }
    }

    /// Whether the socket closed or its service stopped.
    pub fn closed(&mut self) -> bool {
        self.take_answers();
        self.closed
    }

    /// Wait for room in the request lane. Nothing can take it but this
    /// socket, so what follows finds it.
    ///
    /// # Errors
    /// [`SocketClosed`]: the service stopped.
    pub async fn room(&mut self) -> Result<(), SocketClosed> {
        loop {
            self.take_answers();
            let Some(inbound) = &mut self.inbound else {
                return Err(SocketClosed);
            };
            if self.credits.stopped.load(Ordering::Acquire) {
                return Err(SocketClosed);
            }
            if self.requests.free_capacity() != 0 {
                return Ok(());
            }
            let _any = select3(
                self.requests.ready(),
                inbound.answers.ready(),
                self.credits.stopped(),
            )
            .await;
        }
    }

    /// Wait until [`Socket::send`] can take a datagram: a free credit and
    /// room in the lane. A credit frees when its observer reads or goes and
    /// its command has ended.
    ///
    /// # Errors
    /// [`SocketClosed`].
    pub async fn credit(&mut self) -> Result<(), SocketClosed> {
        self.take_answers();
        if self.closed || self.closing {
            return Err(SocketClosed);
        }
        let credits = self.credits;
        poll_fn(|cx| {
            credits.waker.register(cx.waker());
            let open = credits.free.load(Ordering::Acquire) != 0;
            if open || credits.stopped.load(Ordering::Acquire) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
        if credits.stopped.load(Ordering::Acquire) {
            return Err(SocketClosed);
        }
        self.room().await
    }

    /// A free block for a datagram, once the service has one. A cancelled
    /// wait loses nothing: the block waits for the next.
    ///
    /// # Errors
    /// [`SocketClosed`].
    pub async fn reserve(&mut self) -> Result<Block<'m, B, SIZE>, SocketClosed> {
        loop {
            self.take_answers();
            let held = self
                .inbound
                .as_mut()
                .and_then(|lanes| lanes.reserved.take());
            if let Some(block) = held {
                return Ok(block);
            }
            if self.closed || self.closing {
                return Err(SocketClosed);
            }
            if !self.reserving {
                self.room().await?;
                self.reserving = self.requests.try_send(Request::Reserve).is_ok();
            }
            self.answered().await;
        }
    }

    /// Commit a datagram. Writing it into the lane is the acceptance point:
    /// before it a [`Refused`] with the block, after it one result, which
    /// the returned [`Observer`] holds the right to. A command the service
    /// never saw before it stopped ends [`Outcome::Retired`], and its block
    /// comes back through [`Socket::reclaim`].
    ///
    /// # Errors
    /// [`Refused`].
    pub fn send(
        &mut self,
        remote: Remote,
        block: Block<'m, B, SIZE>,
    ) -> Result<Observer<'a>, Refused<'m, B, SIZE>> {
        self.take_answers();
        if self.closed || self.closing || self.credits.stopped.load(Ordering::Acquire) {
            return Err(Refused {
                reason: Refusal::Closed,
                block,
            });
        }
        let free = self.credits.free.load(Ordering::Acquire);
        let tag = u8::try_from(free.trailing_zeros()).unwrap_or(u8::MAX);
        let place = self.places.get(usize::from(tag));
        let permit = self.requests.try_reserve();
        let (Some(place), Some(permit)) = (place, permit) else {
            return Err(Refused {
                reason: Refusal::Busy,
                block,
            });
        };
        place.word.store(WAITING, Ordering::Release);
        self.credits.free.fetch_and(!tag_bit(tag), Ordering::AcqRel);
        permit.send(Request::Send { tag, remote, block });
        Ok(Observer {
            place,
            credits: self.credits,
            tag,
        })
    }

    /// The next received datagram.
    ///
    /// # Errors
    /// [`SocketClosed`], once all that arrived was read.
    pub async fn receive(&mut self) -> Result<Datagram<'m, B, SIZE>, SocketClosed> {
        loop {
            // In this order: nothing follows a close in the lane.
            self.take_answers();
            let Some(inbound) = &mut self.inbound else {
                return Err(SocketClosed);
            };
            if let Some(datagram) = inbound.datagrams.try_receive() {
                return Ok(datagram);
            }
            if self.closed {
                return Err(SocketClosed);
            }
            let _any = select3(
                inbound.datagrams.ready(),
                inbound.answers.ready(),
                self.credits.stopped(),
            )
            .await;
        }
    }

    /// Give a block back.
    ///
    /// # Errors
    /// [`Refused`].
    pub fn release(&mut self, block: Block<'m, B, SIZE>) -> Result<(), Refused<'m, B, SIZE>> {
        self.take_answers();
        if self.credits.stopped.load(Ordering::Acquire) {
            return Err(Refused {
                reason: Refusal::Closed,
                block,
            });
        }
        match self.requests.try_reserve() {
            Some(permit) => {
                permit.send(Request::Release(block));
                Ok(())
            }
            None => Err(Refused {
                reason: Refusal::Busy,
                block,
            }),
        }
    }

    /// End the socket, and wait for that. A stop ends it too.
    pub async fn close(&mut self) {
        loop {
            self.take_answers();
            if self.closed {
                return;
            }
            if !self.closing {
                if self.room().await.is_err() {
                    return;
                }
                self.closing = self.requests.try_send(Request::Close).is_ok();
            }
            self.answered().await;
        }
    }

    /// Datagrams in the lane.
    #[cfg(test)]
    pub(crate) fn received(&self) -> usize {
        self.inbound
            .as_ref()
            .map_or(0, |lanes| lanes.datagrams.len())
    }

    /// After a stop, the next block its lanes hold, one call each: a
    /// reservation that came, a command or a release the service never saw,
    /// and a datagram not read. It goes to the pool's holder.
    pub fn reclaim(&mut self) -> Option<Block<'m, B, SIZE>> {
        self.take_answers();
        let inbound = self.inbound.as_mut()?;
        let requests = inbound.stopped.as_ref()?;
        if let Some(block) = inbound.reserved.take() {
            return Some(block);
        }
        while let Some(request) = requests.try_receive() {
            if let Request::Release(block) | Request::Send { block, .. } = request {
                return Some(block);
            }
        }
        let datagram = inbound.datagrams.try_receive()?;
        Some(datagram.block)
    }
}

#[cfg(test)]
mod tests {
    use super::{FIRST_RESULT, RESULTS, word_of};

    #[test]
    fn every_result_has_its_own_word() {
        for (word, sent) in (FIRST_RESULT..).zip(RESULTS) {
            assert_eq!(word_of(sent), word);
        }
    }
}
