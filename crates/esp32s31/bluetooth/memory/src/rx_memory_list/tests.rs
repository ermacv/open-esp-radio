use oer_esp32s31_hal::types::BluetoothMemoryListSelector;

use super::RxMemoryListClass;

#[test]
fn scan_and_non_scan_classes_map_to_the_two_active_selectors() {
    assert_eq!(
        RxMemoryListClass::Scanning.selector(),
        BluetoothMemoryListSelector::One
    );
    assert_eq!(
        RxMemoryListClass::NonScanning.selector(),
        BluetoothMemoryListSelector::Two
    );
}
