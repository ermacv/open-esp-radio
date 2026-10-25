//! Nonzero compressed links for reviewed controller-SRAM graph fields.

#![forbid(unsafe_code)]

use crate::layouts::ble_word_index;

/// Why an address cannot represent a reviewed bound controller-SRAM link.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ControllerSramLinkAddressError {
    /// The address is not a four-byte-aligned `0x2f`-prefixed SRAM address
    /// with a nonzero word index: the generated BLE codec has no image for
    /// it. The window base would compress to zero, which represents an
    /// unbound link in reviewed header, link-state and scheduler-item
    /// contexts.
    NotEncodable,
}

/// A nonzero compressed link for reviewed bound controller-SRAM contexts.
///
/// The address and its word index come from the generated BLE codec
/// ([`ble_word_index`]), so `Some(address)` never collapses onto the zero
/// image used for `None`. It remains a forgeable geometry value and is not a
/// backing-storage or publication token.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ControllerSramLinkAddress {
    address: u32,
    image: u32,
}

impl ControllerSramLinkAddress {
    pub(crate) fn from_placed(address: crate::layouts::DmaWindowAddress) -> Self {
        Self::new(address.get())
            .expect("the placed layout has an encodable nonzero aligned address")
    }

    /// Validate an aligned address and reject the zero compressed image.
    pub const fn new(address: u32) -> Result<Self, ControllerSramLinkAddressError> {
        match ble_word_index(address) {
            Some(image) => Ok(Self { address, image }),
            None => Err(ControllerSramLinkAddressError::NotEncodable),
        }
    }

    /// Return the exact nonzero low-twenty-bit link image.
    pub const fn compressed_image(self) -> u32 {
        self.image
    }

    /// Return the CPU address behind this bound link.
    ///
    /// The value remains address geometry only. It grants neither CPU access
    /// to the backing allocation nor scheduler publication authority.
    pub const fn controller_address(self) -> u32 {
        self.address
    }
}
