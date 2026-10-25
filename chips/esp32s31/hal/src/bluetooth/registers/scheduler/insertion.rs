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
    registers:
        &'registers mut oer_esp32s31_pac::bluetooth_controller_core::BluetoothControllerCore<B>,
}

impl<B: oer_mmio::Placement> BluetoothSchedulerInsertionExecutionControl
    for HardwareBluetoothSchedulerInsertionExecutionControl<'_, B>
{
    fn publish_execution_lock(&mut self, request: BluetoothSchedulerExecutionLockRequest) {
        self.registers.publish_bluetooth_scheduler_execution_lock_request(oer_esp32s31_pac::bluetooth_controller_core::scheduler_command_0::CompressedSramPointerImage::new(request.address())
                .expect("the placed layout has a nonzero BLE encoding"), request.hardware_list_index().field_bits(), true);
    }

    fn publish_execution_modify(&mut self, index: BluetoothSchedulerHardwareListIndex) {
        self.registers
            .publish_bluetooth_scheduler_execution_modify_request(list_mask(index), true);
    }

    fn publish_execution_modify_list_deletion(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
    ) {
        self.registers
            .publish_bluetooth_scheduler_execution_modify_list_deletion_request(
                list_mask(index),
                true,
                true,
            );
    }

    fn order_after_publication(&mut self) {
        device_fence();
    }
}

/// One-hot command-one selection mask of one hardware list.
const fn list_mask(index: BluetoothSchedulerHardwareListIndex) -> u16 {
    match index.get() {
        0 => 0x0001,
        1 => 0x0002,
        2 => 0x0004,
        3 => 0x0008,
        4 => 0x0010,
        5 => 0x0020,
        6 => 0x0040,
        7 => 0x0080,
        8 => 0x0100,
        9 => 0x0200,
        10 => 0x0400,
        11 => 0x0800,
        12 => 0x1000,
        13 => 0x2000,
        14 => 0x4000,
        15 => 0x8000,
        _ => panic!("typed scheduler list index exceeded its PAC domain"),
    }
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
    /// # Safety
    ///
    /// The request must name the exact merge-selected initialized item. The
    /// caller must retain that pinned item and exclusive list serialization
    /// until insertion-begin and insertion-end return ownership.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller retains the selected item lifetime and scheduler serialization"
    )]
    pub unsafe fn publish_scheduler_execution_lock(
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
    /// # Safety
    ///
    /// The caller must own the insertion reconciliation epoch and exclusive
    /// access to `index` until insertion-end returns ownership.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller retains insertion reconciliation and list serialization"
    )]
    pub unsafe fn publish_scheduler_execution_modify(
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
    /// # Safety
    ///
    /// The caller must own the deletion of `index` and exclusive access to
    /// that list until it clears command-one START.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller retains list deletion and list serialization"
    )]
    pub unsafe fn publish_scheduler_execution_modify_list_deletion(
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
