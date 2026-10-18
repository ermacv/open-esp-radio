//! One bounded primary source-124 interrupt disposition.
//!
//! The restricted PAC owns all register geometry and acknowledgement order;
//! the HAL owns the affine interrupt-register capability. This layer joins
//! those finite operations with the Controller classifier without callbacks,
//! allocation, polling or an RTOS queue.

#![forbid(unsafe_code)]

use crate::interrupt::{
    PrimaryControllerFault, PrimaryInterruptClassification, SchedulerReferenceAction,
    SchedulerWakeCell, SchedulerWakePublication, SchedulerWorkerWake,
};

use oer_esp32s31_hal::bluetooth::{
    BluetoothDiagnosticUnsettled, BluetoothSchedulerReferenceGateObservation,
    BluetoothSchedulerWorkObservation, InterruptRegistersOwner,
};

use oer_esp32s31_hal::bluetooth::{
    BluetoothPrimaryInterruptEpoch, BluetoothSchedulerHardwareListIndex,
};

/// Terminal result of one bounded primary source-124 handler step.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the acknowledged primary epoch must be retained or published"]
pub enum PrimaryInterruptStep {
    /// A reviewed baseline fault preempted ordinary scheduler work.
    Fault(PrimaryControllerFault),
    /// The reference-gate BUSY sample never settled within
    /// [`DIAGNOSTIC_READ_BUDGET`](crate::scheduler::DIAGNOSTIC_READ_BUDGET).
    /// The reference is preserved and no work is published.
    DiagnosticUnsettled(PrimaryInterruptClassification),
    /// The epoch contained no reviewed dynamic scheduler source.
    NoSchedulerWork(PrimaryNoSchedulerWork),
    /// A scheduler wake and matching BUSY observation are ready for publication.
    Scheduler(PrimarySchedulerEvent),
}

/// Acknowledged primary epoch with no reviewed scheduler work.
#[derive(Debug, Eq, PartialEq)]
pub struct PrimaryNoSchedulerWork {
    classification: PrimaryInterruptClassification,
}

/// One classified scheduler publication derived from a single later state read.
#[derive(Debug, Eq, PartialEq)]
pub struct PrimarySchedulerEvent {
    classification: PrimaryInterruptClassification,
    wake: SchedulerWorkerWake,
    work: BluetoothSchedulerWorkObservation,
}

/// Durable Controller disposition of one acknowledged primary epoch.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "fault, recovery or scheduler publication must reach the Controller owner"]
pub enum PrimaryPublishedInterruptStep {
    /// A baseline or unclassified fault published no ordinary work.
    Fault(PrimaryControllerFault),
    /// The reference-gate BUSY sample never settled; no work was published.
    DiagnosticUnsettled(PrimaryInterruptClassification),
    /// The epoch contained no reviewed dynamic scheduler work.
    NoSchedulerWork(PrimaryNoSchedulerWork),
    /// The scheduler handoff accepted the event.
    Scheduler {
        /// Exact classified primary event.
        event: PrimarySchedulerEvent,
        /// Coalescing disposition of the scheduler handoff.
        scheduler: SchedulerWakePublication,
    },
}

impl PrimaryInterruptStep {
    /// Publish one classified primary result into the scheduler handoff.
    ///
    /// Fault and empty outcomes never publish ordinary scheduler work.
    pub fn publish(self, scheduler_wake: &SchedulerWakeCell) -> PrimaryPublishedInterruptStep {
        match self {
            Self::Fault(fault) => PrimaryPublishedInterruptStep::Fault(fault),
            Self::DiagnosticUnsettled(classification) => {
                PrimaryPublishedInterruptStep::DiagnosticUnsettled(classification)
            }
            Self::NoSchedulerWork(epoch) => PrimaryPublishedInterruptStep::NoSchedulerWork(epoch),
            Self::Scheduler(event) => {
                let scheduler = scheduler_wake.publish_from_interrupt(event.wake().class());
                PrimaryPublishedInterruptStep::Scheduler { event, scheduler }
            }
        }
    }
}

impl PrimarySchedulerEvent {
    /// Return the scheduler-worker wake classification.
    pub const fn wake(&self) -> SchedulerWorkerWake {
        self.wake
    }

    /// Whether the same scheduler-state read found the scheduler busy.
    pub const fn scheduler_busy(&self) -> bool {
        self.work.is_busy()
    }

    /// Hardware-list index captured by the same scheduler-state read.
    pub const fn current_hardware_list(&self) -> BluetoothSchedulerHardwareListIndex {
        self.work.current_hardware_list()
    }
}

trait PrimaryInterruptBackend {
    fn capture_primary_and_acknowledge(&mut self) -> BluetoothPrimaryInterruptEpoch;
    fn capture_scheduler_reference_gate(
        &mut self,
    ) -> Result<BluetoothSchedulerReferenceGateObservation, BluetoothDiagnosticUnsettled>;
    fn clear_scheduler_reference(&mut self);
    fn capture_scheduler_work(&mut self) -> BluetoothSchedulerWorkObservation;
}

impl PrimaryInterruptBackend for InterruptRegistersOwner {
    fn capture_primary_and_acknowledge(&mut self) -> BluetoothPrimaryInterruptEpoch {
        self.capture_primary_and_acknowledge()
    }

    fn capture_scheduler_reference_gate(
        &mut self,
    ) -> Result<BluetoothSchedulerReferenceGateObservation, BluetoothDiagnosticUnsettled> {
        self.capture_scheduler_reference_gate(crate::scheduler::DIAGNOSTIC_READ_BUDGET)
    }

    fn clear_scheduler_reference(&mut self) {
        let _cleared = InterruptRegistersOwner::clear_scheduler_reference(self);
    }

    fn capture_scheduler_work(&mut self) -> BluetoothSchedulerWorkObservation {
        self.capture_scheduler_work()
    }
}

fn execute_primary_interrupt_step(
    backend: &mut impl PrimaryInterruptBackend,
) -> PrimaryInterruptStep {
    let classification =
        match PrimaryInterruptClassification::from_epoch(backend.capture_primary_and_acknowledge())
        {
            Ok(classification) => classification,
            Err(fault) => return PrimaryInterruptStep::Fault(fault),
        };

    if let Some(gate) = classification.reference_gate() {
        let Ok(observation) = backend.capture_scheduler_reference_gate() else {
            return PrimaryInterruptStep::DiagnosticUnsettled(classification);
        };
        if gate.classify(observation) == SchedulerReferenceAction::ClearReferenceAndContinue {
            backend.clear_scheduler_reference();
        }
    }

    let Some(work_classifier) = classification.work_classifier() else {
        return PrimaryInterruptStep::NoSchedulerWork(PrimaryNoSchedulerWork { classification });
    };
    let work = backend.capture_scheduler_work();
    let wake = work_classifier.classify(&work);
    PrimaryInterruptStep::Scheduler(PrimarySchedulerEvent {
        classification,
        wake,
        work,
    })
}

/// Capture, acknowledge and classify one primary source-124 interrupt epoch.
///
/// This function is finite. It performs at most the PAC acknowledgement
/// transaction, one reference-gate sample of at most
/// [`DIAGNOSTIC_READ_BUDGET`](crate::scheduler::DIAGNOSTIC_READ_BUDGET)
/// attempts and one later work read. It never
/// invokes a callback, waits for hardware, allocates or wakes an executor.
pub fn step_primary_interrupt(interrupts: &mut InterruptRegistersOwner) -> PrimaryInterruptStep {
    execute_primary_interrupt_step(interrupts)
}

#[cfg(test)]
mod tests;
