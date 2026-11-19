//! The custody of buffers the hardware reads or writes (`CLAUDE.md`, "DMA
//! memory").
//!
//! An arena is one brand's one place: `COUNT` buffers of `SIZE` bytes in the
//! DMA window, declared once by [`arena!`](crate::arena!) and placed by the
//! HAL's placement. Custody is a value:
//!
//! - a [`Handle`] is the CPU's: it gives the buffer's CPU bytes and nothing
//!   else;
//! - [`Arena::lend`] takes the handle and gives a [`Lent`], which gives no
//!   bytes, and the buffer's address for the hardware;
//! - only a [`Released`], the claim that the hardware no longer reads or
//!   writes the buffer, gives a handle back ([`Arena::reclaim`]). Its one
//!   constructor, [`Released::assume`], is `unsafe`: each kind of release is
//!   one HAL atom that observed it and cites its stand record.
//!
//! The CPU's bytes of a buffer are the [`Span`] its handle holds: the
//! release's (where the hardware's unit begins and how long it is) or, for a
//! fresh or returned handle, the whole buffer from the brand's floor. No span
//! begins below the floor (`FLOOR`, 64 for receive: the metadata's fixed
//! prefix and tail, which the hardware shares, never lie under a slice). What
//! lies below the floor of a held buffer is copied out by volatile reads
//! ([`Arena::floor`]), and the HAL reads its fields from the copy through its
//! generated layouts.
//!
//! A held buffer lent on to another task returns through a [`Returning`]:
//! its destructor sets the buffer's bit in the brand's [`ReturnedSet`] with
//! one atomic OR and calls the placement's [`ReturnNotify`] statically; the
//! arena's owner takes the set with [`Arena::returned`]. This crate names no
//! executor and keeps no waker.
//!
//! A dropped [`Handle`] or [`Lent`], or a forgotten [`Returning`], leaves its
//! buffer out of the arena for good: the owners keep theirs. Nothing here is
//! a pool: which buffer goes where, spares and pressure are the owners'.

use core::{
    cell::UnsafeCell,
    marker::PhantomData,
    sync::atomic::{AtomicU32, Ordering},
};

use crate::dma_address::{DmaLayout, DmaRef, DmaWindow, DmaWindowAddress};

/// The most buffers an arena has: the bits of its [`ReturnedSet`].
pub const MAX_BUFFERS: usize = 32 * RETURNED_WORDS;

const RETURNED_WORDS: usize = 4;

/// The brand of one arena: a type with one place, declared with it by
/// [`arena!`](crate::arena!). A handle of one brand fits no other arena.
///
/// # Safety
///
/// The constants are the geometry of the place [`Brand::place`] names,
/// that place is a static no other brand names, and [`Brand::returned`] is
/// a set no other brand names. Only [`arena!`](crate::arena!) implements
/// this trait.
#[allow(unsafe_code, reason = "a brand names its one place and its one set")]
pub unsafe trait Brand: Sized + 'static {
    /// The window the place lies in.
    type Window: DmaWindow;
    /// The buffers.
    const COUNT: usize;
    /// Each buffer's bytes.
    const SIZE: usize;
    /// The offset below which no span of a buffer begins.
    const FLOOR: usize;
    /// The placed static that holds the brand's place.
    type Static: 'static;
    /// The brand's returned set, in a static of its own.
    fn returned() -> &'static ReturnedSet;
    /// The brand's one place, for its arena to claim.
    fn place() -> &'static Self::Static;
}

/// A placed static that hands out the value it holds once, with the proof
/// of its address in the DMA window: the HAL's placement implements it for
/// its placed statics.
///
/// # Safety
///
/// [`PlacedStatic::claim`] gives the value of this static and no other, at
/// most once, and the `DmaRef` it gives with it is that value's address in
/// its window, for as long as the value lives.
#[allow(unsafe_code, reason = "the placement proves the address of its static")]
pub unsafe trait PlacedStatic<T: DmaLayout> {
    /// The value and its address, the first time; `None` once claimed.
    #[allow(
        clippy::mut_from_ref,
        reason = "a claim-once static: the claim makes this the only reference"
    )]
    fn claim(&'static self) -> Option<(&'static mut T, DmaRef<T>)>;
}

/// What is wrong with `B`'s geometry and its place of `BYTES` bytes, if
/// anything: checked when [`arena!`](crate::arena!) declares the brand and
/// again where an arena is built. At most `MAX_BUFFERS` buffers of at most
/// 65 535 bytes make a place below 2^23 bytes, which fits the DMA address
/// width.
#[doc(hidden)]
pub const fn geometry_refusal<B: Brand, const BYTES: usize>() -> Option<&'static str> {
    let Some(place) = B::COUNT.checked_mul(B::SIZE) else {
        return Some("an arena's place is COUNT buffers of SIZE bytes");
    };
    if B::COUNT == 0 || B::COUNT > MAX_BUFFERS {
        Some("an arena has 1 to MAX_BUFFERS buffers")
    } else if place != BYTES {
        Some("an arena's place is COUNT buffers of SIZE bytes")
    } else if B::SIZE > u16::MAX as usize || B::SIZE % 4 != 0 {
        Some("a buffer is at most 65 535 bytes, in words")
    } else if B::FLOOR >= B::SIZE {
        Some("a buffer has bytes above its floor")
    } else {
        None
    }
}

/// The place of a brand's buffers: `BYTES = COUNT × SIZE` bytes, all zero at
/// the start, which never move. Its address is the HAL placement's fact.
#[derive(bytemuck::Zeroable)]
#[zeroable(bound = "")]
#[repr(C, align(4))]
pub struct ArenaPlace<B: Brand, const BYTES: usize> {
    bytes: UnsafeCell<[u8; BYTES]>,
    brand: PhantomData<fn() -> B>,
}

#[allow(unsafe_code, reason = "the place's bytes are reached only by custody")]
// SAFETY: the place gives no access of its own. Its bytes are reached only
// through an `Arena` built once from the place's one `&'static mut`, and a
// buffer's bytes only through the one `Handle` of its index (neither `Copy`
// nor `Clone`, made only by this module), shared for a `&Handle` and unique
// for a `&mut Handle`; a lent buffer has no handle.
unsafe impl<B: Brand, const BYTES: usize> Sync for ArenaPlace<B, BYTES> {}

impl<B: Brand, const BYTES: usize> DmaLayout for ArenaPlace<B, BYTES> {
    type Window = B::Window;
}

/// One brand's arena: its place and the place's address, two words.
pub struct Arena<B: Brand, const BYTES: usize> {
    place: &'static ArenaPlace<B, BYTES>,
    at: DmaRef<ArenaPlace<B, BYTES>>,
}

impl<B: Brand, const BYTES: usize> Clone for Arena<B, BYTES> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<B: Brand, const BYTES: usize> Copy for Arena<B, BYTES> {}

impl<B: Brand, const BYTES: usize> Arena<B, BYTES>
where
    B::Static: PlacedStatic<ArenaPlace<B, BYTES>>,
{
    /// The arena of the brand's own place and its fresh handles, the first
    /// time; `None` once the place is claimed. The arena is made from no
    /// other place.
    pub fn claim() -> Option<(Self, Fresh<B>)> {
        const {
            if let Some(refusal) = geometry_refusal::<B, BYTES>() {
                panic!("{}", refusal);
            }
        }
        let (place, at) = B::place().claim()?;
        Some((Self { place, at }, Fresh::new()))
    }
}

impl<B: Brand, const BYTES: usize> Arena<B, BYTES> {
    /// The CPU's bytes of a held buffer: its handle's span.
    pub fn bytes<'h>(&self, handle: &'h Handle<B>) -> &'h [u8] {
        let start = self.cpu_start(handle.index, handle.span);
        #[allow(unsafe_code, reason = "a held buffer's CPU bytes")]
        // SAFETY: `handle` is the one handle of its buffer (`Handle`), which
        // the hardware does not touch while a handle exists; the span lies in
        // buffer `index < COUNT` (`Span`: `FLOOR <= start <= end <= SIZE`),
        // inside the place (`claim`'s geometry), which is `'static`. The
        // slice lives while `handle` is borrowed shared, so no `&mut` of the
        // same bytes exists meanwhile.
        unsafe {
            core::slice::from_raw_parts(start, handle.span.len())
        }
    }

    /// The CPU's bytes of a held buffer, to write: its handle's span.
    pub fn bytes_mut<'h>(&self, handle: &'h mut Handle<B>) -> &'h mut [u8] {
        let start = self.cpu_start(handle.index, handle.span);
        #[allow(unsafe_code, reason = "a held buffer's CPU bytes")]
        // SAFETY: as `bytes`; the slice lives while `handle` is borrowed
        // unique, so no other slice of the same bytes exists meanwhile.
        unsafe {
            core::slice::from_raw_parts_mut(start, handle.span.len())
        }
    }

    /// Lend a held buffer to the hardware: the handle goes, and the buffer's
    /// address (its first byte, below the floor included) is the HAL's to
    /// publish.
    pub fn lend(&self, handle: Handle<B>) -> (Lent<B>, DmaWindowAddress<B::Window>) {
        let offset = buffer_offset::<B>(handle.index);
        (
            Lent {
                index: handle.index,
                brand: PhantomData,
            },
            self.at.byte_within(offset),
        )
    }

    /// The handle of a released buffer, with the release's span.
    pub fn reclaim(&self, released: Released<B>) -> Handle<B> {
        Handle {
            index: released.index,
            span: released.span,
        }
    }

    /// The buffers returned since the last take, as handles of their whole
    /// buffer from the floor.
    pub fn returned(&self) -> Returned<B> {
        Returned {
            word: 0,
            bits: 0,
            brand: PhantomData,
        }
    }

    /// The bytes below the floor of a held buffer, which the hardware shares
    /// (a receive unit's header, or a chain continuation's first bytes): a
    /// view whose one operation copies them out. A lent buffer has no
    /// handle, so the floor of memory the hardware owns cannot be asked for.
    pub fn floor<'h>(&self, handle: &'h Handle<B>) -> Floor<'h, B, BYTES> {
        Floor {
            place: self.place,
            index: handle.index,
            handle: PhantomData,
        }
    }

    fn cpu_start(&self, index: u16, span: Span<B>) -> *mut u8 {
        let offset = buffer_offset::<B>(index) as usize + usize::from(span.start);
        self.place.bytes.get().cast::<u8>().wrapping_add(offset)
    }
}

/// The floor of one held buffer ([`Arena::floor`]), for as long as its
/// handle is borrowed.
pub struct Floor<'h, B: Brand, const BYTES: usize> {
    place: &'static ArenaPlace<B, BYTES>,
    index: u16,
    handle: PhantomData<&'h Handle<B>>,
}

impl<B: Brand, const BYTES: usize> Floor<'_, B, BYTES> {
    /// The `N = FLOOR` bytes below the floor, copied out by volatile reads:
    /// no reference to them is made, so a write of the hardware in progress
    /// (the receive header's late bytes) is no data race of a slice.
    pub fn copy<const N: usize>(&self) -> [u8; N] {
        const {
            if N != B::FLOOR {
                panic!("a floor's copy is the brand's floor")
            }
        };
        let first = self
            .place
            .bytes
            .get()
            .cast::<u8>()
            .wrapping_add(buffer_offset::<B>(self.index) as usize);
        let mut copy = [0_u8; N];
        for (offset, byte) in copy.iter_mut().enumerate() {
            #[allow(unsafe_code, reason = "a held buffer's floor, copied out")]
            // SAFETY: the buffer `index < COUNT` is held by the handle this
            // view borrows (`Arena::floor`); a lent buffer has no handle. Its
            // first `FLOOR < SIZE` bytes lie inside it, inside the place
            // (`claim`'s geometry), which is `'static`. A volatile read makes
            // no reference to the byte, which the hardware may still write.
            let value = unsafe { core::ptr::read_volatile(first.wrapping_add(offset)) };
            *byte = value;
        }
        copy
    }
}

/// Buffer `index`'s first byte in the place: `index < COUNT`, so below
/// `COUNT × SIZE`, which fits `u32` (`Arena::new`).
const fn buffer_offset<B: Brand>(index: u16) -> u32 {
    (index as u32).wrapping_mul(B::SIZE as u32)
}

/// The CPU's custody of one buffer: its index and the span of its CPU bytes,
/// two words. Neither `Copy` nor `Clone`; made only by this module.
pub struct Handle<B: Brand> {
    index: u16,
    span: Span<B>,
}

impl<B: Brand> Handle<B> {
    /// The span of the buffer's CPU bytes.
    pub const fn span(&self) -> Span<B> {
        self.span
    }

    /// The handle narrowed to `span`, which lies inside its present span: a
    /// writer narrows a whole buffer to what it lends, a reader a unit to
    /// what it reads. A span reaching outside the present one gives the
    /// handle back unchanged: in a holder's hands a span never grows (it
    /// grows only by a fresh or a returned handle).
    pub fn narrow(self, span: Span<B>) -> Result<Self, NotInside<B>> {
        if span.start >= self.span.start && span.end <= self.span.end {
            Ok(Self { span, ..self })
        } else {
            Err(NotInside { handle: self })
        }
    }

    fn full(index: u16) -> Self {
        Self {
            index,
            span: Span::FULL,
        }
    }
}

/// The refusal of [`Handle::narrow`]: the span reached outside the handle's
/// own; the handle, unchanged.
pub struct NotInside<B: Brand> {
    handle: Handle<B>,
}

impl<B: Brand> NotInside<B> {
    /// The handle, as it was.
    pub fn into_handle(self) -> Handle<B> {
        self.handle
    }
}

/// The hardware's custody of one buffer: no bytes, neither `Copy` nor
/// `Clone`; what the HAL keeps while the hardware may read or write it.
pub struct Lent<B: Brand> {
    index: u16,
    brand: PhantomData<fn() -> B>,
}

/// The bytes `[start, end)` of a buffer of `B`, with `FLOOR <= start <= end
/// <= SIZE`.
pub struct Span<B: Brand> {
    start: u16,
    end: u16,
    brand: PhantomData<fn() -> B>,
}

impl<B: Brand> Clone for Span<B> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<B: Brand> Copy for Span<B> {}

impl<B: Brand> PartialEq for Span<B> {
    fn eq(&self, other: &Self) -> bool {
        (self.start, self.end) == (other.start, other.end)
    }
}

impl<B: Brand> Eq for Span<B> {}

impl<B: Brand> core::fmt::Debug for Span<B> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_tuple("Span")
            .field(&self.start)
            .field(&self.end)
            .finish()
    }
}

impl<B: Brand> Span<B> {
    /// The whole buffer from the floor.
    pub const FULL: Self = Self {
        start: B::FLOOR as u16,
        end: B::SIZE as u16,
        brand: PhantomData,
    };

    /// The `len` bytes from `start`, if they lie between the floor and the
    /// buffer's end: where the hardware reported its unit is checked here.
    pub const fn new(start: usize, len: usize) -> Option<Self> {
        let Some(end) = start.checked_add(len) else {
            return None;
        };
        if start >= B::FLOOR && end <= B::SIZE {
            Some(Self {
                start: start as u16,
                end: end as u16,
                brand: PhantomData,
            })
        } else {
            None
        }
    }

    /// The `LEN` bytes from `START`, a span of constants: refused at build
    /// time where [`Span::new`] would refuse it at run time.
    pub const fn fixed<const START: usize, const LEN: usize>() -> Self {
        const {
            match Self::new(START, LEN) {
                Some(span) => span,
                None => panic!("a span lies between the brand's floor and its buffer's end"),
            }
        }
    }

    /// The offset of the span's first byte in its buffer.
    pub const fn start(self) -> usize {
        self.start as usize
    }

    /// The span's bytes.
    pub const fn len(self) -> usize {
        self.end.wrapping_sub(self.start) as usize
    }

    /// Whether the span has no bytes.
    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }
}

/// The claim that the hardware no longer reads or writes a lent buffer, with
/// the span of the unit it wrote there.
pub struct Released<B: Brand> {
    index: u16,
    span: Span<B>,
}

impl<B: Brand> Released<B> {
    /// The release of `lent`, its CPU bytes `span`.
    ///
    /// # Safety
    ///
    /// The hardware no longer reads or writes the buffer `lent` names, and
    /// will not until it is lent again: an observation of the one HAL atom
    /// that made it, citing its stand record. The bytes the hardware may
    /// still write after that observation lie outside `span`.
    #[allow(unsafe_code, reason = "a release proof is a claim about the hardware")]
    pub unsafe fn assume(lent: Lent<B>, span: Span<B>) -> Self {
        Self {
            index: lent.index,
            span,
        }
    }
}

/// The fresh handles of a new arena, each once, in index order.
pub struct Fresh<B: Brand> {
    next: u16,
    brand: PhantomData<fn() -> B>,
}

impl<B: Brand> Fresh<B> {
    const fn new() -> Self {
        Self {
            next: 0,
            brand: PhantomData,
        }
    }
}

impl<B: Brand> Iterator for Fresh<B> {
    type Item = Handle<B>;

    fn next(&mut self) -> Option<Handle<B>> {
        if usize::from(self.next) >= B::COUNT {
            return None;
        }
        let handle = Handle::full(self.next);
        self.next = self.next.wrapping_add(1);
        Some(handle)
    }
}

/// The bits of the buffers returned through [`Returning`], one a buffer.
pub struct ReturnedSet {
    words: [AtomicU32; RETURNED_WORDS],
}

impl ReturnedSet {
    /// The empty set, for the brand's static.
    #[doc(hidden)]
    pub const fn new() -> Self {
        Self {
            words: [const { AtomicU32::new(0) }; RETURNED_WORDS],
        }
    }

    fn word(&self, word: usize) -> &AtomicU32 {
        let [first, second, third, fourth] = &self.words;
        match word {
            0 => first,
            1 => second,
            2 => third,
            _ => fourth,
        }
    }
}

impl Default for ReturnedSet {
    fn default() -> Self {
        Self::new()
    }
}

/// The handles returned, taken word by word.
pub struct Returned<B: Brand> {
    word: u8,
    bits: u32,
    brand: PhantomData<fn() -> B>,
}

impl<B: Brand> Iterator for Returned<B> {
    type Item = Handle<B>;

    fn next(&mut self) -> Option<Handle<B>> {
        while self.bits == 0 {
            if usize::from(self.word) * 32 >= B::COUNT {
                return None;
            }
            self.bits = B::returned()
                .word(usize::from(self.word))
                .swap(0, Ordering::Acquire);
            self.word = self.word.wrapping_add(1);
        }
        let bit = self.bits.trailing_zeros();
        self.bits &= self.bits.wrapping_sub(1);
        let index = (u32::from(self.word.wrapping_sub(1)) * 32).wrapping_add(bit);
        Some(Handle::full(index as u16))
    }
}

/// How the arena's owner learns of a return: a static call the placement
/// gives (raising its executor's signal), never a waker of another hart.
pub trait ReturnNotify {
    /// A buffer was returned.
    fn notify();
}

/// A held buffer lent to another task, which returns to its brand's
/// [`ReturnedSet`] when dropped. Holds only the handle.
pub struct Returning<B: Brand, N: ReturnNotify> {
    handle: Handle<B>,
    notify: PhantomData<fn() -> N>,
}

impl<B: Brand, N: ReturnNotify> Returning<B, N> {
    /// The held `handle`, returning when dropped.
    pub const fn new(handle: Handle<B>) -> Self {
        Self {
            handle,
            notify: PhantomData,
        }
    }

    /// The handle, for its bytes.
    pub const fn handle(&self) -> &Handle<B> {
        &self.handle
    }

    /// The handle, for its bytes to write.
    pub fn handle_mut(&mut self) -> &mut Handle<B> {
        &mut self.handle
    }
}

impl<B: Brand, N: ReturnNotify> Drop for Returning<B, N> {
    fn drop(&mut self) {
        let index = usize::from(self.handle.index);
        B::returned()
            .word(index / 32)
            .fetch_or(1 << (index % 32), Ordering::Release);
        N::notify();
    }
}

/// Declares an arena's brand together with its one place, placed by the
/// HAL's placement macro, and the one way to take that place.
///
/// ```ignore
/// oer_portable_memory::arena! {
///     /// The receive buffers.
///     pub RxBuffers: window Esp32s31DmaWindow, count 48, size 1664,
///         floor 64, placed by oer_chips_esp32s31_hal::dma_static
///         as oer_chips_esp32s31_hal::system::memory::DmaStatic
///         in ".dma.bss.wifi_rx";
/// }
/// ```
///
/// The place's static lies in an anonymous constant: nothing names it but
/// `Brand::place`, and an arena is made only by [`Arena::claim`] from it,
/// through the wrapper's [`PlacedStatic`]. A second static of the place's
/// type is inert.
#[macro_export]
macro_rules! arena {
    (
        $(#[$meta:meta])*
        $vis:vis $name:ident: window $window:ty, count $count:literal, size $size:literal,
            floor $floor:literal,
            placed by $($place:ident)::+ as $($wrap:ident)::+ in $section:literal;
    ) => {
        $(#[$meta])*
        $vis enum $name {}

        const _: () = {
            static RETURNED: $crate::arena::ReturnedSet = $crate::arena::ReturnedSet::new();

            $($place)::+! {
                static PLACE: $crate::arena::ArenaPlace<$name, { $count * $size }> =
                    zeroed in $section;
            }

            // SAFETY: the constants are those of `PLACE`'s type, and `PLACE`
            // and `RETURNED` are statics of this anonymous constant, which
            // nothing outside it names. The impl is this macro's, audited
            // here: it writes no `allow`, so it builds in a crate that
            // forbids `unsafe` (`tests/ui/arena_under_forbid.rs`).
            unsafe impl $crate::arena::Brand for $name {
                type Window = $window;
                const COUNT: usize = $count;
                const SIZE: usize = $size;
                const FLOOR: usize = $floor;

                type Static =
                    $($wrap)::+<$crate::arena::ArenaPlace<$name, { $count * $size }>>;

                fn returned() -> &'static $crate::arena::ReturnedSet {
                    &RETURNED
                }

                fn place() -> &'static Self::Static {
                    &PLACE
                }
            }

            const _: () = if let Some(refusal) =
                $crate::arena::geometry_refusal::<$name, { $count * $size }>()
            {
                panic!("{}", refusal)
            };
        };
    };
}

#[cfg(test)]
mod tests;
