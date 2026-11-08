//! A lending socket: datagrams kept in the port's own buffers, each under a
//! loan of the socket's credit.
//!
//! A lent buffer goes back to its port when the application drops its
//! [`Lent`], and only then is its loan counted back, so a credit frees once
//! its memory has. The service gives the loan to the pool's account at its
//! next turn; after the service, the [`LateSocket`](super::LateSocket) it
//! left does, for whoever holds the pool. Nothing here waits for a buffer: a
//! stop ends the service with loans still out, and they count until they
//! return.

use core::ops::Range;
use core::sync::atomic::{AtomicU32, Ordering};
use core::task::Context;

use embassy_futures::select::select4;
use oer_portable_memory::{AtomicWaker, WakingSpscQueue, WakingSpscReceiver, WakingSpscSender};
use oer_protocols_network_core::{Datagram, GROUPS, LentDatagram, Loan, Loans, Remote};
use oer_protocols_network_port::{EthernetFrame, FrameLengthError};

use super::{Socket, SocketClosed, SocketLanes, lanes};

/// Lent buffers back with their port whose loans the service has not yet
/// given back.
pub(crate) struct Returns {
    back: AtomicU32,
    waker: AtomicWaker,
}

impl Returns {
    const fn new() -> Self {
        Self {
            back: AtomicU32::new(0),
            waker: AtomicWaker::new(),
        }
    }

    fn give(&self) {
        // At most a socket's credit is out, far below the word's range.
        self.back.fetch_add(1, Ordering::AcqRel);
        self.waker.wake();
    }

    fn take(&self) -> u32 {
        self.back.swap(0, Ordering::AcqRel)
    }

    /// Whether a return waits; `cx` hears the next.
    pub(crate) fn poll(&self, cx: &Context<'_>) -> bool {
        self.waker.register(cx.waker());
        self.back.load(Ordering::Acquire) != 0
    }
}

/// Counts its buffer back when dropped.
struct Back<'a>(&'a Returns);

impl Drop for Back<'_> {
    fn drop(&mut self) {
        self.0.give();
    }
}

/// A datagram in a port's buffer, lent to the application. Dropping it gives
/// the buffer back to the port, then counts its loan back.
pub struct Lent<'a, H> {
    frame: H,
    data: Range<usize>,
    remote: Remote,
    /// After `frame`: fields drop in order, so the buffer is back first.
    _back: Back<'a>,
}

impl<H: EthernetFrame> Lent<'_, H> {
    /// Who sent it.
    pub const fn remote(&self) -> Remote {
        self.remote
    }

    /// The datagram's data, in the port's buffer.
    ///
    /// # Errors
    /// [`FrameLengthError`]: the buffer no longer holds what the port showed,
    /// which breaks its own contract.
    pub fn data(&mut self) -> Result<&[u8], FrameLengthError> {
        let view = self.frame.view()?;
        view.payload
            .get(self.data.clone())
            .ok_or(FrameLengthError::OutsideStorage)
    }
}

impl<H> core::fmt::Debug for Lent<'_, H> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("Lent")
            .field("remote", &self.remote)
            .field("data", &self.data)
            .finish_non_exhaustive()
    }
}

/// What a lending socket receives.
#[derive(Debug)]
pub enum Received<'a, 'm, B, const SIZE: usize, H> {
    /// A copy in a block of the socket's group: what the device copied for
    /// its form.
    Copied(Datagram<'m, B, SIZE>),
    /// A datagram in the port's own buffer.
    Lent(Lent<'a, H>),
}

/// The storage of a lending socket: its lanes, and the lane of its loans.
pub struct LendingLanes<
    'a,
    'm,
    B,
    const SIZE: usize,
    const DEPTH: usize,
    const RECEIVED: usize,
    const CREDITS: usize,
    H,
> {
    pub(crate) lanes: SocketLanes<'a, 'm, B, SIZE, DEPTH, RECEIVED, CREDITS>,
    pub(crate) lent: WakingSpscQueue<Lent<'a, H>, RECEIVED>,
    pub(crate) returns: Returns,
}

impl<B, const SIZE: usize, const DEPTH: usize, const RECEIVED: usize, const CREDITS: usize, H>
    LendingLanes<'_, '_, B, SIZE, DEPTH, RECEIVED, CREDITS, H>
{
    #[must_use]
    pub const fn new() -> Self {
        Self {
            lanes: SocketLanes::new(),
            lent: WakingSpscQueue::new(),
            returns: Returns::new(),
        }
    }
}

impl<B, const SIZE: usize, const DEPTH: usize, const RECEIVED: usize, const CREDITS: usize, H>
    Default for LendingLanes<'_, '_, B, SIZE, DEPTH, RECEIVED, CREDITS, H>
{
    fn default() -> Self {
        Self::new()
    }
}

/// The service's end of a lending socket's loans: the lane, and the loans of
/// what it lent, at most its credit, which the lane holds.
pub(crate) struct LentEnd<'a, B, H, const RECEIVED: usize> {
    lent: WakingSpscSender<'a, Lent<'a, H>, RECEIVED>,
    returns: &'a Returns,
    loans: [Option<Loan<B>>; RECEIVED],
}

impl<'a, B, H, const RECEIVED: usize> LentEnd<'a, B, H, RECEIVED> {
    /// Hand `datagram` to the socket.
    ///
    /// # Errors
    /// The lane had no room: the datagram is dropped, its buffer back with
    /// the port, and its loan counted back.
    pub(crate) fn lend(
        &mut self,
        datagram: LentDatagram<B, H>,
        account: &mut Loans<B, GROUPS>,
    ) -> Result<(), LentLaneFull> {
        let LentDatagram {
            remote,
            data,
            frame,
            loan,
            ..
        } = datagram;
        let Some(place) = self.loans.iter_mut().find(|place| place.is_none()) else {
            // More out than the credit, which the lane bounds: the buffer
            // goes back before its loan.
            drop(frame);
            account.give_back(loan);
            return Err(LentLaneFull);
        };
        *place = Some(loan);
        let lent = Lent {
            frame,
            data,
            remote,
            _back: Back(self.returns),
        };
        // A refused one drops here, and counts itself back.
        self.lent.try_send(lent).map_err(|_| LentLaneFull)
    }

    /// Give back to `account` the loans whose buffers came back; whether any.
    pub(crate) fn collect(&mut self, account: &mut Loans<B, GROUPS>) -> bool {
        give_back(self.returns, &mut self.loans, account)
    }

    /// Whether a return waits; `cx` hears the next.
    pub(crate) fn poll(&self, cx: &Context<'_>) -> bool {
        self.returns.poll(cx)
    }

    /// What outlives the service: the loans still out.
    pub(crate) fn into_late(self) -> LateLoans<'a, B, RECEIVED> {
        LateLoans {
            returns: self.returns,
            loans: self.loans,
        }
    }
}

/// The lent lane had no room.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LentLaneFull;

/// Give back as many of `loans` as `returns` counts.
fn give_back<B, const RECEIVED: usize>(
    returns: &Returns,
    loans: &mut [Option<Loan<B>>; RECEIVED],
    account: &mut Loans<B, GROUPS>,
) -> bool {
    let back = returns.take();
    let mut taken = loans.iter_mut().filter_map(Option::take);
    for _ in 0..back {
        let Some(loan) = taken.next() else {
            break;
        };
        account.give_back(loan);
    }
    back != 0
}

/// A lending socket's loans after its service: they come back to whoever
/// holds the pool.
pub(crate) struct LateLoans<'a, B, const RECEIVED: usize> {
    returns: &'a Returns,
    loans: [Option<Loan<B>>; RECEIVED],
}

impl<B, const RECEIVED: usize> LateLoans<'_, B, RECEIVED> {
    pub(crate) fn collect(&mut self, account: &mut Loans<B, GROUPS>) -> bool {
        give_back(self.returns, &mut self.loans, account)
    }

    /// Whether every loan came back.
    pub(crate) fn settled(&self) -> bool {
        self.loans.iter().all(Option::is_none)
    }

    pub(crate) fn poll(&self, cx: &Context<'_>) -> bool {
        self.returns.poll(cx)
    }
}

/// Split `storage` into the application's socket and the service's ends.
pub(crate) fn lending_lanes<
    'a,
    'm,
    B,
    const SIZE: usize,
    const DEPTH: usize,
    const RECEIVED: usize,
    const CREDITS: usize,
    H,
>(
    storage: &'a mut LendingLanes<'a, 'm, B, SIZE, DEPTH, RECEIVED, CREDITS, H>,
) -> (
    LendingSocket<'a, 'm, B, SIZE, DEPTH, RECEIVED, CREDITS, H>,
    super::ServiceEnd<'a, 'm, B, SIZE, DEPTH, RECEIVED, CREDITS>,
    LentEnd<'a, B, H, RECEIVED>,
) {
    let LendingLanes {
        lanes: socket_lanes,
        lent,
        returns,
    } = storage;
    let (socket, end) = lanes(socket_lanes);
    let (sender, receiver) = lent.split();
    (
        LendingSocket {
            socket,
            lent: receiver,
        },
        end,
        LentEnd {
            lent: sender,
            returns,
            loans: [const { None }; RECEIVED],
        },
    )
}

/// What an application holds of a lending socket: a [`Socket`], whose
/// receiving takes lent datagrams too. Dropping it drops the lent datagrams
/// it holds, then the socket; a [`Lent`] in the application's hands stays
/// there, and its loan counts until it drops.
pub struct LendingSocket<
    'a,
    'm,
    B,
    const SIZE: usize,
    const DEPTH: usize,
    const RECEIVED: usize,
    const CREDITS: usize,
    H,
> {
    socket: Socket<'a, 'm, B, SIZE, DEPTH, RECEIVED, CREDITS>,
    lent: WakingSpscReceiver<'a, Lent<'a, H>, RECEIVED>,
}

impl<B, const SIZE: usize, const DEPTH: usize, const RECEIVED: usize, const CREDITS: usize, H> Drop
    for LendingSocket<'_, '_, B, SIZE, DEPTH, RECEIVED, CREDITS, H>
{
    fn drop(&mut self) {
        while let Some(lent) = self.lent.try_receive() {
            drop(lent);
        }
    }
}

impl<
    'a,
    'm,
    B,
    const SIZE: usize,
    const DEPTH: usize,
    const RECEIVED: usize,
    const CREDITS: usize,
    H,
> LendingSocket<'a, 'm, B, SIZE, DEPTH, RECEIVED, CREDITS, H>
{
    /// The socket, for all but receiving.
    pub const fn socket(&mut self) -> &mut Socket<'a, 'm, B, SIZE, DEPTH, RECEIVED, CREDITS> {
        &mut self.socket
    }

    /// The next received datagram, lent or copied.
    ///
    /// # Errors
    /// [`SocketClosed`], once all that arrived was read.
    pub async fn receive(&mut self) -> Result<Received<'a, 'm, B, SIZE, H>, SocketClosed> {
        loop {
            // In this order: nothing follows a close in the lanes.
            self.socket.take_answers();
            if let Some(lent) = self.lent.try_receive() {
                return Ok(Received::Lent(lent));
            }
            let Some(inbound) = &mut self.socket.inbound else {
                return Err(SocketClosed);
            };
            if let Some(datagram) = inbound.datagrams.try_receive() {
                return Ok(Received::Copied(datagram));
            }
            if self.socket.closed {
                return Err(SocketClosed);
            }
            let _any = select4(
                self.lent.ready(),
                inbound.datagrams.ready(),
                inbound.answers.ready(),
                self.socket.credits.stopped(),
            )
            .await;
        }
    }

    /// [`Self::receive`] without waiting: the next datagram if one is here.
    pub fn try_receive(&mut self) -> Option<Received<'a, 'm, B, SIZE, H>> {
        self.socket.take_answers();
        if let Some(lent) = self.lent.try_receive() {
            return Some(Received::Lent(lent));
        }
        self.socket.try_receive().map(Received::Copied)
    }
}
