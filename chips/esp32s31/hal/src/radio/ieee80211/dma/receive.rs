//! The Wi-Fi receive ring over 5.1's arena: the walker's descriptors, the
//! buffers lent to it and the atoms that exchange them.
//!
//! The ring is `N` descriptors and, right after them, the reserved
//! descriptor R. Every descriptor always holds a buffer lent to the walker:
//! a unit is never taken without its replacement in the same step. The cold
//! publication arms them all (BUFFER, NEXT, CONTROL with OWNER last), links
//! them in ring order with the last one's link zero (the accepted tail),
//! gives R its own buffer and the ring's head as its link, and publishes R
//! as `RX_DESCRIPTOR_BASE`, once (`ROM_REV0_WDEV_APPEND_RX_BLOCKS`). R is
//! outside the rotation: its buffer stays lent until the ring's release, and
//! a frame that lands in it before the walker reaches the head is not one of
//! the ring's units.
//!
//! [`RxRing::serve`] gives its caller (the keeper) a view of the units the
//! walker completed, in the ring's order up to `LAST` inclusive, read once
//! at its start. Each unit is consumed by one of [`Single::lend`] (its
//! buffer out, a spare armed in its descriptor: `HW_RX_REBIND_BEFORE_LAST_20261118`,
//! `HW_RX_REBIND_AT_LAST_20261118`) or `copy_in_place` (its frame copied
//! out through a [`Frame`] reader, the same buffers re-armed:
//! `HW_RX_CHAIN_LAPS_20261117`), or dropped: it stays completed in the ring
//! and the view yields nothing after it, so the ring's order is the order of
//! its re-arming. Each re-armed descriptor is linked behind the accepted
//! tail as it is armed; after the view, `serve` rings the reload doorbell if
//! anything was re-armed, and once more when the walker settled with NEXT
//! zero and LAST short of the new tail
//! (`HW_RX_EXHAUSTED_TAIL_APPEND_EXIT_20261119`,
//! `HW_RX_STALL_UNDER_LOAD_EXIT_20261119`). The caller never names a
//! descriptor.

use core::mem::{MaybeUninit, replace};

use oer_portable_memory::arena::{Arena, Brand, Handle, Lent, Quiet, Released, Span};
use oer_portable_time::{
    Clock,
    settle::{Settle, Unsettled, settle},
};

use crate::layouts::{DmaRef, DmaWindow, WifiRxDmaDescriptor, WifiRxDmaDescriptorControl};
use crate::radio::ieee80211::dma::descriptor::{RxDescriptor, cursor_ring_index, rx_armed_word};
use crate::radio::ieee80211::dma::ring_index::RingIndex;
use crate::radio::ieee80211::dma::space::DmaSpace;
use crate::radio::ieee80211::registers::{MacPlacement, WifiRxDmaRegisters};
use crate::system::memory::dma_address::{PlacedTable, PlacementProjection};
use crate::system::memory::dma_static::Placed;

#[cfg(test)]
mod tests;

/// The descriptors of a receive ring: the ring and, right after it, the
/// reserved descriptor R. Placed by the image (`dma_static!`).
#[repr(C)]
#[derive(bytemuck::Zeroable)]
pub struct RxDescriptors<const N: usize> {
    ring: [RxDescriptor; N],
    reserved: RxDescriptor,
}

/// What the ring reads of and writes to the walker's registers.
///
/// Sealed: [`WifiRxDmaRegisters`] is its implementation, and this module's
/// host tests model the walker behind it.
pub trait Walker: sealed::Sealed {
    /// `RX_LAST_DESCRIPTOR` then `RX_NEXT_DESCRIPTOR`, each read after the
    /// one before, then a device fence: their low address bits.
    fn cursor(&mut self) -> (u32, u32);
    /// Select the descriptors' address window.
    fn configure_window(&mut self);
    /// Publish `RX_DESCRIPTOR_BASE`.
    fn write_base(&mut self, head: DmaRef<WifiRxDmaDescriptor>);
    /// Start the walker.
    fn enable(&mut self);
    /// Ring the append/reload doorbell.
    fn ring(&mut self);
    /// Whether the walker accepted the doorbell.
    fn reload(&mut self) -> Reload;
    /// Order descriptor memory and MMIO.
    fn fence(&mut self);
}

mod sealed {
    pub trait Sealed {}
    impl<M: crate::radio::ieee80211::registers::MacPlacement> Sealed
        for crate::radio::ieee80211::registers::WifiRxDmaRegisters<M>
    {
    }
    #[cfg(test)]
    impl Sealed for super::tests::ModelWalker {}
}

impl<M: MacPlacement> Walker for WifiRxDmaRegisters<M> {
    fn cursor(&mut self) -> (u32, u32) {
        let last = self.mac_rx_last_descriptor_low();
        self.order_device_accesses();
        let next = self.mac_rx_next_descriptor().address_low();
        self.order_device_accesses();
        (last, next)
    }

    fn configure_window(&mut self) {
        self.configure_mac_rx_descriptor_window();
    }

    fn write_base(&mut self, head: DmaRef<WifiRxDmaDescriptor>) {
        self.write_mac_rx_descriptor_base(head);
    }

    fn enable(&mut self) {
        self.publish_mac_rx_walker_enable();
    }

    fn ring(&mut self) {
        self.request_mac_rx_descriptor_reload();
    }

    fn reload(&mut self) -> Reload {
        if self.mac_rx_reload_pending() {
            Reload::Pending
        } else {
            Reload::Accepted
        }
    }

    fn fence(&mut self) {
        self.order_device_accesses();
    }
}

/// The doorbell's state: still pending, or accepted by the walker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reload {
    Pending,
    Accepted,
}

/// Why a cold publication was refused: the table's placement has no DMA
/// address in the space. Nothing was written to the walker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Unplaced;

/// The doorbell stayed pending past its settle: the walker did not accept
/// the append. What was re-armed is linked; the ring's owner treats it as
/// the hardware's anomaly.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DoorbellUnsettled;

/// A cold publication refused, with every owner it was given.
pub struct PublishRefused<B: Brand, const N: usize, W> {
    pub reason: Unplaced,
    pub table: Placed<RxDescriptors<N>>,
    pub walker: W,
    pub ring: [Handle<B>; N],
    pub reserved: Handle<B>,
}

/// The receive ring: its descriptors, the buffers lent in them, the arena
/// they come from and the walker's registers.
pub struct RxRing<B, const BYTES: usize, const N: usize, W>
where
    B: Brand<Window = DmaWindow>,
{
    descriptors: &'static RxDescriptors<N>,
    addresses: PlacedTable<WifiRxDmaDescriptor>,
    arena: Arena<B, BYTES>,
    slots: [Lent<B>; N],
    _reserved: Lent<B>,
    walker: W,
    /// The oldest descriptor not yet consumed, in the ring's order.
    start: RingIndex<N>,
    /// The accepted tail: the descriptor whose link is zero.
    tail: RingIndex<N>,
}

impl<B, const BYTES: usize, const N: usize, W> RxRing<B, BYTES, N, W>
where
    B: Brand<Window = DmaWindow>,
    W: Walker,
{
    /// Every descriptor's armed control word: `OWNER` set, `RX_DONE` clear,
    /// its size and length the buffer's.
    const ARMED: WifiRxDmaDescriptorControl = match rx_armed_word(B::SIZE as u32) {
        Some(word) => word,
        None => panic!("a receive buffer's size fits a descriptor's size field"),
    };

    /// The ring's first and last positions; a ring has two descriptors at
    /// least and its floor is the unit's header.
    const HEAD: RingIndex<N> = match RingIndex::checked(0) {
        Some(head) if N >= 2 => head,
        _ => panic!("a receive ring has two descriptors at least"),
    };
    const LAST: RingIndex<N> = Self::HEAD.previous();
    const HEADER: () = if B::FLOOR
        != crate::layouts::WifiRxControlPrefix::SIZE + crate::layouts::WifiRxControlTail::SIZE
    {
        panic!("a receive buffer's floor is the unit's header")
    };

    /// The cold publication, built in its place: every descriptor armed with
    /// one of `ring`'s buffers and linked to the next, the last one's link
    /// zero; R armed with `reserved` and linked to the head; the window
    /// selected and BASE = R. The walker is not started
    /// ([`Self::enable_walker`]).
    ///
    /// # Errors
    ///
    /// [`Unplaced`] when the table has no DMA address in the space `S`: every
    /// owner comes back, and nothing was published.
    #[allow(
        clippy::result_large_err,
        reason = "a refusal returns every owner it was given"
    )]
    pub fn publish_cold<S: DmaSpace>(
        place: &mut MaybeUninit<Self>,
        table: Placed<RxDescriptors<N>>,
        mut walker: W,
        arena: Arena<B, BYTES>,
        ring: [Handle<B>; N],
        reserved: Handle<B>,
    ) -> Result<&mut Self, PublishRefused<B, N, W>> {
        let () = Self::HEADER;
        let projected = PlacementProjection::try_with::<S, _, _>(table, |table, projection| {
            projection.table(
                table
                    .ring
                    .iter()
                    .chain(core::iter::once(&table.reserved))
                    .map(RxDescriptor::layout),
            )
        });
        let (descriptors, addresses) = match projected {
            Ok(projected) => projected,
            Err((_, table)) => {
                return Err(PublishRefused {
                    reason: Unplaced,
                    table,
                    walker,
                    ring,
                    reserved,
                });
            }
        };
        let descriptors: &'static RxDescriptors<N> = descriptors;
        walker.configure_window();
        let mut index = 0;
        let slots = ring.map(|handle| {
            let (lent, buffer) = arena.lend(handle);
            let next = if index + 1 < N {
                addresses.element(index + 1)
            } else {
                None
            };
            if let Some(descriptor) = descriptors.ring.get(index) {
                descriptor.publish_owned(Self::ARMED, buffer, next);
            }
            index += 1;
            lent
        });
        let (reserved, buffer) = arena.lend(reserved);
        descriptors
            .reserved
            .publish_owned(Self::ARMED, buffer, addresses.element(0));
        walker.fence();
        if let Some(r) = addresses.element(N) {
            walker.write_base(r);
        }
        walker.fence();
        Ok(place.write(Self {
            descriptors,
            addresses,
            arena,
            slots,
            _reserved: reserved,
            walker,
            start: Self::HEAD,
            tail: Self::LAST,
        }))
    }

    /// Start the walker on the published ring.
    pub fn enable_walker(&mut self) {
        self.walker.enable();
    }

    /// Serve the completed units: `f` gets the view, in the ring's order up
    /// to `LAST` as read at the start; after it, the doorbell is rung if
    /// anything was re-armed, and once more when the walker settled with
    /// NEXT zero and LAST short of the tail. `settle` bounds each wait for
    /// the doorbell's acceptance, a named settle.
    ///
    /// # Errors
    ///
    /// [`DoorbellUnsettled`] when the doorbell stayed pending past `settle`:
    /// `f`'s result comes with it, and every re-armed descriptor is linked.
    pub fn serve<C: Clock + ?Sized, R>(
        &mut self,
        settle: Settle<'_, C>,
        f: impl FnOnce(&mut Completed<'_, B, BYTES, N, W>) -> R,
    ) -> Result<R, (DoorbellUnsettled, R)> {
        let (last, next) = self.walker.cursor();
        let completed =
            self.completed(cursor_ring_index(last, self.addresses.base(), N), next == 0);
        let mut view = Completed {
            ring: self,
            remaining: completed,
            yielded: 0,
            rearmed: 0,
        };
        let result = f(&mut view);
        if view.rearmed == 0 {
            return Ok(result);
        }
        match self.ring_doorbell(settle) {
            Ok(()) => Ok(result),
            Err(Unsettled) => Err((DoorbellUnsettled, result)),
        }
    }

    /// Descriptors completed from `start` up to `LAST` inclusive. `LAST`
    /// one before `start` is none completed, or the whole ring when the
    /// walker is exhausted (NEXT zero): with the tail written no armed
    /// descriptor is left.
    fn completed(&self, last: Option<usize>, exhausted: bool) -> usize {
        let Some(last) = last.and_then(RingIndex::<N>::checked) else {
            return 0;
        };
        let distance = self.start.distance_to(last) + 1;
        match (distance == N, exhausted) {
            (true, false) => 0,
            _ => distance,
        }
    }

    /// Ring the doorbell, wait for its acceptance, and ring once more when
    /// the walker settled with NEXT zero and LAST short of the tail.
    fn ring_doorbell<C: Clock + ?Sized>(&mut self, within: Settle<'_, C>) -> Result<(), Unsettled> {
        let tail = self.tail.get();
        let base = self.addresses.base();
        let walker = &mut self.walker;
        walker.fence();
        walker.ring();
        walker.fence();
        settle(within, || {
            (walker.reload() == Reload::Accepted).then_some(())
        })?;
        let (last, next) = walker.cursor();
        if next == 0 && cursor_ring_index(last, base, N) != Some(tail) {
            walker.fence();
            walker.ring();
            walker.fence();
            settle(within, || {
                (walker.reload() == Reload::Accepted).then_some(())
            })?;
        }
        Ok(())
    }

    /// Link the re-armed run from `first` to `last` behind the accepted tail:
    /// the old tail's link names `first`, and `last` is the tail.
    fn link(&mut self, first: RingIndex<N>, last: RingIndex<N>) {
        self.walker.fence();
        if let (Some(tail), Some(first)) = (
            self.descriptors.ring.get(self.tail.get()),
            self.addresses.element(first.get()),
        ) {
            tail.publish_next(Some(first));
        }
        self.walker.fence();
        self.tail = last;
        self.start = last.advance(1);
    }

    fn descriptor(&self, index: RingIndex<N>) -> Option<&RxDescriptor> {
        self.descriptors.ring.get(index.get())
    }
}

/// The completed units of one service ([`RxRing::serve`]), in the ring's
/// order up to the `LAST` read at its start.
pub struct Completed<'r, B, const BYTES: usize, const N: usize, W>
where
    B: Brand<Window = DmaWindow>,
{
    ring: &'r mut RxRing<B, BYTES, N, W>,
    /// Completed descriptors from the ring's `start` not yet yielded.
    remaining: usize,
    /// Descriptors yielded since the service began.
    yielded: usize,
    /// Descriptors re-armed since the service began.
    rearmed: usize,
}

/// One completed unit: one descriptor, or a chain of several, which is only
/// copied.
pub enum Unit<'v, 'r, B, const BYTES: usize, const N: usize, W>
where
    B: Brand<Window = DmaWindow>,
{
    Single(Single<'v, 'r, B, BYTES, N, W>),
    Chain(Chain<'v, 'r, B, BYTES, N, W>),
}

impl<'r, B, const BYTES: usize, const N: usize, W> Completed<'r, B, BYTES, N, W>
where
    B: Brand<Window = DmaWindow>,
    W: Walker,
{
    /// The next completed unit, or `None`: none is left up to `LAST`, the
    /// next one is not complete at `LAST`, or a unit before it was dropped
    /// and stays in the ring (what follows it waits for it).
    pub fn next_unit(&mut self) -> Option<Unit<'_, 'r, B, BYTES, N, W>> {
        if self.yielded != self.rearmed {
            return None;
        }
        let head = self.ring.start;
        let count = (1..=self.remaining).find(|&count| {
            self.ring
                .descriptor(head.advance(count - 1))
                .is_some_and(|descriptor| descriptor.word0().rx_done())
        })?;
        self.yielded += count;
        self.remaining -= count;
        Some(if count == 1 {
            Unit::Single(Single {
                view: self,
                index: head,
            })
        } else {
            Unit::Chain(Chain {
                view: self,
                head,
                count,
            })
        })
    }
}

/// A completed unit of one descriptor.
pub struct Single<'v, 'r, B, const BYTES: usize, const N: usize, W>
where
    B: Brand<Window = DmaWindow>,
{
    view: &'v mut Completed<'r, B, BYTES, N, W>,
    index: RingIndex<N>,
}

/// A completed unit of `count` descriptors from `head`, in the ring's order.
pub struct Chain<'v, 'r, B, const BYTES: usize, const N: usize, W>
where
    B: Brand<Window = DmaWindow>,
{
    view: &'v mut Completed<'r, B, BYTES, N, W>,
    head: RingIndex<N>,
    count: usize,
}

/// The bytes the walker wrote in a descriptor's buffer (its `LENGTH`),
/// within the buffer.
fn written<B: Brand>(word0: WifiRxDmaDescriptorControl) -> usize {
    usize::from(word0.length()).min(B::SIZE)
}

impl<B, const BYTES: usize, const N: usize, W> Single<'_, '_, B, BYTES, N, W>
where
    B: Brand<Window = DmaWindow>,
    W: Walker,
{
    /// The bytes the walker wrote in the unit's buffer, its header included:
    /// what the keeper decides by, read from the descriptor's own word.
    pub fn written(&self) -> usize {
        self.view
            .ring
            .descriptor(self.index)
            .map_or(0, |descriptor| written::<B>(descriptor.word0()))
    }

    /// The unit's buffer out, `spare` armed in its descriptor and linked
    /// behind the tail: the exchange of the rebind records. The handle's span
    /// is the frame, from the floor to the bytes the walker wrote; its holder
    /// reads the header from the floor ([`Arena::floor`]).
    pub fn lend(self, spare: Handle<B>) -> Handle<B> {
        let written = self.written();
        let ring = &mut *self.view.ring;
        let index = self.index;
        let Some(slot) = ring.slots.get_mut(index.get()) else {
            // A `RingIndex<N>` lies inside the `N` slots: never reached. The
            // spare comes back to the caller, unlent; nothing was published.
            return spare;
        };
        let (spare, buffer) = ring.arena.lend(spare);
        let unit = replace(slot, spare);
        if let Some(descriptor) = ring.descriptors.ring.get(index.get()) {
            descriptor.publish_owned(RxRing::<B, BYTES, N, W>::ARMED, buffer, None);
        }
        ring.link(index, index);
        self.view.rearmed += 1;
        let span = Span::new(B::FLOOR, written.saturating_sub(B::FLOOR)).unwrap_or(Span::FULL);
        #[allow(
            unsafe_code,
            reason = "the receive ring's release proof: a completed unit"
        )]
        // SAFETY: the descriptor lies at or before the `LAST` the walker
        // reported at this service's start (`serve`), so the walker completed
        // it and moved on, and its descriptor now holds the spare: the walker
        // does not write this buffer again (`HW_RX_REBIND_BEFORE_LAST_20261118`,
        // `HW_RX_REBIND_AT_LAST_20261118`). The span begins at the floor: the
        // header's late bytes 48 to 52 lie under no span
        // (`HW_RX_HEADER_LATE_WRITE_20261118`).
        let released = unsafe { Released::assume(unit, span) };
        ring.arena.reclaim(released)
    }

    /// The unit's frame copied out by `f` through its reader; the same buffer
    /// re-armed and linked behind the tail after `f` returns.
    pub fn copy_in_place<R>(self, f: impl FnOnce(&Frame<'_, B, BYTES, N>) -> R) -> R {
        Chain {
            view: self.view,
            head: self.index,
            count: 1,
        }
        .copy_in_place(f)
    }
}

impl<B, const BYTES: usize, const N: usize, W> Chain<'_, '_, B, BYTES, N, W>
where
    B: Brand<Window = DmaWindow>,
    W: Walker,
{
    /// The chain's frame copied out by `f` through its reader; its buffers
    /// re-armed in place, in order, and linked behind the tail after `f`
    /// returns.
    pub fn copy_in_place<R>(self, f: impl FnOnce(&Frame<'_, B, BYTES, N>) -> R) -> R {
        let ring = &mut *self.view.ring;
        let frame = Frame {
            arena: &ring.arena,
            slots: &ring.slots,
            descriptors: &ring.descriptors.ring,
            head: self.head,
            count: self.count,
        };
        let result = f(&frame);
        let last = self.head.advance(self.count - 1);
        for offset in 0..self.count {
            let index = self.head.advance(offset);
            let next = (offset + 1 < self.count)
                .then(|| index.advance(1))
                .and_then(|next| ring.addresses.element(next.get()));
            if let Some(descriptor) = ring.descriptor(index) {
                descriptor.publish_next(next);
                descriptor.publish_word0(RxRing::<B, BYTES, N, W>::ARMED);
            }
        }
        ring.link(self.head, last);
        self.view.rearmed += self.count;
        result
    }
}

/// The frame of a unit being copied: its reader is the only access to its
/// bytes, which it copies out in order. The first buffer's bytes begin at
/// the floor (below it lies the header); a continuation's begin at its
/// first byte (`HW_RX_CHAIN_LAPS_20261117`: `(n - 1) × SIZE` plus the last
/// descriptor's `LENGTH` are the unit's bytes).
pub struct Frame<'f, B, const BYTES: usize, const N: usize>
where
    B: Brand<Window = DmaWindow>,
{
    arena: &'f Arena<B, BYTES>,
    slots: &'f [Lent<B>; N],
    descriptors: &'f [RxDescriptor; N],
    head: RingIndex<N>,
    count: usize,
}

impl<B, const BYTES: usize, const N: usize> Frame<'_, B, BYTES, N>
where
    B: Brand<Window = DmaWindow>,
{
    /// The frame's bytes: what the walker wrote, less the first buffer's
    /// header.
    pub fn len(&self) -> usize {
        (0..self.count)
            .map(|offset| self.written(offset))
            .sum::<usize>()
            .saturating_sub(B::FLOOR)
    }

    /// Whether the frame has no byte.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Copy the frame's first bytes into `out`, as many as fit: the count
    /// copied.
    pub fn copy_to(&self, out: &mut [u8]) -> usize {
        let mut copied = 0;
        for offset in 0..self.count {
            let written = self.written(offset);
            let Some(lent) = self.slots.get(self.head.advance(offset).get()) else {
                break;
            };
            let span = Span::new(B::FLOOR, written.saturating_sub(B::FLOOR)).unwrap_or(Span::FULL);
            #[allow(
                unsafe_code,
                reason = "the receive ring's borrowed release: a completed chain"
            )]
            // SAFETY: the unit's descriptors lie at or before the `LAST` the
            // walker reported at this service's start, and none is re-armed
            // before the copy ends (`Chain::copy_in_place` re-arms after `f`
            // returns, and this reader lives only inside `f`): the walker
            // completed them and does not write their buffers meanwhile
            // (`HW_RX_CHAIN_LAPS_20261117`).
            let quiet = unsafe { Quiet::assume(lent, span) };
            copied = self.arena.inspect(&quiet, |handle| {
                let mut copied = copied;
                if offset != 0 {
                    let floor: [u8; 64] = self.arena.floor(handle).copy();
                    let head = floor.get(..written.min(B::FLOOR)).unwrap_or(&[]);
                    copied += copy_into(head, out.get_mut(copied..).unwrap_or(&mut []));
                }
                copied += copy_into(
                    self.arena.bytes(handle),
                    out.get_mut(copied..).unwrap_or(&mut []),
                );
                copied
            });
        }
        copied
    }

    fn written(&self, offset: usize) -> usize {
        self.descriptors
            .get(self.head.advance(offset).get())
            .map_or(0, |descriptor| written::<B>(descriptor.word0()))
    }
}

/// Copy as much of `from` as fits in `to`: the count.
fn copy_into(from: &[u8], to: &mut [u8]) -> usize {
    let count = from.len().min(to.len());
    if let (Some(to), Some(from)) = (to.get_mut(..count), from.get(..count)) {
        to.copy_from_slice(from);
    }
    count
}
