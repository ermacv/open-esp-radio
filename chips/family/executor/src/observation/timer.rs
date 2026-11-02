//! Timer boundaries routed to the image's executor observer. Timestamps and
//! deadlines retain their low 32 monotonic microsecond bits; consumers use
//! windows shorter than 2^31 us, as for executor activity.
use oer_trace::{Channel, Domain, Event, Kind};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Phase {
    Registration = 0,
    Arm = 1,
    Armed = 2,
    Stop = 3,
    Interrupt = 4,
    Dispatch = 5,
    Dispatched = 6,
    Invalid = 7,
}
impl Phase {
    fn decode(value: u8) -> Option<Self> {
        Some(match value {
            0 => Self::Registration,
            1 => Self::Arm,
            2 => Self::Armed,
            3 => Self::Stop,
            4 => Self::Interrupt,
            5 => Self::Dispatch,
            6 => Self::Dispatched,
            7 => Self::Invalid,
            _ => return None,
        })
    }
}

/// The event kind carries the bounded executor and phase; both payload words
/// remain available for timestamps. Platform kinds 64..95 are reserved here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Timer<const EXECUTOR: u8, const PHASE: u8> {
    pub at_micros: u32,
    /// Registration/Arm: deadline; Interrupt: acknowledgement time;
    /// Dispatch: whether an alarm fired; other phases: zero.
    pub value: u32,
}
impl<const E: u8, const P: u8> Event for Timer<E, P> {
    const KIND: Kind = const {
        if E >= 4 || P >= 8 {
            panic!("timer event context is bounded");
        }
        Kind::new(Domain::Platform, 64 + E * 8 + P)
    };
    const CHANNEL: Channel = <super::Activity as Event>::CHANNEL;
    fn encode(&self) -> [u32; 2] {
        [self.at_micros, self.value]
    }
    fn decode([at_micros, value]: [u32; 2]) -> Option<Self> {
        Some(Self { at_micros, value })
    }
}
impl<const E: u8, const P: u8> core::fmt::Display for Timer<E, P> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "timer executor={E} phase={P} at_us={} value={}",
            self.at_micros, self.value
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Observation {
    pub executor: u8,
    pub phase: Phase,
    pub at_micros: u32,
    pub value: u32,
}
impl Observation {
    pub fn of<E: Event>(event: &E) -> Option<Self> {
        // Compare complete kinds: another domain's equal local code is not ours.
        (0..4).find_map(|executor| {
            (0..8).find_map(|phase| {
                if E::KIND != Kind::new(Domain::Platform, 64 + executor * 8 + phase) {
                    return None;
                }
                let [at_micros, value] = event.encode();
                Some(Self {
                    executor,
                    phase: Phase::decode(phase)?,
                    at_micros,
                    value,
                })
            })
        })
    }
}

/// Statically dispatch both bounded identities, keeping every observer call
/// visible to the image's stack analysis. No callback table or installation.
#[cfg(all(oer_trace, feature = "chip"))]
pub(crate) fn emit<O: oer_trace::Observer>(
    observer: &O,
    executor: crate::SoftwareInterrupt,
    phase: Phase,
    at: u64,
    value: u64,
) {
    let phase =
        if matches!(phase, Phase::Registration | Phase::Arm) && at.abs_diff(value) >= 1 << 31 {
            Phase::Invalid
        } else {
            phase
        };
    fn deliver<O: oer_trace::Observer, const E: u8>(
        observer: &O,
        phase: Phase,
        at_micros: u32,
        value: u32,
    ) {
        macro_rules! emit {
            ($phase:literal) => {
                oer_trace::observe!(observer, Timer::<E, $phase> { at_micros, value })
            };
        }
        match phase {
            Phase::Registration => emit!(0),
            Phase::Arm => emit!(1),
            Phase::Armed => emit!(2),
            Phase::Stop => emit!(3),
            Phase::Interrupt => emit!(4),
            Phase::Dispatch => emit!(5),
            Phase::Dispatched => emit!(6),
            Phase::Invalid => emit!(7),
        }
    }
    use crate::SoftwareInterrupt;
    match executor {
        SoftwareInterrupt::Zero => deliver::<O, 0>(observer, phase, at as u32, value as u32),
        SoftwareInterrupt::One => deliver::<O, 1>(observer, phase, at as u32, value as u32),
        SoftwareInterrupt::Two => deliver::<O, 2>(observer, phase, at as u32, value as u32),
        SoftwareInterrupt::Three => deliver::<O, 3>(observer, phase, at as u32, value as u32),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn event_kinds_retain_executor_phase_and_both_timestamp_words() {
        let event = Timer::<2, 4> {
            at_micros: u32::MAX,
            value: 17,
        };
        assert_eq!(Timer::<2, 4>::decode(event.encode()), Some(event));
        assert_eq!(
            Observation::of(&event),
            Some(Observation {
                executor: 2,
                phase: Phase::Interrupt,
                at_micros: u32::MAX,
                value: 17
            })
        );
        assert_eq!(
            Observation::of(&super::super::Activity::new(
                0,
                0,
                0,
                0,
                super::super::Phase::WorkStart
            )),
            None
        );
    }
}
