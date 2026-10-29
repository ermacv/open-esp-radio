//! Executor boundaries on the platform monotonic microsecond clock.
//! Times wrap modulo 2^32, like the trace record's microsecond field. Collectors
//! sample more often than half that period (about 35 minutes). Work spans include
//! preemption; only idle spans (bracketed with interrupts masked) measure CPU idle.
use oer_trace::{Channel, Domain, Event, Kind};
pub mod timer;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Phase {
    WorkStart = 0,
    WorkEnd = 1,
    IdleStart = 2,
    IdleEnd = 3,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Activity {
    pub at_micros: u32,
    pub executor: u8,
    pub hart: u8,
    pub level: u8,
    pub phase: Phase,
}
impl Activity {
    pub const fn new(at_micros: u32, executor: u8, hart: u8, level: u8, phase: Phase) -> Self {
        Self {
            at_micros,
            executor,
            hart,
            level,
            phase,
        }
    }
    pub fn of<E: Event>(event: &E) -> Option<Self> {
        (E::KIND == Self::KIND)
            .then(|| Self::decode(event.encode()))
            .flatten()
    }
}
impl Event for Activity {
    const KIND: Kind = Kind::new(Domain::Platform, 40);
    const CHANNEL: Channel = const {
        match Channel::new(Domain::Platform, 7) {
            Ok(channel) => channel,
            Err(_) => panic!("executor channel is in the platform range"),
        }
    };
    fn encode(&self) -> [u32; 2] {
        [
            self.at_micros,
            u32::from_le_bytes([self.executor, self.hart, self.level, self.phase as u8]),
        ]
    }
    fn decode([at_micros, context]: [u32; 2]) -> Option<Self> {
        let [executor, hart, level, phase] = context.to_le_bytes();
        let phase = match phase {
            0 => Phase::WorkStart,
            1 => Phase::WorkEnd,
            2 => Phase::IdleStart,
            3 => Phase::IdleEnd,
            _ => return None,
        };
        Some(Self {
            at_micros,
            executor,
            hart,
            level,
            phase,
        })
    }
}
impl core::fmt::Display for Activity {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "executor {} hart {} level {} {:?} at_us={}",
            self.executor, self.hart, self.level, self.phase, self.at_micros
        )
    }
}
oer_trace::event_set!(pub ExecutorEvents: Activity);

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn boundaries_round_trip_and_refuse_unknown_phases() {
        for phase in [
            Phase::WorkStart,
            Phase::WorkEnd,
            Phase::IdleStart,
            Phase::IdleEnd,
        ] {
            let event = Activity::new(u32::MAX, 2, 0, 2, phase);
            assert_eq!(Activity::decode(event.encode()), Some(event));
        }
        assert_eq!(Activity::decode([0, 4 << 24]), None);
    }
}
