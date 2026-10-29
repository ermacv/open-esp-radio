//! Retained controller-output release failures; no owner is released before reset.
#[cfg(any(test, all(target_arch = "riscv32", oer_b0_configured)))]
use oer_diagnostics::fault::{self as fault_format, Class};
use oer_diagnostics::fault::{Domain, Record};
#[cfg(any(test, all(target_arch = "riscv32", oer_b0_configured)))]
use oer_esp32s31_hal::bluetooth::{
    BluetoothControllerOutputReleaseError as Output, BluetoothInterruptPolicyFault,
    engine::{EngineOutputReleaseError, scheduler::interrupt::SchedulerInterruptError as Channel},
};
#[cfg(target_arch = "riscv32")]
use oer_espressif_staged_runtime::retained_fault;

#[derive(Clone, Copy, Debug, Eq, PartialEq, bytemuck::AnyBitPattern)]
#[repr(C)]
struct Payload {
    reason: u32,
    detail: u32,
}

impl Payload {
    /// Policy faults retain all seven source words in the existing gate slot.
    #[cfg(any(test, all(target_arch = "riscv32", oer_b0_configured)))]
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
        let record = Self { reason, detail };
        Ok(record)
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
    oer_espressif_staged_runtime::fatal::reset_recording::<oer_esp32s31_hal::platform::Port>(
        |claim, _stop| {
            core::hint::black_box(owner);
            match Payload::new(fault) {
                Ok(record) => {
                    if let Ok(envelope) = record.envelope() {
                        let _committed = claim.commit(&RECORD, envelope);
                    }
                }
                Err(policy) => crate::gate_fault::record_interrupt_policy(claim, policy),
            }
        },
    )
}

#[cfg(target_arch = "riscv32")]
pub(crate) fn report_previous() {
    if let Some(mut record) = retained_fault::peek(&RECORD) {
        let _reported = report(
            &mut record,
            oer_esp32s31_hal::peripherals::usb_serial_jtag::early_text,
        );
    }
}

impl Payload {
    #[cfg(any(test, all(target_arch = "riscv32", oer_b0_configured)))]
    fn envelope(&self) -> Result<Record, fault_format::Error> {
        Record::encode(
            if self.reason == 6 && self.detail == 4 {
                Class::Exhaustion
            } else if self.reason == 6 && self.detail == 3 {
                Class::Cancellation
            } else if self.reason == 5 {
                Class::UnexpectedHardware
            } else {
                Class::SoftwareInvariant
            },
            Domain::BluetoothRelease,
            self.reason,
            oer_diagnostics::fault::UNKNOWN_HART,
            0,
            &[self.reason, self.detail],
        )
    }
}
fn report(record: &mut Record, mut write: impl FnMut(&[u8]) -> bool) -> Result<(), ()> {
    record
        .report(|view| {
            let payload: Payload = view.decode(Domain::BluetoothRelease).map_err(|_| ())?;
            payload.reason_name().ok_or(())?;
            let mut accepted = true;
            payload.report(|bytes| {
                if accepted {
                    accepted = write(bytes);
                }
            });
            accepted.then_some(()).ok_or(())
        })
        .map_err(|_| ())
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
            let slot = Payload::new(fault).unwrap();
            let next_boot = slot;
            let mut bytes = std::vec::Vec::new();
            next_boot.report(|part| bytes.extend_from_slice(part));
            assert_eq!(
                bytes,
                std::format!("OER-B0-FAULT: bluetooth-output-release reason={expected}\r\n")
                    .as_bytes()
            );
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
                Payload::new(Output::InterruptPolicy(policy).into()),
                Err(policy)
            );
        }
    }
}

#[cfg(test)]
mod envelope_tests {
    use super::*;
    #[test]
    fn each_refused_chunk_keeps_the_complete_payload_until_replay_succeeds() {
        let payload = Payload::new(Output::SchedulerBusy.into()).unwrap();
        let original = payload.envelope().unwrap();
        let mut record = original;
        let mut expected = std::vec::Vec::new();
        let mut chunks = 0;
        report(&mut record, |bytes| {
            chunks += 1;
            expected.extend_from_slice(bytes);
            true
        })
        .unwrap();
        assert!(record.peek().is_err());
        for refused in 0..chunks {
            let mut reboot = original;
            let mut attempted = 0;
            assert!(
                report(&mut reboot, |_| {
                    let ok = attempted != refused;
                    attempted += 1;
                    ok
                })
                .is_err()
            );
            assert_eq!(reboot, original);
            let mut replay = std::vec::Vec::new();
            report(&mut reboot, |bytes| {
                replay.extend_from_slice(bytes);
                true
            })
            .unwrap();
            assert_eq!(replay, expected);
        }
    }
}
