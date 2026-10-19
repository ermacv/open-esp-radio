//! Nonzero compressed links for reviewed controller-SRAM graph fields.

#![forbid(unsafe_code)]

use oer_esp32s31_hal::types::{
    BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
};

/// First byte in the physical ESP32-S31 internal SRAM window.
pub const BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW: u32 = 0x2f00_0000;
/// First byte after the physical ESP32-S31 internal SRAM window.
///
/// This is deliberately narrower than the controller's 20-bit compressed
/// pointer encoding domain. Linker policy may reserve an additional suffix
/// below this architectural boundary.
pub const BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH: u32 = 0x2f08_0000;

/// Why an address cannot represent a reviewed bound controller-SRAM link.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerSramLinkAddressError {
    /// The positional address is outside the PAC compression domain.
    InvalidAddress(BluetoothControllerSramAddressError),
    /// The window base compresses to zero, which represents an unbound link in
    /// reviewed header, link-state and scheduler-item contexts.
    ZeroCompressedImage,
}

/// A nonzero compressed link for reviewed bound controller-SRAM contexts.
///
/// Unlike the positional PAC address, this semantic memory-layer value cannot
/// collapse `Some(address)` onto the zero image used for `None`. It remains a
/// forgeable geometry value and is not a backing-storage or publication token.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControllerSramLinkAddress(BluetoothControllerSramAddress);

impl ControllerSramLinkAddress {
    /// Validate an aligned address and reject the zero compressed image.
    pub const fn new(address: u32) -> Result<Self, ControllerSramLinkAddressError> {
        let address = match BluetoothControllerSramAddress::new(address) {
            Ok(address) => address,
            Err(error) => {
                return Err(ControllerSramLinkAddressError::InvalidAddress(error));
            }
        };
        if address.compressed_image() == 0 {
            return Err(ControllerSramLinkAddressError::ZeroCompressedImage);
        }
        Ok(Self(address))
    }

    /// Return the exact nonzero low-twenty-bit link image.
    pub const fn compressed_image(self) -> u32 {
        self.0.compressed_image()
    }

    /// Return the validated controller-SRAM identity behind this bound link.
    ///
    /// The value remains address geometry only. It grants neither CPU access
    /// to the backing allocation nor scheduler publication authority.
    pub const fn controller_address(self) -> BluetoothControllerSramAddress {
        self.0
    }
}
