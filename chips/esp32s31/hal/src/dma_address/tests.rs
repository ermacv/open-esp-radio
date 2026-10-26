//! The typed tables and address lists of placed DMA storage.

use super::{PlacedAddresses, PlacedTable};
use crate::layouts::{WifiRxDmaDescriptor, WifiTxDmaDescriptor};

const BASE: u32 = crate::layouts::DMA_WINDOW.start + 0x400;

#[test]
fn a_descriptor_table_yields_each_element_twelve_bytes_apart() {
    let table = PlacedTable::<WifiRxDmaDescriptor>::for_host_model(BASE, 5).unwrap();
    assert_eq!(table.len(), 5);
    assert_eq!(table.base(), BASE);
    assert_eq!(table.first().bits(), BASE);
    for index in 0..5 {
        let expected = BASE + 12 * index as u32;
        assert_eq!(table.address(index), Some(expected));
        assert_eq!(
            table.element(index).map(|element| element.bits()),
            Some(expected)
        );
        assert_eq!(table.index_of(expected), Some(index));
    }
    assert_eq!(table.address(5), None);
    assert!(table.element(5).is_none());
    assert_eq!(table.index_of(BASE + 60), None);
    assert_eq!(table.index_of(BASE + 4), None);
    assert_eq!(table.index_of(BASE - 12), None);
}

#[test]
fn a_table_refuses_no_elements_a_zero_base_and_an_end_past_the_width() {
    assert!(PlacedTable::<WifiTxDmaDescriptor>::for_host_model(BASE, 0).is_none());
    assert!(PlacedTable::<WifiTxDmaDescriptor>::for_host_model(0, 1).is_none());
    assert!(PlacedTable::<WifiTxDmaDescriptor>::for_host_model(u32::MAX - 11, 1).is_none());
    assert!(PlacedTable::<WifiTxDmaDescriptor>::for_host_model(u32::MAX - 23, 1).is_some());
}

#[test]
fn recorded_addresses_read_back_typed_and_absent_ones_do_not() {
    let mut addresses = PlacedAddresses::<3>::new();
    assert!(addresses.get(0).is_none());
    #[allow(unsafe_code, reason = "a host model's synthetic address")]
    // SAFETY: the address is recorded and compared, never dereferenced.
    let address = unsafe { super::placed(BASE) };
    assert!(addresses.record(1, address));
    assert!(!addresses.record(3, address));
    assert_eq!(addresses.get(1), Some(address));
    assert!(addresses.get(0).is_none() && addresses.get(2).is_none() && addresses.get(3).is_none());
    assert_eq!(addresses.bits(), &[0, BASE, 0]);
    assert_eq!(
        addresses.iter().copied().collect::<std::vec::Vec<_>>(),
        [0, BASE, 0]
    );
}

fn allocation() -> (
    &'static mut ([WifiTxDmaDescriptor; 2], [u8; 16]),
    super::PlacementProjection,
) {
    let placed = crate::dma_static::Placed::host(std::boxed::Box::leak(std::boxed::Box::new((
        [const { WifiTxDmaDescriptor::zeroed() }; 2],
        [0; 16],
    ))));
    let (storage, _, projection) = placed
        .into_dma_parts::<crate::ieee80211::dma::space::HostDma>()
        .expect("host allocation has a DMA range");
    (storage, projection)
}

#[test]
fn projections_keep_each_field_in_the_consumed_allocation() {
    let (storage, projection) = allocation();
    let first = projection
        .link::<WifiTxDmaDescriptor>(&storage.0[0])
        .unwrap();
    let table = projection.table::<WifiTxDmaDescriptor>(&storage.0).unwrap();
    assert_eq!(table.first(), first);
    assert_eq!(table.element(1), projection.link(&storage.0[1]).ok());
    let slots = projection.slots(&storage.0).unwrap();
    assert_eq!(slots.slot(1), table.element(1).map(|link| link.address()));
    let region = projection.region(&storage.1).unwrap();
    assert_eq!(region.len(), 16);
    assert_eq!(region.at(15).unwrap().get(), region.start().get() + 15);
    assert!(region.at(16).is_none());
}

#[test]
fn projections_refuse_foreign_fields_empty_extents_and_noncontiguous_tables() {
    use super::ProjectionError::*;
    let (storage, projection) = allocation();
    let foreign = WifiTxDmaDescriptor::zeroed();
    assert_eq!(projection.link(&foreign), Err(OutsideAllocation));
    assert_eq!(projection.region(&[1; 32]), Err(OutsideAllocation));
    assert_eq!(projection.region(&[]), Err(Empty));
    assert_eq!(projection.slots::<[u8; 0]>(&[[]]), Err(Empty));
    assert_eq!(projection.table::<WifiTxDmaDescriptor>(&[]), Err(Empty));
    assert_eq!(
        projection.table([&storage.0[1], &storage.0[0]]),
        Err(NonContiguous)
    );
}

#[test]
fn projections_refuse_misaligned_dma_addresses_and_width_overflow() {
    for (base, error) in [
        (BASE + 1, super::ProjectionError::Misaligned),
        (u32::MAX - 3, super::ProjectionError::AddressWidth),
    ] {
        let placed = crate::dma_static::Placed::host(std::boxed::Box::leak(std::boxed::Box::new(
            WifiTxDmaDescriptor::zeroed(),
        )));
        let (storage, projection) = super::PlacementProjection::for_test(placed, base);
        assert_eq!(projection.link(storage), Err(error));
    }
}

#[test]
fn refusing_a_projection_returns_the_exact_unchanged_placement() {
    let storage = std::boxed::Box::leak(std::boxed::Box::new([7u32; 4]));
    let original = core::ptr::from_ref(storage).addr();
    let placed = crate::dma_static::Placed::host(storage);
    let result = super::PlacementProjection::try_with::<crate::ieee80211::dma::space::HostDma, _, _>(
        placed,
        |_, projection| projection.region(&[0; 128]),
    );
    let (error, returned) = result.err().expect("foreign region is refused");
    assert_eq!(error, super::ProjectionError::OutsideAllocation);
    assert_eq!(core::ptr::from_ref(returned.as_ref()).addr(), original);
    assert_eq!(*returned.into_inner(), [7; 4]);
}
