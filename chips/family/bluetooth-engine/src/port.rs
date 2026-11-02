//! Register atoms for a safe BLE driver, with chip-owned associated receipts.
//!
//! Task and interrupt contracts are separate: no method transfers the P3
//! register owner to P2. A port exports no PAC, raw address, memory projection,
//! graph extraction or reclaim operation. Publication includes the existing
//! device fence. Diagnostic settling belongs to the current HAL coordinator.

/// Task-owned scheduler command atoms. Associated types retain the backend's
/// validated requests, list indices, observations and affine publication proofs.
///
/// A generic driver cannot reuse a consumed publication receipt:
/// ```compile_fail,E0382
/// use oer_espressif_bluetooth_engine::port::SchedulerTaskPort;
/// fn clear_twice<P: SchedulerTaskPort>(port: &mut P, receipt: P::LockPublished) {
///     port.clear_execution_lock(receipt);
///     port.clear_execution_lock(receipt);
/// }
/// ```
/// The two commands retain distinct associated receipts:
/// ```compile_fail,E0308
/// use oer_espressif_bluetooth_engine::port::SchedulerTaskPort;
/// fn wrong_command<P: SchedulerTaskPort>(port: &mut P, receipt: P::LockPublished) {
///     port.clear_execution_modify(receipt);
/// }
/// ```
/// A task port grants no interrupt-bank access:
/// ```compile_fail,E0599
/// use oer_espressif_bluetooth_engine::port::SchedulerTaskPort;
/// fn mask_from_task<P: SchedulerTaskPort>(port: &mut P) {
///     port.mask_run();
/// }
/// ```
pub trait SchedulerTaskPort {
    type LockRequest;
    type LockPublished;
    type LockDisposition;
    type ModifyPublished;
    type ListIndex;
    type Busy;
    type Stopped;

    /// Sample command zero, then command one only when zero is ready.
    fn commands_ready(&mut self) -> bool;
    fn publish_execution_lock(&mut self, request: Self::LockRequest) -> Self::LockPublished;
    fn observe_execution_lock(&mut self, busy: Self::Busy) -> Self::LockDisposition;
    /// Consume the publication receipt when clearing its command START.
    fn clear_execution_lock(&mut self, published: Self::LockPublished);
    fn publish_execution_modify(&mut self, index: Self::ListIndex) -> Self::ModifyPublished;
    fn publish_execution_modify_deletion(
        &mut self,
        index: Self::ListIndex,
    ) -> Self::ModifyPublished;
    fn execution_modify_ready(&mut self) -> bool;
    fn execution_modify_rejected(&mut self) -> bool;
    fn clear_execution_modify(&mut self, published: Self::ModifyPublished);
    fn disable_run_event(&mut self);
    fn request_stop(&mut self);
    /// Global stopped evidence never grants descriptor reclamation.
    fn confirm_stopped(&mut self, busy: Self::Busy) -> Option<Self::Stopped>;
}

/// P3-owned scheduler diagnostic atoms. An unsettled sample is not replaced by
/// an idle value. Selection, fresh sampling and fences retain their HAL owner.
pub trait SchedulerInterruptPort {
    type Busy;
    type LockModify;
    fn sample_busy(&mut self) -> Option<Self::Busy>;
    fn sample_lock_modify(&mut self) -> Option<Self::LockModify>;
    fn mask_run(&mut self);
    fn select_modify_progress(&mut self);
    fn sample_modify_repeats(&mut self) -> Option<bool>;
    fn select_modify_settle(&mut self);
    fn sample_modify_settled(&mut self) -> Option<bool>;
}

/// Sticky controller-time ownership: cancelling a borrower does not clear the
/// original request. One step performs at most one request-bit observation.
pub trait ControllerTimePort {
    type BeginError;
    type Step;
    type StepError;
    fn begin_controller_time_latch(&mut self) -> Result<(), Self::BeginError>;
    fn step_controller_time_latch(&mut self) -> Result<Self::Step, Self::StepError>;
}

/// Physical latch transactions below the durable controller-time owner.
/// Backends may include each fence in the neighboring accessor, as before.
pub trait ControllerTimeRegisters {
    type Request;
    fn publish_latch_request(&mut self, request: Self::Request);
    fn order_after_publication(&mut self);
    fn latch_request_pending(&mut self) -> bool;
    fn order_after_clear_observation(&mut self);
    fn read_latched_time_0(&mut self) -> u32;
}

/// The backend's existing affine terminal-output phases. Constructors and
/// captured enable images stay with that backend; no phase is minted here.
pub trait OutputPhases {
    type Error;
    type Admitted;
    type IdleSampled;
    type HeadsAbsent;
    type DynamicMasked;
    type RunDisabled;
    type Released;
}

/// P2's three terminal-output atoms, alternating with the permanent P3 owner.
/// An idle sample is consumed by the head check; the P3 mask is consumed by
/// RUN disable. These receipts authorize no arena reclamation or owner transfer.
///
/// ```compile_fail,E0308
/// use oer_espressif_bluetooth_engine::port::OutputTaskPort;
/// fn skip_handler<P: OutputTaskPort>(port: &mut P, admitted: P::Admitted) {
///     port.confirm_heads_absent(admitted);
/// }
/// ```
pub trait OutputTaskPort: OutputPhases {
    fn admit_release(&mut self) -> Result<Self::Admitted, Self::Error>;
    fn confirm_heads_absent(
        &mut self,
        sampled: Self::IdleSampled,
    ) -> Result<Self::HeadsAbsent, Self::Error>;
    fn disable_run(&mut self, masked: Self::DynamicMasked) -> Self::RunDisabled;
}

/// P3's existing terminal-output transactions, with the exact task receipts.
/// Sampled NRT admission precedes the last visible fault check before masking
/// and final release confirmation. Unconditional W1C excludes fault/overflow
/// lanes. Release verifies zero enables without reopening them or rerouting;
/// masked pending is not observable. Refusal never yields a released receipt.
/// These transactions retain the current HAL policy and diagnostic settles.
pub trait OutputInterruptPort: OutputPhases {
    fn sample_idle<C: oer_time::Clock + ?Sized>(
        &mut self,
        admitted: &Self::Admitted,
        settle: oer_time::settle::Settle<'_, C>,
    ) -> Result<Self::IdleSampled, Self::Error>;
    fn mask_dynamic(
        &mut self,
        heads: &Self::HeadsAbsent,
    ) -> Result<Self::DynamicMasked, Self::Error>;
    fn release_idle<C: oer_time::Clock + ?Sized>(
        &mut self,
        run: &Self::RunDisabled,
        settle: oer_time::settle::Settle<'_, C>,
    ) -> Result<Self::Released, Self::Error>;
}
