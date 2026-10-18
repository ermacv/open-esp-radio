//! Safe generated-PAC ownership for the MAC RX descriptor walker.

#![forbid(unsafe_code)]

use oer_memory::StableDmaRange;

use crate::layouts::{DmaRef, WifiRxDmaDescriptor};
use crate::{WifiRadioRegisters, device_fence, svd};

const RX_DESCRIPTOR_INTERNAL_SRAM_START: u32 = 0x2f00_0000;
const RX_DESCRIPTOR_INTERNAL_SRAM_END: u32 = 0x2f10_0000;

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

#[inline(always)]
pub(crate) fn set_walker_enabled(registers: &svd::WifiMacRxDma, enabled: bool) {
    registers.rx_control().modify(|_, writer| {
        if enabled {
            writer.walker_enable().set_bit()
        } else {
            writer.walker_enable().clear_bit()
        }
    });
}

#[inline(always)]
pub(crate) fn read_last_descriptor_low(registers: &svd::WifiMacRxDma) -> u32 {
    registers.rx_last_descriptor().read().address_low().bits()
}

#[inline(always)]
pub(crate) fn read_next_descriptor(
    registers: &svd::WifiMacRxDma,
) -> MacRxNextDescriptorObservation {
    let observation = registers.rx_next_descriptor().read();
    MacRxNextDescriptorObservation {
        address_low: observation.address_low().bits(),
        upper_opaque: observation.bits_20_31_opaque().bits(),
    }
}

#[inline(always)]
pub(crate) fn write_descriptor_base(
    registers: &svd::WifiMacRxDma,
    head: DmaRef<WifiRxDmaDescriptor>,
) {
    crate::generated::rx_descriptor_base(registers, head);
}

#[inline(always)]
pub(crate) fn descriptor_reload_pending(registers: &svd::WifiMacRxDma) -> bool {
    registers
        .rx_control()
        .read()
        .append_descriptor_reload()
        .bit()
}

#[inline(always)]
pub(crate) fn request_descriptor_reload(registers: &svd::WifiMacRxDma) {
    registers
        .rx_control()
        .modify(|_, writer| writer.append_descriptor_reload().set_bit());
}

impl WifiRadioRegisters {
    /// Snapshot the complete published RX walker frontier without changing it.
    pub fn mac_rx_dma_snapshot(&self) -> MacRxDmaSnapshot {
        let dma = &self.peripherals.wifi_mac.wifi_mac_rx_dma;
        let control = dma.rx_control().read();
        let next_descriptor = read_next_descriptor(dma);
        MacRxDmaSnapshot {
            walker_enabled: control.walker_enable().bit(),
            reload_pending: control.append_descriptor_reload().bit(),
            descriptor_base: dma.rx_descriptor_base().read().address().bits(),
            next_descriptor_low: next_descriptor.address_low(),
            next_descriptor_upper_unknown: next_descriptor.has_unknown_upper_state(),
            last_descriptor_low: read_last_descriptor_low(dma),
        }
    }

    /// Initialize the RX buffer geometry without publishing a descriptor.
    ///
    /// SOURCE: first four RMWs of complete pinned
    /// `libpp.a[hal_mac.o]::mac_rxbuf_init`. Its final descriptor-base store
    /// is deliberately excluded because the RX ring owner publishes it later.
    pub fn initialize_mac_rx_buffer_prefix(&mut self) {
        let dma = &self.peripherals.wifi_mac.wifi_mac_rx_dma;
        dma.word_0c68_opaque()
            .modify(|_, w| w.bits_0_19_opaque().set(0x000f_ffff));
        dma.word_0c6c_opaque()
            .modify(|_, w| w.bits_0_19_opaque().set(4));
        dma.rx_descriptor_high_window()
            .modify(|_, w| w.address_high().set(0x02f0));
        dma.word_007c_opaque()
            .modify(|_, w| w.bits_0_7_opaque().set(0));
    }

    pub fn mac_rx_last_descriptor_low(&self) -> u32 {
        read_last_descriptor_low(&self.peripherals.wifi_mac.wifi_mac_rx_dma)
    }

    pub fn mac_rx_next_descriptor(&self) -> MacRxNextDescriptorObservation {
        read_next_descriptor(&self.peripherals.wifi_mac.wifi_mac_rx_dma)
    }

    pub fn mac_rx_walker_enabled(&self) -> bool {
        self.peripherals
            .wifi_mac
            .wifi_mac_rx_dma
            .rx_control()
            .read()
            .walker_enable()
            .bit()
    }

    pub fn mac_rx_reload_pending(&self) -> bool {
        descriptor_reload_pending(&self.peripherals.wifi_mac.wifi_mac_rx_dma)
    }

    /// Select the internal-SRAM address window carried by a retained descriptor range.
    pub fn configure_mac_rx_descriptor_window(&mut self, binding: &StableDmaRange<'_>) {
        assert!(
            binding.start() >= RX_DESCRIPTOR_INTERNAL_SRAM_START
                && binding.start() < RX_DESCRIPTOR_INTERNAL_SRAM_END
        );
        self.peripherals
            .wifi_mac
            .wifi_mac_rx_dma
            .rx_descriptor_high_window()
            .modify(|_, w| w.address_high().set(0x02f0));
    }

    /// Publish a descriptor head contained by the retained DMA range.
    pub fn write_mac_rx_descriptor_base(
        &mut self,
        binding: &StableDmaRange<'_>,
        head: DmaRef<WifiRxDmaDescriptor>,
    ) {
        assert!(binding.contains(head.bits(), 1));
        write_descriptor_base(&self.peripherals.wifi_mac.wifi_mac_rx_dma, head);
    }

    /// Start the walker only while its descriptor backing authority is held.
    pub fn publish_mac_rx_walker_enable(&mut self, _binding: &StableDmaRange<'_>) {
        set_walker_enabled(&self.peripherals.wifi_mac.wifi_mac_rx_dma, true);
    }

    /// Ask the live walker to follow newly published descriptor links.
    pub fn request_mac_rx_descriptor_reload(&mut self, _binding: &StableDmaRange<'_>) {
        request_descriptor_reload(&self.peripherals.wifi_mac.wifi_mac_rx_dma);
    }

    /// Start and confirm the walker while descriptor authority is retained.
    pub fn try_enable_mac_rx_walker(&mut self, _binding: &StableDmaRange<'_>) -> bool {
        let control = self.peripherals.wifi_mac.wifi_mac_rx_dma.rx_control();
        let previous = control.read();
        if previous.walker_enable().bit() {
            return false;
        }
        set_walker_enabled(&self.peripherals.wifi_mac.wifi_mac_rx_dma, true);
        device_fence();
        control.read().walker_enable().bit()
    }

    pub fn try_disable_mac_rx_walker(&mut self) -> bool {
        let control = self.peripherals.wifi_mac.wifi_mac_rx_dma.rx_control();
        set_walker_enabled(&self.peripherals.wifi_mac.wifi_mac_rx_dma, false);
        device_fence();
        !control.read().walker_enable().bit()
    }
}
