//! IEEE 802.11 MAC, baseband and channel hardware operations.

pub mod arena;

pub mod baseband;

pub mod channel;

pub mod client;

/// The Wi-Fi MAC DMA: descriptors, the RX walker, its ring and arena, the TX
/// and A-MPDU storage.
pub mod dma;

pub mod mac;

#[cfg(oer_probe)]
#[doc(hidden)]
#[cfg(oer_probe)]
pub mod phy_rate;

pub mod station_wake;

/// The Wi-Fi MAC registers borrowed by one HAL capability: from the runtime
/// owner itself, or from the owner published in a [`arena::RadioOwnerArena`].
pub(crate) enum MacBorrow<'registers> {
    Owned(&'registers mut oer_esp32s31_pac::WifiRadioRegisters),
    Published(core::cell::RefMut<'registers, oer_esp32s31_pac::WifiRadioRegisters>),
}

impl core::ops::Deref for MacBorrow<'_> {
    type Target = oer_esp32s31_pac::WifiRadioRegisters;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Owned(registers) => registers,
            Self::Published(registers) => registers,
        }
    }
}

impl core::ops::DerefMut for MacBorrow<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self {
            Self::Owned(registers) => registers,
            Self::Published(registers) => registers,
        }
    }
}
