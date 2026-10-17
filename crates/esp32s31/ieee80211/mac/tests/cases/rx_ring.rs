use crate::{support::*, *};

#[test]
fn cold_rx_ring_publishes_links_and_hardware_in_order() {
    let descriptors = [Descriptor::new(), Descriptor::new()];
    build_cold_ring(&descriptors, 0x2f00_1000, &[0x2f00_2000, 0x2f00_2800], 1700).unwrap();
    assert_eq!(
        descriptors[0].next_address(),
        0x2f00_1000 + DESCRIPTOR_BYTES
    );
    assert_eq!(descriptors[1].next_address(), 0);

    let mut mmio = MockMmio::default();
    publish_cold_ring(&mut mmio, 0x2f00_1000, true).unwrap();

    assert_eq!(
        mmio.operations(),
        &[
            Operation::Fence,
            Operation::ConfigureRxDescriptorWindow,
            Operation::PublishRxDescriptorBase(0x2f00_1000),
            Operation::PublishRxWalkerEnable,
            Operation::Fence,
        ]
    );
}

#[test]
fn completed_rx_descriptor_rearms_only_for_the_expected_storage() {
    let descriptor = Descriptor::new();
    let completed = 256 | (80 << LENGTH_SHIFT) | BIT_30 | BIT_31;
    descriptor.publish(completed, 0x2f00_3000, 0);
    rearm_descriptor(&descriptor, 0x2f00_3000, 0).unwrap();
    assert_eq!(length(descriptor.word0()), 256);
    assert_ne!(descriptor.word0() & BIT_31, 0);

    descriptor.publish(completed, 0x2f00_3000, 0);
    assert!(rearm_descriptor(&descriptor, 0x2f00_3400, 0).is_err());
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
    const BASE: u32 = 0x2f00_1000;
    const BUFFER_SIZE: u32 = 256;
    let descriptors = [const { Descriptor::new() }; COUNT];
    let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
    let mut prepared = Vec::new();
    let mut mmio = MockMmio::default();
    // A previous last pointer remains diagnostic only. A stopped/rebuilt rev0
    // list must begin at physical zero so it never depends on a cold 31->0
    // wrap link.
    mmio.set_rx_last_descriptor_address(BASE + DESCRIPTOR_BYTES);
    mmio.set_rx_walker_enabled(true);

    let stopped = RxRingStopped::prepare(
        &mut mmio,
        &descriptors,
        BASE,
        &buffers,
        BUFFER_SIZE,
        |index| {
            prepared.push(index);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(prepared, [0, 1, 2, 3]);
    assert_eq!(stopped.initial_start().get(), 0);
    assert_eq!(stopped.accepted_tail().get(), 3);
    assert_eq!(descriptors[2].next_address(), BASE + 3 * DESCRIPTOR_BYTES);
    assert_eq!(descriptors[3].next_address(), 0);
    assert_eq!(descriptors[0].next_address(), BASE + DESCRIPTOR_BYTES);
    assert_eq!(descriptors[1].next_address(), BASE + 2 * DESCRIPTOR_BYTES);
    assert_eq!(mmio.rx_descriptor_base(), BASE);
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
    let completed = BUFFER_SIZE | (80 << LENGTH_SHIFT) | BIT_30 | BIT_31;
    descriptors[0].write_word0(completed);
    descriptors[1].write_word0(completed);
    mmio.set_rx_last_descriptor_address(BASE + DESCRIPTOR_BYTES);
    mmio.set_rx_next_descriptor_address(BASE + 2 * DESCRIPTOR_BYTES);
    assert_eq!(live.take_completed(0).unwrap().index(), 0);
    assert_eq!(live.take_completed(0), None);
    assert_eq!(live.take_completed(1).unwrap().index(), 1);
    confirm_completed_unit_link_release(
        &mut live,
        &mut mmio,
        &descriptors,
        BASE,
        BASE + DESCRIPTOR_BYTES,
        2,
    );

    let mut recycled = Vec::new();
    let first = live
        .recycle_completed_half(&mut mmio, |index| {
            recycled.push(index);
            Ok(())
        })
        .unwrap()
        .unwrap();
    assert_eq!(recycled, [0, 1]);
    assert_eq!(first.head_index.get(), 0);
    assert_eq!(first.tail_index.get(), 1);
    assert_eq!(descriptors[3].next_address(), BASE);
    assert!(mmio.rx_reload_pending);
    assert!(live.reload_pending());
    assert_eq!(live.accepted_tail().get(), 3);

    descriptors[2].write_word0(completed);
    descriptors[3].write_word0(completed);
    assert!(live.take_completed(2).is_some());
    assert!(live.take_completed(3).is_some());

    // Model bit-0 self-clear at a terminal frontier. ROM repairs BASE from the
    // last accepted descriptor's now-published next link before accepting the
    // pending tail and appending the following group.
    mmio.set_rx_walker_enabled(true);
    mmio.set_rx_reload_pending(false);
    mmio.set_rx_next_descriptor_address(0);
    mmio.set_rx_last_descriptor_address(BASE + 3 * DESCRIPTOR_BYTES);
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
            Operation::PublishRxDescriptorBase(BASE),
        ],
        "reload repair must preserve vendor NEXT -> conditional LAST -> BASE order",
    );
    assert_eq!(live.accepted_tail().get(), 1);
    assert!(!live.reload_pending());
    assert!(live.exhausted_republication_probe_pending());
    recycled.clear();
    // LAST reached descriptor three while NEXT was zero, so the base-repair
    // write has been issued but hardware has not yet proved that it fetched
    // descriptor three's newly appended link to descriptor zero.
    assert!(
        live.recycle_completed_half(&mut mmio, |_| Ok(()))
            .unwrap()
            .is_none()
    );
    assert!(live.completion_release_probe_pending());
    mmio.set_rx_next_descriptor_address(BASE);
    assert!(
        live.recycle_completed_half(&mut mmio, |_| Ok(()))
            .unwrap()
            .is_none()
    );
    // Repeated NEXT observations still do not release descriptor three's
    // link. A later completed LAST does.
    descriptors[0].write_word0(descriptors[0].word0() | BIT_30);
    mmio.set_rx_last_descriptor_address(BASE);
    let second = live
        .recycle_completed_half(&mut mmio, |index| {
            recycled.push(index);
            Ok(())
        })
        .unwrap()
        .unwrap();
    assert_eq!(recycled, [2, 3]);
    assert_eq!(second.head_index.get(), 2);
    assert_eq!(second.tail_index.get(), 3);
    assert_eq!(descriptors[1].next_address(), BASE + 2 * DESCRIPTOR_BYTES);
    assert!(
        mmio.operations()
            .contains(&Operation::PublishRxDescriptorBase(BASE))
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
    const BASE: u32 = 0x2f00_1000;
    const BUFFER_SIZE: u32 = 256;

    for retained_index in 0..COUNT {
        let descriptors = [const { Descriptor::new() }; COUNT];
        let buffers = core::array::from_fn(|index| 0x2f01_0000 + index as u32 * 0x400);
        let mut mmio = MockMmio::default();
        mmio.set_rx_last_descriptor_address(BASE + retained_index as u32 * DESCRIPTOR_BYTES);

        let stopped =
            RxRingStopped::prepare(&mut mmio, &descriptors, BASE, &buffers, BUFFER_SIZE, |_| {
                Ok(())
            })
            .unwrap();
        let topology = stopped.topology_snapshot();
        assert_eq!(stopped.initial_start().get(), 0);
        assert_eq!(stopped.accepted_tail().get(), COUNT - 1);
        assert!(topology.valid, "retained descriptor {retained_index}");
        assert_eq!(topology.start_index, 0);
        assert_eq!(topology.tail_index, COUNT - 1);
        assert_eq!(topology.visited_descriptors, COUNT);
        assert_eq!(topology.terminal_descriptors, 1);
        assert_eq!(descriptors[COUNT - 1].next_address(), 0);
    }
}

#[test]
fn stopped_rx_ring_rebuilds_from_the_retained_hardware_next_cursor() {
    const COUNT: usize = 4;
    const BASE: u32 = 0x2f00_1000;
    const BUFFER_SIZE: u32 = 256;
    let descriptors = [const { Descriptor::new() }; COUNT];
    let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
    let mut mmio = MockMmio::default();
    mmio.set_rx_walker_enabled(true);
    mmio.set_rx_next_descriptor_address(BASE + DESCRIPTOR_BYTES);
    mmio.set_rx_last_descriptor_address(BASE);

    let stopped =
        RxRingStopped::prepare(&mut mmio, &descriptors, BASE, &buffers, BUFFER_SIZE, |_| {
            Ok(())
        })
        .unwrap();

    assert_eq!(
        stopped.retained_next_low(),
        rx_descriptor_low(BASE + DESCRIPTOR_BYTES)
    );
    assert_eq!(stopped.retained_last_low(), rx_descriptor_low(BASE));
    assert_eq!(stopped.initial_start().get(), 1);
    assert_eq!(stopped.accepted_tail().get(), 0);
    assert_eq!(descriptors[1].next_address(), BASE + 2 * DESCRIPTOR_BYTES);
    assert_eq!(descriptors[3].next_address(), BASE);
    assert_eq!(descriptors[0].next_address(), 0);
    assert_eq!(mmio.rx_descriptor_base(), BASE + DESCRIPTOR_BYTES);
    assert!(stopped.topology_snapshot().valid);
}

#[test]
fn stopped_rx_ring_rejects_a_nonzero_cursor_outside_its_owned_arena() {
    const COUNT: usize = 4;
    const BASE: u32 = 0x2f00_1000;
    const BUFFER_SIZE: u32 = 256;
    let descriptors = [const { Descriptor::new() }; COUNT];
    let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
    let mut mmio = MockMmio::default();
    mmio.set_rx_walker_enabled(true);
    mmio.set_rx_next_descriptor_address(BASE + COUNT as u32 * DESCRIPTOR_BYTES);

    assert!(matches!(
        RxRingStopped::prepare(
            &mut mmio,
            &descriptors,
            BASE,
            &buffers,
            BUFFER_SIZE,
            |_| Ok(())
        ),
        Err(RxRingError::Corrupt)
    ));
}

#[test]
fn stopped_rx_ring_avoids_a_cold_head_on_the_final_descriptor() {
    const COUNT: usize = 32;
    const BASE: u32 = 0x2f00_1000;
    const BUFFER_SIZE: u32 = 256;

    let descriptors = [const { Descriptor::new() }; COUNT];
    let buffers = core::array::from_fn(|index| 0x2f01_0000 + index as u32 * 0x400);
    let mut mmio = MockMmio::default();
    mmio.set_rx_last_descriptor_address(BASE + (COUNT as u32 - 2) * DESCRIPTOR_BYTES);

    let stopped =
        RxRingStopped::prepare(&mut mmio, &descriptors, BASE, &buffers, BUFFER_SIZE, |_| {
            Ok(())
        })
        .unwrap();

    assert_eq!(stopped.initial_start().get(), 0);
    assert_eq!(stopped.accepted_tail().get(), COUNT - 1);
    assert_eq!(descriptors[0].next_address(), BASE + DESCRIPTOR_BYTES);
    assert_eq!(descriptors[COUNT - 1].next_address(), 0);
    assert_eq!(mmio.rx_descriptor_base(), BASE);
    assert!(stopped.topology_snapshot().valid);
}

#[test]
fn stopped_rx_ring_uses_zero_for_an_invalid_retained_last_pointer() {
    const COUNT: usize = 4;
    const BASE: u32 = 0x2f00_1000;
    const BUFFER_SIZE: u32 = 256;
    let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];

    for retained_last in [0, BASE + 1, BASE + COUNT as u32 * DESCRIPTOR_BYTES] {
        let descriptors = [const { Descriptor::new() }; COUNT];
        let mut mmio = MockMmio::default();
        mmio.set_rx_last_descriptor_address(retained_last);
        let stopped =
            RxRingStopped::prepare(&mut mmio, &descriptors, BASE, &buffers, BUFFER_SIZE, |_| {
                Ok(())
            })
            .unwrap();
        assert_eq!(stopped.initial_start().get(), 0);
        assert!(stopped.topology_snapshot().valid);
    }
}

#[test]
fn stopped_rx_ring_rejects_corrupt_topology_before_walker_enable() {
    const COUNT: usize = 4;
    const BASE: u32 = 0x2f00_1000;
    const BUFFER_SIZE: u32 = 256;
    let descriptors = [const { Descriptor::new() }; COUNT];
    let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
    let mut mmio = MockMmio::default();
    mmio.set_rx_walker_enabled(false);
    let stopped =
        RxRingStopped::prepare(&mut mmio, &descriptors, BASE, &buffers, BUFFER_SIZE, |_| {
            Ok(())
        })
        .unwrap();
    assert!(stopped.topology_snapshot().valid);

    descriptors[0].publish(descriptors[0].word0(), buffers[0], 0);
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
    const BASE: u32 = 0x2f00_1000;
    const BUFFER_SIZE: u32 = 256;
    let descriptors = [const { Descriptor::new() }; COUNT];
    let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
    let mut mmio = MockMmio::default();

    let stopped =
        RxRingStopped::prepare(&mut mmio, &descriptors, BASE, &buffers, BUFFER_SIZE, |_| {
            Ok(())
        })
        .unwrap();
    let mut live = stopped
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .unwrap();
    let completed = BUFFER_SIZE | (80 << LENGTH_SHIFT) | BIT_30 | BIT_31;

    descriptors[0].write_word0(completed);
    mmio.set_rx_last_descriptor_address(BASE);
    mmio.set_rx_next_descriptor_address(BASE + DESCRIPTOR_BYTES);
    assert!(live.take_completed(0).is_some());
    confirm_completed_unit_link_release(&mut live, &mut mmio, &descriptors, BASE, BASE, 1);
    let first = live
        .recycle_completed_batch::<1, _, _>(&mut mmio, |_| Ok(()))
        .unwrap()
        .unwrap();
    assert_eq!(first.head_index.get(), 0);
    assert_eq!(first.tail_index.get(), 0);
    assert_eq!(descriptors[3].next_address(), BASE);

    // Model the doorbell self-clear while the walker still has a live next
    // pointer. No BASE repair is required for this ordinary append.
    mmio.set_rx_walker_enabled(true);
    mmio.set_rx_reload_pending(false);
    mmio.set_rx_next_descriptor_address(BASE + DESCRIPTOR_BYTES);
    assert_eq!(
        live.poll_pending_reload(&mut mmio).unwrap(),
        RxReloadObservation::Settled
    );
    assert_eq!(live.accepted_tail().get(), 0);

    descriptors[1].write_word0(completed);
    mmio.set_rx_last_descriptor_address(BASE + DESCRIPTOR_BYTES);
    mmio.set_rx_next_descriptor_address(BASE + 2 * DESCRIPTOR_BYTES);
    assert!(live.take_completed(1).is_some());
    confirm_completed_unit_link_release(
        &mut live,
        &mut mmio,
        &descriptors,
        BASE,
        BASE + DESCRIPTOR_BYTES,
        1,
    );
    let second = live
        .recycle_completed_batch::<1, _, _>(&mut mmio, |_| Ok(()))
        .unwrap()
        .unwrap();
    assert_eq!(second.head_index.get(), 1);
    assert_eq!(second.tail_index.get(), 1);
    assert_eq!(descriptors[0].next_address(), BASE + DESCRIPTOR_BYTES);

    assert_eq!(
        live.recycle_completed_batch::<0, _, _>(&mut mmio, |_| Ok(())),
        Err(RxRingError::Count)
    );
    assert_eq!(
        live.recycle_completed_batch::<3, _, _>(&mut mmio, |_| Ok(())),
        Err(RxRingError::Count)
    );
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn live_rx_ring_republishes_an_exhausted_software_list_without_a_self_link() {
    const COUNT: usize = 2;
    const BASE: u32 = 0x2f00_1000;
    const BUFFER_SIZE: u32 = 256;
    let descriptors = [const { Descriptor::new() }; COUNT];
    let buffers = [0x2f00_2000, 0x2f00_2200];
    let mut mmio = MockMmio::default();

    let stopped =
        RxRingStopped::prepare(&mut mmio, &descriptors, BASE, &buffers, BUFFER_SIZE, |_| {
            Ok(())
        })
        .unwrap();
    let mut live = stopped
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .unwrap();
    let completed = BUFFER_SIZE | (80 << LENGTH_SHIFT) | BIT_30 | BIT_31;

    // First append descriptor zero normally, making it the accepted tail of
    // the software list 1 -> 0.
    descriptors[0].write_word0(completed);
    mmio.set_rx_last_descriptor_address(BASE);
    mmio.set_rx_next_descriptor_address(BASE + DESCRIPTOR_BYTES);
    assert!(live.take_completed(0).is_some());
    confirm_completed_unit_link_release(&mut live, &mut mmio, &descriptors, BASE, BASE, 1);
    live.recycle_completed_batch::<1, _, _>(&mut mmio, |_| Ok(()))
        .unwrap()
        .unwrap();
    mmio.set_rx_walker_enabled(true);
    mmio.set_rx_reload_pending(false);
    mmio.set_rx_next_descriptor_address(BASE + DESCRIPTOR_BYTES);
    assert_eq!(
        live.poll_pending_reload(&mut mmio).unwrap(),
        RxReloadObservation::Settled
    );
    assert_eq!(live.accepted_tail().get(), 0);

    // Hardware then exhausts that whole list before software returns either
    // node. Discarding 1 -> 0 leaves the vendor software head null, so the
    // returned chain must become the new BASE directly. Linking old tail zero
    // to head one would create the invalid cycle 1 -> 0 -> 1.
    descriptors[1].write_word0(completed);
    descriptors[0].write_word0(completed);
    mmio.set_rx_next_descriptor_address(0);
    mmio.set_rx_last_descriptor_address(BASE);
    assert!(live.take_completed(1).is_some());
    assert!(live.take_completed(0).is_some());
    mmio.operations.clear();
    let append = live
        .recycle_completed_prefix::<COUNT, _, _>(&mut mmio, |_| Ok(()))
        .unwrap()
        .unwrap();

    assert_eq!(append.head_index.get(), 1);
    assert_eq!(append.tail_index.get(), 0);
    assert_eq!(descriptors[1].next_address(), BASE);
    assert_eq!(descriptors[0].next_address(), 0);
    assert_eq!(mmio.rx_descriptor_base(), BASE + DESCRIPTOR_BYTES);
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
    mmio.set_rx_next_descriptor_address(BASE);
    live.observe_exhausted_republication(&mut mmio);
    assert!(
        live.exhausted_republication_probe_pending(),
        "a nonzero cursor outside the republished head is stale evidence"
    );
    mmio.set_rx_next_descriptor_address(BASE + DESCRIPTOR_BYTES);
    live.observe_exhausted_republication(&mut mmio);
    assert!(live.exhausted_republication_probe_pending());
    live.observe_exhausted_republication(&mut mmio);
    assert!(!live.exhausted_republication_probe_pending());

    // Hardware resumes at the newly published head. The next RX edge must
    // inspect that same descriptor rather than the physical slot after the
    // returned chain's tail.
    descriptors[1].write_word0(completed);
    mmio.set_rx_last_descriptor_address(BASE + DESCRIPTOR_BYTES);
    let frontier = live.completed_unit_frontier_through_with(mmio.last_descriptor_low(), |_| true);
    assert_eq!(frontier.unit_count, 1);
    assert_eq!(frontier.descriptor_count, 1);
    assert!(live.take_completed_unit(1).unwrap().is_some());
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn live_rx_ring_does_not_rewrite_a_nonterminal_link_before_next_accepts_it() {
    const COUNT: usize = 4;
    const BASE: u32 = 0x2f00_1000;
    const BUFFER_SIZE: u32 = 256;
    let descriptors = [const { Descriptor::new() }; COUNT];
    let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
    let mut mmio = MockMmio::default();

    let stopped =
        RxRingStopped::prepare(&mut mmio, &descriptors, BASE, &buffers, BUFFER_SIZE, |_| {
            Ok(())
        })
        .unwrap();
    let mut live = stopped
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .unwrap();
    let completed = BUFFER_SIZE | (80 << LENGTH_SHIFT) | BIT_30 | BIT_31;
    descriptors[0].write_word0(completed);

    // LAST/RX_DONE can precede the walker's fetch of descriptor zero's link.
    // Rewriting that nonzero link to the recycle-chain terminal here would
    // strand descriptors one through three.
    let head_low = rx_descriptor_low(BASE);
    let successor_low = rx_descriptor_low(BASE + DESCRIPTOR_BYTES);
    mmio.set_rx_next_descriptor_low(0);
    assert!(!live.observe_completed_unit_link_release(&mut mmio, head_low, 1));
    assert!(live.completion_release_probe_pending());
    assert_eq!(descriptors[0].next_address(), BASE + DESCRIPTOR_BYTES);

    // Even repeated exact old-successor observations are not ownership
    // evidence. Hardware reproduced the stale link fetch after two such samples.
    mmio.set_rx_next_descriptor_low(successor_low);
    assert!(!live.observe_completed_unit_link_release(&mut mmio, head_low, 1));
    assert!(live.completion_release_probe_pending());
    assert_eq!(descriptors[0].next_address(), BASE + DESCRIPTOR_BYTES);

    assert!(!live.observe_completed_unit_link_release(&mut mmio, head_low, 1));
    descriptors[1].write_word0(completed);
    let later_low = rx_descriptor_low(BASE + DESCRIPTOR_BYTES);
    assert!(live.observe_completed_unit_link_release(&mut mmio, later_low, 1));
    assert!(!live.completion_release_probe_pending());
    assert!(live.take_completed_unit(1).unwrap().is_some());
    assert!(live.try_stop(&mut mmio).is_ok());
}

fn exercise_single_descriptor_rx_interleavings<const COUNT: usize>() {
    const BASE: u32 = 0x2f00_1000;
    const BUFFER_SIZE: u32 = 256;
    let descriptors = [const { Descriptor::new() }; COUNT];
    let buffers = core::array::from_fn(|index| 0x2f01_0000 + index as u32 * 0x400);
    let mut mmio = MockMmio::default();
    let stopped =
        RxRingStopped::prepare(&mut mmio, &descriptors, BASE, &buffers, BUFFER_SIZE, |_| {
            Ok(())
        })
        .unwrap();
    let mut live = stopped
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .unwrap();
    let completed = BUFFER_SIZE | (80 << LENGTH_SHIFT) | BIT_30 | BIT_31;

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
            descriptor.write_word0(completed);
            mmio.set_rx_last_descriptor_address(BASE + cursor as u32 * DESCRIPTOR_BYTES);
            assert!(live.take_completed(cursor).is_some());

            // LAST/RX_DONE without the old successor in NEXT does not release
            // the link word. A failed probe must be a read-only transaction.
            mmio.set_rx_next_descriptor_low(0);
            let before_word0 = descriptors[cursor].word0();
            assert!(
                live.recycle_completed_batch::<1, _, _>(&mut mmio, |_| Ok(()))
                    .unwrap()
                    .is_none()
            );
            assert_eq!(descriptors[cursor].word0(), before_word0);
            assert_eq!(descriptors[cursor].next_address(), old_next);

            // Even a stable exact successor is not a link-ownership proof.
            mmio.set_rx_next_descriptor_address(old_next);
            assert!(
                live.recycle_completed_batch::<1, _, _>(&mut mmio, |_| Ok(()))
                    .unwrap()
                    .is_none()
            );
            assert_eq!(descriptors[cursor].next_address(), old_next);
            let later = (cursor + 1) % COUNT;
            descriptors[later].write_word0(descriptors[later].word0() | BIT_30);
            mmio.set_rx_last_descriptor_address(BASE + later as u32 * DESCRIPTOR_BYTES);
            let append = live
                .recycle_completed_batch::<1, _, _>(&mut mmio, |_| Ok(()))
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
    const BASE: u32 = 0x2f00_1000;
    const BUFFER_SIZE: u32 = 256;
    let descriptors = [const { Descriptor::new() }; COUNT];
    let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
    let mut mmio = MockMmio::default();
    let stopped =
        RxRingStopped::prepare(&mut mmio, &descriptors, BASE, &buffers, BUFFER_SIZE, |_| {
            Ok(())
        })
        .unwrap();
    let mut live = stopped
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .unwrap();
    let completed = BUFFER_SIZE | (80 << LENGTH_SHIFT) | BIT_30 | BIT_31;

    descriptors[0].write_word0(completed);
    mmio.set_rx_last_descriptor_address(BASE);
    mmio.set_rx_next_descriptor_address(BASE + DESCRIPTOR_BYTES);
    assert!(live.take_completed(0).is_some());
    confirm_completed_unit_link_release(&mut live, &mut mmio, &descriptors, BASE, BASE, 1);
    live.recycle_completed_batch::<1, _, _>(&mut mmio, |_| Ok(()))
        .unwrap()
        .unwrap();
    assert!(live.reload_pending());
    assert_eq!(live.recycle_start().get(), 1);
    assert_eq!(live.accepted_tail().get(), 3);

    // Hardware-visible pending tail zero is outside the still accepted list
    // 1 -> 2 -> 3. Even if descriptor one is complete, that impossible LAST
    // snapshot must not manufacture ownership before reload settles.
    descriptors[1].write_word0(completed);
    let pending_tail_low = rx_descriptor_low(BASE);
    let frontier = live.completed_unit_frontier_through_with(pending_tail_low, |_| true);
    assert_eq!(frontier, Default::default());

    mmio.set_rx_walker_enabled(true);
    mmio.set_rx_reload_pending(false);
    mmio.set_rx_next_descriptor_address(BASE + DESCRIPTOR_BYTES);
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
    const BASE: u32 = 0x2f00_1000;
    const BUFFER_SIZE: u32 = 256;
    let descriptors = [const { Descriptor::new() }; COUNT];
    let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
    let mut mmio = MockMmio::default();
    let stopped =
        RxRingStopped::prepare(&mut mmio, &descriptors, BASE, &buffers, BUFFER_SIZE, |_| {
            Ok(())
        })
        .unwrap();
    let mut live = stopped
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .unwrap();
    let completed = BUFFER_SIZE | (80 << LENGTH_SHIFT) | BIT_30 | BIT_31;

    descriptors[0].write_word0(completed);
    mmio.set_rx_last_descriptor_address(BASE);
    mmio.set_rx_next_descriptor_address(BASE + DESCRIPTOR_BYTES);
    assert!(live.take_completed(0).is_some());

    // The accepted tail must be zero-terminated until the sole ring owner
    // publishes an append. Model foreign/corrupt mutation of that link.
    descriptors[3].publish(
        descriptors[3].word0(),
        descriptors[3].buffer_address(),
        BASE + 2 * DESCRIPTOR_BYTES,
    );
    let before = core::array::from_fn::<_, COUNT, _>(|index| {
        (
            descriptors[index].word0(),
            descriptors[index].buffer_address(),
            descriptors[index].next_address(),
        )
    });
    let mut prepare_calls = 0;
    assert_eq!(
        live.recycle_completed_batch::<1, _, _>(&mut mmio, |_| {
            prepare_calls += 1;
            Ok(())
        }),
        Err(RxRingError::Corrupt)
    );
    assert_eq!(prepare_calls, 0);
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
    descriptors[3].publish(descriptors[3].word0(), descriptors[3].buffer_address(), 0);
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn live_rx_ring_snapshots_only_the_current_contiguous_frontier() {
    const COUNT: usize = 4;
    const BASE: u32 = 0x2f00_1000;
    const BUFFER_SIZE: u32 = 256;
    let descriptors = [const { Descriptor::new() }; COUNT];
    let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
    let mut mmio = MockMmio::default();

    let stopped =
        RxRingStopped::prepare(&mut mmio, &descriptors, BASE, &buffers, BUFFER_SIZE, |_| {
            Ok(())
        })
        .unwrap();
    let mut live = stopped
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .unwrap();
    let completed = BUFFER_SIZE | (80 << LENGTH_SHIFT) | BIT_30 | BIT_31;

    assert_eq!(live.completed_frontier_len(), 0);
    descriptors[0].write_word0(completed);
    descriptors[1].write_word0(completed);
    descriptors[3].write_word0(completed);
    assert_eq!(live.completed_frontier_len(), 2);

    assert!(live.take_completed(0).is_some());
    mmio.set_rx_last_descriptor_address(BASE);
    mmio.set_rx_next_descriptor_address(BASE + DESCRIPTOR_BYTES);
    assert_eq!(live.completed_frontier_len(), 0);
    confirm_completed_unit_link_release(&mut live, &mut mmio, &descriptors, BASE, BASE, 1);
    let first = live
        .recycle_completed_batch::<1, _, _>(&mut mmio, |_| Ok(()))
        .unwrap()
        .unwrap();
    assert_eq!(first.descriptor_count, 1);

    mmio.set_rx_walker_enabled(true);
    mmio.set_rx_reload_pending(false);
    mmio.set_rx_next_descriptor_address(BASE + DESCRIPTOR_BYTES);
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
    const BASE: u32 = 0x2f00_1000;
    const BUFFER_SIZE: u32 = 256;
    let descriptors = [const { Descriptor::new() }; COUNT];
    let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
    let mut mmio = MockMmio::default();

    let stopped =
        RxRingStopped::prepare(&mut mmio, &descriptors, BASE, &buffers, BUFFER_SIZE, |_| {
            Ok(())
        })
        .unwrap();
    let mut live = stopped
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .unwrap();
    descriptors[0].write_word0(BUFFER_SIZE | (BUFFER_SIZE << LENGTH_SHIFT) | BIT_31);
    descriptors[1].write_word0(BUFFER_SIZE | (80 << LENGTH_SHIFT) | BIT_30 | BIT_31);
    mmio.set_rx_last_descriptor_address(BASE + DESCRIPTOR_BYTES);
    mmio.set_rx_next_descriptor_address(BASE + 2 * DESCRIPTOR_BYTES);

    assert_eq!(live.completed_frontier_len(), 0);
    let frontier = live.completed_unit_frontier();
    assert_eq!(frontier.unit_count, 1);
    assert_eq!(frontier.descriptor_count, 2);
    let unit = live
        .take_completed_unit(frontier.descriptor_count)
        .unwrap()
        .unwrap();
    assert_eq!(unit.head_index(), 0);
    assert_eq!(unit.descriptor_count(), 2);
    assert_eq!(unit.segment_length(0), Some(256));
    assert_eq!(unit.segment_length(1), Some(80));
    assert_eq!(unit.total_length(), 336);
    assert_ne!(unit.staged_word0() & BIT_30, 0);
    assert_eq!(length(unit.staged_word0()), 336);
    confirm_completed_unit_link_release(
        &mut live,
        &mut mmio,
        &descriptors,
        BASE,
        BASE + DESCRIPTOR_BYTES,
        2,
    );

    let mut recycled = Vec::new();
    let append = live
        .recycle_completed_unit(&mut mmio, unit.descriptor_count(), |index| {
            recycled.push(index);
            Ok(())
        })
        .unwrap()
        .unwrap();
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
    const BASE: u32 = 0x2f00_1000;
    const BUFFER_SIZE: u32 = 256;
    let descriptors = [const { Descriptor::new() }; COUNT];
    let buffers = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
    let mut mmio = MockMmio::default();

    let stopped =
        RxRingStopped::prepare(&mut mmio, &descriptors, BASE, &buffers, BUFFER_SIZE, |_| {
            Ok(())
        })
        .unwrap();
    let mut live = stopped
        .try_start(&mut mmio)
        .map_err(|(_, error)| error)
        .unwrap();
    let completed = BUFFER_SIZE | (80 << LENGTH_SHIFT) | BIT_30 | BIT_31;

    descriptors[0].write_word0(completed);
    descriptors[1].write_word0(completed);
    mmio.set_rx_last_descriptor_address(BASE + DESCRIPTOR_BYTES);
    mmio.set_rx_next_descriptor_address(BASE + 2 * DESCRIPTOR_BYTES);
    assert!(live.take_completed(0).is_some());
    assert!(live.take_completed(1).is_some());
    confirm_completed_unit_link_release(
        &mut live,
        &mut mmio,
        &descriptors,
        BASE,
        BASE + DESCRIPTOR_BYTES,
        2,
    );
    let first = live
        .recycle_completed_prefix::<4, _, _>(&mut mmio, |_| Ok(()))
        .unwrap()
        .unwrap();
    assert_eq!(first.head_index.get(), 0);
    assert_eq!(first.tail_index.get(), 1);
    assert_eq!(first.descriptor_count, 2);

    mmio.set_rx_walker_enabled(true);
    mmio.set_rx_reload_pending(false);
    mmio.set_rx_next_descriptor_address(BASE + 2 * DESCRIPTOR_BYTES);
    assert_eq!(
        live.poll_pending_reload(&mut mmio).unwrap(),
        RxReloadObservation::Settled
    );

    descriptors[2].write_word0(completed);
    mmio.set_rx_last_descriptor_address(BASE + 2 * DESCRIPTOR_BYTES);
    mmio.set_rx_next_descriptor_address(BASE + 3 * DESCRIPTOR_BYTES);
    assert!(live.take_completed(2).is_some());
    confirm_completed_unit_link_release(
        &mut live,
        &mut mmio,
        &descriptors,
        BASE,
        BASE + 2 * DESCRIPTOR_BYTES,
        1,
    );
    let second = live
        .recycle_completed_prefix::<4, _, _>(&mut mmio, |_| Ok(()))
        .unwrap()
        .unwrap();
    assert_eq!(second.head_index.get(), 2);
    assert_eq!(second.tail_index.get(), 2);
    assert_eq!(second.descriptor_count, 1);

    assert_eq!(
        live.recycle_completed_prefix::<0, _, _>(&mut mmio, |_| Ok(())),
        Err(RxRingError::Count)
    );
    assert!(live.try_stop(&mut mmio).is_ok());
}

#[test]
fn receive_disable_confirms_the_ring_ownership_edge() {
    let mut mmio = MockMmio::default();
    mmio.set_rx_walker_enabled(true);
    disable_receive(&mut mmio).unwrap();
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
    let mut mmio = MockMmio::default();
    enable_receive(&mut mmio).unwrap();
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

    let mut already_enabled = MockMmio::default();
    already_enabled.set_rx_walker_enabled(true);
    assert_eq!(enable_receive(&mut already_enabled), Err(RxRingError::Busy));
    assert_eq!(
        already_enabled.operations(),
        &[Operation::ObserveRxWalkerEnabled]
    );
}
