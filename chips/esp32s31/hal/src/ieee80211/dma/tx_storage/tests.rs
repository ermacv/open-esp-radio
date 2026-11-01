extern crate std;

use crate::layouts::{DmaRef, WifiTxDmaDescriptor};
use crate::memory::dma_static::{PlacedTxLease, PlacedTxPool};
use oer_memory::{PinnedDmaTxPool, PinnedDmaTxRadioLease};

use super::*;

type Lease = PlacedTxLease<PinnedDmaTxRadioLease<'static, 64, 0, 0>>;
type Pool = PlacedTxPool<64, 0, 0, 2>;

fn free(owner: &mut PinnedTxDmaStorage<Lease>) -> TxDmaFree<'_, Lease> {
    match owner.access() {
        TxDmaAccess::Free(free) => free,
        _ => panic!("fixture owns free DMA"),
    }
}

use crate::ieee80211::dma::space::{HOST_ARENA_BASE, HostDma};

/// The host address of the storage's descriptor.
const DESCRIPTOR_ADDRESS: u32 = HOST_ARENA_BASE;

/// A host-placed pool of two 64-byte buffers.
fn pool() -> &'static Pool {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(PinnedDmaTxPool::new()));
    let pool = PlacedTxPool::pin(crate::memory::dma_static::Placed::host(storage))
        .unwrap_or_else(|_| panic!("host pool geometry"));
    std::boxed::Box::leak(std::boxed::Box::new(pool))
}

/// Buffer `index` of `pool`, as the radio holds it.
fn lease(pool: &'static Pool, index: u8) -> Lease {
    pool.claim_network(index)
        .unwrap()
        .publish(oer_memory::FrameLen::constant::<0>(), |_| {});
    pool.claim_radio(index).unwrap()
}

/// A descriptor without a buffer.
fn bare() -> PinnedTxDmaStorage<Lease> {
    TxDmaStorage::pin::<HostDma, Lease>(crate::memory::dma_static::Placed::host(
        std::boxed::Box::leak(std::boxed::Box::new(TxDmaStorage::new())),
    ))
    .unwrap()
}

/// A descriptor with buffer 0 of a fresh pool attached.
fn storage() -> PinnedTxDmaStorage<Lease> {
    let mut storage = bare();
    assert!(storage.attach(lease(pool(), 0)).is_ok());
    storage
}

fn prepared_head(authority: &dyn PreparedTxDma<DmaRef<WifiTxDmaDescriptor>>) -> u32 {
    authority.descriptor_head().bits()
}

fn hardware_owned_head(authority: &dyn HardwareOwnedTxDma<DmaRef<WifiTxDmaDescriptor>>) -> u32 {
    authority.descriptor_head().bits()
}

#[test]
fn publication_records_hardware_owner_before_start_token() {
    let mut storage = storage();
    free(&mut storage).into_buffer().unwrap()[..4].copy_from_slice(&[1, 2, 3, 4]);
    free(&mut storage).reserve(64, 4).unwrap();

    let mut start_address = 0;
    let publication = storage.publication().unwrap();
    assert_eq!(prepared_head(&publication), DESCRIPTOR_ADDRESS);
    publication.commit(|start| {
        start_address = hardware_owned_head(start);
    });

    assert_eq!(start_address, DESCRIPTOR_ADDRESS);
    assert_eq!(storage.state(), TxDmaState::HardwareOwned);
    assert!(storage.binding().admits_descriptor(DESCRIPTOR_ADDRESS));
    assert!(matches!(storage.access(), TxDmaAccess::HardwareOwned));
    storage
        .release_aborted(MacTxQueueDetached::new_model(DESCRIPTOR_ADDRESS))
        .unwrap();
}

#[test]
fn the_descriptor_publishes_the_attached_buffer() {
    let pool = pool();
    let mut storage = bare();
    let mut buffer = lease(pool, 1);
    let address = buffer.dma_region().unwrap().start().get();
    assert!(storage.attach(buffer).is_ok());
    free(&mut storage).reserve(64, 8).unwrap();
    let published = storage
        .storage
        .as_ref()
        .get_ref()
        .descriptor
        .buffer_address();
    assert_eq!(published, address);
    storage.cancel_reservation().unwrap();
    buffer = storage.detach().expect("free again");
    assert_eq!(buffer.dma_region().unwrap().start().get(), address);
}

#[test]
fn a_bare_descriptor_lends_no_buffer_and_reserves_nothing() {
    let mut storage = bare();
    assert!(!storage.attached());
    assert_eq!(
        free(&mut storage).buffer_mut().err(),
        Some(TxDmaStorageError::Backing)
    );
    assert_eq!(
        free(&mut storage).reserve(64, 8),
        Err(TxDmaStorageError::Backing)
    );
    assert_eq!(storage.state(), TxDmaState::Free);
    assert!(storage.detach().is_none());
}

#[test]
fn a_reservation_cannot_exceed_the_attached_buffer() {
    let mut storage = storage();
    assert_eq!(
        free(&mut storage).reserve(65, 8),
        Err(TxDmaStorageError::InvalidLength)
    );
    assert_eq!(storage.state(), TxDmaState::Free);
}

#[test]
fn a_buffer_attaches_only_to_a_free_empty_descriptor_and_detaches_only_when_free() {
    let pool = pool();
    let mut storage = bare();
    assert!(storage.attach(lease(pool, 0)).is_ok());
    // A second buffer is refused unchanged.
    let second = lease(pool, 1);
    let region = second.dma_region();
    let refused = storage.attach(second).err().expect("one buffer");
    assert_eq!(refused.dma_region(), region);
    drop(refused);
    free(&mut storage).reserve(64, 8).unwrap();
    assert!(storage.detach().is_none(), "a reserved descriptor keeps it");
    storage.publication().unwrap().commit(|_| {});
    assert!(storage.detach().is_none(), "the hardware owns it");
    storage
        .release_aborted(MacTxQueueDetached::new_model(DESCRIPTOR_ADDRESS))
        .unwrap();
    assert!(storage.detach().is_some());
    assert!(!storage.attached());
}

#[test]
fn dropping_a_free_owner_returns_its_buffer_and_a_published_one_leaks_it() {
    let pool = pool();
    {
        let mut storage = bare();
        assert!(storage.attach(lease(pool, 0)).is_ok());
    }
    assert_eq!(pool.claimed_slots(), 0, "the free owner returned its lease");
    {
        let mut storage = bare();
        assert!(storage.attach(lease(pool, 1)).is_ok());
        free(&mut storage).reserve(64, 8).unwrap();
        storage.publication().unwrap().commit(|_| {});
    }
    assert_eq!(pool.claimed_slots(), 1, "the hardware may still read it");
}

#[test]
fn dropped_publication_remains_cancellable_software_state() {
    let mut storage = storage();
    free(&mut storage).reserve(32, 16).unwrap();
    {
        let _publication = storage.publication().unwrap();
    }

    assert_eq!(storage.state(), TxDmaState::Reserved);
    storage.cancel_reservation().unwrap();
    assert_eq!(storage.state(), TxDmaState::Free);
    assert_eq!(storage.descriptor_word0(), WifiTxDmaDescriptorControl::ZERO);
}

#[test]
fn completed_storage_is_not_reused_before_explicit_release() {
    let mut storage = storage();
    free(&mut storage).reserve(64, 8).unwrap();
    storage.publication().unwrap().commit(|_| {});
    storage.mark_completed().unwrap();

    assert_eq!(storage.state(), TxDmaState::Completed);
    assert!(!matches!(storage.access(), TxDmaAccess::Free(_)));
    storage
        .release_completed(MacTxQueueDetached::new_model(DESCRIPTOR_ADDRESS))
        .unwrap();
    assert_eq!(storage.state(), TxDmaState::Free);
}

#[test]
fn detach_proof_must_name_the_active_descriptor() {
    let mut storage = storage();
    free(&mut storage).reserve(64, 8).unwrap();
    storage.publication().unwrap().commit(|_| {});
    storage.mark_completed().unwrap();

    assert_eq!(
        storage.release_completed(MacTxQueueDetached::new_model(DESCRIPTOR_ADDRESS + 4)),
        Err(TxDmaStorageError::Address)
    );
    assert_eq!(storage.state(), TxDmaState::Completed);
    storage
        .release_completed(MacTxQueueDetached::new_model(DESCRIPTOR_ADDRESS))
        .unwrap();
}

#[test]
fn reset_required_storage_remains_quarantined() {
    let mut storage = storage();
    free(&mut storage).reserve(64, 8).unwrap();
    storage.publication().unwrap().commit(|_| {});
    storage.require_reset().unwrap();

    assert_eq!(storage.state(), TxDmaState::ResetRequired);
    assert!(!matches!(storage.access(), TxDmaAccess::Free(_)));
    assert_eq!(storage.mark_completed(), Err(TxDmaStorageError::State));
    assert_eq!(
        storage.release_aborted(MacTxQueueDetached::new_model(DESCRIPTOR_ADDRESS)),
        Err(TxDmaStorageError::State)
    );
    assert!(matches!(storage.access(), TxDmaAccess::ResetRequired));
    // Letting the movable capability leave scope must not unwind. The
    // permanently located backing remains quarantined until reset.
}

#[test]
fn impossible_sequence_can_only_quarantine_backing() {
    let mut storage = storage();
    storage.quarantine();

    assert_eq!(storage.state(), TxDmaState::ResetRequired);
    assert!(!matches!(storage.access(), TxDmaAccess::Free(_)));
    assert_eq!(storage.cancel_reservation(), Err(TxDmaStorageError::State));
    assert_eq!(
        storage.release_completed(MacTxQueueDetached::new_model(DESCRIPTOR_ADDRESS)),
        Err(TxDmaStorageError::State)
    );
}

#[test]
fn zeroed_storage_is_the_free_storage_new_builds() {
    let zeroed = oer_memory::zeroed::zeroed::<TxDmaStorage>();
    let new = TxDmaStorage::new();
    assert_eq!(zeroed.state, new.state);
    assert_eq!(zeroed.state, TxDmaState::Free);
    assert_eq!(
        (
            zeroed.descriptor.word0(),
            zeroed.descriptor.buffer_address(),
            zeroed.descriptor.next_address()
        ),
        (
            new.descriptor.word0(),
            new.descriptor.buffer_address(),
            new.descriptor.next_address()
        )
    );
}
