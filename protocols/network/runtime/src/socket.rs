//! A socket: what an application holds, and the lanes to the service.

use core::ptr;
use core::sync::atomic::{AtomicU32, Ordering};

use embassy_futures::select::select;
use oer_portable_memory::{WakingSpscQueue, WakingSpscReceiver, WakingSpscSender};
use oer_protocols_network_core::{Block, Datagram, Outcome, Remote, SendRefusal};

/// What a socket asks of the service.
pub(crate) enum Request<'m, B, const SIZE: usize> {
    /// A free block; a socket has one such request at a time.
    Reserve,
    /// A block back.
    Release(Block<'m, B, SIZE>),
    /// A datagram, under its observer's tag.
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
    /// A command ended.
    Ended {
        tag: u8,
        sent: Sent,
    },
    /// The command of a dropped observer ended: its tag is free.
    Forgotten {
        tag: u8,
    },
    /// The socket's close finished; every result came before this.
    Closed,
    /// The service stopped, and every result it knows came before this. What
    /// the request lane still holds, it never saw: the lane comes back.
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

/// The socket closed, or its service stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SocketClosed;

/// An observer of another socket.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ForeignObserver;

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

/// The storage of one socket's lanes. `DEPTH` must hold all a socket can be
/// owed unread: every credit's result, a block, a close and a stop.
pub struct SocketLanes<'a, 'm, B, const SIZE: usize, const DEPTH: usize, const RECEIVED: usize> {
    requests: WakingSpscQueue<Request<'m, B, SIZE>, DEPTH>,
    answers: WakingSpscQueue<Answer<'a, 'm, B, SIZE, DEPTH>, DEPTH>,
    datagrams: WakingSpscQueue<Datagram<'m, B, SIZE>, RECEIVED>,
    /// Tags whose observers were dropped.
    dropped: AtomicU32,
}

impl<B, const SIZE: usize, const DEPTH: usize, const RECEIVED: usize>
    SocketLanes<'_, '_, B, SIZE, DEPTH, RECEIVED>
{
    #[must_use]
    pub const fn new() -> Self {
        Self {
            requests: WakingSpscQueue::new(),
            answers: WakingSpscQueue::new(),
            datagrams: WakingSpscQueue::new(),
            dropped: AtomicU32::new(0),
        }
    }
}

impl<B, const SIZE: usize, const DEPTH: usize, const RECEIVED: usize> Default
    for SocketLanes<'_, '_, B, SIZE, DEPTH, RECEIVED>
{
    fn default() -> Self {
        Self::new()
    }
}

/// A socket's lanes in two ends.
pub type Lanes<
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

/// Split `storage` into the application's socket and the service's end.
pub fn lanes<
    'a,
    'm,
    B,
    const SIZE: usize,
    const DEPTH: usize,
    const RECEIVED: usize,
    const CREDITS: usize,
>(
    storage: &'a mut SocketLanes<'a, 'm, B, SIZE, DEPTH, RECEIVED>,
) -> Lanes<'a, 'm, B, SIZE, DEPTH, RECEIVED, CREDITS> {
    const {
        if CREDITS == 0 || CREDITS > 32 {
            panic!("a socket's tags are the bits of one word");
        }
        if DEPTH < CREDITS + 3 {
            panic!("the answer lane holds all it can owe: every credit, a block, a close, a stop");
        }
    };
    let SocketLanes {
        requests,
        answers,
        datagrams,
        dropped,
    } = storage;
    let (request_sender, request_receiver) = requests.split();
    let (answer_sender, answer_receiver) = answers.split();
    let (datagram_sender, datagram_receiver) = datagrams.split();
    (
        Socket {
            requests: request_sender,
            answers: answer_receiver,
            datagrams: datagram_receiver,
            dropped,
            free: tag_mask(const { CREDITS as u8 }),
            ended: [None; CREDITS],
            reserving: false,
            reserved: None,
            closing: false,
            closed: false,
            stopped: None,
        },
        ServiceEnd {
            requests: request_receiver,
            answers: answer_sender,
            datagrams: datagram_sender,
            dropped,
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

pub(crate) const fn tag_bit(tag: u8) -> u32 {
    match 1_u32.checked_shl(tag as u32) {
        Some(bit) => bit,
        None => 0,
    }
}

/// The service's end of one socket's lanes.
pub struct ServiceEnd<
    'a,
    'm,
    B,
    const SIZE: usize,
    const DEPTH: usize,
    const RECEIVED: usize,
    const CREDITS: usize,
> {
    pub(crate) requests: WakingSpscReceiver<'a, Request<'m, B, SIZE>, DEPTH>,
    pub(crate) answers: WakingSpscSender<'a, Answer<'a, 'm, B, SIZE, DEPTH>, DEPTH>,
    pub(crate) datagrams: WakingSpscSender<'a, Datagram<'m, B, SIZE>, RECEIVED>,
    pub(crate) dropped: &'a AtomicU32,
}

/// The right to a datagram's one result. Dropping it gives the result up;
/// the command still runs to its end.
#[derive(Debug)]
#[must_use = "dropping an observer gives up the datagram's result"]
pub struct Observer<'a> {
    tag: u8,
    read: bool,
    dropped: &'a AtomicU32,
}

impl Drop for Observer<'_> {
    fn drop(&mut self) {
        if !self.read {
            self.dropped.fetch_or(tag_bit(self.tag), Ordering::AcqRel);
        }
    }
}

/// What an application holds of a socket.
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
    answers: WakingSpscReceiver<'a, Answer<'a, 'm, B, SIZE, DEPTH>, DEPTH>,
    datagrams: WakingSpscReceiver<'a, Datagram<'m, B, SIZE>, RECEIVED>,
    dropped: &'a AtomicU32,
    /// Tags free for a command.
    free: u32,
    /// Results that came, until read.
    ended: [Option<Sent>; CREDITS],
    /// A reservation is out.
    reserving: bool,
    reserved: Option<Block<'m, B, SIZE>>,
    /// The socket asked to close: no command follows.
    closing: bool,
    /// Every result came.
    closed: bool,
    /// The service stopped, and the requests it never saw.
    stopped: Option<WakingSpscReceiver<'a, Request<'m, B, SIZE>, DEPTH>>,
}

impl<'a, 'm, B, const SIZE: usize, const DEPTH: usize, const RECEIVED: usize, const CREDITS: usize>
    Socket<'a, 'm, B, SIZE, DEPTH, RECEIVED, CREDITS>
{
    /// Take what the service said.
    fn take_answers(&mut self) {
        while let Some(answer) = self.answers.try_receive() {
            match answer {
                Answer::Reserved(block) => {
                    self.reserving = false;
                    self.reserved = Some(block);
                }
                Answer::Ended { tag, sent } => {
                    if let Some(place) = self.ended.get_mut(usize::from(tag)) {
                        *place = Some(sent);
                    }
                }
                Answer::Forgotten { tag } => self.free |= tag_bit(tag),
                Answer::Closed => self.closed = true,
                Answer::Stopped(requests) => {
                    self.closed = true;
                    self.stopped = Some(requests);
                }
            }
        }
        // The service ended this command before it saw the observer go.
        for (tag, place) in (0_u8..).zip(self.ended.iter_mut()) {
            let bit = tag_bit(tag);
            if place.is_some() && self.dropped.load(Ordering::Acquire) & bit != 0 {
                self.dropped.fetch_and(!bit, Ordering::AcqRel);
                *place = None;
                self.free |= bit;
            }
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
            if self.stopped.is_some() {
                return Err(SocketClosed);
            }
            if self.requests.free_capacity() != 0 {
                return Ok(());
            }
            let _either = select(self.requests.ready(), self.answers.ready()).await;
        }
    }

    /// Wait until [`Socket::send`] can take a datagram: a free credit and
    /// room in the lane.
    ///
    /// # Errors
    /// [`SocketClosed`].
    pub async fn credit(&mut self) -> Result<(), SocketClosed> {
        loop {
            self.take_answers();
            if self.closed || self.closing {
                return Err(SocketClosed);
            }
            if self.free != 0 {
                return self.room().await;
            }
            self.answers.ready().await;
        }
    }

    /// A free block for a datagram, once the service has one. A cancelled
    /// wait loses nothing: the block waits for the next.
    ///
    /// # Errors
    /// [`SocketClosed`].
    pub async fn reserve(&mut self) -> Result<Block<'m, B, SIZE>, SocketClosed> {
        loop {
            self.take_answers();
            if let Some(block) = self.reserved.take() {
                return Ok(block);
            }
            if self.closed || self.closing {
                return Err(SocketClosed);
            }
            if !self.reserving {
                self.room().await?;
                self.reserving = self.requests.try_send(Request::Reserve).is_ok();
            }
            self.answers.ready().await;
        }
    }

    /// Commit a datagram. Writing it into the lane is the acceptance point:
    /// before it a [`Refused`] with the block, after it one result, which
    /// the returned [`Observer`] holds the right to.
    ///
    /// # Errors
    /// [`Refused`].
    pub fn send(
        &mut self,
        remote: Remote,
        block: Block<'m, B, SIZE>,
    ) -> Result<Observer<'a>, Refused<'m, B, SIZE>> {
        self.take_answers();
        if self.closed || self.closing {
            return Err(Refused {
                reason: Refusal::Closed,
                block,
            });
        }
        let tag = u8::try_from(self.free.trailing_zeros()).unwrap_or(u8::MAX);
        let permit = self.requests.try_reserve();
        let (true, Some(permit)) = (self.free != 0, permit) else {
            return Err(Refused {
                reason: Refusal::Busy,
                block,
            });
        };
        permit.send(Request::Send { tag, remote, block });
        self.free &= !tag_bit(tag);
        Ok(Observer {
            tag,
            read: false,
            dropped: self.dropped,
        })
    }

    /// The result of `observer`'s command. A wait may be cancelled and begun
    /// again; the result is read once. A command the service never saw
    /// before it stopped ends [`Outcome::Retired`], and its block comes back
    /// through [`Socket::reclaim`].
    ///
    /// # Errors
    /// [`ForeignObserver`], changing nothing.
    pub async fn result(&mut self, observer: &mut Observer<'a>) -> Result<Sent, ForeignObserver> {
        if !ptr::eq(observer.dropped, self.dropped) {
            return Err(ForeignObserver);
        }
        loop {
            self.take_answers();
            let place = self.ended.get_mut(usize::from(observer.tag));
            let result = place
                .and_then(Option::take)
                .or(self.closed.then_some(Sent::Ended(Outcome::Retired)));
            if let Some(sent) = result {
                observer.read = true;
                self.free |= tag_bit(observer.tag);
                return Ok(sent);
            }
            self.answers.ready().await;
        }
    }

    /// The next received datagram.
    ///
    /// # Errors
    /// [`SocketClosed`], once all that arrived was read.
    pub async fn receive(&mut self) -> Result<Datagram<'m, B, SIZE>, SocketClosed> {
        loop {
            // In this order: nothing follows a close in the lane.
            self.take_answers();
            if let Some(datagram) = self.datagrams.try_receive() {
                return Ok(datagram);
            }
            if self.closed {
                return Err(SocketClosed);
            }
            let _either = select(self.datagrams.ready(), self.answers.ready()).await;
        }
    }

    /// Give a block back.
    ///
    /// # Errors
    /// [`Refused`].
    pub fn release(&mut self, block: Block<'m, B, SIZE>) -> Result<(), Refused<'m, B, SIZE>> {
        self.take_answers();
        if self.stopped.is_some() {
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
            self.answers.ready().await;
        }
    }

    /// Whether a result came and waits to be read.
    #[cfg(test)]
    pub(crate) fn ended_count(&mut self) -> usize {
        self.take_answers();
        self.ended.iter().flatten().count()
    }

    /// Datagrams in the lane.
    #[cfg(test)]
    pub(crate) fn received(&self) -> usize {
        self.datagrams.len()
    }

    /// After a stop, the next block its lanes hold, one call each: a
    /// reservation that came, a command or a release the service never saw,
    /// and a datagram not read. It goes to the pool's holder.
    pub fn reclaim(&mut self) -> Option<Block<'m, B, SIZE>> {
        self.take_answers();
        let requests = self.stopped.as_ref()?;
        if let Some(block) = self.reserved.take() {
            return Some(block);
        }
        while let Some(request) = requests.try_receive() {
            if let Request::Release(block) | Request::Send { block, .. } = request {
                return Some(block);
            }
        }
        self.datagrams.try_receive().map(|datagram| datagram.block)
    }
}
