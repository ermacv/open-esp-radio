#![no_std]
//! The HIL platform's own trace events: the triggers that freeze the trace
//! when a boot ends badly. They carry no words; the post-mortem record holds
//! the details, and the trace holds what happened before.

use core::fmt;

use oer_base_trace::{Channel, Domain, Event, Kind};

/// The hang watchdog found a stall and is about to reset the chip.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Hang;

/// A panic is being handled.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Panic;

impl Event for Hang {
    const KIND: Kind = Kind::new(Domain::Platform, 1);
    const CHANNEL: Channel = Channel::new(Domain::Platform, 0);

    fn encode(&self) -> [u32; 2] {
        [0; 2]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        (words == [0; 2]).then_some(Self)
    }
}

impl Event for Panic {
    const KIND: Kind = Kind::new(Domain::Platform, 2);
    const CHANNEL: Channel = Channel::new(Domain::Platform, 0);

    fn encode(&self) -> [u32; 2] {
        [0; 2]
    }

    fn decode(words: [u32; 2]) -> Option<Self> {
        (words == [0; 2]).then_some(Self)
    }
}

impl fmt::Display for Hang {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("platform.hang")
    }
}

impl fmt::Display for Panic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("platform.panic")
    }
}

oer_base_trace::event_set!(pub PlatformTrace: Hang, Panic);

/// The name of a platform trace kind, for the host's decoded trace.
pub fn platform_name(kind: Kind) -> Option<&'static str> {
    if kind == Hang::KIND {
        Some("platform.hang")
    } else if kind == Panic::KIND {
        Some("platform.panic")
    } else {
        None
    }
}

/// The event sets an image's trace holds, for the host's decoded trace; an
/// event of another domain is shown as its domain, id and words. The radio
/// domains' sets join with their radio code.
#[cfg(feature = "describe")]
pub const EVENT_SETS: &[oer_base_trace::Describer] =
    &[<PlatformTrace as oer_base_trace::EventSet>::describe];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_triggers_have_distinct_named_kinds() {
        assert_ne!(Hang::KIND, Panic::KIND);
        assert_eq!(platform_name(Hang::KIND), Some("platform.hang"));
        assert_eq!(platform_name(Panic::KIND), Some("platform.panic"));
        assert_eq!(Hang::decode(Hang.encode()), Some(Hang));
    }
}
