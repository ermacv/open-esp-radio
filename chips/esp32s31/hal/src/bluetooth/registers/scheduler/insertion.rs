//! Restricted insertion-begin execution command transactions.
//!
//! Command zero binds one selected scheduler item to its hardware list for the
//! execution-lock attempt. Command one requests current-head reconciliation
//! for exactly one list. Both publications are finite; their hardware-owned
//! waits remain split into fresh task and interrupt observations.

#![deny(unsafe_code)]

use crate::layouts::{BleSchedulerItem, DmaRef};

use crate::bluetooth::registers::{
    BluetoothInterruptRegisters, BluetoothPlacements, BluetoothSchedulerBusyObservation,
    BluetoothSchedulerHardwareListIndex, BluetoothTaskRegisters, device_fence,
};

/// Validated item and list selected for one insertion execution-lock attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerExecutionLockRequest {
    address: DmaRef<BleSchedulerItem>,
    hardware_list_index: BluetoothSchedulerHardwareListIndex,
}

impl BluetoothSchedulerExecutionLockRequest {
    /// Bind the merge-selected item to its scheduler hardware list.
    pub const fn new(
        address: DmaRef<BleSchedulerItem>,
        hardware_list_index: BluetoothSchedulerHardwareListIndex,
    ) -> Self {
        Self {
            address,
            hardware_list_index,
        }
    }

    /// Selected item address without dereference authority.
    pub const fn address(self) -> DmaRef<BleSchedulerItem> {
        self.address
    }

    /// Hardware list serialized by this attempt.
    pub const fn hardware_list_index(self) -> BluetoothSchedulerHardwareListIndex {
        self.hardware_list_index
    }
}

/// Proof that command zero and its trailing device fence were published.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the execution-lock command still requires a fresh observation"]
pub struct BluetoothSchedulerExecutionLockPublished {
    _private: (),
}

/// Result of one finite execution-lock observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "pending work must return to the executor; terminal work must advance insertion"]
pub enum BluetoothSchedulerExecutionLockDisposition {
    /// Hardware remains busy and command-zero status has not become ready.
    Pending,
    /// Result zero retains command-zero execution lock for begin outcome four.
    ExecutionLockRetained,
    /// Scheduler idle or a nonzero result that the complete vendor body
    /// treats as a failed lock requires current-head reconciliation through
    /// command one.
    ReconcileCurrentHead,
    /// Hardware returned one of the three positional values on which the
    /// complete vendor body asserts.
    UnsupportedHardwareResult,
}

/// Proof that command one and its trailing device fence were published.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the execution-modify command still requires a fresh observation"]
pub struct BluetoothSchedulerExecutionModifyPublished {
    _private: (),
}

/// Result of one finite execution-modify observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "pending work must return to the executor; terminal work must advance insertion"]
pub enum BluetoothSchedulerExecutionModifyDisposition {
    /// The request waits for idle engines, a conflict to clear, command-one
    /// readiness or the settle signal.
    Pending,
    /// Command one reached its accepted reconciliation edge.
    Ready,
    /// The complete vendor body diagnoses positional status 19 on this edge.
    HardwareRejected,
}

trait BluetoothSchedulerInsertionExecutionControl {
    fn publish_execution_lock(&mut self, request: BluetoothSchedulerExecutionLockRequest);
    fn publish_execution_modify(&mut self, index: BluetoothSchedulerHardwareListIndex);
    fn publish_execution_modify_list_deletion(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
    );
    fn order_after_publication(&mut self);
}

trait BluetoothSchedulerInsertionExecutionObservationControl {
    fn observe_execution_lock_ready(&mut self) -> bool;
    fn observe_execution_lock_result(&mut self) -> u8;
    fn observe_execution_modify_ready(&mut self) -> bool;
    fn observe_execution_modify_rejected(&mut self) -> bool;
}

struct HardwareBluetoothSchedulerInsertionExecutionControl<'registers, B: oer_mmio::Placement> {
    registers: &'registers mut oer_esp32s31_pac::bt_mac::BluetoothControllerCore<B>,
}

impl<B: oer_mmio::Placement> BluetoothSchedulerInsertionExecutionControl
    for HardwareBluetoothSchedulerInsertionExecutionControl<'_, B>
{
    fn publish_execution_lock(&mut self, request: BluetoothSchedulerExecutionLockRequest) {
        // The vendor stores the request from zero: every other field of the
        // word is written zero.
        self.registers.modify_scheduler_command_0(|w| {
            w.set_compressed_sram_pointer(request.address());
            w.set_hardware_list_index(request.hardware_list_index().field_bits());
            w.set_status_24(false);
            w.set_bit_25_opaque(false);
            w.set_status_26(false);
            w.set_execution_lock_result(oer_mmio::Bits::constant::<0>());
            w.set_bit_30_opaque(false);
            w.set_start(true);
        });
    }

    fn publish_execution_modify(&mut self, index: BluetoothSchedulerHardwareListIndex) {
        // The vendor stores the request from zero.
        self.registers.modify_scheduler_command_1(|w| {
            w.set_hardware_list_mask(list_mask(index));
            w.set_bit_16_opaque(false);
            w.set_status_17(false);
            w.set_status_18(false);
            w.set_status_19(false);
            w.set_payload_high_20_30(oer_mmio::Bits::constant::<0>());
            w.set_start(true);
        });
    }

    fn publish_execution_modify_list_deletion(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
    ) {
        // The vendor stores the request from zero, with the list-deletion
        // bit.
        self.registers.modify_scheduler_command_1(|w| {
            w.set_hardware_list_mask(list_mask(index));
            w.set_bit_16_opaque(true);
            w.set_status_17(false);
            w.set_status_18(false);
            w.set_status_19(false);
            w.set_payload_high_20_30(oer_mmio::Bits::constant::<0>());
            w.set_start(true);
        });
    }

    fn order_after_publication(&mut self) {
        device_fence();
    }
}

/// One-hot command-one selection mask of one hardware list.
const fn list_mask(index: BluetoothSchedulerHardwareListIndex) -> u16 {
    // The index is below sixteen by construction.
    1u16.wrapping_shl(index.get() as u32)
}

impl<B: oer_mmio::Placement> BluetoothSchedulerInsertionExecutionObservationControl
    for HardwareBluetoothSchedulerInsertionExecutionControl<'_, B>
{
    fn observe_execution_lock_ready(&mut self) -> bool {
        self.registers.read_scheduler_command_0().status_24()
    }

    fn observe_execution_lock_result(&mut self) -> u8 {
        crate::bluetooth::registers::field_byte(
            self.registers
                .read_scheduler_command_0()
                .execution_lock_result(),
        )
    }

    fn observe_execution_modify_ready(&mut self) -> bool {
        self.registers.read_scheduler_command_1().status_17()
    }

    fn observe_execution_modify_rejected(&mut self) -> bool {
        self.registers.read_scheduler_command_1().status_19()
    }
}

fn execute_execution_lock_publication(
    control: &mut impl BluetoothSchedulerInsertionExecutionControl,
    request: BluetoothSchedulerExecutionLockRequest,
) -> BluetoothSchedulerExecutionLockPublished {
    control.publish_execution_lock(request);
    control.order_after_publication();
    BluetoothSchedulerExecutionLockPublished { _private: () }
}

fn execute_execution_modify_publication(
    control: &mut impl BluetoothSchedulerInsertionExecutionControl,
    index: BluetoothSchedulerHardwareListIndex,
) -> BluetoothSchedulerExecutionModifyPublished {
    control.publish_execution_modify(index);
    control.order_after_publication();
    BluetoothSchedulerExecutionModifyPublished { _private: () }
}

fn execute_execution_modify_list_deletion_publication(
    control: &mut impl BluetoothSchedulerInsertionExecutionControl,
    index: BluetoothSchedulerHardwareListIndex,
) -> BluetoothSchedulerExecutionModifyPublished {
    control.publish_execution_modify_list_deletion(index);
    control.order_after_publication();
    BluetoothSchedulerExecutionModifyPublished { _private: () }
}

fn execute_execution_lock_observation(
    control: &mut impl BluetoothSchedulerInsertionExecutionObservationControl,
    scheduler: BluetoothSchedulerBusyObservation,
) -> BluetoothSchedulerExecutionLockDisposition {
    if !scheduler.is_busy() {
        return BluetoothSchedulerExecutionLockDisposition::ReconcileCurrentHead;
    }
    if !control.observe_execution_lock_ready() {
        return BluetoothSchedulerExecutionLockDisposition::Pending;
    }
    match control.observe_execution_lock_result() {
        0 => BluetoothSchedulerExecutionLockDisposition::ExecutionLockRetained,
        1 | 3 | 4 => BluetoothSchedulerExecutionLockDisposition::UnsupportedHardwareResult,
        _ => BluetoothSchedulerExecutionLockDisposition::ReconcileCurrentHead,
    }
}

impl<P: BluetoothPlacements> BluetoothTaskRegisters<P> {
    /// Publish command zero and return after its trailing device fence.
    ///
    /// The request carries a typed reference to the merge-selected item. The
    /// engine retains its submitted pool storage, while the execution-lock state
    /// retains the publication until observation and START clear. The exclusive
    /// register loan serializes the command writes.
    #[doc(hidden)]
    pub fn publish_scheduler_execution_lock(
        &mut self,
        request: BluetoothSchedulerExecutionLockRequest,
    ) -> BluetoothSchedulerExecutionLockPublished {
        let mut control = HardwareBluetoothSchedulerInsertionExecutionControl {
            registers: &mut self.bluetooth.bluetooth_controller_core,
        };
        execute_execution_lock_publication(&mut control, request)
    }

    /// Perform one finite command-zero observation in the reviewed
    /// short-circuit order.
    ///
    /// An idle scheduler performs no command-register read. While busy, the
    /// positional result is read only after command-zero ready is set.
    pub fn observe_scheduler_execution_lock(
        &mut self,
        scheduler: BluetoothSchedulerBusyObservation,
    ) -> BluetoothSchedulerExecutionLockDisposition {
        let mut control = HardwareBluetoothSchedulerInsertionExecutionControl {
            registers: &mut self.bluetooth.bluetooth_controller_core,
        };
        execute_execution_lock_observation(&mut control, scheduler)
    }

    /// Publish command one for exactly one hardware list and return after its
    /// trailing device fence.
    ///
    /// The engine's insertion state selects `index` and retains the returned
    /// publication through reconciliation and START clear. The exclusive
    /// register loan serializes the command writes.
    #[doc(hidden)]
    pub fn publish_scheduler_execution_modify(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
    ) -> BluetoothSchedulerExecutionModifyPublished {
        let mut control = HardwareBluetoothSchedulerInsertionExecutionControl {
            registers: &mut self.bluetooth.bluetooth_controller_core,
        };
        execute_execution_modify_publication(&mut control, index)
    }

    /// Publish command one with its list-deletion mode bit for exactly one
    /// hardware list and return after its trailing device fence. The
    /// command-one observation and START clear are shared with insertion.
    ///
    /// The engine's deletion state retains the returned publication through
    /// command-one START clear. The exclusive register loan serializes the
    /// command writes.
    #[doc(hidden)]
    pub fn publish_scheduler_execution_modify_list_deletion(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
    ) -> BluetoothSchedulerExecutionModifyPublished {
        let mut control = HardwareBluetoothSchedulerInsertionExecutionControl {
            registers: &mut self.bluetooth.bluetooth_controller_core,
        };
        execute_execution_modify_list_deletion_publication(&mut control, index)
    }

    /// Read command-one ready status 17.
    #[doc(hidden)]
    pub fn scheduler_execution_modify_ready(&mut self) -> bool {
        HardwareBluetoothSchedulerInsertionExecutionControl {
            registers: &mut self.bluetooth.bluetooth_controller_core,
        }
        .observe_execution_modify_ready()
    }

    /// Read command-one status 19, which the vendor asserts is clear.
    #[doc(hidden)]
    pub fn scheduler_execution_modify_rejected(&mut self) -> bool {
        HardwareBluetoothSchedulerInsertionExecutionControl {
            registers: &mut self.bluetooth.bluetooth_controller_core,
        }
        .observe_execution_modify_rejected()
    }
}

impl<P: BluetoothPlacements> BluetoothInterruptRegisters<P> {
    /// Select the execution-modify progress signal of the diagnostic pair.
    #[doc(hidden)]
    pub fn select_scheduler_execution_modify_progress(&mut self) {
        super::runtime::select_execution_modify_progress(
            &mut self.peripherals.bluetooth_scheduler_interrupt_runtime,
        );
    }

    /// Read the selected progress signal twice: whether execution modify must
    /// repeat its request, or `None` when the reads disagreed.
    #[doc(hidden)]
    pub fn scheduler_execution_modify_repeats(&mut self) -> Option<bool> {
        super::runtime::sample_execution_modify_repeats(
            &mut self.peripherals.bluetooth_scheduler_interrupt_runtime,
        )
    }

    /// Select the execution-modify settle signal of the diagnostic pair.
    #[doc(hidden)]
    pub fn select_scheduler_execution_modify_settle(&mut self) {
        super::runtime::select_execution_modify_settle(
            &mut self.peripherals.bluetooth_scheduler_interrupt_runtime,
        );
    }

    /// Read the selected settle signal twice: whether execution modify
    /// settled, or `None` when the reads disagreed.
    #[doc(hidden)]
    pub fn scheduler_execution_modify_settled(&mut self) -> Option<bool> {
        super::runtime::sample_execution_modify_settled(
            &mut self.peripherals.bluetooth_scheduler_interrupt_runtime,
        )
    }
}

#[cfg(test)]
mod tests;
