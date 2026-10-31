#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
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

#![no_std]
#![cfg_attr(
    test,
    allow(
        clippy::arithmetic_side_effects,
        clippy::disallowed_macros,
        clippy::disallowed_methods
    )
)]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

#[cfg(test)]
extern crate std;

use core::{cell::Cell, marker::PhantomData, ptr::NonNull};

mod sealed {
    pub trait Sharing {}

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

/// A checked word, used only for the duration of one access. Every caller
/// proves its offset against its authority in an inline const block; array
/// indices are bounded by Index. Placements are sealed and originate in the
/// fixed-register claim or an exclusive, extent-checked host borrow.
struct DeviceWord(*mut u32);

#[allow(
    unsafe_code,
    reason = "the two volatile operations on checked register words"
)]
impl DeviceWord {
    #[inline(always)]
    fn load(self) -> u32 {
        // SAFETY: the private callers check this aligned word belongs to
        // their Block, Part or Reader. Fixed words satisfy assume_owned's
        // device-register promise. Host words stay within their live borrow;
        // host authorities are !Send/!Sync, so no CPU access races this load.
        unsafe { self.0.read_volatile() }
    }

    #[inline(always)]
    fn store(self, value: u32) {
        // SAFETY: only Block and Part writers call this, for words or trigger
        // bits they own. Shared fixed blocks only store whole device words;
        // device accesses are outside Rust memory. Host writers are confined
        // to one thread and retain their exclusive, extent-checked borrow.
        unsafe { self.0.write_volatile(value) }
    }
}

/// Where a register block lives: device registers at a fixed address
/// ([`Fixed`]) or host memory ([`Host`]). Only this crate implements it.
pub trait Placement: sealed::Sealed {}

/// Device registers at the fixed address `BASE`.
///
/// It is made only by [`assume_owned`], so its existence is the promise that
/// the block's words are its alone. It is `Send` (an owner moves between
/// execution contexts). A block or part carries its sharing restriction.
pub struct Fixed<const BASE: usize> {
    _owner: PhantomData<()>,
}

impl<const BASE: usize> sealed::Sealed for Fixed<BASE> {
    type Reading = FixedReading<BASE>;

    #[inline(always)]
    fn word(&self, offset: usize) -> *mut u32 {
        core::ptr::with_exposed_provenance_mut(BASE.wrapping_add(offset))
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
        core::ptr::with_exposed_provenance(BASE.wrapping_add(offset))
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
    /// Whether the policy permits shared access to this block.
    type Sharing: Sharing;
    /// The block's words.
    const WORDS: &'static [usize];
}

/// The sealed sharing policy of a register block.
pub trait Sharing: sealed::Sharing {}

/// Sharing policies of generated register-word lists.
pub mod sharing {
    use core::{cell::Cell, marker::PhantomData};

    /// One context at a time: the block is not `Sync`.
    pub struct Owned(PhantomData<Cell<()>>);
    /// Whole-word stores and side-effect-free reads may be shared.
    pub struct Shared;

    impl super::sealed::Sharing for Owned {}
    impl super::sealed::Sharing for Shared {}
    impl super::Sharing for Owned {}
    impl super::Sharing for Shared {}
}

/// The words of a block that every execution context may reach at once:
/// each write it makes is a whole-word store that reads nothing back (a
/// write-trigger strobe, or a one-field register stored whole), and each
/// read has no side effect. The register generator selects `sharing::Shared`
/// only for a policy's shared block, whose registers it checks are of these
/// kinds; the marker grants this trait.
///
/// Only such a block is `Sync`; an ordinary block at a fixed address is not:
///
/// ```compile_fail,E0277
/// struct Words;
/// impl oer_mmio::Registers for Words {
///     type Sharing = oer_mmio::sharing::Owned;
///     const WORDS: &'static [usize] = &[0];
/// }
/// fn sync<T: Sync>() {}
/// sync::<oer_mmio::Block<oer_mmio::Fixed<0x1000>, Words>>();
/// ```
pub trait SharedRegisters: Registers<Sharing = sharing::Shared> {}

impl<R: Registers<Sharing = sharing::Shared>> SharedRegisters for R {}

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
            if !(BASE.is_multiple_of(4) && !R::WORDS.is_empty() && ascending(R::WORDS)) {
                panic!("a claimed block lists word-aligned words, strictly ascending");
            };
            if !(match end(R::WORDS) {
                Some(end) => BASE.checked_add(end).is_some(),
                None => false,
            }) {
                panic!("a claimed block lies inside the address space");
            };
        };
        Block {
            place: Fixed {
                _owner: PhantomData,
            },
            registers: PhantomData,
            sharing: PhantomData,
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
///     type Sharing = oer_mmio::sharing::Owned;
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
/// A hart-local word (each hart reaches its own copy at one address) is
/// listed once per hart, and the block listing hart `h`'s copy is accessed
/// only on hart `h`.
#[allow(unsafe_code, reason = "the caller promises exclusive registers")]
pub unsafe fn assume_owned<C: Claim>() -> C {
    C::claimed(&sealed::Promise(()))
}

/// The exclusive authority over the words `R` lists of one register block.
///
/// Its reads and writes take the word's byte offset as a const generic;
/// an offset `R` does not list fails to compile.
#[must_use = "dropping a register block permanently loses its register authority"]
pub struct Block<P: Placement, R: Registers> {
    place: P,
    registers: PhantomData<fn() -> R>,
    sharing: PhantomData<R::Sharing>,
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
            if !(ascending(R::WORDS)) {
                panic!("a block lists word-aligned words, strictly ascending");
            };
            match end(R::WORDS) {
                Some(end) => end,
                None => panic!("a block's words lie inside the address space"),
            }
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
            sharing: PhantomData,
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
const fn end(list: &[usize]) -> Option<usize> {
    match list.last() {
        Some(last) => last.checked_add(4),
        None => Some(0),
    }
}

/// Whether the ascending `list` holds `word`.
const fn listed(mut list: &[usize], word: usize) -> bool {
    while !list.is_empty() {
        let Some((low, high)) = list.split_at_checked(list.len() / 2) else {
            return false;
        };
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
        element = element.wrapping_add(1);
    }
    true
}

impl<P: Placement, R: Registers> Block<P, R> {
    /// One volatile read of the word at byte `OFFSET`.
    #[inline(always)]
    pub fn read<const OFFSET: usize>(&self) -> u32 {
        const {
            if !(listed(R::WORDS, OFFSET)) {
                panic!("the block lists the word");
            }
        };
        DeviceWord(self.place.word(OFFSET)).load()
    }

    /// One volatile write of the whole word at byte `OFFSET`.
    #[inline(always)]
    pub fn write<const OFFSET: usize>(&mut self, value: u32) {
        const {
            if !(listed(R::WORDS, OFFSET)) {
                panic!("the block lists the word");
            }
        };
        DeviceWord(self.place.word(OFFSET)).store(value)
    }

    /// One volatile read of element `index` of the word array at `OFFSET`.
    #[inline(always)]
    pub fn read_indexed<const OFFSET: usize, const STRIDE: usize, const COUNT: usize>(
        &self,
        index: Index<COUNT>,
    ) -> u32 {
        const {
            if !(array_listed(R::WORDS, OFFSET, STRIDE, COUNT)) {
                panic!("the block lists every element");
            };
        };
        DeviceWord(
            self.place
                .word(OFFSET.wrapping_add(STRIDE.wrapping_mul(index.get()))),
        )
        .load()
    }

    /// One volatile write of element `index` of the word array at `OFFSET`.
    #[inline(always)]
    pub fn write_indexed<const OFFSET: usize, const STRIDE: usize, const COUNT: usize>(
        &mut self,
        index: Index<COUNT>,
        value: u32,
    ) {
        const {
            if !(array_listed(R::WORDS, OFFSET, STRIDE, COUNT)) {
                panic!("the block lists every element");
            };
        };
        DeviceWord(
            self.place
                .word(OFFSET.wrapping_add(STRIDE.wrapping_mul(index.get()))),
        )
        .store(value)
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
            if !(parts_listed(R::WORDS, D::WORDS)) {
                panic!("the block lists every word of its parts");
            };
            if !(owned_disjoint(D::WORDS)) {
                panic!("the parts of a block own disjoint words");
            };
            if !(triggers_disjoint(D::WORDS)) {
                panic!(
                    "a shared trigger word is owned by no part and its parts' bits are disjoint"
                );
            };
        };
        D::parts(self.place)
    }

    /// The block as a [`Reader`] of the words `W` lists, giving up every
    /// write: no word the block lists is written again.
    pub fn into_reader<W: Words>(self) -> Reader<P, R, W> {
        const {
            if !(all_listed(W::OWNED, R::WORDS) && all_listed(W::OBSERVED, R::WORDS)) {
                panic!("the block lists every word of its reader");
            };
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
                    owner: PhantomData,
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

impl<P: Placement, R: SharedRegisters> Block<P, R> {
    /// One volatile store of the whole word at byte `OFFSET`, through a
    /// shared borrow: the write of a shared block, which reads nothing back.
    #[inline(always)]
    pub fn write_shared<const OFFSET: usize>(&self, value: u32) {
        const {
            if !(listed(R::WORDS, OFFSET)) {
                panic!("the block lists the word");
            }
        };
        DeviceWord(self.place.word(OFFSET)).store(value)
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
            if !(owners_cover(R::WORDS, A::OWNED, B::OWNED)) {
                panic!("the joined parts own every word of the block");
            };
        };
        let Part { place, .. } = first;
        drop(second);
        Self {
            place,
            registers: PhantomData,
            sharing: PhantomData,
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
            if !(owners_cover(R::WORDS, A::OWNED, B::OWNED)) {
                panic!("the joined parts own every word of the block");
            };
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
            sharing: PhantomData,
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
    owner: PhantomData<Cell<()>>,
}

impl<P: Placement, R: Registers, W: Words> Part<P, R, W> {
    /// One volatile read of a word the part owns or observes.
    #[inline(always)]
    pub fn read<const OFFSET: usize>(&self) -> u32 {
        const {
            if !(contains(W::OWNED, OFFSET) || contains(W::OBSERVED, OFFSET)) {
                panic!("a part reads only the words it owns or observes");
            };
        };
        DeviceWord(self.place.word(OFFSET)).load()
    }

    /// One volatile write of a word the part owns.
    #[inline(always)]
    pub fn write<const OFFSET: usize>(&mut self, value: u32) {
        const {
            if !(contains(W::OWNED, OFFSET)) {
                panic!("a part writes only the words it owns");
            };
        };
        DeviceWord(self.place.word(OFFSET)).store(value)
    }

    /// One volatile read of an array element the part owns or observes.
    #[inline(always)]
    pub fn read_indexed<const OFFSET: usize, const STRIDE: usize, const COUNT: usize>(
        &self,
        index: Index<COUNT>,
    ) -> u32 {
        const {
            if !(array_listed(W::OWNED, OFFSET, STRIDE, COUNT)
                || array_listed(W::OBSERVED, OFFSET, STRIDE, COUNT))
            {
                panic!("a part reads only the arrays it owns or observes");
            };
        };
        DeviceWord(
            self.place
                .word(OFFSET.wrapping_add(STRIDE.wrapping_mul(index.get()))),
        )
        .load()
    }

    /// One volatile write of an array element the part owns.
    #[inline(always)]
    pub fn write_indexed<const OFFSET: usize, const STRIDE: usize, const COUNT: usize>(
        &mut self,
        index: Index<COUNT>,
        value: u32,
    ) {
        const {
            if !(array_listed(W::OWNED, OFFSET, STRIDE, COUNT)) {
                panic!("a part writes only the arrays it owns");
            };
        };
        DeviceWord(
            self.place
                .word(OFFSET.wrapping_add(STRIDE.wrapping_mul(index.get()))),
        )
        .store(value)
    }

    /// One volatile store of the part's bits of the shared write-trigger
    /// word at `OFFSET`: `value` in them, zero in every other bit, which
    /// the other parts own and a written zero leaves alone.
    #[inline(always)]
    pub fn write_trigger<const OFFSET: usize>(&mut self, value: u32) {
        let mask = const {
            let mask = trigger_mask(W::TRIGGERS, OFFSET);
            if !(mask != 0) {
                panic!("a part writes only its own trigger words");
            };
            mask
        };
        DeviceWord(self.place.word(OFFSET)).store(value & mask)
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
///     type Sharing = oer_mmio::sharing::Owned;
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

impl<P: Placement, R: Registers, W: Words> Reader<P, R, W> {
    /// One volatile read of a word the reader lists.
    #[inline(always)]
    pub fn read<const OFFSET: usize>(&self) -> u32 {
        const {
            if !(contains(W::OWNED, OFFSET) || contains(W::OBSERVED, OFFSET)) {
                panic!("a reader reads only the words it lists");
            };
        };
        DeviceWord(sealed::Reading::word(&self.place, OFFSET).cast_mut()).load()
    }

    /// One volatile read of an array element the reader lists.
    #[inline(always)]
    pub fn read_indexed<const OFFSET: usize, const STRIDE: usize, const COUNT: usize>(
        &self,
        index: Index<COUNT>,
    ) -> u32 {
        const {
            if !(array_listed(W::OWNED, OFFSET, STRIDE, COUNT)
                || array_listed(W::OBSERVED, OFFSET, STRIDE, COUNT))
            {
                panic!("a reader reads only the arrays it lists");
            };
        };
        DeviceWord(
            sealed::Reading::word(
                &self.place,
                OFFSET.wrapping_add(STRIDE.wrapping_mul(index.get())),
            )
            .cast_mut(),
        )
        .load()
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
        match first.len().checked_add(second.len()) {
            Some(count) if count == block.len() => {}
            _ => return false,
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
            let (Some(a), Some(b)) = (first_base.checked_add(*a), second_base.checked_add(*b))
            else {
                return false;
            };
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
        const {
            if !(I < COUNT) {
                panic!("the index lies inside the array");
            }
        };
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
    pub const MAX: u32 = const {
        if !(WIDTH >= 1 && WIDTH <= 32) {
            panic!("a field is 1 to 32 bits wide");
        };
        u32::MAX.wrapping_shr(32_u32.wrapping_sub(WIDTH))
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
        const {
            if !(V <= Self::MAX) {
                panic!("the value fits the field");
            }
        };
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

#[cfg(test)]
mod tests;
