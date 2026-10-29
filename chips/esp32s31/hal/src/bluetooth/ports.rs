//! Bind the family contracts to the existing disjoint register owners.

use oer_espressif_bluetooth_engine::port::{SchedulerInterruptPort, SchedulerTaskPort};

use super::registers::*;

impl<P: BluetoothPlacements> SchedulerTaskPort for BluetoothTaskRegisters<P> {
    type LockRequest = BluetoothSchedulerExecutionLockRequest;
    type LockPublished = BluetoothSchedulerExecutionLockPublished;
    type LockDisposition = BluetoothSchedulerExecutionLockDisposition;
    type ModifyPublished = BluetoothSchedulerExecutionModifyPublished;
    type ListIndex = BluetoothSchedulerHardwareListIndex;
    type Busy = BluetoothSchedulerBusyObservation;
    type Stopped = BluetoothSchedulerStopped;

    fn commands_ready(&mut self) -> bool {
        self.scheduler_stop_commands_ready()
    }
    fn publish_execution_lock(&mut self, request: Self::LockRequest) -> Self::LockPublished {
        self.publish_scheduler_execution_lock(request)
    }
    fn observe_execution_lock(&mut self, busy: Self::Busy) -> Self::LockDisposition {
        self.observe_scheduler_execution_lock(busy)
    }
    fn clear_execution_lock(&mut self, _published: Self::LockPublished) {
        let _cleared =
            self.clear_scheduler_insertion_command_start(BluetoothSchedulerInsertionCommand::Zero);
    }
    fn publish_execution_modify(&mut self, index: Self::ListIndex) -> Self::ModifyPublished {
        self.publish_scheduler_execution_modify(index)
    }
    fn publish_execution_modify_deletion(
        &mut self,
        index: Self::ListIndex,
    ) -> Self::ModifyPublished {
        self.publish_scheduler_execution_modify_list_deletion(index)
    }
    fn execution_modify_ready(&mut self) -> bool {
        self.scheduler_execution_modify_ready()
    }
    fn execution_modify_rejected(&mut self) -> bool {
        self.scheduler_execution_modify_rejected()
    }
    fn clear_execution_modify(&mut self, _published: Self::ModifyPublished) {
        let _cleared =
            self.clear_scheduler_insertion_command_start(BluetoothSchedulerInsertionCommand::One);
    }
    fn disable_run_event(&mut self) {
        self.disable_scheduler_run_event();
    }
    fn request_stop(&mut self) {
        self.publish_scheduler_lifecycle_request();
    }
    fn confirm_stopped(&mut self, busy: Self::Busy) -> Option<Self::Stopped> {
        self.confirm_scheduler_stopped(busy)
    }
}

impl<P: BluetoothPlacements> SchedulerInterruptPort for BluetoothInterruptRegisters<P> {
    type Busy = BluetoothSchedulerBusyObservation;
    type LockModify = BluetoothSchedulerLockModifyInterruptObservation;

    fn sample_busy(&mut self) -> Option<Self::Busy> {
        self.capture_scheduler_busy()
    }
    fn sample_lock_modify(&mut self) -> Option<Self::LockModify> {
        self.capture_scheduler_lock_modify_interrupt()
    }
    fn mask_run(&mut self) {
        self.mask_scheduler_run_interrupts();
    }
    fn select_modify_progress(&mut self) {
        self.select_scheduler_execution_modify_progress();
    }
    fn sample_modify_repeats(&mut self) -> Option<bool> {
        self.scheduler_execution_modify_repeats()
    }
    fn select_modify_settle(&mut self) {
        self.select_scheduler_execution_modify_settle();
    }
    fn sample_modify_settled(&mut self) -> Option<bool> {
        self.scheduler_execution_modify_settled()
    }
}

#[cfg(feature = "bluetooth-engine")]
impl<M: super::engine::ControllerMemory> oer_espressif_bluetooth_engine::port::ControllerTimePort
    for super::engine::Engine<M>
{
    type BeginError = super::BluetoothControllerTimeLatchBeginError;
    type Step = super::BluetoothControllerTimeLatchStep;
    type StepError = super::BluetoothControllerTimeLatchStepError;

    fn begin_controller_time_latch(&mut self) -> Result<(), Self::BeginError> {
        Self::begin_controller_time_latch(self)
    }
    fn step_controller_time_latch(&mut self) -> Result<Self::Step, Self::StepError> {
        Self::step_controller_time_latch(self)
    }
}

macro_rules! output_phases {
    ($owner:ty) => {
        impl oer_espressif_bluetooth_engine::port::OutputPhases for $owner {
            type Error = super::BluetoothControllerOutputReleaseError;
            type Admitted = super::BluetoothOutputReleaseAdmitted;
            type IdleSampled = super::BluetoothOutputIdleSampled;
            type HeadsAbsent = super::BluetoothSchedulerHeadsAbsent;
            type DynamicMasked = super::BluetoothDynamicSourcesMasked;
            type RunDisabled = super::BluetoothSchedulerRunDisabled;
            type Released = super::BluetoothOutputReleased;
        }
    };
}
output_phases!(super::TaskOwner);
output_phases!(super::InterruptRegistersOwner);

impl oer_espressif_bluetooth_engine::port::OutputTaskPort for super::TaskOwner {
    fn admit_release(&mut self) -> Result<Self::Admitted, Self::Error> {
        self.admit_output_release()
    }
    fn confirm_heads_absent(
        &mut self,
        sampled: Self::IdleSampled,
    ) -> Result<Self::HeadsAbsent, Self::Error> {
        self.confirm_scheduler_heads_absent(sampled)
    }
    fn disable_run(&mut self, masked: Self::DynamicMasked) -> Self::RunDisabled {
        self.disable_scheduler_run(masked)
    }
}

impl oer_espressif_bluetooth_engine::port::OutputInterruptPort for super::InterruptRegistersOwner {
    fn sample_idle<C: oer_time::Clock + ?Sized>(
        &mut self,
        admitted: &Self::Admitted,
        settle: oer_time::settle::Settle<'_, C>,
    ) -> Result<Self::IdleSampled, Self::Error> {
        self.sample_idle_output(admitted, settle)
    }
    fn mask_dynamic(
        &mut self,
        heads: &Self::HeadsAbsent,
    ) -> Result<Self::DynamicMasked, Self::Error> {
        self.mask_output_dynamic_sources(heads)
    }
    fn release_idle<C: oer_time::Clock + ?Sized>(
        &mut self,
        run: &Self::RunDisabled,
        settle: oer_time::settle::Settle<'_, C>,
    ) -> Result<Self::Released, Self::Error> {
        self.release_idle_output(run, settle)
    }
}
