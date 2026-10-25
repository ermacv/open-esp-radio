//! Single transactions of the reviewed common-library scheduler stop.
//!
//! The HAL sequences preamble, request and idle confirmation and owns the
//! pending progress. Neither an idle sample nor the stopped receipt alone
//! releases descriptor/software ownership.

use crate::bluetooth::registers::{
    BluetoothInterruptRegisters, BluetoothPlacements, BluetoothTaskRegisters, device_fence,
};

/// The reviewed lifecycle sequence reached BUSY clear and a device fence.
/// This is global scheduler evidence, not descriptor reclamation permission.
#[derive(Debug)]
#[must_use]
pub struct BluetoothSchedulerStopped {
    _private: (),
}

impl BluetoothSchedulerStopped {
    /// Construct a stopped receipt for host validation of its owners.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub const fn for_host_test() -> Self {
        Self { _private: () }
    }
}

/// Exact stopped item after fenced hardware-head retirement. This affine
/// capability authorizes one bound memory status sample, not CPU recycling.
#[derive(Debug)]
#[must_use]
pub struct BluetoothSchedulerStoppedItem {
    head: crate::bluetooth::registers::BluetoothSchedulerHardwareListHeadEmptyObserved,
}
impl BluetoothSchedulerStoppedItem {
    pub const fn head(
        &self,
    ) -> &crate::bluetooth::registers::BluetoothSchedulerHardwareListHeadEmptyObserved {
        &self.head
    }
    pub fn into_head(
        self,
    ) -> crate::bluetooth::registers::BluetoothSchedulerHardwareListHeadEmptyObserved {
        self.head
    }
}

#[must_use]
pub enum BluetoothSchedulerStoppedHeadRetirement {
    Retired(BluetoothSchedulerStoppedItem),
    Rejected(crate::bluetooth::registers::BluetoothSchedulerHardwareListHeadRetirementObservation),
}

trait HeadControl {
    /// Every observation includes its trailing device fence.
    fn observe(
        &mut self,
        index: crate::bluetooth::registers::BluetoothSchedulerHardwareListIndex,
    ) -> crate::bluetooth::registers::BluetoothSchedulerHardwareListHead;
    /// Clear only the pointer field, preserve unrelated fields, then fence.
    fn clear(&mut self, index: crate::bluetooth::registers::BluetoothSchedulerHardwareListIndex);
}

impl<P: BluetoothPlacements> HeadControl for BluetoothTaskRegisters<P> {
    fn observe(
        &mut self,
        index: crate::bluetooth::registers::BluetoothSchedulerHardwareListIndex,
    ) -> crate::bluetooth::registers::BluetoothSchedulerHardwareListHead {
        super::execute_scheduler_hardware_list_head_observation(self, index)
    }
    fn clear(&mut self, index: crate::bluetooth::registers::BluetoothSchedulerHardwareListIndex) {
        self.bluetooth
            .btdm_scheduler_table
            .modify_entry(index.register_index(), |w| {
                w.clear_head_pointer();
            });
        device_fence();
    }
}

fn retire_head(
    control: &mut impl HeadControl,
    index: crate::bluetooth::registers::BluetoothSchedulerHardwareListIndex,
    expected: crate::bluetooth::registers::BluetoothSchedulerHardwareListHead,
) -> (
    super::BluetoothSchedulerHardwareListHeadRetirementDisposition,
    crate::bluetooth::registers::BluetoothSchedulerHardwareListHead,
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

impl<P: BluetoothPlacements> BluetoothTaskRegisters<P> {
    /// Disable the RUN event source after P3 acknowledged both bank masks.
    #[doc(hidden)]
    pub fn disable_scheduler_run_event(&mut self) {
        self.bluetooth
            .btmac_ble_phy_init
            .modify_interrupt_enable(|w| {
                w.set_scheduler_run(false);
            });
        device_fence();
    }

    /// Read command-zero status 26 and, only when it is ready, command-one
    /// status 18.
    #[doc(hidden)]
    pub fn scheduler_stop_commands_ready(&mut self) -> bool {
        let core = &self.bluetooth.bluetooth_controller_core;
        core.read_scheduler_command_0().status_26() && core.read_scheduler_command_1().status_18()
    }

    /// Publish the fixed scheduler lifecycle request image, then fence.
    #[doc(hidden)]
    pub fn publish_scheduler_lifecycle_request(&mut self) {
        self.bluetooth
            .bluetooth_controller_core
            .write_scheduler_lifecycle_request(|w| {
                w.set_value(0x1);
            });
        device_fence();
    }

    /// When the BUSY sample is clear, fence and return the stopped receipt.
    /// Sampling BUSY and sequencing the stop request belong to the HAL.
    #[doc(hidden)]
    pub fn confirm_scheduler_stopped(
        &mut self,
        busy: crate::bluetooth::registers::BluetoothSchedulerBusyObservation,
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
        run: crate::bluetooth::registers::BluetoothSchedulerHardwareRunCommandPublished,
    ) -> BluetoothSchedulerStoppedHeadRetirement {
        use super::BluetoothSchedulerHardwareListHeadRetirementDisposition as Disposition;
        let (disposition, observed) = retire_head(self, run.index(), run.head());
        match disposition {
            Disposition::Empty => BluetoothSchedulerStoppedHeadRetirement::Retired(BluetoothSchedulerStoppedItem {
                head: crate::bluetooth::registers::BluetoothSchedulerHardwareListHeadEmptyObserved { index: run.index(), completed_head: run.head() },
            }),
            Disposition::ExpectedHeadStillPublished => BluetoothSchedulerStoppedHeadRetirement::Rejected(
                crate::bluetooth::registers::BluetoothSchedulerHardwareListHeadRetirementObservation::ExpectedHeadStillPublished { run, observed }),
            Disposition::UnexpectedHeadChanged => BluetoothSchedulerStoppedHeadRetirement::Rejected(
                crate::bluetooth::registers::BluetoothSchedulerHardwareListHeadRetirementObservation::UnexpectedHeadChanged { run, observed }),
        }
    }
}

impl<P: BluetoothPlacements> BluetoothInterruptRegisters<P> {
    /// Mask the two dynamic scheduler banks before task-side RUN disable.
    #[doc(hidden)]
    pub fn mask_scheduler_run_interrupts(&mut self) {
        let bank = &mut self.peripherals.bluetooth_interrupt_bank;
        bank.modify_irq_enable_0(|w| {
            w.set_source_21(false);
            w.set_sources_27_28(oer_mmio::Bits::constant::<0x0>());
        });
        bank.modify_irq_enable_1(|w| {
            w.set_source_3(false);
        });
        device_fence();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    struct Heads {
        reads: VecDeque<crate::bluetooth::registers::BluetoothSchedulerHardwareListHead>,
        clears: usize,
    }
    impl HeadControl for Heads {
        fn observe(
            &mut self,
            _: crate::bluetooth::registers::BluetoothSchedulerHardwareListIndex,
        ) -> crate::bluetooth::registers::BluetoothSchedulerHardwareListHead {
            self.reads.pop_front().unwrap()
        }
        fn clear(&mut self, _: crate::bluetooth::registers::BluetoothSchedulerHardwareListIndex) {
            self.clears += 1;
        }
    }
    #[test]
    fn stop_detaches_only_its_exact_head_and_verifies_retirement() {
        use super::super::BluetoothSchedulerHardwareListHeadRetirementDisposition as D;
        let expected =
            crate::bluetooth::registers::BluetoothSchedulerHardwareListHead::from_address(
                crate::dma_address::model_reference(0x2f00_0400),
            )
            .unwrap();
        let foreign =
            crate::bluetooth::registers::BluetoothSchedulerHardwareListHead::from_address(
                crate::dma_address::model_reference(0x2f00_0800),
            )
            .unwrap();
        let empty = crate::bluetooth::registers::BluetoothSchedulerHardwareListHead::empty();
        let index = crate::bluetooth::registers::BluetoothSchedulerHardwareListIndex::ZERO;
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
