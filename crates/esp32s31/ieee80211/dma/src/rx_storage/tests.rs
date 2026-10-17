extern crate std;

use self::std::boxed::Box;
use super::*;
use crate::rx_dma::RxDmaBinding;

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
        crate::rx_dma::RxDmaNextDescriptor::validation(self.next_descriptor_low, false)
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
        observed(crate::rx_dma::RxDmaCursorObservation::validation(
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
        (!self.reload_pending()).then(|| settled(crate::rx_dma::RxDmaReloadSettled::validation()))
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
        (!self.ambiguous_enable).then(|| enabled(crate::rx_dma::RxDmaWalkerEnabled::validation()))
    }

    fn try_with_walker_stopped<R>(
        &mut self,
        stopped: impl for<'confirmation> FnOnce(crate::rx_dma::RxDmaWalkerStopped<'confirmation>) -> R,
    ) -> Option<R> {
        self.disable_calls += 1;
        if self.fail_disable {
            return None;
        }
        self.walker = false;
        Some(stopped(crate::rx_dma::RxDmaWalkerStopped::validation()))
    }

    fn fence(&mut self) {}
}

#[test]
fn arena_initializes_in_its_final_location_and_recycles_one_buffer() {
    let mut storage = RxDmaStorage::<2, 16, 20>::new();

    assert_eq!(storage.descriptors().len(), 2);
    assert_eq!(storage.buffers().len(), 2);
    assert_eq!(storage.buffers().as_ptr().addr() & 3, 0);

    storage.prepare_unpublished_buffer(0).unwrap();
    assert!(!storage.buffers()[0].leading_guard_overwritten());
}

#[test]
fn accepted_list_pressure_projects_next_to_the_current_tail() {
    const COUNT: usize = 4;
    const BASE: u32 = 0x2f00_1000;
    const LOW_MASK: u32 = 0x000f_ffff;
    let buffers = [0x2f00_2000, 0x2f00_2100, 0x2f00_2200, 0x2f00_2300];
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20>::new()));
    let mut mmio = MockRxDma::default();
    let prepared = storage
        .prepare_ring(&mut mmio, BASE, &buffers)
        .expect("prepared pressure model");
    let live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live pressure model");

    let next = |index: u32| {
        crate::rx_dma::RxDmaNextDescriptor::validation(
            (BASE + index * crate::descriptor::DESCRIPTOR_BYTES) & LOW_MASK,
            false,
        )
    };
    assert_eq!(live.accepted_list_remaining_from_next(next(0)), Some(4));
    assert_eq!(live.accepted_list_remaining_from_next(next(1)), Some(3));
    assert_eq!(live.accepted_list_remaining_from_next(next(3)), Some(1));
    assert_eq!(
        live.accepted_list_remaining_from_next(crate::rx_dma::RxDmaNextDescriptor::validation(
            0, false
        )),
        Some(0)
    );
    assert_eq!(
        live.accepted_list_remaining_from_next(crate::rx_dma::RxDmaNextDescriptor::validation(
            0, true
        )),
        None,
        "upper-only NEXT is not an exhausted-list proof"
    );
    assert_eq!(
        live.accepted_list_remaining_from_next(crate::rx_dma::RxDmaNextDescriptor::validation(
            (BASE + COUNT as u32 * crate::descriptor::DESCRIPTOR_BYTES) & LOW_MASK,
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
    const BASE: u32 = 0x2f00_1000;
    const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
    // Descriptor zero names physical buffer one; descriptor one names
    // physical buffer zero. Synthetic host addresses preserve that order.
    let buffers = [0x2f00_2200, 0x2f00_2000];
    let mut storage = Box::new(RxDmaStorage::<COUNT, 16, 20>::new());
    storage.buffer_mut(1).unwrap()[4..8].copy_from_slice(&[5, 6, 7, 8]);
    let storage = Box::leak(storage);
    storage
        .bind_descriptor_rotation(1)
        .expect("pre-ring rotation");
    assert_eq!(storage.descriptor_buffer_id(0), Some(1));
    assert_eq!(storage.descriptor_buffer_id(1), Some(0));

    let mut mmio = MockRxDma::default();
    let prepared = storage
        .prepare_ring(&mut mmio, BASE, &buffers)
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
    mmio.last_descriptor_low = (BASE + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
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
    const BASE: u32 = 0x2f00_1000;
    const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
    let buffers = [0x2f00_2000, 0x2f00_2200];
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20>::new()));
    storage.buffer_mut(0).unwrap()[4..8].copy_from_slice(&[1, 2, 3, 4]);
    let mut mmio = MockRxDma::default();
    let prepared = storage
        .prepare_ring(&mut mmio, BASE, &buffers)
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
    mmio.last_descriptor_low = (BASE + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
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
    const BASE: u32 = 0x2f00_1000;
    const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
    let buffers = [0x2f00_2000, 0x2f00_2200];
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20>::new()));
    let mut mmio = MockRxDma::default();
    let prepared = storage
        .prepare_ring(&mut mmio, BASE, &buffers)
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
    mmio.last_descriptor_low = (BASE + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
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
    const BASE: u32 = 0x2f00_1000;
    const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
    let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20>::new()));
    let mut mmio = MockRxDma::default();
    let prepared = storage
        .prepare_ring(&mut mmio, BASE, &buffers)
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
    mmio.last_descriptor_low = (BASE + 3 * crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
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
    const BASE: u32 = 0x2f00_1000;
    let buffers = [0x2f00_2000, 0x2f00_2200];
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20>::new()));
    let mut first_mmio = MockRxDma::default();
    let mut second_mmio = MockRxDma::default();

    let first = storage
        .prepare_ring(&mut first_mmio, BASE, &buffers)
        .expect("first root capability");
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::Prepared);
    assert!(matches!(
        storage.prepare_ring(&mut second_mmio, BASE, &buffers),
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
        storage.prepare_ring(&mut second_mmio, BASE, &buffers),
        Err(RxRingError::Busy)
    ));
}

#[test]
fn live_binding_rejects_a_foreign_storage_arena() {
    const COUNT: usize = 2;
    const BASE: u32 = 0x2f00_1000;
    let buffers = [0x2f00_2000, 0x2f00_2200];
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20>::new()));
    let foreign = RxDmaStorage::<COUNT, 16, 20>::new();
    let mut mmio = MockRxDma::default();
    let live = storage
        .prepare_ring(&mut mmio, BASE, &buffers)
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
    const BASE: u32 = 0x2f00_1000;
    const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
    let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20>::new()));
    let mut mmio = MockRxDma::default();
    let prepared = storage
        .prepare_ring(&mut mmio, BASE, &buffers)
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
    mmio.last_descriptor_low = (BASE + 3 * crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    mmio.next_descriptor_low = BASE & ADDRESS_LOW_MASK;

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
    const BASE: u32 = 0x2f00_1000;
    let buffers = [0x2f00_2000, 0x2f00_2200];
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20>::new()));
    let mut mmio = MockRxDma {
        walker: true,
        fail_disable: true,
        ..MockRxDma::default()
    };

    assert!(matches!(
        storage.prepare_ring(&mut mmio, BASE, &buffers),
        Err(RxRingError::Busy)
    ));
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::ResetRequired);
}

#[test]
fn failed_halted_prepare_quarantines_the_arena_when_walker_stop_is_unproved() {
    const COUNT: usize = 2;
    const BASE: u32 = 0x2f00_1000;
    let buffers = [0x2f00_2000, 0x2f00_2200];
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20>::new()));
    let mut mmio = MockRxDma::default();
    let prepared = storage
        .prepare_ring(&mut mmio, BASE, &buffers)
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
    const BASE: u32 = 0x2f00_1000;
    const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
    let buffers = [0x2f00_2000, 0x2f00_2200];
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20>::new()));
    let mut mmio = MockRxDma::default();
    let prepared = storage
        .prepare_ring(&mut mmio, BASE, &buffers)
        .expect("prepared owner");
    let mut live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live epoch");
    storage.descriptors()[0].write_word0(
        crate::descriptor::rx_armed_word(16).expect("valid size") | crate::descriptor::BIT_30,
    );
    mmio.last_descriptor_low = BASE & ADDRESS_LOW_MASK;
    mmio.next_descriptor_low = (BASE + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    assert!(live.take_completed(0).is_some());

    assert_eq!(
        live.recycle_completed_prefix::<1, _, _>(&mut mmio, |_| Err(RxRingError::Size)),
        Ok(None),
    );
    storage.descriptors()[1].write_word0(
        crate::descriptor::rx_armed_word(16).expect("valid size") | crate::descriptor::BIT_30,
    );
    mmio.last_descriptor_low = (BASE + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    assert_eq!(
        live.recycle_completed_prefix::<1, _, _>(&mut mmio, |_| Err(RxRingError::Size)),
        Err(RxRingError::Size),
    );
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::ResetRequired);
}

#[test]
fn completed_descriptor_metadata_is_validated_before_payload_transfer() {
    const COUNT: usize = 2;
    const BASE: u32 = 0x2f00_1000;
    let buffers = [0x2f00_2000, 0x2f00_2200];

    for corruption in 0..3 {
        let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20>::new()));
        let mut mmio = MockRxDma::default();
        let prepared = storage
            .prepare_ring(&mut mmio, BASE, &buffers)
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
        let mut next_address = BASE + crate::descriptor::DESCRIPTOR_BYTES;
        match corruption {
            0 => word0 = (word0 & !crate::descriptor::SIZE_MASK) | 17,
            1 => buffer_address = buffers[0] + 4,
            2 => next_address = 0,
            _ => unreachable!(),
        }
        descriptor.publish(word0, buffer_address, next_address);

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
    const BASE: u32 = 0x2f00_1000;
    const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
    let buffers = [0x2f00_2000, 0x2f00_2200];
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20>::new()));
    let mut mmio = MockRxDma::default();
    let prepared = storage
        .prepare_ring(&mut mmio, BASE, &buffers)
        .expect("prepared owner");
    let mut live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live epoch");
    storage.descriptors()[0].write_word0(
        crate::descriptor::rx_armed_word(16).expect("valid size") | crate::descriptor::BIT_30,
    );
    mmio.last_descriptor_low = BASE & ADDRESS_LOW_MASK;
    mmio.next_descriptor_low = (BASE + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
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
    mmio.last_descriptor_low = (BASE + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    assert!(
        storage
            .recycle_completed_prefix::<1, _>(&mut live, &mut mmio)
            .unwrap()
            .is_some()
    );
    assert!(live.reload_pending());

    mmio.next_descriptor_low = 0;
    mmio.last_descriptor_low = (BASE + 1) & ADDRESS_LOW_MASK;
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
    const BASE: u32 = 0x2f00_1000;
    const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
    let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20>::new()));
    let mut mmio = MockRxDma::default();
    let prepared = storage
        .prepare_ring(&mut mmio, BASE, &buffers)
        .expect("prepared owner");
    let mut live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live epoch");
    storage.descriptors()[0].write_word0(
        crate::descriptor::rx_armed_word(16).expect("valid size") | crate::descriptor::BIT_30,
    );
    mmio.last_descriptor_low = BASE & ADDRESS_LOW_MASK;
    mmio.next_descriptor_low = (BASE + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
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
    mmio.last_descriptor_low = (BASE + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
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
    mmio.last_descriptor_low = (BASE + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    assert_eq!(
        live.poll_pending_reload(&mut mmio),
        Ok(crate::rx_ring::RxReloadObservation::Settled)
    );
    assert_eq!(
        mmio.descriptor_base,
        BASE + 2 * crate::descriptor::DESCRIPTOR_BYTES
    );
    assert_eq!(storage.lifecycle_state(), RxDmaArenaState::Live);
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn exhausted_reload_records_the_direct_base_repair_facts() {
    const COUNT: usize = 2;
    const BASE: u32 = 0x2f00_1000;
    const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
    let buffers = [0x2f00_2000, 0x2f00_2200];
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20>::new()));
    let mut mmio = MockRxDma::default();
    let prepared = storage
        .prepare_ring(&mut mmio, BASE, &buffers)
        .expect("prepared owner");
    let mut live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live epoch");
    storage.descriptors()[0].write_word0(
        crate::descriptor::rx_armed_word(16).expect("valid size") | crate::descriptor::BIT_30,
    );
    mmio.last_descriptor_low = BASE & ADDRESS_LOW_MASK;
    mmio.next_descriptor_low = (BASE + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
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
    mmio.last_descriptor_low = (BASE + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    assert!(
        storage
            .recycle_completed_prefix::<1, _>(&mut live, &mut mmio)
            .unwrap()
            .is_some()
    );

    // The walker exhausted the previously accepted tail before observing
    // its new link. The vendor suffix republishes that link through BASE.
    mmio.next_descriptor_low = 0;
    mmio.last_descriptor_low = (BASE + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    assert_eq!(
        live.poll_pending_reload(&mut mmio),
        Ok(crate::rx_ring::RxReloadObservation::Settled)
    );
    assert_eq!(mmio.descriptor_base, BASE);
    assert_eq!(
        live.reload_repair_evidence(),
        crate::rx_ring::RxReloadRepairEvidence {
            observations: 1,
            unknown_upper_with_zero_address: 0,
            base_repairs: 1,
            last_next_low: 0,
            last_last_low: Some((BASE + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK),
            last_repair_head: Some(BASE),
        }
    );
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn exhausted_list_reclaims_untouched_prefix_before_terminal_payload() {
    const COUNT: usize = 4;
    const BASE: u32 = 0x2f00_1000;
    const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
    let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20>::new()));
    let mut mmio = MockRxDma::default();
    let prepared = storage
        .prepare_ring(&mut mmio, BASE, &buffers)
        .expect("prepared owner");
    let mut live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live epoch");

    // Return descriptor zero while the old finite list has already
    // exhausted at descriptor three. The vendor append suffix repairs
    // BASE to the returned descriptor, leaving software head at one.
    storage.descriptors()[0].write_word0(
        16 | (4 << crate::descriptor::LENGTH_SHIFT)
            | crate::descriptor::BIT_30
            | crate::descriptor::BIT_31,
    );
    mmio.last_descriptor_low = (BASE + 3 * crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
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
    mmio.last_descriptor_low = BASE & ADDRESS_LOW_MASK;
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
    const BASE: u32 = 0x2f00_1000;
    const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
    let buffers = [0x2f00_2000, 0x2f00_2200];
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20>::new()));
    let mut mmio = MockRxDma::default();
    let prepared = storage
        .prepare_ring(&mut mmio, BASE, &buffers)
        .expect("prepared owner");
    let mut live = prepared
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .expect("live epoch");
    storage.descriptors()[0].write_word0(
        crate::descriptor::rx_armed_word(16).expect("valid size") | crate::descriptor::BIT_30,
    );
    mmio.last_descriptor_low = BASE & ADDRESS_LOW_MASK;
    mmio.next_descriptor_low = (BASE + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
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
    const BASE: u32 = 0x2f00_1000;
    const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
    let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20>::new()));
    let mut mmio = MockRxDma::default();
    let prepared = storage
        .prepare_ring(&mut mmio, BASE, &buffers)
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
    mmio.last_descriptor_low = BASE & ADDRESS_LOW_MASK;
    mmio.next_descriptor_low = (BASE + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
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
    const BASE: u32 = 0x2f00_1000;
    const ADDRESS_LOW_MASK: u32 = 0x000f_ffff;
    let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20>::new()));
    let mut mmio = MockRxDma::default();
    let prepared = storage
        .prepare_ring(&mut mmio, BASE, &buffers)
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
    mmio.last_descriptor_low = (BASE + crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
    mmio.next_descriptor_low = (BASE + 2 * crate::descriptor::DESCRIPTOR_BYTES) & ADDRESS_LOW_MASK;
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
    const BASE: u32 = 0x2f00_1000;
    let buffers = [0x2f00_2000, 0x2f00_2200];
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20>::new()));
    let mut mmio = MockRxDma {
        ambiguous_enable: true,
        ..MockRxDma::default()
    };
    let prepared = storage
        .prepare_ring(&mut mmio, BASE, &buffers)
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
    let new = RxDmaStorage::<4, 16, 20>::new();
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
