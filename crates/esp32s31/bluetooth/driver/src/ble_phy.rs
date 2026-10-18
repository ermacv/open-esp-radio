//! Owned BLE PHY engine activation after common PHY and BTBB initialization.

#[cfg(target_arch = "riscv32")]
use crate::{phy::ControllerPhyJoined, resources::InterruptBankOwner};
#[cfg(any(target_arch = "riscv32", test))]
use oer_bluetooth_hci::BluetoothPublicDeviceAddress;
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_bluetooth_memory::BlePhyLe1MPacketStartCalibration;
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_bluetooth_memory::{
    BlePhyEngineCpuOwned, DirectionFindingWorkspaceCpuOwned, DirectionFindingWorkspaceLink,
    LePacketCapturedTime, PeripheralConnectionCapturedAnchorTime,
};
#[cfg(target_arch = "riscv32")]
use oer_esp32s31_hal::bluetooth::ModemLpTimerLowPowerHardwareInitializedOwner;
#[cfg(any(target_arch = "riscv32", test))]
use oer_esp32s31_hal::{bluetooth::ControllerPublicAddress, types::BluetoothPhyRegisterInitInputs};

/// Controller-global DF storage after its disabled-CTE descriptor is visible to hardware.
///
/// The memory and exact PAC publication proof remain joined for the complete
/// powered epoch. Only an opaque ordinary-role link can be copied out; CPU
/// access and raw descriptor images remain inaccessible.
#[must_use = "hardware-owned direction-finding storage must remain retained"]
#[cfg(target_arch = "riscv32")]
pub(crate) struct DirectionFindingWorkspaceHardwareOwned {
    pub(crate) storage: DirectionFindingWorkspaceCpuOwned,
    pub(crate) _publication: oer_esp32s31_hal::bluetooth::DirectionFindingDisabledBaselineOwner,
}

#[cfg(target_arch = "riscv32")]
impl DirectionFindingWorkspaceHardwareOwned {
    const fn link(&self) -> DirectionFindingWorkspaceLink {
        self.storage.binding().link()
    }
}

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
    scheduler_instant: crate::SchedulerInstant,
}

#[cfg(target_arch = "riscv32")]
impl AlwaysAwakeTimingReady {
    fn from_completed_sample(
        epoch: crate::ControllerSchedulerEpoch,
        sample: crate::ControllerTimeSample,
    ) -> Self {
        Self {
            scheduler_instant: crate::SchedulerInstant::from_image(
                epoch.project_without_reanchor(&sample),
            ),
        }
    }

    /// Consume the post-enable timing proof into its microsecond-domain instant.
    pub const fn into_scheduler_instant(self) -> crate::SchedulerInstant {
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
/// The address-bound environment, resolving-list storage and the PHY
/// membership stay together in [`BlePhyRetainedOwners`] for the whole
/// hardware epoch, with the affine standalone always-awake profile selection.
/// That selection performs no RF MMIO and is not an RF-ready or
/// completed-time proof. This state does not claim that an interrupt route,
/// packet engine, Link Layer role, advertising set, scanner, connection, or
/// HCI dataplane is operational.
#[must_use = "initialized BLE PHY retains every hardware and storage owner"]
#[cfg(target_arch = "riscv32")]
pub struct ControllerBlePhyEngineInitialized<'cells, const MODEM_TIMER_CAPACITY: usize> {
    controller:
        crate::low_power::ControllerLowPowerHardwareInitialized<'cells, MODEM_TIMER_CAPACITY>,
    retained: BlePhyRetainedOwners,
    report: BlePhyInitializationReport,
}

/// PHY membership and the BLE PHY/DF allocation graph still referenced by
/// the Controller.
///
/// The runtime split hands it out by value; the Controller shutdown consumes
/// it, leaving the domain and returning the allocations only after the
/// Controller reset.
#[cfg(target_arch = "riscv32")]
#[must_use = "the BLE PHY graph must return through the Controller shutdown"]
pub struct BlePhyRetainedOwners {
    pub(crate) membership: oer_esp32s31_phy::bluetooth_client::BluetoothPhyMembership,
    pub(crate) storage: BlePhyEngineCpuOwned,
    pub(crate) direction_finding: DirectionFindingWorkspaceHardwareOwned,
}

/// Hardware owners, software endpoints and retained graph of one activated
/// epoch.
#[cfg(target_arch = "riscv32")]
#[must_use = "every activated owner belongs to the running epoch"]
pub struct BlePhyRuntime<'runtime, const MT: usize> {
    /// Task, interrupt and modem-timer endpoints.
    pub endpoints: crate::low_power::ControllerRuntimeEndpoints<'runtime, MT>,
    /// Controller interrupt output, prepared for stable ISR storage.
    pub output: oer_esp32s31_hal::bluetooth::InterruptOutputPreparedOwner,
    /// Initialized low-power timer hardware, not yet started.
    pub timer: ModemLpTimerLowPowerHardwareInitializedOwner,
    /// Packet-start calibration of this PHY.
    pub timing: BlePhyTimingAuthority,
    /// PHY membership and BLE PHY/DF storage, for the shutdown.
    pub retained: BlePhyRetainedOwners,
    /// The DF workspace link ordinary roles reference.
    pub direction_finding: DirectionFindingWorkspaceLink,
}

#[cfg(target_arch = "riscv32")]
impl<'cells, const MODEM_TIMER_CAPACITY: usize>
    ControllerBlePhyEngineInitialized<'cells, MODEM_TIMER_CAPACITY>
{
    /// Observe completion of the source-owned normal BLE PHY transaction.
    pub const fn report(&self) -> BlePhyInitializationReport {
        self.report
    }

    /// Prepare the controller interrupt output and hand every owner of the
    /// epoch to its runtime role.
    ///
    /// The bank leaves this engine's custody here for the first time, so no
    /// CPU-route installer can have received it; this complete initialization
    /// is the matching Controller epoch. The timer is returned unstarted and
    /// only with the prepared output, so it cannot be started before the
    /// output.
    #[allow(
        unsafe_code,
        reason = "the completed epoch and first custody transfer discharge the output contract"
    )]
    pub fn activate(self) -> BlePhyRuntime<'cells, MODEM_TIMER_CAPACITY> {
        let Self {
            mut controller,
            retained,
            report: _,
        } = self;
        let interrupts: InterruptBankOwner = controller.take_interrupt_owner();
        let timer = controller.take_timer_hardware();
        // SAFETY: the owner is the completed matching Controller
        // initialization, and the bank has just left its one-shot custody,
        // so all three CPU routes remain inactive.
        let output = unsafe { interrupts.prepare_controller_output() };
        let timing = BlePhyTimingAuthority::new(retained.storage.le_1m_packet_start_calibration());
        let direction_finding = retained.direction_finding.link();
        BlePhyRuntime {
            endpoints: controller.split_runtime(),
            output,
            timer,
            timing,
            retained,
            direction_finding,
        }
    }
}

#[cfg(target_arch = "riscv32")]
impl<'cells, const MODEM_TIMER_CAPACITY: usize> ControllerPhyJoined<'cells, MODEM_TIMER_CAPACITY> {
    /// Publish the recovered BLE PHY register transaction while consuming and
    /// retaining the complete address-bound allocation graph. The BLE base
    /// stack enable also touches the shared radio registers, which `lease`
    /// lends.
    ///
    /// The transition is fail-stop after its first MMIO write. It has no
    /// caller-supplied raw address and no escape that releases the storage
    /// while hardware may still retain either pointer. `public_address`
    /// remains in canonical display order at this boundary; only the HAL owns
    /// conversion to Controller wire order. The returned state therefore
    /// proves both BLE PHY completion and public-address readiness.
    #[allow(
        unsafe_code,
        reason = "this affine state and consumed storage prove the narrow HAL bridge prerequisites"
    )]
    pub fn initialize_ble_phy_engine<T>(
        mut self,
        lease: &mut oer_esp32s31_hal::shared_radio::SharedRadioLease<'_, T>,
        storage: BlePhyEngineCpuOwned,
        direction_finding: DirectionFindingWorkspaceCpuOwned,
        public_address: BluetoothPublicDeviceAddress,
    ) -> ControllerBlePhyEngineInitialized<'cells, MODEM_TIMER_CAPACITY> {
        let controller = &mut self.controller;
        let report = apply_register_init_then_public_address(
            &storage,
            public_address,
            controller,
            |controller, inputs| {
                // SAFETY: `self` retains the powered scheduler, low-power
                // owners and the PHY/BTBB membership. `storage` is a consumed
                // static owner for the complete published allocation graph
                // and is moved into the return state.
                unsafe {
                    controller
                        .task_mut()
                        .enable_ble_base_stack_hardware(lease, inputs);
                }
            },
            |controller, address| {
                controller.task_mut().program_public_device_address(address);
            },
        );
        let descriptor = direction_finding
            .binding()
            .disabled_cte_descriptor_address();
        // SAFETY: the complete powered Controller remains in `self`; the
        // initialized static workspace is consumed into the returned state,
        // which retains it together with the exact PAC publication proof.
        let publication = unsafe {
            self.controller
                .task_mut()
                .prepare_direction_finding_disabled_baseline(descriptor)
        };

        let ControllerPhyJoined {
            controller,
            membership,
        } = self;
        ControllerBlePhyEngineInitialized {
            controller,
            retained: BlePhyRetainedOwners {
                membership,
                storage,
                direction_finding: DirectionFindingWorkspaceHardwareOwned {
                    storage: direction_finding,
                    _publication: publication,
                },
            },
            report,
        }
    }
}

#[cfg(any(target_arch = "riscv32", test))]
fn apply_register_init_then_public_address<T>(
    storage: &oer_esp32s31_bluetooth_memory::BlePhyEngineCpuOwned,
    public_address: BluetoothPublicDeviceAddress,
    target: &mut T,
    initialize: impl FnOnce(&mut T, BluetoothPhyRegisterInitInputs),
    publish_address: impl FnOnce(&mut T, ControllerPublicAddress),
) -> BlePhyInitializationReport {
    let report = apply_register_init(storage, |inputs| initialize(target, inputs));
    publish_address(
        target,
        ControllerPublicAddress::from_canonical_bytes(public_address.canonical_bytes()),
    );
    report
}

#[cfg(any(target_arch = "riscv32", test))]
fn apply_register_init(
    storage: &oer_esp32s31_bluetooth_memory::BlePhyEngineCpuOwned,
    initialize: impl FnOnce(BluetoothPhyRegisterInitInputs),
) -> BlePhyInitializationReport {
    let binding = storage.binding();
    initialize(BluetoothPhyRegisterInitInputs::normal_controller_profile(
        binding.environment_address(),
        binding.resolving_list_address(),
    ));
    BlePhyInitializationReport
}

#[cfg(test)]
mod tests;
