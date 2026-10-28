use bytemuck::Zeroable;
use std::boxed::Box;

use super::*;

fn table() -> LeDeviceTable {
    LeDeviceTable::place::<crate::bluetooth::engine::HostDma<0x2f07_e508>>(
        crate::memory::dma_static::Placed::host(Box::leak(Box::new(LeDeviceTableStorage::zeroed()))),
    )
}

const PUBLIC: LeFilterAcceptListDevice = LeFilterAcceptListDevice {
    random: false,
    address: [0x66, 0x55, 0x44, 0x33, 0x22, 0x11],
};
const RANDOM_DEVICE: LeFilterAcceptListDevice = LeFilterAcceptListDevice {
    random: true,
    address: [0xa6, 0xa5, 0xa4, 0xa3, 0xa2, 0xc1],
};

fn entry(table: &LeDeviceTable, index: usize) -> DeviceEntryImage {
    table.read(index)
}

#[test]
fn published_entries_retain_device_identity_and_address_type() {
    let mut table = table();
    table.add(PUBLIC).unwrap();
    table.add(RANDOM_DEVICE).unwrap();
    // The images the vendor table held for the same devices on the stand.
    assert_eq!(entry(&table, 0), PUBLIC.image());
    assert!(entry(&table, 0).flags_and_address.valid());
    assert!(entry(&table, 0).flags_and_address.accept_list_member());
    assert!(!entry(&table, 0).flags_and_address.periodic_list_member());
    assert!(!entry(&table, 0).flags_and_address.random());
    assert_eq!(entry(&table, 1), RANDOM_DEVICE.image());
    assert!(entry(&table, 1).flags_and_address.random());
    assert!(!entry(&table, 1).flags_and_address.periodic_list_member());
    let publication = table.publication();
    assert_eq!(publication.first_entry().bits(), 0x2f07_e508);
    assert_eq!(publication.count(), 2);
}

#[test]
fn a_device_is_added_once_and_the_list_is_bounded() {
    let mut table = table();
    table.add(PUBLIC).unwrap();
    table.add(PUBLIC).unwrap();
    assert_eq!(table.len(), 1);
    for index in 1..BLUETOOTH_FILTER_ACCEPT_LIST_CAPACITY {
        let mut device = RANDOM_DEVICE;
        device.address[0] = index as u8;
        table.add(device).unwrap();
    }
    assert_eq!(table.add(RANDOM_DEVICE), Err(LeDeviceTableError::Full));
    assert_eq!(
        table.publication().count(),
        BLUETOOTH_FILTER_ACCEPT_LIST_CAPACITY as u8
    );
}

#[test]
fn removal_keeps_the_members_packed_and_clear_empties_the_table() {
    let mut table = table();
    table.add(PUBLIC).unwrap();
    table.add(RANDOM_DEVICE).unwrap();
    table.remove(PUBLIC).unwrap();
    assert_eq!(table.len(), 1);
    assert_eq!(entry(&table, 0), RANDOM_DEVICE.image());
    assert_eq!(entry(&table, 1), DeviceEntryImage::ZERO);
    assert_eq!(table.remove(PUBLIC), Err(LeDeviceTableError::NotFound));
    table.clear();
    assert!(table.is_empty());
    assert_eq!(entry(&table, 0), DeviceEntryImage::ZERO);
    assert_eq!(table.publication().count(), 0);
}

#[test]
fn publication_tracks_every_count_through_capacity_and_back_to_empty() {
    let mut table = table();
    let first_entry = table.publication().first_entry();
    for index in 0..BLUETOOTH_FILTER_ACCEPT_LIST_CAPACITY {
        let mut device = PUBLIC;
        device.address[0] = index as u8;
        table.add(device).unwrap();
        let publication = table.publication();
        assert_eq!(usize::from(publication.count()), index + 1);
        assert_eq!(publication.first_entry(), first_entry);
    }
    assert_eq!(table.add(RANDOM_DEVICE), Err(LeDeviceTableError::Full));
    assert_eq!(usize::from(table.publication().count()), table.len());

    for index in 0..BLUETOOTH_FILTER_ACCEPT_LIST_CAPACITY {
        let mut device = PUBLIC;
        device.address[0] = index as u8;
        table.remove(device).unwrap();
        let publication = table.publication();
        assert_eq!(usize::from(publication.count()), table.len());
        assert_eq!(publication.first_entry(), first_entry);
    }
    assert!(table.is_empty());
    assert_eq!(table.remove(PUBLIC), Err(LeDeviceTableError::NotFound));
    assert_eq!(table.publication().count(), 0);
}
