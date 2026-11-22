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
//!
//! A unit's slots and descriptors are looked up once, where the view finds
//! it, and carried by reference: nothing after that looks anything up. A
//! length the walker must not write (a single unit's outside the floor and
//! the buffer, a chain's last one empty or past the buffer) and a doorbell the walker does not accept are the
//! hardware's anomalies ([`RingAnomaly`]): the view stops at the first, and
//! `serve` returns it to the keeper, which raises the one indication.

use core::mem::{MaybeUninit, replace};

use oer_portable_memory::Occupied;
use oer_portable_memory::arena::{Arena, Brand, Handle, Lent, Quiet, Released, Span};
use oer_portable_time::{
    Duration,
    settle::{Settle, SettleCounter, SettleLimit, Unsettled, settle},
};

use crate::layouts::{DmaRef, DmaWindow, WifiRxDmaDescriptor, WifiRxDmaDescriptorControl};
use crate::radio::ieee80211::dma::descriptor::{RxDescriptor, cursor_ring_index, rx_armed_word};
use crate::radio::ieee80211::dma::ring_index::RingIndex;
use crate::radio::ieee80211::dma::space::DmaSpace;
use crate::radio::ieee80211::init::MacStopReset;
use crate::radio::ieee80211::mac::MacCoreOpen;
use crate::radio::ieee80211::registers::{MacPlacement, WifiRxDmaRegisters};
use crate::system::memory::dma_address::PlacementProjection;
use crate::system::memory::dma_static::Placed;

#[cfg(test)]
mod tests;

/// How long the doorbell may stay pending once rung, a named settle.
///
/// SOURCE: the W1 runs of 2026-10-19 saw the doorbell accepted at its first
/// observation every time (`max_attempts` 1, the receive path's
/// `RELOAD_SETTLE_LIMIT`); `HW_RX_STALL_UNDER_LOAD_EXIT_20261119`'s 15 817
/// exits rang it under this limit, none unsettled.
pub const RELOAD_SETTLE: SettleLimit = match SettleLimit::new(Duration::from_micros(20)) {
    Some(limit) => limit,
    None => panic!("the doorbell's settle is a valid settle"),
};

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
    /// The walker's buffer configuration, before the first BASE.
    fn initialize_prefix(&mut self);
    /// Select the descriptors' address window.
    fn configure_window(&mut self);
    /// Publish `RX_DESCRIPTOR_BASE`.
    fn write_base(&mut self, head: DmaRef<WifiRxDmaDescriptor>);
    /// Start the walker.
    fn enable(&mut self);
    /// Clear the walker's enable: its edge.
    fn disable(&mut self);
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

    fn initialize_prefix(&mut self) {
        self.initialize_mac_rx_buffer_prefix();
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

    fn disable(&mut self) {
        self.clear_mac_rx_walker_enable();
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

/// Where the walker's NEXT points.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NextAt {
    /// NEXT zero.
    Zero,
    /// A descriptor of the ring.
    Ring(usize),
    /// Another address (R's among them).
    Elsewhere,
}

/// What [`RxRing::standing`] read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Standing {
    /// LAST's ring index, `None` outside the ring.
    pub last: Option<usize>,
    pub next: NextAt,
    /// The accepted tail's index and whether its descriptor is completed.
    pub tail: usize,
    pub tail_done: bool,
    /// The cursor read the same before and after the tail's word.
    pub unchanged: bool,
}

/// The doorbell's state: still pending, or accepted by the walker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Reload {
    Pending,
    Accepted,
}

/// What the walker did that it must not: the keeper raises the one
/// indication, the stop takes the ring down; the observed word goes to the
/// backend's diagnostic record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RingAnomaly {
    /// A completed descriptor's `LENGTH` the walker must not write: a
    /// single unit's outside `[FLOOR, SIZE]`, a chain's last one zero or
    /// past `SIZE`. The unit is neither lent nor copied; its control word.
    Length { word: WifiRxDmaDescriptorControl },
    /// The doorbell still pending past [`RELOAD_SETTLE`]: what was re-armed
    /// is linked, the walker did not take it.
    DoorbellPending,
}

/// Why a table was not placed: it has no DMA address in the space.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Unplaced;

/// A ring's descriptor table, placed once: the descriptors and the DMA
/// address of each, R's included. Every publication of the ring reuses it.
pub struct RxTable<const N: usize> {
    descriptors: &'static RxDescriptors<N>,
    /// Each ring descriptor's own address.
    links: [DmaRef<WifiRxDmaDescriptor>; N],
    /// The ring's first descriptor's address, R's link.
    head: DmaRef<WifiRxDmaDescriptor>,
    /// R's address, the walker's BASE.
    reserved: DmaRef<WifiRxDmaDescriptor>,
    /// The ring's first address, which the walker's cursor reads against.
    base: u32,
}

impl<const N: usize> RxTable<N> {
    /// Project the placed table in the space `S`, once.
    ///
    /// # Errors
    ///
    /// [`Unplaced`] when it has no DMA address there: the placement comes back
    /// whole, and nothing was written.
    pub fn place<S: DmaSpace>(
        table: Placed<RxDescriptors<N>>,
    ) -> Result<Self, (Unplaced, Placed<RxDescriptors<N>>)> {
        let projected = PlacementProjection::try_with::<S, _, _>(table, |table, projection| {
            let addresses = projection.table(
                table
                    .ring
                    .iter()
                    .chain(core::iter::once(&table.reserved))
                    .map(RxDescriptor::layout),
            )?;
            let mut links = [addresses.first(); N];
            for (link, descriptor) in links.iter_mut().zip(&table.ring) {
                *link = projection.link(descriptor.layout())?;
            }
            let reserved = projection.link(table.reserved.layout())?;
            Ok((links, addresses.first(), reserved, addresses.base()))
        });
        match projected {
            Ok((descriptors, (links, head, reserved, base))) => Ok(Self {
                descriptors,
                links,
                head,
                reserved,
                base,
            }),
            Err((_, table)) => Err((Unplaced, table)),
        }
    }
}

/// What a ring is made of while it is not published: its table, the
/// walker's registers, its arena and every buffer's handle. The port holds
/// it between a release and the next publication.
pub struct RxRingParts<B: Brand, const BYTES: usize, const N: usize, W> {
    pub table: RxTable<N>,
    pub walker: W,
    pub arena: Arena<B, BYTES>,
    pub ring: [Handle<B>; N],
    pub reserved: Handle<B>,
}

/// The one start a publication gives: [`RxRing::enable_walker`] takes it.
///
/// Only a publication makes one:
///
/// ```compile_fail,E0451
/// use oer_chips_esp32s31_hal::radio::ieee80211::dma::receive::WalkerStart;
/// let _made = WalkerStart { _private: () };
/// ```
///
/// and it starts the walker once:
///
/// ```compile_fail,E0382
/// use oer_chips_esp32s31_hal::layouts::DmaWindow;
/// use oer_chips_esp32s31_hal::radio::ieee80211::dma::receive::{RxRing, Walker, WalkerStart};
/// use oer_portable_memory::arena::Brand;
/// fn twice<B: Brand<Window = DmaWindow>, const BYTES: usize, const N: usize, W: Walker>(
///     ring: &mut RxRing<B, BYTES, N, W>,
///     start: WalkerStart,
/// ) {
///     let _first = ring.enable_walker(start);
///     let _second = ring.enable_walker(start);
/// }
/// ```
#[must_use = "the walker of a published ring starts with it"]
pub struct WalkerStart {
    _private: (),
}

/// The walker started on a published ring: what the MAC's receive release
/// borrows and the walker's edge takes.
///
/// ```compile_fail,E0451
/// use oer_chips_esp32s31_hal::radio::ieee80211::dma::receive::WalkerEnabled;
/// let _made = WalkerEnabled { _private: () };
/// ```
#[must_use = "the walker's edge takes it"]
pub struct WalkerEnabled {
    _private: (),
}

/// The walker's edge made: its enable cleared and ordered. The MAC's stop
/// request takes it.
///
/// ```compile_fail,E0451
/// use oer_chips_esp32s31_hal::radio::ieee80211::dma::receive::WalkerEdge;
/// let _made = WalkerEdge { _private: () };
/// ```
#[must_use = "the MAC's stop request takes it"]
pub struct WalkerEdge {
    pub(crate) _private: (),
}

/// The accepted tail: the descriptor whose link is zero.
struct Tail<const N: usize> {
    index: RingIndex<N>,
    descriptor: &'static RxDescriptor,
}

/// The receive ring: its descriptors and their addresses, the buffers lent
/// in them, the arena they come from and the walker's registers.
pub struct RxRing<B, const BYTES: usize, const N: usize, W>
where
    B: Brand<Window = DmaWindow>,
{
    table: RxTable<N>,
    arena: Arena<B, BYTES>,
    slots: [Lent<B>; N],
    reserved: Lent<B>,
    walker: W,
    /// The oldest descriptor not yet consumed, in the ring's order.
    start: RingIndex<N>,
    tail: Tail<N>,
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

    /// The cold publication, built in its place: the walker's buffer
    /// configuration; every descriptor armed with one of the parts' buffers
    /// and linked to the next, the last one's link zero; R armed with its
    /// buffer and linked to the head; the window selected and BASE = R. It
    /// takes the MAC core opened with its runtime stop held
    /// (`WifiMacHal::open_core`), before the one BASE. The walker is not
    /// started: the start it gives does ([`Self::enable_walker`]).
    pub fn publish_cold(
        place: &mut MaybeUninit<Self>,
        parts: RxRingParts<B, BYTES, N, W>,
        core: MacCoreOpen,
    ) -> (Occupied<'_, Self>, WalkerStart) {
        let () = Self::HEADER;
        let MacCoreOpen { .. } = core;
        let RxRingParts {
            table,
            mut walker,
            arena,
            ring,
            reserved,
        } = parts;
        let descriptors = table.descriptors;
        // The accepted tail is the ring's last descriptor (`N >= 2`).
        let last = descriptors
            .ring
            .iter()
            .fold(&descriptors.reserved, |_, descriptor| descriptor);
        walker.initialize_prefix();
        walker.configure_window();
        let mut descriptor = descriptors.ring.iter();
        let mut next = table.links.iter().skip(1);
        let slots = ring.map(|handle| {
            let (lent, buffer) = arena.lend(handle);
            if let Some(descriptor) = descriptor.next() {
                descriptor.publish_owned(Self::ARMED, buffer, next.next().copied());
            }
            lent
        });
        let (reserved, buffer) = arena.lend(reserved);
        descriptors
            .reserved
            .publish_owned(Self::ARMED, buffer, Some(table.head));
        walker.fence();
        walker.write_base(table.reserved);
        walker.fence();
        let ring = Occupied::fill(
            place,
            Self {
                table,
                arena,
                slots,
                reserved,
                walker,
                start: Self::HEAD,
                tail: Tail {
                    index: Self::LAST,
                    descriptor: last,
                },
            },
        );
        (ring, WalkerStart { _private: () })
    }

    /// Start the walker on the published ring, once a publication.
    pub fn enable_walker(&mut self, start: WalkerStart) -> WalkerEnabled {
        let WalkerStart { _private: () } = start;
        self.walker.fence();
        self.walker.enable();
        self.walker.fence();
        WalkerEnabled { _private: () }
    }

    /// The walker's edge: its enable cleared, then ordered
    /// (`HW_WIFIMAC_STOP_WALKER_FIRST_20261119`: the edge before the MAC's
    /// stop request). Nothing is read back.
    pub fn stop_walker(&mut self, enabled: WalkerEnabled) -> WalkerEdge {
        let WalkerEnabled { _private: () } = enabled;
        self.walker.disable();
        self.walker.fence();
        WalkerEdge { _private: () }
    }

    /// The ring's release after the MAC block's reset: every buffer's handle
    /// back with the table, the walker's registers and the arena, and the
    /// place, empty. The handles' spans are the whole buffer above the floor.
    pub fn release<'p>(
        ring: Occupied<'p, Self>,
        reset: MacStopReset,
    ) -> (RxRingParts<B, BYTES, N, W>, &'p mut MaybeUninit<Self>) {
        let MacStopReset { .. } = reset;
        let (ring, place) = ring.vacate();
        let RxRing {
            table,
            arena,
            slots,
            reserved,
            walker,
            ..
        } = ring;
        let release = |lent: Lent<B>| {
            #[allow(
                unsafe_code,
                reason = "the receive ring's release proof: the block's reset"
            )]
            // SAFETY: `reset` is made only by the radio domain's reset of the
            // MAC block after a stop (`reset_stopped_wifi_mac_block`), which
            // takes the MAC's stop request, which takes the walker's edge,
            // which takes the walker's one start of a publication: the reset
            // followed this ring's edge and stop request. The ring holds the
            // walker's registers, so no other ring was published meanwhile.
            // After such a reset no buffer or descriptor changed
            // (`HW_RST_WIFIMAC_AFTER_STOP_PORT_ORDER_20261119`,
            // `HW_RST_WIFIMAC_AFTER_STOP_REQUEST_20261119`,
            // `HW_RST_WIFIMAC_RUNNING_RX_20261119`): the walker writes none
            // of these buffers again.
            let released = unsafe { Released::assume(lent, Span::FULL) };
            arena.reclaim(released)
        };
        let parts = RxRingParts {
            table,
            walker,
            arena,
            ring: slots.map(release),
            reserved: release(reserved),
        };
        (parts, place)
    }

    /// Where the walker stands: its cursor, the accepted tail's word, then
    /// its cursor again; an observation for a probe's record of a stall,
    /// which the ring decides nothing by.
    pub fn standing(&mut self) -> Standing {
        let base = self.table.base;
        let (last, next) = self.walker.cursor();
        let tail = self.tail.descriptor.word0();
        let again = self.walker.cursor();
        Standing {
            last: cursor_ring_index(last, base, N),
            next: match next {
                0 => NextAt::Zero,
                next => match cursor_ring_index(next, base, N) {
                    Some(index) => NextAt::Ring(index),
                    None => NextAt::Elsewhere,
                },
            },
            tail: self.tail.index.get(),
            tail_done: tail.rx_done(),
            unchanged: again == (last, next),
        }
    }

    /// Serve the completed units: `f` gets the view, in the ring's order up
    /// to `LAST` as read at the start; after it, the doorbell is rung if
    /// anything was re-armed, and once more when the walker settled with
    /// NEXT zero and LAST short of the tail, each wait bounded by
    /// [`RELOAD_SETTLE`] on `counter`.
    ///
    /// # Errors
    ///
    /// A [`RingAnomaly`] with `f`'s result: the doorbell not accepted, or the
    /// length at which the view stopped. What was re-armed is linked and, but
    /// for the first anomaly, rung.
    pub fn serve<K: SettleCounter + ?Sized, R>(
        &mut self,
        counter: &K,
        f: impl FnOnce(&mut Completed<'_, B, BYTES, N, W>) -> R,
    ) -> Result<R, (RingAnomaly, R)> {
        let (last, next) = self.walker.cursor();
        let remaining = self.completed(cursor_ring_index(last, self.table.base, N), next == 0);
        let mut view = Completed {
            ring: self,
            remaining,
            yielded: 0,
            rearmed: 0,
            anomaly: None,
        };
        let result = f(&mut view);
        let (rearmed, anomaly) = (view.rearmed, view.anomaly);
        let rung = if rearmed == 0 {
            Ok(())
        } else {
            self.ring_doorbell(Settle::new(RELOAD_SETTLE, counter))
        };
        match (rung, anomaly) {
            (Err(Unsettled), _) => Err((RingAnomaly::DoorbellPending, result)),
            (Ok(()), Some(anomaly)) => Err((anomaly, result)),
            (Ok(()), None) => Ok(result),
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
    fn ring_doorbell<K: SettleCounter + ?Sized>(
        &mut self,
        within: Settle<'_, K>,
    ) -> Result<(), Unsettled> {
        let tail = self.tail.index.get();
        let base = self.table.base;
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
    /// The anomaly the view stopped at.
    anomaly: Option<RingAnomaly>,
}

/// One completed unit: one descriptor, or a chain of several, which is only
/// copied.
pub enum Unit<'v, B, const BYTES: usize, const N: usize, W>
where
    B: Brand<Window = DmaWindow>,
{
    Single(Single<'v, B, BYTES, N, W>),
    Chain(Chain<'v, B, BYTES, N, W>),
}

/// What re-arming links: the walker's fence, the tail, the ring's start and
/// the service's count.
struct Linker<'v, const N: usize, W> {
    walker: &'v mut W,
    start: &'v mut RingIndex<N>,
    tail: &'v mut Tail<N>,
    rearmed: &'v mut usize,
}

impl<const N: usize, W: Walker> Linker<'_, N, W> {
    /// Link the re-armed run of `count` from `first` to `last` behind the
    /// accepted tail: the old tail's link names `first`, and `last` is the
    /// tail.
    fn link(
        self,
        first: DmaRef<WifiRxDmaDescriptor>,
        last: RingIndex<N>,
        descriptor: &'static RxDescriptor,
        count: usize,
    ) {
        self.walker.fence();
        self.tail.descriptor.publish_next(Some(first));
        self.walker.fence();
        *self.tail = Tail {
            index: last,
            descriptor,
        };
        *self.start = last.advance(1);
        *self.rearmed += count;
    }
}

/// The bytes a chain's last buffer holds: below the floor only, or past it
/// too.
enum LastBytes<B: Brand> {
    /// `n <= FLOOR` bytes, all below the floor; the empty span above it.
    Floor(usize, Span<B>),
    /// The floor's bytes and the span above it.
    Beyond(Span<B>),
}

impl<B: Brand> Clone for LastBytes<B> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<B: Brand> Copy for LastBytes<B> {}

impl<'r, B, const BYTES: usize, const N: usize, W> Completed<'r, B, BYTES, N, W>
where
    B: Brand<Window = DmaWindow>,
    W: Walker,
{
    /// The next completed unit, or `None`: none is left up to `LAST`, the
    /// next one is not complete at `LAST`, a unit before it was dropped and
    /// stays in the ring (what follows it waits for it), or the view stopped
    /// at a [`RingAnomaly`].
    pub fn next_unit(&mut self) -> Option<Unit<'_, B, BYTES, N, W>> {
        let Completed {
            ring,
            remaining,
            yielded,
            rearmed,
            anomaly,
        } = self;
        if anomaly.is_some() || *yielded != *rearmed {
            return None;
        }
        let RxRing {
            table: RxTable {
                descriptors, links, ..
            },
            arena,
            slots,
            walker,
            start,
            tail,
            ..
        } = &mut **ring;
        let head = *start;
        let descriptors: &'static [RxDescriptor; N] = &descriptors.ring;
        // The run from `head`, wrapping once: `a` from `head` to the ring's
        // end, `b` from its start. A continuation's `LENGTH` is not read: its
        // bytes are `SIZE` (`HW_RX_CHAIN_LAPS_20261117`), and no record names
        // what the walker leaves in that field.
        // A `RingIndex<N>` is below `N`, so `split_at_checked` at it never
        // fails; the language cannot see that, and its `None` is not a case.
        let (descriptors_b, descriptors_a) = descriptors.split_at_checked(head.get())?;
        let count = descriptors_a
            .iter()
            .chain(descriptors_b)
            .take(*remaining)
            .position(|descriptor| descriptor.word0().rx_done())?
            + 1;
        let a = descriptors_a.len().min(count);
        let (run_a, _) = descriptors_a.split_at_checked(a)?;
        let (run_b, _) = descriptors_b.split_at_checked(count - a)?;
        let (_, links_a) = links.split_at_checked(head.get())?;
        let (slots_b, slots_a) = slots.split_at_mut_checked(head.get())?;
        let (slots_a, _) = slots_a.split_at_mut_checked(a)?;
        let (slots_b, _) = slots_b.split_at_mut_checked(count - a)?;
        let last_descriptor = run_b.last().or(run_a.last())?;
        let head_link = *links_a.first()?;
        let last = head.advance(count - 1);
        let mut stop = |word: WifiRxDmaDescriptorControl| {
            *anomaly = Some(RingAnomaly::Length { word });
            None
        };
        let word = last_descriptor.word0();
        let length = usize::from(word.length());
        let linker = Linker {
            walker,
            start,
            tail,
            rearmed,
        };
        let unit = if count == 1 {
            let [slot] = slots_a else {
                return None;
            };
            let Some(span) = length
                .checked_sub(B::FLOOR)
                .and_then(|bytes| Span::new(B::FLOOR, bytes))
            else {
                return stop(word);
            };
            Unit::Single(Single {
                slot,
                descriptor: last_descriptor,
                link: head_link,
                index: head,
                span,
                arena: *arena,
                linker,
            })
        } else {
            let last_bytes = match (length, Span::new(B::FLOOR, 0)) {
                (0, _) | (_, None) => return stop(word),
                (length, Some(empty)) if length <= B::FLOOR => LastBytes::Floor(length, empty),
                (length, Some(_)) => match Span::new(B::FLOOR, length - B::FLOOR) {
                    Some(span) => LastBytes::Beyond(span),
                    None => return stop(word),
                },
            };
            Unit::Chain(Chain {
                slots: (slots_a, slots_b),
                descriptors: (run_a, run_b),
                head_link,
                last,
                last_descriptor,
                count,
                last_bytes,
                arena: *arena,
                linker,
            })
        };
        *yielded += count;
        *remaining -= count;
        Some(unit)
    }
}

/// A completed unit of one descriptor.
pub struct Single<'v, B, const BYTES: usize, const N: usize, W>
where
    B: Brand<Window = DmaWindow>,
{
    slot: &'v mut Lent<B>,
    descriptor: &'static RxDescriptor,
    link: DmaRef<WifiRxDmaDescriptor>,
    index: RingIndex<N>,
    /// The frame: from the floor to the walker's `LENGTH`, checked.
    span: Span<B>,
    arena: Arena<B, BYTES>,
    linker: Linker<'v, N, W>,
}

/// A completed unit of `count` descriptors, in the ring's order.
pub struct Chain<'v, B, const BYTES: usize, const N: usize, W>
where
    B: Brand<Window = DmaWindow>,
{
    slots: (&'v mut [Lent<B>], &'v mut [Lent<B>]),
    descriptors: (&'static [RxDescriptor], &'static [RxDescriptor]),
    head_link: DmaRef<WifiRxDmaDescriptor>,
    last: RingIndex<N>,
    last_descriptor: &'static RxDescriptor,
    count: usize,
    last_bytes: LastBytes<B>,
    arena: Arena<B, BYTES>,
    linker: Linker<'v, N, W>,
}

impl<B, const BYTES: usize, const N: usize, W> Single<'_, B, BYTES, N, W>
where
    B: Brand<Window = DmaWindow>,
    W: Walker,
{
    /// The bytes the walker wrote in the unit's buffer, its header included:
    /// what the keeper decides by.
    pub fn written(&self) -> usize {
        B::FLOOR + self.span.len()
    }

    /// The unit's buffer out, `spare` armed in its descriptor and linked
    /// behind the tail: the exchange of the rebind records. The handle's span
    /// is the frame; its holder reads the header from the floor
    /// ([`Arena::floor`]).
    pub fn lend(self, spare: Handle<B>) -> Handle<B> {
        let Single {
            slot,
            descriptor,
            link,
            index,
            span,
            arena,
            linker,
        } = self;
        let (spare, buffer) = arena.lend(spare);
        let unit = replace(slot, spare);
        descriptor.publish_owned(RxRing::<B, BYTES, N, W>::ARMED, buffer, None);
        linker.link(link, index, descriptor, 1);
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
        arena.reclaim(released)
    }

    /// The unit's frame copied out by `f` through its reader; the same buffer
    /// re-armed and linked behind the tail after `f` returns.
    pub fn copy_in_place<R>(self, f: impl FnOnce(&Frame<'_, B, BYTES>) -> R) -> R {
        let Single {
            slot,
            descriptor,
            link,
            index,
            span,
            arena,
            linker,
        } = self;
        let frame = Frame {
            arena: &arena,
            lents: (core::slice::from_ref(&*slot), &[]),
            first: span,
            last: None,
            count: 1,
        };
        let result = f(&frame);
        descriptor.publish_next(None);
        descriptor.publish_word0(RxRing::<B, BYTES, N, W>::ARMED);
        linker.link(link, index, descriptor, 1);
        result
    }
}

impl<B, const BYTES: usize, const N: usize, W> Chain<'_, B, BYTES, N, W>
where
    B: Brand<Window = DmaWindow>,
    W: Walker,
{
    /// The chain's descriptors, its last one's included.
    pub fn count(&self) -> usize {
        self.count
    }

    /// The control word of each of the chain's descriptors but its last, in
    /// the chain's order, as the walker left it: an observation for the
    /// record of what a continuation's `LENGTH` holds
    /// (`HW_RX_CHAIN_CONTINUATION_WORD_20261120`); the view decides nothing
    /// by it.
    pub fn continuation_words(&self) -> impl Iterator<Item = WifiRxDmaDescriptorControl> + '_ {
        self.descriptors
            .0
            .iter()
            .chain(self.descriptors.1)
            .take(self.count.saturating_sub(1))
            .map(RxDescriptor::word0)
    }

    /// The chain's frame copied out by `f` through its reader; its buffers
    /// re-armed in place, in order, and linked behind the tail after `f`
    /// returns. Its descriptors' links to each other are the walker's own
    /// path and stay; the last one's becomes zero.
    pub fn copy_in_place<R>(self, f: impl FnOnce(&Frame<'_, B, BYTES>) -> R) -> R {
        let Chain {
            slots,
            descriptors,
            head_link,
            last,
            last_descriptor,
            count,
            last_bytes,
            arena,
            linker,
        } = self;
        let frame = Frame {
            arena: &arena,
            lents: (&*slots.0, &*slots.1),
            first: Span::FULL,
            last: Some(last_bytes),
            count,
        };
        let result = f(&frame);
        last_descriptor.publish_next(None);
        for descriptor in descriptors.0.iter().chain(descriptors.1) {
            descriptor.publish_word0(RxRing::<B, BYTES, N, W>::ARMED);
        }
        linker.link(head_link, last, last_descriptor, count);
        result
    }
}

/// The frame of a unit being copied: its reader is the only access to its
/// bytes, which it copies out in order. The first buffer's bytes begin at
/// the floor (below it lies the header); a continuation's begin at its
/// first byte (`HW_RX_CHAIN_LAPS_20261117`: `(n - 1) × SIZE` plus the last
/// descriptor's `LENGTH` are the unit's bytes).
pub struct Frame<'f, B, const BYTES: usize>
where
    B: Brand<Window = DmaWindow>,
{
    arena: &'f Arena<B, BYTES>,
    lents: (&'f [Lent<B>], &'f [Lent<B>]),
    /// The first buffer's span: the unit's frame for a single unit, the
    /// whole buffer above the floor for a chain.
    first: Span<B>,
    /// A chain's last buffer; `None` for a single unit.
    last: Option<LastBytes<B>>,
    count: usize,
}

impl<B, const BYTES: usize> Frame<'_, B, BYTES>
where
    B: Brand<Window = DmaWindow>,
{
    /// The frame's bytes: what the walker wrote, less the first buffer's
    /// header.
    pub fn len(&self) -> usize {
        let middle = self.count.saturating_sub(2) * B::SIZE;
        let last = match self.last {
            None => 0,
            Some(LastBytes::Floor(bytes, _)) => bytes,
            Some(LastBytes::Beyond(span)) => B::FLOOR + span.len(),
        };
        self.first.len() + middle + last
    }

    /// Whether the frame has no byte.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Copy the frame's first bytes into `out`, as many as fit: the count
    /// copied.
    pub fn copy_to(&self, out: &mut [u8]) -> usize {
        let mut copied = 0;
        for (offset, lent) in self.lents.0.iter().chain(self.lents.1).enumerate() {
            let last = offset + 1 == self.count;
            let (span, below) = match (offset, last, self.last) {
                (0, _, _) => (self.first, 0),
                (_, true, Some(LastBytes::Floor(bytes, empty))) => (empty, bytes),
                (_, true, Some(LastBytes::Beyond(span))) => (span, B::FLOOR),
                _ => (Span::FULL, B::FLOOR),
            };
            #[allow(
                unsafe_code,
                reason = "the receive ring's borrowed release: a completed unit"
            )]
            // SAFETY: the unit's descriptors lie at or before the `LAST` the
            // walker reported at this service's start, and none is re-armed
            // before the copy ends (`copy_in_place` re-arms after `f`
            // returns, and this reader lives only inside `f`): the walker
            // completed them and does not write their buffers meanwhile
            // (`HW_RX_CHAIN_LAPS_20261117`).
            let quiet = unsafe { Quiet::assume(lent, span) };
            copied = self.arena.inspect(&quiet, |handle| {
                let floor: [u8; 64] = self.arena.floor(handle).copy();
                let bytes = floor.iter().take(below).chain(self.arena.bytes(handle));
                let mut copied = copied;
                for (to, from) in out.iter_mut().skip(copied).zip(bytes) {
                    *to = *from;
                    copied += 1;
                }
                copied
            });
        }
        copied
    }
}
