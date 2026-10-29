//! B0's retained Bluetooth gate diagnostic. Owners stay on the interrupted
//! stacks and in the gate until the fatal path resets the chip.
#[cfg(test)]
use core::fmt;
#[cfg(any(test, all(target_arch = "riscv32", oer_b0_configured)))]
use oer_diagnostics::fault::{self as fault_format, Class};
use oer_diagnostics::fault::{Domain, Record};
#[cfg(any(test, all(target_arch = "riscv32", oer_b0_configured)))]
use oer_esp32s31_bluetooth_driver::interrupt::gate::{GateFault, GateFaultReason, GateTimerState};
#[cfg(any(test, all(target_arch = "riscv32", oer_b0_configured)))]
use oer_esp32s31_hal::bluetooth::BluetoothModemLpTimerStoppedInterruptFault;
#[cfg(target_arch = "riscv32")]
use oer_espressif_staged_runtime::retained_fault;

/// Reason-tagged observations; the common envelope owns version and integrity.
#[derive(Clone, Copy, bytemuck::AnyBitPattern)]
#[repr(C)]
struct Payload {
    reason: u32,
    /// Reason-tagged encoded payload; each codec names its own source words.
    payload: [u32; 8],
}

impl Payload {
    #[cfg(any(test, all(target_arch = "riscv32", oer_b0_configured)))]
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
        Self {
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
        }
    }

    /// Reject semantically invalid payloads after envelope integrity validation.
    fn validate(&self) -> Option<Self> {
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
        let valid = (1..=11).contains(&self.reason) && payload_valid;
        valid.then_some(*self)
    }
}

impl Payload {
    /// Emit the complete early-console line with statically dispatched writes.
    fn report(&self, mut write: impl FnMut(&[u8])) -> Result<(), ()> {
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
            _ => return Err(()),
        };
        if matches!(self.reason, 10 | 11) {
            let [ena0, ena1, status0, status1, nrt0, nrt1, task_enable, _] = self.payload;
            write(b"OER-B0-FAULT: bluetooth-interrupt-policy reason=");
            write(reason.as_bytes());
            for (name, word) in [
                (b" ena0=".as_slice(), ena0),
                (b" ena1=".as_slice(), ena1),
                (b" status0=".as_slice(), status0),
                (b" status1=".as_slice(), status1),
                (b" nrt0=".as_slice(), nrt0),
                (b" nrt1=".as_slice(), nrt1),
                (b" task-enable=".as_slice(), task_enable),
            ] {
                write(name);
                hex(word, 8, &mut write);
            }
        } else if self.reason == 8 {
            let [_, raw_ticks, _, _, _, overflows, scale_shift, _] = self.payload;
            write(b"OER-B0-FAULT: bluetooth-controller-time reason=");
            write(reason.as_bytes());
            write(b" overflows=");
            hex(overflows, 8, &mut write);
            write(b" raw-ticks=");
            hex(raw_ticks, 8, &mut write);
            write(b" scale-shift=");
            decimal(scale_shift, &mut write);
        } else {
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
                _ => return Err(()),
            };
            write(b"OER-B0-FAULT: bluetooth-gate reason=");
            write(reason.as_bytes());
            write(b" timer=");
            write(timer.as_bytes());
            write(b" lp=");
            hex(lp, 8, &mut write);
            if nrt_present == 1 {
                write(b" nrt=");
                hex(nrt0, 8, &mut write);
                write(b",");
                hex(nrt1, 8, &mut write);
            } else {
                write(b" nrt=unavailable");
            }
            if matches!(self.reason, 4 | 5) {
                write(b" stopped-status=");
                hex(stopped_status, 8, &mut write);
                write(b" pending0024=");
                hex(pending_0024, 2, &mut write);
                write(b" pending002c=");
                hex(pending_002c, 2, &mut write);
            }
            if self.reason == 7 {
                write(b" overflows=");
                hex(stopped_status, 8, &mut write);
                write(b" captured-inputs=");
                hex(pending_0024, 8, &mut write);
            }
            if self.reason == 9 {
                write(b" nrt-captured=");
                hex(stopped_status, 8, &mut write);
                write(b",");
                hex(pending_0024, 8, &mut write);
            }
        }
        write(b"\r\n");
        Ok(())
    }
}

fn hex(mut word: u32, minimum_digits: usize, write: &mut impl FnMut(&[u8])) {
    let mut digits = [b'0'; 8];
    for digit in digits.iter_mut().rev() {
        let nibble = (word & 0xf) as u8;
        *digit = if nibble < 10 {
            b'0'.wrapping_add(nibble)
        } else {
            b'a'.wrapping_add(nibble - 10)
        };
        word >>= 4;
    }
    let mut hex = digits.as_slice();
    while let [b'0', rest @ ..] = hex {
        if hex.len() <= minimum_digits {
            break;
        }
        hex = rest;
    }
    write(b"0x");
    write(hex);
}

fn decimal(mut word: u32, write: &mut impl FnMut(&[u8])) {
    let mut digits = [b'0'; u32::MAX.ilog10() as usize + 1];
    for digit in digits.iter_mut().rev() {
        *digit = b'0'.wrapping_add((word % 10) as u8);
        word /= 10;
    }
    let mut decimal = digits.as_slice();
    while let [b'0', rest @ ..] = decimal {
        if rest.is_empty() {
            break;
        }
        decimal = rest;
    }
    write(decimal);
}

// The original formatter is a test-only byte oracle for direct output.
#[cfg(test)]
impl fmt::Display for Payload {
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

impl Payload {
    #[cfg(any(test, all(target_arch = "riscv32", oer_b0_configured)))]
    fn interrupt_policy(fault: oer_esp32s31_hal::bluetooth::BluetoothInterruptPolicyFault) -> Self {
        use oer_esp32s31_hal::bluetooth::BluetoothInterruptPolicyFault;
        let reason = match fault {
            BluetoothInterruptPolicyFault::ForeignEnable(_) => 10,
            BluetoothInterruptPolicyFault::ReleaseNotQuiet(_) => 11,
        };
        let o = fault.observation();
        // Encode all source words in the common-format slot; no gate-entry sample
        // exists for pre-initialization admission. The reason selects this layout.
        Self {
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
        }
    }
    #[cfg(any(test, all(target_arch = "riscv32", oer_b0_configured)))]
    fn controller_time(
        fault: oer_esp32s31_bluetooth_driver::controller_time::ControllerTimeRangeFault,
    ) -> Self {
        // Reuse the same retained slot and word layout; this reason has no gate
        // observation. The raw word and count are the full source observation.
        Self {
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
        }
    }
}

#[cfg(target_arch = "riscv32")]
oer_espressif_staged_runtime::retained_static! {
    static RECORD: Record;
}

/// Never returns, so no live gate, task or DMA owner is dropped or reused.
#[cfg(all(target_arch = "riscv32", oer_b0_configured))]
pub(crate) fn reset(fault: GateFault) -> ! {
    reset_record(|| Payload::new(fault))
}

#[cfg(all(target_arch = "riscv32", oer_b0_configured))]
pub(crate) fn reset_controller_time(
    fault: oer_esp32s31_bluetooth_driver::controller_time::ControllerTimeRangeFault,
) -> ! {
    reset_record(|| Payload::controller_time(fault))
}

#[cfg(all(target_arch = "riscv32", oer_b0_configured))]
pub(crate) fn record_interrupt_policy(
    claim: &retained_fault::Claim<'_>,
    fault: oer_esp32s31_hal::bluetooth::BluetoothInterruptPolicyFault,
) {
    commit(claim, Payload::interrupt_policy(fault));
}

#[cfg(all(target_arch = "riscv32", oer_b0_configured))]
fn reset_record(capture: impl FnOnce() -> Payload) -> ! {
    oer_espressif_staged_runtime::fatal::reset_recording::<oer_esp32s31_hal::platform::Port>(
        |claim, _stop| commit(claim, capture()),
    )
}
#[cfg(all(target_arch = "riscv32", oer_b0_configured))]
fn commit(claim: &retained_fault::Claim<'_>, payload: Payload) {
    if let Ok(envelope) = payload.envelope() {
        let _committed = claim.commit(&RECORD, envelope);
    }
}

/// Print before the early console is retired to the application.
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
        let [a, b, c, d, e, f, g, h] = self.payload;
        let words = [self.reason, a, b, c, d, e, f, g, h];
        Record::encode(
            match self.reason {
                7 | 8 => Class::Exhaustion,
                4..=6 | 9..=11 => Class::UnexpectedHardware,
                _ => Class::SoftwareInvariant,
            },
            Domain::BluetoothGate,
            self.reason,
            oer_diagnostics::fault::UNKNOWN_HART,
            0,
            &words,
        )
    }
}
fn report(record: &mut Record, mut write: impl FnMut(&[u8]) -> bool) -> Result<(), ()> {
    record
        .report(|view| {
            let payload: Payload = view.decode(Domain::BluetoothGate).map_err(|_| ())?;
            let payload = payload.validate().ok_or(())?;
            let mut accepted = true;
            payload.report(|bytes| {
                if accepted {
                    accepted = write(bytes);
                }
            })?;
            accepted.then_some(()).ok_or(())
        })
        .map_err(|_| ())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(any(test, all(target_arch = "riscv32", oer_b0_configured)))]
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
        let storage = std::cell::RefCell::new(Payload::new(fault(GateFaultReason::VacantTimer)));
        // The record outlives both boot-local owners, just as the NOLOAD slot does.
        {
            let previous_boot = storage.borrow();
            assert_eq!(previous_boot.payload[1], 0x40002);
        }
        let record = storage.borrow_mut().validate().expect("next boot's record");
        assert_eq!(
            record.to_string(),
            "OER-B0-FAULT: bluetooth-gate reason=vacant-timer timer=stopped lp=0x00040002 nrt=0x12345678,0xabcdef01"
        );
    }

    #[test]
    fn stopped_payload_bytes_and_unknown_status_survive_retention_and_reporting() {
        let mut slot = Payload::new(fault(GateFaultReason::StoppedTimer(
            BluetoothModemLpTimerStoppedInterruptFault::UnexpectedStatus {
                status: 0xfedcba98,
                pending: Default::default(),
            },
        )));
        // Arbitrary retained bytes, including their high bits, are not booleans.
        slot.payload[6] = 0xa5;
        slot.payload[7] = 0xe7;

        let record = slot.validate().unwrap();
        assert_eq!(record.payload[5], 0xfedcba98);
        assert!(
            record
                .to_string()
                .ends_with("stopped-status=0xfedcba98 pending0024=0xa5 pending002c=0xe7")
        );
    }

    #[test]
    fn software_pending_keeps_the_fault_sample_as_well_as_the_entry_sample() {
        let slot = Payload::new(fault(GateFaultReason::StoppedTimer(
            BluetoothModemLpTimerStoppedInterruptFault::SoftwarePending {
                status: 0x40000,
                pending: Default::default(),
            },
        )));
        let record = slot.validate().unwrap();
        assert_eq!(record.payload[1], 0x40002);
        assert_eq!(record.payload[5], 0x40000);
        assert!(record.to_string().contains("stopped-status=0x00040000"));
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
            let slot = Payload::new(fault(reason));
            assert!(
                slot.validate()
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
        assert_eq!(core::mem::size_of::<Payload>(), 36);
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
        let slot = Payload::new(fault);
        let text = slot.validate().unwrap().to_string();
        assert!(text.contains("reason=controller-time-overflow-exhausted"));
        assert!(text.contains("nrt=0x00000012,0x00000034 overflows=0xffffffff"));

        let slot = Payload::controller_time(ControllerTimeRangeFault {
            raw_ticks: 0xfedcba98,
            overflows: u32::MAX,
            scale:
                oer_esp32s31_hal::bluetooth::BluetoothControllerHalInitConfig::reviewed_standalone()
                    .controller_time_scale(),
        });
        let text = slot.validate().unwrap().to_string();
        assert_eq!(
            text,
            "OER-B0-FAULT: bluetooth-controller-time reason=controller-time-range-exhausted overflows=0xffffffff raw-ticks=0xfedcba98 scale-shift=0"
        );
    }
}

#[cfg(test)]
mod nrt_tests {
    use super::*;
    #[cfg(any(test, all(target_arch = "riscv32", oer_b0_configured)))]
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
        let slot = Payload::new(fault);
        assert_eq!(core::mem::size_of::<Payload>(), 36);
        let record = slot.validate().unwrap();
        assert_eq!(
            record.to_string(),
            "OER-B0-FAULT: bluetooth-gate reason=nrt-primary-fault timer=ready lp=0x00040000 nrt=0x00000000,0x00000000 nrt-captured=0x87654321,0xa5000002"
        );
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
            let slot = Payload::new(GateFault::InterruptPolicy(cause));
            assert_eq!(core::mem::size_of::<Payload>(), 36);
            let record = slot.validate().unwrap();
            let text = record.to_string();
            assert!(text.contains("ena0=0x81234567 ena1=0xfedcba98 status0=0x12345678 status1=0x87654321 nrt0=0xdeadbeef nrt1=0xbadcaffe task-enable=0x80020000"));
        }
    }
}

#[cfg(test)]
mod direct_output_tests {
    use super::*;

    #[test]
    fn every_record_shape_preserves_the_formatted_bytes_and_crlf() {
        for reason in 1..=11 {
            for timer in 0..=3 {
                for nrt_present in 0..=1 {
                    for word in [0, 1, 0xf, 0x10, 0xff, 0x100, 999_999_999, u32::MAX] {
                        let record = Payload {
                            reason,
                            payload: [timer, word, nrt_present, word, word, word, word, word],
                        };
                        let mut output = std::vec::Vec::new();
                        record
                            .report(|bytes| output.extend_from_slice(bytes))
                            .unwrap();
                        // The original test-only Display is the old wire-text contract.
                        assert_eq!(output, std::format!("{record}\r\n").as_bytes());
                    }
                }
            }
        }
    }

    #[test]
    fn invalid_reason_or_timer_emits_nothing() {
        for (reason, timer) in [(0, 0), (12, 0), (u32::MAX, 0), (1, 4), (9, u32::MAX)] {
            let record = Payload {
                reason,
                payload: [timer, 0, 0, 0, 0, 0, 0, 0],
            };
            assert!(
                record
                    .report(|_| panic!("invalid record emits no bytes"))
                    .is_err()
            );
        }
    }
}

#[cfg(test)]
mod envelope_tests {
    use super::*;
    #[test]
    fn each_refused_chunk_keeps_the_complete_payload_until_replay_succeeds() {
        let payload = Payload {
            reason: 10,
            payload: [u32::MAX, 2, 3, 4, 5, 6, 7, 0],
        };
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
