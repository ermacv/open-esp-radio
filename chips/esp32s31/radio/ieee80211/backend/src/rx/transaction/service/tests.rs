extern crate std;

use core::cell::RefCell;
use std::{boxed::Box, vec::Vec};

use crate::mac::rx::pool::NetworkRxFrame;
use oer_chips_esp32s31_hal::radio::ieee80211::dma::{
    descriptor::rx_armed_word, rx_dma::model::RxWalkerModel, rx_ring::RxDmaArenaState,
    space::HostDma,
};

use super::*;
use crate::rx::counters::WifiRxCounters;
use crate::rx::sink::CountingAdmission;
use crate::rx::test_ring::{BUFFER as RING_BUFFER, Ring, SLOTS as RING_SLOTS};
use crate::rx::transaction::{CompletedUnit, IngressClass, IngressRoute, Preview};
use core::cell::Cell;

const COUNT: usize = 4;
const BUFFER: usize = 8_192;
const SLOTS: usize = 2;
const CAPACITY: usize = 64;

/// A publisher with free capacity which no test frame reaches.
struct Unreached;

impl<'pool> Publisher<'pool, CAPACITY, SLOTS> for Unreached {
    const DEPTH: usize = SLOTS;

    fn free_capacity(&self) -> usize {
        SLOTS
    }

    fn preview(&self, unit: CompletedUnit, _: [u8; 24]) -> Preview {
        self.unclassified_preview(unit)
    }

    fn unclassified_preview(&self, unit: CompletedUnit) -> Preview {
        Preview {
            unit,
            frame_control: None,
            class: IngressClass::Unclassified,
            route: IngressRoute::Malformed,
        }
    }

    fn try_send(
        &self,
        frame: NetworkRxFrame<'pool, SLOTS, CAPACITY>,
    ) -> Result<(), NetworkRxFrame<'pool, SLOTS, CAPACITY>> {
        Err(frame)
    }
}

#[derive(Default)]
struct Recorder {
    observations: RefCell<Vec<Observation>>,
    discards: RefCell<Vec<Discard>>,
    services: RefCell<Vec<ServiceObservation>>,
}

impl Admission for Recorder {
    fn observe(&self, observation: Observation) {
        self.observations.borrow_mut().push(observation);
    }
}

impl Hooks for &Recorder {
    fn observing(&self) -> bool {
        true
    }

    fn stage_discarded(&self, discard: Discard) {
        self.discards.borrow_mut().push(discard);
    }

    fn service_completed(&self, observation: ServiceObservation) {
        self.services.borrow_mut().push(observation);
    }
}

#[test]
fn a_chain_longer_than_the_length_field_is_a_counted_oversized_drop() {
    let placed = oer_chips_esp32s31_hal::radio::ieee80211::dma::rx_storage::RxDmaStorage::place(
        oer_chips_esp32s31_hal::system::memory::dma_static::Placed::host(Box::leak(Box::new(
            RxDmaStorage::<COUNT, BUFFER, BUFFER, HostDma>::new(),
        ))),
    )
    .unwrap();
    let storage = placed.storage();
    let buffers = storage.buffer_addresses();
    let base = placed.descriptor_base();
    let mut hardware = RxWalkerModel::new();
    let mut ring = placed
        .prepare_ring(&mut hardware)
        .and_then(|ring| ring.core_ready_for_test().publish(&mut hardware))
        .unwrap()
        .try_start(&mut hardware)
        .map_err(|(_, error)| error)
        .unwrap();

    // Two full segments and a 100-byte terminal: more bytes than one
    // descriptor's 14-bit length field holds.
    for (descriptor, &buffer) in storage.descriptors().iter().zip(buffers.iter()).take(2) {
        storage.dma_write(&ring, buffer, &[0; 4]).unwrap();
        descriptor.write_word0(
            oer_chips_esp32s31_hal::radio::ieee80211::dma::descriptor::rx_landed_word(
                BUFFER as u32,
                BUFFER as u32,
                false,
            ),
        );
    }
    storage.descriptors()[2].write_word0(
        oer_chips_esp32s31_hal::radio::ieee80211::dma::descriptor::rx_landed_word(
            BUFFER as u32,
            100,
            true,
        ),
    );
    hardware.set_cursor(base, 2, Some(3));

    let pool = RxStagePool::<SLOTS, CAPACITY>::new();
    let recorder = Recorder::default();
    let (mut descriptors, mut units, mut bytes) = (0, 0, 0);
    let progress = service(
        &mut ring,
        storage,
        &pool,
        &Unreached,
        &recorder,
        Counters {
            descriptors: &mut descriptors,
            units: &mut units,
            bytes: &mut bytes,
        },
        &mut hardware,
        &mut crate::rx::test_ring::NoBufferFullCounter,
        Settle::new(
            crate::rx::receive::RELOAD_SETTLE_LIMIT,
            &oer_portable_time_virtual::VirtualClock::<1>::default(),
        ),
        &recorder,
    )
    .expect("an oversized chain is dropped, not a ring fault");

    let length = 2 * BUFFER + 100;
    assert!(rx_armed_word(u32::try_from(length).unwrap()).is_none());
    assert_eq!(*recorder.discards.borrow(), [Discard::Oversized]);
    assert_eq!(
        *recorder.observations.borrow(),
        [Observation::DiscardRetained {
            unit: CompletedUnit {
                head_index: 0,
                descriptor_count: 3,
                payload_length: length,
            },
            reason: Discard::Oversized,
        }]
    );
    let services = recorder.services.borrow();
    assert_eq!(services.len(), 1);
    assert_eq!(services[0].discarded_units, 1);
    assert_eq!(services[0].staged_units, 0);
    assert_eq!(services[0].recycled_descriptors, 3);
    assert_eq!((descriptors, units, bytes), (3, 1, 0));
    assert_eq!(progress, DatapathRxProgress::RecycledAppendPending);

    // The chain's descriptors are back with the walker and the arena lives.
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::Live);
    assert!(ring.observed_mask().is_empty());
    assert!(!ring.reload_pending());
    for descriptor in &storage.descriptors()[..3] {
        assert_eq!(descriptor.word0(), rx_armed_word(BUFFER as u32).unwrap());
    }
    assert!(ring.topology_snapshot().valid);
    assert!(ring.try_stop(&mut hardware).is_ok());
}

type Storage = RxDmaStorage<COUNT, BUFFER, BUFFER, HostDma>;

/// A publisher which, at the moment a frame becomes visible above the
/// radio, reads its descriptor's word0.
struct WordAtPublish {
    storage: &'static Storage,
    base: u32,
    seen: RefCell<Vec<oer_chips_esp32s31_hal::layouts::WifiRxDmaDescriptorControl>>,
}

impl<'pool> Publisher<'pool, CAPACITY, SLOTS> for WordAtPublish {
    const DEPTH: usize = SLOTS;

    fn free_capacity(&self) -> usize {
        SLOTS
    }

    fn preview(&self, unit: CompletedUnit, _: [u8; 24]) -> Preview {
        self.unclassified_preview(unit)
    }

    fn unclassified_preview(&self, unit: CompletedUnit) -> Preview {
        Unreached.unclassified_preview(unit)
    }

    fn try_send(
        &self,
        frame: NetworkRxFrame<'pool, SLOTS, CAPACITY>,
    ) -> Result<(), NetworkRxFrame<'pool, SLOTS, CAPACITY>> {
        let offset = frame.segment().descriptor_address - self.base;
        let index = (offset
            / oer_chips_esp32s31_hal::radio::ieee80211::dma::descriptor::DESCRIPTOR_BYTES)
            as usize;
        self.seen
            .borrow_mut()
            .push(self.storage.descriptors()[index].word0());
        Ok(())
    }
}

#[test]
fn a_staged_frame_is_published_only_after_its_descriptor_owner_bit_is_clear() {
    let placed = oer_chips_esp32s31_hal::radio::ieee80211::dma::rx_storage::RxDmaStorage::place(
        oer_chips_esp32s31_hal::system::memory::dma_static::Placed::host(Box::leak(Box::new(
            Storage::new(),
        ))),
    )
    .unwrap();
    let storage = placed.storage();
    let base = placed.descriptor_base();
    let mut hardware = RxWalkerModel::new();
    // This fixture starts directly at a retained ring cursor.
    hardware.next_descriptor_low =
        base & oer_chips_esp32s31_hal::radio::ieee80211::registers::WifiDescriptorAddressLow::MAX;
    let mut ring = placed
        .prepare_ring(&mut hardware)
        .and_then(|ring| ring.core_ready_for_test().publish(&mut hardware))
        .unwrap()
        .try_start(&mut hardware)
        .map_err(|(_, error)| error)
        .unwrap();
    let completed = oer_chips_esp32s31_hal::radio::ieee80211::dma::descriptor::rx_landed_word(
        BUFFER as u32,
        40,
        true,
    );
    storage.descriptors()[0].write_word0(completed);
    hardware.set_cursor(base, 0, Some(1));

    let pool = RxStagePool::<SLOTS, CAPACITY>::new();
    let recorder = Recorder::default();
    let publisher = WordAtPublish {
        storage,
        base,
        seen: RefCell::new(Vec::new()),
    };
    let (mut descriptors, mut units, mut bytes) = (0, 0, 0);
    service(
        &mut ring,
        storage,
        &pool,
        &publisher,
        &recorder,
        Counters {
            descriptors: &mut descriptors,
            units: &mut units,
            bytes: &mut bytes,
        },
        &mut hardware,
        &mut crate::rx::test_ring::NoBufferFullCounter,
        Settle::new(
            crate::rx::receive::RELOAD_SETTLE_LIMIT,
            &oer_portable_time_virtual::VirtualClock::<1>::default(),
        ),
        &recorder,
    )
    .expect("one frame is staged");
    assert_eq!((units, bytes), (1, 40));
    assert_eq!(*publisher.seen.borrow(), [completed.with_owner(false)]);
    assert_eq!(ring.reload_evidence().walker_on_detached, 0);
    assert!(ring.try_stop(&mut hardware).is_ok());
}

/// Keeps what it is sent, while it has room.
struct Keeping<'a> {
    room: Cell<usize>,
    /// Sends refused though `room` says there is some.
    refuse: Cell<usize>,
    kept: RefCell<Vec<NetworkRxFrame<'a, RING_SLOTS, RING_BUFFER>>>,
}

impl<'a> Keeping<'a> {
    fn new(room: usize) -> Self {
        Self {
            room: Cell::new(room),
            refuse: Cell::new(0),
            kept: RefCell::new(Vec::new()),
        }
    }

    fn first_bytes(&self) -> Vec<u8> {
        self.kept
            .borrow()
            .iter()
            .map(|frame| frame.segment().buffer[0])
            .collect()
    }
}

impl<'a> Publisher<'a, RING_BUFFER, RING_SLOTS> for Keeping<'a> {
    const DEPTH: usize = 4;

    fn free_capacity(&self) -> usize {
        self.room.get()
    }

    fn preview(&self, unit: CompletedUnit, bytes: [u8; 24]) -> Preview {
        crate::rx::sink::frame_control_preview(unit, bytes)
    }

    fn unclassified_preview(&self, unit: CompletedUnit) -> Preview {
        crate::rx::sink::unclassified_preview(unit)
    }

    fn try_send(
        &self,
        frame: NetworkRxFrame<'a, RING_SLOTS, RING_BUFFER>,
    ) -> Result<(), NetworkRxFrame<'a, RING_SLOTS, RING_BUFFER>> {
        if self.room.get() == 0 {
            return Err(frame);
        }
        if self.refuse.get() != 0 {
            self.refuse.set(self.refuse.get() - 1);
            return Err(frame);
        }
        self.room.set(self.room.get() - 1);
        self.kept.borrow_mut().push(frame);
        Ok(())
    }
}

fn admission(counters: &'static WifiRxCounters) -> CountingAdmission<'static> {
    CountingAdmission::new(counters)
}

fn counters() -> &'static WifiRxCounters {
    Box::leak(Box::new(WifiRxCounters::new()))
}

#[test]
fn a_frame_that_landed_in_r_goes_out_first_and_once() {
    let mut ring = Ring::cold();
    let publisher = Keeping::new(4);
    let admission = admission(counters());
    ring.land_reserved(&[0xb1; 64]);
    ring.settle(&publisher, &admission);
    assert_eq!(publisher.first_bytes(), [0xb1]);
    assert_eq!(publisher.kept.borrow()[0].segment().buffer, [0xb1; 64]);
    assert!(!ring.reserved_held(), "R is rearmed once the copy is out");
    assert_eq!(ring.pool.claimed_slots(), 1);
    ring.settle(&publisher, &admission);
    assert_eq!(publisher.first_bytes(), [0xb1], "published once");
}

#[test]
fn a_frame_in_r_goes_ahead_of_a_ring_frame_of_the_same_pass() {
    let mut ring = Ring::cold();
    let publisher = Keeping::new(4);
    let admission = admission(counters());
    ring.land_reserved(&[0xb1; 64]);
    ring.complete_with(&[0xb2; 64]);
    ring.settle(&publisher, &admission);
    assert_eq!(publisher.first_bytes(), [0xb1, 0xb2]);
}

#[test]
fn a_full_queue_leaves_the_frame_in_r_and_takes_no_later_frame() {
    let mut ring = Ring::cold();
    let publisher = Keeping::new(0);
    let admission = admission(counters());
    ring.land_reserved(&[0xb1; 64]);
    ring.complete_with(&[0xb2; 64]);
    assert_eq!(
        ring.settle(&publisher, &admission),
        DatapathRxProgress::StageCapacityBlocked
    );
    assert!(ring.reserved_held(), "R keeps its frame");
    assert_eq!(ring.pool.claimed_slots(), 0, "no copy stays out");
    publisher.room.set(4);
    ring.settle(&publisher, &admission);
    assert_eq!(publisher.first_bytes(), [0xb1, 0xb2]);
}

#[test]
fn a_refused_publication_keeps_the_frame_in_r_and_returns_the_copy() {
    let mut ring = Ring::cold();
    let publisher = Keeping::new(4);
    publisher.refuse.set(1);
    let admission = admission(counters());
    ring.land_reserved(&[0xb1; 64]);
    assert_eq!(
        ring.settle(&publisher, &admission),
        DatapathRxProgress::StageCapacityBlocked
    );
    assert!(ring.reserved_held(), "R is rearmed only after publication");
    assert_eq!(ring.pool.claimed_slots(), 0, "the copy came back");
    ring.settle(&publisher, &admission);
    assert_eq!(
        publisher.first_bytes(),
        [0xb1],
        "published once it could be"
    );
    assert!(!ring.reserved_held());
}

#[test]
fn a_copy_still_out_holds_the_next_landing_and_the_pause_sees_it() {
    let mut ring = Ring::cold();
    let publisher = Keeping::new(4);
    let admission = admission(counters());
    ring.land_reserved(&[0xb1; 64]);
    ring.settle(&publisher, &admission);
    // A pause waits on the claimed slots ([`crate::rx::session`]).
    assert_eq!(ring.pool.claimed_slots(), 1);
    ring.land_reserved(&[0xb2; 64]);
    assert_eq!(
        ring.settle(&publisher, &admission),
        DatapathRxProgress::StageCapacityBlocked
    );
    assert!(ring.reserved_held());
    publisher.kept.borrow_mut().clear();
    assert_eq!(ring.pool.claimed_slots(), 0);
    ring.settle(&publisher, &admission);
    assert_eq!(publisher.first_bytes(), [0xb2]);
}

#[test]
fn a_warm_start_publishes_only_its_ring_frames() {
    let mut ring = Ring::new();
    let publisher = Keeping::new(4);
    let admission = admission(counters());
    ring.complete_with(&[0xb2; 64]);
    ring.settle(&publisher, &admission);
    assert_eq!(publisher.first_bytes(), [0xb2]);
    assert!(!ring.reserved_held());
}

/// Admits `limit` bytes and records what it observed.
struct Limited<'a> {
    limit: usize,
    inner: CountingAdmission<'a>,
    observed: RefCell<Vec<Observation>>,
}

impl Admission for Limited<'_> {
    fn maximum_payload_length(&self, _: CompletedUnit, physical_capacity: usize) -> usize {
        self.limit.min(physical_capacity)
    }

    fn observe(&self, observation: Observation) {
        self.observed.borrow_mut().push(observation);
        self.inner.observe(observation);
    }
}

fn limited(limit: usize) -> Limited<'static> {
    Limited {
        limit,
        inner: admission(counters()),
        observed: RefCell::new(Vec::new()),
    }
}

/// An RX-control prefix and frame control `control`, the MPDU's start.
fn with_control(control: [u8; 2]) -> [u8; PUBLIC_HEADER_SIZE + 24] {
    let mut frame = [0_u8; PUBLIC_HEADER_SIZE + 24];
    frame[PUBLIC_HEADER_SIZE..PUBLIC_HEADER_SIZE + 2].copy_from_slice(&control);
    frame
}

const PROTECTED_DATA: [u8; 2] = [0x08, 0x40];
const BEACON: [u8; 2] = [0x80, 0x00];

#[test]
fn a_frame_in_r_longer_than_the_policy_admits_is_dropped_and_counted() {
    let mut ring = Ring::cold();
    let publisher = Keeping::new(4);
    let admission = limited(16);
    ring.land_reserved(&[0xb1; 64]);
    ring.settle(&publisher, &admission);
    assert_eq!(publisher.first_bytes(), [0_u8; 0]);
    assert!(!ring.reserved_held(), "R is rearmed as a discard is");
    assert!(matches!(
        admission.observed.borrow()[0],
        Observation::DiscardRetained {
            reason: Discard::TooLong,
            ..
        }
    ));
}

#[test]
fn a_protected_frame_in_r_leaves_the_critical_credit_alone() {
    let mut ring = Ring::cold();
    let publisher = Keeping::new(1);
    let admission = limited(RING_BUFFER);
    ring.land_reserved(&with_control(PROTECTED_DATA));
    ring.settle(&publisher, &admission);
    assert_eq!(publisher.kept.borrow().len(), 0);
    assert_eq!(publisher.room.get(), 1, "the last credit stays");
    assert!(matches!(
        admission.observed.borrow()[0],
        Observation::OverloadDiscardedAndRecycled(_)
    ));
}

#[test]
fn a_critical_frame_in_r_takes_the_critical_credit() {
    let mut ring = Ring::cold();
    let publisher = Keeping::new(1);
    let admission = limited(RING_BUFFER);
    ring.land_reserved(&with_control(BEACON));
    ring.settle(&publisher, &admission);
    assert_eq!(publisher.kept.borrow().len(), 1);
    assert!(matches!(
        admission.observed.borrow()[0],
        Observation::CriticalReserveAdmitted(_)
    ));
}

#[test]
fn a_critical_frame_in_r_waits_when_no_credit_is_left() {
    let mut ring = Ring::cold();
    let publisher = Keeping::new(0);
    let admission = limited(RING_BUFFER);
    ring.land_reserved(&with_control(BEACON));
    assert_eq!(
        ring.settle(&publisher, &admission),
        DatapathRxProgress::StageCapacityBlocked
    );
    assert!(ring.reserved_held());
    assert!(matches!(
        admission.observed.borrow()[0],
        Observation::CriticalAdmissionBlocked(_)
    ));
}
