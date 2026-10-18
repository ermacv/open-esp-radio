use super::*;
use std::{sync::Mutex, vec, vec::Vec};

#[derive(Default)]
struct Recorder(Mutex<Vec<(usize, usize, RxOwnershipEdge)>>);

impl RxOwnershipObserver for Recorder {
    fn observe(&self, arena: usize, buffer: usize, edge: RxOwnershipEdge) {
        self.0.lock().unwrap().push((arena, buffer, edge));
    }
}

#[test]
fn rotated_real_leases_observe_final_drop_and_ordered_publication_separately() {
    const COUNT: usize = 4;
    const BASE: u32 = 0x2f00_1000;
    let addresses = [0x2f00_2000, 0x2f00_2200, 0x2f00_2400, 0x2f00_2600];
    let recorder = Box::leak(Box::new(Recorder::default()));
    let storage = Box::leak(Box::new(RxDmaStorage::<COUNT, 16, 20>::new()));
    storage.set_ownership_observer(recorder);
    storage.bind_descriptor_rotation(1).unwrap();
    let arena = core::ptr::from_ref(storage).addr();
    let mut mmio = MockRxDma::default();
    let prepared = storage.prepare_ring(&mut mmio, BASE, &addresses).unwrap();
    let mut live = prepared.try_start(&mut mmio).map_err(|(_, e)| e).unwrap();
    recorder.0.lock().unwrap().clear();
    for descriptor in &storage.descriptors()[..2] {
        descriptor.write_word0(
            16 | (8 << crate::descriptor::LENGTH_SHIFT)
                | crate::descriptor::BIT_30
                | crate::descriptor::BIT_31,
        );
    }
    mmio.last_descriptor_low = (BASE + 3 * crate::descriptor::DESCRIPTOR_BYTES) & 0x000f_ffff;
    mmio.next_descriptor_low = 0;
    let first = storage
        .take_completed_unit(&mut live, 1)
        .unwrap()
        .unwrap()
        .detach_single()
        .unwrap()
        .into_buffer();
    let second = storage
        .take_completed_unit(&mut live, 1)
        .unwrap()
        .unwrap()
        .detach_single()
        .unwrap()
        .into_buffer();
    let pool = oer_memory::ExternalRxHandoffPool::<16, 2>::new();
    let first = pool.try_claim_radio(first, 0).map_err(drop).unwrap();
    let length = first.frame().len();
    let first = pool.claim_network(first.republish(0, length));
    let second = pool.try_claim_radio(second, 1).map_err(drop).unwrap();
    let length = second.frame().len();
    let second = pool.claim_network(second.republish(0, length));
    drop(second);
    assert_eq!(
        storage.recycle_released_prefix::<COUNT, _>(&mut live, &mut mmio),
        Ok(None)
    );
    use RxOwnershipEdge::*;
    assert_eq!(
        *recorder.0.lock().unwrap(),
        vec![
            (arena, 1, Detached),
            (arena, 2, Detached),
            (arena, 2, Released),
        ]
    );
    drop(first);
    assert!(
        storage
            .recycle_released_prefix::<COUNT, _>(&mut live, &mut mmio)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        *recorder.0.lock().unwrap(),
        vec![
            (arena, 1, Detached),
            (arena, 2, Detached),
            (arena, 2, Released),
            (arena, 1, Released),
            (arena, 1, Republished),
            (arena, 2, Republished),
        ]
    );
    assert_eq!(storage.detached_buffer_count(), 0);
    assert_eq!(storage.released_buffer_count(), 0);
}

#[test]
fn invalid_detach_emits_no_event_and_stopped_reclaim_is_not_publication() {
    let recorder = Box::leak(Box::new(Recorder::default()));
    let storage = Box::leak(Box::new(RxDmaStorage::<2, 16, 20>::new()));
    storage.set_ownership_observer(recorder);
    assert!(storage.detach_buffer(0, 0).is_err());
    assert!(storage.detach_buffer(17, 0).is_err());
    assert!(storage.detach_buffer(8, 2).is_err());
    assert!(recorder.0.lock().unwrap().is_empty());
    // Native model: no DMA actor exists. Exercise the same static allocation
    // callback while the root is still reusable, then prepare a stopped ring.
    let buffer = storage.detach_buffer(8, 0).unwrap();
    assert!(storage.detach_buffer(8, 0).is_err());
    drop(buffer);
    let mut mmio = MockRxDma::default();
    let addresses = [0x2f00_2000, 0x2f00_2200];
    let _stopped = storage
        .prepare_ring(&mut mmio, 0x2f00_1000, &addresses)
        .unwrap();
    let edges: Vec<_> = recorder
        .0
        .lock()
        .unwrap()
        .iter()
        .map(|(_, buffer, edge)| (*buffer, *edge))
        .collect();
    use RxOwnershipEdge::*;
    assert_eq!(
        edges,
        vec![
            (0, Detached),
            (0, Released),
            (0, ReclaimedWhileStopped),
            (1, ReclaimedWhileStopped)
        ]
    );
}
