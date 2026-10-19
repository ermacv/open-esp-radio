extern crate std;

use self::std::boxed::Box;
use super::*;
use crate::settle::{Settle, SettleLimit};
use oer_time::{Clock, Duration, Instant};

/// A clock which never moves: a reload that settles is never cut short.
struct Frozen;

impl Clock for Frozen {
    fn now(&self) -> Instant {
        Instant::from_micros(0)
    }
}

fn frozen_settle() -> Settle<'static> {
    Settle::new(
        SettleLimit::new(Duration::from_micros(20)).unwrap(),
        &Frozen,
    )
}
use crate::ieee80211::dma::{rx_dma::RxDmaBinding, space::HostDma};

/// Place a host storage as a placed static would be.
fn place<const COUNT: usize, const BUFFER: usize, const STORAGE: usize, S: DmaSpace>(
    storage: &'static mut RxDmaStorage<COUNT, BUFFER, STORAGE, S>,
) -> PlacedRxDma<COUNT, BUFFER, STORAGE, S> {
    RxDmaStorage::place(Placed::host(storage)).expect("the host storage has a layout")
}

#[derive(Default)]
struct MockRxDma {
    walker: bool,
    descriptor_base: u32,
    last_descriptor_low: u32,
    next_descriptor_low: u32,
    fail_disable: bool,
    ambiguous_enable: bool,
    reload: bool,
    disable_calls: usize,
    publications: usize,
}

impl crate::ieee80211::dma::rx_dma::sealed::Sealed for MockRxDma {}

impl RxDma for MockRxDma {
    fn last_descriptor_low(&mut self) -> u32 {
        self.last_descriptor_low
    }

    fn next_descriptor_low(&mut self) -> u32 {
        self.next_descriptor_low
    }

    fn next_descriptor(&mut self) -> crate::ieee80211::dma::rx_dma::RxDmaNextDescriptor {
        crate::ieee80211::dma::rx_dma::RxDmaNextDescriptor::for_host_test(
            self.next_descriptor_low,
            false,
        )
    }

    fn with_ordered_cursor<R>(
        &mut self,
        observed: impl for<'confirmation> FnOnce(
            crate::ieee80211::dma::rx_dma::RxDmaCursorObservation<'confirmation>,
        ) -> R,
    ) -> R {
        let last = self.last_descriptor_low();
        self.fence();
        let next = self.next_descriptor_low();
        self.fence();
        observed(crate::ieee80211::dma::rx_dma::RxDmaCursorObservation::for_host_test(last, next))
    }

    fn walker_enabled(&mut self) -> bool {
        self.walker
    }

    fn reload_pending(&mut self) -> bool {
        self.reload
    }

    fn try_with_reload_settled<R>(
        &mut self,
        settled: impl for<'confirmation> FnOnce(
            crate::ieee80211::dma::rx_dma::RxDmaReloadSettled<'confirmation>,
        ) -> R,
    ) -> Option<R> {
        (!self.reload_pending())
            .then(|| settled(crate::ieee80211::dma::rx_dma::RxDmaReloadSettled::for_host_test()))
    }

    fn configure_descriptor_window(&mut self, _: &RxDmaBinding<'_>) {}

    fn write_descriptor_base(&mut self, binding: &RxDmaBinding<'_>) {
        self.publications += 1;
        self.descriptor_base = binding.reserved_address();
    }

    fn publish_walker_enable(&mut self, _: &RxDmaBinding<'_>) {
        self.walker = true;
    }

    fn request_reload(&mut self, _: &RxDmaBinding<'_>) {
        self.publications += 1;
    }

    fn try_with_walker_enabled<R>(
        &mut self,
        _: &RxDmaBinding<'_>,
        enabled: impl for<'confirmation> FnOnce(
            crate::ieee80211::dma::rx_dma::RxDmaWalkerEnabled<'confirmation>,
        ) -> R,
    ) -> Option<R> {
        self.walker = true;
        (!self.ambiguous_enable)
            .then(|| enabled(crate::ieee80211::dma::rx_dma::RxDmaWalkerEnabled::for_host_test()))
    }

    fn try_with_walker_stopped<R>(
        &mut self,
        binding: &RxDmaBinding<'_>,
        stopped: impl for<'confirmation> FnOnce(
            crate::ieee80211::dma::rx_dma::RxDmaWalkerStopped<'confirmation>,
        ) -> R,
    ) -> Option<R> {
        self.disable_calls += 1;
        if self.fail_disable {
            return None;
        }
        self.walker = false;
        Some(stopped(
            crate::ieee80211::dma::rx_dma::RxDmaWalkerStopped::for_host_test(binding),
        ))
    }

    fn fence(&mut self) {}
}

#[test]
fn arena_initializes_in_its_final_location_and_recycles_one_buffer() {
    let mut storage = RxDmaStorage::<2, 16, 20, HostDma>::new();

    assert_eq!(storage.descriptors().len(), 2);
    assert_eq!(storage.buffers().len(), 2);
    assert_eq!(storage.buffers().as_ptr().addr() & 3, 0);

    storage.prepare_unpublished_buffer(0).unwrap();
    assert!(!storage.buffers()[0].leading_guard_overwritten());
}

#[test]
fn a_placed_storage_publishes_its_own_addresses() {
    const COUNT: usize = 2;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    assert_eq!(
        storage.buffer_addresses(),
        &[0; COUNT],
        "nothing before placing"
    );
    let placed = place(storage);
    let storage = placed.storage();
    let base = placed.descriptor_base();
    assert_eq!(
        base,
        HostDma::address(storage, storage.descriptors().as_ptr()).unwrap()
    );
    for (index, &address) in storage.buffer_addresses().iter().enumerate() {
        assert_eq!(
            address,
            HostDma::address(storage, storage.buffers()[index].0.get()).unwrap()
        );
    }

    let mut mmio = MockRxDma::default();
    let stopped = placed.prepare_ring(&mut mmio).unwrap();
    for (descriptor, &address) in storage.descriptors().iter().zip(storage.buffer_addresses()) {
        assert_eq!(descriptor.buffer_address(), address);
    }
    assert_eq!(
        storage.descriptors()[0].next_address(),
        base + crate::ieee80211::dma::descriptor::DESCRIPTOR_BYTES
    );
    // BASE names the reserved descriptor right after the ring.
    assert_eq!(
        mmio.descriptor_base,
        base + COUNT as u32 * crate::ieee80211::dma::descriptor::DESCRIPTOR_BYTES
    );
    let live = stopped
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .unwrap();
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn accepted_list_pressure_projects_next_to_the_current_tail() {
    const COUNT: usize = 4;
    const LOW_MASK: u32 = 0x000f_ffff;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let placed = place(storage);
    let base = placed.descriptor_base();
    let mut mmio = MockRxDma::default();
    let prepared = placed
        .prepare_ring(&mut mmio)
        .expect("prepared pressure model");
    let live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live pressure model");

    let next = |index: u32| {
        crate::ieee80211::dma::rx_dma::RxDmaNextDescriptor::for_host_test(
            (base + index * crate::ieee80211::dma::descriptor::DESCRIPTOR_BYTES) & LOW_MASK,
            false,
        )
    };
    assert_eq!(live.accepted_list_remaining_from_next(next(0)), Some(4));
    assert_eq!(live.accepted_list_remaining_from_next(next(1)), Some(3));
    assert_eq!(live.accepted_list_remaining_from_next(next(3)), Some(1));
    assert_eq!(
        live.accepted_list_remaining_from_next(
            crate::ieee80211::dma::rx_dma::RxDmaNextDescriptor::for_host_test(0, false)
        ),
        Some(0)
    );
    assert_eq!(
        live.accepted_list_remaining_from_next(
            crate::ieee80211::dma::rx_dma::RxDmaNextDescriptor::for_host_test(0, true)
        ),
        None,
        "upper-only NEXT is not an exhausted-list proof"
    );
    assert_eq!(
        live.accepted_list_remaining_from_next(
            crate::ieee80211::dma::rx_dma::RxDmaNextDescriptor::for_host_test(
                (base + COUNT as u32 * crate::ieee80211::dma::descriptor::DESCRIPTOR_BYTES)
                    & LOW_MASK,
                false,
            )
        ),
        None,
        "a foreign cursor must not alias a ring credit"
    );

    live.try_stop(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("stop pressure model");
}

#[test]
fn completed_ownership_follows_a_permuted_descriptor_buffer_binding() {
    const COUNT: usize = 2;
    const ADDRESS_LOW_MASK: u32 = oer_esp32s31_pac::WifiDescriptorAddressLow::MAX;
    // Descriptor zero names physical buffer one; descriptor one names
    // physical buffer zero.
    let mut storage = Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new());
    storage.buffer_mut(1).unwrap()[4..8].copy_from_slice(&[5, 6, 7, 8]);
    let storage = Box::leak(storage);
    let placed = RxDmaStorage::place_rotated(Placed::host(storage), 1).expect("pre-ring rotation");
    let storage = placed.storage();
    let base = placed.descriptor_base();
    assert_eq!(storage.descriptor_buffer_id(0), Some(1));
    assert_eq!(storage.descriptor_buffer_id(1), Some(0));

    let mut mmio = MockRxDma::default();
    let prepared = placed
        .prepare_ring(&mut mmio)
        .expect("prepared rotated owner");
    let mut live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live rotated epoch");
    storage.descriptors()[0].write_word0(crate::ieee80211::dma::descriptor::rx_landed_word(
        16, 8, true,
    ));
    mmio.last_descriptor_low =
        (base + crate::ieee80211::dma::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    mmio.next_descriptor_low = 0;

    let frozen = live.freeze_cursor(&mut mmio).unwrap();
    let detached = storage
        .take_completed_unit(&mut live, frozen)
        .unwrap()
        .expect("completed rotated unit")
        .detach_single(&mut mmio)
        .expect("rotated buffer detaches");
    let pool = oer_memory::ExternalRxHandoffPool::<16, 1>::new();
    let radio = pool
        .try_claim_radio(detached.into_buffer(), 0)
        .map_err(drop)
        .expect("rotated handoff slot");
    let length = radio.frame().len();
    let mut network = pool.claim_network(radio.republish(0, length));
    assert_eq!(
        network.with_frame(|frame| frame[4..8].to_vec()),
        [5, 6, 7, 8],
        "descriptor zero must expose its bound physical buffer one"
    );
    drop(network);
    assert!(
        storage
            .recycle_released_prefix::<COUNT, _>(&mut live, &mut mmio)
            .unwrap()
            .is_some()
    );
}

#[test]
fn detached_buffer_cannot_rearm_until_the_network_lease_returns() {
    const COUNT: usize = 2;
    const ADDRESS_LOW_MASK: u32 = oer_esp32s31_pac::WifiDescriptorAddressLow::MAX;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    storage.buffer_mut(0).unwrap()[4..8].copy_from_slice(&[1, 2, 3, 4]);
    let placed = place(storage);
    let storage = placed.storage();
    let base = placed.descriptor_base();
    let mut mmio = MockRxDma::default();
    let prepared = placed.prepare_ring(&mut mmio).expect("prepared owner");
    let mut live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live epoch");
    storage.descriptors()[0].write_word0(crate::ieee80211::dma::descriptor::rx_landed_word(
        16, 8, true,
    ));
    // Model the finite list exhausted at its accepted tail. Merely seeing
    // NEXT name descriptor one is not sufficient: hardware proved that image
    // may precede hardware fetching descriptor zero's link word.
    mmio.last_descriptor_low =
        (base + crate::ieee80211::dma::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    mmio.next_descriptor_low = 0;

    let frozen = live.freeze_cursor(&mut mmio).unwrap();
    let detached = storage
        .take_completed_unit(&mut live, frozen)
        .unwrap()
        .expect("completed unit")
        .detach_single(&mut mmio)
        .expect("single descriptor detaches");
    let pool = oer_memory::ExternalRxHandoffPool::<16, 1>::new();
    let radio = pool
        .try_claim_radio(detached.into_buffer(), 0)
        .map_err(drop)
        .expect("external handoff slot");
    let length = radio.frame().len();
    let mut network = pool.claim_network(radio.republish(0, length));
    assert_eq!(
        network.with_frame(|frame| frame[4..8].to_vec()),
        [1, 2, 3, 4]
    );
    assert_eq!(
        storage.recycle_released_prefix::<COUNT, _>(&mut live, &mut mmio),
        Ok(None),
        "live network token must keep the descriptor CPU-owned"
    );
    assert!(storage.descriptors()[0].word0().rx_done());

    drop(network);
    let append = storage
        .recycle_released_prefix::<COUNT, _>(&mut live, &mut mmio)
        .expect("released prefix reclaim")
        .expect("one descriptor appended");
    assert_eq!(append.descriptor_count, 1);
    assert_eq!(storage.released_buffer_count(), 0);
    assert!(!storage.descriptors()[0].word0().rx_done());
}

#[test]
fn role_neutral_prefix_recycle_normalizes_a_returned_staged_buffer() {
    const COUNT: usize = 2;
    const ADDRESS_LOW_MASK: u32 = oer_esp32s31_pac::WifiDescriptorAddressLow::MAX;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let placed = place(storage);
    let storage = placed.storage();
    let base = placed.descriptor_base();
    let mut mmio = MockRxDma::default();
    let prepared = placed.prepare_ring(&mut mmio).expect("prepared owner");
    let mut live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live epoch");
    storage.descriptors()[0].write_word0(crate::ieee80211::dma::descriptor::rx_landed_word(
        16, 8, true,
    ));
    mmio.last_descriptor_low =
        (base + crate::ieee80211::dma::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    mmio.next_descriptor_low = 0;

    let frozen = live.freeze_cursor(&mut mmio).unwrap();
    let detached = storage
        .take_completed_unit(&mut live, frozen)
        .unwrap()
        .expect("completed unit")
        .detach_single(&mut mmio)
        .expect("single descriptor detaches");
    let pool = oer_memory::ExternalRxHandoffPool::<16, 1>::new();
    let radio = pool
        .try_claim_radio(detached.into_buffer(), 0)
        .map_err(drop)
        .expect("external handoff slot");
    let length = radio.frame().len();
    drop(pool.claim_network(radio.republish(0, length)));
    assert_eq!(storage.released_buffer_count(), 1);

    let append = storage
        .recycle_completed_prefix::<COUNT, _>(&mut live, &mut mmio)
        .expect("role-neutral prefix reclaim")
        .expect("observed returned descriptor appends");
    assert_eq!(append.descriptor_count, 1);
    assert_eq!(storage.released_buffer_count(), 0);
    assert_eq!(
        storage.buffer_for_descriptor(0).unwrap().state(),
        RX_BUFFER_RING,
        "republished DMA descriptor and buffer state must move together"
    );
    assert!(!storage.descriptors()[0].word0().rx_done());
}

#[test]
fn released_dma_buffers_rearm_only_as_a_contiguous_ring_prefix() {
    const COUNT: usize = 4;
    const ADDRESS_LOW_MASK: u32 = oer_esp32s31_pac::WifiDescriptorAddressLow::MAX;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let placed = place(storage);
    let storage = placed.storage();
    let base = placed.descriptor_base();
    let mut mmio = MockRxDma::default();
    let prepared = placed.prepare_ring(&mut mmio).expect("prepared owner");
    let mut live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live epoch");
    for descriptor in &storage.descriptors()[..2] {
        descriptor.write_word0(crate::ieee80211::dma::descriptor::rx_landed_word(
            16, 8, true,
        ));
    }
    mmio.last_descriptor_low =
        (base + 3 * crate::ieee80211::dma::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    mmio.next_descriptor_low = 0;

    let frozen = live.freeze_cursor(&mut mmio).unwrap();
    let first = storage
        .take_completed_unit(&mut live, frozen)
        .unwrap()
        .expect("first completed unit")
        .detach_single(&mut mmio)
        .expect("first single descriptor");
    let frozen = live.freeze_cursor(&mut mmio).unwrap();
    let second = storage
        .take_completed_unit(&mut live, frozen)
        .unwrap()
        .expect("second completed unit")
        .detach_single(&mut mmio)
        .expect("second single descriptor");
    let pool = oer_memory::ExternalRxHandoffPool::<16, 2>::new();
    let first = pool
        .try_claim_radio(first.into_buffer(), 0)
        .map_err(drop)
        .expect("first handoff slot");
    let first_length = first.frame().len();
    let first = pool.claim_network(first.republish(0, first_length));
    let second = pool
        .try_claim_radio(second.into_buffer(), 1)
        .map_err(drop)
        .expect("second handoff slot");
    let second_length = second.frame().len();
    let second = pool.claim_network(second.republish(0, second_length));

    drop(second);
    assert_eq!(storage.released_buffer_count(), 1);
    assert_eq!(
        storage.recycle_released_prefix::<COUNT, _>(&mut live, &mut mmio),
        Ok(None),
        "a returned successor must not skip a retained ring head"
    );

    drop(first);
    let append = storage
        .recycle_released_prefix::<COUNT, _>(&mut live, &mut mmio)
        .expect("released prefix reclaim")
        .expect("both returned descriptors append together");
    assert_eq!(append.head_index.get(), 0);
    assert_eq!(append.descriptor_count, 2);
    assert_eq!(storage.released_buffer_count(), 0);
    assert_eq!(live.recycle_start().get(), 2);
    assert!(
        storage.descriptors()[..2]
            .iter()
            .all(|descriptor| !descriptor.word0().rx_done())
    );
}

#[test]
fn the_consumed_placement_moves_through_one_prepared_live_halted_graph() {
    let placed = place(Box::leak(Box::new(
        RxDmaStorage::<2, 16, 20, HostDma>::new(),
    )));
    let storage = placed.storage();
    let mut mmio = MockRxDma::default();
    let prepared = placed.prepare_ring(&mut mmio).expect("initial root owner");
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::Prepared);
    let live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live owner");
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::Live);
    let halted = live
        .try_stop(&mut mmio)
        .unwrap_or_else(|_| panic!("walker stops"));
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::Prepared);
    let _prepared = storage
        .prepare_halted(halted, &mut mmio)
        .unwrap_or_else(|_| panic!("same stopped arena is reusable"));
}

#[test]
fn live_binding_rejects_a_foreign_storage_arena() {
    const COUNT: usize = 2;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let placed = place(storage);
    let foreign = RxDmaStorage::<COUNT, 16, 20, HostDma>::new();
    let mut mmio = MockRxDma::default();
    let mut live = placed
        .prepare_ring(&mut mmio)
        .expect("prepared owner")
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live owner");

    let frozen = live.freeze_cursor(&mut mmio).unwrap();
    assert_eq!(
        foreign.completed_unit_frontier_through_cursor(&live, frozen),
        Err(RxRingError::DescriptorOwnerAddress)
    );

    let _halted = live
        .try_stop(&mut mmio)
        .unwrap_or_else(|_| panic!("walker stops"));
}

#[test]
fn first_frontier_returns_one_unit_from_a_terminal_burst() {
    const COUNT: usize = 4;
    const ADDRESS_LOW_MASK: u32 = oer_esp32s31_pac::WifiDescriptorAddressLow::MAX;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let placed = place(storage);
    let storage = placed.storage();
    let base = placed.descriptor_base();
    let mut mmio = MockRxDma::default();
    let prepared = placed.prepare_ring(&mut mmio).expect("prepared owner");
    let mut live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live epoch");

    for descriptor in storage.descriptors() {
        descriptor.write_word0(crate::ieee80211::dma::descriptor::rx_landed_word(
            16, 4, true,
        ));
    }
    mmio.last_descriptor_low =
        (base + 3 * crate::ieee80211::dma::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    mmio.next_descriptor_low = base & ADDRESS_LOW_MASK;

    let frozen = live.freeze_cursor(&mut mmio).unwrap();
    let complete = storage
        .completed_unit_frontier_through_cursor(&live, frozen)
        .expect("complete burst frontier");
    assert_eq!(complete.unit_count, COUNT);
    assert_eq!(complete.descriptor_count, COUNT);

    let first = storage
        .first_completed_unit_frontier_through_cursor(&live, frozen)
        .expect("first unit frontier");
    assert_eq!(first.unit_count, 1);
    assert_eq!(first.descriptor_count, 1);
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn failed_root_prepare_quarantines_the_arena_when_walker_stop_is_unproved() {
    const COUNT: usize = 2;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let placed = place(storage);
    let storage = placed.storage();
    let mut mmio = MockRxDma {
        walker: true,
        fail_disable: true,
        ..MockRxDma::default()
    };

    assert!(matches!(
        placed.prepare_ring(&mut mmio),
        Err(RxRingError::WalkerStopUnconfirmed)
    ));
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::ResetRequired);
}

#[test]
fn failed_halted_prepare_quarantines_the_arena_when_walker_stop_is_unproved() {
    const COUNT: usize = 2;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let placed = place(storage);
    let storage = placed.storage();
    let mut mmio = MockRxDma::default();
    let prepared = placed.prepare_ring(&mut mmio).expect("prepared owner");
    let live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live epoch");
    let halted = match live.try_stop(&mut mmio) {
        Ok(halted) => halted,
        Err(_) => panic!("walker stops"),
    };

    // Model an external hardware fault between epochs: the walker became
    // active again and refuses the stop request made by preparation.
    mmio.walker = true;
    mmio.fail_disable = true;
    let (_halted, error) = match storage.prepare_halted(halted, &mut mmio) {
        Ok(_) => panic!("unproved stop must reject preparation"),
        Err(failure) => failure,
    };

    assert_eq!(error, RxRingError::WalkerStopUnconfirmed);
    assert!(mmio.walker);
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::ResetRequired);
}

#[test]
fn partial_live_buffer_rearm_quarantines_the_arena() {
    const COUNT: usize = 2;
    const ADDRESS_LOW_MASK: u32 = oer_esp32s31_pac::WifiDescriptorAddressLow::MAX;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let placed = place(storage);
    let storage = placed.storage();
    let base = placed.descriptor_base();
    let mut mmio = MockRxDma::default();
    let prepared = placed.prepare_ring(&mut mmio).expect("prepared owner");
    let mut live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live epoch");
    storage.descriptors()[0].write_word0(
        crate::ieee80211::dma::descriptor::rx_armed_word(16)
            .expect("valid size")
            .with_rx_done(true),
    );
    mmio.last_descriptor_low = base & ADDRESS_LOW_MASK;
    mmio.next_descriptor_low =
        (base + crate::ieee80211::dma::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    // An upper owner keeps descriptor zero's buffer, so its rearm fails
    // inside the recycle transaction, after every descriptor check passed.
    let frozen = live.freeze_cursor(&mut mmio).unwrap();
    let detached = storage
        .take_completed_unit(&mut live, frozen)
        .unwrap()
        .expect("completed unit")
        .detach_single(&mut mmio)
        .expect("single descriptor detaches");

    assert_eq!(
        storage.recycle_completed_prefix::<1, _>(&mut live, &mut mmio),
        Ok(None),
    );
    storage.descriptors()[1].write_word0(
        crate::ieee80211::dma::descriptor::rx_armed_word(16)
            .expect("valid size")
            .with_rx_done(true),
    );
    mmio.last_descriptor_low =
        (base + crate::ieee80211::dma::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    assert_eq!(
        storage.recycle_completed_prefix::<1, _>(&mut live, &mut mmio),
        Err(RxRingError::BufferOwnershipConflict),
    );
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::ResetRequired);
    drop(detached);
}

#[test]
fn completed_descriptor_metadata_is_validated_before_payload_transfer() {
    const COUNT: usize = 2;

    for corruption in 0..3 {
        let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
        let placed = place(storage);
        let storage = placed.storage();
        let base = placed.descriptor_base();
        let buffers = *storage.buffer_addresses();
        let mut mmio = MockRxDma::default();
        let prepared = placed.prepare_ring(&mut mmio).expect("prepared owner");
        let mut live = prepared
            .try_start(&mut mmio)
            .map_err(|(_, error)| error)
            .expect("live epoch");
        let descriptor = &storage.descriptors()[0];
        let mut word0 = crate::ieee80211::dma::descriptor::rx_landed_word(16, 8, true);
        let mut buffer_address = buffers[0];
        let mut next_address = base + crate::ieee80211::dma::descriptor::DESCRIPTOR_BYTES;
        match corruption {
            0 => word0 = word0.with_size(17).unwrap(),
            1 => buffer_address = buffers[0] + 4,
            2 => next_address = 0,
            _ => unreachable!(),
        }
        descriptor.inject_corrupt_words(word0, buffer_address, next_address);
        // The walker reports descriptor zero as LAST: its image is final.
        mmio.last_descriptor_low = base & 0x000f_ffff;

        let frozen = live.freeze_cursor(&mut mmio).unwrap();
        assert!(matches!(
            storage.take_completed(&mut live, frozen, 0),
            Err(RxRingError::Corrupt)
        ));
        assert!(live.observed_mask().is_empty());
        assert_eq!(storage.lifecycle_state(), RxDmaArenaState::ResetRequired);
        assert!(live.try_stop(&mut mmio).is_ok());
    }
}

#[test]
fn impossible_reload_frontier_quarantines_the_static_arena() {
    const COUNT: usize = 2;
    const ADDRESS_LOW_MASK: u32 = oer_esp32s31_pac::WifiDescriptorAddressLow::MAX;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let placed = place(storage);
    let storage = placed.storage();
    let base = placed.descriptor_base();
    let mut mmio = MockRxDma::default();
    let prepared = placed.prepare_ring(&mut mmio).expect("prepared owner");
    let mut live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live epoch");
    storage.descriptors()[0].write_word0(
        crate::ieee80211::dma::descriptor::rx_armed_word(16)
            .expect("valid size")
            .with_rx_done(true),
    );
    mmio.last_descriptor_low = base & ADDRESS_LOW_MASK;
    mmio.next_descriptor_low =
        (base + crate::ieee80211::dma::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    let frozen = live.freeze_cursor(&mut mmio).unwrap();
    assert!(
        storage
            .take_completed(&mut live, frozen, 0)
            .unwrap()
            .is_some()
    );
    assert!(
        storage
            .recycle_completed_prefix::<1, _>(&mut live, &mut mmio)
            .unwrap()
            .is_none()
    );
    storage.descriptors()[1].write_word0(
        crate::ieee80211::dma::descriptor::rx_armed_word(16)
            .expect("valid size")
            .with_rx_done(true),
    );
    mmio.last_descriptor_low =
        (base + crate::ieee80211::dma::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    assert!(
        storage
            .recycle_completed_prefix::<1, _>(&mut live, &mut mmio)
            .unwrap()
            .is_some()
    );
    assert!(live.reload_pending());

    mmio.next_descriptor_low = 0;
    mmio.last_descriptor_low = (base + 1) & ADDRESS_LOW_MASK;
    assert_eq!(
        live.poll_pending_reload(&mut mmio),
        Err(RxRingError::Corrupt)
    );
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::ResetRequired);
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn in_arena_intermediate_last_rerings_the_doorbell() {
    const COUNT: usize = 4;
    const ADDRESS_LOW_MASK: u32 = oer_esp32s31_pac::WifiDescriptorAddressLow::MAX;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let placed = place(storage);
    let storage = placed.storage();
    let base = placed.descriptor_base();
    let mut mmio = MockRxDma::default();
    let prepared = placed.prepare_ring(&mut mmio).expect("prepared owner");
    let mut live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live epoch");
    let reserved = base + COUNT as u32 * crate::ieee80211::dma::descriptor::DESCRIPTOR_BYTES;
    assert_eq!(mmio.descriptor_base, reserved);
    storage.descriptors()[0].write_word0(
        crate::ieee80211::dma::descriptor::rx_armed_word(16)
            .expect("valid size")
            .with_rx_done(true),
    );
    mmio.last_descriptor_low = base & ADDRESS_LOW_MASK;
    mmio.next_descriptor_low =
        (base + crate::ieee80211::dma::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    let frozen = live.freeze_cursor(&mut mmio).unwrap();
    assert!(
        storage
            .take_completed(&mut live, frozen, 0)
            .unwrap()
            .is_some()
    );
    assert!(
        storage
            .recycle_completed_prefix::<1, _>(&mut live, &mut mmio)
            .unwrap()
            .is_none()
    );
    storage.descriptors()[1].write_word0(
        crate::ieee80211::dma::descriptor::rx_armed_word(16)
            .expect("valid size")
            .with_rx_done(true),
    );
    mmio.last_descriptor_low =
        (base + crate::ieee80211::dma::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    assert!(
        storage
            .recycle_completed_prefix::<1, _>(&mut live, &mut mmio)
            .unwrap()
            .is_some()
    );
    assert!(live.reload_pending());
    let publications = mmio.publications;

    // Old accepted tail is descriptor 3 and the pending tail is 0, but
    // hardware reports the earlier descriptor 1 with NEXT zero. The ring
    // rings the doorbell once more instead of writing BASE.
    mmio.next_descriptor_low = 0;
    mmio.last_descriptor_low =
        (base + crate::ieee80211::dma::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    assert_eq!(
        live.poll_pending_reload(&mut mmio),
        Ok(crate::ieee80211::dma::rx_ring::RxReloadObservation::Pending)
    );
    assert_eq!(mmio.publications, publications + 1, "one more doorbell");
    assert_eq!(mmio.descriptor_base, reserved, "BASE is not written again");
    assert_eq!(live.reload_evidence().reload_rerings, 1);

    // The doorbell resumed the walker at LAST.next.
    mmio.next_descriptor_low =
        (base + 2 * crate::ieee80211::dma::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    assert_eq!(
        live.poll_pending_reload(&mut mmio),
        Ok(crate::ieee80211::dma::rx_ring::RxReloadObservation::Settled)
    );
    assert_eq!(live.accepted_tail().get(), 0);
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::Live);
    assert!(live.try_stop(&mut mmio).is_ok());
}

const REPAIR_COUNT: usize = 2;
const ADDRESS_LOW_MASK: u32 = oer_esp32s31_pac::WifiDescriptorAddressLow::MAX;

/// A live two-descriptor ring whose walker exhausted the accepted tail
/// after its doorbell was rung while NEXT was still nonzero, so the settle
/// rang it once more and the walker resumed at the appended head (`base`).
fn ring_after_exhausted_reload_rering() -> (
    &'static RxDmaStorage<REPAIR_COUNT, 16, 20, HostDma>,
    RxRingLive<'static, REPAIR_COUNT>,
    crate::ieee80211::dma::rx_dma::model::RxWalkerModel,
    u32,
) {
    const COUNT: usize = REPAIR_COUNT;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let placed = place(storage);
    let storage = placed.storage();
    let base = placed.descriptor_base();
    let mut mmio = crate::ieee80211::dma::rx_dma::model::RxWalkerModel::new();
    let prepared = placed.prepare_ring(&mut mmio).expect("prepared owner");
    let mut live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live epoch");
    storage.descriptors()[0].write_word0(
        crate::ieee80211::dma::descriptor::rx_armed_word(16)
            .expect("valid size")
            .with_rx_done(true),
    );
    mmio.set_cursor(base, 0, Some(1));
    let frozen = live.freeze_cursor(&mut mmio).unwrap();
    assert!(
        storage
            .take_completed(&mut live, frozen, 0)
            .unwrap()
            .is_some()
    );
    assert!(
        storage
            .recycle_completed_prefix::<1, _>(&mut live, &mut mmio)
            .unwrap()
            .is_none()
    );
    storage.descriptors()[1].write_word0(
        crate::ieee80211::dma::descriptor::rx_armed_word(16)
            .expect("valid size")
            .with_rx_done(true),
    );
    // The walker still reports a descriptor of the old list when the
    // doorbell rings, so the doorbell resumes nothing.
    mmio.set_cursor(base, 1, Some(1));
    assert!(
        storage
            .recycle_completed_prefix::<1, _>(&mut live, &mut mmio)
            .unwrap()
            .is_some()
    );

    // It then exhausted the old tail before it followed the new link.
    mmio.clear_operations();
    mmio.set_cursor(base, 1, None);
    live.complete_pending_reload(&mut mmio, frozen_settle())
        .expect("the re-rung doorbell resumes the walker");
    (storage, live, mmio, base)
}

#[test]
fn exhausted_reload_records_the_rering_facts() {
    use crate::ieee80211::dma::rx_dma::model::RxWalkerOperation;
    let (storage, live, mut mmio, base) = ring_after_exhausted_reload_rering();
    assert_eq!(
        live.reload_evidence(),
        crate::ieee80211::dma::rx_ring::RxReloadEvidence {
            max_reload_attempts: 1,
            observations: 2,
            unknown_upper_with_zero_address: 0,
            reload_rerings: 1,
            rering_failures: 0,
            last_next_low: base & ADDRESS_LOW_MASK,
            last_last_low: None,
            base_writes: 1,
            reserved_landings: 0,
            reserved_late_landings: 0,
            walker_on_detached: 0,
        }
    );
    // A second doorbell, NEXT = LAST.next, and BASE untouched.
    assert_eq!(mmio.operation_count(RxWalkerOperation::RequestReload), 1);
    assert!(
        !mmio
            .operations()
            .iter()
            .any(|operation| matches!(operation, RxWalkerOperation::WriteDescriptorBase(_)))
    );
    assert_eq!(mmio.next_descriptor_low, base & ADDRESS_LOW_MASK);
    assert_eq!(
        mmio.descriptor_base,
        base + REPAIR_COUNT as u32 * crate::ieee80211::dma::descriptor::DESCRIPTOR_BYTES
    );
    assert_eq!(live.accepted_tail().get(), 0);
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::Live);
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn exhausted_list_reclaims_untouched_prefix_before_terminal_payload() {
    use crate::ieee80211::dma::rx_dma::model::{RxWalkerModel, RxWalkerOperation};
    const COUNT: usize = 4;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let placed = place(storage);
    let storage = placed.storage();
    let base = placed.descriptor_base();
    let mut mmio = RxWalkerModel::new();
    let prepared = placed.prepare_ring(&mut mmio).expect("prepared owner");
    let mut live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live epoch");
    let done = crate::ieee80211::dma::descriptor::rx_landed_word(16, 4, true);

    // Return descriptor zero while the old finite list has already
    // exhausted at descriptor three. The doorbell, rung with NEXT zero,
    // resumes the walker at three's new link: descriptor zero.
    storage.descriptors()[0].write_word0(done);
    mmio.set_cursor(base, 3, None);
    let frozen = live.freeze_cursor(&mut mmio).unwrap();
    let unit = storage
        .take_completed_unit(&mut live, frozen)
        .expect("completed descriptor")
        .expect("unit owner");
    unit.recycle(&mut mmio)
        .expect("live append")
        .expect("returned descriptor");
    live.complete_pending_reload(&mut mmio, frozen_settle())
        .expect("doorbell resumes the walker");
    assert_eq!(mmio.next_descriptor_low, base & ADDRESS_LOW_MASK);
    assert_eq!(live.recycle_start().get(), 1);
    assert_eq!(live.accepted_tail().get(), 0);
    assert_eq!(live.reload_evidence().reload_rerings, 0);

    // Hardware walked one and two without writing them, ended a frame in
    // three and exhausted at zero. NEXT=0/LAST=tail proves the untouched
    // links released: one unit of three descriptors, one short of the ring.
    storage.descriptors()[3].write_word0(done);
    storage.descriptors()[0].write_word0(done);
    mmio.set_cursor(base, 0, None);
    let frozen = live.freeze_cursor(&mut mmio).unwrap();
    let frontier = storage
        .first_completed_unit_frontier_through_cursor(&live, frozen)
        .expect("exhausted frontier");
    assert_eq!(frontier.unit_count, 1);
    assert_eq!(frontier.descriptor_count, COUNT - 1);

    let frozen = live.freeze_cursor(&mut mmio).unwrap();
    let unit = storage
        .take_completed_unit(&mut live, frozen)
        .expect("released unit")
        .expect("terminal payload owner");
    assert_eq!(unit.descriptor_count(), COUNT - 1);
    assert_eq!(unit.total_length(), 4);
    assert_eq!(unit.segment(0), Some(&[][..]));
    assert_eq!(unit.segment(1), Some(&[][..]));
    assert_eq!(unit.segment(2).map(<[u8]>::len), Some(4));
    let append = unit
        .recycle(&mut mmio)
        .expect("exhausted list recycle")
        .expect("unit republished");
    assert_eq!(append.head_index.get(), 1);
    assert_eq!(append.descriptor_count, COUNT - 1);
    live.complete_pending_reload(&mut mmio, frozen_settle())
        .expect("doorbell resumes the walker");
    assert_eq!(
        mmio.next_descriptor_low,
        (base + crate::ieee80211::dma::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK
    );
    assert_eq!(live.recycle_start().get(), 0);
    assert_eq!(live.accepted_tail().get(), 3);
    assert!(live.topology_snapshot().valid);
    assert_eq!(
        mmio.operation_count(RxWalkerOperation::WriteDescriptorBase(
            base + COUNT as u32 * crate::ieee80211::dma::descriptor::DESCRIPTOR_BYTES
        )),
        1,
        "the cold publication is the only BASE write"
    );
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn dropping_a_taken_completed_unit_requires_radio_reset() {
    const COUNT: usize = 2;
    const ADDRESS_LOW_MASK: u32 = oer_esp32s31_pac::WifiDescriptorAddressLow::MAX;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let placed = place(storage);
    let storage = placed.storage();
    let base = placed.descriptor_base();
    let mut mmio = MockRxDma::default();
    let prepared = placed.prepare_ring(&mut mmio).expect("prepared owner");
    let mut live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live epoch");
    storage.descriptors()[0].write_word0(
        crate::ieee80211::dma::descriptor::rx_armed_word(16)
            .expect("valid size")
            .with_rx_done(true),
    );
    mmio.last_descriptor_low = base & ADDRESS_LOW_MASK;
    mmio.next_descriptor_low =
        (base + crate::ieee80211::dma::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    let frozen = live.freeze_cursor(&mut mmio).unwrap();
    let unit = storage
        .take_completed_unit(&mut live, frozen)
        .unwrap()
        .expect("completed unit");

    drop(unit);
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::ResetRequired);
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn frozen_last_releases_the_descriptor_equal_to_last() {
    const COUNT: usize = 4;
    const ADDRESS_LOW_MASK: u32 = oer_esp32s31_pac::WifiDescriptorAddressLow::MAX;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let placed = place(storage);
    let storage = placed.storage();
    let base = placed.descriptor_base();
    let mut mmio = MockRxDma::default();
    let prepared = placed.prepare_ring(&mut mmio).expect("prepared owner");
    let mut live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live epoch");
    assert_eq!(live.recycle_start().get(), 0);
    assert_eq!(live.accepted_tail().get(), COUNT - 1);

    storage.descriptors()[0].write_word0(crate::ieee80211::dma::descriptor::rx_landed_word(
        16, 4, true,
    ));
    mmio.last_descriptor_low = base & ADDRESS_LOW_MASK;
    mmio.next_descriptor_low =
        (base + crate::ieee80211::dma::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    let frozen_cursor = live.freeze_cursor(&mut mmio).unwrap();
    let unit = storage
        .take_completed_unit(&mut live, frozen_cursor)
        .expect("completed descriptor")
        .expect("descriptor owner");
    let credit = unit
        .retain_discard()
        .expect("unit is at the recycle frontier");
    assert_eq!(credit.descriptor_count(), 1);
    let append = credit
        .append(&mut mmio)
        .expect("LAST itself releases the observed descriptor");
    assert_eq!(append.head_index.get(), 0);
    assert_eq!(append.descriptor_count, 1);
    assert!(!storage.descriptors()[0].word0().rx_done());
    assert!(live.observed_mask().is_empty());
    live.complete_pending_reload(&mut mmio, frozen_settle())
        .expect("vendor reload suffix");
    assert_eq!(live.accepted_tail().get(), 0);
    assert!(live.topology_snapshot().valid);
    assert!(live.try_stop(&mut mmio).is_ok());
}

/// Two completed single-descriptor frames, LAST on the first: the walker
/// still writes the second, so only the first unit can be taken.
fn two_completed_frames_last_on_the_first() -> (
    &'static RxDmaStorage<4, 16, 20, HostDma>,
    MockRxDma,
    RxRingLive<'static, 4>,
) {
    const ADDRESS_LOW_MASK: u32 = oer_esp32s31_pac::WifiDescriptorAddressLow::MAX;
    let storage = Box::leak(Box::new(RxDmaStorage::<4, 16, 20, HostDma>::new()));
    let placed = place(storage);
    let storage = placed.storage();
    let base = placed.descriptor_base();
    let mut mmio = MockRxDma::default();
    let live = placed
        .prepare_ring(&mut mmio)
        .expect("prepared owner")
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live epoch");
    for descriptor in &storage.descriptors()[..2] {
        descriptor.write_word0(crate::ieee80211::dma::descriptor::rx_landed_word(
            16, 4, true,
        ));
    }
    mmio.last_descriptor_low = base & ADDRESS_LOW_MASK;
    mmio.next_descriptor_low =
        (base + crate::ieee80211::dma::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    (storage, mmio, live)
}

#[test]
fn a_take_never_passes_the_frozen_last() {
    let (storage, mut mmio, mut live) = two_completed_frames_last_on_the_first();
    let cursor = live.freeze_cursor(&mut mmio).unwrap();
    let first = storage
        .take_completed_unit(&mut live, cursor)
        .unwrap()
        .expect("LAST itself is released");
    assert_eq!(first.head_index(), 0);
    first.retain_for_deferred_recycle();
    // Descriptor one carries RX_DONE, but it lies beyond the frozen LAST.
    assert!(
        storage
            .take_completed_unit(&mut live, cursor)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        storage.completed_unit_frontier_through_cursor(&live, cursor),
        Ok(RxCompletedUnitFrontier::default())
    );
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::Live);
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn a_descriptor_take_never_passes_the_frozen_last() {
    let (storage, mut mmio, mut live) = two_completed_frames_last_on_the_first();
    // Past LAST the walker may still be writing the descriptor: a torn
    // image there is no ring fault, because it is never read.
    let descriptor = &storage.descriptors()[1];
    descriptor.inject_corrupt_words(
        descriptor.word0().with_size(17).unwrap(),
        descriptor.buffer_address(),
        descriptor.next_address(),
    );
    let cursor = live.freeze_cursor(&mut mmio).unwrap();
    assert!(
        storage
            .take_completed(&mut live, cursor, 1)
            .unwrap()
            .is_none()
    );
    assert!(!live.observed_mask().contains(1));
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::Live);
    assert!(matches!(
        storage.take_completed(&mut live, cursor, 4),
        Err(RxRingError::Count)
    ));
    assert_eq!(
        storage
            .take_completed(&mut live, cursor, 0)
            .unwrap()
            .expect("LAST itself is released")
            .index(),
        0
    );
    let descriptor = &storage.descriptors()[1];
    descriptor.inject_corrupt_words(
        descriptor.word0().with_size(16).unwrap(),
        descriptor.buffer_address(),
        descriptor.next_address(),
    );
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn a_descriptor_take_refuses_a_cursor_of_an_earlier_live_epoch() {
    let (storage, mut mmio, mut live) = two_completed_frames_last_on_the_first();
    let earlier = live.freeze_cursor(&mut mmio).unwrap();
    let halted = live
        .try_stop(&mut mmio)
        .unwrap_or_else(|_| panic!("walker stops"));
    let mut live = storage
        .prepare_halted(halted, &mut mmio)
        .unwrap_or_else(|_| panic!("halted arena rebuilds"))
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("next live epoch");
    storage.descriptors()[0].write_word0(crate::ieee80211::dma::descriptor::rx_landed_word(
        16, 4, true,
    ));
    assert!(
        storage
            .take_completed(&mut live, earlier, 0)
            .unwrap()
            .is_none()
    );
    let current = live.freeze_cursor(&mut mmio).unwrap();
    assert!(
        storage
            .take_completed(&mut live, current, 0)
            .unwrap()
            .is_some()
    );
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn a_cursor_of_an_earlier_generation_takes_nothing() {
    let (storage, mut mmio, mut live) = two_completed_frames_last_on_the_first();
    let stale = live.freeze_cursor(&mut mmio).unwrap();
    storage
        .take_completed_unit(&mut live, stale)
        .unwrap()
        .expect("first unit")
        .retain_for_deferred_recycle();
    storage
        .claim_deferred_recycle(&mut live, stale)
        .and_then(|credit| {
            credit
                .map(|credit| {
                    assert_eq!(credit.descriptor_count(), 1);
                    credit.append(&mut mmio)
                })
                .transpose()
        })
        .unwrap()
        .expect("LAST releases descriptor zero");
    live.complete_pending_reload(&mut mmio, frozen_settle())
        .unwrap();
    // The append changed the generation. The walker has moved on to one.
    mmio.last_descriptor_low = mmio.next_descriptor_low;
    mmio.next_descriptor_low = 0;
    assert!(
        storage
            .take_completed_unit(&mut live, stale)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        storage.first_completed_unit_frontier_through_cursor(&live, stale),
        Ok(RxCompletedUnitFrontier::default())
    );
    let fresh = live.freeze_cursor(&mut mmio).unwrap();
    let second = storage
        .take_completed_unit(&mut live, fresh)
        .unwrap()
        .expect("a fresh cursor releases descriptor one");
    assert_eq!(second.head_index(), 1);
    second.retain_for_deferred_recycle();
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn a_cursor_of_an_earlier_live_epoch_takes_nothing() {
    let (storage, mut mmio, mut live) = two_completed_frames_last_on_the_first();
    let earlier = live.freeze_cursor(&mut mmio).unwrap();
    let halted = live
        .try_stop(&mut mmio)
        .unwrap_or_else(|_| panic!("walker stops"));
    let mut live = storage
        .prepare_halted(halted, &mut mmio)
        .unwrap_or_else(|_| panic!("halted arena rebuilds"))
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("next live epoch");
    // The rebuilt epoch restarts its generation count at zero on the same
    // arena; only the epoch tells the two cursors apart.
    storage.descriptors()[0].write_word0(crate::ieee80211::dma::descriptor::rx_landed_word(
        16, 4, true,
    ));
    assert!(
        storage
            .take_completed_unit(&mut live, earlier)
            .unwrap()
            .is_none()
    );
    let current = live.freeze_cursor(&mut mmio).unwrap();
    assert_ne!(current, earlier);
    storage
        .take_completed_unit(&mut live, current)
        .unwrap()
        .expect("the current epoch's cursor releases descriptor zero")
        .retain_for_deferred_recycle();
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn frozen_last_unit_reclaim_returns_only_one_vendor_chain() {
    const COUNT: usize = 4;
    const ADDRESS_LOW_MASK: u32 = oer_esp32s31_pac::WifiDescriptorAddressLow::MAX;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let placed = place(storage);
    let storage = placed.storage();
    let base = placed.descriptor_base();
    let mut mmio = MockRxDma::default();
    let prepared = placed.prepare_ring(&mut mmio).expect("prepared owner");
    let mut live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live epoch");

    for index in 0..2 {
        storage.descriptors()[index].write_word0(
            crate::ieee80211::dma::descriptor::rx_landed_word(16, 4, true),
        );
    }
    mmio.last_descriptor_low =
        (base + crate::ieee80211::dma::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    mmio.next_descriptor_low =
        (base + 2 * crate::ieee80211::dma::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    let cursor = live.freeze_cursor(&mut mmio).unwrap();

    let frozen = live.freeze_cursor(&mut mmio).unwrap();
    storage
        .take_completed_unit(&mut live, frozen)
        .expect("first unit inspection")
        .expect("first unit owner")
        .retain_for_deferred_recycle();
    let append = storage
        .claim_deferred_recycle(&mut live, cursor)
        .and_then(|credit| {
            credit
                .map(|credit| {
                    assert_eq!(credit.descriptor_count(), 1);
                    credit.append(&mut mmio)
                })
                .transpose()
        })
        .expect("first vendor unit reclaim")
        .expect("first unit precedes LAST");
    assert_eq!(append.head_index.get(), 0);
    assert_eq!(append.descriptor_count, 1);
    assert!(live.observed_mask().is_empty());
    assert!(
        storage.descriptors()[1].word0().rx_done(),
        "the following complete unit must remain a distinct vendor chain"
    );
    live.complete_pending_reload(&mut mmio, frozen_settle())
        .expect("first vendor reload suffix");

    let frozen = live.freeze_cursor(&mut mmio).unwrap();
    storage
        .take_completed_unit(&mut live, frozen)
        .expect("second unit inspection")
        .expect("second unit owner")
        .retain_for_deferred_recycle();
    assert!(
        storage
            .claim_deferred_recycle(&mut live, cursor)
            .and_then(|credit| credit
                .map(|credit| {
                    assert_eq!(credit.descriptor_count(), 1);
                    credit.append(&mut mmio)
                })
                .transpose())
            .expect("stale generation check")
            .is_none(),
        "one frozen cursor must not authorize a second append generation"
    );
    let refreshed = live.freeze_cursor(&mut mmio).unwrap();
    storage
        .claim_deferred_recycle(&mut live, refreshed)
        .and_then(|credit| {
            credit
                .map(|credit| {
                    assert_eq!(credit.descriptor_count(), 1);
                    credit.append(&mut mmio)
                })
                .transpose()
        })
        .expect("refreshed cursor check")
        .expect("refreshed LAST releases the second vendor unit");
    live.complete_pending_reload(&mut mmio, frozen_settle())
        .expect("second vendor reload suffix");
    assert!(live.topology_snapshot().valid);
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn ambiguous_start_quarantines_the_arena_when_walker_is_observed_live() {
    const COUNT: usize = 2;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let placed = place(storage);
    let storage = placed.storage();
    let mut mmio = MockRxDma {
        ambiguous_enable: true,
        ..MockRxDma::default()
    };
    let prepared = placed.prepare_ring(&mut mmio).expect("prepared owner");

    let (_prepared, error) = match prepared.try_start(&mut mmio) {
        Ok(_) => panic!("ambiguous enable is not a live capability"),
        Err(failure) => failure,
    };
    assert_eq!(error, RxRingError::WalkerStartUnconfirmed);
    assert!(mmio.walker);
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::ResetRequired);
}

#[test]
fn zeroed_arena_is_the_reusable_identity_bound_arena_new_builds() {
    let zeroed = oer_memory::zeroed::zeroed::<RxDmaStorage<4, 16, 20>>();
    let new = RxDmaStorage::<4, 16, 20, HostDma>::new();
    assert_eq!(zeroed.lifecycle_state(), RxDmaArenaState::Reusable);
    assert_eq!(zeroed.lifecycle_state(), new.lifecycle_state());
    for index in 0..4 {
        assert_eq!(zeroed.descriptor_buffer_id(index), Some(index));
        assert_eq!(
            zeroed.descriptor_buffer_id(index),
            new.descriptor_buffer_id(index)
        );
    }
    for (zeroed, new) in zeroed.descriptors().iter().zip(new.descriptors()) {
        assert_eq!(
            (
                zeroed.word0(),
                zeroed.buffer_address(),
                zeroed.next_address()
            ),
            (new.word0(), new.buffer_address(), new.next_address())
        );
    }
}

fn live_host_arena<const COUNT: usize>() -> (
    &'static RxDmaStorage<COUNT, 16, 20, HostDma>,
    &'static [u32; COUNT],
    MockRxDma,
    RxRingLive<'static, COUNT>,
) {
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let placed = place(storage);
    let storage = placed.storage();
    let buffers = storage.buffer_addresses();
    let mut mmio = MockRxDma::default();
    let live = placed
        .prepare_ring(&mut mmio)
        .expect("prepared owner")
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live epoch");
    (storage, buffers, mmio, live)
}

#[test]
fn host_dma_port_writes_only_inside_one_ring_buffer() {
    const COUNT: usize = 2;
    let (storage, buffers, mut mmio, mut live) = live_host_arena::<COUNT>();
    let base = live.descriptor_base();

    // A write inside a buffer lands there, where the CPU reads the
    // completed descriptor's segment.
    storage
        .dma_write(&live, buffers[1] + 4, &[1, 2, 3, 4])
        .unwrap();
    let mut read = [0; 4];
    storage.dma_read(&live, buffers[1] + 4, &mut read).unwrap();
    assert_eq!(read, [1, 2, 3, 4]);
    storage.descriptors()[1].write_word0(crate::ieee80211::dma::descriptor::rx_landed_word(
        16, 8, true,
    ));
    // The walker reports descriptor one as LAST: only that releases it.
    mmio.last_descriptor_low = (live.descriptor_base()
        + crate::ieee80211::dma::descriptor::DESCRIPTOR_BYTES)
        & 0x000f_ffff;
    let frozen = live.freeze_cursor(&mut mmio).unwrap();
    let completed = storage
        .take_completed(&mut live, frozen, 1)
        .unwrap()
        .expect("completed descriptor");
    assert_eq!(completed.segment().buffer[4..8], [1, 2, 3, 4]);

    let buffer_bytes = |live: &RxRingLive<'_, COUNT>| {
        buffers.map(|address| {
            let mut bytes = [0; 20];
            storage.dma_read(live, address, &mut bytes).unwrap();
            bytes
        })
    };
    let descriptor_words = || {
        storage.descriptors().each_ref().map(|descriptor| {
            (
                descriptor.word0(),
                descriptor.buffer_address(),
                descriptor.next_address(),
            )
        })
    };
    let (bytes_before, words_before) = (buffer_bytes(&live), descriptor_words());

    // Outside every buffer: below the arena, the descriptor array, and the
    // words after the last buffer.
    for address in [
        crate::ieee80211::dma::space::HOST_ARENA_BASE - 4,
        base,
        base + crate::ieee80211::dma::descriptor::DESCRIPTOR_BYTES,
        buffers[COUNT - 1] + 20,
    ] {
        assert_eq!(
            storage.dma_write(&live, address, &[0xff; 4]),
            Err(HostDmaAccessError::OutsideBuffers),
            "address {address:#x}"
        );
    }
    // Crossing the end of buffer zero into buffer one.
    assert_eq!(
        storage.dma_write(&live, buffers[0] + 18, &[0xff; 4]),
        Err(HostDmaAccessError::OutsideBuffers)
    );
    assert_eq!(buffer_bytes(&live), bytes_before);
    assert_eq!(descriptor_words(), words_before);
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn host_dma_port_refuses_a_foreign_ring_and_a_detached_buffer() {
    const COUNT: usize = 2;
    let (storage, buffers, mut mmio, mut live) = live_host_arena::<COUNT>();
    let (_, _, mut other_mmio, other) = live_host_arena::<COUNT>();
    assert_eq!(
        storage.dma_write(&other, buffers[0], &[0xff]),
        Err(HostDmaAccessError::ForeignRing)
    );

    storage.descriptors()[0].write_word0(crate::ieee80211::dma::descriptor::rx_landed_word(
        16, 8, true,
    ));
    // The walker reports descriptor zero as LAST: only that releases it.
    mmio.last_descriptor_low = live.descriptor_base() & 0x000f_ffff;
    let frozen = live.freeze_cursor(&mut mmio).unwrap();
    let detached = storage
        .take_completed_unit(&mut live, frozen)
        .unwrap()
        .expect("completed unit")
        .detach_single(&mut mmio)
        .expect("single descriptor detaches");
    assert_eq!(
        storage.dma_write(&live, buffers[0], &[0xff]),
        Err(HostDmaAccessError::NotInRing)
    );
    let mut read = [0; 1];
    assert_eq!(
        storage.dma_read(&live, buffers[0], &mut read),
        Err(HostDmaAccessError::NotInRing)
    );
    drop(detached);
    assert!(live.try_stop(&mut mmio).is_ok());
    assert!(other.try_stop(&mut other_mmio).is_ok());
}

/// A forwarding layer which stops the walker for another arena's binding.
struct ForeignStopProof {
    walker: crate::ieee80211::dma::rx_dma::model::RxWalkerModel,
    foreign: &'static RxDmaBinding<'static>,
}

impl ForeignStopProof {
    fn new() -> Self {
        let other = place(Box::leak(Box::new(
            RxDmaStorage::<2, 16, 20, HostDma>::new(),
        )));
        let base = other.descriptor_base();
        let other = other.storage();
        let foreign = RxDmaBinding::new(&other.descriptors, base, &other.arena).unwrap();
        Self {
            walker: crate::ieee80211::dma::rx_dma::model::RxWalkerModel::new(),
            foreign: Box::leak(Box::new(foreign)),
        }
    }
}

impl crate::ieee80211::dma::rx_dma::sealed::Sealed for ForeignStopProof {}

impl RxDma for ForeignStopProof {
    fn last_descriptor_low(&mut self) -> u32 {
        self.walker.last_descriptor_low()
    }

    fn next_descriptor_low(&mut self) -> u32 {
        self.walker.next_descriptor_low()
    }

    fn next_descriptor(&mut self) -> crate::ieee80211::dma::rx_dma::RxDmaNextDescriptor {
        self.walker.next_descriptor()
    }

    fn with_ordered_cursor<R>(
        &mut self,
        observed: impl for<'confirmation> FnOnce(
            crate::ieee80211::dma::rx_dma::RxDmaCursorObservation<'confirmation>,
        ) -> R,
    ) -> R {
        self.walker.with_ordered_cursor(observed)
    }

    fn walker_enabled(&mut self) -> bool {
        self.walker.walker_enabled()
    }

    fn reload_pending(&mut self) -> bool {
        self.walker.reload_pending()
    }

    fn try_with_reload_settled<R>(
        &mut self,
        settled: impl for<'confirmation> FnOnce(
            crate::ieee80211::dma::rx_dma::RxDmaReloadSettled<'confirmation>,
        ) -> R,
    ) -> Option<R> {
        self.walker.try_with_reload_settled(settled)
    }

    fn configure_descriptor_window(&mut self, binding: &RxDmaBinding<'_>) {
        self.walker.configure_descriptor_window(binding);
    }

    fn write_descriptor_base(&mut self, binding: &RxDmaBinding<'_>) {
        self.walker.write_descriptor_base(binding);
    }

    fn publish_walker_enable(&mut self, binding: &RxDmaBinding<'_>) {
        self.walker.publish_walker_enable(binding);
    }

    fn request_reload(&mut self, binding: &RxDmaBinding<'_>) {
        self.walker.request_reload(binding);
    }

    fn try_with_walker_enabled<R>(
        &mut self,
        binding: &RxDmaBinding<'_>,
        enabled: impl for<'confirmation> FnOnce(
            crate::ieee80211::dma::rx_dma::RxDmaWalkerEnabled<'confirmation>,
        ) -> R,
    ) -> Option<R> {
        self.walker.try_with_walker_enabled(binding, enabled)
    }

    fn try_with_walker_stopped<R>(
        &mut self,
        _: &RxDmaBinding<'_>,
        stopped: impl for<'confirmation> FnOnce(
            crate::ieee80211::dma::rx_dma::RxDmaWalkerStopped<'confirmation>,
        ) -> R,
    ) -> Option<R> {
        self.walker.try_with_walker_stopped(self.foreign, stopped)
    }

    fn fence(&mut self) {
        self.walker.fence();
    }
}

#[test]
fn a_stop_proof_for_another_arena_poisons_the_live_arena() {
    let storage = Box::leak(Box::new(RxDmaStorage::<2, 16, 20, HostDma>::new()));
    let placed = place(storage);
    let storage = placed.storage();
    let mut mmio = ForeignStopProof::new();
    let live = placed
        .prepare_ring(&mut mmio)
        .unwrap()
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .unwrap();

    let (live, error) = live
        .try_stop(&mut mmio)
        .err()
        .expect("a foreign stop proof must not halt this ring");
    assert_eq!(error, RxRingError::Corrupt);
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::ResetRequired);
    drop(live);
}

#[test]
fn a_halted_rebuild_refuses_a_stop_proof_for_another_arena() {
    let storage = Box::leak(Box::new(RxDmaStorage::<2, 16, 20, HostDma>::new()));
    let placed = place(storage);
    let storage = placed.storage();
    let mut model = crate::ieee80211::dma::rx_dma::model::RxWalkerModel::new();
    let halted = placed
        .prepare_ring(&mut model)
        .unwrap()
        .try_start(&mut model)
        .map_err(|(_, error)| error)
        .unwrap()
        .try_stop(&mut model)
        .map_err(|(_, error)| error)
        .unwrap();

    // Something started the walker again; the rebuild must stop it first,
    // and a proof for another arena's binding does not prove that.
    let mut mmio = ForeignStopProof::new();
    mmio.walker.walker_enabled = true;
    let (_halted, error) = storage
        .prepare_halted(halted, &mut mmio)
        .err()
        .expect("a foreign stop proof must not authorize a rebuild");
    assert_eq!(error, RxRingError::Corrupt);
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::ResetRequired);
}

#[test]
fn a_walker_that_does_not_stop_returns_the_live_owner_unconfirmed() {
    use crate::ieee80211::dma::rx_dma::model::{RxWalkerModel, RxWalkerOperation};
    let storage = Box::leak(Box::new(RxDmaStorage::<2, 16, 20, HostDma>::new()));
    let placed = place(storage);
    let storage = placed.storage();
    let mut mmio = RxWalkerModel::new();
    let live = placed
        .prepare_ring(&mut mmio)
        .unwrap()
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .unwrap();

    mmio.refuse_stop = true;
    let (live, error) = live
        .try_stop(&mut mmio)
        .err()
        .expect("an unconfirmed stop keeps the live owner");
    assert_eq!(error, RxRingError::WalkerStopUnconfirmed);
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::Live);
    assert!(mmio.walker_enabled);

    mmio.refuse_stop = false;
    assert!(live.try_stop(&mut mmio).is_ok());
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::Prepared);
    assert_eq!(mmio.operation_count(RxWalkerOperation::Stop), 2);
}

#[test]
fn a_chain_longer_than_the_length_field_is_owned_and_recycled() {
    use crate::ieee80211::dma::rx_dma::model::RxWalkerModel;
    const COUNT: usize = 4;
    const BUFFER: u32 = 8_192;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 8_192, 8_196, HostDma>::new()));
    let placed = place(storage);
    let storage = placed.storage();
    let base = placed.descriptor_base();
    let buffers = *storage.buffer_addresses();
    let mut mmio = RxWalkerModel::new();
    let mut live = placed
        .prepare_ring(&mut mmio)
        .unwrap()
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .unwrap();

    // Two full segments and a 100-byte terminal: 16 484 bytes, more than
    // the 14-bit length field holds.
    for (descriptor, &buffer) in storage.descriptors().iter().zip(&buffers).take(2) {
        storage
            .dma_write(&live, buffer, &[0; 4])
            .expect("the walker filled the segment");
        descriptor.write_word0(crate::ieee80211::dma::descriptor::rx_landed_word(
            BUFFER, BUFFER, false,
        ));
    }
    storage.descriptors()[2].write_word0(crate::ieee80211::dma::descriptor::rx_landed_word(
        BUFFER, 100, true,
    ));
    mmio.set_cursor(base, 2, Some(3));
    let cursor = live.freeze_cursor(&mut mmio).unwrap();
    let frontier = storage
        .completed_unit_frontier_through_cursor(&live, cursor)
        .unwrap();
    assert_eq!(frontier.descriptor_count, 3);

    let unit = storage
        .take_completed_unit(&mut live, cursor)
        .expect("an oversized chain is air input, not a ring fault")
        .expect("the complete chain is owned");
    assert_eq!(unit.descriptor_count(), 3);
    assert_eq!(unit.total_length(), 2 * BUFFER as usize + 100);
    assert!(
        crate::ieee80211::dma::descriptor::rx_armed_word(
            u32::try_from(unit.total_length()).unwrap()
        )
        .is_none(),
        "no descriptor word holds the chain's length"
    );
    assert_eq!(unit.metadata().staged_word0(), None);
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::Live);

    // Its consumer drops it as it drops any chained unit.
    unit.retain_for_deferred_recycle();
    let append = storage
        .claim_deferred_recycle(&mut live, cursor)
        .and_then(|credit| {
            credit
                .map(|credit| {
                    assert_eq!(credit.descriptor_count(), 3);
                    credit.append(&mut mmio)
                })
                .transpose()
        })
        .unwrap()
        .expect("the frozen LAST releases the chain");
    assert_eq!(append.head_index.get(), 0);
    assert_eq!(append.descriptor_count, 3);
    live.complete_pending_reload(&mut mmio, frozen_settle())
        .unwrap();
    assert!(live.observed_mask().is_empty());
    for index in 0..3 {
        assert_eq!(
            storage.descriptors()[index].word0(),
            crate::ieee80211::dma::descriptor::rx_armed_word(BUFFER).unwrap(),
            "descriptor {index} is rearmed"
        );
    }
    assert!(!storage.buffers()[0].leading_guard_overwritten());
    assert!(live.topology_snapshot().valid);
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::Live);
    assert!(live.try_stop(&mut mmio).is_ok());
}

/// A live ring with one appended descriptor whose reload is pending.
fn ring_with_pending_reload(
    mmio: &mut crate::ieee80211::dma::rx_dma::model::RxWalkerModel,
) -> (
    &'static RxDmaStorage<2, 16, 20, HostDma>,
    RxRingLive<'static, 2>,
) {
    let storage = Box::leak(Box::new(RxDmaStorage::<2, 16, 20, HostDma>::new()));
    let placed = place(storage);
    let storage = placed.storage();
    let base = placed.descriptor_base();
    let mut live = placed
        .prepare_ring(mmio)
        .unwrap()
        .try_start(mmio)
        .map_err(|(_, error)| error)
        .unwrap();
    storage.descriptors()[0].write_word0(
        crate::ieee80211::dma::descriptor::rx_armed_word(16)
            .unwrap()
            .with_rx_done(true),
    );
    mmio.set_cursor(base, 0, Some(1));
    let cursor = live.freeze_cursor(mmio).unwrap();
    storage
        .take_completed_unit(&mut live, cursor)
        .unwrap()
        .unwrap()
        .retain_for_deferred_recycle();
    storage
        .claim_deferred_recycle(&mut live, cursor)
        .and_then(|credit| {
            credit
                .map(|credit| {
                    assert_eq!(credit.descriptor_count(), 1);
                    credit.append(mmio)
                })
                .transpose()
        })
        .unwrap()
        .expect("the frozen LAST releases descriptor zero");
    assert!(live.reload_pending());
    (storage, live)
}

#[test]
fn a_reload_doorbell_that_never_clears_stays_pending_after_the_limit() {
    use crate::ieee80211::dma::rx_dma::model::{RxWalkerModel, RxWalkerOperation};
    use core::cell::Cell;

    /// Each read is one microsecond later.
    struct Stepping(Cell<u64>);

    impl Clock for Stepping {
        fn now(&self) -> Instant {
            let now = self.0.get();
            self.0.set(now + 1);
            Instant::from_micros(now)
        }
    }

    let mut mmio = RxWalkerModel::new();
    let (storage, mut live) = ring_with_pending_reload(&mut mmio);
    mmio.reload_stuck = true;
    mmio.clear_operations();

    let clock = Stepping(Cell::new(100));
    let settle = Settle::new(SettleLimit::new(Duration::from_micros(5)).unwrap(), &clock);
    assert_eq!(
        live.complete_pending_reload(&mut mmio, settle),
        Err(RxRingError::ReloadPending)
    );
    // The start read is 100; the checks read 101..=105, and 105 ends it.
    assert_eq!(live.reload_evidence().max_reload_attempts, 5);
    assert!(live.reload_pending(), "the append stays pending");
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::Live);

    mmio.reload_stuck = false;
    mmio.clear_operations();
    assert_eq!(
        live.complete_pending_reload(&mut mmio, frozen_settle()),
        Ok(())
    );
    assert_eq!(
        mmio.operation_count(RxWalkerOperation::ObserveReloadPending),
        1
    );
    assert!(!live.reload_pending());
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn a_settle_limit_is_nonzero_and_at_most_twenty_microseconds() {
    assert_eq!(SettleLimit::new(Duration::from_micros(0)), None);
    assert_eq!(SettleLimit::new(Duration::from_micros(21)), None);
    assert!(SettleLimit::new(Duration::from_micros(20)).is_some());
}

#[test]
fn reload_attempts_record_the_largest_observation_count() {
    use crate::ieee80211::dma::rx_dma::model::RxWalkerModel;
    let mut mmio = RxWalkerModel::new();
    mmio.reload_latency = 3;
    let (storage, mut live) = ring_with_pending_reload(&mut mmio);
    assert_eq!(live.reload_evidence().max_reload_attempts, 0);

    live.complete_pending_reload(&mut mmio, frozen_settle())
        .unwrap();
    assert_eq!(live.reload_evidence().max_reload_attempts, 4);
    assert!(!live.reload_pending());

    // Descriptor one completes and is appended behind descriptor zero; its
    // quicker reload does not lower the epoch's maximum.
    mmio.reload_latency = 1;
    storage.descriptors()[1].write_word0(
        crate::ieee80211::dma::descriptor::rx_armed_word(16)
            .unwrap()
            .with_rx_done(true),
    );
    mmio.set_cursor(live.descriptor_base(), 1, Some(0));
    let cursor = live.freeze_cursor(&mut mmio).unwrap();
    storage
        .take_completed_unit(&mut live, cursor)
        .unwrap()
        .unwrap()
        .retain_for_deferred_recycle();
    storage
        .claim_deferred_recycle(&mut live, cursor)
        .and_then(|credit| {
            credit
                .map(|credit| {
                    assert_eq!(credit.descriptor_count(), 1);
                    credit.append(&mut mmio)
                })
                .transpose()
        })
        .unwrap()
        .expect("the frozen LAST releases descriptor one");
    assert!(live.reload_pending());
    live.complete_pending_reload(&mut mmio, frozen_settle())
        .unwrap();
    assert!(!live.reload_pending());
    assert_eq!(live.reload_evidence().max_reload_attempts, 4);
    assert!(live.try_stop(&mut mmio).is_ok());
}

/// A live host ring whose walker model plays the DMA engine: frames land
/// where NEXT points and the walker follows each descriptor's link.
struct Walked<const COUNT: usize> {
    storage: &'static RxDmaStorage<COUNT, 16, 20, HostDma>,
    base: u32,
    walker: crate::ieee80211::dma::rx_dma::model::RxWalkerModel,
    live: RxRingLive<'static, COUNT>,
}

impl<const COUNT: usize> Walked<COUNT> {
    fn start() -> Self {
        let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
        let placed = place(storage);
        let storage = placed.storage();
        let base = placed.descriptor_base();
        let mut walker = crate::ieee80211::dma::rx_dma::model::RxWalkerModel::new();
        let live = placed
            .prepare_ring(&mut walker)
            .unwrap()
            .try_start(&mut walker)
            .map_err(|(_, error)| error)
            .unwrap();
        Self {
            storage,
            base,
            walker,
            live,
        }
    }

    fn reserved_address(&self) -> u32 {
        self.base + COUNT as u32 * crate::ieee80211::dma::descriptor::DESCRIPTOR_BYTES
    }

    /// Land one `length`-byte frame (`terminal`: its last segment) where
    /// NEXT points, as the model walker does; whether it was written.
    fn land(&mut self, length: u32, terminal: bool) -> bool {
        let table = self.storage.walker_table().unwrap();
        let crate::ieee80211::dma::rx_dma::model::RxWalkerLanding::Landed(low) =
            self.walker.land(table, length, terminal)
        else {
            return false;
        };
        let descriptor = table.descriptor_at_low(low).unwrap();
        self.storage
            .dma_write(&self.live, descriptor.buffer_address(), &[0xa5; 4])
            .unwrap();
        true
    }

    fn base_writes(&self) -> usize {
        self.walker
            .operations()
            .iter()
            .filter(|operation| {
                matches!(
                    operation,
                    crate::ieee80211::dma::rx_dma::model::RxWalkerOperation::WriteDescriptorBase(_)
                )
            })
            .count()
    }
}

#[test]
fn base_names_only_the_reserved_descriptor_once_per_epoch_across_full_ring_releases() {
    const COUNT: usize = 4;
    let mut ring = Walked::<COUNT>::start();
    assert_eq!(
        ring.walker.operation_count(
            crate::ieee80211::dma::rx_dma::model::RxWalkerOperation::WriteDescriptorBase(
                ring.reserved_address()
            )
        ),
        1
    );
    assert_eq!(ring.walker.descriptor_base, ring.reserved_address());
    ring.walker.clear_operations();
    let pool = oer_memory::ExternalRxHandoffPool::<16, COUNT>::new();

    // The epoch's first frame lands in R.
    assert!(ring.land(4, true));
    ring.storage
        .observe_reserved(&mut ring.live, &mut ring.walker)
        .unwrap();
    for round in 0..24 {
        // Frames fill every armed descriptor until the walker runs out.
        while ring.land(4, true) {}
        assert_eq!(ring.walker.next_descriptor_low, 0, "round {round}");
        let mut leases = std::vec::Vec::new();
        let frozen = ring.live.freeze_cursor(&mut ring.walker).unwrap();
        while let Some(unit) = ring
            .storage
            .take_completed_unit(&mut ring.live, frozen)
            .unwrap()
        {
            let detached = unit.detach_single(&mut ring.walker).unwrap();
            let slot = leases.len();
            let radio = pool
                .try_claim_radio(detached.into_buffer(), slot)
                .map_err(drop)
                .unwrap();
            let length = radio.frame().len();
            leases.push(pool.claim_network(radio.republish(0, length)));
        }
        assert!(
            !ring.live.observed_mask().contains(COUNT),
            "R is never observed"
        );
        // Every lease returns: the whole ring is released at once.
        drop(leases);
        assert_eq!(ring.storage.released_buffer_count(), COUNT);
        let append = ring
            .storage
            .recycle_released_prefix::<COUNT, _>(&mut ring.live, &mut ring.walker)
            .unwrap()
            .expect("all but the walker's LAST append");
        assert_eq!(append.descriptor_count, COUNT - 1, "round {round}");
        ring.live
            .complete_pending_reload(&mut ring.walker, frozen_settle())
            .unwrap();
        // The doorbell resumed the walker at the old tail's new link.
        assert_eq!(
            ring.walker.next_descriptor_low,
            append.head_address & ADDRESS_LOW_MASK
        );
        // The old LAST stays released until the walker moves past it.
        assert_eq!(ring.storage.released_buffer_count(), 1);
        assert_eq!(ring.live.observed_mask().count_ones(), 1);
        ring.storage
            .observe_reserved(&mut ring.live, &mut ring.walker)
            .unwrap();
        assert_eq!(ring.base_writes(), 0, "round {round}");
        ring.walker.clear_operations();
        // The next round's first frame moves the walker on; the old LAST
        // rejoins with the next release.
    }
    let evidence = ring.live.reload_evidence();
    assert_eq!(evidence.base_writes, 1);
    assert_eq!(evidence.reserved_landings, 1);
    assert_eq!(evidence.reserved_late_landings, 0);
    assert_eq!(evidence.reload_rerings, 0);
    assert_eq!(ring.storage.reserved_descriptor().next_address(), 0);
    assert_eq!(ring.storage.lifecycle_state(), RxDmaArenaState::Live);
    assert!(ring.live.topology_snapshot().valid);
    assert!(ring.live.try_stop(&mut ring.walker).is_ok());
}

#[test]
fn a_cold_landing_is_dropped_and_r_next_clears_only_at_the_first_ring_take() {
    const COUNT: usize = 4;
    let mut ring = Walked::<COUNT>::start();
    let reserved = ring.storage.reserved_descriptor();
    let armed = crate::ieee80211::dma::descriptor::rx_armed_word(16).unwrap();
    assert_eq!(reserved.next_address(), ring.base);
    assert_eq!(reserved.word0(), armed);

    // The first frame lands in R: counted, dropped, R rearmed; its link
    // still leads the walker to the ring head.
    assert!(ring.land(4, true));
    assert_eq!(
        ring.walker.last_descriptor_low,
        ring.reserved_address() & ADDRESS_LOW_MASK
    );
    let frozen = ring.live.freeze_cursor(&mut ring.walker).unwrap();
    assert_eq!(
        ring.storage
            .first_completed_unit_frontier_through_cursor(&ring.live, frozen,)
            .unwrap()
            .unit_count,
        0,
        "LAST naming R releases no ring descriptor"
    );
    ring.storage
        .observe_reserved(&mut ring.live, &mut ring.walker)
        .unwrap();
    assert_eq!(ring.live.reload_evidence().reserved_landings, 1);
    assert_eq!(reserved.word0(), armed);
    assert!(!ring.storage.reserved_buffer.leading_guard_overwritten());
    assert_eq!(
        reserved.next_address(),
        ring.base,
        "not cleared before a take"
    );
    assert!(!ring.live.reserved_left());

    // The next frame lands in the head; until it is taken, R keeps its link.
    assert!(ring.land(4, true));
    ring.storage
        .observe_reserved(&mut ring.live, &mut ring.walker)
        .unwrap();
    assert_eq!(reserved.next_address(), ring.base);
    let frozen = ring.live.freeze_cursor(&mut ring.walker).unwrap();
    let unit = ring
        .storage
        .take_completed_unit(&mut ring.live, frozen)
        .unwrap()
        .expect("the head's unit");
    assert!(!unit.metadata().continues_reserved_landing());
    unit.retain_for_deferred_recycle();
    assert_eq!(reserved.next_address(), 0, "cleared by the first take");
    assert!(ring.live.reserved_left());
    assert_eq!(ring.live.reload_evidence().reserved_landings, 1);

    // Later takes leave it cleared and count nothing.
    assert!(ring.land(4, true));
    let frozen = ring.live.freeze_cursor(&mut ring.walker).unwrap();
    ring.storage
        .take_completed_unit(&mut ring.live, frozen)
        .unwrap()
        .expect("the second unit")
        .retain_for_deferred_recycle();
    assert_eq!(reserved.next_address(), 0);
    ring.storage
        .observe_reserved(&mut ring.live, &mut ring.walker)
        .unwrap();
    let evidence = ring.live.reload_evidence();
    assert_eq!(
        (evidence.reserved_landings, evidence.reserved_late_landings),
        (1, 0)
    );
    assert!(ring.live.try_stop(&mut ring.walker).is_ok());
}

#[test]
fn a_frame_continuing_from_r_into_the_head_is_taken_as_its_tail() {
    const COUNT: usize = 4;
    let mut ring = Walked::<COUNT>::start();
    // R holds the frame's first segment (no RX_DONE), the head its end.
    assert!(ring.land(16, false));
    assert!(ring.land(8, true));
    // A frame still being written into R is left alone.
    ring.storage
        .observe_reserved(&mut ring.live, &mut ring.walker)
        .unwrap();
    assert_eq!(ring.live.reload_evidence().reserved_landings, 0);
    let frozen = ring.live.freeze_cursor(&mut ring.walker).unwrap();
    let unit = ring
        .storage
        .take_completed_unit(&mut ring.live, frozen)
        .unwrap()
        .expect("the head's unit");
    assert!(unit.metadata().continues_reserved_landing());
    unit.retain_for_deferred_recycle();
    assert_eq!(ring.live.reload_evidence().reserved_landings, 1);
    assert!(!ring.storage.reserved_buffer.leading_guard_overwritten());
    assert_eq!(ring.storage.reserved_descriptor().next_address(), 0);
    ring.storage
        .observe_reserved(&mut ring.live, &mut ring.walker)
        .unwrap();
    assert_eq!(ring.storage.lifecycle_state(), RxDmaArenaState::Live);
    assert!(ring.live.try_stop(&mut ring.walker).is_ok());
}

#[test]
fn a_late_landing_in_r_is_counted_and_fails_closed() {
    const COUNT: usize = 4;
    for late in 0..3 {
        let mut ring = Walked::<COUNT>::start();
        assert!(ring.land(4, true));
        assert!(ring.land(4, true));
        let frozen = ring.live.freeze_cursor(&mut ring.walker).unwrap();
        ring.storage
            .take_completed_unit(&mut ring.live, frozen)
            .unwrap()
            .expect("the head's unit")
            .retain_for_deferred_recycle();
        assert!(ring.live.reserved_left());
        let reserved = ring.storage.reserved_descriptor();
        match late {
            0 => reserved.write_word0(reserved.word0().with_rx_done(true)),
            1 => ring
                .storage
                .dma_write(&ring.live, reserved.buffer_address(), &[0xa5; 4])
                .unwrap(),
            _ => ring.walker.last_descriptor_low = ring.reserved_address() & ADDRESS_LOW_MASK,
        }
        assert_eq!(
            ring.storage
                .observe_reserved(&mut ring.live, &mut ring.walker),
            Err(RxRingError::Corrupt),
            "late landing {late}"
        );
        let evidence = ring.live.reload_evidence();
        assert_eq!(evidence.reserved_late_landings, 1);
        assert_eq!(evidence.reserved_landings, 1, "the epoch's first frame");
        assert_eq!(
            ring.storage.lifecycle_state(),
            RxDmaArenaState::ResetRequired
        );
        assert!(ring.live.try_stop(&mut ring.walker).is_ok());
    }
}

#[test]
fn a_retained_next_naming_r_is_a_cold_start() {
    const COUNT: usize = 4;
    let mut ring = Walked::<COUNT>::start();
    let reserved = ring.reserved_address();
    // Stopped while NEXT named R: the walker resumes at R.
    assert_eq!(ring.walker.next_descriptor_low, reserved & ADDRESS_LOW_MASK);
    let halted = ring
        .live
        .try_stop(&mut ring.walker)
        .map_err(|(_, error)| error)
        .unwrap();
    ring.walker.clear_operations();
    let stopped = ring
        .storage
        .prepare_halted(halted, &mut ring.walker)
        .map_err(|(_, error)| error)
        .expect("NEXT = R is a cold start, not a corrupt cursor");
    assert_eq!(stopped.initial_start().get(), 0);
    assert_eq!(stopped.retained_next_low(), reserved & ADDRESS_LOW_MASK);
    assert_eq!(stopped.reserved_snapshot().next_address, ring.base);
    assert_eq!(
        ring.walker
            .operations()
            .iter()
            .filter(|operation| **operation
                == crate::ieee80211::dma::rx_dma::model::RxWalkerOperation::WriteDescriptorBase(
                    reserved
                ))
            .count(),
        1
    );
    let live = stopped
        .try_start(&mut ring.walker)
        .map_err(|(_, error)| error)
        .unwrap();
    assert_eq!(
        live.reload_evidence().base_writes,
        1,
        "a new epoch's one write"
    );
    assert!(live.try_stop(&mut ring.walker).is_ok());
}

/// A ring whose whole list was consumed (every descriptor completed, the
/// walker exhausted at the accepted tail) with descriptor 0 appended behind
/// the tail; the model loses its first `lost` doorbells.
fn exhausted_with_head_returned(lost: u32) -> Walked<4> {
    let mut ring = Walked::<4>::start();
    ring.walker.lose_reloads = lost;
    ring.walker.next_descriptor_low = ring.base & ADDRESS_LOW_MASK;
    while ring.land(4, true) {}
    ring.walker.clear_operations();
    let frozen = ring.live.freeze_cursor(&mut ring.walker).unwrap();
    ring.storage
        .take_completed_unit(&mut ring.live, frozen)
        .unwrap()
        .expect("the head's unit")
        .recycle(&mut ring.walker)
        .unwrap()
        .expect("descriptor 0 appends behind the exhausted tail");
    ring
}

#[test]
fn a_lost_doorbell_is_rerung_once_and_counted() {
    use crate::ieee80211::dma::rx_dma::model::RxWalkerOperation;
    let mut ring = exhausted_with_head_returned(1);
    ring.live
        .complete_pending_reload(&mut ring.walker, frozen_settle())
        .unwrap();
    assert_eq!(
        ring.walker
            .operation_count(RxWalkerOperation::RequestReload),
        2
    );
    assert_eq!(ring.base_writes(), 0);
    assert_eq!(
        ring.walker.next_descriptor_low,
        ring.base & ADDRESS_LOW_MASK
    );
    let evidence = ring.live.reload_evidence();
    assert_eq!((evidence.reload_rerings, evidence.rering_failures), (1, 0));
    assert_eq!(ring.live.accepted_tail().get(), 0);
    assert_eq!(ring.storage.lifecycle_state(), RxDmaArenaState::Live);
    assert!(ring.live.try_stop(&mut ring.walker).is_ok());

    // Without a lost doorbell nothing is re-rung.
    let mut ring = exhausted_with_head_returned(0);
    ring.live
        .complete_pending_reload(&mut ring.walker, frozen_settle())
        .unwrap();
    assert_eq!(
        ring.walker
            .operation_count(RxWalkerOperation::RequestReload),
        1
    );
    assert_eq!(ring.live.reload_evidence().reload_rerings, 0);
    assert!(ring.live.try_stop(&mut ring.walker).is_ok());
}

#[test]
fn doorbells_that_never_resume_the_walker_fail_closed_after_one_rering() {
    use crate::ieee80211::dma::rx_dma::model::RxWalkerOperation;
    let mut ring = exhausted_with_head_returned(u32::MAX);
    assert_eq!(
        ring.live
            .complete_pending_reload(&mut ring.walker, frozen_settle()),
        Err(RxRingError::Corrupt)
    );
    assert_eq!(
        ring.walker
            .operation_count(RxWalkerOperation::RequestReload),
        2
    );
    let evidence = ring.live.reload_evidence();
    assert_eq!((evidence.reload_rerings, evidence.rering_failures), (1, 1));
    assert_eq!(ring.base_writes(), 0);
    assert_eq!(
        ring.storage.lifecycle_state(),
        RxDmaArenaState::ResetRequired
    );
    assert!(ring.live.try_stop(&mut ring.walker).is_ok());
}

#[test]
fn a_unit_spanning_the_whole_ring_is_rejected() {
    const COUNT: usize = 4;
    let mut ring = Walked::<COUNT>::start();
    // The walker passed three descriptors unwritten and ended a frame in the
    // accepted tail: NEXT=0/LAST=tail releases one unit of all four.
    let done = crate::ieee80211::dma::descriptor::rx_landed_word(16, 4, true);
    ring.storage.descriptors()[COUNT - 1].write_word0(done);
    ring.walker.set_cursor(ring.base, COUNT - 1, None);
    let frozen = ring.live.freeze_cursor(&mut ring.walker).unwrap();
    let frontier = ring
        .storage
        .first_completed_unit_frontier_through_cursor(&ring.live, frozen)
        .unwrap();
    assert_eq!(frontier.descriptor_count, COUNT);
    let frozen = ring.live.freeze_cursor(&mut ring.walker).unwrap();
    let unit = ring
        .storage
        .take_completed_unit(&mut ring.live, frozen)
        .unwrap()
        .expect("the whole-ring unit");
    assert_eq!(unit.recycle(&mut ring.walker), Err(RxRingError::Corrupt));
    assert_eq!(
        ring.storage.lifecycle_state(),
        RxDmaArenaState::ResetRequired
    );
    assert_eq!(ring.base_writes(), 1, "only the cold publication");
    assert!(ring.live.try_stop(&mut ring.walker).is_ok());

    // No product geometry can carry a frame that long.
    use crate::ieee80211::dma::rx_ring::rx_ring_outspans_any_unit;
    assert!(rx_ring_outspans_any_unit(32, 1_664));
    assert!(rx_ring_outspans_any_unit(96, 4_608));
    assert!(!rx_ring_outspans_any_unit(COUNT, 16));
    assert!(!rx_ring_outspans_any_unit(2, 8_192));
}

impl<const COUNT: usize> Walked<COUNT> {
    /// The low address of ring descriptor `index`, as the walker reports it.
    fn low(&self, index: usize) -> u32 {
        (self.base + index as u32 * crate::ieee80211::dma::descriptor::DESCRIPTOR_BYTES)
            & ADDRESS_LOW_MASK
    }

    /// Land the epoch's first frame in R, then a frame in every armed
    /// descriptor until the walker runs out.
    fn fill(&mut self) {
        assert!(self.land(4, true));
        self.storage
            .observe_reserved(&mut self.live, &mut self.walker)
            .unwrap();
        while self.land(4, true) {}
        assert_eq!(self.walker.next_descriptor_low, 0);
    }

    /// Take and detach every completed unit, as staging does.
    fn detach_all(
        &mut self,
        pool: &'static oer_memory::ExternalRxHandoffPool<16, COUNT>,
    ) -> std::vec::Vec<oer_memory::ExternalRxNetworkLease<'static, 16>> {
        let mut leases = std::vec::Vec::new();
        let frozen = self.live.freeze_cursor(&mut self.walker).unwrap();
        while let Some(unit) = self
            .storage
            .take_completed_unit(&mut self.live, frozen)
            .unwrap()
        {
            let detached = unit.detach_single(&mut self.walker).unwrap();
            let radio = pool
                .try_claim_radio(detached.into_buffer(), leases.len())
                .map_err(drop)
                .unwrap();
            let length = radio.frame().len();
            leases.push(pool.claim_network(radio.republish(0, length)));
        }
        leases
    }
}

#[test]
fn a_detach_clears_the_owner_bit_and_fences_before_the_lease_is_returned() {
    use crate::ieee80211::dma::rx_dma::model::RxWalkerOperation;
    let mut ring = Walked::<4>::start();
    assert!(ring.land(4, true));
    ring.storage
        .observe_reserved(&mut ring.live, &mut ring.walker)
        .unwrap();
    assert!(ring.land(12, true));
    let completed = ring.storage.descriptors()[0].word0();
    assert!(completed.owner(), "hardware leaves the owner bit set");
    let frozen = ring.live.freeze_cursor(&mut ring.walker).unwrap();
    let unit = ring
        .storage
        .take_completed_unit(&mut ring.live, frozen)
        .unwrap()
        .expect("descriptor zero completed");
    ring.walker.clear_operations();
    let detached = unit.detach_single(&mut ring.walker).unwrap();
    // The lease exists only now: its descriptor already carries the
    // completed image with the owner bit clear, and the write was fenced.
    let word0 = ring.storage.descriptors()[0].word0();
    assert_eq!(word0, completed.with_owner(false));
    assert_eq!(
        (word0.rx_done(), word0.length(), word0.size()),
        (true, 12, 16)
    );
    assert_eq!(ring.walker.operations(), [RxWalkerOperation::Fence]);
    // The staged metadata keeps the completed image.
    assert!(detached.descriptor_word0().owner());
    // The other descriptors are untouched.
    assert!(ring.storage.descriptors()[1].word0().owner());
    drop(detached);
    assert!(ring.live.try_stop(&mut ring.walker).is_ok());
}

#[test]
fn rearm_restores_the_owner_bit_and_a_doorbell_still_follows_a_detached_last() {
    use crate::ieee80211::dma::descriptor::rx_armed_word;
    use crate::ieee80211::dma::rx_dma::model::RxWalkerLanding;
    const COUNT: usize = 4;
    let mut ring = Walked::<COUNT>::start();
    ring.fill();
    let pool = Box::leak(Box::new(
        oer_memory::ExternalRxHandoffPool::<16, COUNT>::new(),
    ));
    let leases = ring.detach_all(pool);
    assert_eq!(leases.len(), COUNT);
    // Every detached descriptor, LAST (the accepted tail) included, carries
    // the owner bit clear.
    for descriptor in ring.storage.descriptors() {
        assert!(!descriptor.word0().owner());
    }
    assert_eq!(ring.walker.last_descriptor_low, ring.low(COUNT - 1));
    drop(leases);
    let append = ring
        .storage
        .recycle_released_prefix::<COUNT, _>(&mut ring.live, &mut ring.walker)
        .unwrap()
        .expect("all but LAST append");
    assert_eq!(append.descriptor_count, COUNT - 1);
    // The rearm set the owner bit again; LAST stays detached-clear.
    for descriptor in &ring.storage.descriptors()[..COUNT - 1] {
        assert_eq!(descriptor.word0(), rx_armed_word(16).unwrap());
    }
    assert!(!ring.storage.descriptors()[COUNT - 1].word0().owner());
    // H1: the doorbell follows LAST.next without checking LAST's owner bit.
    ring.live
        .complete_pending_reload(&mut ring.walker, frozen_settle())
        .unwrap();
    assert_eq!(ring.walker.next_descriptor_low, ring.low(0));
    assert_eq!(
        ring.walker
            .land(ring.storage.walker_table().unwrap(), 4, true),
        RxWalkerLanding::Landed(ring.low(0))
    );
    assert_eq!(ring.live.reload_evidence().walker_on_detached, 0);
    assert!(ring.live.try_stop(&mut ring.walker).is_ok());
}

#[test]
fn a_frozen_cursor_naming_a_detached_descriptor_fails_closed_and_is_counted() {
    const COUNT: usize = 4;
    let mut ring = Walked::<COUNT>::start();
    ring.fill();
    let pool = Box::leak(Box::new(
        oer_memory::ExternalRxHandoffPool::<16, COUNT>::new(),
    ));
    let leases = ring.detach_all(pool);
    assert!(ring.live.freeze_cursor(&mut ring.walker).is_ok());
    // The walker stopped on the barrier of a held lease's descriptor.
    ring.walker.next_descriptor_low = ring.low(1);
    assert_eq!(
        ring.live.freeze_cursor(&mut ring.walker).err(),
        Some(RxRingError::Corrupt)
    );
    assert_eq!(ring.live.reload_evidence().walker_on_detached, 1);
    assert_eq!(
        ring.storage.lifecycle_state(),
        RxDmaArenaState::ResetRequired
    );
    drop(leases);
    assert!(ring.live.try_stop(&mut ring.walker).is_ok());
}

#[test]
fn a_settled_doorbell_leaving_next_on_a_detached_descriptor_fails_closed() {
    const COUNT: usize = 4;
    let mut ring = Walked::<COUNT>::start();
    ring.fill();
    let pool = Box::leak(Box::new(
        oer_memory::ExternalRxHandoffPool::<16, COUNT>::new(),
    ));
    let mut leases = ring.detach_all(pool);
    // Every lease but LAST's returns; LAST stays held, detached.
    let held = leases.pop().expect("LAST's lease");
    drop(leases);
    ring.walker.lose_reloads = 1;
    let append = ring
        .storage
        .recycle_released_prefix::<COUNT, _>(&mut ring.live, &mut ring.walker)
        .unwrap()
        .expect("the returned prefix appends");
    assert_eq!(append.descriptor_count, COUNT - 1);
    // Instead of resuming at the appended head the walker reports the held
    // descriptor.
    ring.walker.next_descriptor_low = ring.low(COUNT - 1);
    assert_eq!(
        ring.live
            .complete_pending_reload(&mut ring.walker, frozen_settle()),
        Err(RxRingError::Corrupt)
    );
    let evidence = ring.live.reload_evidence();
    assert_eq!(evidence.walker_on_detached, 1);
    assert_eq!(evidence.rering_failures, 0);
    assert_eq!(
        ring.storage.lifecycle_state(),
        RxDmaArenaState::ResetRequired
    );
    drop(held);
    assert!(ring.live.try_stop(&mut ring.walker).is_ok());
}

#[test]
fn walker_on_detached_cannot_fire_on_a_just_completed_descriptor() {
    const COUNT: usize = 4;
    let mut ring = Walked::<COUNT>::start();
    assert!(ring.land(4, true));
    ring.storage
        .observe_reserved(&mut ring.live, &mut ring.walker)
        .unwrap();
    for _ in 0..3 {
        assert!(ring.land(4, true));
    }
    // Descriptor 2 carries RX_DONE, but the walker has not moved its
    // cursor past it yet: LAST still names 1 and NEXT holds 2.
    ring.walker.last_descriptor_low = ring.low(1);
    ring.walker.next_descriptor_low = ring.low(2);
    assert!(ring.storage.descriptors()[2].word0().rx_done());

    let cursor = ring.live.freeze_cursor(&mut ring.walker).unwrap();
    let frontier = ring
        .storage
        .completed_unit_frontier_through_cursor(&ring.live, cursor)
        .unwrap();
    // The LAST-bounded frontier stops at LAST: descriptor 2 is not in it.
    assert_eq!((frontier.unit_count, frontier.descriptor_count), (2, 2));
    let pool = Box::leak(Box::new(
        oer_memory::ExternalRxHandoffPool::<16, COUNT>::new(),
    ));
    let mut leases = std::vec::Vec::new();
    let mut remaining = frontier.descriptor_count;
    while remaining != 0 {
        let unit = ring
            .storage
            .take_completed_unit(&mut ring.live, cursor)
            .unwrap()
            .expect("a unit through LAST");
        remaining -= unit.descriptor_count();
        let detached = unit.detach_single(&mut ring.walker).unwrap();
        let radio = pool
            .try_claim_radio(detached.into_buffer(), leases.len())
            .map_err(drop)
            .unwrap();
        let length = radio.frame().len();
        leases.push(pool.claim_network(radio.republish(0, length)));
    }
    // The take itself stops at LAST: the same cursor hands out nothing more.
    assert!(
        ring.storage
            .take_completed_unit(&mut ring.live, cursor)
            .unwrap()
            .is_none()
    );
    // Descriptor 2 was not handed out: it is neither observed nor detached.
    let refreshed = ring.live.freeze_cursor(&mut ring.walker).unwrap();
    assert_eq!(
        ring.storage
            .completed_unit_frontier_through_cursor(&ring.live, refreshed)
            .unwrap()
            .unit_count,
        0
    );
    assert!(!ring.live.observed_mask().contains(2));
    assert!(ring.storage.descriptors()[2].word0().owner());
    assert!(!ring.storage.descriptors()[1].word0().owner());
    assert_eq!(ring.live.reload_evidence().walker_on_detached, 0);

    // Descriptor 0 returns and appends behind the accepted tail; the settle
    // after the doorbell still reads NEXT on descriptor 2.
    drop(leases.remove(0));
    let append = ring
        .storage
        .recycle_released_prefix::<COUNT, _>(&mut ring.live, &mut ring.walker)
        .unwrap()
        .expect("LAST past descriptor 0 releases its link");
    assert_eq!(append.descriptor_count, 1);
    ring.live
        .complete_pending_reload(&mut ring.walker, frozen_settle())
        .unwrap();
    assert_eq!(ring.walker.next_descriptor_low, ring.low(2));
    assert!(!ring.live.reload_pending());
    assert!(ring.live.freeze_cursor(&mut ring.walker).is_ok());
    assert_eq!(ring.live.reload_evidence().walker_on_detached, 0);
    assert_eq!(ring.storage.lifecycle_state(), RxDmaArenaState::Live);
    drop(leases);
    assert!(ring.live.try_stop(&mut ring.walker).is_ok());
}

#[test]
fn walker_on_detached_cannot_fire_on_a_detached_last_at_exhaustion() {
    const COUNT: usize = 4;
    let mut ring = Walked::<COUNT>::start();
    ring.fill();
    let cursor = ring.live.freeze_cursor(&mut ring.walker).unwrap();
    assert_eq!(cursor.next_descriptor_low(), 0);
    assert_eq!(cursor.last_descriptor_low(), ring.low(COUNT - 1));
    let frontier = ring
        .storage
        .completed_unit_frontier_through_cursor(&ring.live, cursor)
        .unwrap();
    assert_eq!(frontier.descriptor_count, COUNT);
    let pool = Box::leak(Box::new(
        oer_memory::ExternalRxHandoffPool::<16, COUNT>::new(),
    ));
    let leases = ring.detach_all(pool);
    assert_eq!(leases.len(), COUNT);
    // LAST, the accepted tail, is detached; NEXT = 0 names no descriptor.
    assert!(!ring.storage.descriptors()[COUNT - 1].word0().owner());
    assert!(ring.live.freeze_cursor(&mut ring.walker).is_ok());
    drop(leases);
    ring.storage
        .recycle_released_prefix::<COUNT, _>(&mut ring.live, &mut ring.walker)
        .unwrap()
        .expect("all but LAST append");
    ring.live
        .complete_pending_reload(&mut ring.walker, frozen_settle())
        .unwrap();
    assert!(ring.live.freeze_cursor(&mut ring.walker).is_ok());
    assert_eq!(ring.live.reload_evidence().walker_on_detached, 0);
    assert_eq!(ring.storage.lifecycle_state(), RxDmaArenaState::Live);
    assert!(ring.live.try_stop(&mut ring.walker).is_ok());
}
