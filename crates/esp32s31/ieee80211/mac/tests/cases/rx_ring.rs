use crate::{support::*, *};

type Arena<const COUNT: usize> = HostRxArena<COUNT, 256, 260>;
const BUFFER_SIZE: u32 = 256;
const COMPLETED: u32 = BUFFER_SIZE | (80 << LENGTH_SHIFT) | BIT_30 | BIT_31;

#[test]
fn cold_rx_ring_publishes_links_and_hardware_in_order() {
    let arena = HostRxArena::<2, 1700, 1704>::new();
    let descriptors = arena.descriptors();
    let mut mmio = MockMmio::default();
    let stopped = arena.prepare(&mut mmio).unwrap();
    assert_eq!(descriptors[0].next_address(), arena.address(1));
    assert_eq!(descriptors[1].next_address(), 0);
    assert_eq!(
        mmio.operations(),
        &[
            Operation::ObserveRxWalkerEnabled,
            Operation::Fence,
            Operation::ObserveRxNextDescriptor,
            Operation::ObserveRxLastDescriptor,
            Operation::Fence,
            Operation::ConfigureRxDescriptorWindow,
            Operation::PublishRxDescriptorBase(arena.base),
            Operation::Fence,
        ]
    );

    // The walker enable is the later, separate edge of the cold path.
    mmio.operations.clear();
    let live = stopped
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .unwrap();
    assert_eq!(
        mmio.operations(),
        &[
            Operation::ObserveRxWalkerEnabled,
            Operation::PublishRxWalkerEnable,
            Operation::Fence,
            Operation::ObserveRxWalkerEnabled,
        ]
    );
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn completed_rx_descriptor_rearms_only_for_the_expected_storage() {
    let arena = Arena::<2>::new();
    let (storage, descriptors) = (arena.storage, arena.descriptors());
    let mut mmio = MockMmio::default();
    let mut live = arena.start(&mut mmio);
    descriptors[0].write_word0(COMPLETED);
    mmio.set_rx_last_descriptor_address(arena.base);
    mmio.set_rx_next_descriptor_address(arena.address(1));
    assert!(storage.take_completed(&mut live, 0).unwrap().is_some());
    confirm_completed_unit_link_release(
        &mut live,
        &mut mmio,
        descriptors,
        arena.base,
        arena.base,
        1,
    );
    assert!(
        storage
            .recycle_completed_prefix::<1, _>(&mut live, &mut mmio)
            .unwrap()
            .is_some()
    );
    assert_eq!(length(descriptors[0].word0()), 256);
    assert_ne!(descriptors[0].word0() & BIT_31, 0);
    assert_eq!(descriptors[0].word0() & BIT_30, 0);

    // Descriptor one completed while the append settled. A buffer address
    // which is not the arena buffer bound to it must not be rearmed.
    mmio.set_rx_reload_pending(false);
    assert!(storage.take_completed(&mut live, 1).unwrap().is_some());
    let completed = descriptors[1].word0();
    descriptors[1].inject_corrupt_words(completed, arena.buffers[0], descriptors[1].next_address());
    assert_eq!(
        storage.recycle_completed_prefix::<1, _>(&mut live, &mut mmio),
        Err(RxRingError::Corrupt)
    );
    assert_eq!(descriptors[1].word0(), completed);
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn recycled_rx_buffer_restores_both_migration_sentinels() {
    let mut storage = [0x5a; 20];
    prepare_recycled_buffer(&mut storage, 16).unwrap();
    assert_eq!(&storage[..4], &RX_BUFFER_SENTINEL.to_le_bytes());
    assert_eq!(&storage[4..16], &[0x5a; 12]);
    assert_eq!(&storage[16..20], &RX_BUFFER_SENTINEL.to_le_bytes());
    assert_eq!(
        prepare_recycled_buffer(&mut storage[..16], 16),
        Err(RxRingError::Size)
    );
}

#[test]
fn live_rx_ring_owns_physical_cold_order_reload_and_rom_base_repair() {
    const COUNT: usize = 4;
    let arena = Arena::<COUNT>::new();
    let (storage, descriptors, base) = (arena.storage, arena.descriptors(), arena.base);
    let mut mmio = MockMmio::default();
    // A previous last pointer remains diagnostic only. A stopped/rebuilt rev0
    // list must begin at physical zero so it never depends on a cold 31->0
    // wrap link.
    mmio.set_rx_last_descriptor_address(base + DESCRIPTOR_BYTES);
    mmio.set_rx_walker_enabled(true);

    let stopped = arena.prepare(&mut mmio).unwrap();
    assert_eq!(stopped.initial_start().get(), 0);
    assert_eq!(stopped.accepted_tail().get(), 3);
    assert_eq!(descriptors[2].next_address(), base + 3 * DESCRIPTOR_BYTES);
    assert_eq!(descriptors[3].next_address(), 0);
    assert_eq!(descriptors[0].next_address(), base + DESCRIPTOR_BYTES);
    assert_eq!(descriptors[1].next_address(), base + 2 * DESCRIPTOR_BYTES);
    assert_eq!(mmio.rx_descriptor_base(), base);
    let disable = mmio
        .operations()
        .iter()
        .position(|operation| *operation == Operation::StopRxWalker)
        .unwrap();
    let retained_last = mmio
        .operations()
        .iter()
        .position(|operation| *operation == Operation::ObserveRxLastDescriptor)
        .unwrap();
    assert!(disable < retained_last);
    assert!(mmio.operations()[disable + 1..retained_last].contains(&Operation::Fence));
    let topology = stopped.topology_snapshot();
    assert!(topology.valid);
    assert_eq!(topology.start_index, 0);
    assert_eq!(topology.tail_index, 3);
    assert_eq!(topology.visited_descriptors, COUNT);
    assert_eq!(topology.terminal_descriptors, 1);

    let mut live = stopped
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .unwrap();
    // The buffers held stale payload; starting touches no buffer, so these
    // are the guards preparation restored in every buffer.
    for index in 0..COUNT {
        assert!(
            arena.guards_restored(&live, index),
            "buffer {index} prepared"
        );
    }
    // DMA has written into every buffer; descriptors zero and one complete.
    for index in 0..COUNT {
        arena.dma_write(&live, index, &[0xa5; 80]);
    }
    descriptors[0].write_word0(COMPLETED);
    descriptors[1].write_word0(COMPLETED);
    mmio.set_rx_last_descriptor_address(base + DESCRIPTOR_BYTES);
    mmio.set_rx_next_descriptor_address(base + 2 * DESCRIPTOR_BYTES);
    assert_eq!(
        storage
            .take_completed(&mut live, 0)
            .unwrap()
            .unwrap()
            .index(),
        0
    );
    assert!(storage.take_completed(&mut live, 0).unwrap().is_none());
    assert_eq!(
        storage
            .take_completed(&mut live, 1)
            .unwrap()
            .unwrap()
            .index(),
        1
    );
    confirm_completed_unit_link_release(
        &mut live,
        &mut mmio,
        descriptors,
        base,
        base + DESCRIPTOR_BYTES,
        2,
    );

    let first = storage
        .recycle_completed_half(&mut live, &mut mmio)
        .unwrap()
        .unwrap();
    let recycled = |arena: &Arena<COUNT>, live: &RxRingLive<'_, COUNT>| {
        (0..COUNT)
            .filter(|&index| arena.guards_restored(live, index))
            .collect::<Vec<_>>()
    };
    assert_eq!(recycled(&arena, &live), [0, 1]);
    assert_eq!(first.head_index.get(), 0);
    assert_eq!(first.tail_index.get(), 1);
    assert_eq!(descriptors[3].next_address(), base);
    assert!(mmio.rx_reload_pending);
    assert!(live.reload_pending());
    assert_eq!(live.accepted_tail().get(), 3);

    for index in 0..COUNT {
        arena.dma_write(&live, index, &[0xa5; 80]);
    }
    descriptors[2].write_word0(COMPLETED);
    descriptors[3].write_word0(COMPLETED);
    assert!(storage.take_completed(&mut live, 2).unwrap().is_some());
    assert!(storage.take_completed(&mut live, 3).unwrap().is_some());

    // Model bit-0 self-clear at a terminal frontier. ROM repairs BASE from the
    // last accepted descriptor's now-published next link before accepting the
    // pending tail and appending the following group.
    mmio.set_rx_walker_enabled(true);
    mmio.set_rx_reload_pending(false);
    mmio.set_rx_next_descriptor_address(0);
    mmio.set_rx_last_descriptor_address(base + 3 * DESCRIPTOR_BYTES);
    mmio.operations.clear();
    assert_eq!(
        live.poll_pending_reload(&mut mmio).unwrap(),
        RxReloadObservation::Settled
    );
    assert_eq!(
        &mmio.operations()[..6],
        &[
            Operation::ObserveRxReloadPending,
            Operation::ObserveRxNextDescriptor,
            Operation::Fence,
            Operation::ObserveRxLastDescriptor,
            Operation::Fence,
            Operation::PublishRxDescriptorBase(base),
        ],
        "reload repair must preserve vendor NEXT -> conditional LAST -> BASE order",
    );
    assert_eq!(live.accepted_tail().get(), 1);
    assert!(!live.reload_pending());
    assert!(live.exhausted_republication_probe_pending());
    // LAST reached descriptor three while NEXT was zero, so the base-repair
    // write has been issued but hardware has not yet proved that it fetched
    // descriptor three's newly appended link to descriptor zero.
    assert!(
        storage
            .recycle_completed_half(&mut live, &mut mmio)
            .unwrap()
            .is_none()
    );
    assert!(live.completion_release_probe_pending());
    mmio.set_rx_next_descriptor_address(base);
    assert!(
        storage
            .recycle_completed_half(&mut live, &mut mmio)
            .unwrap()
            .is_none()
    );
    assert!(recycled(&arena, &live).is_empty());
    // Repeated NEXT observations still do not release descriptor three's
    // link. A later completed LAST does.
    descriptors[0].write_word0(descriptors[0].word0() | BIT_30);
    mmio.set_rx_last_descriptor_address(base);
    let second = storage
        .recycle_completed_half(&mut live, &mut mmio)
        .unwrap()
        .unwrap();
    assert_eq!(recycled(&arena, &live), [2, 3]);
    assert_eq!(second.head_index.get(), 2);
    assert_eq!(second.tail_index.get(), 3);
    assert_eq!(descriptors[1].next_address(), base + 2 * DESCRIPTOR_BYTES);
    assert!(
        mmio.operations()
            .contains(&Operation::PublishRxDescriptorBase(base))
    );
    assert_eq!(live.accepted_tail().get(), 1);
    assert!(live.reload_pending());
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn reload_repair_observation_reads_last_only_after_zero_next() {
    const BASE: u32 = 0x2f00_1000;
    let mut mmio = MockMmio::default();
    mmio.set_rx_next_descriptor_address(BASE + DESCRIPTOR_BYTES);
    mmio.set_rx_last_descriptor_address(BASE);

    let active = mmio.with_reload_repair_observation(|observation| {
        (
            observation.next_descriptor_low(),
            observation.exhausted_last_descriptor_low(),
        )
    });
    assert_eq!(active, (rx_descriptor_low(BASE + DESCRIPTOR_BYTES), None));
    assert_eq!(
        mmio.operations(),
        &[Operation::ObserveRxNextDescriptor, Operation::Fence],
    );

    mmio.operations.clear();
    mmio.set_rx_next_descriptor_address(0);
    let exhausted = mmio.with_reload_repair_observation(|observation| {
        (
            observation.next_descriptor_low(),
            observation.exhausted_last_descriptor_low(),
        )
    });
    assert_eq!(exhausted, (0, Some(rx_descriptor_low(BASE))));
    assert_eq!(
        mmio.operations(),
        &[
            Operation::ObserveRxNextDescriptor,
            Operation::Fence,
            Operation::ObserveRxLastDescriptor,
            Operation::Fence,
        ],
    );
}

#[test]
fn stopped_rx_ring_ignores_every_retained_last_for_cold_publication() {
    const COUNT: usize = 32;

    for retained_index in 0..COUNT {
        let arena = Arena::<COUNT>::new();
        let mut mmio = MockMmio::default();
        mmio.set_rx_last_descriptor_address(arena.address(retained_index));

        let stopped = arena.prepare(&mut mmio).unwrap();
        let topology = stopped.topology_snapshot();
        assert_eq!(stopped.initial_start().get(), 0);
        assert_eq!(stopped.accepted_tail().get(), COUNT - 1);
        assert!(topology.valid, "retained descriptor {retained_index}");
        assert_eq!(topology.start_index, 0);
        assert_eq!(topology.tail_index, COUNT - 1);
        assert_eq!(topology.visited_descriptors, COUNT);
        assert_eq!(topology.terminal_descriptors, 1);
        assert_eq!(arena.descriptors()[COUNT - 1].next_address(), 0);
    }
}

#[test]
fn stopped_rx_ring_rebuilds_from_the_retained_hardware_next_cursor() {
    const COUNT: usize = 4;
    let arena = Arena::<COUNT>::new();
    let (descriptors, base) = (arena.descriptors(), arena.base);
    let mut mmio = MockMmio::default();
    mmio.set_rx_walker_enabled(true);
    mmio.set_rx_next_descriptor_address(base + DESCRIPTOR_BYTES);
    mmio.set_rx_last_descriptor_address(base);

    let stopped = arena.prepare(&mut mmio).unwrap();

    assert_eq!(
        stopped.retained_next_low(),
        rx_descriptor_low(base + DESCRIPTOR_BYTES)
    );
    assert_eq!(stopped.retained_last_low(), rx_descriptor_low(base));
    assert_eq!(stopped.initial_start().get(), 1);
    assert_eq!(stopped.accepted_tail().get(), 0);
    assert_eq!(descriptors[1].next_address(), base + 2 * DESCRIPTOR_BYTES);
    assert_eq!(descriptors[3].next_address(), base);
    assert_eq!(descriptors[0].next_address(), 0);
    assert_eq!(mmio.rx_descriptor_base(), base + DESCRIPTOR_BYTES);
    assert!(stopped.topology_snapshot().valid);
}

#[test]
fn stopped_rx_ring_rejects_a_nonzero_cursor_outside_its_owned_arena() {
    const COUNT: usize = 4;
    let arena = Arena::<COUNT>::new();
    let mut mmio = MockMmio::default();
    mmio.set_rx_walker_enabled(true);
    mmio.set_rx_next_descriptor_address(arena.address(COUNT));

    assert!(matches!(
        arena.prepare(&mut mmio),
        Err(RxRingError::Corrupt)
    ));
}

#[test]
fn stopped_rx_ring_avoids_a_cold_head_on_the_final_descriptor() {
    const COUNT: usize = 32;
    let arena = Arena::<COUNT>::new();
    let (descriptors, base) = (arena.descriptors(), arena.base);
    let mut mmio = MockMmio::default();
    mmio.set_rx_last_descriptor_address(arena.address(COUNT - 2));

    let stopped = arena.prepare(&mut mmio).unwrap();

    assert_eq!(stopped.initial_start().get(), 0);
    assert_eq!(stopped.accepted_tail().get(), COUNT - 1);
    assert_eq!(descriptors[0].next_address(), base + DESCRIPTOR_BYTES);
    assert_eq!(descriptors[COUNT - 1].next_address(), 0);
    assert_eq!(mmio.rx_descriptor_base(), base);
    assert!(stopped.topology_snapshot().valid);
}

#[test]
fn stopped_rx_ring_uses_zero_for_an_invalid_retained_last_pointer() {
    const COUNT: usize = 4;

    for retained_offset in [None, Some(1), Some(COUNT as u32 * DESCRIPTOR_BYTES)] {
        let arena = Arena::<COUNT>::new();
        let mut mmio = MockMmio::default();
        mmio.set_rx_last_descriptor_address(
            retained_offset.map_or(0, |offset| arena.base + offset),
        );
        let stopped = arena.prepare(&mut mmio).unwrap();
        assert_eq!(stopped.initial_start().get(), 0);
        assert!(stopped.topology_snapshot().valid);
    }
}

#[test]
fn stopped_rx_ring_rejects_corrupt_topology_before_walker_enable() {
    const COUNT: usize = 4;
    let arena = Arena::<COUNT>::new();
    let descriptors = arena.descriptors();
    let mut mmio = MockMmio::default();
    mmio.set_rx_walker_enabled(false);
    let stopped = arena.prepare(&mut mmio).unwrap();
    assert!(stopped.topology_snapshot().valid);

    descriptors[0].inject_corrupt_words(descriptors[0].word0(), arena.buffers[0], 0);
    assert!(!stopped.topology_snapshot().valid);
    let (stopped, error) = match stopped.try_start(&mut mmio) {
        Ok(_) => panic!("corrupt RX topology started"),
        Err(failure) => failure,
    };
    assert_eq!(error, RxRingError::Corrupt);
    assert!(!mmio.walker_enabled());
    assert!(!stopped.topology_snapshot().valid);
}

#[test]
fn live_rx_ring_can_replenish_one_descriptor_per_rom_append() {
    const COUNT: usize = 4;
    let arena = Arena::<COUNT>::new();
    let (storage, descriptors, base) = (arena.storage, arena.descriptors(), arena.base);
    let mut mmio = MockMmio::default();
    let mut live = arena.start(&mut mmio);

    descriptors[0].write_word0(COMPLETED);
    mmio.set_rx_last_descriptor_address(base);
    mmio.set_rx_next_descriptor_address(base + DESCRIPTOR_BYTES);
    assert!(storage.take_completed(&mut live, 0).unwrap().is_some());
    confirm_completed_unit_link_release(&mut live, &mut mmio, descriptors, base, base, 1);
    let first = storage
        .recycle_completed_prefix::<1, _>(&mut live, &mut mmio)
        .unwrap()
        .unwrap();
    assert_eq!(first.head_index.get(), 0);
    assert_eq!(first.tail_index.get(), 0);
    assert_eq!(descriptors[3].next_address(), base);

    // Model the doorbell self-clear while the walker still has a live next
    // pointer. No BASE repair is required for this ordinary append.
    mmio.set_rx_walker_enabled(true);
    mmio.set_rx_reload_pending(false);
    mmio.set_rx_next_descriptor_address(base + DESCRIPTOR_BYTES);
    assert_eq!(
        live.poll_pending_reload(&mut mmio).unwrap(),
        RxReloadObservation::Settled
    );
    assert_eq!(live.accepted_tail().get(), 0);

    descriptors[1].write_word0(COMPLETED);
    mmio.set_rx_last_descriptor_address(base + DESCRIPTOR_BYTES);
    mmio.set_rx_next_descriptor_address(base + 2 * DESCRIPTOR_BYTES);
    assert!(storage.take_completed(&mut live, 1).unwrap().is_some());
    confirm_completed_unit_link_release(
        &mut live,
        &mut mmio,
        descriptors,
        base,
        base + DESCRIPTOR_BYTES,
        1,
    );
    let second = storage
        .recycle_completed_prefix::<1, _>(&mut live, &mut mmio)
        .unwrap()
        .unwrap();
    assert_eq!(second.head_index.get(), 1);
    assert_eq!(second.tail_index.get(), 1);
    assert_eq!(descriptors[0].next_address(), base + DESCRIPTOR_BYTES);

    // A group bound must name at least one and at most every descriptor.
    assert_eq!(
        storage.recycle_completed_prefix::<0, _>(&mut live, &mut mmio),
        Err(RxRingError::Count)
    );
    assert_eq!(
        storage.recycle_completed_prefix::<{ COUNT + 1 }, _>(&mut live, &mut mmio),
        Err(RxRingError::Count)
    );
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn live_rx_ring_republishes_an_exhausted_software_list_without_a_self_link() {
    const COUNT: usize = 2;
    let arena = Arena::<COUNT>::new();
    let (storage, descriptors, base) = (arena.storage, arena.descriptors(), arena.base);
    let mut mmio = MockMmio::default();
    let mut live = arena.start(&mut mmio);

    // First append descriptor zero normally, making it the accepted tail of
    // the software list 1 -> 0.
    descriptors[0].write_word0(COMPLETED);
    mmio.set_rx_last_descriptor_address(base);
    mmio.set_rx_next_descriptor_address(base + DESCRIPTOR_BYTES);
    assert!(storage.take_completed(&mut live, 0).unwrap().is_some());
    confirm_completed_unit_link_release(&mut live, &mut mmio, descriptors, base, base, 1);
    storage
        .recycle_completed_prefix::<1, _>(&mut live, &mut mmio)
        .unwrap()
        .unwrap();
    mmio.set_rx_walker_enabled(true);
    mmio.set_rx_reload_pending(false);
    mmio.set_rx_next_descriptor_address(base + DESCRIPTOR_BYTES);
    assert_eq!(
        live.poll_pending_reload(&mut mmio).unwrap(),
        RxReloadObservation::Settled
    );
    assert_eq!(live.accepted_tail().get(), 0);

    // Hardware then exhausts that whole list before software returns either
    // node. Discarding 1 -> 0 leaves the vendor software head null, so the
    // returned chain must become the new BASE directly. Linking old tail zero
    // to head one would create the invalid cycle 1 -> 0 -> 1.
    descriptors[1].write_word0(COMPLETED);
    descriptors[0].write_word0(COMPLETED);
    mmio.set_rx_next_descriptor_address(0);
    mmio.set_rx_last_descriptor_address(base);
    assert!(storage.take_completed(&mut live, 1).unwrap().is_some());
    assert!(storage.take_completed(&mut live, 0).unwrap().is_some());
    mmio.operations.clear();
    let append = storage
        .recycle_completed_prefix::<COUNT, _>(&mut live, &mut mmio)
        .unwrap()
        .unwrap();

    assert_eq!(append.head_index.get(), 1);
    assert_eq!(append.tail_index.get(), 0);
    assert_eq!(descriptors[1].next_address(), base);
    assert_eq!(descriptors[0].next_address(), 0);
    assert_eq!(mmio.rx_descriptor_base(), base + DESCRIPTOR_BYTES);
    assert_eq!(live.recycle_start().get(), 1);
    assert!(!live.reload_pending());
    assert!(live.exhausted_republication_probe_pending());
    assert!(!mmio.operations().contains(&Operation::RequestRxReload));

    // A timer is not evidence that hardware accepted BASE. Keep polling while
    // NEXT is still exhausted. Even an exact cursor match retains one final
    // cooperative probe: the returned head may complete while this task is
    // still consuming the IRQ which exhausted the preceding list.
    live.observe_exhausted_republication(&mut mmio);
    assert!(live.exhausted_republication_probe_pending());
    mmio.set_rx_next_descriptor_address(base);
    live.observe_exhausted_republication(&mut mmio);
    assert!(
        live.exhausted_republication_probe_pending(),
        "a nonzero cursor outside the republished head is stale evidence"
    );
    mmio.set_rx_next_descriptor_address(base + DESCRIPTOR_BYTES);
    live.observe_exhausted_republication(&mut mmio);
    assert!(live.exhausted_republication_probe_pending());
    live.observe_exhausted_republication(&mut mmio);
    assert!(!live.exhausted_republication_probe_pending());

    // Hardware resumes at the newly published head. The next RX edge must
    // inspect that same descriptor rather than the physical slot after the
    // returned chain's tail.
    descriptors[1].write_word0(COMPLETED);
    mmio.set_rx_last_descriptor_address(base + DESCRIPTOR_BYTES);
    let frontier = live.completed_unit_frontier_through_with(mmio.last_descriptor_low(), |_| true);
    assert_eq!(frontier.unit_count, 1);
    assert_eq!(frontier.descriptor_count, 1);
    let unit = storage.take_completed_unit(&mut live, 1).unwrap().unwrap();
    assert_eq!(unit.head_index(), 1);
    unit.retain_for_deferred_recycle();
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn live_rx_ring_does_not_rewrite_a_nonterminal_link_before_next_accepts_it() {
    const COUNT: usize = 4;
    let arena = Arena::<COUNT>::new();
    let (storage, descriptors, base) = (arena.storage, arena.descriptors(), arena.base);
    let mut mmio = MockMmio::default();
    let mut live = arena.start(&mut mmio);
    descriptors[0].write_word0(COMPLETED);

    // LAST/RX_DONE can precede the walker's fetch of descriptor zero's link.
    // Rewriting that nonzero link to the recycle-chain terminal here would
    // strand descriptors one through three.
    mmio.set_rx_last_descriptor_address(base);
    mmio.set_rx_next_descriptor_low(0);
    assert!(!live.observe_current_completed_unit_link_release(&mut mmio, 1));
    assert!(live.completion_release_probe_pending());
    assert_eq!(descriptors[0].next_address(), base + DESCRIPTOR_BYTES);

    // Even repeated exact old-successor observations are not ownership
    // evidence. Hardware reproduced the stale link fetch after two such samples.
    mmio.set_rx_next_descriptor_address(base + DESCRIPTOR_BYTES);
    assert!(!live.observe_current_completed_unit_link_release(&mut mmio, 1));
    assert!(live.completion_release_probe_pending());
    assert_eq!(descriptors[0].next_address(), base + DESCRIPTOR_BYTES);

    assert!(!live.observe_current_completed_unit_link_release(&mut mmio, 1));
    descriptors[1].write_word0(COMPLETED);
    mmio.set_rx_last_descriptor_address(base + DESCRIPTOR_BYTES);
    assert!(live.observe_current_completed_unit_link_release(&mut mmio, 1));
    assert!(!live.completion_release_probe_pending());
    storage
        .take_completed_unit(&mut live, 1)
        .unwrap()
        .unwrap()
        .retain_for_deferred_recycle();
    assert!(live.try_stop(&mut mmio).is_ok());
}

fn exercise_single_descriptor_rx_interleavings<const COUNT: usize>() {
    let arena = Arena::<COUNT>::new();
    let (storage, descriptors) = (arena.storage, arena.descriptors());
    let mut mmio = MockMmio::default();
    let mut live = arena.start(&mut mmio);

    // Two complete rotations cover both the cold physical topology and the
    // live topology assembled entirely through append/reload transactions.
    for epoch in 0..2 {
        for (cursor, descriptor) in descriptors.iter().enumerate() {
            assert_eq!(
                live.recycle_start().get(),
                cursor,
                "epoch {epoch}, cursor {cursor}"
            );
            let old_next = descriptor.next_address();
            assert_ne!(
                old_next, 0,
                "the live head must not also be the accepted terminal"
            );
            descriptor.write_word0(COMPLETED);
            mmio.set_rx_last_descriptor_address(arena.address(cursor));
            assert!(storage.take_completed(&mut live, cursor).unwrap().is_some());

            // LAST/RX_DONE without the old successor in NEXT does not release
            // the link word. A failed probe must be a read-only transaction.
            mmio.set_rx_next_descriptor_low(0);
            let before_word0 = descriptors[cursor].word0();
            assert!(
                storage
                    .recycle_completed_prefix::<1, _>(&mut live, &mut mmio)
                    .unwrap()
                    .is_none()
            );
            assert_eq!(descriptors[cursor].word0(), before_word0);
            assert_eq!(descriptors[cursor].next_address(), old_next);

            // Even a stable exact successor is not a link-ownership proof.
            mmio.set_rx_next_descriptor_address(old_next);
            assert!(
                storage
                    .recycle_completed_prefix::<1, _>(&mut live, &mut mmio)
                    .unwrap()
                    .is_none()
            );
            assert_eq!(descriptors[cursor].next_address(), old_next);
            let later = (cursor + 1) % COUNT;
            descriptors[later].write_word0(descriptors[later].word0() | BIT_30);
            mmio.set_rx_last_descriptor_address(arena.address(later));
            let append = storage
                .recycle_completed_prefix::<1, _>(&mut live, &mut mmio)
                .unwrap()
                .unwrap();
            assert_eq!(append.head_index.get(), cursor);
            assert_eq!(append.tail_index.get(), cursor);
            assert_eq!(descriptors[cursor].next_address(), 0);
            assert_eq!(descriptors[cursor].word0() & BIT_30, 0);
            assert!(live.topology_snapshot().valid);

            mmio.set_rx_walker_enabled(true);
            mmio.set_rx_reload_pending(false);
            mmio.set_rx_next_descriptor_address(old_next);
            assert_eq!(
                live.poll_pending_reload(&mut mmio).unwrap(),
                RxReloadObservation::Settled
            );
            assert_eq!(live.accepted_tail().get(), cursor);
            let topology = live.topology_snapshot();
            assert!(topology.valid, "epoch {epoch}, cursor {cursor}");
            assert_eq!(topology.visited_descriptors, COUNT);
            assert_eq!(topology.terminal_descriptors, 1);
            assert_eq!(topology.tail_index, cursor);
        }
    }
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn live_rx_ring_preserves_topology_across_every_two_and_four_slot_interleaving() {
    exercise_single_descriptor_rx_interleavings::<2>();
    exercise_single_descriptor_rx_interleavings::<4>();
}

#[test]
fn live_rx_frontier_rejects_last_beyond_the_accepted_tail_during_reload() {
    const COUNT: usize = 4;
    let arena = Arena::<COUNT>::new();
    let (storage, descriptors, base) = (arena.storage, arena.descriptors(), arena.base);
    let mut mmio = MockMmio::default();
    let mut live = arena.start(&mut mmio);

    descriptors[0].write_word0(COMPLETED);
    mmio.set_rx_last_descriptor_address(base);
    mmio.set_rx_next_descriptor_address(base + DESCRIPTOR_BYTES);
    assert!(storage.take_completed(&mut live, 0).unwrap().is_some());
    confirm_completed_unit_link_release(&mut live, &mut mmio, descriptors, base, base, 1);
    storage
        .recycle_completed_prefix::<1, _>(&mut live, &mut mmio)
        .unwrap()
        .unwrap();
    assert!(live.reload_pending());
    assert_eq!(live.recycle_start().get(), 1);
    assert_eq!(live.accepted_tail().get(), 3);

    // Hardware-visible pending tail zero is outside the still accepted list
    // 1 -> 2 -> 3. Even if descriptor one is complete, that impossible LAST
    // snapshot must not manufacture ownership before reload settles.
    descriptors[1].write_word0(COMPLETED);
    let pending_tail_low = rx_descriptor_low(base);
    let frontier = live.completed_unit_frontier_through_with(pending_tail_low, |_| true);
    assert_eq!(frontier, Default::default());

    mmio.set_rx_walker_enabled(true);
    mmio.set_rx_reload_pending(false);
    mmio.set_rx_next_descriptor_address(base + DESCRIPTOR_BYTES);
    assert_eq!(
        live.poll_pending_reload(&mut mmio).unwrap(),
        RxReloadObservation::Settled
    );
    let frontier = live.completed_unit_frontier_through_with(pending_tail_low, |_| true);
    assert_eq!(frontier.unit_count, 1);
    assert_eq!(frontier.descriptor_count, 1);
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn live_rx_recycle_rejects_a_corrupt_append_tail_before_any_mutation() {
    const COUNT: usize = 4;
    let arena = Arena::<COUNT>::new();
    let (storage, descriptors, base) = (arena.storage, arena.descriptors(), arena.base);
    let mut mmio = MockMmio::default();
    let mut live = arena.start(&mut mmio);

    arena.dma_write(&live, 0, &[0xa5; 80]);
    descriptors[0].write_word0(COMPLETED);
    mmio.set_rx_last_descriptor_address(base);
    mmio.set_rx_next_descriptor_address(base + DESCRIPTOR_BYTES);
    assert!(storage.take_completed(&mut live, 0).unwrap().is_some());

    // The accepted tail must be zero-terminated until the sole ring owner
    // publishes an append. Model foreign/corrupt mutation of that link.
    descriptors[3].inject_corrupt_words(
        descriptors[3].word0(),
        descriptors[3].buffer_address(),
        base + 2 * DESCRIPTOR_BYTES,
    );
    let before = core::array::from_fn::<_, COUNT, _>(|index| {
        (
            descriptors[index].word0(),
            descriptors[index].buffer_address(),
            descriptors[index].next_address(),
        )
    });
    assert_eq!(
        storage.recycle_completed_prefix::<1, _>(&mut live, &mut mmio),
        Err(RxRingError::Corrupt)
    );
    // No buffer was prepared: the DMA payload still covers buffer zero's
    // leading guard.
    assert_eq!(arena.dma_guards(&live, 0).0, [0xa5; 4]);
    for (index, expected) in before.into_iter().enumerate() {
        assert_eq!(
            (
                descriptors[index].word0(),
                descriptors[index].buffer_address(),
                descriptors[index].next_address(),
            ),
            expected
        );
    }

    // Restore the deliberately corrupted host model so teardown can prove a
    // conventional halted list.
    descriptors[3].inject_corrupt_words(descriptors[3].word0(), descriptors[3].buffer_address(), 0);
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn live_rx_ring_snapshots_only_the_current_contiguous_frontier() {
    const COUNT: usize = 4;
    let arena = Arena::<COUNT>::new();
    let (storage, descriptors, base) = (arena.storage, arena.descriptors(), arena.base);
    let mut mmio = MockMmio::default();
    let mut live = arena.start(&mut mmio);

    assert_eq!(live.completed_frontier_len(), 0);
    descriptors[0].write_word0(COMPLETED);
    descriptors[1].write_word0(COMPLETED);
    descriptors[3].write_word0(COMPLETED);
    assert_eq!(live.completed_frontier_len(), 2);

    assert!(storage.take_completed(&mut live, 0).unwrap().is_some());
    mmio.set_rx_last_descriptor_address(base);
    mmio.set_rx_next_descriptor_address(base + DESCRIPTOR_BYTES);
    assert_eq!(live.completed_frontier_len(), 0);
    confirm_completed_unit_link_release(&mut live, &mut mmio, descriptors, base, base, 1);
    let first = storage
        .recycle_completed_prefix::<1, _>(&mut live, &mut mmio)
        .unwrap()
        .unwrap();
    assert_eq!(first.descriptor_count, 1);

    mmio.set_rx_walker_enabled(true);
    mmio.set_rx_reload_pending(false);
    mmio.set_rx_next_descriptor_address(base + DESCRIPTOR_BYTES);
    assert_eq!(
        live.poll_pending_reload(&mut mmio).unwrap(),
        RxReloadObservation::Settled
    );
    assert_eq!(live.completed_frontier_len(), 1);
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn live_rx_ring_transfers_and_recycles_one_chained_unit_atomically() {
    const COUNT: usize = 4;
    let arena = Arena::<COUNT>::new();
    let (storage, descriptors, base) = (arena.storage, arena.descriptors(), arena.base);
    let mut mmio = MockMmio::default();
    let mut live = arena.start(&mut mmio);
    // DMA fills buffer zero completely, ends the frame in buffer one and
    // writes the next frame into buffers two and three.
    arena.dma_write(&live, 0, &[0xa5; 256]);
    for index in 1..COUNT {
        arena.dma_write(&live, index, &[0xa5; 80]);
    }
    descriptors[0].write_word0(BUFFER_SIZE | (BUFFER_SIZE << LENGTH_SHIFT) | BIT_31);
    descriptors[1].write_word0(COMPLETED);
    mmio.set_rx_last_descriptor_address(base + DESCRIPTOR_BYTES);
    mmio.set_rx_next_descriptor_address(base + 2 * DESCRIPTOR_BYTES);

    assert_eq!(live.completed_frontier_len(), 0);
    let frontier = live.completed_unit_frontier();
    assert_eq!(frontier.unit_count, 1);
    assert_eq!(frontier.descriptor_count, 2);
    // The unit token keeps the ring borrowed until its recycle, so prove the
    // link release before the transfer; the proof does not depend on it.
    confirm_completed_unit_link_release(
        &mut live,
        &mut mmio,
        descriptors,
        base,
        base + DESCRIPTOR_BYTES,
        2,
    );
    let unit = storage
        .take_completed_unit(&mut live, frontier.descriptor_count)
        .unwrap()
        .unwrap();
    assert_eq!(unit.head_index(), 0);
    assert_eq!(unit.descriptor_count(), 2);
    assert_eq!(unit.metadata().segment_length(0), Some(256));
    assert_eq!(unit.metadata().segment_length(1), Some(80));
    assert_eq!(unit.total_length(), 336);
    assert_ne!(unit.metadata().staged_word0() & BIT_30, 0);
    assert_eq!(length(unit.metadata().staged_word0()), 336);

    let append = unit.recycle(&mut mmio).unwrap().unwrap();
    let recycled = (0..COUNT)
        .filter(|&index| arena.guards_restored(&live, index))
        .collect::<Vec<_>>();
    assert_eq!(recycled, [0, 1]);
    assert_eq!(append.descriptor_count, 2);
    assert_eq!(live.recycle_start().get(), 2);
    assert_eq!(descriptors[0].word0() & BIT_30, 0);
    assert_eq!(descriptors[1].word0() & BIT_30, 0);
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn live_rx_ring_replenishes_the_available_variable_prefix() {
    const COUNT: usize = 4;
    let arena = Arena::<COUNT>::new();
    let (storage, descriptors, base) = (arena.storage, arena.descriptors(), arena.base);
    let mut mmio = MockMmio::default();
    let mut live = arena.start(&mut mmio);

    descriptors[0].write_word0(COMPLETED);
    descriptors[1].write_word0(COMPLETED);
    mmio.set_rx_last_descriptor_address(base + DESCRIPTOR_BYTES);
    mmio.set_rx_next_descriptor_address(base + 2 * DESCRIPTOR_BYTES);
    assert!(storage.take_completed(&mut live, 0).unwrap().is_some());
    assert!(storage.take_completed(&mut live, 1).unwrap().is_some());
    confirm_completed_unit_link_release(
        &mut live,
        &mut mmio,
        descriptors,
        base,
        base + DESCRIPTOR_BYTES,
        2,
    );
    let first = storage
        .recycle_completed_prefix::<4, _>(&mut live, &mut mmio)
        .unwrap()
        .unwrap();
    assert_eq!(first.head_index.get(), 0);
    assert_eq!(first.tail_index.get(), 1);
    assert_eq!(first.descriptor_count, 2);

    mmio.set_rx_walker_enabled(true);
    mmio.set_rx_reload_pending(false);
    mmio.set_rx_next_descriptor_address(base + 2 * DESCRIPTOR_BYTES);
    assert_eq!(
        live.poll_pending_reload(&mut mmio).unwrap(),
        RxReloadObservation::Settled
    );

    descriptors[2].write_word0(COMPLETED);
    mmio.set_rx_last_descriptor_address(base + 2 * DESCRIPTOR_BYTES);
    mmio.set_rx_next_descriptor_address(base + 3 * DESCRIPTOR_BYTES);
    assert!(storage.take_completed(&mut live, 2).unwrap().is_some());
    confirm_completed_unit_link_release(
        &mut live,
        &mut mmio,
        descriptors,
        base,
        base + 2 * DESCRIPTOR_BYTES,
        1,
    );
    let second = storage
        .recycle_completed_prefix::<4, _>(&mut live, &mut mmio)
        .unwrap()
        .unwrap();
    assert_eq!(second.head_index.get(), 2);
    assert_eq!(second.tail_index.get(), 2);
    assert_eq!(second.descriptor_count, 1);

    assert_eq!(
        storage.recycle_completed_prefix::<0, _>(&mut live, &mut mmio),
        Err(RxRingError::Count)
    );
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn receive_disable_confirms_the_ring_ownership_edge() {
    let arena = Arena::<2>::new();
    let mut mmio = MockMmio::default();
    let live = arena.start(&mut mmio);
    assert!(mmio.walker_enabled());
    mmio.operations.clear();
    assert!(live.try_stop(&mut mmio).is_ok());
    assert!(!mmio.walker_enabled());
    assert_eq!(
        mmio.operations(),
        &[
            Operation::StopRxWalker,
            Operation::Fence,
            Operation::ObserveRxWalkerEnabled,
            Operation::ObserveRxWalkerEnabled,
        ]
    );
}

#[test]
fn receive_enable_is_a_separate_confirmed_hardware_edge() {
    let arena = Arena::<2>::new();
    let mut mmio = MockMmio::default();
    let stopped = arena.prepare(&mut mmio).unwrap();
    assert!(
        !mmio
            .operations()
            .contains(&Operation::PublishRxWalkerEnable)
    );
    mmio.operations.clear();
    let live = stopped
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .unwrap();
    assert!(mmio.walker_enabled());
    assert_eq!(
        mmio.operations(),
        &[
            Operation::ObserveRxWalkerEnabled,
            Operation::PublishRxWalkerEnable,
            Operation::Fence,
            Operation::ObserveRxWalkerEnabled,
            Operation::ObserveRxWalkerEnabled,
        ]
    );
    assert!(live.try_stop(&mut mmio).is_ok());

    // A walker found enabled at the start edge refuses a second enable. The
    // failed start reads the enable once more and quarantines the arena.
    let arena = Arena::<2>::new();
    let mut already_enabled = MockMmio::default();
    let stopped = arena.prepare(&mut already_enabled).unwrap();
    already_enabled.set_rx_walker_enabled(true);
    already_enabled.operations.clear();
    let error = match stopped.try_start(&mut already_enabled) {
        Ok(_) => panic!("an enabled walker accepted a second enable"),
        Err((_, error)) => error,
    };
    assert_eq!(error, RxRingError::Busy);
    assert_eq!(
        already_enabled.operations(),
        &[
            Operation::ObserveRxWalkerEnabled,
            Operation::ObserveRxWalkerEnabled,
        ]
    );
    assert_eq!(
        arena.storage.lifecycle_state(),
        RxDmaArenaState::ResetRequired
    );
}
