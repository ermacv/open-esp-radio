use std::boxed::Box;

use super::*;

#[test]
fn a_null_compressed_base_is_rejected_before_a_table_can_be_published() {
    let storage = std::boxed::Box::leak(std::boxed::Box::new(LeDeviceTableStorage::new()));
    assert!(matches!(
        LeDeviceTable::bind_model(storage, BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW),
        Err(LeDeviceTableBindError::ZeroCompressedImage)
    ));
}

fn table() -> LeDeviceTable {
    LeDeviceTable::bind_model(
        Box::leak(Box::new(LeDeviceTableStorage::new())),
        0x2f07_e508,
    )
    .unwrap()
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
    assert!(!entry(&table, 0).flags_and_address.random());
    assert_eq!(entry(&table, 1), RANDOM_DEVICE.image());
    assert!(entry(&table, 1).flags_and_address.random());
    let publication = table.publication();
    assert_eq!(publication.first_entry().bits(), 0x2f07_e508);
    assert_eq!(publication.count().get(), 2);
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
        table.publication().count().get(),
        BLUETOOTH_FILTER_ACCEPT_LIST_CAPACITY as u32
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
    assert_eq!(table.publication().count().get(), 0);
}

#[test]
fn storage_outside_controller_sram_is_refused() {
    assert_eq!(
        LeDeviceTable::bind_model(
            Box::leak(Box::new(LeDeviceTableStorage::new())),
            0x2f07_fff0
        )
        .err(),
        Some(LeDeviceTableBindError::ExtentOutsidePhysicalSram)
    );
}
