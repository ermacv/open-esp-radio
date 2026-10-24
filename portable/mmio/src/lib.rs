//! The one volatile register-access core under the generated register code.
//!
//! The register generator renders typed register blocks that hold a
//! [`Block`] (or the [`Part`]s of one) and call only its offset-checked
//! reads and writes; the generated code contains no `unsafe`. Everything a
//! volatile access needs to be sound is decided here:
//!
//! - **Where a block lives.** A [`Span`] is the exclusive authority over one
//!   range of device registers. Its constructor is the one `unsafe` promise a
//!   chip's PAC root makes; splitting it and taking a [`Block`] from it are
//!   checked at compile time, so two blocks never cover the same address.
//!   [`Block::host`] places a block in host memory instead, for tests of the
//!   rendered accessors.
//! - **Which words an access touches.** Every offset, stride and count is a
//!   const generic, checked against the block's size when the access is
//!   monomorphized. A runtime array index is an [`Index`], made only by a
//!   checked constructor.
//! - **Who may write a word.** [`Block::split`] divides a block into two
//!   [`Part`]s whose owned word lists ([`Words`]) are disjoint, again at
//!   compile time; a part writes only its owned words and also reads the
//!   words it observes.
//!
//! Memory the hardware reads by DMA is typed here too: [`DmaWindowAddress`]
//! and [`DmaRef`] are made only by their `unsafe` constructors (a chip HAL's
//! `dma_address` module), and [`VolatileCell`] is the volatile word the
//! generated memory layouts are built from.

#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

#[cfg(test)]
extern crate std;

use core::{
    cell::{Cell, UnsafeCell},
    marker::PhantomData,
    ptr::NonNull,
};

mod sealed {
    /// The address arithmetic of a placement; private to this crate.
    pub trait Sealed {
        /// The word at byte `offset` from the block's base.
        fn word(&self, offset: usize) -> *mut u32;
        /// A second handle on the same placement, for the parts of a split.
        fn duplicate(&self) -> Self;
    }
}

/// Where a register block lives: device registers at a fixed address
/// ([`Fixed`]) or host memory ([`Host`]). Only this crate implements it.
pub trait Placement: sealed::Sealed {}

/// Device registers at the fixed address `BASE`.
///
/// It is made only from a [`Span`], so its existence is the span's promise
/// that the registers are this block's alone. It is `Send` (an owner moves
/// between execution contexts) and not `Sync`.
pub struct Fixed<const BASE: usize> {
    _owner: PhantomData<Cell<()>>,
}

impl<const BASE: usize> sealed::Sealed for Fixed<BASE> {
    #[inline(always)]
    fn word(&self, offset: usize) -> *mut u32 {
        core::ptr::with_exposed_provenance_mut(BASE + offset)
    }

    fn duplicate(&self) -> Self {
        Self {
            _owner: PhantomData,
        }
    }
}

impl<const BASE: usize> Placement for Fixed<BASE> {}

/// Host memory standing in for a register block in a test.
pub struct Host<'a> {
    base: NonNull<u32>,
    _memory: PhantomData<&'a mut [u32]>,
}

impl sealed::Sealed for Host<'_> {
    #[inline(always)]
    fn word(&self, offset: usize) -> *mut u32 {
        self.base.as_ptr().wrapping_add(offset / 4)
    }

    fn duplicate(&self) -> Self {
        Self {
            base: self.base,
            _memory: PhantomData,
        }
    }
}

impl Placement for Host<'_> {}

/// The exclusive authority over the device registers `START..END`.
///
/// ```compile_fail,E0133
/// let span = oer_mmio::Span::<0x1000, 0x1100>::assume_owned();
/// ```
#[must_use = "dropping a span permanently loses its register authority"]
pub struct Span<const START: usize, const END: usize> {
    _owner: PhantomData<Cell<()>>,
}

impl<const START: usize, const END: usize> Span<START, END> {
    /// Take the registers `START..END`.
    ///
    /// # Safety
    ///
    /// `START..END` are device registers, word aligned, that no other code
    /// accesses for as long as the span or anything taken from it exists,
    /// and no other span over any of them exists at the same time.
    #[allow(unsafe_code, reason = "the caller promises exclusive registers")]
    pub const unsafe fn assume_owned() -> Self {
        const {
            assert!(
                START <= END && START.is_multiple_of(4) && END.is_multiple_of(4),
                "a span is a word-aligned address range"
            );
        };
        Self {
            _owner: PhantomData,
        }
    }

    /// Divide the span at `MID`; the halves cover disjoint registers.
    pub const fn split_at<const MID: usize>(self) -> (Span<START, MID>, Span<MID, END>) {
        const {
            assert!(
                START <= MID && MID <= END && MID.is_multiple_of(4),
                "a span splits at a word-aligned address inside it"
            );
        };
        (
            Span {
                _owner: PhantomData,
            },
            Span {
                _owner: PhantomData,
            },
        )
    }

    /// The register block of `SIZE` bytes at the span's start; the rest of
    /// the span is given up.
    pub const fn into_block<const SIZE: usize>(self) -> Block<Fixed<START>, SIZE> {
        const {
            assert!(
                SIZE.is_multiple_of(4) && START + SIZE <= END,
                "a block lies inside its span"
            );
        };
        Block {
            place: Fixed {
                _owner: PhantomData,
            },
        }
    }
}

/// The exclusive authority over one register block of `SIZE` bytes.
///
/// Its reads and writes take the word's byte offset as a const generic;
/// an offset outside the block fails to compile.
#[must_use = "dropping a register block permanently loses its register authority"]
pub struct Block<P: Placement, const SIZE: usize> {
    place: P,
}

/// Host memory too small for the block placed in it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostTooSmall {
    /// The block's size in bytes.
    pub required: usize,
    /// The memory's size in bytes.
    pub available: usize,
}

impl<'a, const SIZE: usize> Block<Host<'a>, SIZE> {
    /// A block over `words`, which holds at least `SIZE` bytes.
    pub fn host(words: &'a mut [u32]) -> Result<Self, HostTooSmall> {
        let available = words.len().saturating_mul(4);
        if available < SIZE {
            return Err(HostTooSmall {
                required: SIZE,
                available,
            });
        }
        Ok(Self {
            place: Host {
                base: NonNull::from(words).cast(),
                _memory: PhantomData,
            },
        })
    }
}

/// Whether the word at byte `offset` lies inside a block of `size` bytes.
const fn in_block(offset: usize, size: usize) -> bool {
    offset.is_multiple_of(4) && offset < size && size - offset >= 4
}

/// Whether every element of an array of `count` words, `stride` bytes apart
/// from `offset`, lies inside a block of `size` bytes.
const fn array_in_block(offset: usize, stride: usize, count: usize, size: usize) -> bool {
    if count == 0 || !stride.is_multiple_of(4) {
        return false;
    }
    match stride.checked_mul(count - 1) {
        Some(span) => match offset.checked_add(span) {
            Some(last) => in_block(offset, size) && in_block(last, size),
            None => false,
        },
        None => false,
    }
}

#[allow(
    unsafe_code,
    reason = "the volatile accesses of a block's checked words"
)]
impl<P: Placement, const SIZE: usize> Block<P, SIZE> {
    /// One volatile read of the word at byte `OFFSET`.
    #[inline(always)]
    pub fn read<const OFFSET: usize>(&self) -> u32 {
        const { assert!(in_block(OFFSET, SIZE), "the word lies inside the block") };
        // SAFETY: the block is the exclusive authority over its `SIZE`
        // bytes (a span's promise, or host memory it borrows mutably) and
        // the word lies inside them, checked at compile time above.
        unsafe { self.place.word(OFFSET).read_volatile() }
    }

    /// One volatile write of the whole word at byte `OFFSET`.
    #[inline(always)]
    pub fn write<const OFFSET: usize>(&mut self, value: u32) {
        const { assert!(in_block(OFFSET, SIZE), "the word lies inside the block") };
        // SAFETY: as in `read`; `&mut self` makes this the only access.
        unsafe { self.place.word(OFFSET).write_volatile(value) }
    }

    /// One volatile read of element `index` of the word array at `OFFSET`.
    #[inline(always)]
    pub fn read_indexed<const OFFSET: usize, const STRIDE: usize, const COUNT: usize>(
        &self,
        index: Index<COUNT>,
    ) -> u32 {
        const {
            assert!(
                array_in_block(OFFSET, STRIDE, COUNT, SIZE),
                "every element lies inside the block"
            );
        };
        // SAFETY: every element lies inside the block (checked at compile
        // time above) and `index < COUNT` by `Index`'s constructor.
        unsafe {
            self.place
                .word(OFFSET + STRIDE * index.get())
                .read_volatile()
        }
    }

    /// One volatile write of element `index` of the word array at `OFFSET`.
    #[inline(always)]
    pub fn write_indexed<const OFFSET: usize, const STRIDE: usize, const COUNT: usize>(
        &mut self,
        index: Index<COUNT>,
        value: u32,
    ) {
        const {
            assert!(
                array_in_block(OFFSET, STRIDE, COUNT, SIZE),
                "every element lies inside the block"
            );
        };
        // SAFETY: as in `read_indexed`; `&mut self` makes this the only access.
        unsafe {
            self.place
                .word(OFFSET + STRIDE * index.get())
                .write_volatile(value)
        }
    }

    /// Divide the block into a part owning `A`'s words and one owning `B`'s.
    ///
    /// The two owned lists are disjoint and lie inside the block, checked
    /// at compile time; each part may also read the words it observes.
    pub fn split<A: Words, B: Words>(self) -> (Part<P, SIZE, A>, Part<P, SIZE, B>) {
        const {
            assert!(
                disjoint(A::OWNED, B::OWNED),
                "the parts of a block own disjoint words"
            );
            assert!(
                all_in_block(A::OWNED, SIZE)
                    && all_in_block(A::OBSERVED, SIZE)
                    && all_in_block(B::OWNED, SIZE)
                    && all_in_block(B::OBSERVED, SIZE),
                "every word of a part lies inside the block"
            );
        };
        let other = self.place.duplicate();
        (
            Part {
                place: self.place,
                words: PhantomData,
            },
            Part {
                place: other,
                words: PhantomData,
            },
        )
    }

    /// The block again, from the two parts of one split.
    pub fn join<A: Words, B: Words>(first: Part<P, SIZE, A>, second: Part<P, SIZE, B>) -> Self {
        let Part { place, .. } = first;
        drop(second);
        Self { place }
    }
}

/// The words of a block one [`Part`] owns (reads and writes) and observes
/// (reads only), as byte offsets; an array lists every element.
pub trait Words {
    /// The words the part reads and writes; no other part owns them.
    const OWNED: &'static [usize];
    /// The words the part only reads.
    const OBSERVED: &'static [usize];
}

const fn contains(mut list: &[usize], word: usize) -> bool {
    while let [first, rest @ ..] = list {
        if *first == word {
            return true;
        }
        list = rest;
    }
    false
}

const fn disjoint(mut first: &[usize], second: &[usize]) -> bool {
    while let [word, rest @ ..] = first {
        if contains(second, *word) {
            return false;
        }
        first = rest;
    }
    true
}

const fn all_in_block(mut list: &[usize], size: usize) -> bool {
    while let [word, rest @ ..] = list {
        if !in_block(*word, size) {
            return false;
        }
        list = rest;
    }
    true
}

const fn array_listed(list: &[usize], offset: usize, stride: usize, count: usize) -> bool {
    let mut element = 0;
    while element < count {
        if !contains(list, offset + stride * element) {
            return false;
        }
        element += 1;
    }
    true
}

/// One part of a split [`Block`]: it writes only the words `W` owns and
/// reads those and the words `W` observes.
#[must_use = "a part must be joined with its sibling to recover the block"]
pub struct Part<P: Placement, const SIZE: usize, W: Words> {
    place: P,
    words: PhantomData<fn() -> W>,
}

#[allow(unsafe_code, reason = "the volatile accesses of a part's listed words")]
impl<P: Placement, const SIZE: usize, W: Words> Part<P, SIZE, W> {
    /// One volatile read of a word the part owns or observes.
    #[inline(always)]
    pub fn read<const OFFSET: usize>(&self) -> u32 {
        const {
            assert!(
                contains(W::OWNED, OFFSET) || contains(W::OBSERVED, OFFSET),
                "a part reads only the words it owns or observes"
            );
        };
        // SAFETY: the split checked that every listed word lies inside the
        // block, whose authority the part shares only for the words it lists.
        unsafe { self.place.word(OFFSET).read_volatile() }
    }

    /// One volatile write of a word the part owns.
    #[inline(always)]
    pub fn write<const OFFSET: usize>(&mut self, value: u32) {
        const {
            assert!(
                contains(W::OWNED, OFFSET),
                "a part writes only the words it owns"
            );
        };
        // SAFETY: as in `read`; no other part owns the word (the split
        // checked the owned lists are disjoint) and `&mut self` makes this
        // the part's only access.
        unsafe { self.place.word(OFFSET).write_volatile(value) }
    }

    /// One volatile read of an array element the part owns or observes.
    #[inline(always)]
    pub fn read_indexed<const OFFSET: usize, const STRIDE: usize, const COUNT: usize>(
        &self,
        index: Index<COUNT>,
    ) -> u32 {
        const {
            assert!(
                array_listed(W::OWNED, OFFSET, STRIDE, COUNT)
                    || array_listed(W::OBSERVED, OFFSET, STRIDE, COUNT),
                "a part reads only the arrays it owns or observes"
            );
        };
        // SAFETY: every element is a listed word inside the block (checked
        // above and by the split) and `index < COUNT` by `Index`.
        unsafe {
            self.place
                .word(OFFSET + STRIDE * index.get())
                .read_volatile()
        }
    }

    /// One volatile write of an array element the part owns.
    #[inline(always)]
    pub fn write_indexed<const OFFSET: usize, const STRIDE: usize, const COUNT: usize>(
        &mut self,
        index: Index<COUNT>,
        value: u32,
    ) {
        const {
            assert!(
                array_listed(W::OWNED, OFFSET, STRIDE, COUNT),
                "a part writes only the arrays it owns"
            );
        };
        // SAFETY: as in `read_indexed`; the part owns every element.
        unsafe {
            self.place
                .word(OFFSET + STRIDE * index.get())
                .write_volatile(value)
        }
    }
}

/// The compile-time checks the rendered register transactions assert next
/// to each transaction: `const _: () = assert!(check::within(..))`.
pub mod check {
    /// Whether `image` sets only bits of `fields`.
    pub const fn within(image: u32, fields: u32) -> bool {
        image & !fields == 0
    }

    /// Whether the kept, input and set masks are disjoint, cover the whole
    /// word, and leave only bits of `fields` to change.
    pub const fn partition(keep: u32, input: u32, set: u32, fields: u32) -> bool {
        keep & input == 0
            && keep & set == 0
            && input & set == 0
            && keep | input | set == u32::MAX
            && within(!keep, fields)
    }
}

/// An index below `COUNT`: an element of a register array.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Index<const COUNT: usize>(usize);

/// An index outside its array.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndexError {
    /// The refused index.
    pub index: usize,
    /// The array's element count.
    pub count: usize,
}

impl<const COUNT: usize> Index<COUNT> {
    /// The index `index`, if it is below `COUNT`.
    pub const fn new(index: usize) -> Result<Self, IndexError> {
        if index < COUNT {
            Ok(Self(index))
        } else {
            Err(IndexError {
                index,
                count: COUNT,
            })
        }
    }

    /// The index `I`, checked at compile time.
    pub const fn constant<const I: usize>() -> Self {
        const { assert!(I < COUNT, "the index lies inside the array") };
        Self(I)
    }

    /// The index as a number below `COUNT`.
    pub const fn get(self) -> usize {
        self.0
    }
}

/// An unsigned value of exactly `WIDTH` bits: a field without a narrower
/// domain.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Bits<const WIDTH: u32>(u32);

/// A value wider than its field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ValueError {
    /// The refused value.
    pub value: u32,
    /// The field's width in bits.
    pub width: u32,
}

impl<const WIDTH: u32> Bits<WIDTH> {
    /// The largest value.
    pub const MAX: u32 = {
        assert!(WIDTH >= 1 && WIDTH <= 32, "a field is 1 to 32 bits wide");
        u32::MAX >> (32 - WIDTH)
    };

    /// The value `value`, if it fits `WIDTH` bits.
    pub const fn new(value: u32) -> Result<Self, ValueError> {
        if value <= Self::MAX {
            Ok(Self(value))
        } else {
            Err(ValueError {
                value,
                width: WIDTH,
            })
        }
    }

    /// The value `V`, checked at compile time.
    pub const fn constant<const V: u32>() -> Self {
        const { assert!(V <= Self::MAX, "the value fits the field") };
        Self(V)
    }

    /// The value.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// A word in memory the hardware may change: volatile access only.
///
/// It is not `Sync`; its zero bytes are a valid value.
#[repr(transparent)]
pub struct VolatileCell<T: Copy>(UnsafeCell<T>);

#[allow(unsafe_code, reason = "the volatile accesses of a cell's own value")]
impl<T: Copy> VolatileCell<T> {
    /// A cell holding `value`.
    pub const fn new(value: T) -> Self {
        Self(UnsafeCell::new(value))
    }

    /// One volatile read.
    #[inline(always)]
    pub fn get(&self) -> T {
        // SAFETY: the cell is not `Sync`, so no other thread accesses it;
        // the pointer is valid and aligned for `T` for as long as `self`.
        unsafe { self.0.get().read_volatile() }
    }

    /// One volatile write.
    #[inline(always)]
    pub fn set(&self, value: T) {
        // SAFETY: as in `get`; `UnsafeCell` permits the write through `&self`
        // and no reference into the cell's value exists.
        unsafe { self.0.get().write_volatile(value) }
    }
}

#[allow(unsafe_code, reason = "a transparent cell of a zeroable value")]
// SAFETY: `VolatileCell<T>` is a transparent `UnsafeCell<T>`, which has the
// layout and validity of `T`; all-zero bytes are a valid `T: Zeroable`.
unsafe impl<T: Copy + bytemuck::Zeroable> bytemuck::Zeroable for VolatileCell<T> {}

/// The RAM region a chip's DMA engines reach, as the generated `layouts`
/// module of its PAC names it.
pub trait DmaWindow {
    /// The region's addresses.
    const RANGE: core::ops::Range<u32>;
}

/// An address inside the DMA window `W` whose memory stays reserved for
/// the hardware while it may read the address, as the hardware reads it.
pub struct DmaWindowAddress<W: DmaWindow> {
    address: u32,
    window: PhantomData<fn() -> W>,
}

impl<W: DmaWindow> DmaWindowAddress<W> {
    /// The address `address`.
    ///
    /// # Safety
    ///
    /// `address` lies inside `W::RANGE`, in a static placed there or a
    /// buffer lent to the hardware, which stays reserved for as long as the
    /// hardware may read the address.
    #[allow(unsafe_code, reason = "the caller promises a placed address")]
    pub const unsafe fn new_unchecked(address: u32) -> Self {
        Self {
            address,
            window: PhantomData,
        }
    }

    /// The address as the hardware reads it.
    pub const fn get(self) -> u32 {
        self.address
    }
}

impl<W: DmaWindow> Clone for DmaWindowAddress<W> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<W: DmaWindow> Copy for DmaWindowAddress<W> {}

impl<W: DmaWindow> PartialEq for DmaWindowAddress<W> {
    fn eq(&self, other: &Self) -> bool {
        self.address == other.address
    }
}

impl<W: DmaWindow> Eq for DmaWindowAddress<W> {}

impl<W: DmaWindow> core::hash::Hash for DmaWindowAddress<W> {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.address.hash(state);
    }
}

impl<W: DmaWindow> core::fmt::Debug for DmaWindowAddress<W> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_tuple("DmaWindowAddress")
            .field(&self.address)
            .finish()
    }
}

/// A memory layout placed only in its DMA window, which a [`DmaRef`] may
/// point at.
pub trait DmaLayout {
    /// The window the layout lies in.
    type Window: DmaWindow;
}

/// The address of an `L` inside its DMA window.
pub struct DmaRef<L: DmaLayout> {
    address: DmaWindowAddress<L::Window>,
    layout: PhantomData<fn() -> L>,
}

impl<L: DmaLayout> DmaRef<L> {
    /// The reference to the `L` at `address`.
    ///
    /// # Safety
    ///
    /// An `L`, aligned as `L` requires, lies at `address` for as long as the
    /// hardware may follow the reference.
    #[allow(unsafe_code, reason = "the caller promises a placed layout")]
    pub const unsafe fn new_unchecked(address: DmaWindowAddress<L::Window>) -> Self {
        Self {
            address,
            layout: PhantomData,
        }
    }

    /// The referenced address.
    pub const fn address(self) -> DmaWindowAddress<L::Window> {
        self.address
    }

    /// The address as the hardware reads it.
    pub const fn bits(self) -> u32 {
        self.address.address
    }
}

impl<L: DmaLayout> Clone for DmaRef<L> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<L: DmaLayout> Copy for DmaRef<L> {}

impl<L: DmaLayout> PartialEq for DmaRef<L> {
    fn eq(&self, other: &Self) -> bool {
        self.address == other.address
    }
}

impl<L: DmaLayout> Eq for DmaRef<L> {}

impl<L: DmaLayout> core::fmt::Debug for DmaRef<L> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_tuple("DmaRef")
            .field(&self.address)
            .finish()
    }
}

#[cfg(test)]
mod tests;
