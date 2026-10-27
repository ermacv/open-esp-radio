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
    /// A visible interrupt-policy fault or terminal readback failure.
    InterruptPolicy(crate::bluetooth::BluetoothInterruptPolicyFault),
}

/// The handler's side of the release: the bank.
trait BankControl {
    fn fault_acknowledged(&self) -> bool;
    fn observation(&self) -> crate::bluetooth::BluetoothInterruptObservation;
    fn mask_dynamic(&mut self);
    fn clear_pending(&mut self);
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
    task_enable: u32,
) -> Result<(), BluetoothControllerOutputReleaseError> {
    use BluetoothControllerOutputReleaseError as Error;
    if busy.is_busy() {
        return Err(Error::SchedulerBusy);
    }
    check_visible_faults(bank, task_enable)?;
    if bank.fault_acknowledged() {
        return Err(Error::InterruptFault);
    }
    Ok(())
}

fn check_visible_faults(
    bank: &impl BankControl,
    task_enable: u32,
) -> Result<(), BluetoothControllerOutputReleaseError> {
    let primary = bank.observation();
    if crate::bluetooth::interrupt_policy::release_fault_visible(primary) {
        return Err(BluetoothControllerOutputReleaseError::InterruptPolicy(
            crate::bluetooth::BluetoothInterruptPolicyFault::ReleaseNotQuiet(
                crate::bluetooth::BluetoothInterruptPolicyObservation {
                    primary,
                    task_enable,
                },
            ),
        ));
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

/// Mask all sourced terminal-cleanup sources and fence before the task disables
/// RUN in its own context.
fn mask(
    bank: &mut impl BankControl,
    task_enable: u32,
) -> Result<(), BluetoothControllerOutputReleaseError> {
    // The last visible fault check precedes the mask. Neither fixed terminal
    // image clears a later fault, but release cannot report hidden pending.
    // The next prepare_controller_output clears the baseline fault lanes
    // before enabling them, silently discarding such a retained fault. This
    // is a known limitation; release does not reopen enables to observe it.
    // SOURCE: HW32_S31_BT_MASKED_FIXED_W1C_20261027 and
    // HW32_S31_BT_OUTPUT_OBSERVER_20261027 (PR #757): tested pending is
    // hidden/retained under the mask; reopening raises matrix124 and modem
    // wake vector0x10 even at MAP0. Fault lanes are not hardware-qualified.
    check_visible_faults(bank, task_enable)?;
    if bank.fault_acknowledged() {
        return Err(BluetoothControllerOutputReleaseError::InterruptFault);
    }
    bank.mask_dynamic();
    bank.fence();
    let primary = bank.observation();
    if primary.enable_0 != 0
        || primary.enable_1 != 0
        || crate::bluetooth::interrupt_policy::release_fault_visible(primary)
    {
        return Err(BluetoothControllerOutputReleaseError::InterruptPolicy(
            crate::bluetooth::BluetoothInterruptPolicyFault::ReleaseNotQuiet(
                crate::bluetooth::BluetoothInterruptPolicyObservation {
                    primary,
                    task_enable,
                },
            ),
        ));
    }
    Ok(())
}

/// Disable RUN and fence.
fn disable_run(task: &mut impl TaskControl) {
    task.disable_run();
    task.fence();
}

fn release(
    bank: &mut impl BankControl,
    busy: BluetoothSchedulerBusyObservation,
    task_enable: u32,
) -> Result<(), BluetoothControllerOutputReleaseError> {
    use BluetoothControllerOutputReleaseError as Error;
    // Refuse changed execution and any still-visible fault. Masked fault
    // arrivals stay latched because no terminal clear selects their lanes.
    if busy.is_busy() {
        return Err(Error::SchedulerBusy);
    }
    check_visible_faults(bank, task_enable)?;
    if bank.fault_acknowledged() {
        return Err(Error::InterruptFault);
    }
    bank.clear_pending();
    bank.release_output();
    bank.fence();
    crate::bluetooth::BluetoothInterruptPolicyObservation {
        primary: bank.observation(),
        task_enable,
    }
    .confirm_released()
    .map_err(Error::InterruptPolicy)
}

struct BankHardware<'a, P: BluetoothPlacements> {
    bank: &'a mut oer_esp32s31_pac::bt_mac::BluetoothInterruptBank<P::BtMac>,
    /// The handler acknowledged a fault during the epoch.
    fault_acknowledged: bool,
}

impl<P: BluetoothPlacements> BankControl for BankHardware<'_, P> {
    fn observation(&self) -> crate::bluetooth::BluetoothInterruptObservation {
        crate::bluetooth::observation::primary(self.bank)
    }
    fn fault_acknowledged(&self) -> bool {
        self.fault_acknowledged
    }

    fn mask_dynamic(&mut self) {
        self.bank.modify_irq_enable_0(|w| {
            // SOURCE: BLOB_S31_BLE_IRQ_RELEASE_SCOPE_20261027: ISO/coex
            // group masks precede acknowledgement; include baseline and dynamics.
            w.set_source_0(false);
            w.set_source_1(false);
            w.set_source_8(false);
            w.set_source_13(false);
            w.set_source_15(false);
            w.set_source_16(false);
            w.set_source_21(false);
            w.set_sources_27_28(oer_mmio::Bits::constant::<0x0>());
        });
        self.bank.modify_irq_enable_1(|w| {
            w.set_controller_time_overflow(false);
            w.set_source_3(false);
            w.set_source_4(false);
            w.set_source_5(false);
            w.set_source_6(false);
            w.set_source_8(false);
            w.set_source_9(false);
            w.set_source_12(false);
            w.set_source_13(false);
        });
    }

    fn clear_pending(&mut self) {
        // SOURCE: POLICY_S31_BLE_TERMINAL_NONFAULT_ACK_20261027,
        // BLOB_LIBBTDM_CONTROLLER_IRQ / BLOB_S31_BLE_IRQ_RELEASE_SCOPE_20261027.
        // Fixed W1C works while masked; never derive a clear image from masked
        // STATUS. Shared primary/NRT CLEAR words exclude every fault lane.
        // Bank1 CC0/CC1/overflow: hw32-20261027-ble-masked-01 (#757).
        // Bank0/NRT and the remaining feature lanes are vendor-covered only,
        // not hardware-verified. No enable is opened for observation.
        self.bank.clear_bluetooth_terminal_non_fault_bank_0();
        self.bank.clear_bluetooth_terminal_non_fault_bank_1();
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
        let task = &mut self.task.bluetooth.btmac_ble_phy_init;
        // SOURCE: BLOB_S31_BLE_TASK_IRQ_INHERITANCE_20261027 and
        // BLOB_S31_BT_TASK17_SELECTION_SCOPE_20261027: the generic task-mask
        // helper selects argument lanes; task17 stays unnamed.
        task.modify_interrupt_enable(|w| {
            w.set_scheduler_run(false);
            w.set_source_17(false);
            w.set_bits_0_13_opaque(oer_mmio::Bits::constant::<0>());
            w.set_bits_15_16_opaque(oer_mmio::Bits::constant::<0>());
            w.set_bits_18_31_opaque(oer_mmio::Bits::constant::<0>());
        });
        // SOURCE: BLOB_LIBBLE_PHY_INIT_REGISTERS: init publishes all task WT
        // lanes together. The opaque task status is not read; this store makes
        // no readback or universal silicon-clear claim and never reads/ORs a WT word.
        task.trigger_interrupt_clear(|t| {
            t.bit_0_opaque()
                .bits_1_2_opaque(oer_mmio::Bits::constant::<0x3>())
                .bit_3_opaque()
                .bit_4_opaque()
                .bits_5_9_opaque(oer_mmio::Bits::constant::<0x1f>())
                .bit_10_opaque()
                .bit_11_opaque()
                .bits_12_13_opaque(oer_mmio::Bits::constant::<0x3>())
                .scheduler_run()
                .bit_15_opaque()
                .bit_16_opaque()
                .bit_17_opaque()
                .bits_18_19_opaque(oer_mmio::Bits::constant::<0x3>())
                .bit_20_opaque()
                .bit_21_opaque()
                .bits_22_23_opaque(oer_mmio::Bits::constant::<0x3>())
                .bit_24_opaque()
                .bits_25_27_opaque(oer_mmio::Bits::constant::<0x7>())
                .bit_28_opaque()
                .bits_29_31_opaque(oer_mmio::Bits::constant::<0x7>());
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
        task_enable: u32,
    ) -> Result<(), BluetoothControllerOutputReleaseError> {
        sample_idle(
            &mut BankHardware::<P> {
                bank: &mut self.peripherals.bluetooth_interrupt_bank,
                fault_acknowledged,
            },
            busy,
            task_enable,
        )
    }

    /// Mask all sourced terminal-cleanup sources and fence.
    pub(crate) fn mask_output_dynamic_sources(
        &mut self,
        fault_acknowledged: bool,
        task_enable: u32,
    ) -> Result<(), BluetoothControllerOutputReleaseError> {
        mask(
            &mut BankHardware::<P> {
                bank: &mut self.peripherals.bluetooth_interrupt_bank,
                fault_acknowledged,
            },
            task_enable,
        )
    }

    /// Discard the fixed non-fault primary/NRT subset under the mask when a BUSY
    /// sample taken after RUN was disabled is idle and no fault is pending
    /// or was acknowledged. Masked STATUS does not prove pending is absent.
    pub(crate) fn release_idle_output(
        &mut self,
        busy: BluetoothSchedulerBusyObservation,
        fault_acknowledged: bool,
        task_enable: u32,
    ) -> Result<(), BluetoothControllerOutputReleaseError> {
        release(
            &mut BankHardware::<P> {
                bank: &mut self.peripherals.bluetooth_interrupt_bank,
                fault_acknowledged,
            },
            busy,
            task_enable,
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
