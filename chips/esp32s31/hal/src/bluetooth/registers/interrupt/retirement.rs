//! Terminal output release after independent task and CPU-route retirement.
//!
//! The idle preflight retains every hardware list pointer on rejection. The
//! accepted suffix composes the reviewed scheduler source mask/run-disable,
//! link-basic acknowledgement and controller-output release transactions.
//! It does not stop the modem counter or revoke PHY/RX/DF publications.
//!
//! Scheduler BUSY is sampled by the HAL, which bounds the diagnostic retry,
//! so release is two transactions: quiescence consumes the preflight sample,
//! release consumes a fresh sample taken after the mask/disable fence.

use super::*;
use crate::bluetooth::registers::{
    BluetoothSchedulerBusyObservation, BluetoothSchedulerHardwareListIndex, BluetoothTaskRegisters,
};

/// Hardware obligation preventing terminal Controller interrupt-output release.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothControllerOutputReleaseError {
    /// The task-side controller-time latch is still owned. The HAL, which
    /// owns the latch request, reports this before any register access.
    ControllerTimePending,
    /// The scheduler still reports hardware execution.
    SchedulerBusy,
    /// The two diagnostic reads of scheduler BUSY never agreed within the
    /// HAL's attempt budget. The HAL reports this before any register write.
    SchedulerBusyUnsettled,
    /// A list still names hardware-visible memory; it is left unchanged.
    PublishedHead(BluetoothSchedulerHardwareListIndex),
    /// A fatal or unclassified primary source remains unacknowledged.
    InterruptFault,
}

trait Control {
    fn head_present(&mut self, index: BluetoothSchedulerHardwareListIndex) -> bool;
    fn fault_pending(&mut self) -> bool;
    fn mask_dynamic(&mut self);
    fn disable_run(&mut self);
    fn fence(&mut self);
    fn clear_dynamic(&mut self);
    fn release_output(&mut self);
}

fn validate_idle(
    control: &mut impl Control,
    busy: BluetoothSchedulerBusyObservation,
) -> Result<(), BluetoothControllerOutputReleaseError> {
    use BluetoothControllerOutputReleaseError as Error;
    if busy.is_busy() {
        return Err(Error::SchedulerBusy);
    }
    for index in 0..16 {
        let index = BluetoothSchedulerHardwareListIndex::new(index).expect("hardware list");
        if control.head_present(index) {
            return Err(Error::PublishedHead(index));
        }
    }
    if control.fault_pending() {
        return Err(Error::InterruptFault);
    }
    Ok(())
}

fn quiesce(
    control: &mut impl Control,
    busy: BluetoothSchedulerBusyObservation,
) -> Result<(), BluetoothControllerOutputReleaseError> {
    validate_idle(control, busy)?;
    control.mask_dynamic();
    control.disable_run();
    control.fence();
    Ok(())
}

fn release(
    control: &mut impl Control,
    busy: BluetoothSchedulerBusyObservation,
) -> Result<(), BluetoothControllerOutputReleaseError> {
    use BluetoothControllerOutputReleaseError as Error;
    // Do not acknowledge a new fault or release output if execution changed
    // during the preflight. CPU routing stays disabled on every result.
    if busy.is_busy() {
        return Err(Error::SchedulerBusy);
    }
    if control.fault_pending() {
        return Err(Error::InterruptFault);
    }
    control.clear_dynamic();
    control.release_output();
    control.fence();
    Ok(())
}

struct Hardware<'a, P: BluetoothPlacements> {
    task: &'a mut BluetoothTaskRegisters<P>,
    output: &'a mut BluetoothInterruptOutputPrepared<P>,
}

impl<P: BluetoothPlacements> Control for Hardware<'_, P> {
    fn head_present(&mut self, index: BluetoothSchedulerHardwareListIndex) -> bool {
        self.task.scheduler_head_present(index)
    }

    fn fault_pending(&mut self) -> bool {
        let mut control = HardwareInterruptControl {
            bank: &mut self.output.peripherals.bluetooth_interrupt_bank,
        };
        let bank_0 = control.sample_bank_0();
        let bank_1 = control.sample_bank_1();
        let faults = BluetoothPrimaryFaultSources::from_status(
            control.bank_0_status(&bank_0),
            control.bank_1_status(&bank_1),
        );
        faults.is_fault()
    }

    fn mask_dynamic(&mut self) {
        let bank = &mut self.output.peripherals.bluetooth_interrupt_bank;
        bank.modify_irq_enable_0(|w| {
            w.set_source_21(false);
            w.set_sources_27_28(oer_mmio::Bits::constant::<0x0>());
        });
        bank.modify_irq_enable_1(|w| {
            w.set_source_3(false);
        });
    }

    fn disable_run(&mut self) {
        self.task
            .bluetooth
            .btmac_ble_phy_init
            .modify_interrupt_enable(|w| {
                w.set_scheduler_run(false);
            });
    }

    fn fence(&mut self) {
        device_fence();
    }

    fn clear_dynamic(&mut self) {
        let mut control = HardwareInterruptControl {
            bank: &mut self.output.peripherals.bluetooth_interrupt_bank,
        };
        control.clear_scheduler_run_bank_0();
        control.clear_scheduler_run_bank_1();
    }

    fn release_output(&mut self) {
        execute_primary_release(&mut HardwareInterruptControl {
            bank: &mut self.output.peripherals.bluetooth_interrupt_bank,
        });
    }
}

/// Proof that the idle preflight passed and the dynamic scheduler sources
/// were masked, RUN disabled and a device fence completed.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "a quiesced output awaits a fresh idle sample before release"]
pub struct BluetoothControllerOutputQuiesced {
    _private: (),
}

impl<P: BluetoothPlacements> BluetoothInterruptOutputPrepared<P> {
    /// Sample scheduler BUSY once through the diagnostic pair; `None` when
    /// the two reads disagreed. The HAL owns the bounded retry.
    pub fn capture_scheduler_busy(&mut self) -> Option<BluetoothSchedulerBusyObservation> {
        crate::bluetooth::registers::scheduler::runtime::sample_scheduler_busy(
            &mut self.peripherals.bluetooth_scheduler_interrupt_runtime,
        )
        .map(BluetoothSchedulerBusyObservation::new)
    }

    /// Observe idle admission without changing masks, heads or pending status.
    /// The caller retains the task and unrouted interrupt partitions throughout.
    pub fn validate_idle_controller(
        &mut self,
        task: &mut BluetoothTaskRegisters<P>,
        busy: BluetoothSchedulerBusyObservation,
    ) -> Result<(), BluetoothControllerOutputReleaseError> {
        validate_idle(&mut Hardware { task, output: self }, busy)
    }

    /// Admit an idle, headless scheduler without primary faults, then mask the
    /// dynamic scheduler sources, disable RUN and fence.
    ///
    /// The lifecycle caller must own the retired command task and must have
    /// removed CPU routes before calling. Rejection before the preflight
    /// passes changes nothing; no list head is erased to manufacture idle.
    pub fn quiesce_idle_controller_output(
        &mut self,
        task: &mut BluetoothTaskRegisters<P>,
        busy: BluetoothSchedulerBusyObservation,
    ) -> Result<BluetoothControllerOutputQuiesced, BluetoothControllerOutputReleaseError> {
        quiesce(&mut Hardware { task, output: self }, busy)?;
        Ok(BluetoothControllerOutputQuiesced { _private: () })
    }

    /// Release output when a BUSY sample taken after quiescence is still idle
    /// and no primary fault is pending.
    ///
    /// Rejection retains both partitions and leaves dynamic sources masked and
    /// RUN disabled. Faults are never cleared.
    pub fn try_release_idle_controller_output(
        mut self,
        task: &mut BluetoothTaskRegisters<P>,
        _quiesced: BluetoothControllerOutputQuiesced,
        busy: BluetoothSchedulerBusyObservation,
    ) -> Result<BluetoothInterruptSetup<P>, (BluetoothControllerOutputReleaseError, Self)> {
        if let Err(error) = release(
            &mut Hardware {
                task,
                output: &mut self,
            },
            busy,
        ) {
            return Err((error, self));
        }
        Ok(BluetoothInterruptSetup {
            peripherals: self.peripherals,
        })
    }
}

#[cfg(test)]
mod tests;
