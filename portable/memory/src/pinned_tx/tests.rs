extern crate std;

use core::cell::Cell;
use std::boxed::Box;

use super::*;

type TestPool = PinnedDmaTxPool<32, 8, 4, 1>;
type TestHandle = PinnedDmaTxPoolHandle<32, 8, 4, 1>;

/// A target's cache maintenance, observable: it marks the first byte.
enum MarkDmaReadPrepared {}

impl DmaReadPrepare for MarkDmaReadPrepared {
    fn prepare_for_dma_read(storage: &mut [u8]) {
        storage[0] = 0x5a;
    }
}

struct ReturnProbe<'pool> {
    pool: &'pool TestHandle,
    returned: &'pool Cell<Option<u8>>,
}

impl DmaIndexReturn for ReturnProbe<'_> {
    fn return_index(&self, index: u8) {
        assert_eq!(
            self.pool.pool.slots[usize::from(index)]
                .state
                .load(Ordering::Acquire),
            SLOT_FREE,
            "the backing must release its slot before queue publication"
        );
        self.returned.set(Some(index));
    }
}

/// A pool as production holds it: zeroed static storage, pinned.
fn pinned_pool() -> TestHandle {
    let pool = Box::leak(Box::new(crate::zeroed::zeroed::<TestPool>()));
    TestPool::pin_static(pool)
}

fn prepared_radio(pool: &TestHandle) -> PinnedDmaTxRadioLease<'_, 32, 8, 4> {
    let network = pool.claim_network(0).unwrap();
    let (index, ()) = network.publish(crate::FrameLen::constant::<4>(), |frame| {
        frame.copy_from_slice(&[1, 2, 3, 4])
    });
    pool.claim_radio(index).unwrap()
}

#[test]
fn dropped_stage_leases_restore_the_slot() {
    let pool = pinned_pool();
    drop(pool.claim_network(0).unwrap());

    let radio = prepared_radio(&pool);
    assert_eq!(radio.ethernet(), &[1, 2, 3, 4]);
    drop(radio);

    assert_eq!(pool.claim_network(0).unwrap().release(), 0);
}

#[test]
fn claimed_slots_tracks_publication_and_returns_across_slots() {
    type TwoSlotPool = PinnedDmaTxPool<32, 8, 4, 2>;
    let pool = TwoSlotPool::pin_static(Box::leak(Box::new(TwoSlotPool::new())));
    assert_eq!(pool.claimed_slots(), 0);

    let first = pool.claim_network(0).unwrap();
    assert_eq!(pool.claimed_slots(), 1);
    let second = pool.claim_network(1).unwrap();
    assert_eq!(pool.claimed_slots(), 2);

    let (index, ()) = first.publish(crate::FrameLen::constant::<4>(), |frame| {
        frame.copy_from_slice(&[1, 2, 3, 4])
    });
    assert_eq!(pool.claimed_slots(), 2);
    let radio = pool.claim_radio(index).unwrap();
    assert_eq!(radio.ethernet(), &[1, 2, 3, 4]);
    assert_eq!(pool.claimed_slots(), 2);

    drop(second);
    assert_eq!(pool.claimed_slots(), 1);
    assert_eq!(radio.release(), 0);
    assert_eq!(pool.claimed_slots(), 0);
    assert_eq!(pool.claim_network(0).unwrap().release(), 0);
}

#[test]
fn radio_backing_runs_target_prepare_only_at_dma_publication_edge() {
    type MarkedPool = PinnedDmaTxPool<32, 8, 4, 1, MarkDmaReadPrepared>;
    let pool = MarkedPool::pin_static(Box::leak(Box::new(MarkedPool::new())));
    let network = pool.claim_network(0).unwrap();
    let (index, ()) = network.publish(crate::FrameLen::constant::<4>(), |frame| {
        frame.copy_from_slice(&[1, 2, 3, 4])
    });
    let mut radio = pool.claim_radio(index).unwrap();

    assert_eq!(radio.storage_mut()[0], 0);
    StableDmaBacking::prepare_for_dma_read(&mut radio);
    assert_eq!(radio.storage_mut()[0], 0x5a);
}

#[test]
fn failed_transactional_writer_never_publishes_the_slot() {
    let pool = pinned_pool();
    let lease = pool.claim_network(0).unwrap();
    let (lease, error) = lease
        .try_publish(crate::FrameLen::constant::<4>(), |frame| {
            frame.copy_from_slice(&[1, 2, 3, 4]);
            Err::<(), _>(7)
        })
        .unwrap_err();
    assert_eq!(error, 7);
    assert_eq!(lease.release(), 0);
    assert_eq!(pool.claim_network(0).unwrap().release(), 0);
}

#[test]
fn returning_backing_releases_before_publishing_its_index() {
    let pool = pinned_pool();
    let returned = Cell::new(None);
    let backing = ReturningStableDmaBacking::new(
        prepared_radio(&pool),
        ReturnProbe {
            pool: &pool,
            returned: &returned,
        },
    );

    assert_eq!(returned.get(), None);
    drop(backing);
    assert_eq!(returned.get(), Some(0));
    assert_eq!(pool.claim_network(0).unwrap().release(), 0);
}

#[test]
fn forgotten_backing_remains_quarantined() {
    let pool = pinned_pool();
    let returned = Cell::new(None);
    let backing = ReturningStableDmaBacking::new(
        prepared_radio(&pool),
        ReturnProbe {
            pool: &pool,
            returned: &returned,
        },
    );

    core::mem::forget(backing);

    assert_eq!(returned.get(), None);
    assert_eq!(pool.claimed_slots(), 1);
    assert_eq!(pool.pool.slots[0].state.load(Ordering::Acquire), SLOT_RADIO);
}

#[test]
fn forgotten_tagged_returning_lease_outlives_its_cpu_only_borrows() {
    let pool = pinned_pool();
    let returned = Cell::new(None);
    let dma_address;
    {
        let mut tag = 17;
        let returner = ReturnProbe {
            pool: &pool,
            returned: &returned,
        };
        let mut backing = TaggedStableDmaBacking::new(
            &mut tag,
            ReturningStableDmaBacking::new(prepared_radio(&pool), returner),
        );
        let region = backing.stable_dma_region().into_mut_slice();
        region[0] = 0x7b;
        dma_address = region.as_ptr();
        core::mem::forget(backing);
        // Forget ends this borrow; keeping DMA alive must not touch the tag.
        tag = 29;
        assert_eq!(tag, 29);
    }
    assert_eq!(returned.get(), None);
    assert_eq!(pool.pool.slots[0].state.load(Ordering::Acquire), SLOT_RADIO);
    #[allow(
        unsafe_code,
        reason = "the test models a DMA read after abandoning its lease"
    )]
    // SAFETY: the static pool stays allocated and the leaked lease keeps its
    // slot reserved. No other CPU or DMA writer exists in this test.
    let observed = unsafe { dma_address.read() };
    assert_eq!(observed, 0x7b);
}

#[test]
fn radio_claim_outlives_the_affine_handle_without_reinitializing_storage() {
    let mut radio = {
        let handle = pinned_pool();
        handle
            .claim_network(0)
            .unwrap()
            .publish(crate::FrameLen::constant::<4>(), |frame| frame.fill(0x5a));
        handle.claim_radio(0).unwrap()
    };
    assert_eq!(radio.ethernet(), &[0x5a; 4]);
    radio.ethernet_mut()[1] = 0x33;
    assert_eq!(radio.release(), 0);
}

#[test]
fn forgotten_radio_and_dropped_handle_keep_the_dma_allocation_reserved() {
    let (slot, address) = {
        let handle = pinned_pool();
        handle
            .claim_network(0)
            .unwrap()
            .publish(crate::FrameLen::constant::<4>(), |frame| frame.fill(0x6b));
        let radio = handle.claim_radio(0).unwrap();
        let address = radio.ethernet().as_ptr();
        core::mem::forget(radio);
        (handle.pool.get_ref().slots.first().unwrap(), address)
    };
    assert_eq!(slot.state.load(Ordering::Acquire), SLOT_RADIO);
    #[allow(unsafe_code, reason = "model a DMA read after the handle has ended")]
    // SAFETY: permanent storage remains allocated, the forgotten lease keeps
    // the slot reserved, and this test has no concurrent writer.
    let observed = unsafe { address.read() };
    assert_eq!(observed, 0x6b);
}

#[test]
fn a_zeroed_pool_is_the_free_coherent_pool_new_builds() {
    let pool = pinned_pool();
    for slot in &pool.pool.slots {
        assert_eq!(slot.state.load(Ordering::Acquire), SLOT_FREE);
        assert_eq!(slot.length.load(Ordering::Acquire), 0);
    }
    // The default `Coherent` pool leaves the bytes as written.
    let mut radio = prepared_radio(&pool);
    StableDmaBacking::prepare_for_dma_read(&mut radio);
    assert_eq!(radio.ethernet(), &[1, 2, 3, 4]);
}

#[test]
fn pre_pin_locations_name_the_exact_regions_later_lent_to_radio() {
    let storage = Box::leak(Box::new(PinnedDmaTxPool::<64, 8, 4, 3>::new()));
    let locations = storage.dma_storage_locations();
    let pool = PinnedDmaTxPool::pin_static(storage);
    for (index, (pointer, capacity)) in locations.into_iter().enumerate() {
        let index = u8::try_from(index).unwrap();
        pool.claim_network(index)
            .unwrap()
            .publish(crate::FrameLen::constant::<0>(), |_| {});
        let mut lease = pool.claim_radio(index).unwrap();
        let bytes = lease.stable_dma_region().into_mut_slice();
        assert_eq!(bytes.as_ptr(), pointer);
        assert_eq!(bytes.len(), capacity);
        drop(lease);
    }
}

#[test]
fn a_rejected_transition_keeps_the_slot_and_returns_observations() {
    let pool = pinned_pool();
    assert!(matches!(
        pool.claim_radio(0),
        Err(PoolError::State {
            index: 0,
            expected: SLOT_READY,
            observed: SLOT_FREE
        })
    ));
    assert!(matches!(
        pool.claim_network(1),
        Err(PoolError::Index { index: 1, slots: 1 })
    ));
    let owner = pool.claim_network(0).unwrap();
    assert!(matches!(
        pool.claim_network(0),
        Err(PoolError::State {
            observed: SLOT_NETWORK,
            ..
        })
    ));
    assert_eq!(pool.claimed_slots(), 1);
    drop(owner);
    assert_eq!(pool.claimed_slots(), 0);
}
