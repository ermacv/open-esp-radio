//! Production interrupt admission and terminal readback policy.
use super::{BluetoothInterruptObservation, InterruptSetupOwner, TaskOwner};
use oer_esp32s31_pac::bt_mac;

// These are policy sets, with geometry supplied by the PAC. Baseline faults
// are serviced by fail-closed capture; dynamic lanes feed the scheduler.
struct Bank0Lanes(u32);
struct Bank1Lanes(u32);
struct TaskLanes(u32);
struct ProductionLanes {
    baseline_0: Bank0Lanes,
    baseline_1: Bank1Lanes,
    dynamic_0: Bank0Lanes,
    dynamic_1: Bank1Lanes,
    task: TaskLanes,
}
const PRODUCTION: ProductionLanes = ProductionLanes {
    baseline_0: Bank0Lanes(bt_mac::irq_enable_0::SOURCE_15_MASK),
    baseline_1: Bank1Lanes(
        bt_mac::irq_enable_1::CONTROLLER_TIME_OVERFLOW_MASK
            | bt_mac::irq_enable_1::SOURCE_8_MASK
            | bt_mac::irq_enable_1::SOURCE_9_MASK
            | bt_mac::irq_enable_1::SOURCE_12_MASK,
    ),
    dynamic_0: Bank0Lanes(
        bt_mac::irq_enable_0::SOURCE_21_MASK | bt_mac::irq_enable_0::SOURCES_27_28_MASK,
    ),
    dynamic_1: Bank1Lanes(bt_mac::irq_enable_1::SOURCE_3_MASK),
    // Task14 is cleared before RUN and masked/acknowledged at terminal stop.
    task: TaskLanes(bt_mac::interrupt_enable::SCHEDULER_RUN_MASK),
};

// SOURCE: BLOB_S31_BLE_IRQ_RELEASE_SCOPE_20261027 and
// HW25_S31_BLE_RELEASE_20261027: common teardown leaves these feature enables.
// Terminal policy masks/acknowledges them; admission never enables them.
const FEATURES_0: Bank0Lanes = Bank0Lanes(
    bt_mac::irq_enable_0::SOURCE_0_MASK
        | bt_mac::irq_enable_0::SOURCE_1_MASK
        | bt_mac::irq_enable_0::SOURCE_8_MASK
        | bt_mac::irq_enable_0::SOURCE_13_MASK
        | bt_mac::irq_enable_0::SOURCE_16_MASK,
);
const FEATURES_1: Bank1Lanes = Bank1Lanes(
    bt_mac::irq_enable_1::SOURCE_4_MASK
        | bt_mac::irq_enable_1::SOURCE_5_MASK
        | bt_mac::irq_enable_1::SOURCE_6_MASK
        | bt_mac::irq_enable_1::SOURCE_13_MASK,
);
pub(super) const fn unsupported_primary_pending(primary: BluetoothInterruptObservation) -> bool {
    primary.status_0 & !(PRODUCTION.baseline_0.0 | PRODUCTION.dynamic_0.0 | FEATURES_0.0) != 0
        || primary.status_1 & !(PRODUCTION.baseline_1.0 | PRODUCTION.dynamic_1.0 | FEATURES_1.0)
            != 0
}

/// Sequential source observations; values grant no register or release authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothInterruptPolicyObservation {
    pub primary: BluetoothInterruptObservation,
    pub task_enable: u32,
}

/// An interrupt invariant failure. Owners must remain quarantined until reset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothInterruptPolicyFault {
    /// An enable outside production's explicit set was inherited.
    ForeignEnable(BluetoothInterruptPolicyObservation),
    /// Unsupported pending blocked cleanup, or terminal readback stayed nonzero.
    ReleaseNotQuiet(BluetoothInterruptPolicyObservation),
}
impl BluetoothInterruptPolicyFault {
    pub const fn observation(self) -> BluetoothInterruptPolicyObservation {
        match self {
            Self::ForeignEnable(observation) | Self::ReleaseNotQuiet(observation) => observation,
        }
    }
}
impl BluetoothInterruptPolicyObservation {
    /// Check before production writes any controller interrupt enables.
    pub fn admit(self) -> Result<(), BluetoothInterruptPolicyFault> {
        if self.primary.enable_0 & !(PRODUCTION.baseline_0.0 | PRODUCTION.dynamic_0.0) != 0
            || self.primary.enable_1 & !(PRODUCTION.baseline_1.0 | PRODUCTION.dynamic_1.0) != 0
            || self.task_enable & !PRODUCTION.task.0 != 0
        {
            Err(BluetoothInterruptPolicyFault::ForeignEnable(self))
        } else {
            Ok(())
        }
    }
    /// Check the supplied primary/NRT status images and all enable words.
    /// A masked STATUS image does not establish that retained pending is zero.
    pub fn confirm_released(self) -> Result<(), BluetoothInterruptPolicyFault> {
        if [
            self.primary.enable_0,
            self.primary.enable_1,
            self.primary.status_0,
            self.primary.status_1,
            self.primary.nrt_0,
            self.primary.nrt_1,
            self.task_enable,
        ]
        .into_iter()
        .any(|word| word != 0)
        {
            Err(BluetoothInterruptPolicyFault::ReleaseNotQuiet(self))
        } else {
            Ok(())
        }
    }
}
impl InterruptSetupOwner {
    /// Admit inherited enables before HAL/PHY initialization can mask them.
    /// This writes nothing. A refusal retains all source observations.
    pub fn admit_interrupts(&self, task: &TaskOwner) -> Result<(), BluetoothInterruptPolicyFault> {
        BluetoothInterruptPolicyObservation {
            primary: self.interrupt_observation(),
            task_enable: task.interrupt_enable_image(),
        }
        .admit()
    }
}

#[cfg(test)]
mod tests;
