extern crate std;

use core::cell::RefCell;
use std::{boxed::Box, vec::Vec};

use oer_esp32s31_ieee80211_dma::{
    descriptor::{LENGTH_SHIFT, OWNER, RX_DONE, SIZE_MASK, rx_armed_word},
    rx_dma::model::RxWalkerModel,
    rx_ring::RxDmaArenaState,
    space::HostDma,
};
use oer_esp32s31_ieee80211_mac::rx::pool::NetworkRxFrame;

use super::*;
use crate::rx::transaction::{CompletedUnit, IngressClass, IngressRoute, Preview};

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
    let storage = Box::leak(Box::new(RxDmaStorage::<
        COUNT,
        BUFFER,
        { BUFFER + 4 },
        HostDma,
    >::new()));
    let buffers = Box::leak(Box::new([0; COUNT]));
    let base = storage.dma_layout(buffers).unwrap();
    let mut hardware = RxWalkerModel::new();
    let mut ring = storage
        .prepare_ring(&mut hardware, base, buffers)
        .unwrap()
        .try_start(&mut hardware)
        .map_err(|(_, error)| error)
        .unwrap();

    // Two full segments and a 100-byte terminal: more bytes than one
    // descriptor's 14-bit length field holds.
    for (descriptor, &buffer) in storage.descriptors().iter().zip(buffers.iter()).take(2) {
        storage.dma_write(&ring, buffer, &[0; 4]).unwrap();
        descriptor.write_word0(BUFFER as u32 | ((BUFFER as u32) << LENGTH_SHIFT) | OWNER);
    }
    storage.descriptors()[2].write_word0(BUFFER as u32 | (100 << LENGTH_SHIFT) | RX_DONE | OWNER);
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
        &recorder,
    )
    .expect("an oversized chain is dropped, not a ring fault");

    let length = 2 * BUFFER + 100;
    assert!(length > SIZE_MASK as usize);
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
