extern crate std;

use self::std::boxed::Box;
use super::*;
use crate::{rx_dma::RxDmaBinding, space::HostDma};

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

impl RxDma for MockRxDma {
    fn last_descriptor_low(&mut self) -> u32 {
        self.last_descriptor_low
    }

    fn next_descriptor_low(&mut self) -> u32 {
        self.next_descriptor_low
    }

    fn next_descriptor(&mut self) -> crate::rx_dma::RxDmaNextDescriptor {
        crate::rx_dma::RxDmaNextDescriptor::for_host_test(self.next_descriptor_low, false)
    }

    fn with_ordered_cursor<R>(
        &mut self,
        observed: impl for<'confirmation> FnOnce(
            crate::rx_dma::RxDmaCursorObservation<'confirmation>,
        ) -> R,
    ) -> R {
        let last = self.last_descriptor_low();
        self.fence();
        let next = self.next_descriptor_low();
        self.fence();
        observed(crate::rx_dma::RxDmaCursorObservation::for_host_test(
            last, next,
        ))
    }

    fn walker_enabled(&mut self) -> bool {
        self.walker
    }

    fn reload_pending(&mut self) -> bool {
        self.reload
    }

    fn try_with_reload_settled<R>(
        &mut self,
        settled: impl for<'confirmation> FnOnce(crate::rx_dma::RxDmaReloadSettled<'confirmation>) -> R,
    ) -> Option<R> {
        (!self.reload_pending())
            .then(|| settled(crate::rx_dma::RxDmaReloadSettled::for_host_test()))
    }

    fn configure_descriptor_window(&mut self, _: &RxDmaBinding<'_>) {}

    fn write_descriptor_base(&mut self, _: &RxDmaBinding<'_>, address: u32) {
        self.publications += 1;
        self.descriptor_base = address;
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
        enabled: impl for<'confirmation> FnOnce(crate::rx_dma::RxDmaWalkerEnabled<'confirmation>) -> R,
    ) -> Option<R> {
        self.walker = true;
        (!self.ambiguous_enable)
            .then(|| enabled(crate::rx_dma::RxDmaWalkerEnabled::for_host_test()))
    }

    fn try_with_walker_stopped<R>(
        &mut self,
        binding: &RxDmaBinding<'_>,
        stopped: impl for<'confirmation> FnOnce(crate::rx_dma::RxDmaWalkerStopped<'confirmation>) -> R,
    ) -> Option<R> {
        self.disable_calls += 1;
        if self.fail_disable {
            return None;
        }
        self.walker = false;
        Some(stopped(crate::rx_dma::RxDmaWalkerStopped::for_host_test(
            binding,
        )))
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
fn accepted_list_pressure_projects_next_to_the_current_tail() {
    const COUNT: usize = 4;
    const LOW_MASK: u32 = 0x000f_ffff;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let mut buffers = [0; COUNT];
    let base = storage.dma_layout(&mut buffers).unwrap();
    let mut mmio = MockRxDma::default();
    let prepared = storage
        .prepare_ring(&mut mmio, base, &buffers)
        .expect("prepared pressure model");
    let live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live pressure model");

    let next = |index: u32| {
        crate::rx_dma::RxDmaNextDescriptor::for_host_test(
            (base + index * crate::descriptor::DESCRIPTOR_BYTES) & LOW_MASK,
            false,
        )
    };
    assert_eq!(live.accepted_list_remaining_from_next(next(0)), Some(4));
    assert_eq!(live.accepted_list_remaining_from_next(next(1)), Some(3));
    assert_eq!(live.accepted_list_remaining_from_next(next(3)), Some(1));
    assert_eq!(
        live.accepted_list_remaining_from_next(crate::rx_dma::RxDmaNextDescriptor::for_host_test(
            0, false
        )),
        Some(0)
    );
    assert_eq!(
        live.accepted_list_remaining_from_next(crate::rx_dma::RxDmaNextDescriptor::for_host_test(
            0, true
        )),
        None,
        "upper-only NEXT is not an exhausted-list proof"
    );
    assert_eq!(
        live.accepted_list_remaining_from_next(crate::rx_dma::RxDmaNextDescriptor::for_host_test(
            (base + COUNT as u32 * crate::descriptor::DESCRIPTOR_BYTES) & LOW_MASK,
            false,
        )),
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
    const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
    // Descriptor zero names physical buffer one; descriptor one names
    // physical buffer zero.
    let mut storage = Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new());
    storage.buffer_mut(1).unwrap()[4..8].copy_from_slice(&[5, 6, 7, 8]);
    let storage = Box::leak(storage);
    let mut buffers = [0; COUNT];
    let base = storage
        .dma_layout_rotated(&mut buffers, 1)
        .expect("pre-ring rotation");
    assert_eq!(storage.descriptor_buffer_id(0), Some(1));
    assert_eq!(storage.descriptor_buffer_id(1), Some(0));

    let mut mmio = MockRxDma::default();
    let prepared = storage
        .prepare_ring(&mut mmio, base, &buffers)
        .expect("prepared rotated owner");
    let mut live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live rotated epoch");
    storage.descriptors()[0].write_word0(
        16 | (8 << crate::descriptor::LENGTH_SHIFT)
            | crate::descriptor::BIT_30
            | crate::descriptor::BIT_31,
    );
    mmio.last_descriptor_low = (base + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    mmio.next_descriptor_low = 0;

    let detached = storage
        .take_completed_unit(&mut live, 1)
        .unwrap()
        .expect("completed rotated unit")
        .detach_single()
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
    const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let mut buffers = [0; COUNT];
    let base = storage.dma_layout(&mut buffers).unwrap();
    storage.buffer_mut(0).unwrap()[4..8].copy_from_slice(&[1, 2, 3, 4]);
    let mut mmio = MockRxDma::default();
    let prepared = storage
        .prepare_ring(&mut mmio, base, &buffers)
        .expect("prepared owner");
    let mut live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live epoch");
    storage.descriptors()[0].write_word0(
        16 | (8 << crate::descriptor::LENGTH_SHIFT)
            | crate::descriptor::BIT_30
            | crate::descriptor::BIT_31,
    );
    // Model the finite list exhausted at its accepted tail. Merely seeing
    // NEXT name descriptor one is not sufficient: hardware proved that image
    // may precede hardware fetching descriptor zero's link word.
    mmio.last_descriptor_low = (base + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    mmio.next_descriptor_low = 0;

    let detached = storage
        .take_completed_unit(&mut live, 1)
        .unwrap()
        .expect("completed unit")
        .detach_single()
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
    assert_ne!(
        storage.descriptors()[0].word0() & crate::descriptor::BIT_30,
        0
    );

    drop(network);
    let append = storage
        .recycle_released_prefix::<COUNT, _>(&mut live, &mut mmio)
        .expect("released prefix reclaim")
        .expect("one descriptor appended");
    assert_eq!(append.descriptor_count, 1);
    assert_eq!(storage.released_buffer_count(), 0);
    assert_eq!(
        storage.descriptors()[0].word0() & crate::descriptor::BIT_30,
        0
    );
}

#[test]
fn role_neutral_prefix_recycle_normalizes_a_returned_staged_buffer() {
    const COUNT: usize = 2;
    const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let mut buffers = [0; COUNT];
    let base = storage.dma_layout(&mut buffers).unwrap();
    let mut mmio = MockRxDma::default();
    let prepared = storage
        .prepare_ring(&mut mmio, base, &buffers)
        .expect("prepared owner");
    let mut live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live epoch");
    storage.descriptors()[0].write_word0(
        16 | (8 << crate::descriptor::LENGTH_SHIFT)
            | crate::descriptor::BIT_30
            | crate::descriptor::BIT_31,
    );
    mmio.last_descriptor_low = (base + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    mmio.next_descriptor_low = 0;

    let detached = storage
        .take_completed_unit(&mut live, 1)
        .unwrap()
        .expect("completed unit")
        .detach_single()
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
    assert_eq!(
        storage.descriptors()[0].word0() & crate::descriptor::BIT_30,
        0
    );
}

#[test]
fn released_dma_buffers_rearm_only_as_a_contiguous_ring_prefix() {
    const COUNT: usize = 4;
    const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let mut buffers = [0; COUNT];
    let base = storage.dma_layout(&mut buffers).unwrap();
    let mut mmio = MockRxDma::default();
    let prepared = storage
        .prepare_ring(&mut mmio, base, &buffers)
        .expect("prepared owner");
    let mut live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live epoch");
    for descriptor in &storage.descriptors()[..2] {
        descriptor.write_word0(
            16 | (8 << crate::descriptor::LENGTH_SHIFT)
                | crate::descriptor::BIT_30
                | crate::descriptor::BIT_31,
        );
    }
    mmio.last_descriptor_low = (base + 3 * crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    mmio.next_descriptor_low = 0;

    let first = storage
        .take_completed_unit(&mut live, 1)
        .unwrap()
        .expect("first completed unit")
        .detach_single()
        .expect("first single descriptor");
    let second = storage
        .take_completed_unit(&mut live, 1)
        .unwrap()
        .expect("second completed unit")
        .detach_single()
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
            .all(|descriptor| descriptor.word0() & crate::descriptor::BIT_30 == 0)
    );
}

#[test]
fn one_arena_cannot_issue_two_root_ring_capabilities() {
    const COUNT: usize = 2;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let mut buffers = [0; COUNT];
    let base = storage.dma_layout(&mut buffers).unwrap();
    let mut first_mmio = MockRxDma::default();
    let mut second_mmio = MockRxDma::default();

    let first = storage
        .prepare_ring(&mut first_mmio, base, &buffers)
        .expect("first root capability");
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::Prepared);
    assert!(matches!(
        storage.prepare_ring(&mut second_mmio, base, &buffers),
        Err(RxRingError::Busy)
    ));

    let live = first
        .try_start(&mut first_mmio)
        .map_err(|(_, error)| error)
        .expect("first live epoch");
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::Live);
    let _halted = live
        .try_stop(&mut first_mmio)
        .unwrap_or_else(|_| panic!("walker stops"));
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::Prepared);
    assert!(matches!(
        storage.prepare_ring(&mut second_mmio, base, &buffers),
        Err(RxRingError::Busy)
    ));
}

#[test]
fn live_binding_rejects_a_foreign_storage_arena() {
    const COUNT: usize = 2;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let mut buffers = [0; COUNT];
    let base = storage.dma_layout(&mut buffers).unwrap();
    let foreign = RxDmaStorage::<COUNT, 16, 20, HostDma>::new();
    let mut mmio = MockRxDma::default();
    let live = storage
        .prepare_ring(&mut mmio, base, &buffers)
        .expect("prepared owner")
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live owner");

    assert_eq!(
        foreign.completed_unit_frontier(&live),
        Err(RxRingError::DescriptorOwnerAddress)
    );

    let _halted = live
        .try_stop(&mut mmio)
        .unwrap_or_else(|_| panic!("walker stops"));
}

#[test]
fn first_frontier_returns_one_unit_from_a_terminal_burst() {
    const COUNT: usize = 4;
    const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let mut buffers = [0; COUNT];
    let base = storage.dma_layout(&mut buffers).unwrap();
    let mut mmio = MockRxDma::default();
    let prepared = storage
        .prepare_ring(&mut mmio, base, &buffers)
        .expect("prepared owner");
    let live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live epoch");

    for descriptor in storage.descriptors() {
        descriptor.write_word0(
            16 | (4 << crate::descriptor::LENGTH_SHIFT)
                | crate::descriptor::BIT_30
                | crate::descriptor::BIT_31,
        );
    }
    mmio.last_descriptor_low = (base + 3 * crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    mmio.next_descriptor_low = base & ADDRESS_LOW_MASK;

    let complete = storage
        .completed_unit_frontier_through_cursor(
            &live,
            mmio.last_descriptor_low,
            mmio.next_descriptor_low,
        )
        .expect("complete burst frontier");
    assert_eq!(complete.unit_count, COUNT);
    assert_eq!(complete.descriptor_count, COUNT);

    let first = storage
        .first_completed_unit_frontier_through_cursor(
            &live,
            mmio.last_descriptor_low,
            mmio.next_descriptor_low,
        )
        .expect("first unit frontier");
    assert_eq!(first.unit_count, 1);
    assert_eq!(first.descriptor_count, 1);
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn failed_root_prepare_quarantines_the_arena_when_walker_stop_is_unproved() {
    const COUNT: usize = 2;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let mut buffers = [0; COUNT];
    let base = storage.dma_layout(&mut buffers).unwrap();
    let mut mmio = MockRxDma {
        walker: true,
        fail_disable: true,
        ..MockRxDma::default()
    };

    assert!(matches!(
        storage.prepare_ring(&mut mmio, base, &buffers),
        Err(RxRingError::Busy)
    ));
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::ResetRequired);
}

#[test]
fn failed_halted_prepare_quarantines_the_arena_when_walker_stop_is_unproved() {
    const COUNT: usize = 2;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let mut buffers = [0; COUNT];
    let base = storage.dma_layout(&mut buffers).unwrap();
    let mut mmio = MockRxDma::default();
    let prepared = storage
        .prepare_ring(&mut mmio, base, &buffers)
        .expect("prepared owner");
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

    assert_eq!(error, RxRingError::Busy);
    assert!(mmio.walker);
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::ResetRequired);
}

#[test]
fn partial_live_buffer_rearm_quarantines_the_arena() {
    const COUNT: usize = 2;
    const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let mut buffers = [0; COUNT];
    let base = storage.dma_layout(&mut buffers).unwrap();
    let mut mmio = MockRxDma::default();
    let prepared = storage
        .prepare_ring(&mut mmio, base, &buffers)
        .expect("prepared owner");
    let mut live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live epoch");
    storage.descriptors()[0].write_word0(
        crate::descriptor::rx_armed_word(16).expect("valid size") | crate::descriptor::BIT_30,
    );
    mmio.last_descriptor_low = base & ADDRESS_LOW_MASK;
    mmio.next_descriptor_low = (base + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    // An upper owner keeps descriptor zero's buffer, so its rearm fails
    // inside the recycle transaction, after every descriptor check passed.
    let detached = storage
        .take_completed_unit(&mut live, 1)
        .unwrap()
        .expect("completed unit")
        .detach_single()
        .expect("single descriptor detaches");

    assert_eq!(
        storage.recycle_completed_prefix::<1, _>(&mut live, &mut mmio),
        Ok(None),
    );
    storage.descriptors()[1].write_word0(
        crate::descriptor::rx_armed_word(16).expect("valid size") | crate::descriptor::BIT_30,
    );
    mmio.last_descriptor_low = (base + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    assert_eq!(
        storage.recycle_completed_prefix::<1, _>(&mut live, &mut mmio),
        Err(RxRingError::Busy),
    );
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::ResetRequired);
    drop(detached);
}

#[test]
fn completed_descriptor_metadata_is_validated_before_payload_transfer() {
    const COUNT: usize = 2;

    for corruption in 0..3 {
        let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
        let mut buffers = [0; COUNT];
        let base = storage.dma_layout(&mut buffers).unwrap();
        let mut mmio = MockRxDma::default();
        let prepared = storage
            .prepare_ring(&mut mmio, base, &buffers)
            .expect("prepared owner");
        let mut live = prepared
            .try_start(&mut mmio)
            .map_err(|(_, error)| error)
            .expect("live epoch");
        let descriptor = &storage.descriptors()[0];
        let mut word0 = 16
            | (8 << crate::descriptor::LENGTH_SHIFT)
            | crate::descriptor::BIT_30
            | crate::descriptor::BIT_31;
        let mut buffer_address = buffers[0];
        let mut next_address = base + crate::descriptor::DESCRIPTOR_BYTES;
        match corruption {
            0 => word0 = (word0 & !crate::descriptor::SIZE_MASK) | 17,
            1 => buffer_address = buffers[0] + 4,
            2 => next_address = 0,
            _ => unreachable!(),
        }
        descriptor.inject_corrupt_words(word0, buffer_address, next_address);

        assert!(matches!(
            storage.take_completed(&mut live, 0),
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
    const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let mut buffers = [0; COUNT];
    let base = storage.dma_layout(&mut buffers).unwrap();
    let mut mmio = MockRxDma::default();
    let prepared = storage
        .prepare_ring(&mut mmio, base, &buffers)
        .expect("prepared owner");
    let mut live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live epoch");
    storage.descriptors()[0].write_word0(
        crate::descriptor::rx_armed_word(16).expect("valid size") | crate::descriptor::BIT_30,
    );
    mmio.last_descriptor_low = base & ADDRESS_LOW_MASK;
    mmio.next_descriptor_low = (base + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    assert!(storage.take_completed(&mut live, 0).unwrap().is_some());
    assert!(
        storage
            .recycle_completed_prefix::<1, _>(&mut live, &mut mmio)
            .unwrap()
            .is_none()
    );
    storage.descriptors()[1].write_word0(
        crate::descriptor::rx_armed_word(16).expect("valid size") | crate::descriptor::BIT_30,
    );
    mmio.last_descriptor_low = (base + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
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
fn in_arena_intermediate_last_repairs_from_its_successor() {
    const COUNT: usize = 4;
    const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let mut buffers = [0; COUNT];
    let base = storage.dma_layout(&mut buffers).unwrap();
    let mut mmio = MockRxDma::default();
    let prepared = storage
        .prepare_ring(&mut mmio, base, &buffers)
        .expect("prepared owner");
    let mut live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live epoch");
    storage.descriptors()[0].write_word0(
        crate::descriptor::rx_armed_word(16).expect("valid size") | crate::descriptor::BIT_30,
    );
    mmio.last_descriptor_low = base & ADDRESS_LOW_MASK;
    mmio.next_descriptor_low = (base + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    assert!(storage.take_completed(&mut live, 0).unwrap().is_some());
    assert!(
        storage
            .recycle_completed_prefix::<1, _>(&mut live, &mut mmio)
            .unwrap()
            .is_none()
    );
    storage.descriptors()[1].write_word0(
        crate::descriptor::rx_armed_word(16).expect("valid size") | crate::descriptor::BIT_30,
    );
    mmio.last_descriptor_low = (base + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    assert!(
        storage
            .recycle_completed_prefix::<1, _>(&mut live, &mut mmio)
            .unwrap()
            .is_some()
    );
    assert!(live.reload_pending());

    // Old accepted tail is descriptor 3 and the pending tail is 0, but
    // hardware can still report the earlier descriptor 1 while the vendor
    // worker is returning several completed units. Its successor is the
    // exact base-repair value used by wDev_AppendRxBlocks.
    mmio.next_descriptor_low = 0;
    mmio.last_descriptor_low = (base + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    assert_eq!(
        live.poll_pending_reload(&mut mmio),
        Ok(crate::rx_ring::RxReloadObservation::Settled)
    );
    assert_eq!(
        mmio.descriptor_base,
        base + 2 * crate::descriptor::DESCRIPTOR_BYTES
    );
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::Live);
    assert!(live.try_stop(&mut mmio).is_ok());
}

const REPAIR_COUNT: usize = 2;
const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;

/// A live two-descriptor ring whose walker exhausted the accepted tail
/// before it observed the new link, so the reload suffix republished the
/// head (`base`) through BASE.
fn ring_after_exhausted_reload_repair() -> (
    &'static RxDmaStorage<REPAIR_COUNT, 16, 20, HostDma>,
    RxRingLive<'static, REPAIR_COUNT>,
    MockRxDma,
    u32,
) {
    const COUNT: usize = REPAIR_COUNT;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let buffers = Box::leak(Box::new([0; COUNT]));
    let base = storage.dma_layout(buffers).unwrap();
    let mut mmio = MockRxDma::default();
    let prepared = storage
        .prepare_ring(&mut mmio, base, buffers)
        .expect("prepared owner");
    let mut live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live epoch");
    storage.descriptors()[0].write_word0(
        crate::descriptor::rx_armed_word(16).expect("valid size") | crate::descriptor::BIT_30,
    );
    mmio.last_descriptor_low = base & ADDRESS_LOW_MASK;
    mmio.next_descriptor_low = (base + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    assert!(storage.take_completed(&mut live, 0).unwrap().is_some());
    assert!(
        storage
            .recycle_completed_prefix::<1, _>(&mut live, &mut mmio)
            .unwrap()
            .is_none()
    );
    storage.descriptors()[1].write_word0(
        crate::descriptor::rx_armed_word(16).expect("valid size") | crate::descriptor::BIT_30,
    );
    mmio.last_descriptor_low = (base + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    assert!(
        storage
            .recycle_completed_prefix::<1, _>(&mut live, &mut mmio)
            .unwrap()
            .is_some()
    );

    // The walker exhausted the previously accepted tail before observing
    // its new link. The vendor suffix republishes that link through base.
    mmio.next_descriptor_low = 0;
    mmio.last_descriptor_low = (base + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    assert_eq!(
        live.poll_pending_reload(&mut mmio),
        Ok(crate::rx_ring::RxReloadObservation::Settled)
    );
    assert_eq!(mmio.descriptor_base, base);
    (storage, live, mmio, base)
}

#[test]
fn exhausted_reload_records_the_direct_base_repair_facts() {
    let (_storage, live, mut mmio, base) = ring_after_exhausted_reload_repair();
    assert_eq!(
        live.reload_repair_evidence(),
        crate::rx_ring::RxReloadRepairEvidence {
            max_reload_attempts: 0,
            observations: 1,
            unknown_upper_with_zero_address: 0,
            base_repairs: 1,
            last_next_low: 0,
            last_last_low: Some((base + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK),
            last_repair_head: Some(base),
            exhausted_republications: 1,
            republications_accepted: 0,
        }
    );
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn a_republished_head_is_accepted_once_when_the_walker_fetches_it() {
    let (_storage, mut live, mut mmio, base) = ring_after_exhausted_reload_repair();
    let accepted = |live: &RxRingLive<'_, REPAIR_COUNT>| {
        let evidence = live.reload_repair_evidence();
        (
            evidence.exhausted_republications,
            evidence.republications_accepted,
        )
    };
    // NEXT still zero: the BASE store has not reached the walker.
    live.observe_exhausted_republication(&mut mmio);
    assert_eq!(accepted(&live), (1, 0));
    // The walker fetched the head; the probe's last observation and any
    // later one count nothing more.
    mmio.next_descriptor_low = base & ADDRESS_LOW_MASK;
    live.observe_exhausted_republication(&mut mmio);
    assert_eq!(accepted(&live), (1, 1));
    live.observe_exhausted_republication(&mut mmio);
    live.observe_exhausted_republication(&mut mmio);
    assert!(!live.exhausted_republication_probe_pending());
    assert_eq!(accepted(&live), (1, 1));
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn exhausted_list_reclaims_untouched_prefix_before_terminal_payload() {
    const COUNT: usize = 4;
    const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let mut buffers = [0; COUNT];
    let base = storage.dma_layout(&mut buffers).unwrap();
    let mut mmio = MockRxDma::default();
    let prepared = storage
        .prepare_ring(&mut mmio, base, &buffers)
        .expect("prepared owner");
    let mut live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live epoch");

    // Return descriptor zero while the old finite list has already
    // exhausted at descriptor three. The vendor append suffix repairs
    // base to the returned descriptor, leaving software head at one.
    storage.descriptors()[0].write_word0(
        16 | (4 << crate::descriptor::LENGTH_SHIFT)
            | crate::descriptor::BIT_30
            | crate::descriptor::BIT_31,
    );
    mmio.last_descriptor_low = (base + 3 * crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    mmio.next_descriptor_low = 0;
    let unit = storage
        .take_completed_unit(&mut live, 1)
        .expect("completed descriptor")
        .expect("unit owner");
    unit.recycle(&mut mmio)
        .expect("live append")
        .expect("returned descriptor");
    live.complete_pending_reload(&mut mmio)
        .expect("vendor reload repair");
    assert_eq!(live.recycle_start().get(), 1);
    assert_eq!(live.accepted_tail().get(), 0);

    // Hardware consumed only the repaired terminal before exhausting
    // again. Descriptors one through three retain their guards and armed
    // lengths, but NEXT=0/LAST=tail proves their links are released.
    storage.descriptors()[0].write_word0(
        16 | (4 << crate::descriptor::LENGTH_SHIFT)
            | crate::descriptor::BIT_30
            | crate::descriptor::BIT_31,
    );
    mmio.last_descriptor_low = base & ADDRESS_LOW_MASK;
    mmio.next_descriptor_low = 0;
    let frontier = storage
        .first_completed_unit_frontier_through_cursor(
            &live,
            mmio.last_descriptor_low,
            mmio.next_descriptor_low,
        )
        .expect("exhausted frontier");
    assert_eq!(frontier.unit_count, 1);
    assert_eq!(frontier.descriptor_count, COUNT);

    let unit = storage
        .take_completed_unit(&mut live, frontier.descriptor_count)
        .expect("released unit")
        .expect("terminal payload owner");
    assert_eq!(unit.descriptor_count(), COUNT);
    assert_eq!(unit.total_length(), 4);
    assert_eq!(unit.segment(0), Some(&[][..]));
    assert_eq!(unit.segment(1), Some(&[][..]));
    assert_eq!(unit.segment(2), Some(&[][..]));
    assert_eq!(unit.segment(3).map(<[u8]>::len), Some(4));
    let append = unit
        .recycle(&mut mmio)
        .expect("exhausted list recycle")
        .expect("full list republished");
    assert_eq!(append.head_index.get(), 1);
    assert_eq!(append.descriptor_count, COUNT);
    assert_eq!(live.recycle_start().get(), 1);
    assert_eq!(live.accepted_tail().get(), 0);
    assert!(live.topology_snapshot().valid);
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn dropping_a_taken_completed_unit_requires_radio_reset() {
    const COUNT: usize = 2;
    const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let mut buffers = [0; COUNT];
    let base = storage.dma_layout(&mut buffers).unwrap();
    let mut mmio = MockRxDma::default();
    let prepared = storage
        .prepare_ring(&mut mmio, base, &buffers)
        .expect("prepared owner");
    let mut live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live epoch");
    storage.descriptors()[0].write_word0(
        crate::descriptor::rx_armed_word(16).expect("valid size") | crate::descriptor::BIT_30,
    );
    mmio.last_descriptor_low = base & ADDRESS_LOW_MASK;
    mmio.next_descriptor_low = (base + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    let frontier = storage
        .completed_unit_frontier_through(&live, mmio.last_descriptor_low)
        .unwrap();
    let unit = storage
        .take_completed_unit(&mut live, frontier.descriptor_count)
        .unwrap()
        .expect("completed unit");

    drop(unit);
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::ResetRequired);
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn frozen_last_releases_the_descriptor_equal_to_last() {
    const COUNT: usize = 4;
    const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let mut buffers = [0; COUNT];
    let base = storage.dma_layout(&mut buffers).unwrap();
    let mut mmio = MockRxDma::default();
    let prepared = storage
        .prepare_ring(&mut mmio, base, &buffers)
        .expect("prepared owner");
    let mut live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live epoch");
    assert_eq!(live.recycle_start().get(), 0);
    assert_eq!(live.accepted_tail().get(), COUNT - 1);

    storage.descriptors()[0].write_word0(
        16 | (4 << crate::descriptor::LENGTH_SHIFT)
            | crate::descriptor::BIT_30
            | crate::descriptor::BIT_31,
    );
    mmio.last_descriptor_low = base & ADDRESS_LOW_MASK;
    mmio.next_descriptor_low = (base + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    let frozen_cursor = live.freeze_cursor(&mut mmio);
    let unit = storage
        .take_completed_unit(&mut live, 1)
        .expect("completed descriptor")
        .expect("descriptor owner");
    unit.retain_for_deferred_recycle();

    let append = storage
        .recycle_completed_unit_through_frozen_last(&mut live, &mut mmio, frozen_cursor, 1)
        .expect("frozen LAST reclaim")
        .expect("LAST itself releases the observed descriptor");
    assert_eq!(append.head_index.get(), 0);
    assert_eq!(append.descriptor_count, 1);
    assert_eq!(
        storage.descriptors()[0].word0() & crate::descriptor::BIT_30,
        0
    );
    assert!(live.observed_mask().is_empty());
    live.complete_pending_reload(&mut mmio)
        .expect("vendor reload suffix");
    assert_eq!(live.accepted_tail().get(), 0);
    assert!(live.topology_snapshot().valid);
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn frozen_last_unit_reclaim_returns_only_one_vendor_chain() {
    const COUNT: usize = 4;
    const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let mut buffers = [0; COUNT];
    let base = storage.dma_layout(&mut buffers).unwrap();
    let mut mmio = MockRxDma::default();
    let prepared = storage
        .prepare_ring(&mut mmio, base, &buffers)
        .expect("prepared owner");
    let mut live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live epoch");

    for index in 0..2 {
        storage.descriptors()[index].write_word0(
            16 | (4 << crate::descriptor::LENGTH_SHIFT)
                | crate::descriptor::BIT_30
                | crate::descriptor::BIT_31,
        );
    }
    mmio.last_descriptor_low = (base + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    mmio.next_descriptor_low = (base + 2 * crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    let cursor = live.freeze_cursor(&mut mmio);

    storage
        .take_completed_unit(&mut live, 1)
        .expect("first unit inspection")
        .expect("first unit owner")
        .retain_for_deferred_recycle();
    let append = storage
        .recycle_completed_unit_through_frozen_last(&mut live, &mut mmio, cursor, 1)
        .expect("first vendor unit reclaim")
        .expect("first unit precedes LAST");
    assert_eq!(append.head_index.get(), 0);
    assert_eq!(append.descriptor_count, 1);
    assert!(live.observed_mask().is_empty());
    assert_ne!(
        storage.descriptors()[1].word0() & crate::descriptor::BIT_30,
        0,
        "the following complete unit must remain a distinct vendor chain"
    );
    live.complete_pending_reload(&mut mmio)
        .expect("first vendor reload suffix");

    storage
        .take_completed_unit(&mut live, 1)
        .expect("second unit inspection")
        .expect("second unit owner")
        .retain_for_deferred_recycle();
    assert!(
        storage
            .recycle_completed_unit_through_frozen_last(&mut live, &mut mmio, cursor, 1,)
            .expect("stale generation check")
            .is_none(),
        "one frozen cursor must not authorize a second append generation"
    );
    let refreshed = live.freeze_cursor(&mut mmio);
    storage
        .recycle_completed_unit_through_frozen_last(&mut live, &mut mmio, refreshed, 1)
        .expect("refreshed cursor check")
        .expect("refreshed LAST releases the second vendor unit");
    live.complete_pending_reload(&mut mmio)
        .expect("second vendor reload suffix");
    assert!(live.topology_snapshot().valid);
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn ambiguous_start_quarantines_the_arena_when_walker_is_observed_live() {
    const COUNT: usize = 2;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let mut buffers = [0; COUNT];
    let base = storage.dma_layout(&mut buffers).unwrap();
    let mut mmio = MockRxDma {
        ambiguous_enable: true,
        ..MockRxDma::default()
    };
    let prepared = storage
        .prepare_ring(&mut mmio, base, &buffers)
        .expect("prepared owner");

    let (_prepared, error) = match prepared.try_start(&mut mmio) {
        Ok(_) => panic!("ambiguous enable is not a live capability"),
        Err(failure) => failure,
    };
    assert_eq!(error, RxRingError::Busy);
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

/// The host space moved to the DMA window's low end, where the first
/// descriptor's low address bits are zero.
enum AtWindowStart {}

impl crate::space::DmaSpace for AtWindowStart {
    fn address<Arena: ?Sized, Field: ?Sized>(arena: &Arena, field: *const Field) -> Option<u32> {
        HostDma::address(arena, field)
            .map(|address| address - crate::space::HOST_ARENA_BASE + crate::descriptor::DMA_LOW)
    }
}

#[test]
fn a_descriptor_base_that_reads_as_an_exhausted_list_is_refused() {
    let storage = Box::leak(Box::new(RxDmaStorage::<2, 16, 20, AtWindowStart>::new()));
    let mut buffers = [0; 2];
    let base = storage.dma_layout(&mut buffers).unwrap();
    assert_eq!(base, crate::descriptor::DMA_LOW);
    let mut mmio = MockRxDma::default();
    assert!(matches!(
        storage.prepare_ring(&mut mmio, base, &buffers),
        Err(RxRingError::Address)
    ));
}

fn live_host_arena<const COUNT: usize>() -> (
    &'static RxDmaStorage<COUNT, 16, 20, HostDma>,
    &'static [u32; COUNT],
    MockRxDma,
    RxRingLive<'static, COUNT>,
) {
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20, HostDma>::new()));
    let buffers = Box::leak(Box::new([0; COUNT]));
    let base = storage.dma_layout(buffers).unwrap();
    let mut mmio = MockRxDma::default();
    let live = storage
        .prepare_ring(&mut mmio, base, buffers)
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
    storage.descriptors()[1].write_word0(
        16 | (8 << crate::descriptor::LENGTH_SHIFT)
            | crate::descriptor::BIT_30
            | crate::descriptor::BIT_31,
    );
    let completed = storage
        .take_completed(&mut live, 1)
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
        crate::space::HOST_ARENA_BASE - 4,
        base,
        base + crate::descriptor::DESCRIPTOR_BYTES,
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

    storage.descriptors()[0].write_word0(
        16 | (8 << crate::descriptor::LENGTH_SHIFT)
            | crate::descriptor::BIT_30
            | crate::descriptor::BIT_31,
    );
    let detached = storage
        .take_completed_unit(&mut live, 1)
        .unwrap()
        .expect("completed unit")
        .detach_single()
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
    walker: crate::rx_dma::model::RxWalkerModel,
    foreign: &'static RxDmaBinding<'static>,
}

impl ForeignStopProof {
    fn new() -> Self {
        let other = Box::leak(Box::new(RxDmaStorage::<2, 16, 20, HostDma>::new()));
        let base = other.dma_layout(&mut [0; 2]).unwrap();
        let foreign = RxDmaBinding::new(&other.descriptors, base, &other.arena).unwrap();
        Self {
            walker: crate::rx_dma::model::RxWalkerModel::new(),
            foreign: Box::leak(Box::new(foreign)),
        }
    }
}

impl RxDma for ForeignStopProof {
    fn last_descriptor_low(&mut self) -> u32 {
        self.walker.last_descriptor_low()
    }

    fn next_descriptor_low(&mut self) -> u32 {
        self.walker.next_descriptor_low()
    }

    fn next_descriptor(&mut self) -> crate::rx_dma::RxDmaNextDescriptor {
        self.walker.next_descriptor()
    }

    fn with_ordered_cursor<R>(
        &mut self,
        observed: impl for<'confirmation> FnOnce(
            crate::rx_dma::RxDmaCursorObservation<'confirmation>,
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
        settled: impl for<'confirmation> FnOnce(crate::rx_dma::RxDmaReloadSettled<'confirmation>) -> R,
    ) -> Option<R> {
        self.walker.try_with_reload_settled(settled)
    }

    fn configure_descriptor_window(&mut self, binding: &RxDmaBinding<'_>) {
        self.walker.configure_descriptor_window(binding);
    }

    fn write_descriptor_base(&mut self, binding: &RxDmaBinding<'_>, address: u32) {
        self.walker.write_descriptor_base(binding, address);
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
        enabled: impl for<'confirmation> FnOnce(crate::rx_dma::RxDmaWalkerEnabled<'confirmation>) -> R,
    ) -> Option<R> {
        self.walker.try_with_walker_enabled(binding, enabled)
    }

    fn try_with_walker_stopped<R>(
        &mut self,
        _: &RxDmaBinding<'_>,
        stopped: impl for<'confirmation> FnOnce(crate::rx_dma::RxDmaWalkerStopped<'confirmation>) -> R,
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
    let mut buffers = [0; 2];
    let base = storage.dma_layout(&mut buffers).unwrap();
    let mut mmio = ForeignStopProof::new();
    let live = storage
        .prepare_ring(&mut mmio, base, &buffers)
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
    let mut buffers = [0; 2];
    let base = storage.dma_layout(&mut buffers).unwrap();
    let mut model = crate::rx_dma::model::RxWalkerModel::new();
    let halted = storage
        .prepare_ring(&mut model, base, &buffers)
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
fn a_walker_that_does_not_stop_returns_the_live_owner_busy() {
    use crate::rx_dma::model::{RxWalkerModel, RxWalkerOperation};
    let storage = Box::leak(Box::new(RxDmaStorage::<2, 16, 20, HostDma>::new()));
    let mut buffers = [0; 2];
    let base = storage.dma_layout(&mut buffers).unwrap();
    let mut mmio = RxWalkerModel::new();
    let live = storage
        .prepare_ring(&mut mmio, base, &buffers)
        .unwrap()
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .unwrap();

    mmio.refuse_stop = true;
    let (live, error) = live
        .try_stop(&mut mmio)
        .err()
        .expect("an unconfirmed stop keeps the live owner");
    assert_eq!(error, RxRingError::Busy);
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::Live);
    assert!(mmio.walker_enabled);

    mmio.refuse_stop = false;
    assert!(live.try_stop(&mut mmio).is_ok());
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::Prepared);
    assert_eq!(mmio.operation_count(RxWalkerOperation::Stop), 2);
}

#[test]
fn a_chain_longer_than_the_length_field_is_owned_and_recycled() {
    use crate::descriptor::{BIT_30, BIT_31, LENGTH_SHIFT, SIZE_MASK};
    use crate::rx_dma::model::RxWalkerModel;
    const COUNT: usize = 4;
    const BUFFER: u32 = 8_192;
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 8_192, 8_196, HostDma>::new()));
    let mut buffers = [0; COUNT];
    let base = storage.dma_layout(&mut buffers).unwrap();
    let mut mmio = RxWalkerModel::new();
    let mut live = storage
        .prepare_ring(&mut mmio, base, &buffers)
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
        descriptor.write_word0(BUFFER | (BUFFER << LENGTH_SHIFT) | BIT_31);
    }
    storage.descriptors()[2].write_word0(BUFFER | (100 << LENGTH_SHIFT) | BIT_30 | BIT_31);
    mmio.set_cursor(base, 2, Some(3));
    let cursor = live.freeze_cursor(&mut mmio);
    let frontier = storage
        .completed_unit_frontier_through_cursor(
            &live,
            cursor.last_descriptor_low(),
            cursor.next_descriptor_low(),
        )
        .unwrap();
    assert_eq!(frontier.descriptor_count, 3);

    let unit = storage
        .take_completed_unit(&mut live, frontier.descriptor_count)
        .expect("an oversized chain is air input, not a ring fault")
        .expect("the complete chain is owned");
    assert_eq!(unit.descriptor_count(), 3);
    assert_eq!(unit.total_length(), 2 * BUFFER as usize + 100);
    assert!(unit.total_length() > SIZE_MASK as usize);
    assert_eq!(unit.metadata().staged_word0(), None);
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::Live);

    // Its consumer drops it as it drops any chained unit.
    unit.retain_for_deferred_recycle();
    let append = storage
        .recycle_completed_unit_through_frozen_last(&mut live, &mut mmio, cursor, 3)
        .unwrap()
        .expect("the frozen LAST releases the chain");
    assert_eq!(append.head_index.get(), 0);
    assert_eq!(append.descriptor_count, 3);
    live.complete_pending_reload(&mut mmio).unwrap();
    assert!(live.observed_mask().is_empty());
    for index in 0..3 {
        assert_eq!(
            storage.descriptors()[index].word0(),
            crate::descriptor::rx_armed_word(BUFFER).unwrap(),
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
    mmio: &mut crate::rx_dma::model::RxWalkerModel,
) -> (
    &'static RxDmaStorage<2, 16, 20, HostDma>,
    RxRingLive<'static, 2>,
) {
    let storage = Box::leak(Box::new(RxDmaStorage::<2, 16, 20, HostDma>::new()));
    let buffers = Box::leak(Box::new([0; 2]));
    let base = storage.dma_layout(buffers).unwrap();
    let mut live = storage
        .prepare_ring(mmio, base, buffers)
        .unwrap()
        .try_start(mmio)
        .map_err(|(_, error)| error)
        .unwrap();
    storage.descriptors()[0]
        .write_word0(crate::descriptor::rx_armed_word(16).unwrap() | crate::descriptor::BIT_30);
    mmio.set_cursor(base, 0, Some(1));
    let cursor = live.freeze_cursor(mmio);
    storage
        .take_completed_unit(&mut live, 1)
        .unwrap()
        .unwrap()
        .retain_for_deferred_recycle();
    storage
        .recycle_completed_unit_through_frozen_last(&mut live, mmio, cursor, 1)
        .unwrap()
        .expect("the frozen LAST releases descriptor zero");
    assert!(live.reload_pending());
    (storage, live)
}

#[test]
fn a_reload_doorbell_that_never_clears_is_busy_after_the_limit() {
    use crate::rx_dma::model::{RxWalkerModel, RxWalkerOperation};
    use crate::rx_ring::RX_DESCRIPTOR_RELOAD_ATTEMPT_LIMIT;
    let mut mmio = RxWalkerModel::new();
    let (storage, mut live) = ring_with_pending_reload(&mut mmio);
    mmio.reload_stuck = true;
    mmio.clear_operations();

    assert_eq!(
        live.complete_pending_reload(&mut mmio),
        Err(RxRingError::Busy)
    );
    assert!(mmio.overflowed(), "every attempt observed the doorbell");
    assert_eq!(
        live.reload_repair_evidence().max_reload_attempts,
        RX_DESCRIPTOR_RELOAD_ATTEMPT_LIMIT
    );
    assert!(live.reload_pending(), "the append stays pending");
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::Live);

    mmio.reload_stuck = false;
    mmio.clear_operations();
    assert_eq!(live.complete_pending_reload(&mut mmio), Ok(()));
    assert_eq!(
        mmio.operation_count(RxWalkerOperation::ObserveReloadPending),
        1
    );
    assert!(!live.reload_pending());
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn reload_attempts_record_the_largest_observation_count() {
    use crate::rx_dma::model::RxWalkerModel;
    let mut mmio = RxWalkerModel::new();
    mmio.reload_latency = 3;
    let (storage, mut live) = ring_with_pending_reload(&mut mmio);
    assert_eq!(live.reload_repair_evidence().max_reload_attempts, 0);

    live.complete_pending_reload(&mut mmio).unwrap();
    assert_eq!(live.reload_repair_evidence().max_reload_attempts, 4);
    assert!(!live.reload_pending());

    // Descriptor one completes and is appended behind descriptor zero; its
    // quicker reload does not lower the epoch's maximum.
    mmio.reload_latency = 1;
    storage.descriptors()[1]
        .write_word0(crate::descriptor::rx_armed_word(16).unwrap() | crate::descriptor::BIT_30);
    mmio.set_cursor(live.descriptor_base(), 1, Some(0));
    let cursor = live.freeze_cursor(&mut mmio);
    storage
        .take_completed_unit(&mut live, 1)
        .unwrap()
        .unwrap()
        .retain_for_deferred_recycle();
    storage
        .recycle_completed_unit_through_frozen_last(&mut live, &mut mmio, cursor, 1)
        .unwrap()
        .expect("the frozen LAST releases descriptor one");
    assert!(live.reload_pending());
    live.complete_pending_reload(&mut mmio).unwrap();
    assert!(!live.reload_pending());
    assert_eq!(live.reload_repair_evidence().max_reload_attempts, 4);
    assert!(live.try_stop(&mut mmio).is_ok());
}
