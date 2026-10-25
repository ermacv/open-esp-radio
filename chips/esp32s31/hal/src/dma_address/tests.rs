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
