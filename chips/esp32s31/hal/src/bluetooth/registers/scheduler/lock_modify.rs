//! Affine transaction for scheduler insert-with-lock-modify.
//!
//! The task-side request register and interrupt-side scheduler-state register
//! deliberately have different physical owners. Publication is one finite
//! task-owned MMIO transaction. Wait observations remain split into two value
//! tokens until the controller coordinator joins them at one decision point;
//! neither owner aliases the other register partition.

#![deny(unsafe_code)]

use crate::layouts::{BleSchedulerItem, DmaRef};

use crate::bluetooth::registers::{
    BluetoothInterruptRegisters, BluetoothPlacements, BluetoothSchedulerHardwareListIndex,
    BluetoothTaskRegisters, device_fence,
};

/// One validated scheduler insert-with-lock-modify request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerLockModifyRequest {
    address: DmaRef<BleSchedulerItem>,
    hardware_list_index: BluetoothSchedulerHardwareListIndex,
}

impl BluetoothSchedulerLockModifyRequest {
    /// Bind one controller address to its scheduler hardware list.
    pub const fn new(
        address: DmaRef<BleSchedulerItem>,
        hardware_list_index: BluetoothSchedulerHardwareListIndex,
    ) -> Self {
        Self {
            address,
            hardware_list_index,
        }
    }

    /// Return the validated CPU address without granting dereference access.
    pub const fn address(self) -> DmaRef<BleSchedulerItem> {
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
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub const fn from_fields_for_host_test(start: bool, result: u8) -> Self {
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

    /// Construct semantic fields for host validation without reproducing
    /// register geometry in a controller test.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub const fn from_fields_for_host_test(busy: bool, start: bool, result: u8) -> Self {
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

#[cfg(test)]
impl BluetoothSchedulerLockModifyPublished {
    /// Construct a host-only publication proof for controller state-machine
    /// validation. Production builds can obtain this value only from live
    /// publication.
    #[doc(hidden)]
    pub const fn for_host_test() -> Self {
        Self { _private: () }
    }
}

trait BluetoothSchedulerLockModifyControl {
    fn clear_hardware_list_index(&mut self);
    fn publish_hardware_list_index(&mut self, index: BluetoothSchedulerHardwareListIndex);
    fn publish_request(&mut self, address: DmaRef<BleSchedulerItem>);
    fn order_after_publication(&mut self);
}

struct HardwareBluetoothSchedulerLockModifyControl<'registers, B: oer_mmio::Placement> {
    registers: &'registers mut oer_esp32s31_pac::bt_mac::BluetoothControllerCore<B>,
}

impl<B: oer_mmio::Placement> BluetoothSchedulerLockModifyControl
    for HardwareBluetoothSchedulerLockModifyControl<'_, B>
{
    fn clear_hardware_list_index(&mut self) {
        self.registers.modify_word_036c_opaque(|w| {
            w.set_lock_modify_hardware_list_index(oer_mmio::Bits::constant::<0x0>());
        });
    }

    fn publish_hardware_list_index(&mut self, index: BluetoothSchedulerHardwareListIndex) {
        self.registers.modify_word_036c_opaque(|w| {
            w.set_lock_modify_hardware_list_index(index.field_bits());
        });
    }

    fn publish_request(&mut self, address: DmaRef<BleSchedulerItem>) {
        // The vendor stores the request from zero: every other field of the
        // word is written zero.
        self.registers.modify_scheduler_lock_modify_request(|w| {
            w.set_compressed_sram_pointer(address);
            w.set_zero_image_20_26(oer_mmio::Bits::constant::<0>());
            w.set_result(oer_mmio::Bits::constant::<0>());
            w.set_start(true);
        });
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

impl<P: BluetoothPlacements> BluetoothTaskRegisters<P> {
    /// Execute the finite task-side publication body and return after one
    /// device-ordering fence.
    ///
    /// The controller layer admits this operation only after a fresh split
    /// observation ended the pre-publication wait. The method itself owns
    /// all three writes: two independent fresh-read updates of the operational
    /// word followed by the typed request publication.
    ///
    /// The request carries a typed reference to the initialized item in retained
    /// pool storage. The engine's scheduler state retains its publication until
    /// ownership returns; the exclusive register loan serializes these writes.
    #[doc(hidden)]
    pub fn publish_scheduler_lock_modify(
        &mut self,
        request: BluetoothSchedulerLockModifyRequest,
    ) -> BluetoothSchedulerLockModifyPublished {
        let mut control = HardwareBluetoothSchedulerLockModifyControl {
            registers: &mut self.bluetooth.bluetooth_controller_core,
        };
        execute_scheduler_lock_modify_publication(&mut control, request)
    }

    /// Capture task-owned START and RESULT fields without exposing their raw
    /// register image or its generated bit geometry.
    pub fn capture_scheduler_lock_modify_task(
        &mut self,
    ) -> BluetoothSchedulerLockModifyTaskObservation {
        let request = self
            .bluetooth
            .bluetooth_controller_core
            .read_scheduler_lock_modify_request();
        let (start, result) = (
            request.start(),
            crate::bluetooth::registers::field_byte(request.result()),
        );
        BluetoothSchedulerLockModifyTaskObservation { start, result }
    }
}

impl<P: BluetoothPlacements> BluetoothInterruptRegisters<P> {
    /// Sample scheduler BUSY once through the interrupt-owned diagnostic pair
    /// without borrowing any task-side controller register; `None` when the
    /// two reads disagreed.
    pub fn capture_scheduler_lock_modify_interrupt(
        &mut self,
    ) -> Option<BluetoothSchedulerLockModifyInterruptObservation> {
        super::runtime::sample_scheduler_busy(
            &mut self.peripherals.bluetooth_scheduler_interrupt_runtime,
        )
        .map(|busy| BluetoothSchedulerLockModifyInterruptObservation { busy })
    }
}

#[cfg(test)]
mod tests;
