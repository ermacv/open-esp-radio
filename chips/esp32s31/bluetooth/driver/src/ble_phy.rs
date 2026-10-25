//! Owned BLE PHY engine activation after common PHY and BTBB initialization.

#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::bluetooth::engine::ControllerMemory;

#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::power::PlatformClockBackend;
#[cfg(target_arch = "riscv32")]
use oer_interrupt_table::signal::SignalExecutor;

#[cfg(target_arch = "riscv32")]
use crate::{low_power::ControllerLowPowerHardwareInitialized, resources::InterruptBankOwner};
#[cfg(target_arch = "riscv32")]
use oer_bluetooth_hci::BluetoothPublicDeviceAddress;
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::bluetooth::ControllerPublicAddress;
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::bluetooth::ModemLpTimerLowPowerHardwareInitializedOwner;
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::bluetooth::engine::BlePhyLe1MPacketStartCalibration;
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::bluetooth::engine::{
    LePacketCapturedTime, PeripheralConnectionCapturedAnchorTime,
};

/// Observation that the source-owned normal BLE PHY transaction completed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlePhyInitializationReport;

/// One post-enable timing observation in the retained always-awake epoch.
///
/// Only a completed generation-keyed controller-time request owned by an
/// initialized BLE PHY can create this affine token. It deliberately cannot be
/// copied or manufactured from a detached scheduler image. It proves no RF
/// wake or analog readiness; the supported profile keeps those transitions
/// outside the DTM event path.
#[must_use = "the always-awake timing observation must be consumed by DTM scheduling"]
#[cfg(target_arch = "riscv32")]
pub struct AlwaysAwakeTimingReady {
    scheduler_instant: oer_esp32s31_hal::bluetooth::engine::scheduler::time::SchedulerInstant,
}

#[cfg(target_arch = "riscv32")]
impl AlwaysAwakeTimingReady {
    fn from_completed_sample(
        epoch: crate::ControllerSchedulerEpoch,
        sample: crate::ControllerTimeSample,
    ) -> Self {
        Self {
            scheduler_instant:
                oer_esp32s31_hal::bluetooth::engine::scheduler::time::SchedulerInstant::from_image(
                    epoch.project_without_reanchor(&sample),
                ),
        }
    }

    /// Consume the post-enable timing proof into its microsecond-domain instant.
    pub const fn into_scheduler_instant(
        self,
    ) -> oer_esp32s31_hal::bluetooth::engine::scheduler::time::SchedulerInstant {
        self.scheduler_instant
    }
}

/// Exclusive BLE-PHY authority for completing always-awake timing samples.
///
/// This capability is created only while splitting a fully initialized BLE-PHY
/// owner. It remains private inside the published task service, so a scheduler
/// or detached controller-time sample cannot manufacture this ordered timing
/// observation. The authority makes no RF-readiness claim.
#[must_use = "the timing authority must remain owned by the BLE-PHY task service"]
#[cfg(target_arch = "riscv32")]
pub struct BlePhyTimingAuthority {
    le_1m_packet_start_calibration: BlePhyLe1MPacketStartCalibration,
}

#[cfg(target_arch = "riscv32")]
impl BlePhyTimingAuthority {
    fn new(le_1m_packet_start_calibration: BlePhyLe1MPacketStartCalibration) -> Self {
        Self {
            le_1m_packet_start_calibration,
        }
    }

    pub fn complete_always_awake(
        &mut self,
        epoch: crate::ControllerSchedulerEpoch,
        sample: crate::ControllerTimeSample,
    ) -> AlwaysAwakeTimingReady {
        AlwaysAwakeTimingReady::from_completed_sample(epoch, sample)
    }

    /// Scheduler-time packet start of one captured LE 1M packet, in
    /// microseconds normalized by this PHY's packet-start calibration.
    pub fn complete_le_1m_packet_start(
        &mut self,
        epoch: crate::ControllerSchedulerEpoch,
        captured: LePacketCapturedTime,
    ) -> u32 {
        let captured_micros = epoch.project_le_packet_capture(captured);
        self.le_1m_packet_start_calibration
            .normalize_controller_micros(captured_micros)
    }

    /// Scheduler-time packet start of one captured connection anchor, in
    /// microseconds normalized by this PHY's packet-start calibration.
    pub fn complete_le_1m_peripheral_connection_packet_start(
        &mut self,
        epoch: crate::ControllerSchedulerEpoch,
        captured: PeripheralConnectionCapturedAnchorTime,
    ) -> u32 {
        let captured_micros = epoch.project_peripheral_connection_capture(captured);
        self.le_1m_packet_start_calibration
            .normalize_controller_micros(captured_micros)
    }
}

/// Powered Controller after BLE PHY init and public-address publication.
///
/// The task's owning HAL engine retains the complete address-bound arena;
/// the shared actor retains its PHY membership, while this state retains
/// the affine standalone always-awake profile selection.
/// That selection performs no RF MMIO and is not an RF-ready or
/// completed-time proof. This state does not claim that an interrupt route,
/// packet engine, Link Layer role, advertising set, scanner, connection, or
/// HCI dataplane is operational.
#[must_use = "initialized BLE PHY retains every hardware and storage owner"]
#[cfg(target_arch = "riscv32")]
pub struct ControllerBlePhyEngineInitialized<
    'cells,
    const MODEM_TIMER_CAPACITY: usize,
    X: SignalExecutor,
    M: ControllerMemory,
> {
    controller:
        crate::low_power::ControllerLowPowerHardwareInitialized<'cells, MODEM_TIMER_CAPACITY, X, M>,
    report: BlePhyInitializationReport,
}

/// Hardware owners, software endpoints and retained graph of one activated
/// epoch.
#[cfg(target_arch = "riscv32")]
#[must_use = "every activated owner belongs to the running epoch"]
pub struct BlePhyRuntime<'runtime, const MT: usize, X: SignalExecutor, M: ControllerMemory> {
    /// Task, interrupt and modem-timer endpoints.
    pub endpoints: crate::low_power::ControllerRuntimeEndpoints<'runtime, MT, X, M>,
    /// The controller's interrupt bank, its output not prepared: the gate
    /// of its interrupt handler prepares it when the epoch opens.
    pub interrupts: oer_esp32s31_hal::bluetooth::InterruptSetupOwner,
    /// Initialized low-power timer hardware, not yet started: the gate of
    /// its interrupt handler starts it after the output is prepared.
    pub timer: ModemLpTimerLowPowerHardwareInitializedOwner,
    /// Packet-start calibration of this PHY.
    pub timing: BlePhyTimingAuthority,
}

#[cfg(target_arch = "riscv32")]
impl<'cells, const MODEM_TIMER_CAPACITY: usize, X: SignalExecutor, M: ControllerMemory>
    ControllerBlePhyEngineInitialized<'cells, MODEM_TIMER_CAPACITY, X, M>
{
    /// Observe completion of the source-owned normal BLE PHY transaction.
    pub const fn report(&self) -> BlePhyInitializationReport {
        self.report
    }

    /// Hand every owner of the epoch to its runtime role.
    ///
    /// The bank leaves this engine's custody here for the first time, its
    /// output not prepared, together with the unstarted timer: the gate of
    /// their interrupt handler prepares the output, then starts the timer,
    /// when the epoch opens, so no source is armed while the task holds its
    /// owner. This complete initialization is the matching Controller epoch.
    pub fn activate(self) -> BlePhyRuntime<'cells, MODEM_TIMER_CAPACITY, X, M> {
        let Self {
            mut controller,
            report: _,
        } = self;
        let interrupts: InterruptBankOwner = controller.take_interrupt_owner();
        let timer = controller.take_timer_hardware();
        let interrupts = interrupts.into_setup();
        let timing = BlePhyTimingAuthority::new(
            controller
                .task_mut()
                .engine()
                .le_1m_packet_start_calibration(),
        );
        BlePhyRuntime {
            endpoints: controller.split_runtime(),
            interrupts,
            timer,
            timing,
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<'cells, const MODEM_TIMER_CAPACITY: usize, X: SignalExecutor, M: ControllerMemory>
    ControllerLowPowerHardwareInitialized<'cells, MODEM_TIMER_CAPACITY, X, M>
{
    /// Publish the recovered BLE PHY register transaction while consuming and
    /// retaining the complete address-bound allocation graph. The BLE base
    /// stack enable also touches the shared radio registers, which `radio`
    /// lends.
    ///
    /// The transition is fail-stop after its first MMIO write. It has no
    /// caller-supplied raw address and no escape that releases the storage
    /// while hardware may still retain either pointer. `public_address`
    /// remains in canonical display order at this boundary; only the HAL owns
    /// conversion to Controller wire order. The returned state therefore
    /// proves both BLE PHY completion and public-address readiness.
    pub fn initialize_ble_phy_engine<B: PlatformClockBackend>(
        mut self,
        ready: oer_esp32s31_phy::bluetooth_client::BluetoothRfReady<'_, B>,
        public_address: BluetoothPublicDeviceAddress,
    ) -> Result<
        ControllerBlePhyEngineInitialized<'cells, MODEM_TIMER_CAPACITY, X, M>,
        ControllerBlePhyInitializationFailure<'cells, MODEM_TIMER_CAPACITY, X, M>,
    > {
        if let Err(error) = self.task_mut().engine().initialize_ble_phy(
            ready.into_radio(),
            ControllerPublicAddress::from_canonical_bytes(public_address.canonical_bytes()),
        ) {
            return Err(ControllerBlePhyInitializationFailure {
                error,
                _controller: self,
            });
        }
        let controller = self;
        Ok(ControllerBlePhyEngineInitialized {
            controller,
            report: BlePhyInitializationReport,
        })
    }
}

#[cfg(test)]
mod tests;

#[cfg(target_arch = "riscv32")]
#[must_use]
pub struct ControllerBlePhyInitializationFailure<
    'cells,
    const MT: usize,
    X: SignalExecutor,
    M: ControllerMemory,
> {
    pub error: oer_esp32s31_hal::bluetooth::engine::EngineInitError,
    _controller: ControllerLowPowerHardwareInitialized<'cells, MT, X, M>,
}
