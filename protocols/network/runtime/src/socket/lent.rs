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

mod prefix;
pub use prefix::ReceivedPrefix;

use core::future::{Future, poll_fn};
use core::ops::Range;
use core::pin::pin;
use core::sync::atomic::{AtomicU32, Ordering};
use core::task::{Context, Poll};

use oer_portable_memory::{AtomicWaker, WakingSpscQueue, WakingSpscReceiver, WakingSpscSender};
use oer_protocols_network_core::{Datagram, GROUPS, LentDatagram, Loan, Loans, Remote, Unlent};
use oer_protocols_network_port::{EthernetFrame, FrameLengthError};

use super::{Socket, SocketClosed, SocketLanes, lanes};

/// Lent buffers back with their port whose loans the service has not yet
/// given back.
pub(crate) struct Returns {
    back: AtomicU32,
    waker: AtomicWaker,
    servicing: AtomicU32,
}

impl Returns {
    const fn new() -> Self {
        Self {
            back: AtomicU32::new(0),
            waker: AtomicWaker::new(),
            servicing: AtomicU32::new(0),
        }
    }

    /// Out of line, so its wake keeps its own `Waker::wake` debug scope and
    /// the image's stack proof resolves it, as the service's `closed` and the
    /// station's same-hart return do: a caller that drops a [`Lent`] on
    /// several paths (each `or_else` of [`LendingSocket::take`]) can
    /// otherwise merge their inlined wakes into one block without that scope.
    #[inline(never)]
    fn give(&self, count: u32) {
        // At most a socket's credit is out, far below the word's range.
        self.back.fetch_add(count, Ordering::AcqRel);
        self.waker.wake();
    }

    const READY: u32 = 1;
    const OFFERED: u32 = 2;

    /// Only a registered consumer can request one intake deferral after a
    /// publication. A pending child future may be kept without polling it,
    /// so this readiness is never permission to wait for its completion.
    fn advertise(&self) {
        self.servicing.fetch_or(Self::READY, Ordering::AcqRel);
    }

    fn active(&self) {
        self.servicing.fetch_and(!Self::READY, Ordering::AcqRel);
    }

    fn offer(&self) {
        let _offered = self
            .servicing
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                (state & Self::READY != 0).then_some(state | Self::OFFERED)
            });
    }

    fn servicing(&self) -> bool {
        self.servicing.load(Ordering::Acquire) & Self::OFFERED != 0
    }

    /// Consume the publication's one deferral. The next intake does not
    /// depend on the consumer polling or cancelling its receive future.
    fn take_offer(&self) -> bool {
        if !self.servicing() {
            return false;
        }
        self.servicing.fetch_and(!Self::OFFERED, Ordering::AcqRel) & Self::OFFERED != 0
    }

    /// The synchronous prefix ended (or its wait was cancelled). Payload
    /// returns precede this credit publication and continuation wake.
    fn finished(&self, count: u32) {
        if count != 0 {
            self.back.fetch_add(count, Ordering::AcqRel);
        }
        let offered = self.servicing.swap(0, Ordering::AcqRel) & Self::OFFERED != 0;
        if count != 0 || offered {
            self.waker.wake();
        }
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
struct Back<'a>(Option<&'a Returns>);

impl Drop for Back<'_> {
    fn drop(&mut self) {
        if let Some(returns) = self.0.take() {
            returns.give(1);
        }
    }
}

/// No ownership is taken by a pending receive. Ending or cancelling that
/// wait withdraws its readiness and any unused deferral, independently of loans.
struct Waiting<'a>(Option<&'a Returns>);
impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        if let Some(returns) = self.0.take() {
            returns.finished(0);
        }
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
    pub(crate) lent: WakingSpscQueue<Option<Lent<'a, H>>, RECEIVED>,
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
    lent: WakingSpscSender<'a, Option<Lent<'a, H>>, RECEIVED>,
    returns: &'a Returns,
    loans: [Option<Loan<B>>; RECEIVED],
    staged: bool,
}

impl<'a, B, H, const RECEIVED: usize> LentEnd<'a, B, H, RECEIVED> {
    /// Hand `datagram` to the socket, within a batch that ends with
    /// [`Self::flush`] before its service waits.
    ///
    /// # Errors
    /// The lane had no room ([`Unlent::LaneFull`]): the buffer stays in the
    /// source cell and the loan is returned. The caller drops that buffer
    /// before giving the loan back to its account.
    pub(crate) fn lend(
        &mut self,
        datagram: LentDatagram<B>,
        frame: &mut Option<H>,
    ) -> Result<(), Unlent<B>> {
        let LentDatagram {
            remote, data, loan, ..
        } = datagram;
        let Some(place) = self.loans.iter_mut().find(|place| place.is_none()) else {
            // More out than the credit, which the lane bounds: the buffer
            // goes back before its loan.
            return Err(Unlent::LaneFull(Some(loan)));
        };
        let mut loan = Some(loan);
        // Build at the socket's vacant cell. Until it is reserved the frame
        // stays in the port's cell and the loan stays here. No intermediate
        // datagram owns the frame, and a refusal counts no synthetic return.
        let sent = self.lent.try_send_held_with(|| {
            let frame = frame.take()?;
            *place = loan.take();
            Some(Some(Lent {
                frame,
                data,
                remote,
                _back: Back(Some(self.returns)),
            }))
        });
        if sent {
            self.staged = true;
            Ok(())
        } else {
            Err(Unlent::LaneFull(loan))
        }
    }

    /// End a batch of lends: the socket hears its datagrams once.
    pub(crate) fn flush(&mut self) {
        if core::mem::take(&mut self.staged) {
            self.returns.offer();
        }
        self.lent.flush();
    }

    pub(crate) fn defer_intake(&self) -> bool {
        self.returns.take_offer()
    }

    pub(crate) fn offer_pending(&self) -> bool {
        self.returns.servicing()
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
            returns,
        },
        end,
        LentEnd {
            lent: sender,
            returns,
            loans: [const { None }; RECEIVED],
            staged: false,
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
    lent: WakingSpscReceiver<'a, Option<Lent<'a, H>>, RECEIVED>,
    returns: &'a Returns,
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

    #[inline(never)]
    fn receive_control(&mut self) {
        self.socket.take_answers();
    }

    fn receive_control_pending(&self) -> bool {
        self.socket.credits.stopped.load(Ordering::Acquire)
            || self
                .socket
                .inbound
                .as_ref()
                .is_some_and(|inbound| !inbound.answers.is_empty())
    }

    /// The next received datagram, lent or copied.
    ///
    /// # Errors
    /// [`SocketClosed`], once all that arrived was read.
    pub async fn receive(&mut self) -> Result<Received<'a, 'm, B, SIZE, H>, SocketClosed> {
        let _waiting = Waiting(Some(self.returns));
        loop {
            self.returns.active();
            // In this order: nothing follows a close in the lanes.
            self.socket.take_answers();
            if let Some(received) = self.take() {
                return Ok(received);
            }
            if self.socket.closed {
                return Err(SocketClosed);
            }
            self.waiting().await?;
        }
    }

    async fn waiting(&mut self) -> Result<(), SocketClosed> {
        poll_fn(|cx| {
            let Some(inbound) = &self.socket.inbound else {
                return Poll::Ready(Err(SocketClosed));
            };
            let mut ready = self.lent.poll_ready(cx).is_ready();
            ready |= inbound.datagrams.poll_ready(cx).is_ready();
            ready |= inbound.answers.poll_ready(cx).is_ready();
            ready |= pin!(self.socket.credits.stopped()).poll(cx).is_ready();
            // Every source is registered before advertising. Recheck after
            // it: an offer before the advertisement is seen in the queues;
            // an offer after it sees readiness and wakes these registrations.
            self.returns.advertise();
            ready |= !self.lent.is_empty()
                || !inbound.datagrams.is_empty()
                || !inbound.answers.is_empty()
                || self.socket.credits.stopped.load(Ordering::Acquire);
            if ready {
                Poll::Ready(Ok(()))
            } else {
                Poll::Pending
            }
        })
        .await
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
        let loaned = || lent.try_receive().flatten().map(Received::Lent);
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

    impl oer_portable_memory::BatchReturn for Frame {
        type Batch = ();
    }

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

    fn loan_prefix<B: 'static>(ends: &mut Ends<B>, count: usize) {
        for id in 0..count {
            let loan = ends.pool.loans().lend(0).unwrap();
            let datagram = LentDatagram {
                flow: 0,
                remote: PEER,
                data: 0..1,
                loan,
            };
            assert!(
                ends.lent
                    .lend(datagram, &mut Some(Frame(std::vec![id as u8])))
                    .is_ok()
            );
        }
        ends.lent.flush();
    }

    #[test]
    fn a_prefix_publishes_returned_credit_once_and_keeps_a_retained_owner_out() {
        use core::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        use std::task::{Wake, Waker};
        struct Wakes(AtomicUsize);
        impl Wake for Wakes {
            fn wake(self: Arc<Self>) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        struct Brand;
        let mut ends = ends::<Brand>();
        loan_prefix(&mut ends, 8);
        let returns = ends.lent.returns;
        let wakes = Arc::new(Wakes(AtomicUsize::new(0)));
        let waker = Waker::from(wakes.clone());
        assert!(!ends.lent.poll(&core::task::Context::from_waker(&waker)));
        let mut retained = None;
        let prefix = ends
            .socket
            .try_receive_prefix(8, |cell| {
                assert_eq!(returns.published(), 0, "no partial credit publication");
                if cell.as_mut().unwrap().data().unwrap()[0] == 3 {
                    retained = cell.take();
                }
                true
            })
            .unwrap();
        assert!(matches!(prefix, super::ReceivedPrefix::Lent(8)));
        assert_eq!(returns.published(), 7);
        assert_eq!(wakes.0.load(Ordering::SeqCst), 1);
        ends.lent.collect(ends.pool.loans());
        assert_eq!(ends.pool.loans().outstanding(), 1);
        drop(retained);
        ends.lent.collect(ends.pool.loans());
        assert!(ends.pool.loans().released());
    }

    #[test]
    fn a_publication_defers_once_without_polling_or_cancelling_the_waiter() {
        struct Brand;
        let mut ends = ends::<Brand>();
        let cx = &mut core::task::Context::from_waker(core::task::Waker::noop());
        let mut wait = Box::pin(ends.socket.receive_prefix(8, |_| true));
        assert!(wait.as_mut().poll(cx).is_pending());
        for _ in 0..2 {
            let loan = ends.pool.loans().lend(0).unwrap();
            assert!(
                ends.lent
                    .lend(
                        LentDatagram {
                            flow: 0,
                            remote: PEER,
                            data: 0..1,
                            loan
                        },
                        &mut Some(Frame(std::vec![1]))
                    )
                    .is_ok()
            );
            ends.lent.flush();
            assert!(ends.lent.defer_intake());
            assert!(!ends.lent.defer_intake());
            ends.lent.flush();
            assert!(
                !ends.lent.defer_intake(),
                "a flush without new owners cannot rearm the offer"
            );
            assert_eq!(
                ends.lent.returns.published(),
                0,
                "deferral returns no owner or credit"
            );
        }
        assert_eq!(ends.pool.loans().outstanding(), 2);
        drop(wait);
        assert_eq!(ends.lent.returns.published(), 0);
        assert!(matches!(
            ends.socket.try_receive_prefix(8, |_| true),
            Some(super::ReceivedPrefix::Lent(2))
        ));
        ends.lent.collect(ends.pool.loans());
        assert!(ends.pool.loans().released());
    }

    #[test]
    fn replacing_a_prefix_cell_with_another_sockets_owner_keeps_both_accounts() {
        struct First;
        struct Second;
        let mut first = ends::<First>();
        let mut second = ends::<Second>();
        loan_prefix(&mut first, 1);
        loan_prefix(&mut second, 1);
        let Some(Received::Lent(other)) = second.socket.try_receive() else {
            panic!("the second socket lent its frame");
        };
        let mut replacement = Some(other);
        let mut retained = None;
        let prefix = first.socket.try_receive_prefix(8, |cell| {
            retained = core::mem::replace(cell, replacement.take());
            true
        });
        assert!(matches!(prefix, Some(super::ReceivedPrefix::Lent(1))));
        assert_eq!(first.lent.returns.published(), 0);
        assert_eq!(second.lent.returns.published(), 1);
        first.lent.collect(first.pool.loans());
        second.lent.collect(second.pool.loans());
        assert_eq!(first.pool.loans().outstanding(), 1);
        assert!(second.pool.loans().released());
        drop(retained);
        first.lent.collect(first.pool.loans());
        assert!(first.pool.loans().released());
    }

    #[test]
    fn physical_batch_accounting_precedes_credit_and_continuation_publication() {
        use core::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        use std::task::{Wake, Waker};
        struct CheckedFrame(Vec<u8>, Arc<AtomicUsize>);
        impl EthernetFrame for CheckedFrame {
            fn view(&mut self) -> Result<EthernetFrameView<'_>, FrameLengthError> {
                Ok(EthernetFrameView {
                    destination: [2; 6],
                    source: [4; 6],
                    ether_type: 0x0800,
                    payload: &self.0,
                })
            }
        }
        #[derive(Default)]
        struct PhysicalBatch {
            returned: Option<Arc<AtomicUsize>>,
            count: usize,
        }
        impl Drop for PhysicalBatch {
            fn drop(&mut self) {
                if let Some(returned) = &self.returned {
                    returned.fetch_add(self.count, Ordering::Release);
                }
            }
        }
        impl oer_portable_memory::BatchReturn for CheckedFrame {
            type Batch = PhysicalBatch;
            fn return_in(self, batch: &mut PhysicalBatch) {
                let Self(bytes, returned) = self;
                drop(bytes);
                batch.returned = Some(returned);
                batch.count += 1;
            }
        }
        struct Observe {
            physical: Arc<AtomicUsize>,
            returns: &'static super::Returns,
        }
        impl Wake for Observe {
            fn wake(self: Arc<Self>) {
                assert_eq!(self.physical.load(Ordering::Acquire), 8);
                assert_eq!(self.returns.published(), 8);
                assert!(!self.returns.servicing());
            }
        }
        struct Brand;
        let memory = Box::leak(Box::new([[0; SIZE]; 1]));
        let (mut pool, _) = claim::<Brand, SIZE, 1>(memory).unwrap();
        pool.loans().pool(LANE, 0).unwrap();
        pool.loans().grant(0, LANE).unwrap();
        let storage = Box::leak(Box::new(LendingLanes::<
            Brand,
            SIZE,
            4,
            LANE,
            2,
            CheckedFrame,
        >::new()));
        let (mut socket, _end, mut lent) = lending_lanes(storage);
        let physical = Arc::new(AtomicUsize::new(0));
        lent.returns.advertise();
        for id in 0..8 {
            let loan = pool.loans().lend(0).unwrap();
            let frame = CheckedFrame(std::vec![id], physical.clone());
            let sent = lent.lend(
                LentDatagram {
                    flow: 0,
                    remote: PEER,
                    data: 0..1,
                    loan,
                },
                &mut Some(frame),
            );
            assert!(sent.is_ok());
        }
        lent.flush();
        assert!(lent.offer_pending());
        let waker = Waker::from(Arc::new(Observe {
            physical: physical.clone(),
            returns: lent.returns,
        }));
        assert!(!lent.poll(&core::task::Context::from_waker(&waker)));
        let prefix = socket
            .try_receive_prefix(8, |_| {
                assert_eq!(physical.load(Ordering::Acquire), 0);
                assert_eq!(lent.returns.published(), 0);
                true
            })
            .unwrap();
        assert!(matches!(prefix, super::ReceivedPrefix::Lent(8)));
        lent.collect(pool.loans());
        assert!(pool.loans().released());
    }

    #[test]
    fn cancelling_an_advertised_prefix_wait_releases_servicing_without_returning_owners() {
        use core::future::Future;
        struct Brand;
        let mut ends = ends::<Brand>();
        let mut next = Box::pin(ends.socket.receive_prefix(8, |_| true));
        let cx = &mut core::task::Context::from_waker(core::task::Waker::noop());
        assert!(next.as_mut().poll(cx).is_pending());
        for id in 0..8 {
            let loan = ends.pool.loans().lend(0).unwrap();
            assert!(
                ends.lent
                    .lend(
                        LentDatagram {
                            flow: 0,
                            remote: PEER,
                            data: 0..1,
                            loan
                        },
                        &mut Some(Frame(std::vec![id]))
                    )
                    .is_ok()
            );
        }
        ends.lent.flush();
        assert!(ends.lent.offer_pending());
        assert_eq!(ends.lent.unreturned(), 0);
        drop(next);
        assert!(!ends.lent.offer_pending());
        assert_eq!(ends.pool.loans().outstanding(), 8);
        assert_eq!(
            ends.lent.unreturned(),
            0,
            "cancellation returns no synthetic credit"
        );
        let _prefix = ends.socket.try_receive_prefix(8, |_| true).unwrap();
        ends.lent.collect(ends.pool.loans());
        assert!(ends.pool.loans().released());
    }

    #[test]
    fn taking_an_owner_then_unwinding_keeps_it_out_and_the_remaining_prefix_valid() {
        struct Brand;
        let mut ends = ends::<Brand>();
        loan_prefix(&mut ends, 8);
        let mut kept = None;
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _prefix = ends.socket.try_receive_prefix(8, |cell| {
                    kept = cell.take();
                    panic!("after retaining the first owner");
                });
            }))
            .is_err()
        );
        let mut seen = Vec::new();
        let prefix = ends
            .socket
            .try_receive_prefix(8, |cell| {
                seen.push(cell.as_mut().unwrap().data().unwrap()[0]);
                true
            })
            .unwrap();
        assert!(matches!(prefix, super::ReceivedPrefix::Lent(7)));
        assert_eq!(seen, [1, 2, 3, 4, 5, 6, 7]);
        ends.lent.collect(ends.pool.loans());
        assert_eq!(ends.pool.loans().outstanding(), 1);
        drop(kept);
        ends.lent.collect(ends.pool.loans());
        assert!(ends.pool.loans().released());
    }

    #[test]
    fn a_prefix_stops_at_a_retained_frame_and_preserves_the_following_order() {
        struct Brand;
        let mut ends = ends::<Brand>();
        loan_prefix(&mut ends, 8);
        let mut kept = None;
        let mut seen = Vec::new();
        let _prefix = ends
            .socket
            .try_receive_prefix(8, |cell| {
                let id = cell.as_mut().unwrap().data().unwrap()[0];
                seen.push(id);
                if id == 2 {
                    kept = cell.take();
                    false
                } else {
                    true
                }
            })
            .unwrap();
        assert_eq!(seen, [0, 1, 2]);
        let _prefix = ends
            .socket
            .try_receive_prefix(8, |cell| {
                seen.push(cell.as_mut().unwrap().data().unwrap()[0]);
                true
            })
            .unwrap();
        assert_eq!(seen, [0, 1, 2, 3, 4, 5, 6, 7]);
        drop(kept);
        ends.lent.collect(ends.pool.loans());
        assert!(ends.pool.loans().released());
    }

    #[test]
    fn a_copied_datagram_waits_behind_at_most_one_loan_prefix() {
        struct Brand;
        let mut ends = ends::<Brand>();
        copy_then_loans(&mut ends);
        ends.lent.flush();
        let mut seen = Vec::new();
        let prefix = ends
            .socket
            .try_receive_prefix(8, |cell| {
                seen.push(cell.as_mut().unwrap().data().unwrap()[0]);
                true
            })
            .unwrap();
        assert!(matches!(prefix, super::ReceivedPrefix::Lent(1)));
        let prefix = ends
            .socket
            .try_receive_prefix(8, |_| panic!("the copy's turn"))
            .unwrap();
        let super::ReceivedPrefix::Copied(datagram) = prefix else {
            panic!("copy");
        };
        assert_eq!(datagram.block.octets(), b"copy");
        ends.pool.give(datagram.block);
        while ends
            .socket
            .try_receive_prefix(8, |cell| {
                seen.push(cell.as_mut().unwrap().data().unwrap()[0]);
                true
            })
            .is_some()
        {}
        assert_eq!(seen, (0..LANE as u8).collect::<Vec<_>>());
        ends.lent.collect(ends.pool.loans());
        assert!(ends.pool.loans().released());
        assert_eq!(ends.pool.outstanding(), 0);
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
                loan,
            };
            assert!(
                ends.lent
                    .lend(datagram, &mut Some(Frame(std::vec![n as u8])))
                    .is_ok()
            );
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
