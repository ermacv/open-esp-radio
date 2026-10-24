//! Typed radio-root observations, shared by the boot console and trace.

use oer_esp32s31_hal::power::PlatformClock;
use oer_trace::{Channel, Domain, Event, Kind};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RadioRoot {
    Handoff,
    ClockReleased(PlatformClock),
    ClockRefused(PlatformClock),
    Passed,
    ClockFailed,
}

impl Event for RadioRoot {
    const KIND: Kind = Kind::new(Domain::Platform, 1);
    const CHANNEL: Channel = Channel::new(Domain::Platform, 0);

    fn encode(&self) -> [u32; 2] {
        let clock = |clock| match clock {
            PlatformClock::Pll160m => 0,
            PlatformClock::AnalogI2cMaster => 1,
            PlatformClock::Mpll => 2,
            PlatformClock::ModemCoexistence => 3,
            PlatformClock::ModemLowPowerTimer => 4,
        };
        match *self {
            Self::Handoff => [0, 0],
            Self::ClockReleased(value) => [2, clock(value)],
            Self::ClockRefused(value) => [3, clock(value)],
            Self::Passed => [4, 0],
            Self::ClockFailed => [5, 0],
        }
    }

    fn decode([event, clock]: [u32; 2]) -> Option<Self> {
        Some(match (event, clock) {
            (0, 0) => Self::Handoff,
            (4, 0) => Self::Passed,
            (5, 0) => Self::ClockFailed,
            (2 | 3, 0..=4) => {
                let clock = match clock {
                    0 => PlatformClock::Pll160m,
                    1 => PlatformClock::AnalogI2cMaster,
                    2 => PlatformClock::Mpll,
                    3 => PlatformClock::ModemCoexistence,
                    _ => PlatformClock::ModemLowPowerTimer,
                };
                if event == 2 {
                    Self::ClockReleased(clock)
                } else {
                    Self::ClockRefused(clock)
                }
            }
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_ids_preserve_the_platform_clock_identity() {
        for (id, clock) in [
            PlatformClock::Pll160m,
            PlatformClock::AnalogI2cMaster,
            PlatformClock::Mpll,
            PlatformClock::ModemCoexistence,
            PlatformClock::ModemLowPowerTimer,
        ]
        .into_iter()
        .enumerate()
        {
            for (tag, event) in [
                (2, RadioRoot::ClockReleased(clock)),
                (3, RadioRoot::ClockRefused(clock)),
            ] {
                assert_eq!(event.encode(), [tag, id as u32]);
                assert_eq!(RadioRoot::decode([tag, id as u32]), Some(event));
            }
        }
    }
}
