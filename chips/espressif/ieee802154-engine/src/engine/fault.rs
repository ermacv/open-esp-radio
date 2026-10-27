//! Unsupported-event observations and their retained diagnostic codec.

use super::Ieee802154State;
use crate::types::{Ieee802154Event, Ieee802154EventMask, Ieee802154EventObservation};
use core::fmt::{self, Write};

/// Impossible hardware input; no selected event has been acknowledged or dispatched.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fault {
    /// A sample includes an unnamed lane or the unsupported clock-match event.
    UnhandledEvents {
        /// Complete sampled event set, including each unnamed lane separately.
        observation: Ieee802154EventObservation,
        /// Software state at entry, before any event was handled.
        state: Ieee802154State,
    },
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
const MAGIC: u32 = 0x154f_0001;

/// Twenty-four retained bytes: versioned reason, complete event sample, state
/// and checksum. Ordinary stores do not promise power-fail atomicity.
#[derive(Clone, Copy, bytemuck::AnyBitPattern)]
#[repr(C)]
pub struct FaultRecord {
    magic: u32,
    named: [u8; NAMED.len()],
    events_7: u8,
    events_13: u8,
    state: u8,
    reserved: u8,
    checksum: u32,
}

impl FaultRecord {
    /// Retain a terminal fault before software reset, without releasing owners.
    pub fn record(&mut self, fault: Fault) {
        let Fault::UnhandledEvents { observation, state } = fault;
        self.magic = 0;
        for (byte, event) in self.named.iter_mut().zip(NAMED) {
            *byte = u8::from(observation.contains(event));
        }
        self.events_7 = u8::from(observation.events_7());
        self.events_13 = u8::from(observation.events_13());
        self.state = state as u8;
        self.reserved = 0;
        self.checksum = self.checksum();
        self.magic = MAGIC;
    }

    fn checksum(&self) -> u32 {
        self.named
            .iter()
            .copied()
            .chain([self.events_7, self.events_13, self.state, self.reserved])
            .fold(MAGIC, |sum, byte| {
                (sum ^ u32::from(byte)).wrapping_mul(0x0100_0193)
            })
    }

    fn fault(&self) -> Option<Fault> {
        if self.magic != MAGIC || self.checksum != self.checksum() || self.reserved != 0 {
            return None;
        }
        if self.named.iter().any(|byte| *byte > 1) || self.events_7 > 1 || self.events_13 > 1 {
            return None;
        }
        let state = STATES
            .into_iter()
            .find(|state| *state as u8 == self.state)?;
        let events = self.named.iter().zip(NAMED).fold(
            Ieee802154EventMask::NONE,
            |events, (byte, event)| {
                if *byte == 1 {
                    events.with(event)
                } else {
                    events
                }
            },
        );
        Some(Fault::UnhandledEvents {
            observation: Ieee802154EventObservation::new(
                events,
                self.events_7 == 1,
                self.events_13 == 1,
            ),
            state,
        })
    }

    /// Report a valid previous-boot fault once. Refused output retains it.
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
        self.magic = 0;
        Ok(())
    }
}

const _: () = assert!(core::mem::size_of::<FaultRecord>() == 24);

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
                    observation: Ieee802154EventObservation::new(events, events_7, events_13),
                    state,
                };
                let mut record: FaultRecord = bytemuck::Zeroable::zeroed();
                assert_eq!(record.fault(), None);
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
                record.report(|_| panic!("already reported")).unwrap();
            }
        }
    }

    #[test]
    fn corrupt_record_is_not_reported() {
        let mut record: FaultRecord = bytemuck::Zeroable::zeroed();
        record.record(Fault::UnhandledEvents {
            observation: Ieee802154EventObservation::new(Ieee802154EventMask::NONE, true, false),
            state: Ieee802154State::Rx,
        });
        record.events_13 ^= 1;
        assert_eq!(record.fault(), None);
        record.report(|_| panic!("invalid record")).unwrap();
    }
}
