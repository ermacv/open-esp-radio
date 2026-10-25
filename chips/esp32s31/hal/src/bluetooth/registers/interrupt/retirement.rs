//! Terminal output release of an idle scheduler, while the interrupt
//! handler still holds the bank.
//!
//! The bank stays with its handler for the whole epoch, so the release is a
//! sequence of steps that alternate between the handler (the bank and the
//! diagnostic pair) and the task (the scheduler's list heads and RUN). The
//! writes keep the reviewed order: mask the dynamic scheduler sources, disable
//! RUN, fence; acknowledge the dynamic sources, release the controller output,
//! fence. It does not stop the modem counter or revoke PHY/RX/DF publications.
//!
//! Scheduler BUSY is sampled by the HAL, which bounds the diagnostic retry:
//! the idle sample precedes the mask, and a fresh sample after the fence that
//! disables RUN precedes the release.

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
    /// A fatal or unclassified primary source is pending, or the handler
    /// acknowledged one during the epoch.
    InterruptFault,
}

/// The handler's side of the release: the bank.
trait BankControl {
    fn fault_pending(&mut self) -> bool;
    fn mask_dynamic(&mut self);
    fn clear_dynamic(&mut self);
    fn release_output(&mut self);
    fn fence(&mut self);
}

/// The task's side of the release: the scheduler's list heads and RUN.
trait TaskControl {
    fn head_present(&mut self, index: BluetoothSchedulerHardwareListIndex) -> bool;
    fn disable_run(&mut self);
    fn fence(&mut self);
}

/// The idle sample: no execution and no fault.
fn sample_idle(
    bank: &mut impl BankControl,
    busy: BluetoothSchedulerBusyObservation,
) -> Result<(), BluetoothControllerOutputReleaseError> {
    use BluetoothControllerOutputReleaseError as Error;
    if busy.is_busy() {
        return Err(Error::SchedulerBusy);
    }
    if bank.fault_pending() {
        return Err(Error::InterruptFault);
    }
    Ok(())
}

/// No list names hardware-visible memory.
fn confirm_heads_absent(
    task: &mut impl TaskControl,
) -> Result<(), BluetoothControllerOutputReleaseError> {
    for index in (0..16).filter_map(BluetoothSchedulerHardwareListIndex::new) {
        if task.head_present(index) {
            return Err(BluetoothControllerOutputReleaseError::PublishedHead(index));
        }
    }
    Ok(())
}

/// Mask the dynamic scheduler sources and fence before the task disables
/// RUN in its own context.
fn mask(bank: &mut impl BankControl) {
    bank.mask_dynamic();
    bank.fence();
}

/// Disable RUN and fence.
fn disable_run(task: &mut impl TaskControl) {
    task.disable_run();
    task.fence();
}

fn release(
    bank: &mut impl BankControl,
    busy: BluetoothSchedulerBusyObservation,
) -> Result<(), BluetoothControllerOutputReleaseError> {
    use BluetoothControllerOutputReleaseError as Error;
    // Do not acknowledge a new fault or release output if execution changed
    // after the mask.
    if busy.is_busy() {
        return Err(Error::SchedulerBusy);
    }
    if bank.fault_pending() {
        return Err(Error::InterruptFault);
    }
    bank.clear_dynamic();
    bank.release_output();
    bank.fence();
    Ok(())
}

struct BankHardware<'a, P: BluetoothPlacements> {
    bank: &'a mut oer_esp32s31_pac::bt_mac::BluetoothInterruptBank<P::BtMac>,
    /// The handler acknowledged a fault during the epoch.
    fault_acknowledged: bool,
}

impl<P: BluetoothPlacements> BankControl for BankHardware<'_, P> {
    fn fault_pending(&mut self) -> bool {
        if self.fault_acknowledged {
            return true;
        }
        let mut control = HardwareInterruptControl {
            bank: &mut *self.bank,
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
        self.bank.modify_irq_enable_0(|w| {
            w.set_source_21(false);
            w.set_sources_27_28(oer_mmio::Bits::constant::<0x0>());
        });
        self.bank.modify_irq_enable_1(|w| {
            w.set_source_3(false);
        });
    }

    fn clear_dynamic(&mut self) {
        let mut control = HardwareInterruptControl {
            bank: &mut *self.bank,
        };
        control.clear_scheduler_run_bank_0();
        control.clear_scheduler_run_bank_1();
    }

    fn release_output(&mut self) {
        execute_primary_release(&mut HardwareInterruptControl {
            bank: &mut *self.bank,
        });
    }

    fn fence(&mut self) {
        device_fence();
    }
}

struct TaskHardware<'a, P: BluetoothPlacements> {
    task: &'a mut BluetoothTaskRegisters<P>,
}

impl<P: BluetoothPlacements> TaskControl for TaskHardware<'_, P> {
    fn head_present(&mut self, index: BluetoothSchedulerHardwareListIndex) -> bool {
        self.task.scheduler_head_present(index)
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
}

impl<P: BluetoothPlacements> BluetoothInterruptRegisters<P> {
    /// Admit release on an idle BUSY sample without a pending or
    /// acknowledged fault. Writes nothing.
    pub(crate) fn sample_idle_output(
        &mut self,
        busy: BluetoothSchedulerBusyObservation,
        fault_acknowledged: bool,
    ) -> Result<(), BluetoothControllerOutputReleaseError> {
        sample_idle(
            &mut BankHardware::<P> {
                bank: &mut self.peripherals.bluetooth_interrupt_bank,
                fault_acknowledged,
            },
            busy,
        )
    }

    /// Mask the dynamic scheduler sources and fence.
    pub(crate) fn mask_output_dynamic_sources(&mut self) {
        mask(&mut BankHardware::<P> {
            bank: &mut self.peripherals.bluetooth_interrupt_bank,
            fault_acknowledged: false,
        });
    }

    /// Acknowledge the dynamic sources and release the output when a BUSY
    /// sample taken after RUN was disabled is idle and no fault is pending
    /// or was acknowledged. Faults are never cleared.
    pub(crate) fn release_idle_output(
        &mut self,
        busy: BluetoothSchedulerBusyObservation,
        fault_acknowledged: bool,
    ) -> Result<(), BluetoothControllerOutputReleaseError> {
        release(
            &mut BankHardware::<P> {
                bank: &mut self.peripherals.bluetooth_interrupt_bank,
                fault_acknowledged,
            },
            busy,
        )
    }

    /// The released bank, as the inactive interrupt partition. No MMIO.
    pub(crate) fn into_released(self) -> BluetoothInterruptSetup<P> {
        BluetoothInterruptSetup {
            peripherals: self.peripherals,
        }
    }
}

impl<P: BluetoothPlacements> BluetoothTaskRegisters<P> {
    /// No scheduler list names hardware-visible memory. Writes nothing.
    pub(crate) fn confirm_scheduler_heads_absent(
        &mut self,
    ) -> Result<(), BluetoothControllerOutputReleaseError> {
        confirm_heads_absent(&mut TaskHardware { task: self })
    }

    /// Disable scheduler RUN and fence.
    pub(crate) fn disable_scheduler_run(&mut self) {
        disable_run(&mut TaskHardware { task: self });
    }
}

#[cfg(test)]
mod tests;
