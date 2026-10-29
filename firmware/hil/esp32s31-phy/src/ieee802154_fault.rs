//! Image-local IEEE 802.15.4 retained diagnostic codec and terminal policy.

use core::fmt::{self, Write};
use oer_diagnostics::fault::{self as fault_format, Class, Domain, Record};
use oer_espressif_ieee802154_engine::engine::fault::{ServiceError, ServiceProgress, ServiceStage};
use oer_espressif_ieee802154_engine::engine::{
    Fault, Ieee802154State, StatisticsCounter, StatisticsOverflow,
};
use oer_espressif_ieee802154_mac_values::{
    Ieee802154DebugCounter, Ieee802154Event, Ieee802154EventMask, Ieee802154EventObservation,
    Ieee802154RxAbortReason, Ieee802154RxAbortReasonObservation, Ieee802154RxStateCode,
    Ieee802154RxStatus, Ieee802154TxAbortReason, Ieee802154TxAbortReasonObservation,
};
#[cfg(target_arch = "riscv32")]
use oer_espressif_staged_runtime::retained_fault;

// Only this image uses the hardware engine; retain its source observations
// without adding a record to unrelated staged images.
#[cfg(target_arch = "riscv32")]
oer_espressif_staged_runtime::retained_static! {
    static EVENT_FAULT: Record;
}

#[cfg(target_arch = "riscv32")]
pub(crate) fn reset(fault: Fault) -> ! {
    oer_espressif_staged_runtime::fatal::reset_recording::<oer_esp32s31_hal::platform::Port>(
        |claim, _stop| {
            if let Ok(record) = encode(fault) {
                let _committed = claim.commit(&EVENT_FAULT, record);
            }
        },
    )
}

#[cfg(target_arch = "riscv32")]
pub(crate) fn report_previous() {
    if let Some(mut record) = retained_fault::peek(&EVENT_FAULT) {
        let _reported = report(
            &mut record,
            oer_esp32s31_hal::peripherals::usb_serial_jtag::early_text,
        );
    }
}

// Diagnostic encoding order, not hardware bit positions or write authority.
const NAMED: [Ieee802154Event; 12] = [
    Ieee802154Event::TxDone,
    Ieee802154Event::RxDone,
    Ieee802154Event::AckTxDone,
    Ieee802154Event::AckRxDone,
    Ieee802154Event::RxAbort,
    Ieee802154Event::TxAbort,
    Ieee802154Event::EdDone,
    Ieee802154Event::Timer0Overflow,
    Ieee802154Event::Timer1Overflow,
    Ieee802154Event::ClockCountMatch,
    Ieee802154Event::TxSfdDone,
    Ieee802154Event::RxSfdDone,
];
const STATES: [Ieee802154State; 11] = [
    Ieee802154State::Disable,
    Ieee802154State::Idle,
    Ieee802154State::Sleep,
    Ieee802154State::Rx,
    Ieee802154State::TxAck,
    Ieee802154State::TxEnhAck,
    Ieee802154State::TxCca,
    Ieee802154State::Tx,
    Ieee802154State::RxAck,
    Ieee802154State::Ed,
    Ieee802154State::Cca,
];
const RX_REASONS: [Ieee802154RxAbortReason; 16] = [
    Ieee802154RxAbortReason::RxStop,
    Ieee802154RxAbortReason::SfdTimeout,
    Ieee802154RxAbortReason::CrcError,
    Ieee802154RxAbortReason::InvalidLength,
    Ieee802154RxAbortReason::FilterFail,
    Ieee802154RxAbortReason::NoRss,
    Ieee802154RxAbortReason::CoexistenceBreak,
    Ieee802154RxAbortReason::UnexpectedAck,
    Ieee802154RxAbortReason::RxRestart,
    Ieee802154RxAbortReason::TxAckTimeout,
    Ieee802154RxAbortReason::TxAckStop,
    Ieee802154RxAbortReason::TxAckCoexistenceBreak,
    Ieee802154RxAbortReason::EnhancedAckSecurityError,
    Ieee802154RxAbortReason::EdAbort,
    Ieee802154RxAbortReason::EdStop,
    Ieee802154RxAbortReason::EdCoexistenceReject,
];
const TX_REASONS: [Ieee802154TxAbortReason; 15] = [
    Ieee802154TxAbortReason::RxAckStop,
    Ieee802154TxAbortReason::RxAckSfdTimeout,
    Ieee802154TxAbortReason::RxAckCrcError,
    Ieee802154TxAbortReason::RxAckInvalidLength,
    Ieee802154TxAbortReason::RxAckFilterFail,
    Ieee802154TxAbortReason::RxAckNoRss,
    Ieee802154TxAbortReason::RxAckCoexistenceBreak,
    Ieee802154TxAbortReason::RxAckTypeNotAck,
    Ieee802154TxAbortReason::RxAckRestart,
    Ieee802154TxAbortReason::RxAckTimeout,
    Ieee802154TxAbortReason::TxStop,
    Ieee802154TxAbortReason::TxCoexistenceBreak,
    Ieee802154TxAbortReason::TxSecurityError,
    Ieee802154TxAbortReason::CcaFailed,
    Ieee802154TxAbortReason::CcaBusy,
];
const COUNTERS: [Ieee802154DebugCounter; 15] = [
    Ieee802154DebugCounter::SfdTimeout,
    Ieee802154DebugCounter::CrcError,
    Ieee802154DebugCounter::EdAbort,
    Ieee802154DebugCounter::CcaFail,
    Ieee802154DebugCounter::RxFilterFail,
    Ieee802154DebugCounter::NoRssDetect,
    Ieee802154DebugCounter::RxAbortCoex,
    Ieee802154DebugCounter::RxRestart,
    Ieee802154DebugCounter::TxAckAbortCoex,
    Ieee802154DebugCounter::EdScanBreakCoex,
    Ieee802154DebugCounter::RxAckAbortCoex,
    Ieee802154DebugCounter::RxAckTimeout,
    Ieee802154DebugCounter::TxBreakCoex,
    Ieee802154DebugCounter::TxSecurityError,
    Ieee802154DebugCounter::CcaBusy,
];
const STAGES: [ServiceStage; 6] = [
    ServiceStage::Statistics,
    ServiceStage::Dispatch,
    ServiceStage::RxAbort,
    ServiceStage::RxDone,
    ServiceStage::AckRxDone,
    ServiceStage::TxAbort,
];
// Diagnostic layout: named events in bits 0..=11, unnamed 7/13 at 12/13,
// entry state at 14..=17, service reason at 18, completed statistics at 19;
// upper bits are reserved. Service details preserve exact typed observations.
const STATE_SHIFT: u32 = 14;
const STATE_MASK: u32 = 0xf;
const REASON: u32 = 1 << 18;
const RECORD_BITS: u32 = (1 << 20) - 1;

/// Source sample, ordered progress and typed details inside the common envelope.
#[derive(Clone, Copy, bytemuck::AnyBitPattern)]
#[repr(C)]
pub(crate) struct Payload {
    raw: u32,
    details: [u32; 8],
}

fn event_bits(events: Ieee802154EventMask) -> u32 {
    NAMED.into_iter().enumerate().fold(0, |bits, (bit, event)| {
        bits | u32::from(events.contains(event)).wrapping_shl(bit as u32)
    })
}
fn events_from(bits: u32) -> Ieee802154EventMask {
    NAMED
        .into_iter()
        .enumerate()
        .fold(Ieee802154EventMask::NONE, |events, (bit, event)| {
            if bits & 1u32.wrapping_shl(bit as u32) != 0 {
                events.with(event)
            } else {
                events
            }
        })
}
fn rx_reason(reason: Ieee802154RxAbortReasonObservation) -> u32 {
    match reason {
        Ieee802154RxAbortReasonObservation::Named(reason) => reason as u32,
        Ieee802154RxAbortReasonObservation::Unclassified => 16,
    }
}
fn tx_reason(reason: Ieee802154TxAbortReasonObservation) -> u32 {
    match reason {
        Ieee802154TxAbortReasonObservation::Named(reason) => reason as u32,
        Ieee802154TxAbortReasonObservation::Unclassified => 15,
    }
}
fn rx_from(code: u32) -> Option<Ieee802154RxAbortReasonObservation> {
    if code == 16 {
        Some(Ieee802154RxAbortReasonObservation::Unclassified)
    } else {
        RX_REASONS
            .into_iter()
            .find(|reason| *reason as u32 == code)
            .map(Ieee802154RxAbortReasonObservation::Named)
    }
}
fn tx_from(code: u32) -> Option<Ieee802154TxAbortReasonObservation> {
    if code == 15 {
        Some(Ieee802154TxAbortReasonObservation::Unclassified)
    } else {
        TX_REASONS
            .into_iter()
            .find(|reason| *reason as u32 == code)
            .map(Ieee802154TxAbortReasonObservation::Named)
    }
}
fn status_bits(status: Ieee802154RxStatus) -> u32 {
    let order = if status.state().is_after_receive_sfd() {
        2
    } else if status.state().is_receive_sfd() {
        1
    } else {
        0
    };
    u32::from(status.filter_fail_reason())
        | (u32::from(status.state().value()) << 8)
        | (order << 16)
        | (u32::from(status.preamble_match()) << 18)
        | (u32::from(status.sfd_match()) << 19)
        | (rx_reason(status.abort_reason()) << 20)
}
fn status_from(bits: u32) -> Option<Ieee802154RxStatus> {
    if bits >> 25 != 0 {
        return None;
    }
    let order = match (bits >> 16) & 3 {
        0 => core::cmp::Ordering::Less,
        1 => core::cmp::Ordering::Equal,
        2 => core::cmp::Ordering::Greater,
        _ => return None,
    };
    Some(Ieee802154RxStatus::new(
        bits as u8,
        rx_from((bits >> 20) & 31)?,
        Ieee802154RxStateCode::from_parts((bits >> 8) as u8, order),
        bits & (1 << 18) != 0,
        bits & (1 << 19) != 0,
    ))
}

impl Payload {
    /// Retain a terminal fault before software reset, without releasing owners.
    pub fn record(&mut self, fault: Fault) {
        self.details = [0; 8];
        let (observation, state, service) = match fault {
            Fault::UnhandledEvents { observation, state } => (observation, state, None),
            Fault::Service {
                reason,
                observation,
                entry_state,
                state,
                rx_abort,
                tx_abort,
                progress,
            } => {
                let word = (state as u32)
                    | (rx_reason(rx_abort) << 4)
                    | (tx_reason(tx_abort) << 9)
                    | (event_bits(progress.completed) << 14)
                    | (u32::from(progress.acknowledged) << 26)
                    | (u32::from(progress.rx_abort_phase_done) << 27)
                    | ((progress.stage as u32) << 28)
                    | (u32::from(progress.coexistence_break_notified) << 31);
                let (tag, parameter, status) = match reason {
                    ServiceError::EventState(event) => (0, event as u32, 0),
                    ServiceError::RxAbortUnclassified(status) => (1, 0, status_bits(status)),
                    ServiceError::RxAbortState(status) => (2, 0, status_bits(status)),
                    ServiceError::TxAbortUnclassified => (3, 0, 0),
                    ServiceError::FrequencyCode(code) => (4, u32::from(code), 0),
                    ServiceError::Statistics(error) => {
                        let counter = match error.counter {
                            StatisticsCounter::Transmit => 0,
                            StatisticsCounter::Deferred => 1,
                            StatisticsCounter::TxDone => 2,
                            StatisticsCounter::RxDone => 3,
                            StatisticsCounter::TxCoexistenceBreak => 4,
                            StatisticsCounter::Hardware(counter) => {
                                5u32.wrapping_add(counter as u32)
                            }
                        };
                        self.details = [
                            0,
                            0,
                            0,
                            error.accumulated as u32,
                            (error.accumulated >> 32) as u32,
                            error.increment as u32,
                            (error.increment >> 32) as u32,
                            u32::from(error.drained),
                        ];
                        (6, counter, 0)
                    }
                };
                let [detail, reason_word, status_word, ..] = &mut self.details;
                *detail = word;
                *reason_word = tag | (parameter << 8);
                *status_word = status;
                (
                    observation,
                    entry_state,
                    Some(progress.statistics_completed),
                )
            }
        };
        self.raw = NAMED.into_iter().enumerate().fold(0, |bits, (bit, event)| {
            bits | u32::from(observation.contains(event)).wrapping_shl(bit as u32)
        }) | (u32::from(observation.events_7()) << 12)
            | (u32::from(observation.events_13()) << 13)
            | ((state as u32) << STATE_SHIFT)
            | match service {
                Some(statistics) => REASON | (u32::from(statistics) << 19),
                None => 0,
            };
    }

    fn fault(&self) -> Option<Fault> {
        if self.raw & !RECORD_BITS != 0 {
            return None;
        }
        let observation = Ieee802154EventObservation::from_parts(
            events_from(self.raw),
            self.raw & (1 << 12) != 0,
            self.raw & (1 << 13) != 0,
        );
        let state = STATES
            .into_iter()
            .find(|candidate| *candidate as u32 == (self.raw >> STATE_SHIFT) & STATE_MASK)?;
        if self.raw & REASON == 0 {
            return (self.raw & (1 << 19) == 0 && self.details == [0; 8])
                .then_some(Fault::UnhandledEvents { observation, state });
        }
        let [
            word,
            reason,
            status,
            accumulated_lo,
            accumulated_hi,
            increment_lo,
            increment_hi,
            drained,
        ] = self.details;
        if reason >> 16 != 0 {
            return None;
        }
        let parameter = (reason >> 8) as u8;
        let tag = reason & 255;
        let reason = match tag {
            0 => ServiceError::EventState(NAMED.into_iter().nth(usize::from(parameter))?),
            1 if parameter == 0 => ServiceError::RxAbortUnclassified(status_from(status)?),
            2 if parameter == 0 => ServiceError::RxAbortState(status_from(status)?),
            3 if parameter == 0 => ServiceError::TxAbortUnclassified,
            4 => ServiceError::FrequencyCode(parameter),
            6 if drained <= 13 => ServiceError::Statistics(StatisticsOverflow {
                counter: match parameter {
                    0 => StatisticsCounter::Transmit,
                    1 => StatisticsCounter::Deferred,
                    2 => StatisticsCounter::TxDone,
                    3 => StatisticsCounter::RxDone,
                    4 => StatisticsCounter::TxCoexistenceBreak,
                    code => {
                        StatisticsCounter::Hardware(COUNTERS.into_iter().find(|counter| {
                            5u32.wrapping_add(*counter as u32) == u32::from(code)
                        })?)
                    }
                },
                accumulated: u64::from(accumulated_lo) | (u64::from(accumulated_hi) << 32),
                increment: u64::from(increment_lo) | (u64::from(increment_hi) << 32),
                drained: drained as u8,
            }),
            _ => return None,
        };
        if tag != 6
            && [
                accumulated_lo,
                accumulated_hi,
                increment_lo,
                increment_hi,
                drained,
            ] != [0; 5]
        {
            return None;
        }
        if tag != 1 && tag != 2 && status != 0 {
            return None;
        }
        Some(Fault::Service {
            reason,
            observation,
            entry_state: state,
            state: STATES
                .into_iter()
                .find(|state| *state as u32 == word & 15)?,
            rx_abort: rx_from((word >> 4) & 31)?,
            tx_abort: tx_from((word >> 9) & 31)?,
            progress: ServiceProgress {
                acknowledged: word & (1 << 26) != 0,
                completed: events_from((word >> 14) & 0xfff),
                rx_abort_phase_done: word & (1 << 27) != 0,
                coexistence_break_notified: word & (1 << 31) != 0,
                statistics_completed: self.raw & (1 << 19) != 0,
                stage: STAGES
                    .into_iter()
                    .find(|stage| *stage as u32 == (word >> 28) & 7)?,
            },
        })
    }

    /// Render the validated source payload. The envelope owns acknowledgement.
    pub fn report(&mut self, write: impl FnMut(&[u8]) -> bool) -> fmt::Result {
        let Some(fault) = self.fault() else {
            return Ok(());
        };
        struct Output<F>(F);
        impl<F: FnMut(&[u8]) -> bool> Write for Output<F> {
            fn write_str(&mut self, text: &str) -> fmt::Result {
                if (self.0)(text.as_bytes()) {
                    Ok(())
                } else {
                    Err(fmt::Error)
                }
            }
        }
        writeln!(Output(write), "previous IEEE 802.15.4 fault {fault:?}")?;
        Ok(())
    }
}

const _: () = {
    if core::mem::size_of::<Payload>() != 36 {
        panic!("fault payload must preserve all nine observation words");
    }
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_unhandled_lane_and_state_survives_reset_and_output_refusal() {
        for state in STATES {
            for (events_7, clock, events_13) in [
                (true, false, false),
                (false, true, false),
                (false, false, true),
                (true, true, true),
            ] {
                let events = if clock {
                    Ieee802154EventMask::NAMED
                } else {
                    Ieee802154Event::RxDone.mask()
                };
                let fault = Fault::UnhandledEvents {
                    observation: Ieee802154EventObservation::from_parts(
                        events, events_7, events_13,
                    ),
                    state,
                };
                let mut record: Payload = bytemuck::Zeroable::zeroed();
                record.record(fault);
                assert_eq!(record.fault(), Some(fault));
                assert!(record.report(|_| false).is_err());
                assert_eq!(record.fault(), Some(fault));
                let mut output = std::vec::Vec::new();
                record
                    .report(|bytes| {
                        output.extend_from_slice(bytes);
                        true
                    })
                    .unwrap();
                let output = std::string::String::from_utf8(output).unwrap();
                assert!(output.contains(&std::format!("{fault:?}")));
            }
        }
    }

    #[test]
    fn all_fourteen_event_flags_round_trip_independently() {
        assert_eq!(core::mem::size_of::<Payload>(), 36);
        for flags in 0..(1u32 << 14) {
            let events = NAMED.into_iter().enumerate().fold(
                Ieee802154EventMask::NONE,
                |events, (bit, event)| {
                    if flags & (1 << bit) != 0 {
                        events.with(event)
                    } else {
                        events
                    }
                },
            );
            let fault = Fault::UnhandledEvents {
                observation: Ieee802154EventObservation::from_parts(
                    events,
                    flags & (1 << 12) != 0,
                    flags & (1 << 13) != 0,
                ),
                state: Ieee802154State::Rx,
            };
            let mut record: Payload = bytemuck::Zeroable::zeroed();
            record.record(fault);
            assert_eq!(record.fault(), Some(fault));
        }
    }

    #[test]
    fn invalid_state_reason_and_reserved_bits_are_rejected() {
        for raw in [
            0xf << STATE_SHIFT,
            REASON | (7 << 28),
            REASON | (15 << STATE_SHIFT),
            1 << 20,
            1 << 31,
        ] {
            let mut record = Payload {
                raw,
                details: [0; 8],
            };

            assert_eq!(record.fault(), None);
            record.report(|_| panic!("invalid encoding")).unwrap();
        }
    }
}

#[cfg(test)]
mod service_tests {
    use super::*;

    fn round_trip(fault: Fault) {
        let mut record: Payload = bytemuck::Zeroable::zeroed();
        record.record(fault);
        assert_eq!(record.fault(), Some(fault));
        assert!(record.report(|_| false).is_err());
        assert_eq!(record.fault(), Some(fault));
        record.report(|_| true).unwrap();
        assert_eq!(record.fault(), Some(fault));
    }

    #[test]
    fn service_reasons_preserve_full_observations_and_progress() {
        let mut reasons = std::vec![ServiceError::TxAbortUnclassified];
        for event in NAMED {
            reasons.push(ServiceError::EventState(event));
        }
        for value in [0, 127, 255] {
            reasons.push(ServiceError::FrequencyCode(value));
            for ordering in [
                core::cmp::Ordering::Less,
                core::cmp::Ordering::Equal,
                core::cmp::Ordering::Greater,
            ] {
                for abort in RX_REASONS
                    .into_iter()
                    .map(Ieee802154RxAbortReasonObservation::Named)
                    .chain([Ieee802154RxAbortReasonObservation::Unclassified])
                {
                    let status = Ieee802154RxStatus::new(
                        value,
                        abort,
                        Ieee802154RxStateCode::from_parts(value, ordering),
                        true,
                        false,
                    );
                    reasons.push(ServiceError::RxAbortState(status));
                    reasons.push(ServiceError::RxAbortUnclassified(status));
                }
            }
        }
        for counter in [
            StatisticsCounter::Transmit,
            StatisticsCounter::Deferred,
            StatisticsCounter::TxDone,
            StatisticsCounter::RxDone,
            StatisticsCounter::TxCoexistenceBreak,
        ]
        .into_iter()
        .chain(COUNTERS.into_iter().map(StatisticsCounter::Hardware))
        {
            for drained in [0, 1, 12, 13] {
                reasons.push(ServiceError::Statistics(StatisticsOverflow {
                    counter,
                    accumulated: u64::MAX,
                    increment: 0x1234_5678_9abc_def0,
                    drained,
                }));
            }
        }
        for reason in reasons {
            for stage in STAGES {
                for flags in 0..16 {
                    round_trip(Fault::Service {
                        reason,
                        observation: Ieee802154EventObservation::from_parts(
                            Ieee802154EventMask::NAMED,
                            true,
                            true,
                        ),
                        entry_state: Ieee802154State::Tx,
                        state: Ieee802154State::RxAck,
                        rx_abort: Ieee802154RxAbortReason::EnhancedAckSecurityError.into(),
                        tx_abort: Ieee802154TxAbortReasonObservation::Unclassified,
                        progress: ServiceProgress {
                            acknowledged: flags & 1 != 0,
                            completed: Ieee802154EventMask::NAMED,
                            rx_abort_phase_done: flags & 2 != 0,
                            coexistence_break_notified: flags & 4 != 0,
                            statistics_completed: flags & 8 != 0,
                            stage,
                        },
                    });
                }
            }
        }
    }

    #[test]
    fn invalid_service_fields_are_rejected() {
        for details in [
            [0, 255, 0, 0, 0, 0, 0, 0],
            [0, 3, 1, 0, 0, 0, 0, 0],
            [0, 6, 0, 0, 0, 0, 0, 14],
            [15, 3, 0, 0, 0, 0, 0, 0],
            [7 << 28, 3, 0, 0, 0, 0, 0, 0],
        ] {
            let record = Payload {
                raw: REASON,
                details,
            };

            assert_eq!(record.fault(), None);
        }
    }
}

fn encode(fault: Fault) -> Result<Record, fault_format::Error> {
    let class = match fault {
        Fault::Service {
            reason: ServiceError::Statistics(_),
            ..
        } => Class::Exhaustion,
        _ => Class::UnexpectedHardware,
    };
    let mut payload: Payload = bytemuck::Zeroable::zeroed();
    payload.record(fault);
    let [a, b, c, d, e, f, g, h] = payload.details;
    let reason = b;
    let words = [payload.raw, a, b, c, d, e, f, g, h];
    Record::encode(
        class,
        Domain::Ieee802154,
        reason,
        oer_diagnostics::fault::UNKNOWN_HART,
        0,
        &words,
    )
}
fn report(record: &mut Record, write: impl FnMut(&[u8]) -> bool) -> fmt::Result {
    record
        .report(|view| {
            let payload: Payload = view.decode(Domain::Ieee802154).map_err(|_| fmt::Error)?;
            let mut payload = payload;
            payload.fault().ok_or(fmt::Error)?;
            payload.report(write)
        })
        .map_err(|_| fmt::Error)
}
#[cfg(test)]
mod envelope_tests {
    use super::*;
    #[test]
    fn reset_and_output_refusal_preserve_the_entire_event_snapshot() {
        let fault = Fault::UnhandledEvents {
            observation: Ieee802154EventObservation::from_parts(
                Ieee802154EventMask::NAMED,
                true,
                true,
            ),
            state: Ieee802154State::Rx,
        };
        let mut reboot = encode(fault).unwrap();
        assert_eq!(
            reboot
                .peek()
                .unwrap()
                .decode::<Payload>(Domain::Ieee802154)
                .unwrap()
                .fault(),
            Some(fault)
        );
        assert!(report(&mut reboot, |_| false).is_err());
        assert!(reboot.peek().is_ok());
        report(&mut reboot, |_| true).unwrap();
        assert!(reboot.peek().is_err());
    }
}
