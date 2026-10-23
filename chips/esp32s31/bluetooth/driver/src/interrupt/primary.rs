//! Capture/acknowledge the primary source-124 epoch and its ordered suffix.
//!
//! P3 performs only the hardware protocol and latches positional inputs.
//! The P2 consumer classifies their correlated, coalesced facts. In particular,
//! source presence and its own later deferred-work sample travel as one key;
//! OR-ing the two inputs independently could invent a marked event.

use oer_esp32s31_hal::bluetooth::engine::scheduler::interrupt::SchedulerInterruptClock;
use oer_esp32s31_hal::bluetooth::{
    BluetoothPrimaryInterruptEpoch, BluetoothSchedulerReferenceGateObservation,
    BluetoothSchedulerWorkObservation, InterruptRegistersOwner,
};
use oer_esp32s31_hal::settle::Unsettled;

/// Captured primary inputs, before P2 classification.
/// Empty epochs carry no work. Fault-source presence and a failed diagnostic
/// settle remain sticky in the worker's latch, without panicking in P3.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "latch captured primary facts for the P2 consumer"]
pub struct PrimaryInterruptCapture(pub(super) u32);

impl PrimaryInterruptCapture {
    pub(super) const FAULT_SHIFT: u32 = 16;
    pub(super) const FAULT_MASK: u32 = 0x1f << Self::FAULT_SHIFT;
    pub(super) const UNSETTLED: u32 = 1 << 21;

    /// Fault-free positional sources paired with this event's work sample.
    fn work(
        epoch: &BluetoothPrimaryInterruptEpoch,
        work: BluetoothSchedulerWorkObservation,
    ) -> Self {
        let key = u32::from(epoch.bank_0_source_21_pending())
            | (u32::from(epoch.bank_0_sources_27_or_28_pending()) << 1)
            | (u32::from(epoch.bank_1_source_3_pending()) << 2)
            | (u32::from(work.deferred_work_requested()) << 3);
        Self(1 << key)
    }
}

trait PrimaryInterruptBackend {
    fn capture_primary_and_acknowledge(&mut self) -> BluetoothPrimaryInterruptEpoch;
    fn capture_scheduler_reference_gate(
        &mut self,
    ) -> Result<BluetoothSchedulerReferenceGateObservation, Unsettled>;
    fn clear_scheduler_reference(&mut self);
    fn capture_scheduler_work(&mut self) -> BluetoothSchedulerWorkObservation;
}

struct PrimaryHardware<'a> {
    interrupts: &'a mut InterruptRegistersOwner,
    clock: SchedulerInterruptClock<'a>,
}

impl PrimaryInterruptBackend for PrimaryHardware<'_> {
    fn capture_primary_and_acknowledge(&mut self) -> BluetoothPrimaryInterruptEpoch {
        self.interrupts.capture_primary_and_acknowledge()
    }
    fn capture_scheduler_reference_gate(
        &mut self,
    ) -> Result<BluetoothSchedulerReferenceGateObservation, Unsettled> {
        self.interrupts
            .capture_scheduler_reference_gate(crate::diagnostic_settle(&self.clock))
    }
    fn clear_scheduler_reference(&mut self) {
        let _cleared = self.interrupts.clear_scheduler_reference();
    }
    fn capture_scheduler_work(&mut self) -> BluetoothSchedulerWorkObservation {
        self.interrupts.capture_scheduler_work()
    }
}

fn execute_primary_interrupt_capture(
    backend: &mut impl PrimaryInterruptBackend,
) -> PrimaryInterruptCapture {
    let epoch = backend.capture_primary_and_acknowledge();
    let fault = epoch.fault_sources();
    // These positional captures also gate the hardware suffix: baseline or
    // unreviewed sources must not perform its diagnostic/read/write protocol.
    if fault.is_fault() {
        let sources = u32::from(fault.bank_0_source_15_pending())
            | (u32::from(fault.bank_1_source_8_pending()) << 1)
            | (u32::from(fault.bank_1_source_9_pending()) << 2)
            | (u32::from(fault.bank_1_source_12_pending()) << 3)
            | (u32::from(fault.unclassified_pending()) << 4);
        return PrimaryInterruptCapture(sources << PrimaryInterruptCapture::FAULT_SHIFT);
    }
    if epoch.bank_1_source_3_pending() {
        let Ok(gate) = backend.capture_scheduler_reference_gate() else {
            return PrimaryInterruptCapture(PrimaryInterruptCapture::UNSETTLED);
        };
        if !gate.is_busy() {
            backend.clear_scheduler_reference();
        }
    }
    if epoch.bank_0_source_21_pending()
        || epoch.bank_0_sources_27_or_28_pending()
        || epoch.bank_1_source_3_pending()
    {
        // A distinct read after the optional reference clear, never the
        // earlier diagnostic sample reused as a work observation.
        PrimaryInterruptCapture::work(&epoch, backend.capture_scheduler_work())
    } else {
        PrimaryInterruptCapture(0)
    }
}

/// Capture/ack/latch inputs without scheduler classification, accounting,
/// formatting or task callbacks. The only conditional hardware suffix is
/// the existing bounded reference-gate protocol, in its original order.
pub(crate) fn step_primary_interrupt(
    interrupts: &mut InterruptRegistersOwner,
    clock: &dyn oer_time::Clock,
) -> PrimaryInterruptCapture {
    execute_primary_interrupt_capture(&mut PrimaryHardware {
        interrupts,
        clock: SchedulerInterruptClock::new(clock),
    })
}

#[cfg(test)]
mod tests;
