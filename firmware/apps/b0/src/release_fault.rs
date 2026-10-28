//! Retained controller-output release failures; no owner is released before reset.
use oer_esp32s31_hal::bluetooth::{
    BluetoothControllerOutputReleaseError as Output, BluetoothInterruptPolicyFault,
    engine::{EngineOutputReleaseError, scheduler::interrupt::SchedulerInterruptError as Channel},
};

const MAGIC: u32 = 0x4252_4601;

#[derive(Clone, Copy, Debug, Eq, PartialEq, bytemuck::AnyBitPattern)]
#[repr(C)]
struct Record {
    magic: u32,
    reason: u32,
    detail: u32,
    checksum: u32,
}
const _: () = assert!(core::mem::size_of::<Record>() == 16);

impl Record {
    /// Policy faults retain all seven source words in the existing gate slot.
    fn new(fault: EngineOutputReleaseError) -> Result<Self, BluetoothInterruptPolicyFault> {
        let (reason, detail) = match fault {
            EngineOutputReleaseError::Output(output) => match output {
                Output::InterruptPolicy(policy) => return Err(policy),
                Output::ControllerTimePending => (1, 0),
                Output::SchedulerBusy => (2, 0),
                Output::SchedulerBusyUnsettled => (3, 0),
                Output::PublishedHead(index) => (4, u32::from(index.get())),
                Output::InterruptFault => (5, 0),
            },
            EngineOutputReleaseError::Channel(channel) => (
                6,
                match channel {
                    Channel::NotInstalled => 1,
                    Channel::Occupied => 2,
                    Channel::Abandoned => 3,
                    Channel::SequenceExhausted => 4,
                    Channel::ForeignReply => 5,
                    Channel::ForeignContext => 6,
                    Channel::DiagnosticUnsettled => 7,
                    Channel::Quarantined => 8,
                    Channel::NotStopped => 9,
                    Channel::OutputReleased => 10,
                },
            ),
        };
        let mut record = Self {
            magic: MAGIC,
            reason,
            detail,
            checksum: 0,
        };
        record.checksum = record.sum();
        Ok(record)
    }

    fn sum(&self) -> u32 {
        [self.magic, self.reason, self.detail]
            .into_iter()
            .fold(0x811c_9dc5_u32, |sum, word| {
                (sum ^ word).wrapping_mul(0x0100_0193)
            })
    }

    fn reason_name(&self) -> Option<&'static [u8]> {
        Some(match (self.reason, self.detail) {
            (1, 0) => b"controller-time-pending",
            (2, 0) => b"scheduler-busy",
            (3, 0) => b"scheduler-busy-unsettled",
            (4, 0..=15) => b"published-head",
            (5, 0) => b"interrupt-fault",
            (6, 1) => b"channel-not-installed",
            (6, 2) => b"channel-occupied",
            (6, 3) => b"channel-abandoned",
            (6, 4) => b"channel-sequence-exhausted",
            (6, 5) => b"channel-foreign-reply",
            (6, 6) => b"channel-foreign-context",
            (6, 7) => b"channel-diagnostic-unsettled",
            (6, 8) => b"channel-quarantined",
            (6, 9) => b"channel-not-stopped",
            (6, 10) => b"channel-output-released",
            _ => return None,
        })
    }

    fn take(&mut self) -> Option<Self> {
        let valid =
            self.magic == MAGIC && self.checksum == self.sum() && self.reason_name().is_some();
        let record = valid.then_some(*self);
        self.magic = 0;
        record
    }

    fn report(&self, mut write: impl FnMut(&[u8])) {
        if let Some(reason) = self.reason_name() {
            write(b"OER-B0-FAULT: bluetooth-output-release reason=");
            write(reason);
            if self.reason == 4 {
                write(b" head=");
                write(&[
                    b'0'.wrapping_add((self.detail / 10) as u8),
                    b'0'.wrapping_add((self.detail % 10) as u8),
                ]);
            }
            write(b"\r\n");
        }
    }
}

#[cfg(target_arch = "riscv32")]
oer_espressif_staged_runtime::retained_static! { static RECORD: Record; }

/// Borrowing the failure keeps its task, arena and register owners quarantined.
#[cfg(all(target_arch = "riscv32", oer_b0_configured))]
pub(crate) fn reset<T>(owner: &mut T, fault: EngineOutputReleaseError) -> ! {
    core::hint::black_box(owner);
    let record = match Record::new(fault) {
        Ok(record) => record,
        Err(policy) => crate::gate_fault::reset_interrupt_policy(policy),
    };
    oer_espressif_staged_runtime::fatal::reset_recording::<oer_esp32s31_hal::platform::Port>(
        |_stop| {
            if let Some(mut slot) = RECORD.try_lock() {
                *slot = record;
                core::mem::forget(slot);
            }
        },
    )
}

#[cfg(target_arch = "riscv32")]
pub(crate) fn report_previous() {
    let previous = RECORD.try_lock().and_then(|mut record| record.take());
    if let Some(previous) = previous {
        previous.report(|bytes| {
            let _ = oer_esp32s31_hal::peripherals::usb_serial_jtag::early_text(bytes);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_esp32s31_hal::bluetooth::{
        BluetoothInterruptObservation, BluetoothInterruptPolicyObservation,
        BluetoothSchedulerHardwareListIndex,
    };

    #[test]
    fn every_release_refusal_survives_reset_and_reports_once() {
        let mut cases = std::vec![
            (
                Output::ControllerTimePending.into(),
                "controller-time-pending".to_owned()
            ),
            (Output::SchedulerBusy.into(), "scheduler-busy".to_owned()),
            (
                Output::SchedulerBusyUnsettled.into(),
                "scheduler-busy-unsettled".to_owned()
            ),
            (Output::InterruptFault.into(), "interrupt-fault".to_owned()),
        ];
        for index in 0..16 {
            cases.push((
                Output::PublishedHead(BluetoothSchedulerHardwareListIndex::new(index).unwrap())
                    .into(),
                std::format!("published-head head={index:02}"),
            ));
        }
        for (channel, name) in [
            (Channel::NotInstalled, "not-installed"),
            (Channel::Occupied, "occupied"),
            (Channel::Abandoned, "abandoned"),
            (Channel::SequenceExhausted, "sequence-exhausted"),
            (Channel::ForeignReply, "foreign-reply"),
            (Channel::ForeignContext, "foreign-context"),
            (Channel::DiagnosticUnsettled, "diagnostic-unsettled"),
            (Channel::Quarantined, "quarantined"),
            (Channel::NotStopped, "not-stopped"),
            (Channel::OutputReleased, "output-released"),
        ] {
            cases.push((channel.into(), std::format!("channel-{name}")));
        }
        for (fault, expected) in cases {
            let mut slot = Record::new(fault).unwrap();
            let next_boot = slot.take().expect("retained cause");
            let mut bytes = std::vec::Vec::new();
            next_boot.report(|part| bytes.extend_from_slice(part));
            assert_eq!(
                bytes,
                std::format!("OER-B0-FAULT: bluetooth-output-release reason={expected}\r\n")
                    .as_bytes()
            );
            assert_eq!(slot.take(), None);
        }
    }

    #[test]
    fn policy_release_faults_keep_every_source_word_for_the_gate_slot() {
        let observation = BluetoothInterruptPolicyObservation {
            primary: BluetoothInterruptObservation {
                enable_0: 1,
                enable_1: 2,
                status_0: 3,
                status_1: 4,
                nrt_0: 5,
                nrt_1: 6,
            },
            task_enable: 7,
        };
        for policy in [
            BluetoothInterruptPolicyFault::ForeignEnable(observation),
            BluetoothInterruptPolicyFault::ReleaseNotQuiet(observation),
        ] {
            assert_eq!(
                Record::new(Output::InterruptPolicy(policy).into()),
                Err(policy)
            );
        }
    }

    #[test]
    fn cold_corrupt_and_invalid_payloads_are_never_reported() {
        let valid = Record::new(Output::SchedulerBusy.into()).unwrap();
        for mut invalid in [
            Record { magic: 0, ..valid },
            Record {
                checksum: valid.checksum ^ 1,
                ..valid
            },
            Record { reason: 7, ..valid },
            Record {
                reason: 4,
                detail: 16,
                ..valid
            },
            Record {
                reason: 6,
                detail: 11,
                ..valid
            },
            Record { detail: 1, ..valid },
        ] {
            if invalid.checksum == valid.checksum && invalid.magic == MAGIC {
                invalid.checksum = invalid.sum();
            }
            assert_eq!(invalid.take(), None);
            assert_eq!(invalid.magic, 0);
        }
    }
}
