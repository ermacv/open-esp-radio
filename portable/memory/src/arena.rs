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
//! The CPU's bytes of a buffer are `[FRONT, FRONT + extent)`: the extent is
//! the release's (what the hardware wrote) or, for a fresh or returned
//! handle, the whole CPU region `SIZE - FRONT - BACK`. The `FRONT` bytes in
//! front of it and the `BACK` bytes behind the region are shared with the
//! hardware for good (a receive header, its tail) and never lie under a
//! slice; the HAL reads them through its layouts' volatile accessors.
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

#[doc(hidden)]
pub mod sealed {
    /// Only [`arena!`](crate::arena!) implements a brand.
    pub trait Sealed {}
}

/// The brand of one arena: a type with one place, declared with it by
/// [`arena!`](crate::arena!). A handle of one brand fits no other arena.
pub trait Brand: sealed::Sealed + Sized + 'static {
    /// The window the place lies in.
    type Window: DmaWindow;
    /// The buffers.
    const COUNT: usize;
    /// Each buffer's bytes.
    const SIZE: usize;
    /// The bytes in front of the CPU's region that the hardware shares.
    const FRONT: usize;
    /// The bytes behind the CPU's region that the hardware shares.
    const BACK: usize;
    /// The brand's returned set, in a static of its own.
    fn returned() -> &'static ReturnedSet;
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
    let Some(shared) = B::FRONT.checked_add(B::BACK) else {
        return Some("a buffer's CPU region is not empty");
    };
    if B::COUNT == 0 || B::COUNT > MAX_BUFFERS {
        Some("an arena has 1 to MAX_BUFFERS buffers")
    } else if place != BYTES {
        Some("an arena's place is COUNT buffers of SIZE bytes")
    } else if B::SIZE > u16::MAX as usize || B::SIZE % 4 != 0 || B::FRONT % 4 != 0 {
        Some("a buffer is at most 65 535 bytes, in words, its front in words")
    } else if shared >= B::SIZE {
        Some("a buffer's CPU region is not empty")
    } else {
        None
    }
}

/// The CPU region's bytes of a buffer of `B`.
const fn region<B: Brand>() -> usize {
    B::SIZE - B::FRONT - B::BACK
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

impl<B: Brand, const BYTES: usize> Arena<B, BYTES> {
    /// The arena of the brand's place, which the HAL's placement claimed
    /// once and whose address `at` it proved, and its fresh handles.
    pub fn new(
        place: &'static mut ArenaPlace<B, BYTES>,
        at: DmaRef<ArenaPlace<B, BYTES>>,
    ) -> (Self, Fresh<B>) {
        const {
            if let Some(refusal) = geometry_refusal::<B, BYTES>() {
                panic!("{}", refusal);
            }
        }
        let arena = Self { place, at };
        (arena, Fresh::new())
    }

    /// The CPU's bytes of a held buffer: `[FRONT, FRONT + extent)`.
    pub fn bytes<'h>(&self, handle: &'h Handle<B>) -> &'h [u8] {
        let start = self.cpu_start(handle.index);
        #[allow(unsafe_code, reason = "a held buffer's CPU bytes")]
        // SAFETY: `handle` is the one handle of its buffer (`Handle`), which
        // the hardware does not touch while a handle exists; the bytes
        // `[FRONT, FRONT + extent)` of buffer `index < COUNT` lie in the
        // place (`new`'s geometry, `extent <= SIZE - FRONT - BACK`), which
        // is `'static`. The slice lives while `handle` is borrowed shared,
        // so no `&mut` of the same bytes exists meanwhile.
        unsafe {
            core::slice::from_raw_parts(start, usize::from(handle.extent))
        }
    }

    /// The CPU's bytes of a held buffer, to write: `[FRONT, FRONT + extent)`.
    pub fn bytes_mut<'h>(&self, handle: &'h mut Handle<B>) -> &'h mut [u8] {
        let start = self.cpu_start(handle.index);
        #[allow(unsafe_code, reason = "a held buffer's CPU bytes")]
        // SAFETY: as `bytes`; the slice lives while `handle` is borrowed
        // unique, so no other slice of the same bytes exists meanwhile.
        unsafe {
            core::slice::from_raw_parts_mut(start, usize::from(handle.extent))
        }
    }

    /// Lend a held buffer to the hardware: the handle goes, and the buffer's
    /// address (its first byte, the front included) is the HAL's to publish.
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

    /// The handle of a released buffer, with the release's extent.
    pub fn reclaim(&self, released: Released<B>) -> Handle<B> {
        Handle {
            index: released.index,
            extent: released.extent.0,
            brand: PhantomData,
        }
    }

    /// The buffers returned since the last take, as handles of their whole
    /// CPU region.
    pub fn returned(&self) -> Returned<B> {
        Returned {
            word: 0,
            bits: 0,
            brand: PhantomData,
        }
    }

    fn cpu_start(&self, index: u16) -> *mut u8 {
        let offset = buffer_offset::<B>(index) as usize + B::FRONT;
        self.place.bytes.get().cast::<u8>().wrapping_add(offset)
    }
}

/// Buffer `index`'s first byte in the place: `index < COUNT`, so below
/// `COUNT × SIZE`, which fits `u32` (`Arena::new`).
const fn buffer_offset<B: Brand>(index: u16) -> u32 {
    (index as u32).wrapping_mul(B::SIZE as u32)
}

/// The CPU's custody of one buffer: its index and the extent of its CPU
/// bytes, one word. Neither `Copy` nor `Clone`; made only by this module.
pub struct Handle<B: Brand> {
    index: u16,
    extent: u16,
    brand: PhantomData<fn() -> B>,
}

impl<B: Brand> Handle<B> {
    /// The extent of the buffer's CPU bytes.
    pub const fn extent(&self) -> usize {
        self.extent as usize
    }

    /// The handle with its extent set to `extent`: the bytes a writer means
    /// to lend, within the CPU region.
    pub fn with_extent(self, extent: Extent<B>) -> Self {
        Self {
            extent: extent.0,
            ..self
        }
    }

    fn full(index: u16) -> Self {
        Self {
            index,
            extent: region::<B>() as u16,
            brand: PhantomData,
        }
    }
}

/// The hardware's custody of one buffer: no bytes, neither `Copy` nor
/// `Clone`; what the HAL keeps while the hardware may read or write it.
pub struct Lent<B: Brand> {
    index: u16,
    brand: PhantomData<fn() -> B>,
}

/// A count of a buffer's CPU bytes, at most its CPU region.
pub struct Extent<B: Brand>(u16, PhantomData<fn() -> B>);

impl<B: Brand> Clone for Extent<B> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<B: Brand> Copy for Extent<B> {}

impl<B: Brand> Extent<B> {
    /// The whole CPU region.
    pub const FULL: Self = Self(region::<B>() as u16, PhantomData);

    /// `bytes`, if it fits the CPU region: a length the hardware reported is
    /// checked here.
    pub const fn new(bytes: usize) -> Option<Self> {
        if bytes <= region::<B>() {
            Some(Self(bytes as u16, PhantomData))
        } else {
            None
        }
    }

    /// The count.
    pub const fn get(self) -> usize {
        self.0 as usize
    }
}

/// The claim that the hardware no longer reads or writes a lent buffer, with
/// the extent of the bytes it wrote there.
pub struct Released<B: Brand> {
    index: u16,
    extent: Extent<B>,
}

impl<B: Brand> Released<B> {
    /// The release of `lent`, its CPU bytes the first `extent`.
    ///
    /// # Safety
    ///
    /// The hardware no longer reads or writes the buffer `lent` names, and
    /// will not until it is lent again: an observation of the one HAL atom
    /// that made it, citing its stand record. The bytes the hardware may
    /// still write after that observation lie outside the CPU region.
    #[allow(unsafe_code, reason = "a release proof is a claim about the hardware")]
    pub unsafe fn assume(lent: Lent<B>, extent: Extent<B>) -> Self {
        Self {
            index: lent.index,
            extent,
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
///         front 56, back 8, placed by oer_chips_esp32s31_hal::dma_static
///         as oer_chips_esp32s31_hal::system::memory::DmaStatic
///         in ".dma.bss.wifi_rx";
/// }
/// ```
///
/// The place's static lies in an anonymous constant: nothing names it but
/// `RxBuffers::place()`, which the placement claims once.
#[macro_export]
macro_rules! arena {
    (
        $(#[$meta:meta])*
        $vis:vis $name:ident: window $window:ty, count $count:literal, size $size:literal,
            front $front:literal, back $back:literal,
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

            impl $crate::arena::sealed::Sealed for $name {}

            impl $crate::arena::Brand for $name {
                type Window = $window;
                const COUNT: usize = $count;
                const SIZE: usize = $size;
                const FRONT: usize = $front;
                const BACK: usize = $back;

                fn returned() -> &'static $crate::arena::ReturnedSet {
                    &RETURNED
                }
            }

            const _: () = if let Some(refusal) =
                $crate::arena::geometry_refusal::<$name, { $count * $size }>()
            {
                panic!("{}", refusal)
            };

            impl $name {
                /// The brand's one place, for its placement to claim.
                $vis fn place()
                    -> &'static $($wrap)::+<$crate::arena::ArenaPlace<$name, { $count * $size }>>
                {
                    &PLACE
                }
            }
        };
    };
}

#[cfg(test)]
mod tests;
