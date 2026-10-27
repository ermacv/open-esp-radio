//! B0's retained Bluetooth gate diagnostic. Owners stay on the interrupted
//! stacks and in the gate until the fatal path resets the chip.
use core::fmt;
use oer_esp32s31_bluetooth_driver::interrupt::gate::{GateFault, GateFaultReason, GateTimerState};
use oer_esp32s31_hal::bluetooth::BluetoothModemLpTimerStoppedInterruptFault;

const MAGIC: u32 = 0x4247_4601;

/// All-bit-pattern retained representation, including a version and checksum.
#[derive(Clone, Copy, bytemuck::AnyBitPattern)]
#[repr(C)]
struct Record {
    magic: u32,
    reason: u32,
    /// Reason-tagged encoded payload; each codec names its own source words.
    payload: [u32; 8],
    checksum: u32,
}

const _: () = assert!(core::mem::size_of::<Record>() == 44);

impl Record {
    fn new(fault: GateFault) -> Self {
        let (gate_reason, observation) = match fault {
            GateFault::Invariant {
                reason,
                observation,
            } => (reason, observation),
            GateFault::InterruptPolicy(cause) => return Self::interrupt_policy(cause),
        };
        let (reason, stopped_status, pending) = match gate_reason {
            GateFaultReason::PlanWithoutPendingTimer => (1, 0, None),
            GateFaultReason::ReplyOccupied => (2, 0, None),
            GateFaultReason::WorkOccupied => (3, 0, None),
            GateFaultReason::StoppedTimer(
                BluetoothModemLpTimerStoppedInterruptFault::SoftwarePending { status, pending },
            ) => (4, status, Some(pending)),
            GateFaultReason::StoppedTimer(
                BluetoothModemLpTimerStoppedInterruptFault::UnexpectedStatus { status, pending },
            ) => (5, status, Some(pending)),
            GateFaultReason::VacantTimer => (6, 0, None),
            GateFaultReason::ControllerTimeOverflow(cause) => (7, cause.overflows, None),
            GateFaultReason::NrtPrimaryFault(cause) => (9, cause.snapshots[0], None),
        };
        let captured_inputs = match gate_reason {
            GateFaultReason::ControllerTimeOverflow(cause) => cause.captured_inputs,
            GateFaultReason::NrtPrimaryFault(cause) => cause.snapshots[1],
            _ => 0,
        };
        let [nrt0, nrt1] = observation.nrt_status.unwrap_or_default();
        let mut record = Self {
            magic: MAGIC,
            reason,
            payload: [
                match observation.timer {
                    GateTimerState::Vacant => 0,
                    GateTimerState::Ready => 1,
                    GateTimerState::AwaitingPlan => 2,
                    GateTimerState::Stopped => 3,
                },
                observation.lp_status,
                u32::from(observation.nrt_status.is_some()),
                nrt0,
                nrt1,
                stopped_status,
                pending.map_or(captured_inputs, |pending| u32::from(pending.state_0024())),
                pending.map_or(0, |pending| u32::from(pending.state_002c())),
            ],
            checksum: 0,
        };
        record.checksum = record.sum();
        record
    }

    fn sum(&self) -> u32 {
        let mut sum = 0x811c_9dc5_u32;
        for word in [self.magic, self.reason].into_iter().chain(self.payload) {
            sum = (sum ^ word).wrapping_mul(0x0100_0193);
        }
        sum
    }

    /// Consumes the record once, rejecting cold, partial or corrupt writes.
    fn take(&mut self) -> Option<Self> {
        let payload_valid = if matches!(self.reason, 10 | 11) {
            let [_, _, _, _, _, _, _, reserved] = self.payload;
            reserved == 0 // the seven policy words are full-width images
        } else {
            let [timer, _, nrt_present, _, _, _, pending_0024, pending_002c] = self.payload;
            timer <= 3
                && nrt_present <= 1
                && (matches!(self.reason, 7 | 9) || pending_0024 <= u32::from(u8::MAX))
                && pending_002c <= u32::from(u8::MAX)
        };
        let valid = self.magic == MAGIC
            && self.checksum == self.sum()
            && (1..=11).contains(&self.reason)
            && payload_valid;
        let record = valid.then_some(*self);
        self.magic = 0;
        record
    }
}

impl fmt::Display for Record {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = match self.reason {
            1 => "plan-without-pending-timer",
            2 => "reply-occupied",
            3 => "work-occupied",
            4 => "stopped-software-pending",
            5 => "stopped-unexpected-status",
            6 => "vacant-timer",
            7 => "controller-time-overflow-exhausted",
            8 => "controller-time-range-exhausted",
            9 => "nrt-primary-fault",
            10 => "foreign-enable",
            11 => "release-not-quiet",
            _ => return Err(fmt::Error),
        };
        if matches!(self.reason, 10 | 11) {
            let [
                ena0,
                ena1,
                status0,
                status1,
                nrt0,
                nrt1,
                task_enable,
                _reserved,
            ] = self.payload;
            return write!(
                f,
                "OER-B0-FAULT: bluetooth-interrupt-policy reason={reason} ena0={:#010x} ena1={:#010x} status0={:#010x} status1={:#010x} nrt0={:#010x} nrt1={:#010x} task-enable={:#010x}",
                ena0, ena1, status0, status1, nrt0, nrt1, task_enable
            );
        }
        if self.reason == 8 {
            let [_, raw_ticks, _, _, _, overflows, scale_shift, _] = self.payload;
            return write!(
                f,
                "OER-B0-FAULT: bluetooth-controller-time reason={reason} overflows={overflows:#010x} raw-ticks={raw_ticks:#010x} scale-shift={scale_shift}"
            );
        }
        let [
            timer,
            lp,
            nrt_present,
            nrt0,
            nrt1,
            stopped_status,
            pending_0024,
            pending_002c,
        ] = self.payload;
        let timer = match timer {
            0 => "vacant",
            1 => "ready",
            2 => "awaiting-plan",
            3 => "stopped",
            _ => return Err(fmt::Error),
        };
        write!(
            f,
            "OER-B0-FAULT: bluetooth-gate reason={reason} timer={timer} lp={:#010x}",
            lp
        )?;
        if nrt_present == 1 {
            write!(f, " nrt={:#010x},{:#010x}", nrt0, nrt1)?;
        } else {
            f.write_str(" nrt=unavailable")?;
        }
        if self.reason == 4 || self.reason == 5 {
            write!(
                f,
                " stopped-status={:#010x} pending0024={:#04x} pending002c={:#04x}",
                stopped_status, pending_0024, pending_002c
            )?;
        }
        if self.reason == 7 {
            write!(
                f,
                " overflows={:#010x} captured-inputs={:#010x}",
                stopped_status, pending_0024
            )?;
        }
        if self.reason == 9 {
            write!(
                f,
                " nrt-captured={:#010x},{:#010x}",
                stopped_status, pending_0024
            )?;
        }
        Ok(())
    }
}

impl Record {
    fn interrupt_policy(fault: oer_esp32s31_hal::bluetooth::BluetoothInterruptPolicyFault) -> Self {
        use oer_esp32s31_hal::bluetooth::BluetoothInterruptPolicyFault;
        let reason = match fault {
            BluetoothInterruptPolicyFault::ForeignEnable(_) => 10,
            BluetoothInterruptPolicyFault::ReleaseNotQuiet(_) => 11,
        };
        let o = fault.observation();
        // Encode the source words in the 44-byte slot; no gate-entry sample
        // exists for pre-initialization admission. The reason selects this layout.
        let mut record = Self {
            magic: MAGIC,
            reason,
            payload: [
                o.primary.enable_0,
                o.primary.enable_1,
                o.primary.status_0,
                o.primary.status_1,
                o.primary.nrt_0,
                o.primary.nrt_1,
                o.task_enable,
                0, // reserved: no opaque task-status read in production
            ],
            checksum: 0,
        };
        record.checksum = record.sum();
        record
    }
    fn controller_time(
        fault: oer_esp32s31_bluetooth_driver::controller_time::ControllerTimeRangeFault,
    ) -> Self {
        // Reuse the same retained slot and word layout; this reason has no gate
        // observation. The raw word and count are the full source observation.
        let mut record = Self {
            magic: MAGIC,
            reason: 8,
            payload: [
                0,
                fault.raw_ticks,
                0,
                0,
                0,
                fault.overflows,
                u32::from(fault.scale.shift_image()),
                0,
            ],
            checksum: 0,
        };
        record.checksum = record.sum();
        record
    }
}

#[cfg(target_arch = "riscv32")]
oer_espressif_staged_runtime::retained_static! {
    static RECORD: Record;
}

/// Never returns, so no live gate, task or DMA owner is dropped or reused.
#[cfg(target_arch = "riscv32")]
pub(crate) fn reset(fault: GateFault) -> ! {
    reset_record(Record::new(fault))
}

#[cfg(target_arch = "riscv32")]
pub(crate) fn reset_controller_time(
    fault: oer_esp32s31_bluetooth_driver::controller_time::ControllerTimeRangeFault,
) -> ! {
    reset_record(Record::controller_time(fault))
}

#[cfg(target_arch = "riscv32")]
fn reset_record(record: Record) -> ! {
    oer_espressif_staged_runtime::fatal::reset_recording(|_stop| {
        if let Some(mut slot) = RECORD.try_lock() {
            *slot = record;
            // Keep the first record claimed until reset, including a nested fault.
            core::mem::forget(slot);
        }
    })
}

/// Print before the early console is retired to the application.
#[cfg(target_arch = "riscv32")]
pub(crate) fn report_previous() {
    use core::fmt::Write;
    struct Early;
    impl Write for Early {
        fn write_str(&mut self, text: &str) -> fmt::Result {
            let _ = oer_esp32s31_hal::usb_serial_jtag::early_text(text.as_bytes());
            Ok(())
        }
    }
    let previous = RECORD.try_lock().and_then(|mut record| record.take());
    if let Some(previous) = previous {
        let _ = writeln!(Early, "{previous}\r");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_esp32s31_bluetooth_driver::interrupt::gate::GateObservation;

    fn fault(reason: GateFaultReason) -> GateFault {
        GateFault::Invariant {
            reason,
            observation: GateObservation {
                lp_status: 0x40002,
                nrt_status: Some([0x12345678, 0xabcdef01]),
                timer: GateTimerState::Stopped,
            },
        }
    }

    #[test]
    fn retained_record_survives_boot_state_recreation_and_is_reported_once() {
        let storage = std::cell::RefCell::new(Record::new(fault(GateFaultReason::VacantTimer)));
        // The record outlives both boot-local owners, just as the NOLOAD slot does.
        {
            let previous_boot = storage.borrow();
            assert_eq!(previous_boot.payload[1], 0x40002);
        }
        let record = storage.borrow_mut().take().expect("next boot's record");
        assert_eq!(
            record.to_string(),
            "OER-B0-FAULT: bluetooth-gate reason=vacant-timer timer=stopped lp=0x00040002 nrt=0x12345678,0xabcdef01"
        );
        assert!(storage.borrow_mut().take().is_none());
    }

    #[test]
    fn stopped_payload_bytes_and_unknown_status_survive_retention_and_reporting() {
        let mut slot = Record::new(fault(GateFaultReason::StoppedTimer(
            BluetoothModemLpTimerStoppedInterruptFault::UnexpectedStatus {
                status: 0xfedcba98,
                pending: Default::default(),
            },
        )));
        // Arbitrary retained bytes, including their high bits, are not booleans.
        slot.payload[6] = 0xa5;
        slot.payload[7] = 0xe7;
        slot.checksum = slot.sum();
        let record = slot.take().unwrap();
        assert_eq!(record.payload[5], 0xfedcba98);
        assert!(
            record
                .to_string()
                .ends_with("stopped-status=0xfedcba98 pending0024=0xa5 pending002c=0xe7")
        );
    }

    #[test]
    fn software_pending_keeps_the_fault_sample_as_well_as_the_entry_sample() {
        let mut slot = Record::new(fault(GateFaultReason::StoppedTimer(
            BluetoothModemLpTimerStoppedInterruptFault::SoftwarePending {
                status: 0x40000,
                pending: Default::default(),
            },
        )));
        let record = slot.take().unwrap();
        assert_eq!(record.payload[1], 0x40002);
        assert_eq!(record.payload[5], 0x40000);
        assert!(record.to_string().contains("stopped-status=0x00040000"));
    }

    #[test]
    fn cold_partial_or_corrupt_records_are_rejected() {
        let mut cold: Record = bytemuck::zeroed();
        assert!(cold.take().is_none());
        let mut partial = Record::new(fault(GateFaultReason::ReplyOccupied));
        partial.checksum = 0;
        assert!(partial.take().is_none());
        let mut corrupt = Record::new(fault(GateFaultReason::WorkOccupied));
        corrupt.payload[4] ^= 0x80000000;
        assert!(corrupt.take().is_none());
    }

    #[test]
    fn every_gate_reason_has_a_stable_retained_diagnostic() {
        for reason in [
            GateFaultReason::PlanWithoutPendingTimer,
            GateFaultReason::ReplyOccupied,
            GateFaultReason::WorkOccupied,
            GateFaultReason::VacantTimer,
            GateFaultReason::StoppedTimer(
                BluetoothModemLpTimerStoppedInterruptFault::SoftwarePending {
                    status: 0x40000,
                    pending: Default::default(),
                },
            ),
        ] {
            let mut slot = Record::new(fault(reason));
            assert!(
                slot.take()
                    .unwrap()
                    .to_string()
                    .starts_with("OER-B0-FAULT: bluetooth-gate reason=")
            );
        }
    }
}

#[cfg(test)]
mod controller_time_tests {
    use super::*;
    use oer_esp32s31_bluetooth_driver::{
        controller_time::ControllerTimeRangeFault,
        interrupt::{ControllerTimeOverflowExhausted, gate::GateObservation},
    };
    #[test]
    fn time_faults_keep_their_source_words_in_the_existing_retained_slot() {
        assert_eq!(core::mem::size_of::<Record>(), 44);
        let fault = GateFault::Invariant {
            reason: GateFaultReason::ControllerTimeOverflow(ControllerTimeOverflowExhausted {
                overflows: u32::MAX,
                captured_inputs: 0x00420000,
            }),
            observation: GateObservation {
                lp_status: 0x40000,
                nrt_status: Some([0x12, 0x34]),
                timer: GateTimerState::Ready,
            },
        };
        let mut slot = Record::new(fault);
        let text = slot.take().unwrap().to_string();
        assert!(text.contains("reason=controller-time-overflow-exhausted"));
        assert!(text.contains("nrt=0x00000012,0x00000034 overflows=0xffffffff"));
        assert!(slot.take().is_none());
        let mut slot = Record::controller_time(ControllerTimeRangeFault {
            raw_ticks: 0xfedcba98,
            overflows: u32::MAX,
            scale:
                oer_esp32s31_hal::bluetooth::BluetoothControllerHalInitConfig::reviewed_standalone()
                    .controller_time_scale(),
        });
        let text = slot.take().unwrap().to_string();
        assert_eq!(
            text,
            "OER-B0-FAULT: bluetooth-controller-time reason=controller-time-range-exhausted overflows=0xffffffff raw-ticks=0xfedcba98 scale-shift=0"
        );
        assert!(slot.take().is_none());
    }
}

#[cfg(test)]
mod nrt_tests {
    use super::*;
    use oer_esp32s31_bluetooth_driver::interrupt::gate::GateObservation;
    use oer_esp32s31_hal::bluetooth::BluetoothNrtInterruptFault;

    #[test]
    fn the_existing_record_keeps_the_full_nrt_fault_pair_and_entry_causes() {
        let fault = GateFault::Invariant {
            reason: GateFaultReason::NrtPrimaryFault(BluetoothNrtInterruptFault {
                snapshots: [0x87654321, 0xa5000002],
            }),
            observation: GateObservation {
                lp_status: 0x40000,
                nrt_status: Some([0, 0]),
                timer: GateTimerState::Ready,
            },
        };
        let mut slot = Record::new(fault);
        assert_eq!(core::mem::size_of::<Record>(), 44);
        let record = slot.take().unwrap();
        assert_eq!(
            record.to_string(),
            "OER-B0-FAULT: bluetooth-gate reason=nrt-primary-fault timer=ready lp=0x00040000 nrt=0x00000000,0x00000000 nrt-captured=0x87654321,0xa5000002"
        );
        assert!(slot.take().is_none());
    }
}

#[cfg(test)]
mod interrupt_policy_tests {
    use super::*;
    use oer_esp32s31_hal::bluetooth::{
        BluetoothInterruptObservation, BluetoothInterruptPolicyFault,
        BluetoothInterruptPolicyObservation,
    };
    #[test]
    fn every_bank_word_survives_the_existing_slot_and_is_reported_once() {
        let observation = BluetoothInterruptPolicyObservation {
            primary: BluetoothInterruptObservation {
                enable_0: 0x81234567,
                enable_1: 0xfedcba98,
                status_0: 0x12345678,
                status_1: 0x87654321,
                nrt_0: 0xdeadbeef,
                nrt_1: 0xbadcaffe,
            },
            task_enable: 0x80020000,
        };
        for cause in [
            BluetoothInterruptPolicyFault::ForeignEnable(observation),
            BluetoothInterruptPolicyFault::ReleaseNotQuiet(observation),
        ] {
            let mut slot = Record::new(GateFault::InterruptPolicy(cause));
            assert_eq!(core::mem::size_of::<Record>(), 44);
            let record = slot.take().unwrap();
            let text = record.to_string();
            assert!(text.contains("ena0=0x81234567 ena1=0xfedcba98 status0=0x12345678 status1=0x87654321 nrt0=0xdeadbeef nrt1=0xbadcaffe task-enable=0x80020000"));
            assert!(slot.take().is_none());
        }
    }
}
