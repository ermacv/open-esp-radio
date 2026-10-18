//! Protocol-defined Bluetooth LE values retained by private SRAM codecs.

#![forbid(unsafe_code)]

/// One protocol-level Bluetooth LE access address in controller bit order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct LeAccessAddress(u32);

impl LeAccessAddress {
    /// Bluetooth Core Direct Test Mode synchronization word.
    pub(super) const DIRECT_TEST_MODE: Self = Self(0x7176_4129);
    /// Bluetooth Core primary advertising Access Address.
    pub(super) const PRIMARY_ADVERTISING: Self = Self(0x8e89_bed6);

    pub(super) const fn from_controller_image(image: u32) -> Self {
        Self(image)
    }

    pub(super) const fn controller_image(self) -> u32 {
        self.0
    }
}

/// One protocol-level Bluetooth LE CRC initialization value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct LeCrcInit(u32);

impl LeCrcInit {
    const CONTROLLER_IMAGE_MASK: u32 = 0x00ff_ffff;

    /// CRC preset shared by DTM and non-periodic primary advertising.
    pub(super) const LE_PRESET: Self = Self(0x0055_5555);

    pub(super) const fn from_controller_word(word: u32) -> Self {
        Self(word & Self::CONTROLLER_IMAGE_MASK)
    }

    pub(super) const fn apply_to_controller_word(self, word: u32) -> u32 {
        (word & !Self::CONTROLLER_IMAGE_MASK) | self.0
    }
}
