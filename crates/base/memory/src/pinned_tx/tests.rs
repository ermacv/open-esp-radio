extern crate std;

use core::cell::Cell;
use std::boxed::Box;

use super::*;

type TestPool = PinnedDmaTxPool<32, 8, 4, 1>;
type TestHandle = PinnedDmaTxPoolHandle<32, 8, 4, 1>;

fn mark_dma_read_prepared(storage: &mut [u8]) {
    storage[0] = 0x5a;
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
    let network = pool.claim_network(0);
    let (index, ()) = network.publish(4, |frame| frame.copy_from_slice(&[1, 2, 3, 4]));
    pool.claim_radio(index)
}

#[test]
fn dropped_stage_leases_restore_the_slot() {
    let pool = pinned_pool();
    drop(pool.claim_network(0));

    let radio = prepared_radio(&pool);
    assert_eq!(radio.ethernet(), &[1, 2, 3, 4]);
    drop(radio);

    assert_eq!(pool.claim_network(0).release(), 0);
}

#[test]
fn radio_backing_runs_target_prepare_only_at_dma_publication_edge() {
    let pool = Box::leak(Box::new(TestPool::new()));
    let pool = TestPool::pin_static_with_dma_read_prepare(pool, mark_dma_read_prepared);
    let mut radio = prepared_radio(&pool);

    assert_eq!(radio.storage_mut()[0], 0);
    StableDmaBacking::prepare_for_dma_read(&mut radio);
    assert_eq!(radio.storage_mut()[0], 0x5a);
}

#[test]
fn failed_transactional_writer_never_publishes_the_slot() {
    let pool = pinned_pool();
    let lease = pool.claim_network(0);
    let (lease, error) = lease
        .try_publish(4, |frame| {
            frame.copy_from_slice(&[1, 2, 3, 4]);
            Err::<(), _>(7)
        })
        .unwrap_err();
    assert_eq!(error, 7);
    assert_eq!(lease.release(), 0);
    assert_eq!(pool.claim_network(0).release(), 0);
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
    assert_eq!(pool.claim_network(0).release(), 0);
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
        handle.claim_network(0).publish(4, |frame| frame.fill(0x5a));
        handle.claim_radio(0)
    };
    assert_eq!(radio.ethernet(), &[0x5a; 4]);
    radio.ethernet_mut()[1] = 0x33;
    assert_eq!(radio.release(), 0);
}

#[test]
fn forgotten_radio_and_dropped_handle_keep_the_dma_allocation_reserved() {
    let (slot, address) = {
        let handle = pinned_pool();
        handle.claim_network(0).publish(4, |frame| frame.fill(0x6b));
        let radio = handle.claim_radio(0);
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
#[should_panic(expected = "pinned TX pool boundary changed")]
fn explicit_pool_audit_checks_quarantined_slots() {
    type TwoSlotPool = PinnedDmaTxPool<32, 8, 4, 2>;
    let pool = TwoSlotPool::pin_static(Box::leak(Box::new(TwoSlotPool::new())));
    #[allow(unsafe_code, reason = "simulated DMA overrun in a pinned allocation")]
    // SAFETY: this test models an external DMA write; no guard reader runs
    // concurrently and the permanent allocation remains live.
    unsafe {
        (*pool.pool.slots[1].dma_overrun_guard.get())[7] = 0;
    }
    let _ = pool.claimed_slots();
}

#[test]
fn a_zeroed_pool_is_the_free_coherent_pool_new_builds() {
    let pool = pinned_pool();
    for slot in &pool.pool.slots {
        assert_eq!(slot.state.load(Ordering::Acquire), SLOT_FREE);
        assert_eq!(slot.length.load(Ordering::Acquire), 0);
    }
    // No prepare hook is coherent memory: the bytes stay as written.
    let mut radio = prepared_radio(&pool);
    StableDmaBacking::prepare_for_dma_read(&mut radio);
    assert_eq!(radio.ethernet(), &[1, 2, 3, 4]);
}

#[test]
#[should_panic(expected = "crossed its backing boundary")]
fn pinning_installs_the_guard_a_dma_overrun_breaks() {
    let pool = pinned_pool();
    let radio = prepared_radio(&pool);
    #[allow(unsafe_code, reason = "simulated DMA overrun")]
    // SAFETY: the test plays the DMA actor that overruns the backing; no
    // lease reads the guard concurrently.
    unsafe {
        (*pool.pool.slots[0].dma_overrun_guard.get())[0] = 0;
    }
    drop(radio);
}
