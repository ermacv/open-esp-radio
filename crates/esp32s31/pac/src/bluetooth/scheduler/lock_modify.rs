//! Affine PAC transaction for scheduler insert-with-lock-modify.
//!
//! The task-side request register and interrupt-side scheduler-state register
//! deliberately have different physical owners. Publication is one finite
//! task-owned MMIO transaction. Wait observations remain split into two value
//! tokens until the controller coordinator joins them at one decision point;
//! neither PAC owner aliases the other register partition.

#![deny(unsafe_code)]

use crate::{
    BluetoothControllerSramAddress, BluetoothInterruptRegisters,
    BluetoothSchedulerHardwareListIndex, BluetoothTaskRegisters, device_fence,
};

/// One validated scheduler insert-with-lock-modify request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerLockModifyRequest {
    address: BluetoothControllerSramAddress,
    hardware_list_index: BluetoothSchedulerHardwareListIndex,
}

impl BluetoothSchedulerLockModifyRequest {
    /// Bind one controller address to its scheduler hardware list.
    pub const fn new(
        address: BluetoothControllerSramAddress,
        hardware_list_index: BluetoothSchedulerHardwareListIndex,
    ) -> Self {
        Self {
            address,
            hardware_list_index,
        }
    }

    /// Return the validated CPU address without granting dereference access.
    pub const fn address(self) -> BluetoothControllerSramAddress {
        self.address
    }

    /// Return the hardware list selected for this item publication.
    pub const fn hardware_list_index(self) -> BluetoothSchedulerHardwareListIndex {
        self.hardware_list_index
    }
}

/// Task-owned request-register fields captured at one event step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the task-side scheduler observation must be joined or discarded"]
pub struct BluetoothSchedulerLockModifyTaskObservation {
    start: bool,
    result: u8,
}

impl BluetoothSchedulerLockModifyTaskObservation {
    /// Construct semantic task fields for host-side controller validation.
    #[cfg(any(feature = "validation-probes", test))]
    #[doc(hidden)]
    pub const fn from_fields_for_validation(start: bool, result: u8) -> Self {
        assert!(
            result <= 0x0f,
            "scheduler result must fit its reviewed field"
        );
        Self { start, result }
    }
}

/// Scheduler BUSY sampled through the interrupt-owned diagnostic pair at one
/// event step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "the interrupt-side scheduler observation must be joined or discarded"]
pub struct BluetoothSchedulerLockModifyInterruptObservation {
    busy: bool,
}

impl BluetoothSchedulerLockModifyInterruptObservation {
    /// Reconstruct one semantic interrupt observation after it crossed an
    /// atomic value-only handoff. No raw register image is accepted here.
    pub const fn from_busy(busy: bool) -> Self {
        Self { busy }
    }

    /// Whether the captured scheduler state was busy.
    pub const fn is_busy(self) -> bool {
        self.busy
    }
}

/// Scheduler-state and request fields sampled for one transition decision.
///
/// The pure type cannot prove temporal atomicity. A live controller must
/// construct it from the task/ISR coordinator at the exact decision point and
/// must not combine unrelated or stale observations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerLockModifyObservation {
    busy: bool,
    start: bool,
    result: u8,
}

impl BluetoothSchedulerLockModifyObservation {
    /// Join one task value and one interrupt value at the coordinator's
    /// current decision point.
    ///
    /// This performs no MMIO. The upper owner is responsible for accepting
    /// only observations belonging to the same scheduler event epoch.
    pub const fn from_split(
        interrupt: BluetoothSchedulerLockModifyInterruptObservation,
        task: BluetoothSchedulerLockModifyTaskObservation,
    ) -> Self {
        Self {
            busy: interrupt.busy,
            start: task.start,
            result: task.result,
        }
    }

    /// Construct semantic fields for host validation without reproducing PAC
    /// register geometry in a controller test.
    #[cfg(any(feature = "validation-probes", test))]
    #[doc(hidden)]
    pub const fn from_fields_for_validation(busy: bool, start: bool, result: u8) -> Self {
        assert!(
            result <= 0x0f,
            "scheduler result must fit its reviewed field"
        );
        Self {
            busy,
            start,
            result,
        }
    }

    /// Whether the reviewed wait predicate remains active.
    ///
    /// Progress is blocked only while both scheduler BUSY and request START
    /// remain set.
    pub const fn wait_active(self) -> bool {
        self.busy && self.start
    }

    /// Project the positional publication result after the in-flight wait ends.
    ///
    /// This method encodes the complete current body: idle scheduler state
    /// reports zero; otherwise bits 30:27 from the request word are retained.
    /// This is not a radio-event or descriptor-completion status. The
    /// Bluetooth state machine makes it public only after request publication.
    #[doc(hidden)]
    pub const fn result_code_after_publication(self) -> u8 {
        if !self.busy { 0 } else { self.result }
    }
}

/// Proof that the three ordered publication writes and trailing device fence
/// completed through the unique task owner.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "a published scheduler request remains in flight"]
pub struct BluetoothSchedulerLockModifyPublished {
    _private: (),
}

#[cfg(any(feature = "validation-probes", test))]
impl BluetoothSchedulerLockModifyPublished {
    /// Construct a host-only publication proof for controller state-machine
    /// validation. Production builds can obtain this value only from live PAC
    /// publication.
    #[doc(hidden)]
    pub const fn for_validation() -> Self {
        Self { _private: () }
    }
}

trait BluetoothSchedulerLockModifyControl {
    fn clear_hardware_list_index(&mut self);
    fn publish_hardware_list_index(&mut self, index: BluetoothSchedulerHardwareListIndex);
    fn publish_request(&mut self, address: BluetoothControllerSramAddress);
    fn order_after_publication(&mut self);
}

struct HardwareBluetoothSchedulerLockModifyControl<'registers> {
    registers: &'registers crate::svd::BluetoothControllerCore,
}

impl BluetoothSchedulerLockModifyControl for HardwareBluetoothSchedulerLockModifyControl<'_> {
    fn clear_hardware_list_index(&mut self) {
        crate::generated::clear_bluetooth_scheduler_lock_modify_hardware_list_index(self.registers);
    }

    fn publish_hardware_list_index(&mut self, index: BluetoothSchedulerHardwareListIndex) {
        let index = crate::generated::BluetoothSchedulerLockModifyHardwareListIndex::new(
            u32::from(index.get()),
        )
        .expect("typed scheduler hardware-list index fits its generated PAC domain");
        crate::generated::publish_bluetooth_scheduler_lock_modify_hardware_list_index(
            self.registers,
            index,
        );
    }

    fn publish_request(&mut self, address: BluetoothControllerSramAddress) {
        crate::svd::zero_based_field_write::publish_bluetooth_scheduler_lock_modify_request(
            self.registers,
            address.compressed_image(),
            true,
        );
    }

    fn order_after_publication(&mut self) {
        device_fence();
    }
}

fn execute_scheduler_lock_modify_publication(
    control: &mut impl BluetoothSchedulerLockModifyControl,
    request: BluetoothSchedulerLockModifyRequest,
) -> BluetoothSchedulerLockModifyPublished {
    control.clear_hardware_list_index();
    control.publish_hardware_list_index(request.hardware_list_index());

    control.publish_request(request.address());
    control.order_after_publication();
    BluetoothSchedulerLockModifyPublished { _private: () }
}

impl BluetoothTaskRegisters {
    /// Execute the finite task-side publication body and return after one
    /// device-ordering fence.
    ///
    /// The controller layer admits this operation only after a fresh split
    /// observation ended the pre-publication wait. The PAC method itself owns
    /// all three writes: two independent fresh-read updates of the operational
    /// word followed by the typed request publication.
    ///
    /// # Safety
    ///
    /// The request address must identify an initialized scheduler item that
    /// remains pinned and inaccessible to the CPU until the Controller returns
    /// ownership. The caller must also serialize the transaction with every
    /// scheduler task and interrupt owner.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller retains descriptor lifetime and scheduler serialization"
    )]
    pub unsafe fn publish_scheduler_lock_modify(
        &mut self,
        request: BluetoothSchedulerLockModifyRequest,
    ) -> BluetoothSchedulerLockModifyPublished {
        let mut control = HardwareBluetoothSchedulerLockModifyControl {
            registers: &self.bluetooth.bluetooth_controller_core,
        };
        execute_scheduler_lock_modify_publication(&mut control, request)
    }

    /// Capture task-owned START and RESULT fields without exposing their raw
    /// register image or its generated bit geometry.
    pub fn capture_scheduler_lock_modify_task(
        &mut self,
    ) -> BluetoothSchedulerLockModifyTaskObservation {
        let (start, result) =
            crate::svd::field_snapshot_read::observe_bluetooth_scheduler_lock_modify_request(
                &self.bluetooth.bluetooth_controller_core,
            );
        BluetoothSchedulerLockModifyTaskObservation { start, result }
    }
}

impl BluetoothInterruptRegisters {
    /// Sample scheduler BUSY once through the interrupt-owned diagnostic pair
    /// without borrowing any task-side controller register; `None` when the
    /// two reads disagreed.
    pub fn capture_scheduler_lock_modify_interrupt(
        &mut self,
    ) -> Option<BluetoothSchedulerLockModifyInterruptObservation> {
        super::runtime::sample_scheduler_busy(
            &self.peripherals.bluetooth_scheduler_interrupt_runtime,
        )
        .map(|busy| BluetoothSchedulerLockModifyInterruptObservation { busy })
    }
}

#[cfg(test)]
mod tests;
