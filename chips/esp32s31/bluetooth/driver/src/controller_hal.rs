//! Owned controller HAL initialization immediately after clock setup.
//!
//! The complete reviewed `r_btdm_task_init` hardware subsequence executes the
//! 49-operation controller HAL component before scheduler initialization.
//! Vendor task-environment and broker setup are software architecture and are
//! replaced by Rust-owned runtime resources rather than copied into this
//! hardware typestate.

use crate::resources::{InterruptBankOwner, TaskResources, separate_interrupt_owner};

use oer_esp32s31_hal::bluetooth::{
    BluetoothControllerHalInitConfig, BluetoothControllerTimeScale, ClockedOwner,
};

/// Controller time scale of the reviewed standalone HAL configuration.
pub const fn reviewed_standalone_time_scale() -> BluetoothControllerTimeScale {
    BluetoothControllerHalInitConfig::reviewed_standalone().controller_time_scale()
}

/// Affine selection of the supported standalone always-awake DTM profile.
///
/// The marker records only that this Controller epoch was constructed without
/// a modem-sleep/wake policy. It performs no MMIO and proves neither that RF
/// is ready nor that a controller-time request has completed. Its private
/// field keeps construction in the clocked-to-controller-HAL transition, and
/// the absence of `Copy` or `Clone` keeps the selection bound to one epoch.
#[must_use = "the standalone always-awake DTM profile belongs to one Controller epoch"]
pub(crate) struct StandaloneAlwaysAwakeDtmProfile {
    _private: (),
}

impl StandaloneAlwaysAwakeDtmProfile {
    const fn mint() -> Self {
        Self { _private: () }
    }

    #[cfg(target_arch = "riscv32")]
    pub(crate) const fn gate_controller_time_request(&self) {
        // Presence of this private affine value in the structural owner is the
        // gate. The marker never accepts an independently supplied task owner.
    }
}

/// Bluetooth hardware after the complete controller HAL-init component.
///
/// This state proves only the reviewed MMIO component and retains its exact
/// scheduler time scale. It does not claim initialized scheduler lists,
/// interrupts, PHY, BTBB, Link Layer, HCI or a running controller. Dropping it
/// is fail-stop because no verified rollback exists after the first write.
#[must_use = "the controller HAL state retains every powered Bluetooth owner"]
pub struct ControllerHalInitialized {
    pub(crate) task: TaskResources,
    pub(crate) interrupts: InterruptBankOwner,
    pub(crate) time_scale: BluetoothControllerTimeScale,
    pub(crate) standalone_dtm_profile: StandaloneAlwaysAwakeDtmProfile,
}

impl ControllerHalInitialized {
    /// Return the scheduler scale established for this hardware epoch.
    pub const fn controller_time_scale(&self) -> BluetoothControllerTimeScale {
        self.time_scale
    }
}

impl ControllerHalInitialized {
    /// Execute the complete reviewed standalone controller HAL component on
    /// the clocked Bluetooth client.
    ///
    /// This consumes the clocked client and arms fail-stop ownership before
    /// the first controller write. The result must continue through
    /// scheduler and interrupt initialization before it can run radio work.
    #[cfg(target_arch = "riscv32")]
    #[allow(
        unsafe_code,
        reason = "the clocked client discharges the HAL transaction's external prerequisites"
    )]
    pub fn initialize(clocked: ClockedOwner) -> Self {
        Self::initialize_with(clocked, |task, config| {
            // SAFETY: `ClockedOwner` proves the enabled Bluetooth clocks and
            // released controller resets, this transition uniquely owns the
            // task and inactive interrupt partitions, and the fixed S31
            // SRAM-prefix profile is retained by the returned affine state.
            unsafe {
                task.initialize_controller_hal(config);
            }
        })
    }

    #[cfg(test)]
    pub fn initialize_for_validation(
        clocked: ClockedOwner,
        initialize: impl FnOnce(&mut TaskResources, BluetoothControllerHalInitConfig),
    ) -> Self {
        Self::initialize_with(clocked, initialize)
    }

    fn initialize_with(
        clocked: ClockedOwner,
        initialize: impl FnOnce(&mut TaskResources, BluetoothControllerHalInitConfig),
    ) -> Self {
        let config = BluetoothControllerHalInitConfig::reviewed_standalone();
        let standalone_dtm_profile = StandaloneAlwaysAwakeDtmProfile::mint();
        let time_scale = config.controller_time_scale();
        let (mut task, interrupts) = separate_interrupt_owner(clocked);
        initialize(&mut task, config);
        Self {
            task,
            interrupts,
            time_scale,
            standalone_dtm_profile,
        }
    }
}
