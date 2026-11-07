//! Source-confirmed CPU-route policy for the three Bluetooth interrupt lines.
//!
//! The numeric identities come from the official ESP32-S31 interrupt table:
//! <https://github.com/espressif/esp-idf/blob//components/soc/esp32s31/include/soc/interrupts.h#L139-L153>.
//! Priority, core affinity and the unusual residency split come from the
//! pinned public BTDM OSAL wrapper plus the complete S31 allocation leaves.
//! This module is policy only: it cannot program an interrupt matrix or mint
//! an active PAC owner.

/// One hardware interrupt source used by the LE Controller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum CpuInterruptSource {
    /// `ETS_MODEM_BT_MAC_INTR_SOURCE`, allocated by the primary BTDM HAL.
    PrimaryBtMac = 124,
    /// `ETS_MODEM_LP_TIMER_INTR_SOURCE`, allocated during controller task init.
    ModemLpTimer = 127,
    /// `ETS_MODEM_BT_MAC_INT1_INTR_SOURCE`, allocated by the BLE NRT stack.
    NrtBtMacInt1 = 133,
}

impl CpuInterruptSource {
    /// Return the audited peripheral-source number.
    pub const fn number(self) -> u16 {
        self as u16
    }
}

/// Handler-residency policy selected by the complete vendor allocation path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterruptHandlerResidency {
    /// The allocator receives the IRAM flag in addition to level three.
    IramRequired,
    /// The allocator receives level three without the IRAM flag.
    ///
    /// This does not grant permission to block or allocate in the handler. It
    /// records only the exact platform allocation policy.
    IramNotRequested,
}

/// Complete platform route policy for one Bluetooth CPU interrupt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CpuInterruptRoutePolicy {
    source: CpuInterruptSource,
    priority_level: u8,
    residency: InterruptHandlerResidency,
    pinned_to_controller_core: bool,
}

impl CpuInterruptRoutePolicy {
    /// Primary source-124 route used by the BTDM HAL ISR.
    pub const PRIMARY: Self = Self {
        source: CpuInterruptSource::PrimaryBtMac,
        priority_level: 3,
        residency: InterruptHandlerResidency::IramRequired,
        pinned_to_controller_core: true,
    };

    /// Modem low-power timer source-127 route used by the controller RTC ISR.
    pub const MODEM_LP_TIMER: Self = Self {
        source: CpuInterruptSource::ModemLpTimer,
        priority_level: 3,
        residency: InterruptHandlerResidency::IramRequired,
        pinned_to_controller_core: true,
    };

    /// NRT source-133 route used by the opaque acknowledgement ISR.
    pub const NRT: Self = Self {
        source: CpuInterruptSource::NrtBtMacInt1,
        priority_level: 3,
        residency: InterruptHandlerResidency::IramNotRequested,
        pinned_to_controller_core: true,
    };

    /// Hardware peripheral interrupt source.
    pub const fn source(self) -> CpuInterruptSource {
        self.source
    }

    /// CPU priority level selected by the pinned wrapper.
    pub const fn priority_level(self) -> u8 {
        self.priority_level
    }

    /// Handler-residency policy selected by the caller/wrapper pair.
    pub const fn residency(self) -> InterruptHandlerResidency {
        self.residency
    }

    /// Whether allocation is forced onto the configured Controller core.
    pub const fn pinned_to_controller_core(self) -> bool {
        self.pinned_to_controller_core
    }
}

#[cfg(test)]
mod tests;
