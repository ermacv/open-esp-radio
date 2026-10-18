//! Generated-PAC ownership for the finite MAC interrupt transaction.

#![forbid(unsafe_code)]

use crate::{
    MacInterruptMask, MacInterruptSnapshot, MacPowerInterruptSnapshot, WifiRadioRegisters,
    device_fence,
    svd::{self, interrupt_snapshot},
};

/// Proof that the connected-STA hardware policy was applied before IRQ activation.
///
/// Construction is private to the exact two-register transaction below. The
/// connected runtime consumes this value when it activates its interrupt
/// epoch, so removing the transaction from that lifecycle becomes a compile
/// error rather than an intermittent runtime regression.
#[must_use = "connected STA interrupt activation requires this preparation proof"]
pub struct ConnectedStaWithoutPowerSavePrepared {
    _private: (),
}

/// Classified field-level readback of the MAC interrupt enable register.
///
/// Unknown images are retained as an explicit state instead of reconstructing
/// a handwritten integer register image from the generated field accessors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacInterruptEnableState {
    /// Every interrupt-enable field is clear.
    Disabled,
    /// Exact complete mask published by the reviewed cold receive initializer.
    ColdRx,
    /// A field combination outside the two reviewed writable images.
    Unknown,
}

pub(crate) fn publish_mac_interrupt_mask(
    interrupt: &svd::WifiMacInterrupt,
    event_mask: MacInterruptMask,
) {
    if event_mask == MacInterruptMask::NONE {
        svd::fixed_register_image::mask_mac_interrupts(interrupt);
        return;
    }

    assert_eq!(
        event_mask,
        MacInterruptMask::COLD_RX,
        "MAC interrupt domain gained an unpublished register image"
    );
    svd::fixed_register_image::enable_cold_rx_mac_interrupts(interrupt);
}

pub(crate) fn observe_mac_interrupt_enable(
    interrupt: &svd::WifiMacInterrupt,
) -> MacInterruptEnableState {
    let enable = interrupt.enable().read();
    let disabled = enable.bits_0_4_opaque().bits() == 0
        && enable.rx_associated_auxiliary_5().bit_is_clear()
        && enable.bit_6_opaque().bit_is_clear()
        && enable.tx_complete().bit_is_clear()
        && enable.bss_color_collision().bit_is_clear()
        && enable.bits_9_10_opaque().bits() == 0
        && enable.watchdog().bit_is_clear()
        && enable.bit_12_opaque().bit_is_clear()
        && enable.bit_13_opaque().bit_is_clear()
        && enable.rx_success().bit_is_clear()
        && enable.sta_beacon_filter().bit_is_clear()
        && enable.bits_16_18_opaque().bits() == 0
        && enable.tx_timeout().bit_is_clear()
        && enable.bit_20_opaque().bit_is_clear()
        && enable.bit_21_opaque().bit_is_clear()
        && enable.bit_22_opaque().bit_is_clear()
        && enable.bit_23_opaque().bit_is_clear()
        && enable.rx_associated_auxiliary_24().bit_is_clear()
        && enable.bits_25_26_opaque().bits() == 0
        && enable.bit_27_opaque().bit_is_clear()
        && enable.bit_28_opaque().bit_is_clear()
        && enable.bits_29_31_opaque().bits() == 0;
    if disabled {
        return MacInterruptEnableState::Disabled;
    }

    let cold_rx = enable.bits_0_4_opaque().bits() == 0
        && enable.rx_associated_auxiliary_5().bit_is_set()
        && enable.bit_6_opaque().bit_is_set()
        && enable.tx_complete().bit_is_set()
        && enable.bss_color_collision().bit_is_set()
        && enable.bits_9_10_opaque().bits() == 0
        && enable.watchdog().bit_is_set()
        && enable.bit_12_opaque().bit_is_set()
        && enable.bit_13_opaque().bit_is_set()
        && enable.rx_success().bit_is_set()
        && enable.sta_beacon_filter().bit_is_clear()
        && enable.bits_16_18_opaque().bits() == 0
        && enable.tx_timeout().bit_is_set()
        && enable.bit_20_opaque().bit_is_clear()
        && enable.bit_21_opaque().bit_is_set()
        && enable.bit_22_opaque().bit_is_clear()
        && enable.bit_23_opaque().bit_is_set()
        && enable.rx_associated_auxiliary_24().bit_is_set()
        && enable.bits_25_26_opaque().bits() == 0
        && enable.bit_27_opaque().bit_is_set()
        && enable.bit_28_opaque().bit_is_set()
        && enable.bits_29_31_opaque().bits() == 0;
    if cold_rx {
        MacInterruptEnableState::ColdRx
    } else {
        MacInterruptEnableState::Unknown
    }
}

#[inline(always)]
fn disable_sta_beacon_filter(
    control: &svd::WifiMacStaBeaconFilter,
    interrupt: &svd::WifiMacInterrupt,
) {
    // Complete libpp[hal_mac.o]::hal_disable_sta_beacon_filter. Preserve
    // the two independent fresh-read RMW edges and their order: hardware
    // filtering is disabled before its matching interrupt source is masked.
    control
        .control()
        .modify(|_, writer| writer.bits_0_2_opaque().set(0));
    interrupt
        .enable()
        .modify(|_, writer| writer.sta_beacon_filter().clear_bit());
}

/// Task-side setup token for one MAC interrupt handoff epoch.
///
/// This token exists after the cold owner has been consumed but before the
/// interrupt is routed to a CPU. Activating it publishes the final mask,
/// clears stale events and consumes all task-side enable/clear access.
pub struct MacInterruptSetup {
    pub(crate) peripheral: svd::WifiMacInterrupt,
    pub(crate) power_peripheral: svd::WifiMacPowerInterrupt,
}

trait MacInterruptActivationBackend {
    fn mask_mac_events(&mut self);
    fn mask_power_events(&mut self);
    fn clear_mac_events(&mut self);
    fn clear_power_events(&mut self);
    fn publish_mac_events(&mut self, event_mask: MacInterruptMask);
    fn publish_power_events(&mut self);
    fn fence(&mut self);
}

fn activate_mac_interrupt_epoch(
    backend: &mut impl MacInterruptActivationBackend,
    event_mask: MacInterruptMask,
) {
    // A previous role may leave a level event latched after its CPU route has
    // been detached. Keep both banks masked until every stale event is
    // acknowledged. Publishing the runtime mask before CLEAR creates a real
    // preemption window in which the regular level route can enter before
    // setup returns.
    backend.mask_mac_events();
    backend.mask_power_events();
    backend.clear_mac_events();
    backend.clear_power_events();
    backend.fence();
    backend.publish_mac_events(event_mask);
    backend.publish_power_events();
    backend.fence();
}

/// One live interrupt bank whose complete enable image can be masked.
trait InterruptBankMaskBackend {
    fn mask_events(&mut self);
    fn fence(&mut self);
}

fn mask_interrupt_bank(backend: &mut impl InterruptBankMaskBackend) {
    // Masking is not an acknowledgement: a latched event stays in STATUS and
    // becomes visible again when its source is unmasked.
    backend.mask_events();
    backend.fence();
}

impl MacInterruptActivationBackend for MacInterruptSetup {
    fn mask_mac_events(&mut self) {
        publish_mac_interrupt_mask(&self.peripheral, MacInterruptMask::NONE);
    }

    fn mask_power_events(&mut self) {
        svd::fixed_register_image::mask_mac_power_interrupts(&self.power_peripheral);
    }

    /// Enable the station TBTT event for the whole route epoch.
    ///
    /// SOURCE: complete pinned `libpp[hal_tsf.o]::hal_enable_sta_tbtt`
    /// enables the station TBTT event at bit 3 of `0x2010_d8b4`. One route
    /// serves every Wi-Fi role in turn, so the mask stays enabled and the
    /// station TBTT enable, which only a connected station sets, gates it.
    fn publish_power_events(&mut self) {
        self.power_peripheral
            .enable()
            .modify(|_, writer| writer.tbtt_0().set_bit());
    }

    fn clear_mac_events(&mut self) {
        crate::generated::mac_interrupt_clear(
            &self.peripheral,
            crate::generated::MacInterruptClearImage::new(u32::MAX),
        );
    }

    fn clear_power_events(&mut self) {
        svd::fixed_register_image::clear_all_mac_power_interrupts(&self.power_peripheral);
    }

    fn publish_mac_events(&mut self, event_mask: MacInterruptMask) {
        publish_mac_interrupt_mask(&self.peripheral, event_mask);
    }

    fn fence(&mut self) {
        device_fence();
    }
}

impl MacInterruptSetup {
    pub(crate) fn from_peripherals(
        peripherals: svd::peripheral_ownership::WifiInterruptPeripherals,
    ) -> Self {
        Self {
            peripheral: peripherals.wifi_mac_interrupt,
            power_peripheral: peripherals.wifi_mac_power_interrupt,
        }
    }

    /// Read the currently published MAC interrupt mask.
    pub fn mac_interrupt_enable(&self) -> MacInterruptEnableState {
        observe_mac_interrupt_enable(&self.peripheral)
    }

    /// Mask every MAC event and acknowledge every stale event.
    pub fn mask_and_clear_all_mac_interrupts(&mut self) {
        publish_mac_interrupt_mask(&self.peripheral, MacInterruptMask::NONE);
        crate::generated::mac_interrupt_clear(
            &self.peripheral,
            crate::generated::MacInterruptClearImage::new(u32::MAX),
        );
        device_fence();
    }

    /// Disable vendor hardware beacon filtering before a connected STA epoch.
    ///
    /// The operation requires both still-disjoint task owners and therefore
    /// cannot race an active ISR. It is the complete vendor disable leaf:
    /// CONTROL bits 2:0 are cleared before interrupt-enable bit 15.
    pub fn prepare_connected_sta_without_power_save(
        &mut self,
        registers: &mut WifiRadioRegisters,
    ) -> ConnectedStaWithoutPowerSavePrepared {
        disable_sta_beacon_filter(
            &registers.peripherals.wifi_mac.wifi_mac_sta_beacon_filter,
            &self.peripheral,
        );
        ConnectedStaWithoutPowerSavePrepared { _private: () }
    }

    /// Publish the runtime event mask and create the finite ISR capability.
    ///
    /// The CPU interrupt route must still be unbound while this transaction
    /// executes. The returned value should be installed in its final static
    /// storage before the platform route is enabled.
    pub fn activate(
        mut self,
        event_mask: MacInterruptMask,
    ) -> (MacInterruptRegisters, MacPowerInterruptRegisters) {
        // Clear stale level events while masked, then publish the complete MAC
        // mask as the last interrupt-producing edge before the caller exposes
        // either ISR capability.
        activate_mac_interrupt_epoch(&mut self, event_mask);
        (
            MacInterruptRegisters {
                peripheral: self.peripheral,
            },
            MacPowerInterruptRegisters {
                peripheral: self.power_peripheral,
            },
        )
    }
}

/// Disjoint generated register capability intended for the hard power ISR.
///
/// This bank is split from the cold owner together with
/// [`MacInterruptRegisters`]. Ordinary [`crate::WifiRadioRegisters`] therefore
/// cannot race its STATUS/CLEAR transaction from task context.
pub struct MacPowerInterruptRegisters {
    peripheral: svd::WifiMacPowerInterrupt,
}

impl InterruptBankMaskBackend for MacPowerInterruptRegisters {
    fn mask_events(&mut self) {
        svd::fixed_register_image::mask_mac_power_interrupts(&self.peripheral);
    }

    fn fence(&mut self) {
        device_fence();
    }
}

impl MacPowerInterruptRegisters {
    #[cfg(oer_probe)]
    pub(crate) fn from_peripheral_for_probe(peripheral: svd::WifiMacPowerInterrupt) -> Self {
        Self { peripheral }
    }

    /// Mask every WDEVPWR cause of the live epoch without acknowledging one.
    ///
    /// Publishes the fixed all-zero ENABLE image, then orders the write. A
    /// latched cause stays in STATUS; the CPU line deasserts once the masked
    /// image reaches the bank.
    pub fn mask_all_power_interrupts(&mut self) {
        mask_interrupt_bank(self);
    }

    /// Sample the complete masked WDEVPWR event image.
    ///
    /// SOURCE: complete `libpp[hal_pwr.o]::hal_pwr_interrupt_get_event` reads `0x2010_d8bc`.
    pub fn power_interrupt_status(&self) -> MacPowerInterruptSnapshot {
        MacPowerInterruptSnapshot(interrupt_snapshot::sample_mac_power_interrupt(
            &self.peripheral,
        ))
    }

    /// Acknowledge the complete sampled WDEVPWR event image.
    ///
    /// SOURCE: complete `libpp[hal_pwr.o]::hal_pwr_interrupt_clr_event` stores its argument to `0x2010_d8c0`.
    pub fn acknowledge_power_interrupts(&mut self, snapshot: MacPowerInterruptSnapshot) {
        interrupt_snapshot::acknowledge_mac_power_interrupt(&self.peripheral, snapshot.0);
        device_fence();
    }
}

/// Disjoint generated register capability intended for the hard MAC ISR.
///
/// It is issued by [`MacInterruptSetup::activate`]; construction is
/// crate-private so application code cannot manufacture another ISR owner or
/// retain task-side interrupt enable/clear access during an active epoch.
pub struct MacInterruptRegisters {
    pub(crate) peripheral: svd::WifiMacInterrupt,
}

impl InterruptBankMaskBackend for MacInterruptRegisters {
    fn mask_events(&mut self) {
        publish_mac_interrupt_mask(&self.peripheral, MacInterruptMask::NONE);
    }

    fn fence(&mut self) {
        device_fence();
    }
}

impl MacInterruptRegisters {
    #[cfg(oer_probe)]
    pub(crate) fn from_peripheral_for_probe(peripheral: svd::WifiMacInterrupt) -> Self {
        Self { peripheral }
    }

    /// Mask every MAC interrupt source of the live epoch without
    /// acknowledging one.
    ///
    /// Publishes the fixed all-zero ENABLE image, then orders the write. A
    /// latched event stays in STATUS. [`Self::unmask_rx_delivery_interrupts`]
    /// afterwards restores only the RX delivery group.
    pub fn mask_all_mac_interrupts(&mut self) {
        mask_interrupt_bank(self);
    }

    /// Sample the complete MAC interrupt status image.
    ///
    /// SOURCE: complete `libpp.a::hal_mac_interrupt_get_event` proves the
    /// status address and complete `libpp[wdev.o]::wDev_ProcessFiq` consumes exactly this
    /// status image. The runtime mask is configured before IRQ activation and
    /// is not sampled by the vendor FIQ transaction.
    pub fn mac_interrupt_status(&self) -> MacInterruptSnapshot {
        MacInterruptSnapshot(interrupt_snapshot::sample_mac_interrupt(&self.peripheral))
    }

    /// Acknowledge the complete sampled event image, then order the ISR edge.
    ///
    /// SOURCE: complete `libpp.a::hal_mac_interrupt_clr_event` is one
    /// full-width store to the generated write-to-clear register.
    pub fn acknowledge_mac_interrupts(&mut self, snapshot: MacInterruptSnapshot) {
        interrupt_snapshot::acknowledge_mac_interrupt(&self.peripheral, snapshot.0);
        device_fence();
    }

    /// Temporarily suppress the observed RX delivery group without changing
    /// any other member of the reviewed runtime mask.
    ///
    /// This is a source-moderation operation, not an acknowledgement. The
    /// hard ISR uses it before acknowledging the RX image. The group contains
    /// RX_SUCCESS and the two auxiliary sources observed on every saturated
    /// RX edge; TX and unrelated sources remain independently live.
    pub fn mask_rx_delivery_interrupts(&mut self) {
        self.peripheral.enable().modify(|_, writer| {
            writer
                .rx_associated_auxiliary_5()
                .clear_bit()
                .rx_success()
                .clear_bit()
                .rx_associated_auxiliary_24()
                .clear_bit()
        });
        device_fence();
    }

    /// Restore the RX delivery group while preserving the current mask.
    ///
    /// A latched RX status which arrived while masked becomes visible through
    /// the level CPU route after this ordered write, so the task does not need
    /// to fabricate a hardware completion edge.
    pub fn unmask_rx_delivery_interrupts(&mut self) {
        self.peripheral.enable().modify(|_, writer| {
            writer
                .rx_associated_auxiliary_5()
                .set_bit()
                .rx_success()
                .set_bit()
                .rx_associated_auxiliary_24()
                .set_bit()
        });
        device_fence();
    }

    /// Mask and acknowledge both interrupt banks, returning task-side setup.
    ///
    /// The caller must first disable both CPU interrupt routes and prove that
    /// neither hard handler retains a reference to `self` or `power`. Owning
    /// both values then closes the finite ISR epoch and makes a later
    /// [`MacInterruptSetup::activate`] transaction possible without stealing
    /// either PAC peripheral a second time.
    pub fn deactivate(self, power: MacPowerInterruptRegisters) -> MacInterruptSetup {
        publish_mac_interrupt_mask(&self.peripheral, MacInterruptMask::NONE);
        svd::fixed_register_image::mask_mac_power_interrupts(&power.peripheral);
        crate::generated::mac_interrupt_clear(
            &self.peripheral,
            crate::generated::MacInterruptClearImage::new(u32::MAX),
        );
        svd::fixed_register_image::clear_all_mac_power_interrupts(&power.peripheral);
        device_fence();
        MacInterruptSetup {
            peripheral: self.peripheral,
            power_peripheral: power.peripheral,
        }
    }
}

#[cfg(test)]
mod tests;
