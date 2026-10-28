//! Unsupported-event observations and their retained diagnostic codec.

use super::Ieee802154State;
use core::fmt::{self, Write};
use oer_espressif_ieee802154_mac_values::{
    Ieee802154Event, Ieee802154EventMask, Ieee802154EventObservation,
};

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
const MAGIC: u32 = 0x154f_0003;
// Diagnostic layout: named events in bits 0..=11, unnamed 7/13 at 12/13,
// state at 14..=17, reason (UnhandledEvents = 0) at 18; upper bits are reserved.
const STATE_SHIFT: u32 = 14;
const STATE_MASK: u32 = 0xf;
const REASON: u32 = 1 << 18;
const RECORD_BITS: u32 = (1 << 19) - 1;

/// Twelve retained bytes: magic, packed diagnostic and checksum. The fourteen
/// event flags, state and reason share one word; its bits are not MMIO encoding.
/// Ordinary stores do not promise power-fail atomicity.
#[derive(Clone, Copy, bytemuck::AnyBitPattern)]
#[repr(C)]
pub struct FaultRecord {
    magic: u32,
    raw: u32,
    checksum: u32,
}

impl FaultRecord {
    /// Retain a terminal fault before software reset, without releasing owners.
    pub fn record(&mut self, fault: Fault) {
        let Fault::UnhandledEvents { observation, state } = fault;
        self.magic = 0;
        self.raw = NAMED.into_iter().enumerate().fold(0, |raw, (bit, event)| {
            raw | (u32::from(observation.contains(event)).wrapping_shl(bit as u32))
        }) | (u32::from(observation.events_7()) << 12)
            | (u32::from(observation.events_13()) << 13)
            | ((state as u32) << STATE_SHIFT);
        self.checksum = self.checksum();
        self.magic = MAGIC;
    }

    fn checksum(&self) -> u32 {
        (MAGIC ^ self.raw).wrapping_mul(0x0100_0193)
    }

    fn fault(&self) -> Option<Fault> {
        if self.magic != MAGIC || self.checksum != self.checksum() {
            return None;
        }
        if self.raw & !RECORD_BITS != 0 || self.raw & REASON != 0 {
            return None;
        }
        let events = NAMED.into_iter().enumerate().fold(
            Ieee802154EventMask::NONE,
            |events, (bit, event)| {
                if self.raw & 1u32.wrapping_shl(bit as u32) != 0 {
                    events.with(event)
                } else {
                    events
                }
            },
        );
        let observation = Ieee802154EventObservation::from_parts(
            events,
            self.raw & (1 << 12) != 0,
            self.raw & (1 << 13) != 0,
        );
        let state = (self.raw >> STATE_SHIFT) & STATE_MASK;
        Some(Fault::UnhandledEvents {
            observation,
            state: STATES
                .into_iter()
                .find(|candidate| *candidate as u32 == state)?,
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

const _: () = assert!(core::mem::size_of::<FaultRecord>() == 12);

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
            observation: Ieee802154EventObservation::from_parts(
                Ieee802154EventMask::NONE,
                true,
                false,
            ),
            state: Ieee802154State::Rx,
        });
        record.raw ^= 1 << 13;
        assert_eq!(record.fault(), None);
        record.report(|_| panic!("invalid record")).unwrap();
    }

    #[test]
    fn all_fourteen_event_flags_round_trip_independently() {
        assert_eq!(core::mem::size_of::<FaultRecord>(), 12);
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
            let mut record: FaultRecord = bytemuck::Zeroable::zeroed();
            record.record(fault);
            assert_eq!(record.fault(), Some(fault));
        }
    }

    #[test]
    fn invalid_state_reason_and_reserved_bits_are_rejected_with_valid_checksum() {
        for raw in [
            0xf << STATE_SHIFT,
            REASON,
            REASON | (1 << STATE_SHIFT),
            1 << 19,
            1 << 31,
        ] {
            let mut record = FaultRecord {
                magic: MAGIC,
                raw,
                checksum: 0,
            };
            record.checksum = record.checksum();
            assert_eq!(record.fault(), None);
            record.report(|_| panic!("invalid encoding")).unwrap();
        }
    }
}
