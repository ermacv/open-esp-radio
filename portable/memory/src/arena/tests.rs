extern crate std;

use std::vec::Vec;

use super::*;
use crate::zeroed::ZeroedStatic;

struct Window;

impl DmaWindow for Window {
    const RANGE: core::ops::Range<u32> = 0x4000_0000..0x4010_0000;
}

/// The tests' placement: a zeroed static at a model address.
struct HostStatic<T, const AT: u32>(ZeroedStatic<T>);

#[allow(unsafe_code, reason = "the tests' placement")]
// SAFETY: the value is the static's own, claimed once by `try_take`; the
// model address is followed by no hardware.
unsafe impl<T: DmaLayout, const AT: u32> PlacedStatic<T> for HostStatic<T, AT> {
    #[allow(clippy::mut_from_ref, reason = "the claim-once static's claim")]
    fn claim(&'static self) -> Option<(&'static mut T, DmaRef<T>)> {
        Some((self.0.try_take()?, DmaRef::model(AT)))
    }
}

macro_rules! rx_place {
    (static $name:ident: $ty:ty = zeroed in $section:literal;) => {
        static $name: HostStatic<$ty, RX_BASE> = HostStatic(ZeroedStatic::new());
    };
}

macro_rules! tx_place {
    (static $name:ident: $ty:ty = zeroed in $section:literal;) => {
        static $name: HostStatic<$ty, TX_BASE> = HostStatic(ZeroedStatic::new());
    };
}

type RxStatic<T> = HostStatic<T, RX_BASE>;
type TxStatic<T> = HostStatic<T, TX_BASE>;

macro_rules! floor_place {
    (static $name:ident: $ty:ty = zeroed in $section:literal;) => {
        static $name: HostStatic<$ty, FLOOR_BASE> = HostStatic(ZeroedStatic::new());
    };
}

type FloorStatic<T> = HostStatic<T, FLOOR_BASE>;
const FLOOR_BASE: u32 = 0x4000_3000;

macro_rules! inspect_place {
    (static $name:ident: $ty:ty = zeroed in $section:literal;) => {
        static $name: HostStatic<$ty, INSPECT_BASE> = HostStatic(ZeroedStatic::new());
    };
}

type InspectStatic<T> = HostStatic<T, INSPECT_BASE>;
const INSPECT_BASE: u32 = 0x4000_4000;

/// The tests' brands. In this crate the macro's own expansion is linted;
/// in another crate it is not (`tests/ui/arena_under_forbid.rs`).
#[allow(unsafe_code, reason = "this crate's own expansion of `arena!`")]
mod brands {
    use super::{
        FLOOR_BASE, FloorStatic, HostStatic, INSPECT_BASE, InspectStatic, RX_BASE, RxStatic,
        TX_BASE, TxStatic, Window, ZeroedStatic,
    };

    crate::arena! {
        pub(super) Rx: window Window, count 40, size 64, floor 8,
            placed by rx_place as RxStatic in ".dma.bss.test_rx";
    }

    crate::arena! {
        pub(super) Tx: window Window, count 3, size 16, floor 0,
            placed by tx_place as TxStatic in ".dma.bss.test_tx";
    }

    crate::arena! {
        pub(super) Fl: window Window, count 3, size 32, floor 16,
            placed by floor_place as FloorStatic in ".dma.bss.test_floor";
    }

    crate::arena! {
        pub(super) In: window Window, count 3, size 16, floor 0,
            placed by inspect_place as InspectStatic in ".dma.bss.test_inspect";
    }
}
use brands::{Fl, In, Rx, Tx};

const RX_BASE: u32 = 0x4000_1000;
const TX_BASE: u32 = 0x4000_2000;

fn release<B: Brand>(lent: Lent<B>, start: usize, len: usize) -> Released<B> {
    #[allow(unsafe_code, reason = "the test is the hardware")]
    // SAFETY: no hardware touches the model's buffers.
    unsafe {
        Released::assume(lent, Span::new(start, len).expect("within the buffer"))
    }
}

// One test holds each place: a brand's place is taken once.
#[test]
fn custody_moves_from_the_cpu_to_the_hardware_and_back_by_a_release() {
    let (arena, fresh) = Arena::<Rx, { 40 * 64 }>::claim().expect("the first claim");
    assert!(
        Arena::<Rx, { 40 * 64 }>::claim().is_none(),
        "a place is claimed once"
    );
    let mut handles: Vec<_> = fresh.collect();
    assert_eq!(handles.len(), 40, "every buffer once");
    assert!(handles.iter().all(|handle| handle.span() == Span::FULL));
    assert_eq!((Span::<Rx>::FULL.start(), Span::<Rx>::FULL.len()), (8, 56));

    // Each handle's bytes are its own CPU region.
    for (value, handle) in handles.iter_mut().enumerate() {
        arena.bytes_mut(handle).fill(value as u8);
    }
    for (value, handle) in handles.iter().enumerate() {
        assert!(arena.bytes(handle).iter().all(|&byte| byte == value as u8));
    }

    // Lending gives the buffer's first byte, below the floor included.
    let third = handles.remove(3);
    let (lent, address) = arena.lend(third);
    assert_eq!(address.get(), RX_BASE + 3 * 64);

    // The hardware writes its metadata and a unit; the release names where
    // the unit begins and how long it is, and the handle gives exactly that.
    let place = arena.place.bytes.get().cast::<u8>();
    for offset in 0..64 {
        #[allow(unsafe_code, reason = "the test is the hardware")]
        // SAFETY: the buffer is lent, inside the place; the test is the
        // hardware.
        unsafe {
            place
                .add(3 * 64 + offset)
                .write(0xA0 | (offset as u8 & 0xF));
        }
    }
    let handle = arena.reclaim(release(lent, 12, 5));
    assert_eq!((handle.span().start(), handle.span().len()), (12, 5));
    assert_eq!(arena.bytes(&handle), &[0xAC, 0xAD, 0xAE, 0xAF, 0xA0]);
}

#[test]
fn a_brand_without_shared_bytes_gives_its_whole_buffer() {
    let (arena, fresh) = Arena::<Tx, { 3 * 16 }>::claim().expect("the first claim");
    let mut handles: Vec<_> = fresh.collect();
    assert_eq!(handles.len(), 3);
    let mut last = handles.pop().expect("three");
    assert_eq!(last.index, 2);
    assert_eq!(arena.bytes_mut(&mut last).len(), 16);
    arena.bytes_mut(&mut last)[3] = 0x5A;
    assert_eq!(arena.bytes(&last).get(3), Some(&0x5A));
    let (lent, address) = arena.lend(last);
    assert_eq!(address.get(), 0x4000_2000 + 2 * 16);
    let back = arena.reclaim(release(lent, 0, 16));
    assert_eq!(arena.bytes(&back).len(), 16);
}

macro_rules! geometry {
    ($name:ident: $count:expr, $size:expr, $floor:expr) => {
        enum $name {}
        #[allow(unsafe_code, reason = "geometry-only brands that never make an arena")]
        // SAFETY: the brand has no place (`()` claims nothing);
        // no arena is made of it.
        unsafe impl Brand for $name {
            type Window = Window;
            const COUNT: usize = $count;
            const SIZE: usize = $size;
            const FLOOR: usize = $floor;
            type Static = ();
            fn place() -> &'static () {
                &()
            }
        }
    };
}

geometry!(Most: MAX_BUFFERS, 65_532, 0);
geometry!(OneWord: 3, 16, 0);
geometry!(TooMany: MAX_BUFFERS + 1, 4, 0);
geometry!(NoBuffer: 0, 4, 0);
geometry!(Highest: 1, 65_532, 65_531);
geometry!(TooWide: 1, 65_536, 0);
geometry!(OddSize: 1, 6, 0);
geometry!(NoBytes: 1, 8, 8);
geometry!(Overflowing: 2, usize::MAX, 0);

#[test]
fn the_geometry_is_refused_exactly_at_its_bounds() {
    assert_eq!(geometry_refusal::<Most, { MAX_BUFFERS * 65_532 }>(), None);
    assert_eq!(geometry_refusal::<Highest, 65_532>(), None);
    assert_eq!(geometry_refusal::<Rx, { 40 * 64 }>(), None);
    let buffers = Some("an arena has 1 to MAX_BUFFERS buffers");
    assert_eq!(
        geometry_refusal::<TooMany, { (MAX_BUFFERS + 1) * 4 }>(),
        buffers
    );
    assert_eq!(geometry_refusal::<NoBuffer, 0>(), buffers);
    let place = Some("an arena's place is COUNT buffers of SIZE bytes");
    assert_eq!(geometry_refusal::<Rx, { 40 * 64 + 4 }>(), place);
    assert_eq!(geometry_refusal::<Overflowing, 0>(), place);
    let buffer = Some("a buffer is at most 65 535 bytes, in words");
    assert_eq!(geometry_refusal::<TooWide, 65_536>(), buffer);
    assert_eq!(geometry_refusal::<OddSize, 6>(), buffer);
    assert_eq!(
        geometry_refusal::<NoBytes, 8>(),
        Some("a buffer has bytes above its floor")
    );
}

#[test]
fn a_span_lies_between_the_floor_and_the_buffer_end() {
    // Rx: floor 8, size 64.
    assert_eq!(Span::<Rx>::new(8, 56), Some(Span::FULL));
    assert_eq!(
        Span::<Rx>::new(64, 0).map(|span| (span.start(), span.len())),
        Some((64, 0))
    );
    assert!(Span::<Rx>::new(64, 0).is_some_and(Span::is_empty));
    assert!(!Span::<Rx>::FULL.is_empty());
    assert_eq!(Span::<Rx>::new(7, 1), None, "below the floor");
    assert_eq!(Span::<Rx>::new(8, 57), None, "past the buffer");
    assert_eq!(Span::<Rx>::new(60, 5), None, "past the buffer");
    assert_eq!(Span::<Rx>::new(usize::MAX, 1), None, "an overflowing end");
    assert_eq!(
        Span::<Rx>::fixed::<20, 30>(),
        Span::new(20, 30).expect("fits")
    );
    assert_eq!(Span::<Tx>::new(0, 16), Some(Span::FULL));
    assert_ne!(Span::<Rx>::new(8, 1), Span::new(8, 2), "another end");
    assert_ne!(Span::<Rx>::new(8, 1), Span::new(9, 1), "another start");
    assert_eq!(
        std::format!("{:?}", Span::<OneWord>::fixed::<4, 8>()),
        "Span(4, 12)"
    );
}

// The floor brand's own place.
#[test]
fn a_held_buffer_s_floor_copies_out_exactly_its_bytes_below_the_floor() {
    let (arena, fresh) = Arena::<Fl, { 3 * 32 }>::claim().expect("the first claim");
    let mut handles: Vec<_> = fresh.collect();
    // The hardware's header in each buffer: buffer `b`'s byte `k` is
    // `b * 64 + k`, below the floor and above it.
    let place = arena.place.bytes.get().cast::<u8>();
    for offset in 0..3 * 32 {
        #[allow(unsafe_code, reason = "the test is the hardware")]
        // SAFETY: inside the place; no slice of these bytes is live.
        unsafe {
            place
                .add(offset)
                .write((offset / 32 * 64 + offset % 32) as u8);
        }
    }
    let middle = handles.remove(1);
    let copy: [u8; 16] = arena.floor(&middle).copy();
    assert_eq!(
        copy,
        core::array::from_fn(|k| (64 + k) as u8),
        "from byte 0, all 16"
    );
    // The handle's own bytes still begin at the floor.
    assert_eq!(arena.bytes(&middle).first(), Some(&(64 + 16)));
    let last = handles.pop().expect("the third");
    let copy: [u8; 16] = arena.floor(&last).copy();
    assert_eq!(copy, core::array::from_fn(|k| (128 + k) as u8));
}

// The `In` brand's place: a borrowed release over a buffer the test lends
// and, as the hardware, writes.
#[test]
fn a_borrowed_release_reads_the_lent_buffer_s_bytes_in_place() {
    let (arena, fresh) = Arena::<In, { 3 * 16 }>::claim().expect("the first claim");
    let mut handles: Vec<_> = fresh.collect();
    let second = handles.remove(1);
    let (lent, _) = arena.lend(second);
    let place = arena.place.bytes.get().cast::<u8>();
    for offset in 0..16 {
        #[allow(unsafe_code, reason = "the test is the hardware")]
        // SAFETY: the buffer is lent, inside the place; the test is the
        // hardware.
        unsafe {
            place.add(16 + offset).write(offset as u8 + 1);
        }
    }
    #[allow(unsafe_code, reason = "the test is the hardware, which is done")]
    // SAFETY: no hardware touches the model's buffers.
    let quiet = unsafe { Quiet::assume(&lent, Span::new(4, 8).expect("within the buffer")) };
    let bytes = arena.inspect(&quiet, |handle| arena.bytes(handle).to_vec());
    assert_eq!(
        bytes,
        (5..13).collect::<Vec<u8>>(),
        "the lent buffer's own bytes, its span"
    );
    drop(quiet);
    // The buffer is still lent: its release gives it back.
    let handle = arena.reclaim(release(lent, 0, 16));
    assert_eq!(arena.bytes(&handle), &(1..17).collect::<Vec<u8>>()[..]);
}
