//! A lending socket: datagrams kept in the port's own buffers, each under a
//! loan of the socket's credit.
//!
//! A lent buffer goes back to its port when the application drops its
//! [`Lent`], and only then is its loan counted back, so a credit frees once
//! its memory has. The service gives the loan to the pool's account at its
//! next pass, a turn or a pass of its wait; after the service, the [`LateSocket`](super::LateSocket) it
//! left does, for whoever holds the pool. Nothing here waits for a buffer: a
//! stop ends the service with loans still out, and they count until they
//! return.

use core::ops::Range;
use core::sync::atomic::{AtomicU32, Ordering};
use core::task::Context;

use embassy_futures::select::select4;
use oer_portable_memory::{AtomicWaker, WakingSpscQueue, WakingSpscReceiver, WakingSpscSender};
use oer_protocols_network_core::{Datagram, GROUPS, LentDatagram, Loan, Loans, Remote, Unlent};
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

    /// Out of line, so its wake keeps its own `Waker::wake` debug scope and
    /// the image's stack proof resolves it, as the service's `closed` and the
    /// station's same-hart return do: a caller that drops a [`Lent`] on
    /// several paths (each `or_else` of [`LendingSocket::take`]) can
    /// otherwise merge their inlined wakes into one block without that scope.
    #[inline(never)]
    fn give(&self) {
        // At most a socket's credit is out, far below the word's range.
        self.back.fetch_add(1, Ordering::AcqRel);
        self.waker.wake();
    }

    fn take(&self) -> u32 {
        self.back.swap(0, Ordering::AcqRel)
    }

    /// The buffers back whose loans are not yet taken, as published now.
    fn published(&self) -> u32 {
        self.back.load(Ordering::Acquire)
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
    /// Hand `datagram` to the socket, within a batch that ends with
    /// [`Self::flush`] before its service waits.
    ///
    /// # Errors
    /// The lane had no room ([`Unlent::LaneFull`]): the datagram is dropped,
    /// its buffer back with the port, and its loan counted back, by the
    /// account or by the drop of its lent datagram.
    pub(crate) fn lend(&mut self, datagram: LentDatagram<B, H>) -> Result<(), Unlent<B>> {
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
            return Err(Unlent::LaneFull(Some(loan)));
        };
        *place = Some(loan);
        let lent = Lent {
            frame,
            data,
            remote,
            _back: Back(self.returns),
        };
        // A refused one drops here, and counts itself back. The socket hears
        // the batch's datagrams once, at [`Self::flush`].
        self.lent
            .try_send_held(lent)
            .map_err(|_| Unlent::LaneFull(None))
    }

    /// End a batch of lends: the socket hears its datagrams once.
    pub(crate) fn flush(&self) {
        self.lent.flush();
    }

    /// Give back to `account` the loans whose buffers came back; whether any.
    pub(crate) fn collect(&mut self, account: &mut Loans<B, GROUPS>) -> bool {
        give_back(self.returns, &mut self.loans, account)
    }

    /// The buffers back with their port whose loans are not yet given back
    /// to the account, as published now.
    pub(crate) fn unreturned(&self) -> u32 {
        self.returns.published()
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
            copy_next: false,
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
/// receiving takes lent datagrams too. Its two lanes take turns: while both
/// hold a datagram, receives alternate between them, so a copy waits behind
/// at most one loan, and a loan behind at most one copy. Dropping it drops
/// the lent datagrams it holds, then the socket; a [`Lent`] in the
/// application's hands stays there, and its loan counts until it drops.
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
    /// The copies' lane goes first at the next receive: the last one took a
    /// loan.
    copy_next: bool,
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
            if let Some(received) = self.take() {
                return Ok(received);
            }
            if self.socket.closed {
                return Err(SocketClosed);
            }
            let Some(inbound) = &mut self.socket.inbound else {
                return Err(SocketClosed);
            };
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
        self.take()
    }

    /// The next datagram of the two lanes, the one whose turn it is first.
    fn take(&mut self) -> Option<Received<'a, 'm, B, SIZE, H>> {
        let lent = &self.lent;
        let copies = self
            .socket
            .inbound
            .as_ref()
            .map(|inbound| &inbound.datagrams);
        let copied = || {
            copies
                .and_then(|lane| lane.try_receive())
                .map(Received::Copied)
        };
        let loaned = || lent.try_receive().map(Received::Lent);
        let received = if self.copy_next {
            copied().or_else(loaned)
        } else {
            loaned().or_else(copied)
        };
        self.copy_next = matches!(received, Some(Received::Lent(_)));
        received
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::boxed::Box;
    use std::vec::Vec;

    use oer_protocols_network_core::{Datagram, Pool, Remote, claim};
    use oer_protocols_network_port::{EthernetFrame, EthernetFrameView, FrameLengthError};

    use super::{LendingLanes, LendingSocket, LentDatagram, LentEnd, Received, lending_lanes};
    use crate::socket::ServiceEnd;

    const SIZE: usize = 64;
    const LANE: usize = 16;
    const PEER: Remote = Remote {
        address: [192, 0, 2, 2],
        port: 7,
    };

    /// A lent buffer of the test's own: its octets are its payload.
    struct Frame(Vec<u8>);

    impl EthernetFrame for Frame {
        fn view(&mut self) -> Result<EthernetFrameView<'_>, FrameLengthError> {
            Ok(EthernetFrameView {
                destination: [2; 6],
                source: [4; 6],
                ether_type: 0x0800,
                payload: &self.0,
            })
        }
    }

    type Socket<B> = LendingSocket<'static, 'static, B, SIZE, 4, LANE, 2, Frame>;

    struct Ends<B: 'static> {
        socket: Socket<B>,
        end: ServiceEnd<'static, 'static, B, SIZE, 4, LANE, 2>,
        lent: LentEnd<'static, B, Frame, LANE>,
        pool: Pool<'static, B, SIZE, 1>,
    }

    fn ends<B: 'static>() -> Ends<B> {
        let memory = Box::leak(Box::new([[0; SIZE]; 1]));
        let (mut pool, _brand) = claim::<B, SIZE, 1>(memory).unwrap();
        pool.loans().pool(LANE, 0).unwrap();
        pool.loans().grant(0, LANE).unwrap();
        let storage = Box::leak(Box::new(LendingLanes::new()));
        let (socket, end, lent) = lending_lanes(storage);
        Ends {
            socket,
            end,
            lent,
            pool,
        }
    }

    /// A format copy arrives first, then `LANE` loans.
    fn copy_then_loans<B>(ends: &mut Ends<B>) {
        let mut block = ends.pool.take().unwrap();
        block.fill(b"copy").unwrap();
        assert!(
            ends.end
                .datagrams
                .try_send(Datagram {
                    remote: PEER,
                    block,
                })
                .is_ok()
        );
        for n in 0..LANE {
            let loan = ends.pool.loans().lend(0).unwrap();
            let datagram = LentDatagram {
                flow: 0,
                remote: PEER,
                data: 0..1,
                frame: Frame(std::vec![n as u8]),
                loan,
            };
            assert!(ends.lent.lend(datagram).is_ok());
        }
    }

    /// What each receive gave: `true` for the copy.
    fn order<B>(
        mut receive: impl FnMut() -> Option<Received<'static, 'static, B, SIZE, Frame>>,
    ) -> Vec<bool> {
        core::iter::from_fn(|| receive())
            .map(|received| matches!(received, Received::Copied(_)))
            .collect()
    }

    #[test]
    fn a_copy_behind_loans_comes_within_two_tries() {
        struct Net;
        let mut ends = ends::<Net>();
        copy_then_loans(&mut ends);
        let socket = &mut ends.socket;
        let order = order(|| socket.try_receive());
        assert_eq!(order.len(), LANE + 1, "everything comes");
        assert_eq!(order.iter().position(|copy| *copy), Some(1));
    }

    #[test]
    fn a_copy_behind_loans_comes_within_two_receives() {
        struct Net;
        let mut ends = ends::<Net>();
        copy_then_loans(&mut ends);
        let socket = &mut ends.socket;
        let cx = &mut core::task::Context::from_waker(core::task::Waker::noop());
        let order = order(|| {
            let mut receive = core::pin::pin!(socket.receive());
            match receive.as_mut().poll(cx) {
                core::task::Poll::Ready(received) => received.ok(),
                core::task::Poll::Pending => None,
            }
        });
        assert_eq!(order.len(), LANE + 1, "everything comes");
        assert_eq!(order.iter().position(|copy| *copy), Some(1));
    }

    #[test]
    fn while_both_lanes_hold_datagrams_receives_alternate() {
        struct Net;
        let mut ends = ends::<Net>();
        copy_then_loans(&mut ends);
        let first = ends.socket.try_receive();
        assert!(matches!(first, Some(Received::Lent(_))));
        let Some(Received::Copied(copy)) = ends.socket.try_receive() else {
            panic!("the copy's turn");
        };
        assert_eq!(copy.block.octets(), b"copy");
        // A second copy waits behind one loan only.
        assert!(ends.end.datagrams.try_send(copy).is_ok());
        assert!(matches!(ends.socket.try_receive(), Some(Received::Lent(_))));
        assert!(matches!(
            ends.socket.try_receive(),
            Some(Received::Copied(_))
        ));
        drop(first);
    }
}
