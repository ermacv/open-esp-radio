//! The receive ring over host memory: a model of the walker's registers,
//! the descriptors in a leaked table and the buffers in an arena whose
//! place the test, as the hardware, writes.

#![allow(clippy::unwrap_used, reason = "tests")]

extern crate std;

use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicPtr, Ordering};
use std::boxed::Box;
use std::collections::VecDeque;
use std::vec;
use std::vec::Vec;

use core::cell::Cell;
use oer_portable_memory::arena::{Arena, ArenaPlace, Brand, Handle, PlacedStatic};
use oer_portable_memory::zeroed::ZeroedStatic;
use oer_portable_time::{Clock, Instant};
use oer_portable_time_virtual::VirtualClock;

use super::{Reload, RingAnomaly, RxDescriptors, RxRing, Unit, Walker};
use crate::layouts::{DmaRef, DmaWindow, WifiRxDmaDescriptor};
use crate::radio::ieee80211::dma::descriptor::rx_landed_word;
use crate::radio::ieee80211::dma::space::{HOST_ARENA_BASE, HostDma};
use crate::system::memory::dma_static::Placed;

/// The walker's registers as the ring uses them: what it was told, and the
/// cursor each read reports (the last one again when none is queued).
#[derive(Default)]
pub(crate) struct ModelWalker {
    base: Option<u32>,
    window: bool,
    enabled: bool,
    rings: usize,
    cursors: VecDeque<(u32, u32)>,
    cursor: (u32, u32),
    /// The walker never accepts the doorbell.
    stuck: bool,
}

impl Walker for ModelWalker {
    fn cursor(&mut self) -> (u32, u32) {
        if let Some(cursor) = self.cursors.pop_front() {
            self.cursor = cursor;
        }
        self.cursor
    }
    fn configure_window(&mut self) {
        self.window = true;
    }
    fn write_base(&mut self, head: DmaRef<WifiRxDmaDescriptor>) {
        self.base = Some(head.bits());
    }
    fn enable(&mut self) {
        self.enabled = true;
    }
    fn ring(&mut self) {
        self.rings += 1;
    }
    fn reload(&mut self) -> Reload {
        if self.stuck {
            Reload::Pending
        } else {
            Reload::Accepted
        }
    }
    fn fence(&mut self) {}
}

/// A host placement whose place the test writes as the hardware: its first
/// byte is published when the arena claims it.
pub(crate) struct HostPlace<T: 'static>(ZeroedStatic<T>, AtomicPtr<u8>);

impl<T> HostPlace<T> {
    fn first_byte(&'static self) -> *mut u8 {
        self.1.load(Ordering::Acquire)
    }
}

/// The arena's model address, apart from the table's.
const ARENA_BASE: u32 = HOST_ARENA_BASE + 0x1_0000;

#[allow(unsafe_code, reason = "the tests' placement")]
// SAFETY: the value is the static's own, claimed once by `try_take`; the
// model address is followed by no hardware.
unsafe impl<T: crate::layouts::DmaLayout> PlacedStatic<T> for HostPlace<T> {
    fn claim(&'static self) -> Option<(&'static mut T, DmaRef<T>)> {
        let value = self.0.try_take()?;
        self.1
            .store(core::ptr::from_mut(value).cast(), Ordering::Release);
        Some((
            value,
            crate::system::memory::dma_address::model_reference(ARENA_BASE),
        ))
    }
}

macro_rules! host_place {
    (static $name:ident: $ty:ty = zeroed in $section:literal;) => {
        static $name: HostPlace<$ty> =
            HostPlace(ZeroedStatic::new(), AtomicPtr::new(core::ptr::null_mut()));
    };
}

const SIZE: usize = 128;
const RING: usize = 4;
const COUNT: usize = 8;
const BYTES: usize = COUNT * SIZE;

/// One test a brand: a brand's place is claimed once.
macro_rules! brand {
    ($name:ident) => {
        #[allow(unsafe_code, reason = "this crate's own expansion of `arena!`")]
        mod $name {
            use super::{AtomicPtr, HostPlace, ZeroedStatic};
            oer_portable_memory::arena! {
                pub(crate) Rx: window crate::layouts::DmaWindow, count 8, size 128, floor 64,
                    placed by host_place as HostPlace in ".dma.bss.receive_test";
            }
        }
    };
}

/// The ring over its own brand, built cold, with the spares the arena has
/// beyond the ring and R, and the first byte of the arena's place.
struct Bench<B: Brand<Window = DmaWindow> + 'static> {
    ring: &'static mut RxRing<B, BYTES, RING, ModelWalker>,
    spares: Vec<Handle<B>>,
    place: *mut u8,
}

fn bench<B: Brand<Window = DmaWindow, Static = HostPlace<ArenaPlace<B, BYTES>>>>() -> Bench<B> {
    let (arena, fresh) = Arena::<B, BYTES>::claim().unwrap();
    let mut fresh: Vec<_> = fresh.collect();
    let spares = fresh.split_off(RING + 1);
    let reserved = fresh.pop().unwrap();
    let ring: [Handle<B>; RING] = fresh.try_into().ok().unwrap();
    let table: &'static mut RxDescriptors<RING> = Box::leak(Box::new(bytemuck::Zeroable::zeroed()));
    let place: &'static mut MaybeUninit<RxRing<B, BYTES, RING, ModelWalker>> =
        Box::leak(Box::new(MaybeUninit::uninit()));
    let ring = RxRing::publish_cold::<HostDma>(
        place,
        Placed::host(table),
        ModelWalker::default(),
        arena,
        ring,
        reserved,
    )
    .ok()
    .unwrap();
    Bench {
        ring,
        spares,
        place: B::place().first_byte(),
    }
}

impl<B: Brand<Window = DmaWindow> + 'static> Bench<B> {
    /// Descriptor `index`'s DMA address (R's at `RING`).
    fn descriptor_address(&self, index: usize) -> u32 {
        self.ring.base + 12 * index as u32
    }

    /// The walker completes descriptor `index` with `length` bytes.
    fn complete(&self, index: usize, length: u32, terminal: bool) {
        self.ring.descriptors.ring[index].write_word0(rx_landed_word(
            SIZE as u32,
            length,
            terminal,
        ));
    }

    /// The walker writes `bytes` from the first byte of arena buffer `buffer`.
    fn write_buffer(&self, buffer: usize, bytes: &[u8]) {
        for (offset, byte) in bytes.iter().enumerate() {
            #[allow(unsafe_code, reason = "the test is the hardware")]
            // SAFETY: inside the place; no slice of these bytes is live.
            unsafe {
                self.place.add(buffer * SIZE + offset).write(*byte);
            }
        }
    }

    /// The walker reports LAST at descriptor `last` (or nothing) and NEXT at
    /// descriptor `next` (or zero).
    fn cursor(&mut self, last: Option<usize>, next: Option<usize>) {
        let low = |index: Option<usize>| index.map_or(0, |index| self.descriptor_address(index));
        let cursor = (low(last), low(next));
        self.ring.walker.cursors.push_back(cursor);
    }

    fn arm_word(&self) -> crate::layouts::WifiRxDmaDescriptorControl {
        RxRing::<B, BYTES, RING, ModelWalker>::ARMED
    }
}

/// A clock that moves 5 µs at every read: a settle on it ends.
struct Ticking(Cell<u64>);

impl Clock for Ticking {
    fn now(&self) -> Instant {
        let now = self.0.get() + 5;
        self.0.set(now);
        Instant::from_micros(now)
    }
}

brand!(cold);
#[test]
fn the_cold_publication_arms_and_links_the_ring_and_publishes_r_once() {
    let bench = bench::<cold::Rx>();
    let ring = &bench.ring;
    for index in 0..RING {
        let descriptor = &ring.descriptors.ring[index];
        assert_eq!(
            descriptor.word0(),
            bench.arm_word(),
            "descriptor {index} armed"
        );
        assert_eq!(
            descriptor.buffer_address(),
            ARENA_BASE + (index * SIZE) as u32
        );
        let next = if index + 1 < RING {
            bench.descriptor_address(index + 1)
        } else {
            0
        };
        assert_eq!(descriptor.next_address(), next, "descriptor {index}'s link");
    }
    assert_eq!(ring.descriptors.reserved.word0(), bench.arm_word());
    assert_eq!(
        ring.descriptors.reserved.next_address(),
        bench.descriptor_address(0)
    );
    assert_eq!(
        ring.walker.base,
        Some(bench.descriptor_address(RING)),
        "BASE = R"
    );
    assert!(ring.walker.window);
    assert!(!ring.walker.enabled, "the walker is started apart");
    assert_eq!(ring.walker.rings, 0);
}

brand!(single);
#[test]
fn a_single_unit_is_exchanged_for_a_spare_linked_behind_the_tail_and_rung_once() {
    let mut bench = bench::<single::Rx>();
    bench.ring.enable_walker();
    let frame: Vec<u8> = (0..50).collect();
    bench.write_buffer(0, &[0xaa; 64]);
    let mut unit = vec![0xaa; 64];
    unit.extend_from_slice(&frame);
    bench.write_buffer(0, &unit);
    bench.complete(0, 114, true);
    bench.cursor(Some(0), Some(1));
    let spare = bench.spares.pop().unwrap();
    let clock: VirtualClock = VirtualClock::new();
    let lent = bench
        .ring
        .serve(&clock, |view| match view.next_unit() {
            Some(Unit::Single(unit)) => {
                assert_eq!(unit.written(), 114);
                Some(unit.lend(spare))
            }
            _ => None,
        })
        .ok()
        .unwrap()
        .unwrap();
    let ring = &bench.ring;
    assert_eq!(
        ring.arena.bytes(&lent),
        &frame[..],
        "the frame, from the floor"
    );
    assert_eq!(
        ring.arena.floor(&lent).copy::<64>(),
        [0xaa; 64],
        "its header below"
    );
    let descriptor = &ring.descriptors.ring[0];
    assert_eq!(descriptor.word0(), bench.arm_word());
    assert_eq!(
        descriptor.buffer_address(),
        ARENA_BASE + ((COUNT - 1) * SIZE) as u32,
        "the spare"
    );
    assert_eq!(descriptor.next_address(), 0, "the new tail");
    assert_eq!(
        ring.descriptors.ring[RING - 1].next_address(),
        bench.descriptor_address(0)
    );
    assert_eq!(ring.walker.rings, 1, "one doorbell");
}

brand!(exhausted);
/// `tasks/262`'s hole: an exhausted ring, one unit exchanged, the service
/// returns; the ring rings, and again when the walker settled with NEXT zero
/// and LAST short of the new tail.
#[test]
fn an_exhausted_ring_is_rung_after_one_exchange_and_again_when_next_stayed_zero() {
    let mut bench = bench::<exhausted::Rx>();
    for index in 0..RING {
        bench.complete(index, 80, true);
    }
    // At the service: all completed, the walker exhausted. After the first
    // doorbell: NEXT still zero, LAST still the old tail.
    bench.cursor(Some(RING - 1), None);
    bench.cursor(Some(RING - 1), None);
    let spare = bench.spares.pop().unwrap();
    let clock: VirtualClock = VirtualClock::new();
    let served = bench.ring.serve(&clock, |view| match view.next_unit() {
        Some(Unit::Single(unit)) => Some(unit.lend(spare)),
        _ => None,
    });
    assert!(served.ok().unwrap().is_some(), "the first unit, of four");
    assert_eq!(bench.ring.walker.rings, 2, "rung, and rung again");
}

brand!(bounded);
#[test]
fn the_view_ends_at_last_and_after_a_dropped_unit() {
    let mut bench = bench::<bounded::Rx>();
    bench.complete(0, 80, true);
    bench.complete(1, 80, true);
    // LAST at 0: descriptor 1 is complete but beyond it.
    bench.cursor(Some(0), Some(1));
    let spare = bench.spares.pop().unwrap();
    let clock: VirtualClock = VirtualClock::new();
    let beyond = bench
        .ring
        .serve(&clock, |view| {
            let lent = match view.next_unit() {
                Some(Unit::Single(unit)) => Some(unit.lend(spare)),
                _ => None,
            };
            (lent.is_some(), view.next_unit().is_some())
        })
        .ok()
        .unwrap();
    assert_eq!(beyond, (true, false), "the unit at LAST, and none after it");
    // Next service: LAST at 1, the walker on 2; descriptor 2 completed too
    // but beyond LAST. The unit at 1 is dropped.
    bench.complete(2, 80, true);
    bench.cursor(Some(1), Some(2));
    let rung = bench.ring.walker.rings;
    let units = bench
        .ring
        .serve(&clock, |view| {
            let mut units = 0;
            while let Some(unit) = view.next_unit() {
                units += 1;
                drop(unit);
            }
            units
        })
        .ok()
        .unwrap();
    assert_eq!(units, 1, "the dropped unit ends the view");
    assert_eq!(
        bench.ring.walker.rings, rung,
        "nothing re-armed, nothing rung"
    );
    assert_eq!(bench.ring.start.get(), 1, "the dropped unit stays first");
}

brand!(chain);
/// `tasks/263`: a chain of three buffers, each continuation counting from
/// its first byte; the reader yields exactly the frame, in order, and the
/// buffers are re-armed only after the copy.
#[test]
fn a_chain_is_copied_from_its_continuations_first_bytes_and_re_armed_after() {
    let mut bench = bench::<chain::Rx>();
    bench.write_buffer(0, &[0x11; SIZE]);
    let first: Vec<u8> = (0..SIZE - 64).map(|k| k as u8).collect();
    bench.write_buffer(0, &[[0x11; 64].as_slice(), &first].concat());
    let second: Vec<u8> = (0..SIZE).map(|k| (100 + k) as u8).collect();
    bench.write_buffer(1, &second);
    let third: Vec<u8> = (0..40).map(|k| (200 + k) as u8).collect();
    bench.write_buffer(2, &third);
    bench.complete(0, SIZE as u32, false);
    bench.complete(1, SIZE as u32, false);
    bench.complete(2, 40, true);
    bench.cursor(Some(2), Some(3));
    let expected: Vec<u8> = [first, second, third].concat();
    let table: &'static RxDescriptors<RING> = bench.ring.descriptors;
    let clock: VirtualClock = VirtualClock::new();
    let copied = bench
        .ring
        .serve(&clock, |view| match view.next_unit() {
            Some(Unit::Chain(chain)) => Some(chain.copy_in_place(|frame| {
                // A completed continuation's word reads as an armed one
                // (its length is the capacity, `RX_DONE` clear): the
                // terminal descriptor's shows the copy precedes the re-arm.
                assert!(
                    table.ring[2].word0().rx_done(),
                    "not re-armed during the copy"
                );
                let mut out = vec![0; frame.len()];
                assert_eq!(frame.copy_to(&mut out), frame.len());
                out
            })),
            _ => None,
        })
        .ok()
        .unwrap()
        .unwrap();
    assert_eq!(copied, expected, "the frame's bytes, in order");
    assert_eq!(copied.len(), (SIZE - 64) + SIZE + 40);
    for index in 0..3 {
        let descriptor = &bench.ring.descriptors.ring[index];
        assert_eq!(descriptor.word0(), bench.arm_word());
        assert_eq!(
            descriptor.buffer_address(),
            ARENA_BASE + (index * SIZE) as u32,
            "in place"
        );
    }
    assert_eq!(
        bench.ring.descriptors.ring[0].next_address(),
        bench.descriptor_address(1)
    );
    assert_eq!(
        bench.ring.descriptors.ring[1].next_address(),
        bench.descriptor_address(2)
    );
    assert_eq!(
        bench.ring.descriptors.ring[2].next_address(),
        0,
        "the new tail"
    );
    assert_eq!(
        bench.ring.descriptors.ring[RING - 1].next_address(),
        bench.descriptor_address(0)
    );
    assert_eq!(bench.ring.walker.rings, 1);
}

/// Serve one view in which the keeper lends the first single unit, or
/// copies nothing: what it was handed.
fn lend_first<B: Brand<Window = DmaWindow> + 'static>(
    bench: &mut Bench<B>,
    clock: &impl Clock,
) -> Result<Option<Handle<B>>, (RingAnomaly, Option<Handle<B>>)> {
    let spare = bench.spares.pop().unwrap();
    bench.ring.serve(clock, |view| match view.next_unit() {
        Some(Unit::Single(unit)) => Some(unit.lend(spare)),
        _ => None,
    })
}

brand!(long);
#[test]
fn a_length_past_the_buffer_or_below_the_floor_stops_the_view_as_an_anomaly() {
    let mut bench = bench::<long::Rx>();
    for (length, case) in [
        (SIZE as u32 + 4, "past the buffer"),
        (30, "below the floor"),
    ] {
        bench.complete(0, length, true);
        bench.cursor(Some(0), Some(1));
        let word = bench.ring.descriptors.ring[0].word0();
        let clock: VirtualClock = VirtualClock::new();
        let served = lend_first(&mut bench, &clock);
        assert!(
            matches!(served, Err((RingAnomaly::Length { word: seen }, None)) if seen == word),
            "{case}: the anomaly, no unit"
        );
        assert_eq!(bench.ring.walker.rings, 0, "{case}: nothing re-armed");
        assert_eq!(
            bench.ring.descriptors.ring[0].word0(),
            word,
            "{case}: the descriptor as the walker left it"
        );
        assert_eq!(bench.ring.start.get(), 0, "{case}");
    }
}

brand!(continuation);
#[test]
fn a_chain_s_continuation_length_field_is_not_read() {
    let mut bench = bench::<continuation::Rx>();
    bench.complete(0, 100, false);
    bench.complete(1, 40, true);
    bench.cursor(Some(1), Some(2));
    let clock: VirtualClock = VirtualClock::new();
    let served = bench.ring.serve(&clock, |view| match view.next_unit() {
        Some(Unit::Chain(chain)) => Some(chain.copy_in_place(|frame| frame.len())),
        _ => None,
    });
    assert!(
        matches!(served, Ok(Some(len)) if len == SIZE - continuation::Rx::FLOOR + 40),
        "the continuation's bytes are SIZE whatever its field holds"
    );
    assert_eq!(bench.ring.walker.rings, 1);
    assert_eq!(bench.ring.start.get(), 2);
}

brand!(stuck);
#[test]
fn a_doorbell_the_walker_does_not_accept_is_an_anomaly_with_the_unit_lent() {
    let mut bench = bench::<stuck::Rx>();
    bench.complete(0, 80, true);
    bench.cursor(Some(0), Some(1));
    bench.ring.walker.stuck = true;
    let clock = Ticking(Cell::new(0));
    let served = lend_first(&mut bench, &clock);
    assert!(
        matches!(served, Err((RingAnomaly::DoorbellPending, Some(_)))),
        "the anomaly, the unit with it"
    );
    assert_eq!(bench.ring.walker.rings, 1, "rung once, never accepted");
    assert_eq!(
        bench.ring.descriptors.ring[RING - 1].next_address(),
        bench.descriptor_address(0),
        "linked"
    );
}
