use super::{
    BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
    BluetoothMemoryListPointerImage,
};

#[test]
fn compressed_controller_address_rejects_unrepresentable_values() {
    assert_eq!(
        BluetoothControllerSramAddress::new(0x2f00_0001),
        Err(BluetoothControllerSramAddressError::Unaligned)
    );
    assert_eq!(
        BluetoothControllerSramAddress::new(0x2e00_0000),
        Err(BluetoothControllerSramAddressError::OutsideEncodableWindow)
    );
    assert_eq!(
        BluetoothControllerSramAddress::new(0x2f40_0000),
        Err(BluetoothControllerSramAddressError::OutsideEncodableWindow)
    );

    let first =
        BluetoothControllerSramAddress::new(0x2f00_0000).expect("window base is representable");
    let last =
        BluetoothControllerSramAddress::new(0x2f3f_fffc).expect("window end is representable");
    assert_eq!(first.address(), 0x2f00_0000);
    assert_eq!(first.compressed_image(), 0);
    assert_eq!(last.compressed_image(), 0x000f_ffff);
    assert_eq!(BluetoothMemoryListPointerImage::Zero.compressed(), 0);
    assert_eq!(
        BluetoothMemoryListPointerImage::Address(last).compressed(),
        0x000f_ffff
    );
}
