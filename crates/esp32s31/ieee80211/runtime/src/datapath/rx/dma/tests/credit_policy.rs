use super::*;

/// Drive the compiled transaction, including deferred discard/reclaim, with a
/// bulk frame ahead of critical traffic after the ordinary credits saturate.
fn saturation<const SLOTS: usize, const DEPTH: usize>(critical_control: u16) {
    const COUNT: usize = 40;
    const CAPACITY: usize = 128;
    const STORAGE: usize = 132;
    const LENGTH: usize = PUBLIC_HEADER_SIZE + 24;
    let capacity = SLOTS.min(DEPTH);
    let completed = capacity + 2;
    let storage = Box::leak(Box::new(
        ReceiveDmaStorage::<COUNT, CAPACITY, STORAGE>::new(),
    ));
    for index in 0..completed {
        let control = if index < capacity {
            0x4008_u16
        } else {
            critical_control
        };
        let buffer = storage.buffer_mut(index).unwrap();
        buffer[PUBLIC_HEADER_SIZE..PUBLIC_HEADER_SIZE + 2].copy_from_slice(&control.to_le_bytes());
        // Unprotected data fixture is EAPOL, not a fabricated critical marker.
        if control == 0x0008 {
            buffer[PUBLIC_HEADER_SIZE + 24..PUBLIC_HEADER_SIZE + 32]
                .copy_from_slice(&[0xaa, 0xaa, 3, 0, 0, 0, 0x88, 0x8e]);
        }
    }
    let addresses = core::array::from_fn(|index| 0x2f00_2000 + index as u32 * 0x200);
    let mut hardware = MockRxDma::default();
    let ring = RxRingStopped::prepare(
        &mut hardware,
        storage.descriptors(),
        BASE,
        &addresses,
        CAPACITY as u32,
        |_| Ok(()),
    )
    .unwrap()
    .try_start(&mut hardware)
    .map_err(|(_, error)| error)
    .unwrap();
    for index in 0..completed {
        let length = LENGTH
            + if critical_control == 0x0008 && index >= capacity {
                8
            } else {
                0
            };
        storage.descriptors()[index]
            .write_word0(CAPACITY as u32 | ((length as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31);
    }
    hardware.release_through(completed - 1, Some(completed));
    let pool = RxStagePool::<SLOTS, CAPACITY>::new();
    let observer = RecordingRxObserver::default();
    let queue = StagedRxQueue::<NoopRawMutex, DEPTH, CAPACITY, SLOTS>::new();
    let (sender, receiver) = queue.split();
    let mut service = StagedRxProducer::new(ring, storage, &pool, NoDelay::new(), sender)
        .with_pipeline_observer(&observer);

    embassy_futures::block_on(service.service(&mut hardware)).unwrap();
    assert_eq!(pool.claimed_slots() as usize, capacity - 1);
    assert_eq!(observer.overload_discarded_units.load(Ordering::Relaxed), 0);
    embassy_futures::block_on(service.service(&mut hardware)).unwrap();
    assert_eq!(pool.claimed_slots() as usize, capacity);
    assert_eq!(observer.overload_discarded_units.load(Ordering::Relaxed), 1);
    assert_eq!(
        observer.critical_reserve_admissions.load(Ordering::Relaxed),
        1
    );
    assert!(observer.critical_admission_blocked.load(Ordering::Relaxed));
    for index in 0..capacity {
        let frame = receiver.try_receive().unwrap();
        let actual = u16::from_le_bytes(
            frame.segment().buffer[PUBLIC_HEADER_SIZE..PUBLIC_HEADER_SIZE + 2]
                .try_into()
                .unwrap(),
        );
        assert_eq!(
            actual,
            if index + 1 == capacity {
                critical_control
            } else {
                0x4008
            }
        );
        drop(frame);
    }
    assert!(receiver.try_receive().is_err());
    assert_eq!(pool.claimed_slots(), 0);
    // The blocked critical unit retains its original owner and is published
    // exactly once after credit return, along with reclaim of the discarded unit.
    embassy_futures::block_on(service.service(&mut hardware)).unwrap();
    let frame = receiver.try_receive().expect("retained critical frame");
    assert_eq!(
        &frame.segment().buffer[PUBLIC_HEADER_SIZE..PUBLIC_HEADER_SIZE + 2],
        &critical_control.to_le_bytes()
    );
    drop(frame);
    assert!(receiver.try_receive().is_err());
    for _ in 0..3 {
        embassy_futures::block_on(service.service(&mut hardware)).unwrap();
    }
    assert_eq!(pool.claimed_slots(), 0);
    assert_eq!(storage.detached_buffer_count(), 0);
    // Returning a lease is not proof that the walker released the final link.
    // With unchanged LAST/NEXT these two buffers must remain unrearmed.
    assert_eq!(storage.released_buffer_count(), 2);
    assert_eq!(service.work_counters().completed_units as usize, completed);
    // A later terminal completion supplies that separate hardware proof.
    storage.descriptors()[completed].write_word0(CAPACITY as u32 | BIT_30 | BIT_31);
    hardware.release_through(completed, Some(completed + 1));
    embassy_futures::block_on(service.service(&mut hardware)).unwrap();
    assert_eq!(storage.released_buffer_count(), 0);
    assert!(receiver.try_receive().is_err());
}

#[test]
fn management_control_and_eapol_retain_a_credit_below_and_at_vendor_capacity() {
    for control in [0x0000, 0x00d4, 0x0008] {
        saturation::<2, 2>(control);
        saturation::<3, 3>(control);
        saturation::<4, 4>(control);
        saturation::<8, 8>(control);
        saturation::<16, 16>(control);
        saturation::<31, 31>(control);
        saturation::<32, 32>(control);
        saturation::<32, 31>(control);
        saturation::<32, 2>(control);
    }
}
