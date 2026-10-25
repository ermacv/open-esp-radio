//! The MAC RX descriptor walker.

use oer_esp32s31_pac::layouts::{DmaRef, WifiRxDmaDescriptor};
use oer_mmio::Bits;

use crate::ieee80211::registers::{MacPlacement, WifiRxDmaRegisters, halfword};

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

/// The internal-SRAM high address window the descriptors lie in: the DMA
/// window the link places them in (`dma-window.x`).
///
/// SOURCE: BLOB_LIBPP_MAC_RXBUF_INIT_PREFIX (`mac_rxbuf_init`).
const DESCRIPTOR_HIGH_WINDOW: Bits<12> = Bits::constant::<0x02f0>();

impl<M: MacPlacement> WifiRxDmaRegisters<M> {
    fn set_walker_enabled(&mut self, enabled: bool) {
        self.dma.modify_rx_control(|w| {
            w.set_walker_enable(enabled);
        });
    }

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
            w.set_bits_0_19_opaque(Bits::constant::<0x000f_ffff>());
        });
        self.dma.modify_word_0c6c_opaque(|w| {
            w.set_bits_0_19_opaque(Bits::constant::<4>());
        });
        self.dma.modify_rx_descriptor_high_window(|w| {
            w.set_address_high(DESCRIPTOR_HIGH_WINDOW);
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
            upper_opaque: halfword(observation.bits_20_31_opaque()),
        }
    }

    pub fn mac_rx_walker_enabled(&self) -> bool {
        self.dma.read_rx_control().walker_enable()
    }

    /// Whether the append doorbell is still pending: the hardware clears it
    /// once it accepted the append.
    pub fn mac_rx_reload_pending(&self) -> bool {
        self.dma.read_rx_control().append_descriptor_reload()
    }

    /// Select the internal-SRAM address window the descriptors lie in: the
    /// DMA window the link places them in (`dma-window.x`).
    pub fn configure_mac_rx_descriptor_window(&mut self) {
        self.dma.modify_rx_descriptor_high_window(|w| {
            w.set_address_high(DESCRIPTOR_HIGH_WINDOW);
        });
    }

    /// Publish a descriptor head: a placed descriptor, so inside the DMA
    /// window by the link's proof.
    pub fn write_mac_rx_descriptor_base(&mut self, head: DmaRef<WifiRxDmaDescriptor>) {
        self.dma.write_rx_descriptor_base(head);
    }

    /// Start the walker.
    pub fn publish_mac_rx_walker_enable(&mut self) {
        self.set_walker_enabled(true);
    }

    /// Ask the live walker to follow newly published descriptor links.
    ///
    /// SOURCE: ROM_REV0_WDEV_APPEND_RX_BLOCKS (`wDev_AppendRxBlocks` ORs the
    /// doorbell into `RX_CONTROL`).
    pub fn request_mac_rx_descriptor_reload(&mut self) {
        self.dma.trigger_rx_control_append_descriptor_reload();
    }

    /// Start and confirm the walker.
    pub fn try_enable_mac_rx_walker(&mut self) -> bool {
        if self.dma.read_rx_control().walker_enable() {
            return false;
        }
        self.set_walker_enabled(true);
        oer_mmio::fence();
        self.dma.read_rx_control().walker_enable()
    }

    pub fn try_disable_mac_rx_walker(&mut self) -> bool {
        self.set_walker_enabled(false);
        oer_mmio::fence();
        !self.dma.read_rx_control().walker_enable()
    }

    /// Start or stop the walker, as a probe image replays the vendor's
    /// `hal_mac_rx_enable` and `hal_mac_rx_disable`.
    #[cfg(oer_probe)]
    pub(crate) fn validation_set_mac_rx_walker_enabled(&mut self, enabled: bool) {
        self.set_walker_enabled(enabled);
    }
}
