#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing
)]
//! The one volatile register-access core under the generated register code.
//!
//! The register generator renders typed register blocks that hold a
//! [`Block`] (or the [`Part`]s of one) and call only its word-checked reads
//! and writes; the generated code contains no `unsafe`. Everything a
//! volatile access needs to be sound is decided here:
//!
//! - **Which words a block has.** A block's authority is the list of its
//!   declared words ([`Registers`]), not the address range they span: two
//!   blocks whose ranges interleave, or a radio view of a platform block
//!   whose other words another owner writes, each reach only their own
//!   words. Every read and write names its word as a const generic, checked
//!   against the list when it is monomorphized; a runtime array index is an
//!   [`Index`], made only by a checked constructor.
//! - **Where a block lives.** A block of device registers is made only by
//!   [`assume_owned`], the one `unsafe` promise a chip's PAC root makes, for
//!   the whole set of blocks it owns at once ([`Claim`]: a block or a tuple of
//!   claims). The PAC asserts at compile time that the claimed blocks list
//!   disjoint words ([`check::disjoint_blocks`]). [`Block::host`] places a
//!   block in host memory instead, for tests of the rendered accessors.
//! - **Who may write a word.** [`Block::split`] divides a block into two
//!   [`Part`]s whose owned word lists ([`Words`]) are disjoint, again at
//!   compile time; a part writes only its owned words and also reads the
//!   words it observes. Joining parts of device registers is checked by
//!   their type; host parts are checked to lie over the same memory.
//!   [`Block::divide`] makes up to four parts, which may also share a
//!   write-trigger word, each storing only its own bits ([`Words::TRIGGERS`],
//!   [`Part::write_trigger`]); joined parts own every word of the block.
//! - **Who may only read.** [`Block::into_reader`] and [`Part::into_reader`]
//!   give up every write for a `Copy` [`Reader`] of the listed words.
//! - **Who may share a block.** A block whose words list [`SharedRegisters`]
//!   (the generator's shared blocks: side-effect-free reads, write-trigger
//!   strobes and one-field registers stored whole) is written through
//!   `&self` with [`Block::write_shared`]; at a [`Fixed`] address it is
//!   `Sync`, so one owner in a `static` serves every execution context.
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
        /// The read-only handle a [`Reader`](super::Reader) keeps.
        type Reading: Reading;
        /// The word at byte `offset` from the block's base.
        fn word(&self, offset: usize) -> *mut u32;
        /// A second handle on the same placement, for the parts of a split.
        fn duplicate(&self) -> Self;
        /// The placement as a read-only handle, which it is copied into.
        fn reading(self) -> Self::Reading;
    }

    /// A placement's read-only handle; private to this crate.
    pub trait Reading: Copy {
        /// The word at byte `offset` from the block's base.
        fn word(&self, offset: usize) -> *const u32;
    }

    /// The word lists of the parts of one division; private to this crate.
    pub trait Divides {
        /// The parts, each over the placement `P` of a block of `R`.
        type Parts<P: super::Placement, R: super::Registers>;
        /// Each part's owned, observed and trigger words.
        const WORDS: &'static [super::PartWords];
        /// Make the parts over `place`.
        fn parts<P: super::Placement, R: super::Registers>(place: P) -> Self::Parts<P, R>;
    }

    /// The token only this crate makes: claims are built by
    /// [`assume_owned`](super::assume_owned) alone.
    pub struct Promise(pub(super) ());

    /// A set of device-register blocks made from one promise; private to
    /// this crate.
    pub trait Claimed: Sized {
        /// The blocks, once the caller of `assume_owned` promised them.
        fn claimed(promise: &Promise) -> Self;
    }
}

/// Where a register block lives: device registers at a fixed address
/// ([`Fixed`]) or host memory ([`Host`]). Only this crate implements it.
pub trait Placement: sealed::Sealed {}

/// Device registers at the fixed address `BASE`.
///
/// It is made only by [`assume_owned`], so its existence is the promise that
/// the block's words are its alone. It is `Send` (an owner moves between
/// execution contexts) and not `Sync`.
pub struct Fixed<const BASE: usize> {
    _owner: PhantomData<Cell<()>>,
}

impl<const BASE: usize> sealed::Sealed for Fixed<BASE> {
    type Reading = FixedReading<BASE>;

    #[inline(always)]
    fn word(&self, offset: usize) -> *mut u32 {
        core::ptr::with_exposed_provenance_mut(BASE + offset)
    }

    fn duplicate(&self) -> Self {
        Self {
            _owner: PhantomData,
        }
    }

    fn reading(self) -> FixedReading<BASE> {
        FixedReading
    }
}

/// The read-only handle on device registers at `BASE` a [`Reader`] keeps:
/// `Copy`, `Send` and `Sync`, since it only reads.
#[derive(Clone, Copy)]
pub struct FixedReading<const BASE: usize>;

impl<const BASE: usize> sealed::Reading for FixedReading<BASE> {
    #[inline(always)]
    fn word(&self, offset: usize) -> *const u32 {
        core::ptr::with_exposed_provenance(BASE + offset)
    }
}

impl<const BASE: usize> Placement for Fixed<BASE> {}

/// Host memory standing in for a register block in a test.
pub struct Host<'a> {
    base: NonNull<u32>,
    _memory: PhantomData<&'a mut [u32]>,
}

impl<'a> sealed::Sealed for Host<'a> {
    type Reading = HostReading<'a>;

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

    fn reading(self) -> HostReading<'a> {
        HostReading {
            base: self.base,
            _memory: PhantomData,
        }
    }
}

/// The read-only handle on host memory a [`Reader`] keeps: `Copy`, and
/// neither `Send` nor `Sync`, like the host block it was taken from.
#[derive(Clone, Copy)]
pub struct HostReading<'a> {
    base: NonNull<u32>,
    _memory: PhantomData<&'a [u32]>,
}

impl sealed::Reading for HostReading<'_> {
    #[inline(always)]
    fn word(&self, offset: usize) -> *const u32 {
        self.base.as_ptr().wrapping_add(offset / 4)
    }
}

impl Placement for Host<'_> {}

/// The declared words of one register block, as byte offsets from its base,
/// strictly ascending; an array lists every element.
pub trait Registers {
    /// The block's words.
    const WORDS: &'static [usize];
}

/// The words of a block that every execution context may reach at once:
/// each write it makes is a whole-word store that reads nothing back (a
/// write-trigger strobe, or a one-field register stored whole), and each
/// read has no side effect. The register generator implements it only for
/// a policy's shared block, whose registers it checks are of these kinds.
///
/// Only such a block is `Sync`; an ordinary block at a fixed address is not:
///
/// ```compile_fail,E0277
/// struct Words;
/// impl oer_mmio::Registers for Words {
///     const WORDS: &'static [usize] = &[0];
/// }
/// fn sync<T: Sync>() {}
/// sync::<oer_mmio::Block<oer_mmio::Fixed<0x1000>, Words>>();
/// ```
pub trait SharedRegisters: Registers {}

/// The interrupt-matrix source of a register owner's interrupt: the owner
/// a handler holds (the interrupt side of a divided block, or the whole
/// block) names the source an interrupt table binds its handler to. The
/// generator implements it from the model's source number; it grants no
/// register access.
pub trait InterruptSource {
    /// The source number of the chip's interrupt matrix.
    const SOURCE: u16;
}

/// Every interrupt-matrix source a register owner's handler serves: the one
/// source of an [`InterruptSource`], or the several sources of a part one
/// handler acknowledges through one bank. The generator implements it alone
/// for a part of several sources, so such a part never stands where one
/// source is expected; it grants no register access.
pub trait InterruptSources {
    /// The source numbers of the chip's interrupt matrix, each once.
    const SOURCES: &'static [u16];
}

impl<T: InterruptSource + ?Sized> InterruptSources for T {
    const SOURCES: &'static [u16] = &[T::SOURCE];
}

/// Blocks of device registers one promise of [`assume_owned`] makes: a
/// [`Block`] at a [`Fixed`] address, or a tuple of up to eight claims.
pub trait Claim: sealed::Claimed {}

impl<const BASE: usize, R: Registers> sealed::Claimed for Block<Fixed<BASE>, R> {
    fn claimed(_promise: &sealed::Promise) -> Self {
        const {
            assert!(
                BASE.is_multiple_of(4) && !R::WORDS.is_empty() && ascending(R::WORDS),
                "a claimed block lists word-aligned words, strictly ascending"
            );
            assert!(
                BASE.checked_add(end(R::WORDS)).is_some(),
                "a claimed block lies inside the address space"
            );
        };
        Block {
            place: Fixed {
                _owner: PhantomData,
            },
            registers: PhantomData,
        }
    }
}

impl<const BASE: usize, R: Registers> Claim for Block<Fixed<BASE>, R> {}

macro_rules! tuple_claims {
    ($($claim:ident),+) => {
        impl<$($claim: Claim),+> sealed::Claimed for ($($claim,)+) {
            fn claimed(promise: &sealed::Promise) -> Self {
                ($($claim::claimed(promise),)+)
            }
        }

        impl<$($claim: Claim),+> Claim for ($($claim,)+) {}
    };
}

tuple_claims!(A);
tuple_claims!(A, B);
tuple_claims!(A, B, C);
tuple_claims!(A, B, C, D);
tuple_claims!(A, B, C, D, E);
tuple_claims!(A, B, C, D, E, F);
tuple_claims!(A, B, C, D, E, F, G);
tuple_claims!(A, B, C, D, E, F, G, H);

/// Take the device-register blocks of `C`.
///
/// ```compile_fail,E0133
/// struct Control;
/// impl oer_mmio::Registers for Control {
///     const WORDS: &'static [usize] = &[0];
/// }
/// let block: oer_mmio::Block<oer_mmio::Fixed<0x1000>, Control> = oer_mmio::assume_owned();
/// ```
///
/// # Safety
///
/// Every word each block of `C` lists, at its block's base, is a device
/// register that no other code accesses for as long as the block or anything
/// taken from it exists, except a word the chip's reviewed shared-word list
/// names with the coordination of its writers; no word is listed by two
/// blocks of `C` or by a block of another claim that exists at the same time.
#[allow(unsafe_code, reason = "the caller promises exclusive registers")]
pub unsafe fn assume_owned<C: Claim>() -> C {
    C::claimed(&sealed::Promise(()))
}

/// Order every device-memory access before this call against every one
/// after it: the fence a driver places at a hardware ownership boundary.
#[inline]
pub fn fence() {
    #[cfg(target_arch = "riscv32")]
    #[allow(unsafe_code, reason = "an ordering instruction")]
    // SAFETY: the instruction only orders memory and device accesses; it
    // reads and writes no memory and changes no register Rust relies on.
    unsafe {
        core::arch::asm!("fence iorw, iorw", options(nostack, preserves_flags))
    }
    #[cfg(not(target_arch = "riscv32"))]
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
}

/// The exclusive authority over the words `R` lists of one register block.
///
/// Its reads and writes take the word's byte offset as a const generic;
/// an offset `R` does not list fails to compile.
#[must_use = "dropping a register block permanently loses its register authority"]
pub struct Block<P: Placement, R: Registers> {
    place: P,
    registers: PhantomData<fn() -> R>,
}

/// Host memory too small for the block placed in it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostTooSmall {
    /// The bytes the block's last word ends at.
    pub required: usize,
    /// The memory's size in bytes.
    pub available: usize,
}

impl<'a, R: Registers> Block<Host<'a>, R> {
    /// A block over `words`, which holds every word `R` lists.
    ///
    /// It is a `const fn` so that a block whose words end past the address
    /// space fails where it is placed, even in a constant.
    pub const fn host(words: &'a mut [u32]) -> Result<Self, HostTooSmall> {
        let required = const {
            assert!(
                ascending(R::WORDS),
                "a block lists word-aligned words, strictly ascending"
            );
            end(R::WORDS)
        };
        let available = words.len().saturating_mul(4);
        if available < required {
            return Err(HostTooSmall {
                required,
                available,
            });
        }
        Ok(Self {
            place: Host {
                base: NonNull::from_mut(words).cast(),
                _memory: PhantomData,
            },
            registers: PhantomData,
        })
    }
}

/// Whether `list` holds word-aligned offsets in strictly ascending order.
const fn ascending(mut list: &[usize]) -> bool {
    let mut previous: Option<usize> = None;
    while let [word, rest @ ..] = list {
        if !word.is_multiple_of(4) {
            return false;
        }
        if let Some(previous) = previous
            && *word <= previous
        {
            return false;
        }
        previous = Some(*word);
        list = rest;
    }
    true
}

/// The byte just past the last word of an ascending `list`.
///
/// It is evaluated only at compile time: a list whose last word ends past
/// the address space fails the evaluation instead of wrapping.
const fn end(list: &[usize]) -> usize {
    match list.last() {
        Some(last) => {
            assert!(
                *last <= usize::MAX - 4,
                "a block's words lie inside the address space"
            );
            *last + 4
        }
        None => 0,
    }
}

/// Whether the ascending `list` holds `word`.
const fn listed(mut list: &[usize], word: usize) -> bool {
    while !list.is_empty() {
        let (low, high) = list.split_at(list.len() / 2);
        let [middle, rest @ ..] = high else {
            return false;
        };
        if *middle == word {
            return true;
        }
        list = if *middle < word { rest } else { low };
    }
    false
}

/// Whether `list` holds every element of an array of `count` words,
/// `stride` bytes apart from `offset`.
const fn array_listed(list: &[usize], offset: usize, stride: usize, count: usize) -> bool {
    if count == 0 {
        return false;
    }
    let mut element = 0;
    while element < count {
        let word = match stride.checked_mul(element) {
            Some(step) => match offset.checked_add(step) {
                Some(word) => word,
                None => return false,
            },
            None => return false,
        };
        if !contains(list, word) {
            return false;
        }
        element += 1;
    }
    true
}

#[allow(
    unsafe_code,
    reason = "the volatile accesses of a block's checked words"
)]
impl<P: Placement, R: Registers> Block<P, R> {
    /// One volatile read of the word at byte `OFFSET`.
    #[inline(always)]
    pub fn read<const OFFSET: usize>(&self) -> u32 {
        const { assert!(listed(R::WORDS, OFFSET), "the block lists the word") };
        // SAFETY: the block is the exclusive authority over the words it
        // lists (`assume_owned`'s promise, or host memory it borrows mutably
        // and checked to reach the end of its last word, an end computed
        // without overflow at compile time) and lists this one, checked at
        // compile time above.
        unsafe { self.place.word(OFFSET).read_volatile() }
    }

    /// One volatile write of the whole word at byte `OFFSET`.
    #[inline(always)]
    pub fn write<const OFFSET: usize>(&mut self, value: u32) {
        const { assert!(listed(R::WORDS, OFFSET), "the block lists the word") };
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
                array_listed(R::WORDS, OFFSET, STRIDE, COUNT),
                "the block lists every element"
            );
        };
        // SAFETY: the block lists every element (checked at compile time
        // above) and `index < COUNT` by `Index`'s constructor.
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
                array_listed(R::WORDS, OFFSET, STRIDE, COUNT),
                "the block lists every element"
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
    /// The two owned lists are disjoint and the block lists every word of
    /// both parts, checked at compile time; each part may also read the
    /// words it observes. It is the division into the two parts `(A, B)`.
    pub fn split<A: Words, B: Words>(self) -> (Part<P, R, A>, Part<P, R, B>) {
        self.divide::<(A, B)>()
    }

    /// Divide the block into the parts of `D`, a tuple of two to four
    /// [`Words`]: one [`Part`] per element, in order.
    ///
    /// Checked at compile time: the block lists every word of every part,
    /// no two parts own the same word, and a shared write-trigger word
    /// ([`Words::TRIGGERS`]) is owned by no part and gives each part its own
    /// bits, disjoint from every other part's.
    pub fn divide<D: Division>(self) -> D::Parts<P, R> {
        const {
            assert!(
                parts_listed(R::WORDS, D::WORDS),
                "the block lists every word of its parts"
            );
            assert!(
                owned_disjoint(D::WORDS),
                "the parts of a block own disjoint words"
            );
            assert!(
                triggers_disjoint(D::WORDS),
                "a shared trigger word is owned by no part and its parts' bits are disjoint"
            );
        };
        D::parts(self.place)
    }

    /// The block as a [`Reader`] of the words `W` lists, giving up every
    /// write: no word the block lists is written again.
    pub fn into_reader<W: Words>(self) -> Reader<P, R, W> {
        const {
            assert!(
                all_listed(W::OWNED, R::WORDS) && all_listed(W::OBSERVED, R::WORDS),
                "the block lists every word of its reader"
            );
        };
        Reader {
            place: self.place.reading(),
            words: PhantomData,
        }
    }
}

/// One part's word lists, as a division checks them: owned, observed and
/// shared write-trigger words with the part's bits.
pub type PartWords = (&'static [usize], &'static [usize], &'static [(usize, u32)]);

/// The parts one [`Block::divide`] makes: a tuple of two to four [`Words`].
pub trait Division: sealed::Divides {}

macro_rules! divisions {
    ($($part:ident),+) => {
        impl<$($part: Words),+> sealed::Divides for ($($part,)+) {
            type Parts<P: Placement, R: Registers> = ($(Part<P, R, $part>,)+);
            const WORDS: &'static [PartWords] =
                &[$(($part::OWNED, $part::OBSERVED, $part::TRIGGERS)),+];

            #[allow(non_snake_case, reason = "one binding per part type")]
            fn parts<P: Placement, R: Registers>(place: P) -> Self::Parts<P, R> {
                $(let $part = Part {
                    place: place.duplicate(),
                    words: PhantomData,
                };)+
                drop(place);
                ($($part,)+)
            }
        }

        impl<$($part: Words),+> Division for ($($part,)+) {}
    };
}

divisions!(A, B);
divisions!(A, B, C);
divisions!(A, B, C, D);

/// Whether `block` lists every owned, observed and trigger word of `parts`,
/// and no part names a trigger word twice or with no bits.
const fn parts_listed(block: &[usize], mut parts: &[PartWords]) -> bool {
    while let [(owned, observed, triggers), rest @ ..] = parts {
        if !all_listed(owned, block) || !all_listed(observed, block) {
            return false;
        }
        let mut list: &[(usize, u32)] = triggers;
        while let [(word, mask), others @ ..] = list {
            if *mask == 0 || !listed(block, *word) || trigger_mask(others, *word) != 0 {
                return false;
            }
            list = others;
        }
        parts = rest;
    }
    true
}

/// Whether no two of `parts` own the same word.
const fn owned_disjoint(mut parts: &[PartWords]) -> bool {
    while let [(owned, ..), rest @ ..] = parts {
        let mut others = rest;
        while let [(other, ..), more @ ..] = others {
            if !disjoint(owned, other) {
                return false;
            }
            others = more;
        }
        parts = rest;
    }
    true
}

/// Whether every trigger word of `parts` is owned by none of them and no two
/// parts have a common bit in it.
const fn triggers_disjoint(all: &[PartWords]) -> bool {
    let mut parts = all;
    while let [(_, _, triggers), rest @ ..] = parts {
        let mut list: &[(usize, u32)] = triggers;
        while let [(word, mask), others @ ..] = list {
            let mut owners = all;
            while let [(owned, ..), more @ ..] = owners {
                if contains(owned, *word) {
                    return false;
                }
                owners = more;
            }
            let mut later = rest;
            while let [(_, _, other), more @ ..] = later {
                if trigger_mask(other, *word) & *mask != 0 {
                    return false;
                }
                later = more;
            }
            list = others;
        }
        parts = rest;
    }
    true
}

/// The bits `triggers` gives its part in `word`; zero if it names none.
const fn trigger_mask(mut triggers: &[(usize, u32)], word: usize) -> u32 {
    while let [(listed, mask), rest @ ..] = triggers {
        if *listed == word {
            return *mask;
        }
        triggers = rest;
    }
    0
}

/// Whether the parts `first` and `second` own every word of `block`: then
/// no other part of the block's division owns or triggers any word.
const fn owners_cover(block: &[usize], first: &[usize], second: &[usize]) -> bool {
    let mut words = block;
    while let [word, rest @ ..] = words {
        if !contains(first, *word) && !contains(second, *word) {
            return false;
        }
        words = rest;
    }
    true
}

#[allow(
    unsafe_code,
    reason = "a shared block at a fixed address is reached from every context"
)]
// SAFETY: a `Block<Fixed<..>, _>` holds no memory: its only state is the
// const address. Its accesses are volatile loads and stores of device
// registers, which are not Rust memory, so accesses from several contexts
// are no data race; `R: SharedRegisters` limits the `&self` writes to
// whole-word stores that read nothing back. Host blocks stay `!Sync`, since
// their words are ordinary memory.
unsafe impl<const BASE: usize, R: SharedRegisters> Sync for Block<Fixed<BASE>, R> {}

#[allow(
    unsafe_code,
    reason = "the volatile whole-word store of a shared block"
)]
impl<P: Placement, R: SharedRegisters> Block<P, R> {
    /// One volatile store of the whole word at byte `OFFSET`, through a
    /// shared borrow: the write of a shared block, which reads nothing back.
    #[inline(always)]
    pub fn write_shared<const OFFSET: usize>(&self, value: u32) {
        const { assert!(listed(R::WORDS, OFFSET), "the block lists the word") };
        // SAFETY: as in `read`. A device word is not Rust memory; a host
        // block is `!Sync` and borrows its memory mutably, so this store
        // through the raw word pointer is the only access while it runs.
        unsafe { self.place.word(OFFSET).write_volatile(value) }
    }
}

impl<const BASE: usize, R: Registers> Block<Fixed<BASE>, R> {
    /// The block again, from the two parts of one split.
    ///
    /// The parts are of the one split there is: device-register blocks come
    /// only from claims of disjoint, non-empty word lists, so at most one
    /// block of this type exists at a time, and its split made the only
    /// parts of this type.
    ///
    /// Together they own every word of the block, checked at compile time, so
    /// no third part of a division is still out.
    pub fn join<A: Words, B: Words>(
        first: Part<Fixed<BASE>, R, A>,
        second: Part<Fixed<BASE>, R, B>,
    ) -> Self {
        const {
            assert!(
                owners_cover(R::WORDS, A::OWNED, B::OWNED),
                "the joined parts own every word of the block"
            );
        };
        let Part { place, .. } = first;
        drop(second);
        Self {
            place,
            registers: PhantomData,
        }
    }
}

impl<'a, R: Registers> Block<Host<'a>, R> {
    /// The block again, from the two parts of one split.
    ///
    /// Blocks over different host memory share a type, so the parts are
    /// checked to lie over the same memory; parts of two blocks are refused
    /// and returned. Together they own every word of the block, checked at
    /// compile time.
    #[allow(
        clippy::type_complexity,
        reason = "the refusal returns both parts unchanged"
    )]
    pub fn try_join<A: Words, B: Words>(
        first: Part<Host<'a>, R, A>,
        second: Part<Host<'a>, R, B>,
    ) -> Result<Self, NotOneSplit<Part<Host<'a>, R, A>, Part<Host<'a>, R, B>>> {
        const {
            assert!(
                owners_cover(R::WORDS, A::OWNED, B::OWNED),
                "the joined parts own every word of the block"
            );
        };
        // Each host block borrows its memory mutably, so two blocks with
        // words never share a base: equal bases mean the parts of one split.
        if first.place.base != second.place.base {
            return Err(NotOneSplit { first, second });
        }
        let Part { place, .. } = first;
        drop(second);
        Ok(Self {
            place,
            registers: PhantomData,
        })
    }
}

/// Two parts given to a join that come from different splits; both are
/// returned unchanged.
pub struct NotOneSplit<F, S> {
    /// The first part given.
    pub first: F,
    /// The second part given.
    pub second: S,
}

impl<F, S> core::fmt::Debug for NotOneSplit<F, S> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("NotOneSplit")
    }
}

/// The words of a block one [`Part`] owns (reads and writes) and observes
/// (reads only), as byte offsets; an array lists every element.
pub trait Words {
    /// The words the part reads and writes; no other part owns them.
    const OWNED: &'static [usize];
    /// The words the part only reads.
    const OBSERVED: &'static [usize];
    /// The write-trigger words the part shares with the other parts of its
    /// division, each with the bits only this part writes: a write-only
    /// word whose written one starts an action and whose written zero does
    /// nothing, so each part stores its own bits with zero in every other.
    const TRIGGERS: &'static [(usize, u32)] = &[];
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

const fn all_listed(mut list: &[usize], words: &[usize]) -> bool {
    while let [word, rest @ ..] = list {
        if !listed(words, *word) {
            return false;
        }
        list = rest;
    }
    true
}

/// One part of a split [`Block`]: it writes only the words `W` owns and
/// reads those and the words `W` observes.
#[must_use = "a part must be joined with its sibling to recover the block"]
pub struct Part<P: Placement, R: Registers, W: Words> {
    place: P,
    words: PhantomData<fn() -> (R, W)>,
}

#[allow(unsafe_code, reason = "the volatile accesses of a part's listed words")]
impl<P: Placement, R: Registers, W: Words> Part<P, R, W> {
    /// One volatile read of a word the part owns or observes.
    #[inline(always)]
    pub fn read<const OFFSET: usize>(&self) -> u32 {
        const {
            assert!(
                contains(W::OWNED, OFFSET) || contains(W::OBSERVED, OFFSET),
                "a part reads only the words it owns or observes"
            );
        };
        // SAFETY: the split checked that the block lists every word of the
        // part, whose authority the part shares only for the words it lists.
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
        // SAFETY: every element is a listed word of the block (checked above
        // and by the split) and `index < COUNT` by `Index`.
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

    /// One volatile store of the part's bits of the shared write-trigger
    /// word at `OFFSET`: `value` in them, zero in every other bit, which
    /// the other parts own and a written zero leaves alone.
    #[inline(always)]
    pub fn write_trigger<const OFFSET: usize>(&mut self, value: u32) {
        let mask = const {
            let mask = trigger_mask(W::TRIGGERS, OFFSET);
            assert!(mask != 0, "a part writes only its own trigger words");
            mask
        };
        // SAFETY: the division checked that the block lists the word, that
        // no part owns it and that no other part has these bits; a device
        // word is not Rust memory, so the other parts' stores of their bits
        // are no data race, and a host part is `!Send`, so they run on this
        // thread one at a time.
        unsafe { self.place.word(OFFSET).write_volatile(value & mask) }
    }

    /// The part as a [`Reader`] of the words it owns and observes, giving
    /// up every write: no word the part owns is written again.
    pub fn into_reader(self) -> Reader<P, R, W> {
        Reader {
            place: self.place.reading(),
            words: PhantomData,
        }
    }
}

/// Read-only authority over the words `W` owns and observes of one block,
/// made only by giving up a [`Block`]'s or a [`Part`]'s writes
/// ([`Block::into_reader`], [`Part::into_reader`]).
///
/// It is `Copy`: every copy only reads, and no owner writes the words it
/// took over. Over device registers it is `Send` and `Sync`; over host
/// memory neither. It has no write:
///
/// ```compile_fail,E0599
/// struct Words;
/// impl oer_mmio::Registers for Words {
///     const WORDS: &'static [usize] = &[0];
/// }
/// impl oer_mmio::Words for Words {
///     const OWNED: &'static [usize] = &[0];
///     const OBSERVED: &'static [usize] = &[];
/// }
/// fn give_up(block: oer_mmio::Block<oer_mmio::Fixed<0x1000>, Words>) {
///     let mut reader = block.into_reader::<Words>();
///     reader.write::<0>(1);
/// }
/// ```
pub struct Reader<P: Placement, R: Registers, W: Words> {
    place: P::Reading,
    words: PhantomData<fn() -> (R, W)>,
}

impl<P: Placement, R: Registers, W: Words> Clone for Reader<P, R, W> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<P: Placement, R: Registers, W: Words> Copy for Reader<P, R, W> {}

#[allow(unsafe_code, reason = "the volatile reads of a reader's listed words")]
impl<P: Placement, R: Registers, W: Words> Reader<P, R, W> {
    /// One volatile read of a word the reader lists.
    #[inline(always)]
    pub fn read<const OFFSET: usize>(&self) -> u32 {
        const {
            assert!(
                contains(W::OWNED, OFFSET) || contains(W::OBSERVED, OFFSET),
                "a reader reads only the words it lists"
            );
        };
        // SAFETY: the block or part the reader was made from lists the word
        // (checked when it was made) and no owner writes it any more but the
        // sibling that owns an observed word; a device word is not Rust
        // memory, and a host reader is `!Send` and borrows its memory for as
        // long as it lives, so no store races this read.
        unsafe { sealed::Reading::word(&self.place, OFFSET).read_volatile() }
    }

    /// One volatile read of an array element the reader lists.
    #[inline(always)]
    pub fn read_indexed<const OFFSET: usize, const STRIDE: usize, const COUNT: usize>(
        &self,
        index: Index<COUNT>,
    ) -> u32 {
        const {
            assert!(
                array_listed(W::OWNED, OFFSET, STRIDE, COUNT)
                    || array_listed(W::OBSERVED, OFFSET, STRIDE, COUNT),
                "a reader reads only the arrays it lists"
            );
        };
        // SAFETY: as in `read`; `index < COUNT` by `Index`.
        unsafe { sealed::Reading::word(&self.place, OFFSET + STRIDE * index.get()).read_volatile() }
    }
}

/// The compile-time checks the rendered register code asserts in `const`
/// items: `const _: () = assert!(check::within(..))`.
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

    /// Whether `first` and `second` together own every word of `block`,
    /// each word exactly once, and nothing else: the two sides of a block.
    pub const fn sides_cover(block: &[usize], first: &[usize], second: &[usize]) -> bool {
        if block.len() != first.len() + second.len() {
            return false;
        }
        let mut words = block;
        while let [word, rest @ ..] = words {
            if super::contains(first, *word) == super::contains(second, *word) {
                return false;
            }
            words = rest;
        }
        true
    }

    /// Whether no two of `blocks` (each a base address and its ascending
    /// word offsets) list the same absolute word.
    pub const fn disjoint_blocks(blocks: &[(usize, &[usize])]) -> bool {
        let mut rest = blocks;
        while let [first, others @ ..] = rest {
            let mut tail = others;
            while let [second, more @ ..] = tail {
                if !disjoint_words(*first, *second) {
                    return false;
                }
                tail = more;
            }
            rest = others;
        }
        true
    }

    /// Whether two blocks' ascending word lists share no absolute word.
    const fn disjoint_words(
        (first_base, first): (usize, &[usize]),
        (second_base, second): (usize, &[usize]),
    ) -> bool {
        let (mut left, mut right) = (first, second);
        while let ([a, left_rest @ ..], [b, right_rest @ ..]) = (left, right) {
            let (a, b) = (first_base + *a, second_base + *b);
            if a == b {
                return false;
            }
            if a < b {
                left = left_rest;
            } else {
                right = right_rest;
            }
        }
        true
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

/// A command start refused, writing nothing: a self-clearing bit of the
/// command register still reads one, and a start would write zero over it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommandBusy {
    /// The register's self-clearing bits that read one: what still runs.
    pub running: u32,
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

    /// The low `WIDTH` bits of `value`: a field read back from its register.
    pub const fn truncate(value: u32) -> Self {
        Self(value & Self::MAX)
    }

    /// The value.
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl<const WIDTH: u32> From<Bits<WIDTH>> for u32 {
    fn from(bits: Bits<WIDTH>) -> Self {
        bits.0
    }
}

/// A word in memory the hardware may change: volatile access only.
///
/// It is not `Sync`; its zero bytes are a valid value.
#[repr(transparent)]
#[derive(bytemuck::Zeroable)]
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
