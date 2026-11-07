extern crate std;

use oer_portable_memory::{HardwareOwnedTxDma, PreparedTxDma, StableDmaBacking, StableDmaRegion};
use std::{boxed::Box, cell::Cell, rc::Rc};

use super::*;

use crate::radio::ieee80211::dma::space::{HOST_ARENA_BASE, HostDma};
use crate::system::memory::dma_static::PlacedTxLease;

/// The host address of every storage's first descriptor.
const DESCRIPTOR_BASE: u32 = HOST_ARENA_BASE;

fn storage() -> PinnedAmpduDmaStorage<4, 256> {
    AmpduDmaStorage::pin::<HostDma>(crate::system::memory::dma_static::Placed::host(
        std::boxed::Box::leak(std::boxed::Box::new(AmpduDmaStorage::new())),
    ))
    .unwrap()
}

struct TestBytes {
    bytes: Box<[u8]>,
    drops: Rc<Cell<usize>>,
    region_calls: Option<Rc<Cell<usize>>>,
    prepare_calls: Option<Rc<Cell<usize>>>,
}

struct LargeTestBytes {
    bytes: Box<[u8]>,
}

impl TestBytes {
    fn new(drops: Rc<Cell<usize>>) -> Self {
        Self {
            bytes: Box::new([0; 128]),
            drops,
            region_calls: None,
            prepare_calls: None,
        }
    }

    fn counting(drops: Rc<Cell<usize>>, region_calls: Rc<Cell<usize>>) -> Self {
        Self {
            bytes: Box::new([0; 128]),
            drops,
            region_calls: Some(region_calls),
            prepare_calls: None,
        }
    }

    fn observing_prepare(drops: Rc<Cell<usize>>, prepare_calls: Rc<Cell<usize>>) -> Self {
        Self {
            bytes: Box::new([0; 128]),
            drops,
            region_calls: None,
            prepare_calls: Some(prepare_calls),
        }
    }
}

impl Drop for TestBytes {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
    }
}

#[allow(
    unsafe_code,
    reason = "test backing owns one non-moving boxed allocation"
)]
// SAFETY: the boxed bytes never move or get released while this backing
// exists, and the backing exposes no operation that frees them early.
// Forgetting the backing leaks its Box, so the allocation survives indefinitely.
unsafe impl StableDmaBacking for TestBytes {
    fn stable_dma_region(&mut self) -> StableDmaRegion<'_> {
        if let Some(calls) = &self.region_calls {
            calls.set(calls.get() + 1);
        }
        // SAFETY: moving `TestBytes` does not move its boxed allocation.
        unsafe { StableDmaRegion::new(&mut self.bytes[..]) }
    }

    fn prepare_for_dma_read(&mut self) {
        if let Some(calls) = &self.prepare_calls {
            calls.set(calls.get() + 1);
        }
    }
}

#[allow(
    unsafe_code,
    reason = "test backing owns one non-moving boxed allocation"
)]
// SAFETY: the boxed bytes never move or get released while this backing
// exists, and the backing exposes no operation that frees them early.
// Forgetting the backing leaks its Box, so the allocation survives indefinitely.
unsafe impl StableDmaBacking for LargeTestBytes {
    fn stable_dma_region(&mut self) -> StableDmaRegion<'_> {
        // SAFETY: moving `LargeTestBytes` does not move its boxed allocation.
        unsafe { StableDmaRegion::new(&mut self.bytes) }
    }
}

/// The host backings, as a placed pool's leases: A-MPDU takes only
/// [`PlacedBacking`]s.
type TestBacking = PlacedTxLease<TestBytes>;

/// [`TestBytes`]' constructors, for a [`TestBacking`].
trait HostBacking {
    fn new(drops: Rc<Cell<usize>>) -> Self;
    fn counting(drops: Rc<Cell<usize>>, region_calls: Rc<Cell<usize>>) -> Self;
    fn observing_prepare(drops: Rc<Cell<usize>>, prepare_calls: Rc<Cell<usize>>) -> Self;
}

impl HostBacking for TestBacking {
    fn new(drops: Rc<Cell<usize>>) -> Self {
        PlacedTxLease::host(TestBytes::new(drops))
    }

    fn counting(drops: Rc<Cell<usize>>, region_calls: Rc<Cell<usize>>) -> Self {
        PlacedTxLease::host(TestBytes::counting(drops, region_calls))
    }

    fn observing_prepare(drops: Rc<Cell<usize>>, prepare_calls: Rc<Cell<usize>>) -> Self {
        PlacedTxLease::host(TestBytes::observing_prepare(drops, prepare_calls))
    }
}

fn descriptor_only_storage() -> PinnedAmpduDmaStorage<2, 0> {
    AmpduDmaStorage::pin::<HostDma>(crate::system::memory::dma_static::Placed::host(Box::leak(
        Box::new(AmpduDmaStorage::new()),
    )))
    .unwrap()
}

fn retained_owner() -> RetainedAmpduDma<'static, TestBacking, 2, 0> {
    let retention = Box::leak(Box::new(RetainedAmpduDmaStorage::new()));
    RetainedAmpduDma::new(descriptor_only_storage(), retention)
}

fn retained_owner_with_slots<const SLOTS: usize>()
-> RetainedAmpduDma<'static, TestBacking, SLOTS, 0> {
    let dma = AmpduDmaStorage::pin::<HostDma>(crate::system::memory::dma_static::Placed::host(
        Box::leak(Box::new(AmpduDmaStorage::new())),
    ))
    .unwrap();
    let retention = Box::leak(Box::new(RetainedAmpduDmaStorage::new()));
    RetainedAmpduDma::new(dma, retention)
}

/// Push `backing` through [`RetainedAmpduDma::push_written`]: its
/// descriptor's buffer starts `offset` octets into its region and holds
/// `capacity`, `transfer` of them read; `write` edits the region first.
/// The region's address, or the refused backing.
fn push_at<'r, B: PlacedBacking, const SLOTS: usize>(
    owner: &mut RetainedAmpduDma<'r, B, SLOTS, 0>,
    backing: B,
    written: WrittenBuffer,
    write: impl FnOnce(&mut [u8]),
) -> Result<usize, (B, PushRefused<()>)> {
    let mut at = 0;
    owner.push_written(
        backing,
        &mut (),
        |(), region| {
            at = region.as_ptr().addr();
            write(region);
            Ok((written, ()))
        },
        |(), ()| Ok(()),
    )?;
    Ok(at)
}

/// A descriptor buffer of 64 octets at `offset`, `transfer` of them read.
const fn written(offset: usize, transfer: u32) -> WrittenBuffer {
    WrittenBuffer {
        offset,
        capacity: 64,
        transfer_length: transfer,
    }
}

#[test]
fn retaining_a_backing_samples_its_dma_region_once() {
    let drops = Rc::new(Cell::new(0));
    let region_calls = Rc::new(Cell::new(0));
    let mut owner = retained_owner();
    owner.begin().unwrap();

    let lease = TestBacking::counting(drops.clone(), region_calls.clone());
    assert_eq!(
        region_calls.get(),
        1,
        "host placement samples the allocation once"
    );
    region_calls.set(0);
    let mut length = 0;
    push_at(&mut owner, lease, written(0, 40), |region| {
        length = region.len()
    })
    .unwrap_or_else(|_| panic!("pushed"));
    assert_eq!(length, 128);
    assert_eq!(region_calls.get(), 1);
    owner.cancel(drop).unwrap();
    assert_eq!(drops.get(), 1);
}

#[test]
fn internal_chain_publishes_phased_dma_authority() {
    let mut storage = storage();
    storage.begin().unwrap();
    let AmpduDmaAccess::Writable(mut software) = storage.access() else {
        panic!("fixture owns software backing")
    };
    software.buffer_mut(0).unwrap()[0] = 0x11;
    software.buffer_mut(1).unwrap()[0] = 0x22;
    let entries = [
        AmpduInternalDescriptor {
            buffer_capacity: 256,
            transfer_length: 100,
        },
        AmpduInternalDescriptor {
            buffer_capacity: 128,
            transfer_length: 80,
        },
    ];
    let publication = storage.publish_internal_chain(&entries).unwrap();
    assert_eq!(publication.descriptor_head().bits(), DESCRIPTOR_BASE);

    let mut start_head = 0;
    publication.commit(|start| start_head = start.descriptor_head().bits());
    assert_eq!(start_head, DESCRIPTOR_BASE);
    assert_eq!(storage.state(), AmpduDmaState::HardwareOwned);
    assert_eq!(storage.descriptor_word0(0).unwrap().size(), 256);
    assert_eq!(storage.descriptor_word0(0).unwrap().length(), 100);
    assert_eq!(
        storage.descriptor_next_address(0),
        Some(DESCRIPTOR_BASE + DESCRIPTOR_BYTES)
    );
    assert_eq!(storage.descriptor_next_address(1), Some(0));

    storage.mark_completed().unwrap();
    assert!(storage.detached_buffer(0).is_err());
    storage
        .mark_detached(MacTxQueueDetached::new_model(DESCRIPTOR_BASE))
        .unwrap();
    assert_eq!(storage.detached_buffer(0).unwrap()[0], 0x11);
    storage.release_detached().unwrap();
    assert_eq!(storage.state(), AmpduDmaState::Free);
}

#[test]
fn terminal_release_clears_only_the_last_published_prefix() {
    let mut storage = storage();
    let entry = AmpduInternalDescriptor {
        buffer_capacity: 256,
        transfer_length: 100,
    };
    storage.begin().unwrap();
    storage
        .publish_internal_chain(&[entry; 4])
        .unwrap()
        .commit(|_| {});
    storage.mark_completed().unwrap();
    storage
        .mark_detached(MacTxQueueDetached::new_model(DESCRIPTOR_BASE))
        .unwrap();

    // Model a selective retry over the detached owner. Descriptors two
    // and three become an unreachable suffix once descriptor one is
    // terminal, but retain their old image until this arena publishes a
    // future transaction which actually uses them.
    storage
        .transition(AmpduDmaState::Detached, AmpduDmaState::Reserved)
        .unwrap();
    storage
        .publish_internal_chain(&[entry; 2])
        .unwrap()
        .commit(|_| {});
    let stale_suffix = storage.descriptor_word0(2).unwrap();
    assert_ne!(stale_suffix, WifiTxDmaDescriptorControl::ZERO);
    storage.mark_completed().unwrap();
    storage
        .mark_detached(MacTxQueueDetached::new_model(DESCRIPTOR_BASE))
        .unwrap();
    storage.release_detached().unwrap();

    assert_eq!(
        storage.descriptor_word0(0),
        Some(WifiTxDmaDescriptorControl::ZERO)
    );
    assert_eq!(
        storage.descriptor_word0(1),
        Some(WifiTxDmaDescriptorControl::ZERO)
    );
    assert_eq!(storage.descriptor_word0(2), Some(stale_suffix));
    assert_eq!(storage.state(), AmpduDmaState::Free);
}

#[test]
fn invalid_internal_chain_does_not_publish_capability() {
    let mut storage = storage();
    storage.begin().unwrap();
    assert!(
        storage
            .publish_internal_chain(&[AmpduInternalDescriptor {
                buffer_capacity: 257,
                transfer_length: 1,
            }])
            .is_err()
    );
    assert_eq!(storage.state(), AmpduDmaState::Reserved);
    storage.cancel().unwrap();
}

#[test]
fn late_internal_word_overflow_does_not_partially_publish_the_chain() {
    let arena = Box::leak(Box::new(AmpduDmaStorage::<2, 20_000>::new()));
    let mut storage =
        AmpduDmaStorage::pin::<HostDma>(crate::system::memory::dma_static::Placed::host(arena))
            .unwrap();
    storage.begin().unwrap();

    assert!(matches!(
        storage.publish_internal_chain(&[
            AmpduInternalDescriptor {
                buffer_capacity: 128,
                transfer_length: 64,
            },
            AmpduInternalDescriptor {
                buffer_capacity: 0x4000,
                transfer_length: 64,
            },
        ]),
        Err(AmpduDmaStorageError::InvalidLength)
    ));
    assert_eq!(
        storage.descriptor_word0(0),
        Some(WifiTxDmaDescriptorControl::ZERO)
    );
    assert_eq!(storage.descriptor_buffer_address(0), Some(0));
    storage.cancel().unwrap();
}

#[test]
fn a_word_past_its_limit_is_refused_at_push_and_nothing_is_published() {
    let retention = Box::leak(Box::new(RetainedAmpduDmaStorage::new()));
    let mut owner = RetainedAmpduDma::new(descriptor_only_storage(), retention);
    owner.begin().unwrap();
    let large = || {
        PlacedTxLease::host(LargeTestBytes {
            bytes: std::vec![0; 20_000].into_boxed_slice(),
        })
    };
    let first = WrittenBuffer {
        offset: 0,
        capacity: 128,
        transfer_length: 64,
    };
    assert!(push_at(&mut owner, large(), first, |_| {}).is_ok());
    let overflow = WrittenBuffer {
        capacity: 0x4000,
        ..first
    };
    assert!(matches!(
        push_at(&mut owner, large(), overflow, |_| {}),
        Err((_, PushRefused::Storage(_)))
    ));
    assert_eq!(owner.held_backing_count(), 1);
    assert_eq!(
        owner.dma().unwrap().descriptor_word0(0),
        Some(WifiTxDmaDescriptorControl::ZERO)
    );
    assert_eq!(owner.dma().unwrap().descriptor_buffer_address(0), Some(0));
    owner.cancel(drop).unwrap();
}

#[test]
fn reset_quarantine_has_no_release_edge() {
    let mut storage = storage();
    storage.begin().unwrap();
    storage.quarantine();
    assert_eq!(storage.state(), AmpduDmaState::ResetRequired);
    assert_eq!(storage.cancel(), Err(AmpduDmaStorageError::State));
    assert_eq!(storage.begin(), Err(AmpduDmaStorageError::State));
}

#[test]
fn detach_proof_must_name_the_aggregate_head() {
    let mut storage = storage();
    storage.begin().unwrap();
    storage
        .publish_internal_chain(&[AmpduInternalDescriptor {
            buffer_capacity: 256,
            transfer_length: 64,
        }])
        .unwrap()
        .commit(|_| {});
    storage.mark_completed().unwrap();

    assert_eq!(
        storage.mark_detached(MacTxQueueDetached::new_model(DESCRIPTOR_BASE + 4)),
        Err(AmpduDmaStorageError::Address)
    );
    assert_eq!(storage.state(), AmpduDmaState::Completed);
    storage
        .mark_detached(MacTxQueueDetached::new_model(DESCRIPTOR_BASE))
        .unwrap();
    storage.release_detached().unwrap();
}

#[test]
fn zero_slot_arena_is_rejected() {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(AmpduDmaStorage::<0, 64>::new()));
    assert!(
        AmpduDmaStorage::pin::<HostDma>(crate::system::memory::dma_static::Placed::host(storage))
            .is_err()
    );
}

#[test]
fn a_retained_chain_keeps_its_backings_through_detach() {
    let drops = Rc::new(Cell::new(0));
    let mut owner = retained_owner();
    owner.begin().unwrap();
    push_at(
        &mut owner,
        TestBacking::new(drops.clone()),
        written(8, 40),
        |region| region[8] = 0x11,
    )
    .unwrap_or_else(|_| panic!("pushed"));
    push_at(
        &mut owner,
        TestBacking::new(drops.clone()),
        written(16, 48),
        |region| region[16] = 0x22,
    )
    .unwrap_or_else(|_| panic!("pushed"));
    let publication = owner.publish_retained_chain(2).unwrap();
    assert_eq!(publication.descriptor_head().bits(), DESCRIPTOR_BASE);
    publication.commit(|start| assert_eq!(start.descriptor_head().bits(), DESCRIPTOR_BASE));
    assert_eq!(owner.state(), AmpduDmaState::HardwareOwned);
    assert_eq!(drops.get(), 0);

    owner.mark_completed().unwrap();
    assert!(owner.software_region_mut(0).is_err());
    owner
        .mark_detached(MacTxQueueDetached::new_model(DESCRIPTOR_BASE))
        .unwrap();
    assert_eq!(owner.software_region_mut(0).unwrap()[8], 0x11);
    assert_eq!(owner.software_region_mut(1).unwrap()[16], 0x22);
    owner.release_detached(drop).unwrap();
    assert_eq!(owner.state(), AmpduDmaState::Free);
    assert_eq!(drops.get(), 2);
}

#[test]
fn a_retained_chain_prepares_each_backing_before_publication() {
    let drops = Rc::new(Cell::new(0));
    let prepare_calls = Rc::new(Cell::new(0));
    let mut owner = retained_owner();
    owner.begin().unwrap();
    push_at(
        &mut owner,
        TestBacking::observing_prepare(drops.clone(), prepare_calls.clone()),
        written(0, 40),
        |_| {},
    )
    .unwrap_or_else(|_| panic!("pushed"));

    assert_eq!(prepare_calls.get(), 0);
    owner.publish_retained_chain(1).unwrap().commit(|_| {});
    assert_eq!(prepare_calls.get(), 1);

    owner.mark_completed().unwrap();
    owner
        .mark_detached(MacTxQueueDetached::new_model(DESCRIPTOR_BASE))
        .unwrap();
    owner.release_detached(drop).unwrap();
    assert_eq!(drops.get(), 1);
}

#[test]
fn full_retention_arena_reports_count_without_mutating_the_owner() {
    let drops = Rc::new(Cell::new(0));
    let mut owner = retained_owner();
    owner.begin().unwrap();
    for _ in 0..2 {
        assert!(
            push_at(
                &mut owner,
                TestBacking::new(drops.clone()),
                written(0, 40),
                |_| {}
            )
            .is_ok()
        );
    }

    let refused = push_at(
        &mut owner,
        TestBacking::new(drops.clone()),
        written(0, 40),
        |_| {},
    );
    assert!(matches!(
        refused,
        Err((_, PushRefused::Storage(AmpduDmaStorageError::Count)))
    ));
    drop(refused);
    assert_eq!(owner.held_backing_count(), 2);
    assert_eq!(drops.get(), 1);
    owner.cancel(drop).unwrap();
    assert_eq!(drops.get(), 3);
}

#[test]
fn dropping_a_hardware_owned_chain_forgets_its_backings_without_unwinding() {
    let drops = Rc::new(Cell::new(0));
    let mut owner = retained_owner();
    owner.begin().unwrap();
    push_at(
        &mut owner,
        TestBacking::new(drops.clone()),
        written(0, 32),
        |_| {},
    )
    .unwrap_or_else(|_| panic!("pushed"));
    owner.publish_retained_chain(1).unwrap().commit(|_| {});

    drop(owner);
    assert_eq!(drops.get(), 0);
}

/// Begin a chain of two fresh leases: their region addresses.
fn push_two(
    owner: &mut RetainedAmpduDma<'static, TestBacking, 2, 0>,
    drops: &Rc<Cell<usize>>,
) -> [usize; 2] {
    owner.begin().unwrap();
    let mut push = || {
        push_at(
            owner,
            TestBacking::new(drops.clone()),
            written(0, 8),
            |_| {},
        )
        .unwrap_or_else(|_| panic!("pushed"))
    };
    let first = push();
    [first, push()]
}

/// Publish the two pushed leases, eight bytes each.
fn publish_two(owner: &mut RetainedAmpduDma<'static, TestBacking, 2, 0>) {
    owner.publish_retained_chain(2).unwrap().commit(|_| {});
}

#[test]
fn every_release_returns_the_leases_in_push_order() {
    let drops = Rc::new(Cell::new(0));
    let address =
        |mut backing: TestBacking| backing.stable_dma_region().as_mut_slice().as_ptr().addr();
    let mut owner = retained_owner();

    let pushed = push_two(&mut owner, &drops);
    let mut returned = std::vec::Vec::new();
    owner
        .cancel(|backing| returned.push(address(backing)))
        .unwrap();
    assert_eq!(returned, pushed);

    let pushed = push_two(&mut owner, &drops);
    publish_two(&mut owner);
    let mut returned = std::vec::Vec::new();
    owner
        .release_aborted(MacTxQueueDetached::new_model(DESCRIPTOR_BASE), |backing| {
            returned.push(address(backing))
        })
        .unwrap();
    assert_eq!(returned, pushed);

    let pushed = push_two(&mut owner, &drops);
    publish_two(&mut owner);
    owner.mark_completed().unwrap();
    owner
        .mark_detached(MacTxQueueDetached::new_model(DESCRIPTOR_BASE))
        .unwrap();
    let mut returned = std::vec::Vec::new();
    owner
        .release_detached(|backing| returned.push(address(backing)))
        .unwrap();
    assert_eq!(returned, pushed);
    // Only the sink dropped them, after reading their addresses.
    assert_eq!(drops.get(), 6);
    assert_eq!(owner.held_backing_count(), 0);
}

#[test]
fn dropping_a_reserved_chain_releases_its_backings() {
    let drops = Rc::new(Cell::new(0));
    let mut owner = retained_owner();
    owner.begin().unwrap();
    push_at(
        &mut owner,
        TestBacking::new(drops.clone()),
        written(0, 40),
        |_| {},
    )
    .unwrap_or_else(|_| panic!("pushed"));

    drop(owner);
    assert_eq!(drops.get(), 1);
}

#[test]
fn free_retained_owner_returns_its_dma_arena() {
    let retention = Box::leak(Box::new(RetainedAmpduDmaStorage::new()));
    let retention_address = core::ptr::from_mut(&mut *retention).addr();
    let owner = RetainedAmpduDma::<TestBacking, 2, 0>::new(descriptor_only_storage(), retention);
    let (dma, returned_retention) = match owner.try_into_parts() {
        Ok(parts) => parts,
        Err(_) => panic!("free owner must return its arena"),
    };

    assert_eq!(dma.state(), AmpduDmaState::Free);
    assert_eq!(
        core::ptr::from_mut(returned_retention).addr(),
        retention_address
    );
}

#[test]
fn retained_owner_is_a_small_handle_over_the_external_lease_arena() {
    assert!(
        core::mem::size_of::<RetainedAmpduDma<'static, TestBacking, 2, 0>>()
            < core::mem::size_of::<RetainedAmpduDmaStorage<TestBacking, 2>>()
    );
}

#[test]
fn zeroed_arena_is_the_free_arena_new_builds() {
    let zeroed = oer_portable_memory::zeroed::zeroed::<AmpduDmaStorage<4, 32>>();
    let new = AmpduDmaStorage::<4, 32>::new();
    assert_eq!(zeroed.state, new.state);
    assert_eq!(zeroed.state, AmpduDmaState::Free);
    assert_eq!(zeroed.lease_generation, new.lease_generation);
    assert_eq!(zeroed.published_count, new.published_count);
    for (zeroed, new) in zeroed.descriptors.iter().zip(&new.descriptors) {
        assert_eq!(
            (
                zeroed.word0(),
                zeroed.buffer_address(),
                zeroed.next_address()
            ),
            (new.word0(), new.buffer_address(), new.next_address())
        );
    }
    for (zeroed, new) in zeroed.buffers.iter().zip(&new.buffers) {
        assert_eq!(zeroed.0, new.0);
    }
}

#[test]
fn review_forgotten_dma_owner_cannot_release_live_backings_from_retention() {
    let drops = Rc::new(Cell::new(0));
    {
        let mut retention = RetainedAmpduDmaStorage::new();
        let mut owner = RetainedAmpduDma::new(descriptor_only_storage(), &mut retention);
        owner.begin().unwrap();
        push_at(
            &mut owner,
            TestBacking::new(drops.clone()),
            written(0, 32),
            |_| {},
        )
        .unwrap_or_else(|_| panic!("pushed"));
        owner.publish_retained_chain(1).unwrap().commit(|_| {});
        assert_eq!(owner.state(), AmpduDmaState::HardwareOwned);
        #[allow(clippy::mem_forget, reason = "safe caller code may forget the owner")]
        core::mem::forget(owner);
        // This is all safe caller code. Forget does not run the owner destructor,
        // and the borrowed retention arena is an ordinary local value.
    }
    assert_eq!(
        drops.get(),
        0,
        "hardware-visible backing was returned/freed without detach"
    );
}

#[test]
#[allow(
    clippy::drop_non_drop,
    reason = "ending the retention arena explicitly demonstrates that its borrowed metadata can end before DMA custody"
)]
fn forgotten_aggregate_retains_allocation_after_borrowed_metadata_ends() {
    let drops = Rc::new(Cell::new(0));
    let dma_address;
    {
        let mut tag = 3;
        let mut retention = RetainedAmpduDmaStorage::new();
        let mut owner = RetainedAmpduDma::new(descriptor_only_storage(), &mut retention);
        owner.begin().unwrap();
        let lease = oer_portable_memory::TaggedStableDmaBacking::new(
            &mut tag,
            TestBacking::new(drops.clone()),
        );
        let mut region_at = core::ptr::null();
        push_at(&mut owner, lease, written(0, 32), |region| {
            region[0] = 0x5d;
            region_at = region.as_ptr();
        })
        .unwrap_or_else(|_| panic!("pushed"));
        dma_address = region_at;
        owner.publish_retained_chain(1).unwrap().commit(|_| {});
        #[allow(clippy::mem_forget, reason = "safe caller code may forget the owner")]
        core::mem::forget(owner);
        // The arena and the CPU-only metadata can both end. Neither owns the
        // allocation once the lease is retained in its ManuallyDrop cell.
        drop(retention);
        tag = 8;
        assert_eq!(tag, 8);
    }
    assert_eq!(drops.get(), 0);
    #[allow(
        unsafe_code,
        reason = "the test models a DMA read after the retaining arena ends"
    )]
    // SAFETY: TestBacking owns the Box; forgetting its owner and dropping the
    // ManuallyDrop arena leave that allocation live, with no competing writer.
    let observed = unsafe { dma_address.read() };
    assert_eq!(observed, 0x5d);
}

#[test]
fn a_retained_chain_publishes_each_lease_at_its_address_in_the_arena_space() {
    let drops = Rc::new(Cell::new(0));
    let mut owner = retained_owner();
    owner.begin().unwrap();
    for (offset, transfer) in [(8, 40), (16, 48)] {
        push_at(
            &mut owner,
            TestBacking::new(drops.clone()),
            written(offset, transfer),
            |_| {},
        )
        .unwrap_or_else(|_| panic!("pushed"));
    }
    owner.publish_retained_chain(2).unwrap().commit(|_| {});

    // A host lease is its own arena: its first byte is the window base.
    assert_eq!(
        owner.dma().unwrap().descriptor_buffer_address(0),
        Some(HOST_ARENA_BASE + 8)
    );
    assert_eq!(
        owner.dma().unwrap().descriptor_buffer_address(1),
        Some(HOST_ARENA_BASE + 16)
    );
    owner.mark_completed().unwrap();
    owner
        .mark_detached(MacTxQueueDetached::new_model(DESCRIPTOR_BASE))
        .unwrap();
    owner.release_detached(drop).unwrap();
    assert_eq!(drops.get(), 2);
}

#[test]
fn an_empty_backing_is_removed_without_leaving_a_lifo_hole() {
    let drops = Rc::new(Cell::new(0));
    let mut owner = retained_owner();
    owner.begin().unwrap();
    let mut empty = TestBytes::new(drops.clone());
    empty.bytes = std::vec::Vec::new().into_boxed_slice();
    let refused = push_at(
        &mut owner,
        PlacedTxLease::host(empty),
        written(0, 40),
        |_| {},
    );
    assert!(matches!(
        refused,
        Err((_, PushRefused::Storage(AmpduDmaStorageError::Address)))
    ));
    drop(refused);
    assert_eq!(owner.held, 0);
    assert_eq!(drops.get(), 1);
    assert!(owner.retention().unwrap().backings[0].is_none());
    push_at(
        &mut owner,
        TestBacking::new(drops.clone()),
        written(0, 40),
        |_| {},
    )
    .unwrap_or_else(|_| panic!("pushed"));
    assert_eq!(owner.held, 1, "the next lease takes position 0");
    owner.cancel(drop).unwrap();
    assert_eq!(drops.get(), 2);
}

#[test]
fn a_failed_insertion_between_successes_preserves_backing_indices() {
    let drops = Rc::new(Cell::new(0));
    let mut owner = retained_owner();
    owner.begin().unwrap();
    let mut tag = |owner: &mut RetainedAmpduDma<'static, TestBacking, 2, 0>, value: u8| {
        push_at(
            owner,
            TestBacking::new(drops.clone()),
            written(0, 40),
            |region| region[0] = value,
        )
        .unwrap_or_else(|_| panic!("pushed"));
    };
    tag(&mut owner, 1);
    let rejected = PlacedTxLease::host_refused(
        TestBytes::new(drops.clone()),
        crate::system::memory::dma_address::ProjectionError::AddressWidth,
    );
    let refused = push_at(&mut owner, rejected, written(0, 40), |_| {});
    assert!(matches!(
        refused,
        Err((_, PushRefused::Storage(AmpduDmaStorageError::Address)))
    ));
    drop(refused);
    assert_eq!(owner.held, 1);
    assert_eq!(drops.get(), 1);
    assert!(owner.retention().unwrap().backings[1].is_none());
    assert!(owner.retention().unwrap().backing_identities[1].is_none());
    assert!(owner.retention().unwrap().backing_descriptors[1].is_none());
    tag(&mut owner, 2);
    let mut returned = std::vec::Vec::new();
    owner
        .cancel(|backing| returned.push(tag_of(backing)))
        .unwrap();
    assert_eq!(returned, [1, 2], "positions 0 and 1, in order");
    assert_eq!(drops.get(), 3);
}

#[test]
fn an_owned_next_slot_refuses_the_append_without_changing_the_owner() {
    let drops = Rc::new(Cell::new(0));
    let mut owner = retained_owner();
    owner.begin().unwrap();
    push_at(
        &mut owner,
        TestBacking::new(drops.clone()),
        written(0, 40),
        |_| {},
    )
    .unwrap_or_else(|_| panic!("pushed"));
    // Model corrupted retained metadata: the next slot already names an
    // identity. The append refuses the backing instead of panicking.
    let identity = owner.retention().unwrap().backing_identities[0];
    owner.retention_mut().unwrap().backing_identities[1] = identity;
    let refused = push_at(
        &mut owner,
        TestBacking::new(drops.clone()),
        written(0, 40),
        |_| {},
    );
    assert!(matches!(
        refused,
        Err((_, PushRefused::Storage(AmpduDmaStorageError::StaleBacking)))
    ));
    drop(refused);
    assert_eq!(owner.held, 1);
    assert_eq!(drops.get(), 1);
    owner.retention_mut().unwrap().backing_identities[1] = None;
    owner.cancel(drop).unwrap();
    assert_eq!(drops.get(), 2);
}

/// Push a counting backing through [`RetainedAmpduDma::push_written`],
/// tagging its first octet; returns its region's address.
fn push_tagged<const SLOTS: usize>(
    owner: &mut RetainedAmpduDma<'static, TestBacking, SLOTS, 0>,
    drops: &Rc<Cell<usize>>,
    regions: &Rc<Cell<usize>>,
    tag: u8,
) -> usize {
    let mut at = 0;
    let pushed = owner.push_written(
        TestBacking::counting(drops.clone(), regions.clone()),
        &mut (),
        |(), region| {
            at = region.as_ptr().addr();
            region[0] = tag;
            Ok::<_, ()>((
                WrittenBuffer {
                    offset: 0,
                    capacity: 64,
                    transfer_length: 40,
                },
                (),
            ))
        },
        |(), ()| Ok(()),
    );
    assert!(pushed.is_ok());
    at
}

fn tag_of(mut backing: TestBacking) -> u8 {
    backing.stable_dma_region().as_mut_slice()[0]
}

#[test]
fn a_written_push_binds_its_word_once_and_a_retained_retry_relinks_it() {
    let (drops, regions) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
    let mut owner = retained_owner_with_slots::<4>();
    owner.begin().unwrap();
    for tag in 0..3 {
        push_tagged(&mut owner, &drops, &regions, tag);
    }
    // A host lease samples its region once at placement, the owner once.
    assert_eq!(regions.get(), 6, "each region sampled once, at intake");
    let words: std::vec::Vec<_> = (0..3).map(|at| owner.bound_word0(at).unwrap()).collect();
    owner.publish_retained_chain(3).unwrap().commit(|_| {});
    owner.mark_completed().unwrap();
    owner
        .mark_detached(MacTxQueueDetached::new_model(DESCRIPTOR_BASE))
        .unwrap();

    let mut returned = std::vec::Vec::new();
    let kept = owner
        .retain_detached(0b101, |backing| returned.push(tag_of(backing)))
        .unwrap();
    assert_eq!((kept, returned), (2, std::vec![1]));
    assert_eq!(owner.state(), AmpduDmaState::Reserved);
    assert_eq!(owner.held_backing_count(), 2);
    // The kept images moved with their leases, unchanged.
    assert_eq!(owner.bound_word0(0), Some(words[0]));
    assert_eq!(owner.bound_word0(1), Some(words[2]));
    assert_eq!(owner.software_region_mut(1).unwrap()[0], 2);
    let sampled = regions.get();
    owner.publish_retained_chain(2).unwrap().commit(|_| {});
    assert_eq!(regions.get(), sampled, "no region sampled at republication");
    assert_eq!(owner.state(), AmpduDmaState::HardwareOwned);
    owner.mark_completed().unwrap();
    owner
        .mark_detached(MacTxQueueDetached::new_model(DESCRIPTOR_BASE))
        .unwrap();
    let mut returned = std::vec::Vec::new();
    owner
        .release_detached(|backing| returned.push(tag_of(backing)))
        .unwrap();
    assert_eq!(returned, std::vec![0, 2]);
    assert_eq!(drops.get(), 3);
}

#[test]
fn a_retained_chain_takes_further_leases_after_its_kept_ones() {
    let (drops, regions) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
    let mut owner = retained_owner_with_slots::<4>();
    owner.begin().unwrap();
    for tag in 0..2 {
        push_tagged(&mut owner, &drops, &regions, tag);
    }
    owner.publish_retained_chain(2).unwrap().commit(|_| {});
    owner
        .mark_aborted_detached(MacTxQueueDetached::new_model(DESCRIPTOR_BASE))
        .unwrap();
    assert_eq!(owner.held_backing_count(), 2, "an abort keeps its leases");
    assert_eq!(owner.retain_detached(0b10, drop).unwrap(), 1);
    push_tagged(&mut owner, &drops, &regions, 7);
    let mut returned = std::vec::Vec::new();
    owner
        .cancel(|backing| returned.push(tag_of(backing)))
        .unwrap();
    assert_eq!(returned, std::vec![1, 7]);
}

#[test]
fn a_retention_outside_the_chain_or_of_a_live_chain_changes_nothing() {
    let (drops, regions) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
    let mut owner = retained_owner_with_slots::<4>();
    owner.begin().unwrap();
    push_tagged(&mut owner, &drops, &regions, 0);
    assert_eq!(
        owner.retain_detached(1, drop),
        Err(AmpduDmaStorageError::State)
    );
    owner.publish_retained_chain(1).unwrap().commit(|_| {});
    owner.mark_completed().unwrap();
    owner
        .mark_detached(MacTxQueueDetached::new_model(DESCRIPTOR_BASE))
        .unwrap();
    assert_eq!(
        owner.retain_detached(0b10, drop),
        Err(AmpduDmaStorageError::Count)
    );
    assert_eq!(owner.state(), AmpduDmaState::Detached);
    assert_eq!(drops.get(), 0);
    owner.release_detached(drop).unwrap();
}

#[test]
fn a_refused_written_push_returns_its_backing_as_written() {
    let drops = Rc::new(Cell::new(0));
    let mut owner = retained_owner_with_slots::<4>();
    let written = WrittenBuffer {
        offset: 0,
        capacity: 64,
        transfer_length: 40,
    };
    let refused = owner.push_written(
        TestBacking::new(drops.clone()),
        &mut (),
        |(), _| Ok::<_, &str>((written, ())),
        |(), ()| Ok(()),
    );
    let Err((backing, PushRefused::Storage(AmpduDmaStorageError::State))) = refused else {
        panic!("an unreserved chain refuses");
    };
    owner.begin().unwrap();
    let refused = owner.push_written(
        backing,
        &mut (),
        |(), region| {
            region[0] = 9;
            Err::<(WrittenBuffer, ()), _>("declined")
        },
        |(), ()| Ok(()),
    );
    let Err((backing, PushRefused::Written("declined"))) = refused else {
        panic!("the writer refused");
    };
    let refused = owner.push_written(
        backing,
        &mut (),
        |(), _| Ok((written, ())),
        |(), ()| Err("late"),
    );
    let Err((backing, PushRefused::Written("late"))) = refused else {
        panic!("the commit refused");
    };
    assert_eq!(tag_of(backing), 9);
    let outside = WrittenBuffer {
        offset: 100,
        ..written
    };
    let refused = owner.push_written(
        TestBacking::new(drops.clone()),
        &mut (),
        |(), _| Ok::<_, &str>((outside, ())),
        |(), ()| Ok(()),
    );
    assert!(matches!(
        refused,
        Err((_, PushRefused::Storage(AmpduDmaStorageError::Address)))
    ));
    drop(refused);
    assert_eq!(owner.held_backing_count(), 0);
    assert_eq!(
        drops.get(),
        2,
        "only the test dropped the returned backings"
    );
}

#[test]
fn a_published_head_keeps_the_leases_behind_it_until_the_release() {
    let (drops, regions) = (Rc::new(Cell::new(0)), Rc::new(Cell::new(0)));
    let mut owner = retained_owner_with_slots::<4>();
    owner.begin().unwrap();
    for tag in 0..3 {
        push_tagged(&mut owner, &drops, &regions, tag);
    }
    assert!(owner.publish_retained_chain(4).is_err());
    owner.publish_retained_chain(1).unwrap().commit(|_| {});
    owner.mark_completed().unwrap();
    owner
        .mark_detached(MacTxQueueDetached::new_model(DESCRIPTOR_BASE))
        .unwrap();
    let mut returned = std::vec::Vec::new();
    owner
        .release_detached(|backing| returned.push(tag_of(backing)))
        .unwrap();
    assert_eq!(returned, std::vec![0, 1, 2]);
}

#[test]
fn a_model_follows_a_published_chain_to_its_last_descriptor() {
    use super::super::tx_storage::{CommitTxDma, TxChainUnreadable};
    let drops = Rc::new(Cell::new(0));
    let mut owner = retained_owner();
    owner.begin().unwrap();
    for (offset, used, mark) in [(8, 40, 0x11), (16, 48, 0x22)] {
        push_at(
            &mut owner,
            TestBacking::new(drops.clone()),
            written(offset, used),
            |region| region[offset] = mark,
        )
        .unwrap_or_else(|_| panic!("pushed"));
    }
    let mut seen = std::vec::Vec::new();
    let mut publication = owner.publish_retained_chain(2).unwrap();
    assert_eq!(
        publication.read_chain(&mut |bytes| seen.push((bytes.len(), bytes[0]))),
        Ok(())
    );
    assert_eq!(seen, [(40, 0x11), (48, 0x22)]);

    // A chain of one ends at its first descriptor, whatever follows it.
    drop(publication);
    let mut seen = 0;
    let mut publication = owner.publish_retained_chain(1).unwrap();
    assert_eq!(publication.read_chain(&mut |_| seen += 1), Ok(()));
    assert_eq!(seen, 1);

    // A descriptor the hardware does not own, then a buffer no lease holds.
    let buffer = publication.retention.backing_identities[0]
        .unwrap()
        .dma_region
        .start();
    let arena = publication.dma.storage.as_ref().get_ref();
    let word0 = arena.descriptors[0].word0();
    arena.descriptors[0].publish_owned(word0.with_tx_owned(false), buffer, None);
    assert_eq!(
        publication.read_chain(&mut |_| ()),
        Err(TxChainUnreadable::NotOwned)
    );
    let arena = publication.dma.storage.as_ref().get_ref();
    arena.descriptors[0].publish_owned(word0, buffer, None);
    publication.retention.backing_identities[0] = None;
    assert_eq!(
        publication.read_chain(&mut |_| ()),
        Err(TxChainUnreadable::OutsideBuffers)
    );
}
