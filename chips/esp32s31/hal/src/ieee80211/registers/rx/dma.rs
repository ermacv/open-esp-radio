//! The MAC RX descriptor walker's register operations.
//!
//! `RX_DESCRIPTOR_BASE` is set only from a typed descriptor reference,
//! `RX_DESCRIPTOR_HIGH_WINDOW` only to its placement's prefix, and the
//! append/reload request is the generated doorbell with its pending read.

use oer_esp32s31_pac::layouts::WifiRxDmaDescriptor;
use oer_mmio::DmaRef;

use crate::ieee80211::registers::{MacPlacement, WifiRxDmaRegisters, bits};

/// Read-only hardware frontier of the MAC RX descriptor walker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacRxDmaSnapshot {
    pub walker_enabled: bool,
    pub reload_pending: bool,
    pub descriptor_base: u32,
    pub next_descriptor_low: u32,
    /// True when the unassigned upper `RX_NEXT_DESCRIPTOR` field is nonzero.
    pub next_descriptor_upper_unknown: bool,
    pub last_descriptor_low: u32,
}

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

/// The receive-buffer geometry `mac_rxbuf_init` publishes before its
/// descriptor base: the opaque twenty-bit limit of `0x0c68`.
///
/// SOURCE: complete pinned `libpp.a[hal_mac.o]::mac_rxbuf_init`.
const RX_BUFFER_LIMIT: u32 = 0x000f_ffff;

/// The opaque twenty-bit value `mac_rxbuf_init` publishes to `0x0c6c`.
///
/// SOURCE: complete pinned `libpp.a[hal_mac.o]::mac_rxbuf_init`.
const RX_BUFFER_UNIT: u32 = 4;

impl<M: MacPlacement> WifiRxDmaRegisters<M> {
    /// Snapshot the complete published RX walker frontier without changing it.
    pub fn mac_rx_dma_snapshot(&self) -> MacRxDmaSnapshot {
        let control = self.dma.read_rx_control();
        let next_descriptor = self.mac_rx_next_descriptor();
        MacRxDmaSnapshot {
            walker_enabled: control.walker_enable(),
            reload_pending: control.append_descriptor_reload(),
            descriptor_base: self.dma.read_rx_descriptor_base().address(),
            next_descriptor_low: next_descriptor.address_low(),
            next_descriptor_upper_unknown: next_descriptor.has_unknown_upper_state(),
            last_descriptor_low: self.mac_rx_last_descriptor_low(),
        }
    }

    /// Initialize the RX buffer geometry without publishing a descriptor.
    ///
    /// SOURCE: first four RMWs of complete pinned
    /// `libpp.a[hal_mac.o]::mac_rxbuf_init`. Its final descriptor-base store
    /// is deliberately excluded because the RX ring owner publishes it later.
    pub fn initialize_mac_rx_buffer_prefix(&mut self) {
        self.dma.modify_word_0c68_opaque(|w| {
            w.set_bits_0_19_opaque(bits(RX_BUFFER_LIMIT));
        });
        self.dma.modify_word_0c6c_opaque(|w| {
            w.set_bits_0_19_opaque(bits(RX_BUFFER_UNIT));
        });
        self.dma.modify_rx_descriptor_high_window(|w| {
            w.set_address_high_prefix();
        });
        self.dma.modify_word_007c_opaque(|w| {
            w.set_bits_0_7_opaque(0);
        });
    }

    pub fn mac_rx_last_descriptor_low(&self) -> u32 {
        self.dma.read_rx_last_descriptor().address_low().get()
    }

    pub fn mac_rx_next_descriptor(&self) -> MacRxNextDescriptorObservation {
        let observation = self.dma.read_rx_next_descriptor();
        MacRxNextDescriptorObservation {
            address_low: observation.address_low().get(),
            upper_opaque: crate::ieee80211::registers::halfword(observation.bits_20_31_opaque()),
        }
    }

    pub fn mac_rx_walker_enabled(&self) -> bool {
        self.dma.read_rx_control().walker_enable()
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
    pub fn write_mac_rx_descriptor_base(&mut self, head: DmaRef<WifiRxDmaDescriptor>) {
        self.dma.modify_rx_descriptor_base(|w| {
            w.set_address(head);
        });
    }

    /// Start the walker.
    pub fn publish_mac_rx_walker_enable(&mut self) {
        self.set_walker_enabled(true);
    }

    /// Ask the live walker to follow newly published descriptor links: ring
    /// the append/reload doorbell. Its completion is
    /// [`Self::mac_rx_reload_pending`] reading false.
    pub fn request_mac_rx_descriptor_reload(&mut self) {
        self.dma.trigger_rx_control_append_descriptor_reload();
    }

    /// Start and confirm the walker.
    pub fn try_enable_mac_rx_walker(&mut self) -> bool {
        if self.mac_rx_walker_enabled() {
            return false;
        }
        self.set_walker_enabled(true);
        oer_riscv::fence();
        self.mac_rx_walker_enabled()
    }

    pub fn try_disable_mac_rx_walker(&mut self) -> bool {
        self.set_walker_enabled(false);
        oer_riscv::fence();
        !self.mac_rx_walker_enabled()
    }

    /// Set `RX_CONTROL` bit 27, the edge complete `hal_init` takes on the
    /// walker's control word.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac.o]::hal_init`, offsets
    /// `0xcc..0x12a`.
    pub(crate) fn set_mac_rx_control_bit_27(&mut self) {
        self.dma.modify_rx_control(|w| {
            w.set_bit_27_opaque(true);
        });
    }

    /// Clear the walker enable without ordering it: the last direct edge of
    /// `mac_txrx_init`.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac.o]::mac_txrx_init`, offsets
    /// `0xee..0x16e`.
    pub(crate) fn clear_mac_rx_walker_enable(&mut self) {
        self.set_walker_enabled(false);
    }

    fn set_walker_enabled(&mut self, enabled: bool) {
        self.dma.modify_rx_control(|w| {
            w.set_walker_enable(enabled);
        });
    }
}

#[cfg(test)]
mod tests;
