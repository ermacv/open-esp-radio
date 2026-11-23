//! The MAC RX descriptor walker's register operations.
//!
//! `RX_DESCRIPTOR_BASE` is set only from a typed descriptor reference,
//! `RX_DESCRIPTOR_HIGH_WINDOW` only to its placement's prefix, and the
//! append/reload request is the generated doorbell with its pending read.

use oer_chips_esp32s31_pac::layouts::WifiRxDmaDescriptor;
use oer_portable_memory::DmaRef;

use crate::radio::ieee80211::registers::{MacPlacement, WifiRxDmaRegisters};

/// Field-level observation of `RX_NEXT_DESCRIPTOR`.
///
/// Keeping the address and unknown hardware state separate preserves the
/// vendor's complete-word zero predicate without publishing a register image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacRxNextDescriptorObservation {
    address_low: u32,
    upper_opaque: u16,
}

impl MacRxNextDescriptorObservation {
    pub const fn address_low(self) -> u32 {
        self.address_low
    }

    pub const fn is_zero(self) -> bool {
        self.address_low == 0 && self.upper_opaque == 0
    }

    pub const fn has_unknown_upper_state(self) -> bool {
        self.upper_opaque != 0
    }
}

/// The opaque twenty-bit initialization image published to `0x0c68` before
/// the descriptor base. Its units and hardware function are not established.
///
/// SOURCE: complete pinned `libpp.a[hal_mac.o]::mac_rxbuf_init`.
const RX_C68_INIT_IMAGE: oer_chips_esp32s31_pac::Bits<20> =
    oer_chips_esp32s31_pac::Bits::constant::<0x000f_ffff>();

/// The opaque twenty-bit value `mac_rxbuf_init` publishes to `0x0c6c`.
///
/// SOURCE: complete pinned `libpp.a[hal_mac.o]::mac_rxbuf_init`.
const RX_C6C_INIT_IMAGE: oer_chips_esp32s31_pac::Bits<20> =
    oer_chips_esp32s31_pac::Bits::constant::<4>();

impl<M: MacPlacement> WifiRxDmaRegisters<M> {
    /// Initialize the RX walker configuration without publishing a descriptor.
    ///
    /// SOURCE: first four RMWs of complete pinned
    /// `libpp.a[hal_mac.o]::mac_rxbuf_init`. Its final descriptor-base store
    /// is deliberately excluded because the RX ring owner publishes it later.
    pub fn initialize_mac_rx_buffer_prefix(&mut self) {
        self.dma.modify_word_0c68_opaque(|w| {
            w.set_bits_0_19_opaque(RX_C68_INIT_IMAGE);
        });
        self.dma.modify_word_0c6c_opaque(|w| {
            w.set_bits_0_19_opaque(RX_C6C_INIT_IMAGE);
        });
        self.dma.modify_rx_descriptor_high_window(|w| {
            w.set_address_high_prefix();
        });
        self.dma.modify_word_007c_opaque(|w| {
            w.set_rx_control_buffer_offset(0);
        });
    }

    pub fn mac_rx_last_descriptor_low(&self) -> u32 {
        self.dma.read_rx_last_descriptor().address_low().get()
    }

    pub fn mac_rx_next_descriptor(&self) -> MacRxNextDescriptorObservation {
        let observation = self.dma.read_rx_next_descriptor();
        MacRxNextDescriptorObservation {
            address_low: observation.address_low().get(),
            upper_opaque: crate::radio::ieee80211::registers::halfword(
                observation.bits_20_31_opaque(),
            ),
        }
    }

    /// Whether the append/reload doorbell is still pending: the hardware
    /// clears it when it accepts the request.
    pub fn mac_rx_reload_pending(&self) -> bool {
        self.dma.rx_control_append_descriptor_reload_pending()
    }

    /// Select the internal-SRAM address window the descriptors lie in: the
    /// DMA window the link places them in, the field's placement prefix.
    pub fn configure_mac_rx_descriptor_window(&mut self) {
        self.dma.modify_rx_descriptor_high_window(|w| {
            w.set_address_high_prefix();
        });
    }

    /// Publish a descriptor head: a placed descriptor, so inside the DMA
    /// window by the link's proof.
    pub(crate) fn write_mac_rx_descriptor_base(&mut self, head: DmaRef<WifiRxDmaDescriptor>) {
        self.dma.modify_rx_descriptor_base(|w| {
            w.set_address(head);
        });
    }

    /// Start the walker.
    pub fn publish_mac_rx_walker_enable(&mut self) {
        self.set_walker(oer_chips_esp32s31_pac::wifi_mac::rx_control::WalkerEnable::Enabled);
    }

    /// Ask the live walker to follow newly published descriptor links: ring
    /// the append/reload doorbell. Its completion is
    /// [`Self::mac_rx_reload_pending`] reading false.
    pub fn request_mac_rx_descriptor_reload(&mut self) {
        self.dma.trigger_rx_control_append_descriptor_reload();
    }

    /// Enable the hardware-beacon selection for RX-buffer reload, the edge
    /// complete `hal_init` takes on the walker's control word.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac.o]::hal_init`, offsets
    /// `0xcc..0x12a`.
    pub(crate) fn enable_mac_rx_hw_beacon_reload(&mut self) {
        self.dma.modify_rx_control(|w| {
            w.set_rxbuf_reload_use_hw_beacon_enable(true);
        });
    }

    /// Clear the walker enable without ordering it: the last direct edge of
    /// `mac_txrx_init`.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac.o]::mac_txrx_init`, offsets
    /// `0xee..0x16e`.
    pub(crate) fn clear_mac_rx_walker_enable(&mut self) {
        self.set_walker(oer_chips_esp32s31_pac::wifi_mac::rx_control::WalkerEnable::Disabled);
    }

    fn set_walker(&mut self, walker: oer_chips_esp32s31_pac::wifi_mac::rx_control::WalkerEnable) {
        self.dma.modify_rx_control(|w| {
            w.set_walker_enable(walker);
        });
    }
}

#[cfg(test)]
mod tests;
