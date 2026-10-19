extern crate std;

use super::*;
fn free<const N: usize>(owner: &mut PinnedTxDmaStorage<N>) -> TxDmaFree<'_, N> {
    match owner.access() {
        TxDmaAccess::Free(free) => free,
        _ => panic!("fixture owns free DMA"),
    }
}

use crate::ieee80211::dma::space::{HOST_ARENA_BASE, HostDma};

/// The host addresses of the storage's descriptor and buffer.
const DESCRIPTOR_ADDRESS: u32 = HOST_ARENA_BASE;
const BUFFER_ADDRESS: u32 =
    HOST_ARENA_BASE + core::mem::offset_of!(TxDmaStorage<64>, buffer) as u32;

fn storage() -> PinnedTxDmaStorage<64> {
    TxDmaStorage::pin::<HostDma>(crate::dma_static::Placed::host(std::boxed::Box::leak(
        std::boxed::Box::new(TxDmaStorage::new()),
    )))
    .unwrap()
}

fn prepared_head(authority: &dyn PreparedTxDma) -> u32 {
    authority.descriptor_head()
}

fn hardware_owned_head(authority: &dyn HardwareOwnedTxDma) -> u32 {
    authority.descriptor_head()
}

#[test]
fn publication_records_hardware_owner_before_start_token() {
    let mut storage = storage();
    free(&mut storage).into_buffer()[..4].copy_from_slice(&[1, 2, 3, 4]);
    free(&mut storage).reserve(64, 4).unwrap();

    let mut start_address = 0;
    let publication = storage.publication().unwrap();
    assert_eq!(prepared_head(&publication), DESCRIPTOR_ADDRESS);
    publication.commit(|start| {
        start_address = hardware_owned_head(start);
    });

    assert_eq!(start_address, DESCRIPTOR_ADDRESS);
    assert_eq!(storage.state(), TxDmaState::HardwareOwned);
    assert_eq!(storage.binding().buffer_address(), BUFFER_ADDRESS);
    assert!(storage.binding().admits_descriptor(DESCRIPTOR_ADDRESS));
    assert!(storage.binding().admits_buffer(BUFFER_ADDRESS + 8, 16));
    assert!(!storage.binding().admits_buffer(BUFFER_ADDRESS + 63, 2));
    assert!(matches!(storage.access(), TxDmaAccess::HardwareOwned));
    storage
        .release_aborted(MacTxQueueDetached::new_model(DESCRIPTOR_ADDRESS))
        .unwrap();
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
    let zeroed = oer_memory::zeroed::zeroed::<TxDmaStorage<64>>();
    let new = TxDmaStorage::<64>::new();
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
    assert_eq!(zeroed.buffer.0, new.buffer.0);
}
