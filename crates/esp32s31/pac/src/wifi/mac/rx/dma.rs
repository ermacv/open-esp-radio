//! Safe generated-PAC ownership for the MAC RX descriptor walker.

#![forbid(unsafe_code)]

use crate::layouts::{DmaRef, WifiRxDmaDescriptor};
use crate::{WifiRxDmaRegisters, device_fence, svd};

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

impl WifiRxDmaRegisters {
    /// Snapshot the complete published RX walker frontier without changing it.
    pub fn mac_rx_dma_snapshot(&self) -> MacRxDmaSnapshot {
        let dma = &self.peripherals.wifi_mac_rx_dma;
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
        let dma = &self.peripherals.wifi_mac_rx_dma;
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
        read_last_descriptor_low(&self.peripherals.wifi_mac_rx_dma)
    }

    pub fn mac_rx_next_descriptor(&self) -> MacRxNextDescriptorObservation {
        read_next_descriptor(&self.peripherals.wifi_mac_rx_dma)
    }

    pub fn mac_rx_walker_enabled(&self) -> bool {
        self.peripherals
            .wifi_mac_rx_dma
            .rx_control()
            .read()
            .walker_enable()
            .bit()
    }

    pub fn mac_rx_reload_pending(&self) -> bool {
        descriptor_reload_pending(&self.peripherals.wifi_mac_rx_dma)
    }

    /// Select the internal-SRAM address window the descriptors lie in: the
    /// DMA window the link places them in (`dma-window.x`).
    pub fn configure_mac_rx_descriptor_window(&mut self) {
        self.peripherals
            .wifi_mac_rx_dma
            .rx_descriptor_high_window()
            .modify(|_, w| w.address_high().set(0x02f0));
    }

    /// Publish a descriptor head: a placed descriptor, so inside the DMA
    /// window by the link's proof.
    pub fn write_mac_rx_descriptor_base(&mut self, head: DmaRef<WifiRxDmaDescriptor>) {
        write_descriptor_base(&self.peripherals.wifi_mac_rx_dma, head);
    }

    /// Start the walker.
    pub fn publish_mac_rx_walker_enable(&mut self) {
        set_walker_enabled(&self.peripherals.wifi_mac_rx_dma, true);
    }

    /// Ask the live walker to follow newly published descriptor links.
    pub fn request_mac_rx_descriptor_reload(&mut self) {
        request_descriptor_reload(&self.peripherals.wifi_mac_rx_dma);
    }

    /// Start and confirm the walker.
    pub fn try_enable_mac_rx_walker(&mut self) -> bool {
        let control = self.peripherals.wifi_mac_rx_dma.rx_control();
        let previous = control.read();
        if previous.walker_enable().bit() {
            return false;
        }
        set_walker_enabled(&self.peripherals.wifi_mac_rx_dma, true);
        device_fence();
        control.read().walker_enable().bit()
    }

    pub fn try_disable_mac_rx_walker(&mut self) -> bool {
        let control = self.peripherals.wifi_mac_rx_dma.rx_control();
        set_walker_enabled(&self.peripherals.wifi_mac_rx_dma, false);
        device_fence();
        !control.read().walker_enable().bit()
    }
}

impl WifiRxDmaRegisters {
    /// Order descriptor memory and MMIO at a hardware ownership boundary.
    pub fn order_device_accesses(&mut self) {
        device_fence();
    }

    /// The walker's control register, for a MAC initialization that writes
    /// it in its own order.
    pub(crate) fn rx_control(&self) -> &svd::wifi_mac_rx_dma::RxControl {
        self.peripherals.wifi_mac_rx_dma.rx_control()
    }
}
