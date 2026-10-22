//! Owned controller HAL initialization immediately after clock setup.
//!
//! The complete reviewed `r_btdm_task_init` hardware subsequence executes the
//! 49-operation controller HAL component before scheduler initialization.
//! Vendor task-environment and broker setup are software architecture and are
//! replaced by Rust-owned runtime resources rather than copied into this
//! hardware typestate.

use oer_esp32s31_hal::bluetooth::engine::ControllerMemory;

use crate::resources::{InterruptBankOwner, TaskResources, separate_interrupt_owner};

use oer_esp32s31_hal::bluetooth::{
    BluetoothControllerHalInitConfig, BluetoothControllerTimeScale, InterruptSetupOwner,
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
pub struct ControllerHalInitialized<M: ControllerMemory> {
    pub(crate) task: TaskResources<M>,
    pub(crate) interrupts: InterruptBankOwner,
    pub(crate) time_scale: BluetoothControllerTimeScale,
    pub(crate) standalone_dtm_profile: StandaloneAlwaysAwakeDtmProfile,
}

impl<M: ControllerMemory> ControllerHalInitialized<M> {
    /// Return the scheduler scale established for this hardware epoch.
    pub const fn controller_time_scale(&self) -> BluetoothControllerTimeScale {
        self.time_scale
    }
}

impl<M: ControllerMemory> ControllerHalInitialized<M> {
    /// Execute the complete reviewed standalone controller HAL component on
    /// the clocked Bluetooth client.
    ///
    /// This consumes the arena's owning engine and inactive IRQ bank before
    /// the first controller write. The result must continue through
    /// scheduler and interrupt initialization before it can run radio work.
    ///
    /// `ClockedOwner` proves the enabled Bluetooth clocks and released
    /// controller resets, and this transition uniquely owns the task and
    /// inactive interrupt partitions.
    #[cfg(target_arch = "riscv32")]
    pub fn initialize(
        engine: oer_esp32s31_hal::bluetooth::engine::Engine<M>,
        interrupts: InterruptSetupOwner,
    ) -> Result<Self, ControllerHalInitializationFailure<M>> {
        Self::initialize_with(engine, interrupts, |task, config| {
            task.initialize_controller_hal(config)
        })
    }

    #[cfg(test)]
    pub fn initialize_for_validation(
        engine: oer_esp32s31_hal::bluetooth::engine::Engine<M>,
        interrupts: InterruptSetupOwner,
        initialize: impl FnOnce(&mut TaskResources<M>, BluetoothControllerHalInitConfig),
    ) -> Self {
        Self::initialize_with(engine, interrupts, |task, config| {
            task.engine().initialize_hal_for_host_test(config)?;
            initialize(task, config);
            Ok(())
        })
        .ok()
        .expect("validation starts with a fresh model engine")
    }

    fn initialize_with(
        engine: oer_esp32s31_hal::bluetooth::engine::Engine<M>,
        interrupts: InterruptSetupOwner,
        initialize: impl FnOnce(
            &mut TaskResources<M>,
            BluetoothControllerHalInitConfig,
        )
            -> Result<(), oer_esp32s31_hal::bluetooth::engine::EngineInitError>,
    ) -> Result<Self, ControllerHalInitializationFailure<M>> {
        let config = BluetoothControllerHalInitConfig::reviewed_standalone();
        let standalone_dtm_profile = StandaloneAlwaysAwakeDtmProfile::mint();
        let time_scale = config.controller_time_scale();
        let (mut task, interrupts) = separate_interrupt_owner(engine, interrupts);
        if let Err(error) = initialize(&mut task, config) {
            return Err(ControllerHalInitializationFailure {
                error,
                _task: task,
                _interrupts: interrupts,
            });
        }
        Ok(Self {
            task,
            interrupts,
            time_scale,
            standalone_dtm_profile,
        })
    }
}

/// Rejected HAL initialization retains the complete owning engine and IRQ bank.
#[must_use]
pub struct ControllerHalInitializationFailure<M: ControllerMemory> {
    pub error: oer_esp32s31_hal::bluetooth::engine::EngineInitError,
    _task: TaskResources<M>,
    _interrupts: InterruptBankOwner,
}
