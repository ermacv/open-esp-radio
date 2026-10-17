//! Single transactions of the reviewed common-library scheduler stop.
//!
//! The HAL sequences preamble, request and idle confirmation and owns the
//! pending progress. Neither an idle sample nor the stopped receipt alone
//! releases descriptor/software ownership.

use crate::{BluetoothInterruptRegisters, BluetoothTaskRegisters, device_fence};

/// The reviewed lifecycle sequence reached BUSY clear and a device fence.
/// This is global scheduler evidence, not descriptor reclamation permission.
#[derive(Debug)]
#[must_use]
pub struct BluetoothSchedulerStopped {
    _private: (),
}

impl BluetoothSchedulerStopped {
    /// Construct a stopped receipt for host validation of its owners.
    #[cfg(any(feature = "validation-probes", test))]
    #[doc(hidden)]
    pub const fn for_validation() -> Self {
        Self { _private: () }
    }
}

/// Exact stopped item after fenced hardware-head retirement. This affine
/// capability authorizes one bound memory status sample, not CPU recycling.
#[derive(Debug)]
#[must_use]
pub struct BluetoothSchedulerStoppedItem {
    head: crate::BluetoothSchedulerHardwareListHeadEmptyObserved,
}
impl BluetoothSchedulerStoppedItem {
    pub const fn head(&self) -> &crate::BluetoothSchedulerHardwareListHeadEmptyObserved {
        &self.head
    }
    pub fn into_head(self) -> crate::BluetoothSchedulerHardwareListHeadEmptyObserved {
        self.head
    }
    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub const fn from_head_for_validation(
        head: crate::BluetoothSchedulerHardwareListHeadEmptyObserved,
    ) -> Self {
        Self { head }
    }
}

#[must_use]
pub enum BluetoothSchedulerStoppedHeadRetirement {
    Retired(BluetoothSchedulerStoppedItem),
    Rejected(crate::BluetoothSchedulerHardwareListHeadRetirementObservation),
}

trait HeadControl {
    /// Every observation includes its trailing device fence.
    fn observe(
        &mut self,
        index: crate::BluetoothSchedulerHardwareListIndex,
    ) -> crate::BluetoothSchedulerHardwareListHead;
    /// Clear only the pointer field, preserve unrelated fields, then fence.
    fn clear(&mut self, index: crate::BluetoothSchedulerHardwareListIndex);
}

impl HeadControl for BluetoothTaskRegisters {
    fn observe(
        &mut self,
        index: crate::BluetoothSchedulerHardwareListIndex,
    ) -> crate::BluetoothSchedulerHardwareListHead {
        super::execute_scheduler_hardware_list_head_observation(self, index)
    }
    fn clear(&mut self, index: crate::BluetoothSchedulerHardwareListIndex) {
        crate::generated::clear_bluetooth_scheduler_hardware_list_head(
            &self.bluetooth.btdm_scheduler_table,
            index.get() as usize,
        );
        device_fence();
    }
}

fn retire_head(
    control: &mut impl HeadControl,
    index: crate::BluetoothSchedulerHardwareListIndex,
    expected: crate::BluetoothSchedulerHardwareListHead,
) -> (
    super::BluetoothSchedulerHardwareListHeadRetirementDisposition,
    crate::BluetoothSchedulerHardwareListHead,
) {
    let observed = control.observe(index);
    if observed.address().is_some() && observed != expected {
        return (
            super::BluetoothSchedulerHardwareListHeadRetirementDisposition::UnexpectedHeadChanged,
            observed,
        );
    }
    if observed == expected {
        control.clear(index);
    }
    let observed = control.observe(index);
    (
        super::classify_scheduler_hardware_list_head_retirement(expected, observed),
        observed,
    )
}

impl BluetoothTaskRegisters {
    /// Mask both dynamic scheduler run-interrupt banks, disable the RUN event
    /// source, then fence.
    #[doc(hidden)]
    pub fn publish_scheduler_stop_preamble(
        &mut self,
        interrupts: &mut BluetoothInterruptRegisters,
    ) {
        let bank = &interrupts.peripherals.bluetooth_interrupt_bank;
        crate::generated::mask_bluetooth_scheduler_run_interrupts_bank_0(bank);
        crate::generated::mask_bluetooth_scheduler_run_interrupts_bank_1(bank);
        crate::generated::disable_ble_scheduler_run_event_source(
            &self.bluetooth.btmac_ble_phy_init,
        );
        device_fence();
    }

    /// Read command-zero status 26 and, only when it is ready, command-one
    /// status 18.
    #[doc(hidden)]
    pub fn scheduler_stop_commands_ready(&mut self) -> bool {
        let core = &self.bluetooth.bluetooth_controller_core;
        crate::svd::field_read::observe_bluetooth_scheduler_software_list_command_0_status_26(core)
            && crate::svd::field_read::observe_bluetooth_scheduler_software_list_command_1_status_18(
                core,
            )
    }

    /// Publish the fixed scheduler lifecycle request image, then fence.
    #[doc(hidden)]
    pub fn publish_scheduler_lifecycle_request(&mut self) {
        crate::svd::fixed_register_image::publish_bluetooth_scheduler_lifecycle_request(
            &self.bluetooth.bluetooth_controller_core,
        );
        device_fence();
    }

    /// When the BUSY sample is clear, fence and return the stopped receipt.
    /// Sampling BUSY and sequencing the stop request belong to the HAL.
    #[doc(hidden)]
    pub fn confirm_scheduler_stopped(
        &mut self,
        busy: crate::BluetoothSchedulerBusyObservation,
    ) -> Option<BluetoothSchedulerStopped> {
        if busy.is_busy() {
            return None;
        }
        device_fence();
        Some(BluetoothSchedulerStopped { _private: () })
    }

    /// Detach only the exact RUN head after common stop. A foreign head is
    /// retained as an invariant failure. The final fresh fenced read is still
    /// required, and software unlink/recycle remain separate transitions.
    pub fn retire_stopped_scheduler_head(
        &mut self,
        _stopped: BluetoothSchedulerStopped,
        run: crate::BluetoothSchedulerHardwareRunCommandPublished,
    ) -> BluetoothSchedulerStoppedHeadRetirement {
        use super::BluetoothSchedulerHardwareListHeadRetirementDisposition as Disposition;
        let (disposition, observed) = retire_head(self, run.index(), run.head());
        match disposition {
            Disposition::Empty => BluetoothSchedulerStoppedHeadRetirement::Retired(BluetoothSchedulerStoppedItem {
                head: crate::BluetoothSchedulerHardwareListHeadEmptyObserved { index: run.index(), completed_head: run.head() },
            }),
            Disposition::ExpectedHeadStillPublished => BluetoothSchedulerStoppedHeadRetirement::Rejected(
                crate::BluetoothSchedulerHardwareListHeadRetirementObservation::ExpectedHeadStillPublished { run, observed }),
            Disposition::UnexpectedHeadChanged => BluetoothSchedulerStoppedHeadRetirement::Rejected(
                crate::BluetoothSchedulerHardwareListHeadRetirementObservation::UnexpectedHeadChanged { run, observed }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    struct Heads {
        reads: VecDeque<crate::BluetoothSchedulerHardwareListHead>,
        clears: usize,
    }
    impl HeadControl for Heads {
        fn observe(
            &mut self,
            _: crate::BluetoothSchedulerHardwareListIndex,
        ) -> crate::BluetoothSchedulerHardwareListHead {
            self.reads.pop_front().unwrap()
        }
        fn clear(&mut self, _: crate::BluetoothSchedulerHardwareListIndex) {
            self.clears += 1;
        }
    }
    #[test]
    fn stop_detaches_only_its_exact_head_and_verifies_retirement() {
        use super::super::BluetoothSchedulerHardwareListHeadRetirementDisposition as D;
        let expected = crate::BluetoothSchedulerHardwareListHead::from_address(
            crate::BluetoothControllerSramAddress::new(0x2f00_0400).unwrap(),
        )
        .unwrap();
        let foreign = crate::BluetoothSchedulerHardwareListHead::from_address(
            crate::BluetoothControllerSramAddress::new(0x2f00_0800).unwrap(),
        )
        .unwrap();
        let empty = crate::BluetoothSchedulerHardwareListHead::empty();
        let index = crate::BluetoothSchedulerHardwareListIndex::ZERO;
        let mut model = Heads {
            reads: [expected, empty].into(),
            clears: 0,
        };
        assert!(matches!(
            retire_head(&mut model, index, expected).0,
            D::Empty
        ));
        assert_eq!(model.clears, 1);
        let mut model = Heads {
            reads: [foreign].into(),
            clears: 0,
        };
        assert!(matches!(
            retire_head(&mut model, index, expected).0,
            D::UnexpectedHeadChanged
        ));
        assert_eq!(model.clears, 0);
        let mut model = Heads {
            reads: [expected, expected].into(),
            clears: 0,
        };
        assert!(matches!(
            retire_head(&mut model, index, expected).0,
            D::ExpectedHeadStillPublished
        ));
        let mut model = Heads {
            reads: [empty, empty].into(),
            clears: 0,
        };
        assert!(matches!(
            retire_head(&mut model, index, expected).0,
            D::Empty
        ));
        assert_eq!(model.clears, 0);
    }
}
