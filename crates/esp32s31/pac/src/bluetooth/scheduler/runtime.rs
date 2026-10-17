//! Restricted scheduler MMIO shared by the primary ISR and its worker.

#![deny(unsafe_code)]

use crate::{
    BluetoothInterruptRegisters, BluetoothSchedulerHardwareListHead,
    BluetoothSchedulerHardwareListHeadEmptyObserved, BluetoothTaskRegisters, device_fence,
    svd::{zero_based_field_write, zero_register_write},
};

/// Scheduler BUSY sampled through the diagnostic pair for bank-one source 3.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerReferenceGateObservation {
    busy: bool,
}

impl BluetoothSchedulerReferenceGateObservation {
    const fn new(busy: bool) -> Self {
        Self { busy }
    }

    /// Construct a semantic gate observation for upper-layer validation.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub const fn from_busy_for_validation(busy: bool) -> Self {
        Self::new(busy)
    }

    /// Whether the scheduler was busy at the reference-gate temporal point.
    pub const fn is_busy(self) -> bool {
        self.busy
    }
}

/// Affine proof that the source-124 reference-clear MMIO and trailing device
/// fence completed.
///
/// The vendor follows this hardware edge with a check of its private intrusive
/// scheduler lists. The open scheduler represents that software invariant with
/// affine states instead, so this token proves only the ordered hardware edge.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the completed reference clear must precede the later scheduler observation"]
pub struct BluetoothSchedulerReferenceCleared {
    _private: (),
}

/// Later field-level `SCHEDULER_STATE` observation used for deferred work.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the scheduler work observation must be consumed by one controller path"]
pub struct BluetoothSchedulerWorkObservation {
    busy: bool,
    state_29: bool,
    current_hardware_list: BluetoothSchedulerHardwareListIndex,
}

impl BluetoothSchedulerWorkObservation {
    const fn new(
        busy: bool,
        state_29: bool,
        current_hardware_list: BluetoothSchedulerHardwareListIndex,
    ) -> Self {
        Self {
            busy,
            state_29,
            current_hardware_list,
        }
    }

    /// Construct semantic deferred-work fields for upper-layer validation.
    #[cfg(any(feature = "validation-probes", test))]
    #[doc(hidden)]
    pub const fn from_fields_for_validation(
        busy: bool,
        state_29: bool,
        current_hardware_list: u8,
    ) -> Self {
        assert!(current_hardware_list < 16);
        Self::new(
            busy,
            state_29,
            BluetoothSchedulerHardwareListIndex(current_hardware_list),
        )
    }

    /// Whether the scheduler was busy at the deferred-work temporal point.
    pub const fn is_busy(&self) -> bool {
        self.busy
    }

    /// Whether the reviewed deferred-work predicate is true at this point.
    pub const fn deferred_work_requested(&self) -> bool {
        self.busy && self.state_29
    }

    /// Hardware-list index captured from the same scheduler-state sample.
    pub const fn current_hardware_list(&self) -> BluetoothSchedulerHardwareListIndex {
        self.current_hardware_list
    }
}

/// Scheduler BUSY sampled through the diagnostic pair by a task-side
/// transaction.
///
/// One sample publishes the scheduler-status selection and reads the complete
/// diagnostic value twice; the observation exists only when both reads agree.
/// The task borrows the interrupt owner for the sample, so no interrupt-side
/// selection can separate the selector write from its value reads.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the scheduler BUSY sample must be consumed by one controller path"]
pub struct BluetoothSchedulerBusyObservation {
    busy: bool,
}

impl BluetoothSchedulerBusyObservation {
    pub(crate) const fn new(busy: bool) -> Self {
        Self { busy }
    }

    /// Construct a semantic BUSY sample for upper-layer validation.
    #[cfg(any(feature = "validation-probes", test))]
    #[doc(hidden)]
    pub const fn from_busy_for_validation(busy: bool) -> Self {
        Self::new(busy)
    }

    /// Whether the scheduler was busy when this sample was accepted.
    pub const fn is_busy(&self) -> bool {
        self.busy
    }

    /// Consume this sample at the software-list removal gate.
    ///
    /// A busy sample returns immediately and cannot authorize command-register
    /// reads. Only an idle sample yields the affine capability required by the
    /// task owner for the remaining short-circuit transaction.
    pub const fn into_software_list_removal_gate(
        self,
    ) -> BluetoothSchedulerSoftwareListRemovalInterruptStep {
        if self.busy {
            BluetoothSchedulerSoftwareListRemovalInterruptStep::Pending
        } else {
            BluetoothSchedulerSoftwareListRemovalInterruptStep::Idle(
                BluetoothSchedulerSoftwareListRemovalIdle { _private: () },
            )
        }
    }
}

/// Finished hardware-list mask transferred by one scheduler-worker pop attempt.
///
/// The observation is affine because copying it would permit the same
/// transferred bit to mint more than one list token.
///
/// ```compile_fail
/// use oer_esp32s31_pac::BluetoothSchedulerFinishedListObservation;
///
/// fn cannot_reuse(observation: BluetoothSchedulerFinishedListObservation) {
///     let first = observation.pop_lowest();
///     let second = observation.pop_lowest();
///     drop((first, second));
/// }
/// ```
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the scheduler finished-list observation must be drained or explicitly discarded"]
pub struct BluetoothSchedulerFinishedListObservation(u16);

/// Affine permission for the task owner to continue one software-list removal
/// observation after a fresh interrupt-side sample found the scheduler idle.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the idle removal capability must be consumed by the task-side capture"]
pub struct BluetoothSchedulerSoftwareListRemovalIdle {
    _private: (),
}

/// Interrupt-side result of consuming one fresh scheduler-state sample.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "pending requires a new event; idle must continue through the task owner"]
pub enum BluetoothSchedulerSoftwareListRemovalInterruptStep {
    /// Scheduler BUSY was set; no command register may be read for this event.
    Pending,
    /// Scheduler BUSY was clear; task-owned command reads may now continue.
    Idle(BluetoothSchedulerSoftwareListRemovalIdle),
}

/// Affine proof that one fresh scheduler event reached the complete reviewed
/// software-list removal predicate.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the ready removal proof must advance the exact retained scheduler item"]
pub struct BluetoothSchedulerSoftwareListRemovalReady {
    head: BluetoothSchedulerHardwareListHeadEmptyObserved,
}

impl BluetoothSchedulerSoftwareListRemovalReady {
    /// Hardware list retained by the exact post-completion empty-head proof.
    pub const fn index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.head.index()
    }

    /// Originally published head retained by the completed RUN epoch.
    pub const fn completed_head(&self) -> BluetoothSchedulerHardwareListHead {
        self.head.completed_head()
    }
}

#[cfg(feature = "validation-probes")]
impl BluetoothSchedulerSoftwareListRemovalReady {
    /// Bind host-only semantic empty-head evidence to a simulated ready gate.
    #[doc(hidden)]
    pub const fn from_head_for_validation(
        head: BluetoothSchedulerHardwareListHeadEmptyObserved,
    ) -> Self {
        Self { head }
    }
}

/// Result of consuming one finite scheduler software-list removal observation.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "pending requires a new scheduler event; ready must advance ownership"]
pub enum BluetoothSchedulerSoftwareListRemovalJoin {
    /// The scheduler was busy or at least one task-owned command status was
    /// not ready.
    Pending {
        /// Unchanged empty-head proof for a later fresh scheduler event.
        head: BluetoothSchedulerHardwareListHeadEmptyObserved,
    },
    /// Both task-owned command statuses were set after the idle observation.
    Ready(BluetoothSchedulerSoftwareListRemovalReady),
}

/// One of the sixteen scheduler hardware-list indices.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerHardwareListIndex(u8);

impl BluetoothSchedulerHardwareListIndex {
    /// The first scheduler hardware list.
    pub const ZERO: Self = Self(0);

    /// Validate one zero-based hardware-list index.
    pub const fn new(index: u8) -> Option<Self> {
        if index < 16 { Some(Self(index)) } else { None }
    }

    /// Return the zero-based hardware-list index in `0..16`.
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// Result of consuming at most one list from a captured finished-list set.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the selected list or remaining observation must be handled"]
pub enum BluetoothSchedulerFinishedListPop {
    /// The captured observation contains no remaining finished list.
    Complete,
    /// The lowest-numbered finished list was selected.
    List {
        /// Affine proof that this hardware list occurred in the transferred set.
        observed: BluetoothSchedulerFinishedHardwareListObserved,
        /// Remaining captured lists after consuming `observed`.
        remaining: BluetoothSchedulerFinishedListObservation,
    },
}

/// Affine proof that one list occurred in a fenced finished-list transfer.
///
/// This token is not an item-completion proof. It authorizes the owning
/// scheduler layer to rescan exactly one retained list after the PAC's
/// `status -> report -> device fence` transaction. The private constructor and
/// absence of `Clone`/`Copy` prevent a positional index from being substituted
/// for that temporal event. A fresh later transfer may observe the same list
/// again; global acknowledgement and item completion require the upper affine
/// scheduler epoch.
///
/// ```compile_fail
/// use oer_esp32s31_pac::BluetoothSchedulerFinishedHardwareListObserved;
///
/// fn cannot_reuse(observed: BluetoothSchedulerFinishedHardwareListObserved) {
///     let first_consumer = observed;
///     let second_consumer = observed;
///     drop((first_consumer, second_consumer));
/// }
/// ```
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the observed finished list must be matched to an owned scheduler list"]
pub struct BluetoothSchedulerFinishedHardwareListObserved {
    index: BluetoothSchedulerHardwareListIndex,
}

impl BluetoothSchedulerFinishedHardwareListObserved {
    /// Hardware-list index carried by this exact transferred observation.
    pub const fn index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.index
    }
}

impl BluetoothSchedulerFinishedListObservation {
    /// Construct a semantic set of finished hardware lists for host-only
    /// controller tests.
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn from_lists_for_validation(lists: &[u8]) -> Option<Self> {
        let mut image = 0u16;
        for &list in lists {
            if list >= 16 {
                return None;
            }
            image |= 1u16 << list;
        }
        Some(Self(image))
    }

    /// Whether no hardware list was reported finished.
    pub const fn is_empty(&self) -> bool {
        self.0 == 0
    }

    /// Consume at most one lowest-numbered finished list.
    ///
    /// The physical mask layout remains private to the PAC. Callers retain an
    /// opaque observation between executor turns and receive only a semantic
    /// list index for item selection.
    pub const fn pop_lowest(self) -> BluetoothSchedulerFinishedListPop {
        if self.0 == 0 {
            BluetoothSchedulerFinishedListPop::Complete
        } else {
            let index = self.0.trailing_zeros() as u8;
            BluetoothSchedulerFinishedListPop::List {
                observed: BluetoothSchedulerFinishedHardwareListObserved {
                    index: BluetoothSchedulerHardwareListIndex(index),
                },
                remaining: Self(self.0 & !(1u16 << index)),
            }
        }
    }
}

trait BluetoothSchedulerInterruptControl {
    fn read_scheduler_busy(&mut self) -> Option<bool>;
    fn read_scheduler_state(&mut self) -> SchedulerStateObservation;
    fn clear_scheduler_reference(&mut self);
}

/// The complete selector images the scheduler transactions publish.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DiagnosticSelector {
    SchedulerStatus,
    ExecutionModifyProgress,
    ExecutionModifySettle,
}

/// Selector and value accesses of the MAC diagnostic pair.
trait BluetoothDiagnosticControl {
    fn select(&mut self, selector: DiagnosticSelector);
    fn read_diagnostic_value(&mut self) -> DiagnosticValue;
}

/// One complete diagnostic-value sample, compared field by field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DiagnosticValue {
    value_0_low_7: u8,
    value_0_bit_7: bool,
    value_1: u8,
    value_2: u8,
    value_3: u8,
}

/// Select the scheduler-status signal and return its BUSY bit, or `None`
/// when the two reads disagree.
///
/// As in the vendor sample, no fence separates the selector write from the
/// reads. The vendor repeats the reads until they agree; that retry, bounded,
/// belongs to the HAL.
fn execute_scheduler_status_sample(control: &mut impl BluetoothDiagnosticControl) -> Option<bool> {
    control.select(DiagnosticSelector::SchedulerStatus);
    agreed_diagnostic_value(control).map(|value| value.value_0_bit_7)
}

/// Read the selected value twice and return it when both complete reads are
/// equal. The value crosses from the MAC clock domain, so a disagreeing pair
/// carries no sample.
fn agreed_diagnostic_value(
    control: &mut impl BluetoothDiagnosticControl,
) -> Option<DiagnosticValue> {
    let first = control.read_diagnostic_value();
    (first == control.read_diagnostic_value()).then_some(first)
}

/// Whether execution modify must repeat its request: with the progress
/// selector, lane-zero bits 2:0 hold 2, or hold 3 while lane-one bits 5:4
/// hold 1 (complete-word bits 13:12 of the vendor comparison).
const fn execution_modify_repeats(value: DiagnosticValue) -> bool {
    let state = value.value_0_low_7 & 0x7;
    state == 2 || (state == 3 && (value.value_1 >> 4) & 0x3 == 1)
}

/// Whether execution modify settled: with the settle selector, the complete
/// value differs from 9.
const fn execution_modify_settled(value: DiagnosticValue) -> bool {
    !(value.value_0_low_7 == 9
        && !value.value_0_bit_7
        && value.value_1 == 0
        && value.value_2 == 0
        && value.value_3 == 0)
}

struct HardwareDiagnosticControl<'a> {
    registers: &'a crate::svd::BluetoothSchedulerInterruptRuntime,
}

impl BluetoothDiagnosticControl for HardwareDiagnosticControl<'_> {
    fn select(&mut self, selector: DiagnosticSelector) {
        match selector {
            DiagnosticSelector::SchedulerStatus => {
                crate::svd::fixed_register_write::select_bluetooth_scheduler_status_diagnostic(
                    self.registers,
                );
            }
            DiagnosticSelector::ExecutionModifyProgress => {
                crate::svd::fixed_register_write::select_bluetooth_scheduler_execution_modify_progress_diagnostic(
                    self.registers,
                );
            }
            DiagnosticSelector::ExecutionModifySettle => {
                crate::svd::fixed_register_write::select_bluetooth_scheduler_execution_modify_settle_diagnostic(
                    self.registers,
                );
            }
        }
    }

    fn read_diagnostic_value(&mut self) -> DiagnosticValue {
        let (value_0_low_7, value_0_bit_7, value_1, value_2, value_3) =
            crate::svd::field_snapshot_read::observe_bluetooth_diagnostic_value(self.registers);
        DiagnosticValue {
            value_0_low_7,
            value_0_bit_7,
            value_1,
            value_2,
            value_3,
        }
    }
}

/// Sample scheduler BUSY once through the diagnostic pair of the interrupt
/// owner; `None` when the two reads disagree.
pub(crate) fn sample_scheduler_busy(
    registers: &crate::svd::BluetoothSchedulerInterruptRuntime,
) -> Option<bool> {
    execute_scheduler_status_sample(&mut HardwareDiagnosticControl { registers })
}

/// Select the execution-modify progress signal, as the vendor does before it
/// publishes the request.
pub(crate) fn select_execution_modify_progress(
    registers: &crate::svd::BluetoothSchedulerInterruptRuntime,
) {
    HardwareDiagnosticControl { registers }.select(DiagnosticSelector::ExecutionModifyProgress);
}

/// Read the already selected progress signal twice and report whether
/// execution modify must repeat its request; `None` when the reads disagree.
pub(crate) fn sample_execution_modify_repeats(
    registers: &crate::svd::BluetoothSchedulerInterruptRuntime,
) -> Option<bool> {
    agreed_diagnostic_value(&mut HardwareDiagnosticControl { registers })
        .map(execution_modify_repeats)
}

/// Select the execution-modify settle signal.
pub(crate) fn select_execution_modify_settle(
    registers: &crate::svd::BluetoothSchedulerInterruptRuntime,
) {
    HardwareDiagnosticControl { registers }.select(DiagnosticSelector::ExecutionModifySettle);
}

/// Read the already selected settle signal twice and report whether
/// execution modify settled; `None` when the reads disagree.
pub(crate) fn sample_execution_modify_settled(
    registers: &crate::svd::BluetoothSchedulerInterruptRuntime,
) -> Option<bool> {
    agreed_diagnostic_value(&mut HardwareDiagnosticControl { registers })
        .map(execution_modify_settled)
}

#[derive(Clone, Copy)]
struct SchedulerStateObservation {
    busy: bool,
    state_29: bool,
    current_hardware_list: BluetoothSchedulerHardwareListIndex,
}

struct HardwareSchedulerInterruptControl<'a> {
    registers: &'a crate::svd::BluetoothSchedulerInterruptRuntime,
}

impl BluetoothSchedulerInterruptControl for HardwareSchedulerInterruptControl<'_> {
    fn read_scheduler_busy(&mut self) -> Option<bool> {
        sample_scheduler_busy(self.registers)
    }

    fn read_scheduler_state(&mut self) -> SchedulerStateObservation {
        let (busy, state_29, current_link_index) =
            crate::svd::field_snapshot_read::observe_bluetooth_scheduler_interrupt_state(
                self.registers,
            );
        SchedulerStateObservation {
            busy,
            state_29,
            current_hardware_list: BluetoothSchedulerHardwareListIndex(current_link_index),
        }
    }

    fn clear_scheduler_reference(&mut self) {
        zero_register_write::clear_bluetooth_scheduler_reference(self.registers);
    }
}

trait BluetoothSchedulerFinishedListControl {
    fn read_finished_list_status(&mut self) -> u16;
    fn write_finished_list_report(&mut self, value: u16);
}

trait BluetoothSchedulerSoftwareListRemovalControl {
    fn read_command_0_status_26(&mut self) -> bool;
    fn read_command_1_status_18(&mut self) -> bool;
}

struct HardwareSchedulerSoftwareListRemovalControl<'a> {
    registers: &'a crate::svd::BluetoothControllerCore,
}

impl BluetoothSchedulerSoftwareListRemovalControl
    for HardwareSchedulerSoftwareListRemovalControl<'_>
{
    fn read_command_0_status_26(&mut self) -> bool {
        crate::svd::field_read::observe_bluetooth_scheduler_software_list_command_0_status_26(
            self.registers,
        )
    }

    fn read_command_1_status_18(&mut self) -> bool {
        crate::svd::field_read::observe_bluetooth_scheduler_software_list_command_1_status_18(
            self.registers,
        )
    }
}

struct HardwareSchedulerFinishedListControl<'a> {
    registers: &'a crate::svd::BluetoothControllerCore,
}

impl BluetoothSchedulerFinishedListControl for HardwareSchedulerFinishedListControl<'_> {
    fn read_finished_list_status(&mut self) -> u16 {
        crate::svd::field_read::observe_bluetooth_scheduler_finished_lists(self.registers)
    }

    fn write_finished_list_report(&mut self, value: u16) {
        zero_based_field_write::bluetooth_scheduler_finished_list_report(self.registers, value);
    }
}

fn execute_reference_gate_observation(
    control: &mut impl BluetoothSchedulerInterruptControl,
) -> Option<BluetoothSchedulerReferenceGateObservation> {
    control
        .read_scheduler_busy()
        .map(BluetoothSchedulerReferenceGateObservation::new)
}

fn execute_clear_scheduler_reference(control: &mut impl BluetoothSchedulerInterruptControl) {
    control.clear_scheduler_reference();
}

fn execute_work_observation(
    control: &mut impl BluetoothSchedulerInterruptControl,
) -> BluetoothSchedulerWorkObservation {
    let state = control.read_scheduler_state();
    BluetoothSchedulerWorkObservation::new(state.busy, state.state_29, state.current_hardware_list)
}

fn execute_finished_list_transfer(
    control: &mut impl BluetoothSchedulerFinishedListControl,
) -> BluetoothSchedulerFinishedListObservation {
    let value = control.read_finished_list_status();
    control.write_finished_list_report(value);
    BluetoothSchedulerFinishedListObservation(value)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BluetoothSchedulerSoftwareListRemovalDisposition {
    Pending,
    Ready,
}

fn execute_software_list_removal_finish(
    control: &mut impl BluetoothSchedulerSoftwareListRemovalControl,
) -> BluetoothSchedulerSoftwareListRemovalDisposition {
    let command_0_status_26 = control.read_command_0_status_26();
    let command_1_status_18 = command_0_status_26 && control.read_command_1_status_18();
    if command_0_status_26 && command_1_status_18 {
        BluetoothSchedulerSoftwareListRemovalDisposition::Ready
    } else {
        BluetoothSchedulerSoftwareListRemovalDisposition::Pending
    }
}

fn join_software_list_removal(
    head: BluetoothSchedulerHardwareListHeadEmptyObserved,
    disposition: BluetoothSchedulerSoftwareListRemovalDisposition,
) -> BluetoothSchedulerSoftwareListRemovalJoin {
    match disposition {
        BluetoothSchedulerSoftwareListRemovalDisposition::Pending => {
            BluetoothSchedulerSoftwareListRemovalJoin::Pending { head }
        }
        BluetoothSchedulerSoftwareListRemovalDisposition::Ready => {
            BluetoothSchedulerSoftwareListRemovalJoin::Ready(
                BluetoothSchedulerSoftwareListRemovalReady { head },
            )
        }
    }
}

impl BluetoothInterruptRegisters {
    /// Sample scheduler BUSY once through the diagnostic pair for bank-one
    /// source 3.
    ///
    /// `None` means the two diagnostic reads disagreed; the HAL owns the
    /// bounded retry. The later work observation is intentionally a separate MMIO method:
    /// the complete source-124 handler can clear `SCHEDULER_REFERENCE` between
    /// the two samples, and it reads `SCHEDULER_STATE` instead.
    pub fn capture_scheduler_reference_gate(
        &mut self,
    ) -> Option<BluetoothSchedulerReferenceGateObservation> {
        let mut control = HardwareSchedulerInterruptControl {
            registers: &self.peripherals.bluetooth_scheduler_interrupt_runtime,
        };
        execute_reference_gate_observation(&mut control)
    }

    /// Publish the complete zero image to `SCHEDULER_REFERENCE`.
    ///
    /// The primary classifier authorizes this only after bank-one source 3
    /// observed a clear diagnostic BUSY sample at the reference gate. This MMIO
    /// operation deliberately does not reproduce the vendor's following
    /// intrusive-list assertion; the open scheduler has no such mutable list
    /// representation.
    pub fn clear_scheduler_reference(&mut self) -> BluetoothSchedulerReferenceCleared {
        let mut control = HardwareSchedulerInterruptControl {
            registers: &self.peripherals.bluetooth_scheduler_interrupt_runtime,
        };
        execute_clear_scheduler_reference(&mut control);
        device_fence();
        BluetoothSchedulerReferenceCleared { _private: () }
    }

    /// Read the later scheduler-state image used to classify deferred work.
    pub fn capture_scheduler_work(&mut self) -> BluetoothSchedulerWorkObservation {
        let mut control = HardwareSchedulerInterruptControl {
            registers: &self.peripherals.bluetooth_scheduler_interrupt_runtime,
        };
        execute_work_observation(&mut control)
    }

    /// Sample scheduler BUSY once through the diagnostic pair for a
    /// task-side scheduler decision; `None` when the two reads disagreed.
    pub fn capture_scheduler_busy(&mut self) -> Option<BluetoothSchedulerBusyObservation> {
        sample_scheduler_busy(&self.peripherals.bluetooth_scheduler_interrupt_runtime)
            .map(BluetoothSchedulerBusyObservation::new)
    }
}

impl BluetoothTaskRegisters {
    /// Finish one task-owned software-list removal observation.
    ///
    /// The consumed idle capability proves the matching diagnostic BUSY
    /// sample was clear before either command register is touched. Command one
    /// is not read when command zero is clear. The operation performs at most
    /// two reads and always returns immediately.
    pub fn finish_scheduler_software_list_removal(
        &mut self,
        _idle: BluetoothSchedulerSoftwareListRemovalIdle,
        head: BluetoothSchedulerHardwareListHeadEmptyObserved,
    ) -> BluetoothSchedulerSoftwareListRemovalJoin {
        let mut control = HardwareSchedulerSoftwareListRemovalControl {
            registers: &self.bluetooth.bluetooth_controller_core,
        };
        join_software_list_removal(head, execute_software_list_removal_finish(&mut control))
    }
}

impl BluetoothTaskRegisters {
    /// Transfer the exact finished-list mask preceding one worker pop attempt.
    ///
    /// This reads `0x2010_125c`, truncates to its low halfword, then writes
    /// that value as a complete zero-high image to `0x2010_1260`. The method
    /// deliberately calls the second operation a report: its hardware clear
    /// or acknowledgement semantics are not independently established. The
    /// complete reference worker dispatches the returned mask through its
    /// finished-item selector before every software completed-list pop.
    pub fn transfer_scheduler_finished_lists(
        &mut self,
    ) -> BluetoothSchedulerFinishedListObservation {
        let mut control = HardwareSchedulerFinishedListControl {
            registers: &self.bluetooth.bluetooth_controller_core,
        };
        let observation = execute_finished_list_transfer(&mut control);
        device_fence();
        observation
    }
}

#[cfg(test)]
mod tests;
