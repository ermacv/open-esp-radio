extern crate std;

use oer_memory::{HardwareOwnedTxDma, PreparedTxDma, StableDmaBacking, StableDmaRegion};
use std::{boxed::Box, cell::Cell, rc::Rc};

use super::*;
use crate::descriptor::{length, size};

const DESCRIPTOR_BASE: u32 = 0x2f00_1000;
const BUFFER_BASE: u32 = 0x2f01_0000;

fn storage() -> PinnedAmpduDmaStorage<4, 256> {
    AmpduDmaStorage::pin_static_model(
        std::boxed::Box::leak(std::boxed::Box::new(AmpduDmaStorage::new())),
        DESCRIPTOR_BASE,
        BUFFER_BASE,
    )
    .unwrap()
}

struct TestBacking {
    bytes: Box<[u8; 128]>,
    drops: Rc<Cell<usize>>,
    region_calls: Option<Rc<Cell<usize>>>,
    prepare_calls: Option<Rc<Cell<usize>>>,
}

struct LargeTestBacking {
    bytes: Box<[u8]>,
}

impl TestBacking {
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

impl Drop for TestBacking {
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
unsafe impl StableDmaBacking for TestBacking {
    fn stable_dma_region(&mut self) -> StableDmaRegion<'_> {
        if let Some(calls) = &self.region_calls {
            calls.set(calls.get() + 1);
        }
        // SAFETY: moving `TestBacking` does not move its boxed allocation.
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
unsafe impl StableDmaBacking for LargeTestBacking {
    fn stable_dma_region(&mut self) -> StableDmaRegion<'_> {
        // SAFETY: moving `LargeTestBacking` does not move its boxed allocation.
        unsafe { StableDmaRegion::new(&mut self.bytes) }
    }
}

fn descriptor_only_storage() -> PinnedAmpduDmaStorage<2, 0> {
    AmpduDmaStorage::pin_static_model(
        Box::leak(Box::new(AmpduDmaStorage::new())),
        DESCRIPTOR_BASE,
        0,
    )
    .unwrap()
}

fn retained_owner() -> RetainedAmpduDma<'static, TestBacking, 2, 0> {
    let retention = Box::leak(Box::new(RetainedAmpduDmaStorage::new()));
    RetainedAmpduDma::new(descriptor_only_storage(), retention)
}

fn retained_owner_with_slots<const SLOTS: usize>()
-> RetainedAmpduDma<'static, TestBacking, SLOTS, 0> {
    let dma = AmpduDmaStorage::pin_static_model(
        Box::leak(Box::new(AmpduDmaStorage::new())),
        DESCRIPTOR_BASE,
        0,
    )
    .unwrap();
    let retention = Box::leak(Box::new(RetainedAmpduDmaStorage::new()));
    RetainedAmpduDma::new(dma, retention)
}

#[test]
fn retaining_a_backing_samples_its_dma_region_once() {
    let drops = Rc::new(Cell::new(0));
    let region_calls = Rc::new(Cell::new(0));
    let mut owner = retained_owner();
    owner.begin().unwrap();

    let (backing, bytes) = owner
        .push_backing_region(TestBacking::counting(drops.clone(), region_calls.clone()))
        .unwrap();
    assert_eq!(bytes.len(), 128);
    assert_eq!(region_calls.get(), 1);
    drop(owner.pop_last_backing(backing).unwrap());
    assert_eq!(drops.get(), 1);
}

#[test]
fn internal_chain_publishes_phased_dma_authority() {
    let mut storage = storage();
    storage.begin().unwrap();
    storage.buffer_mut(0).unwrap()[0] = 0x11;
    storage.buffer_mut(1).unwrap()[0] = 0x22;
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
    assert_eq!(publication.descriptor_head(), DESCRIPTOR_BASE);

    let mut start_head = 0;
    publication.commit(|start| start_head = start.descriptor_head());
    assert_eq!(start_head, DESCRIPTOR_BASE);
    assert_eq!(storage.state(), AmpduDmaState::HardwareOwned);
    assert_eq!(size(storage.descriptor_word0(0).unwrap()), 256);
    assert_eq!(length(storage.descriptor_word0(0).unwrap()), 100);
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
    assert_ne!(stale_suffix, 0);
    storage.mark_completed().unwrap();
    storage
        .mark_detached(MacTxQueueDetached::new_model(DESCRIPTOR_BASE))
        .unwrap();
    storage.release_detached().unwrap();

    assert_eq!(storage.descriptor_word0(0), Some(0));
    assert_eq!(storage.descriptor_word0(1), Some(0));
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
        AmpduDmaStorage::pin_static_model(arena, DESCRIPTOR_BASE, BUFFER_BASE).unwrap();
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
    assert_eq!(storage.descriptor_word0(0), Some(0));
    assert_eq!(storage.descriptor_buffer_address(0), Some(0));
    storage.cancel().unwrap();
}

#[test]
fn late_external_word_overflow_does_not_partially_publish_the_chain() {
    let retention = Box::leak(Box::new(RetainedAmpduDmaStorage::new()));
    let mut owner = RetainedAmpduDma::new(descriptor_only_storage(), retention);
    owner.begin().unwrap();
    let first = owner
        .push_backing(LargeTestBacking {
            bytes: std::vec![0; 20_000].into_boxed_slice(),
        })
        .unwrap();
    let second = owner
        .push_backing(LargeTestBacking {
            bytes: std::vec![0; 20_000].into_boxed_slice(),
        })
        .unwrap();
    let first_address = owner
        .reserved_backing_mut(&first)
        .unwrap()
        .bytes
        .as_ptr()
        .addr();
    let second_address = owner
        .reserved_backing_mut(&second)
        .unwrap()
        .bytes
        .as_ptr()
        .addr();
    let entries = [
        first.external_descriptor(first_address, 128, 64).unwrap(),
        second
            .external_descriptor(second_address, 0x4000, 64)
            .unwrap(),
    ];

    assert!(matches!(
        owner.publish_external_chain(&entries),
        Err(AmpduDmaStorageError::InvalidLength)
    ));
    assert_eq!(owner.dma().descriptor_word0(0), Some(0));
    assert_eq!(owner.dma().descriptor_buffer_address(0), Some(0));
    owner.cancel().unwrap();
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
    assert!(AmpduDmaStorage::pin_static_model(storage, DESCRIPTOR_BASE, BUFFER_BASE).is_err());
}

#[test]
fn external_chain_retains_backings_through_detach() {
    let drops = Rc::new(Cell::new(0));
    let mut owner = retained_owner();
    owner.begin().unwrap();
    let first = owner.push_backing(TestBacking::new(drops.clone())).unwrap();
    let second = owner.push_backing(TestBacking::new(drops.clone())).unwrap();
    owner.reserved_backing_mut(&first).unwrap().bytes[8] = 0x11;
    owner.reserved_backing_mut(&second).unwrap().bytes[16] = 0x22;
    let first_address = owner
        .reserved_backing_mut(&first)
        .unwrap()
        .bytes
        .as_ptr()
        .addr()
        + 8;
    let second_address = owner
        .reserved_backing_mut(&second)
        .unwrap()
        .bytes
        .as_ptr()
        .addr()
        + 16;
    let entries = [
        first.external_descriptor(first_address, 64, 40).unwrap(),
        second.external_descriptor(second_address, 64, 48).unwrap(),
    ];
    let publication = owner.publish_external_chain(&entries).unwrap();
    assert_eq!(publication.descriptor_head(), DESCRIPTOR_BASE);
    publication.commit(|start| assert_eq!(start.descriptor_head(), DESCRIPTOR_BASE));
    assert_eq!(owner.state(), AmpduDmaState::HardwareOwned);
    assert_eq!(drops.get(), 0);

    owner.mark_completed().unwrap();
    assert!(owner.detached_backing_mut(&first).is_err());
    owner
        .mark_detached(MacTxQueueDetached::new_model(DESCRIPTOR_BASE))
        .unwrap();
    assert_eq!(owner.detached_backing_mut(&first).unwrap().bytes[8], 0x11);
    owner.release_detached().unwrap();
    assert_eq!(owner.state(), AmpduDmaState::Free);
    assert_eq!(drops.get(), 2);
}

#[test]
fn external_chain_prepares_each_retained_backing_before_publication() {
    let drops = Rc::new(Cell::new(0));
    let prepare_calls = Rc::new(Cell::new(0));
    let mut owner = retained_owner();
    owner.begin().unwrap();
    let backing = owner
        .push_backing(TestBacking::observing_prepare(
            drops.clone(),
            prepare_calls.clone(),
        ))
        .unwrap();
    let address = owner
        .reserved_backing_mut(&backing)
        .unwrap()
        .bytes
        .as_ptr()
        .addr();
    let entry = backing.external_descriptor(address, 64, 40).unwrap();

    assert_eq!(prepare_calls.get(), 0);
    owner
        .publish_external_chain(&[entry])
        .unwrap()
        .commit(|_| {});
    assert_eq!(prepare_calls.get(), 1);

    owner.mark_completed().unwrap();
    owner
        .mark_detached(MacTxQueueDetached::new_model(DESCRIPTOR_BASE))
        .unwrap();
    owner.release_detached().unwrap();
    assert_eq!(drops.get(), 1);
}

#[test]
fn detached_external_chain_can_be_republished_for_retry() {
    let drops = Rc::new(Cell::new(0));
    let mut owner = retained_owner();
    owner.begin().unwrap();
    let backing = owner.push_backing(TestBacking::new(drops.clone())).unwrap();
    let address = owner
        .reserved_backing_mut(&backing)
        .unwrap()
        .bytes
        .as_ptr()
        .addr()
        + 8;
    let entry = backing.external_descriptor(address, 64, 40).unwrap();
    assert_eq!(entry.backing.unwrap().index(), backing.index());
    assert_eq!(entry.address, address);
    owner
        .publish_external_chain(&[entry])
        .unwrap()
        .commit(|_| {});
    owner.mark_completed().unwrap();
    owner
        .mark_detached(MacTxQueueDetached::new_model(DESCRIPTOR_BASE))
        .unwrap();

    owner.begin_retry().unwrap();
    assert_eq!(owner.held_backing_count(), 1);
    let retry = backing.external_descriptor(address, 64, 40).unwrap();
    owner
        .publish_external_chain(&[retry])
        .unwrap()
        .commit(|_| {});
    owner.mark_completed().unwrap();
    owner
        .mark_detached(MacTxQueueDetached::new_model(DESCRIPTOR_BASE))
        .unwrap();
    assert_eq!(owner.detached_region_mut(address, 64).unwrap().len(), 64);
    owner.release_detached().unwrap();
    assert_eq!(drops.get(), 1);
}

#[test]
fn detached_compaction_rejects_more_logical_entries_than_dma_slots() {
    let drops = Rc::new(Cell::new(0));
    let mut owner = retained_owner();
    owner.begin().unwrap();
    let first = owner.push_backing(TestBacking::new(drops.clone())).unwrap();
    let second = owner.push_backing(TestBacking::new(drops.clone())).unwrap();
    let first_address = owner
        .reserved_backing_mut(&first)
        .unwrap()
        .bytes
        .as_ptr()
        .addr();
    let second_address = owner
        .reserved_backing_mut(&second)
        .unwrap()
        .bytes
        .as_ptr()
        .addr();
    owner
        .commit_backing_descriptor(&first, first_address, 64, 32)
        .unwrap();
    owner
        .commit_backing_descriptor(&second, second_address, 64, 32)
        .unwrap();
    owner.publish_retained_chain(2).unwrap().commit(|_| {});
    owner.mark_completed().unwrap();
    owner
        .mark_detached(MacTxQueueDetached::new_model(DESCRIPTOR_BASE))
        .unwrap();

    assert_eq!(
        owner.compact_active_backings(&[0, 1, 0]),
        Err(AmpduDmaStorageError::Count)
    );
    assert_eq!(owner.state(), AmpduDmaState::Detached);
    assert_eq!(
        owner
            .detached_logical_region_mut(0, first_address, 64)
            .unwrap()[0],
        0
    );
    assert_eq!(
        owner
            .detached_logical_region_mut(1, second_address, 64)
            .unwrap()[0],
        0
    );
    owner.release_detached().unwrap();
    assert_eq!(drops.get(), 2);
}

#[test]
fn detached_compaction_supports_every_slot_admitted_by_the_owner() {
    const SLOTS: usize = 33;

    let drops = Rc::new(Cell::new(0));
    let mut owner = retained_owner_with_slots::<SLOTS>();
    owner.begin().unwrap();
    let mut last_address = 0;
    for _ in 0..SLOTS {
        let backing = owner.push_backing(TestBacking::new(drops.clone())).unwrap();
        let address = owner
            .reserved_backing_mut(&backing)
            .unwrap()
            .bytes
            .as_ptr()
            .addr();
        owner
            .commit_backing_descriptor(&backing, address, 64, 32)
            .unwrap();
        last_address = address;
    }
    owner.publish_retained_chain(SLOTS).unwrap().commit(|_| {});
    owner.mark_completed().unwrap();
    owner
        .mark_detached(MacTxQueueDetached::new_model(DESCRIPTOR_BASE))
        .unwrap();

    owner.compact_active_backings(&[32]).unwrap();
    assert_eq!(
        owner
            .detached_logical_region_mut(0, last_address, 64)
            .unwrap()
            .len(),
        64
    );
    owner.release_detached().unwrap();
    assert_eq!(drops.get(), SLOTS);
}

#[test]
fn full_retention_arena_reports_count_without_mutating_the_owner() {
    let drops = Rc::new(Cell::new(0));
    let mut owner = retained_owner();
    owner.begin().unwrap();
    owner.push_backing(TestBacking::new(drops.clone())).unwrap();
    owner.push_backing(TestBacking::new(drops.clone())).unwrap();

    assert!(matches!(
        owner.push_backing(TestBacking::new(drops.clone())),
        Err(AmpduDmaStorageError::Count)
    ));
    assert_eq!(owner.held_backing_count(), 2);
    assert_eq!(drops.get(), 1);
    owner.cancel().unwrap();
    assert_eq!(drops.get(), 3);
}

#[test]
fn retained_publication_fails_closed_when_descriptor_image_is_missing() {
    let drops = Rc::new(Cell::new(0));
    let mut owner = retained_owner();
    owner.begin().unwrap();
    let first = owner.push_backing(TestBacking::new(drops.clone())).unwrap();
    let second = owner.push_backing(TestBacking::new(drops.clone())).unwrap();
    let first_address = owner
        .reserved_backing_mut(&first)
        .unwrap()
        .bytes
        .as_ptr()
        .addr();
    let second_address = owner
        .reserved_backing_mut(&second)
        .unwrap()
        .bytes
        .as_ptr()
        .addr();
    owner
        .commit_backing_descriptor(&first, first_address, 64, 32)
        .unwrap();
    owner
        .commit_backing_descriptor(&second, second_address, 64, 32)
        .unwrap();

    // Model corrupted retained metadata directly. The safe publication
    // boundary must reject it before mutating even an earlier valid
    // descriptor or handing the arena to hardware.
    owner.retention_mut().backing_descriptors[usize::from(second.index)] = None;
    assert!(matches!(
        owner.publish_retained_chain(2),
        Err(AmpduDmaStorageError::StaleBacking)
    ));
    assert_eq!(owner.state(), AmpduDmaState::Reserved);
    assert_eq!(owner.dma().descriptor_word0(0), Some(0));
    assert_eq!(owner.dma().descriptor_buffer_address(0), Some(0));
    owner.cancel().unwrap();
    assert_eq!(drops.get(), 2);
}

#[test]
fn dropping_hardware_owned_external_chain_forgets_backings_without_unwinding() {
    let drops = Rc::new(Cell::new(0));
    let mut owner = retained_owner();
    owner.begin().unwrap();
    let backing = owner.push_backing(TestBacking::new(drops.clone())).unwrap();
    let address = owner
        .reserved_backing_mut(&backing)
        .unwrap()
        .bytes
        .as_ptr()
        .addr();
    let entry = backing.external_descriptor(address, 64, 32).unwrap();
    owner
        .publish_external_chain(&[entry])
        .unwrap()
        .commit(|_| {});

    drop(owner);
    assert_eq!(drops.get(), 0);
}

#[test]
fn dropping_reserved_external_chain_releases_backings() {
    let drops = Rc::new(Cell::new(0));
    let mut owner = retained_owner();
    owner.begin().unwrap();
    owner.push_backing(TestBacking::new(drops.clone())).unwrap();

    drop(owner);
    assert_eq!(drops.get(), 1);
}

#[test]
fn reserved_backing_insert_can_be_rolled_back_transactionally() {
    let drops = Rc::new(Cell::new(0));
    let mut owner = retained_owner();
    owner.begin().unwrap();
    let first = owner.push_backing(TestBacking::new(drops.clone())).unwrap();
    let second = owner.push_backing(TestBacking::new(drops.clone())).unwrap();

    assert!(owner.pop_last_backing(first).is_err());
    assert_eq!(owner.held_backing_count(), 2);
    drop(owner.pop_last_backing(second).unwrap());
    assert_eq!(owner.held_backing_count(), 1);
    assert_eq!(drops.get(), 1);

    owner.cancel().unwrap();
    assert_eq!(drops.get(), 2);
}

#[test]
fn stale_external_chain_is_not_partially_published_after_slot_reuse() {
    let drops = Rc::new(Cell::new(0));
    let mut owner = retained_owner();
    owner.begin().unwrap();
    let backing = owner.push_backing(TestBacking::new(drops.clone())).unwrap();
    let address = owner
        .reserved_backing_mut(&backing)
        .unwrap()
        .bytes
        .as_ptr()
        .addr();
    let stale = backing.external_descriptor(address, 64, 32).unwrap();
    owner.cancel().unwrap();
    owner.begin().unwrap();
    let _replacement = owner.push_backing(TestBacking::new(drops.clone())).unwrap();

    assert!(matches!(
        owner.publish_external_chain(&[stale]),
        Err(AmpduDmaStorageError::StaleBacking)
    ));
    assert_eq!(owner.state(), AmpduDmaState::Reserved);
    assert_eq!(owner.held_backing_count(), 1);
    assert_eq!(owner.dma().descriptor_word0(0), Some(0));
    assert_eq!(owner.dma().descriptor_buffer_address(0), Some(0));
    assert_eq!(drops.get(), 1);

    owner.cancel().unwrap();
    assert_eq!(drops.get(), 2);
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
    let zeroed = oer_memory::zeroed::zeroed::<AmpduDmaStorage<4, 32>>();
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
        let backing = owner.push_backing(TestBacking::new(drops.clone())).unwrap();
        let address = owner
            .reserved_backing_mut(&backing)
            .unwrap()
            .bytes
            .as_ptr()
            .addr();
        let entry = backing.external_descriptor(address, 64, 32).unwrap();
        owner
            .publish_external_chain(&[entry])
            .unwrap()
            .commit(|_| {});
        assert_eq!(owner.state(), AmpduDmaState::HardwareOwned);
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
        let lease =
            oer_memory::TaggedStableDmaBacking::new(&mut tag, TestBacking::new(drops.clone()));
        let (backing, region) = owner.push_backing_region(lease).unwrap();
        region[0] = 0x5d;
        dma_address = region.as_ptr();
        let descriptor = backing
            .external_descriptor(dma_address.addr(), 64, 32)
            .unwrap();
        owner
            .publish_external_chain(&[descriptor])
            .unwrap()
            .commit(|_| {});
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
