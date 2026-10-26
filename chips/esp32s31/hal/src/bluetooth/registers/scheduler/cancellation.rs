//! Restricted scheduler cancellation transactions.
//!
//! The vendor cancellation paths skip listed items one at a time through the
//! skip request and, when lock/modify is enabled, bracket or follow those
//! requests with a control sequence: publish the hardware-list index,
//! acknowledge interrupt source 7, set operational control bit zero, wait for
//! two status edges and clear the control bit. The hardware effect of either
//! operation is not established, so every observation keeps its positional
//! result. Publications are finite; hardware-owned waits are split into fresh
//! observations, like the insertion commands.

#![deny(unsafe_code)]

use crate::layouts::{BleSchedulerItem, DmaRef};

use crate::bluetooth::registers::{
    BluetoothInterruptRegisters, BluetoothPlacements, BluetoothSchedulerBusyObservation,
    BluetoothSchedulerHardwareListIndex, BluetoothTaskRegisters, device_fence,
};

/// Validated listed item and its hardware list for one skip request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerSkipRequest {
    address: DmaRef<BleSchedulerItem>,
    hardware_list_index: BluetoothSchedulerHardwareListIndex,
}

impl BluetoothSchedulerSkipRequest {
    /// Bind one listed item to the hardware list that holds it.
    pub const fn new(
        address: DmaRef<BleSchedulerItem>,
        hardware_list_index: BluetoothSchedulerHardwareListIndex,
    ) -> Self {
        Self {
            address,
            hardware_list_index,
        }
    }

    /// Listed item address without dereference authority.
    pub const fn address(self) -> DmaRef<BleSchedulerItem> {
        self.address
    }

    /// Hardware list that holds the item.
    pub const fn hardware_list_index(self) -> BluetoothSchedulerHardwareListIndex {
        self.hardware_list_index
    }
}

/// Proof that one skip request and its trailing device fence were published.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "a published skip request must be observed and cleared"]
pub struct BluetoothSchedulerSkipPublished {
    _private: (),
}

/// Positional two-bit skip result reported while the scheduler was busy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothSchedulerSkipResult {
    /// Positional result one; the vendor cancellation loop continues.
    One,
    /// Positional result two; the vendor cancellation loop stops.
    Two,
    /// Positional result three; the vendor cancellation loop continues.
    Three,
}

/// Result of one finite skip-request observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "pending work must return to the executor; terminal work must clear the request"]
pub enum BluetoothSchedulerSkipDisposition {
    /// The scheduler is busy and the request is still started.
    Pending,
    /// The request completed while the scheduler was busy.
    Completed(BluetoothSchedulerSkipResult),
    /// The scheduler became idle before the request completed. The vendor
    /// reports this as positional result four and stops its loop.
    SchedulerIdle,
    /// Positional result zero, on which the vendor callers assert.
    UnsupportedHardwareResult,
}

/// Proof that a completed skip request was cleared.
#[derive(Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerSkipCleared {
    _private: (),
}

/// Proof that the cancellation hardware-list index was published.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the indexed cancellation control still requires its acknowledgement and request"]
pub struct BluetoothSchedulerCancellationIndexed {
    hardware_list_index: BluetoothSchedulerHardwareListIndex,
}

impl BluetoothSchedulerCancellationIndexed {
    /// Hardware list named by the published index.
    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.hardware_list_index
    }
}

/// Proof that interrupt source 7 was acknowledged for one cancellation.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "the acknowledgement admits exactly one cancellation control request"]
pub struct BluetoothSchedulerCancellationSourceAcknowledged {
    _private: (),
}

/// Waiting phase of a requested cancellation control.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CancellationControlPhase {
    AwaitingStatus0324,
    AwaitingStatus0208,
}

/// Proof that operational control bit zero was set for one hardware list.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "a requested cancellation control must be observed and released"]
pub struct BluetoothSchedulerCancellationRequested {
    hardware_list_index: BluetoothSchedulerHardwareListIndex,
    phase: CancellationControlPhase,
}

impl BluetoothSchedulerCancellationRequested {
    /// Hardware list named by the request.
    pub const fn hardware_list_index(&self) -> BluetoothSchedulerHardwareListIndex {
        self.hardware_list_index
    }
}

/// Result of one finite cancellation-control observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "pending work must return to the executor"]
pub enum BluetoothSchedulerCancellationDisposition {
    /// The scheduler is busy and a vendor wait predicate still holds.
    Pending,
    /// Both vendor wait predicates ended while the scheduler was busy.
    Settled,
    /// The scheduler became idle, which also ends both vendor waits.
    SchedulerIdle,
}

/// Proof that operational control bit zero was cleared.
#[derive(Debug, Eq, PartialEq)]
pub struct BluetoothSchedulerCancellationReleased {
    _private: (),
}

trait BluetoothSchedulerCancellationControl {
    fn publish_skip(&mut self, request: BluetoothSchedulerSkipRequest);
    fn observe_skip(&mut self) -> (bool, u8);
    fn clear_skip(&mut self);
    fn clear_list_index(&mut self);
    fn publish_list_index(&mut self, index: BluetoothSchedulerHardwareListIndex);
    fn set_control(&mut self);
    fn clear_control(&mut self);
    fn observe_status_0324(&mut self) -> bool;
    fn observe_status_0208(&mut self) -> bool;
    fn order_after_publication(&mut self);
}

struct HardwareBluetoothSchedulerCancellationControl<'registers, B: oer_mmio::Placement> {
    registers: &'registers mut oer_esp32s31_pac::bt_mac::BluetoothControllerCore<B>,
}

impl<B: oer_mmio::Placement> BluetoothSchedulerCancellationControl
    for HardwareBluetoothSchedulerCancellationControl<'_, B>
{
    fn publish_skip(&mut self, request: BluetoothSchedulerSkipRequest) {
        // The vendor stores the request from zero: every other field of the
        // word is written zero.
        self.registers.modify_scheduler_skip_request(|w| {
            w.set_compressed_sram_pointer(request.address());
            w.set_hardware_list_index(request.hardware_list_index().field_bits());
            w.set_result(oer_mmio::Bits::constant::<0>());
            w.set_start(true);
            w.set_bits_24_27_opaque(oer_mmio::Bits::constant::<0>());
            w.set_bit_30_opaque(false);
        });
    }

    fn observe_skip(&mut self) -> (bool, u8) {
        let request = self.registers.read_scheduler_skip_request();
        (
            request.start(),
            crate::bluetooth::registers::field_byte(request.result()),
        )
    }

    fn clear_skip(&mut self) {
        // The vendor stores the zero image: every field of the word is
        // written zero.
        self.registers.modify_scheduler_skip_request(|w| {
            w.clear_compressed_sram_pointer();
            w.set_hardware_list_index(oer_mmio::Bits::constant::<0>());
            w.set_result(oer_mmio::Bits::constant::<0>());
            w.set_start(false);
            w.set_bits_24_27_opaque(oer_mmio::Bits::constant::<0>());
            w.set_bit_30_opaque(false);
        });
    }

    fn clear_list_index(&mut self) {
        self.registers.modify_word_036c_opaque(|w| {
            w.set_cancellation_hardware_list_index(oer_mmio::Bits::constant::<0x0>());
        });
    }

    fn publish_list_index(&mut self, index: BluetoothSchedulerHardwareListIndex) {
        self.registers.modify_word_036c_opaque(|w| {
            w.set_cancellation_hardware_list_index(index.field_bits());
        });
    }

    fn set_control(&mut self) {
        self.registers.modify_word_0204_opaque(|w| {
            w.set_control_0(true);
        });
    }

    fn clear_control(&mut self) {
        self.registers.modify_word_0204_opaque(|w| {
            w.set_control_0(false);
        });
    }

    fn observe_status_0324(&mut self) -> bool {
        self.registers.read_word_0324_opaque().status_0()
    }

    fn observe_status_0208(&mut self) -> bool {
        self.registers.read_word_0208_opaque().status_0()
    }

    fn order_after_publication(&mut self) {
        device_fence();
    }
}

fn execute_skip_publication(
    control: &mut impl BluetoothSchedulerCancellationControl,
    request: BluetoothSchedulerSkipRequest,
) -> BluetoothSchedulerSkipPublished {
    control.publish_skip(request);
    control.order_after_publication();
    BluetoothSchedulerSkipPublished { _private: () }
}

fn execute_skip_observation(
    control: &mut impl BluetoothSchedulerCancellationControl,
    scheduler: BluetoothSchedulerBusyObservation,
) -> BluetoothSchedulerSkipDisposition {
    if !scheduler.is_busy() {
        return BluetoothSchedulerSkipDisposition::SchedulerIdle;
    }
    let (start, result) = control.observe_skip();
    if start {
        return BluetoothSchedulerSkipDisposition::Pending;
    }
    match result {
        1 => BluetoothSchedulerSkipDisposition::Completed(BluetoothSchedulerSkipResult::One),
        2 => BluetoothSchedulerSkipDisposition::Completed(BluetoothSchedulerSkipResult::Two),
        3 => BluetoothSchedulerSkipDisposition::Completed(BluetoothSchedulerSkipResult::Three),
        _ => BluetoothSchedulerSkipDisposition::UnsupportedHardwareResult,
    }
}

fn execute_skip_clear(
    control: &mut impl BluetoothSchedulerCancellationControl,
    _published: BluetoothSchedulerSkipPublished,
) -> BluetoothSchedulerSkipCleared {
    control.clear_skip();
    control.order_after_publication();
    BluetoothSchedulerSkipCleared { _private: () }
}

fn execute_cancellation_indexing(
    control: &mut impl BluetoothSchedulerCancellationControl,
    index: BluetoothSchedulerHardwareListIndex,
) -> BluetoothSchedulerCancellationIndexed {
    control.clear_list_index();
    control.publish_list_index(index);
    BluetoothSchedulerCancellationIndexed {
        hardware_list_index: index,
    }
}

fn execute_cancellation_request(
    control: &mut impl BluetoothSchedulerCancellationControl,
    indexed: BluetoothSchedulerCancellationIndexed,
    _acknowledged: BluetoothSchedulerCancellationSourceAcknowledged,
) -> BluetoothSchedulerCancellationRequested {
    control.set_control();
    control.order_after_publication();
    BluetoothSchedulerCancellationRequested {
        hardware_list_index: indexed.hardware_list_index,
        phase: CancellationControlPhase::AwaitingStatus0324,
    }
}

fn execute_cancellation_observation(
    control: &mut impl BluetoothSchedulerCancellationControl,
    requested: &mut BluetoothSchedulerCancellationRequested,
    scheduler: BluetoothSchedulerBusyObservation,
) -> BluetoothSchedulerCancellationDisposition {
    if !scheduler.is_busy() {
        return BluetoothSchedulerCancellationDisposition::SchedulerIdle;
    }
    // The vendor waits are sequential: the second predicate is sampled only
    // after the first ended, and the first is not sampled again.
    if requested.phase == CancellationControlPhase::AwaitingStatus0324 {
        if !control.observe_status_0324() {
            return BluetoothSchedulerCancellationDisposition::Pending;
        }
        requested.phase = CancellationControlPhase::AwaitingStatus0208;
    }
    if control.observe_status_0208() {
        BluetoothSchedulerCancellationDisposition::Pending
    } else {
        BluetoothSchedulerCancellationDisposition::Settled
    }
}

fn execute_cancellation_release(
    control: &mut impl BluetoothSchedulerCancellationControl,
    _requested: BluetoothSchedulerCancellationRequested,
) -> BluetoothSchedulerCancellationReleased {
    control.clear_control();
    control.order_after_publication();
    BluetoothSchedulerCancellationReleased { _private: () }
}

impl<P: BluetoothPlacements> BluetoothTaskRegisters<P> {
    /// Publish one skip request and return after its trailing device fence.
    ///
    /// The request carries a typed reference to the listed item in retained pool
    /// storage. The scheduler transaction retains the returned publication until
    /// observation and clear; the exclusive register loan serializes the writes.
    #[doc(hidden)]
    pub fn publish_scheduler_skip(
        &mut self,
        request: BluetoothSchedulerSkipRequest,
    ) -> BluetoothSchedulerSkipPublished {
        let mut control = HardwareBluetoothSchedulerCancellationControl {
            registers: &mut self.bluetooth.bluetooth_controller_core,
        };
        execute_skip_publication(&mut control, request)
    }

    /// Perform one finite skip-request observation. An idle scheduler
    /// performs no request-register read.
    pub fn observe_scheduler_skip(
        &mut self,
        _published: &BluetoothSchedulerSkipPublished,
        scheduler: BluetoothSchedulerBusyObservation,
    ) -> BluetoothSchedulerSkipDisposition {
        let mut control = HardwareBluetoothSchedulerCancellationControl {
            registers: &mut self.bluetooth.bluetooth_controller_core,
        };
        execute_skip_observation(&mut control, scheduler)
    }

    /// Write zero to the skip request after a terminal observation.
    pub fn clear_scheduler_skip(
        &mut self,
        published: BluetoothSchedulerSkipPublished,
    ) -> BluetoothSchedulerSkipCleared {
        let mut control = HardwareBluetoothSchedulerCancellationControl {
            registers: &mut self.bluetooth.bluetooth_controller_core,
        };
        execute_skip_clear(&mut control, published)
    }

    /// Clear and then publish the cancellation hardware-list index through
    /// two fresh-read updates.
    ///
    /// The scheduler executor runs this step only after lock/modify becomes
    /// idle, and the transaction retains its affine index token through
    /// acknowledgement and request. The exclusive register loan serializes the operational word.
    #[doc(hidden)]
    pub fn index_scheduler_cancellation(
        &mut self,
        index: BluetoothSchedulerHardwareListIndex,
    ) -> BluetoothSchedulerCancellationIndexed {
        let mut control = HardwareBluetoothSchedulerCancellationControl {
            registers: &mut self.bluetooth.bluetooth_controller_core,
        };
        execute_cancellation_indexing(&mut control, index)
    }

    /// Set operational control bit zero after the index and the source-7
    /// acknowledgement, and return after a trailing device fence.
    pub fn request_scheduler_cancellation(
        &mut self,
        indexed: BluetoothSchedulerCancellationIndexed,
        acknowledged: BluetoothSchedulerCancellationSourceAcknowledged,
    ) -> BluetoothSchedulerCancellationRequested {
        let mut control = HardwareBluetoothSchedulerCancellationControl {
            registers: &mut self.bluetooth.bluetooth_controller_core,
        };
        execute_cancellation_request(&mut control, indexed, acknowledged)
    }

    /// Perform one finite observation of the two sequential vendor waits.
    pub fn observe_scheduler_cancellation(
        &mut self,
        requested: &mut BluetoothSchedulerCancellationRequested,
        scheduler: BluetoothSchedulerBusyObservation,
    ) -> BluetoothSchedulerCancellationDisposition {
        let mut control = HardwareBluetoothSchedulerCancellationControl {
            registers: &mut self.bluetooth.bluetooth_controller_core,
        };
        execute_cancellation_observation(&mut control, requested, scheduler)
    }

    /// Clear operational control bit zero and return after a trailing
    /// device fence.
    pub fn release_scheduler_cancellation(
        &mut self,
        requested: BluetoothSchedulerCancellationRequested,
    ) -> BluetoothSchedulerCancellationReleased {
        let mut control = HardwareBluetoothSchedulerCancellationControl {
            registers: &mut self.bluetooth.bluetooth_controller_core,
        };
        execute_cancellation_release(&mut control, requested)
    }
}

impl<P: BluetoothPlacements> BluetoothInterruptRegisters<P> {
    /// Acknowledge interrupt source 7 for one indexed cancellation.
    pub fn acknowledge_scheduler_cancellation_source(
        &mut self,
        _indexed: &BluetoothSchedulerCancellationIndexed,
    ) -> BluetoothSchedulerCancellationSourceAcknowledged {
        self.peripherals
            .bluetooth_interrupt_bank
            .acknowledge_bluetooth_scheduler_cancellation_source();
        device_fence();
        BluetoothSchedulerCancellationSourceAcknowledged { _private: () }
    }
}

#[cfg(test)]
mod tests;
