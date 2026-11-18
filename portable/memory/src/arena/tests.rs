extern crate std;

use std::vec::Vec;

use super::*;
use crate::zeroed::ZeroedStatic;

struct Window;

impl DmaWindow for Window {
    const RANGE: core::ops::Range<u32> = 0x4000_0000..0x4010_0000;
}

macro_rules! host_place {
    (static $name:ident: $ty:ty = zeroed in $section:literal;) => {
        static $name: ZeroedStatic<$ty> = ZeroedStatic::new();
    };
}

crate::arena! {
    Rx: window Window, count 40, size 64, front 8, back 4,
        placed by host_place as ZeroedStatic in ".dma.bss.test_rx";
}

crate::arena! {
    Tx: window Window, count 3, size 16, front 0, back 0,
        placed by host_place as ZeroedStatic in ".dma.bss.test_tx";
}

const RX_BASE: u32 = 0x4000_1000;

struct Counted;

static NOTIFIED: AtomicU32 = AtomicU32::new(0);

impl ReturnNotify for Counted {
    fn notify() {
        NOTIFIED.fetch_add(1, Ordering::Relaxed);
    }
}

fn model_ref<L: DmaLayout>(address: u32) -> DmaRef<L> {
    DmaRef::model(address)
}

fn release<B: Brand>(lent: Lent<B>, extent: usize) -> Released<B> {
    #[allow(unsafe_code, reason = "the test is the hardware")]
    // SAFETY: no hardware touches the model's buffers.
    unsafe {
        Released::assume(lent, Extent::new(extent).expect("within the region"))
    }
}

// One test holds each place: a brand's place is taken once.
#[test]
fn custody_moves_from_the_cpu_to_the_hardware_and_back_by_a_release() {
    let (arena, fresh) = Arena::<Rx, { 40 * 64 }>::new(Rx::place().take(), model_ref(RX_BASE));
    let mut handles: Vec<_> = fresh.collect();
    assert_eq!(handles.len(), 40, "every buffer once");
    assert!(handles.iter().all(|handle| handle.extent() == 64 - 8 - 4));

    // Each handle's bytes are its own CPU region.
    for (value, handle) in handles.iter_mut().enumerate() {
        arena.bytes_mut(handle).fill(value as u8);
    }
    for (value, handle) in handles.iter().enumerate() {
        assert!(arena.bytes(handle).iter().all(|&byte| byte == value as u8));
    }

    // Lending gives the buffer's first byte, its front included.
    let third = handles.remove(3);
    let (lent, address) = arena.lend(third);
    assert_eq!(address.get(), RX_BASE + 3 * 64);

    // The hardware writes the front and a frame; the release names the
    // frame's extent and the handle gives exactly that.
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
    let handle = arena.reclaim(release(lent, 5));
    assert_eq!(handle.extent(), 5);
    assert_eq!(arena.bytes(&handle), &[0xA8, 0xA9, 0xAA, 0xAB, 0xAC]);
    assert!(Extent::<Rx>::new(64 - 8 - 4 + 1).is_none());
    assert_eq!(Extent::<Rx>::new(52).map(Extent::get), Some(52));
    assert_eq!(Extent::<Rx>::FULL.get(), 52);

    // Returning: each drop sets its bit and notifies, on another thread; the
    // take gives each handle once, whole, and then nothing.
    let before = NOTIFIED.load(Ordering::Relaxed);
    let units: Vec<_> = handles
        .drain(..)
        .chain([handle])
        .map(Returning::<Rx, Counted>::new)
        .collect();
    std::thread::spawn(move || drop(units))
        .join()
        .expect("the returner");
    assert_eq!(NOTIFIED.load(Ordering::Relaxed) - before, 40);
    let mut indices: Vec<_> = arena
        .returned()
        .map(|handle| {
            assert_eq!(handle.extent(), 52);
            handle.index
        })
        .collect();
    indices.sort_unstable();
    assert_eq!(indices, (0..40).collect::<Vec<u16>>());
    assert_eq!(arena.returned().count(), 0);

    // A unit's handle gives its bytes while it is held, and its drop
    // returns exactly it.
    let mut unit = Returning::<Rx, Counted>::new(Handle::full(39));
    arena.bytes_mut(unit.handle_mut()).fill(7);
    assert!(arena.bytes(unit.handle()).iter().all(|&byte| byte == 7));
    drop(unit);
    let again: Vec<_> = arena.returned().map(|handle| handle.index).collect();
    assert_eq!(again, [39]);
}

#[test]
fn a_brand_without_shared_bytes_gives_its_whole_buffer() {
    let (arena, fresh) = Arena::<Tx, { 3 * 16 }>::new(Tx::place().take(), model_ref(0x4000_2000));
    let mut handles: Vec<_> = fresh.collect();
    assert_eq!(handles.len(), 3);
    let mut last = handles.pop().expect("three");
    assert_eq!(last.index, 2);
    assert_eq!(arena.bytes_mut(&mut last).len(), 16);
    let short = last.with_extent(Extent::new(7).expect("fits"));
    assert_eq!(arena.bytes(&short).len(), 7);
    let (lent, address) = arena.lend(short);
    assert_eq!(address.get(), 0x4000_2000 + 2 * 16);
    let back = arena.reclaim(release(lent, 16));
    assert_eq!(arena.bytes(&back).len(), 16);
}

#[test]
fn a_returned_set_reaches_its_every_word() {
    let set = ReturnedSet::new();
    for word in 0..RETURNED_WORDS {
        set.word(word).store(1 << word, Ordering::Relaxed);
    }
    for word in 0..RETURNED_WORDS {
        assert_eq!(set.word(word).load(Ordering::Relaxed), 1 << word);
    }
}
