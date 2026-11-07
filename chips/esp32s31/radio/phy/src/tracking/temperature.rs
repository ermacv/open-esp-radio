//! Acquisition provenance for the retained PHY sensor value.
//!
//! The clock is the runtime's monotonic microsecond clock. Age is measured
//! conservatively from the start of the acquisition, including sensor waits.
//! A timestamp from inspecting state never makes a measurement fresh.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Acquisition {
    Unobserved,
    /// A completed sensor result without a usable acquisition clock.
    Undated,
    /// Acquisition exceeded the representable runtime observation interval.
    Overlong,
    /// The acquisition clock ran backwards before sensor completion.
    ClockReversed,
    Window {
        started: u64,
        completed: u64,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Observation {
    /// PHY sensor units, not a Celsius conversion.
    pub value: i16,
    pub acquisition: Acquisition,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Freshness {
    Unknown,
    ClockReversed,
    Fresh { age_micros: u64 },
    Stale { age_micros: u64 },
}

impl Observation {
    /// `maximum_age_micros` is an observation policy, not a safe calibration
    /// deferral period. Equality remains fresh; future timestamps are rejected.
    pub const fn freshness(self, now_micros: u64, maximum_age_micros: u64) -> Freshness {
        if matches!(self.acquisition, Acquisition::ClockReversed) {
            return Freshness::ClockReversed;
        }
        let Acquisition::Window { started, completed } = self.acquisition else {
            return Freshness::Unknown;
        };
        if completed < started || now_micros < completed {
            return Freshness::ClockReversed;
        }
        let age_micros = now_micros - started;
        if age_micros <= maximum_age_micros {
            Freshness::Fresh { age_micros }
        } else {
            Freshness::Stale { age_micros }
        }
    }
}

#[cfg(test)]
mod tests;

// Keep timestamp storage byte-aligned. An aligned u64 enum would increase
// alignment of the entire registered owner and every containing RV32 role
// future. These are ordinary semantic timestamps, not an MMIO/ABI image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct StoredAcquisition {
    started: [u8; 8],
    elapsed: [u8; 4],
    kind: Kind,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Unobserved,
    Undated,
    Window,
    Overlong,
    ClockReversed,
}
impl StoredAcquisition {
    pub const UNOBSERVED: Self = Self {
        started: [0; 8],
        elapsed: [0; 4],
        kind: Kind::Unobserved,
    };
    pub const fn new(acquisition: Acquisition) -> Self {
        match acquisition {
            Acquisition::Unobserved => Self::UNOBSERVED,
            Acquisition::Undated => Self {
                kind: Kind::Undated,
                ..Self::UNOBSERVED
            },
            Acquisition::Overlong => Self {
                kind: Kind::Overlong,
                ..Self::UNOBSERVED
            },
            Acquisition::ClockReversed => Self {
                kind: Kind::ClockReversed,
                ..Self::UNOBSERVED
            },
            Acquisition::Window { started, completed } => {
                if completed < started {
                    return Self::new(Acquisition::ClockReversed);
                }
                let elapsed = completed - started;
                if elapsed > u32::MAX as u64 {
                    return Self::new(Acquisition::Overlong);
                }
                Self {
                    kind: Kind::Window,
                    started: started.to_le_bytes(),
                    elapsed: (elapsed as u32).to_le_bytes(),
                }
            }
        }
    }
    pub const fn get(self) -> Acquisition {
        match self.kind {
            Kind::Unobserved => Acquisition::Unobserved,
            Kind::Undated => Acquisition::Undated,
            Kind::Overlong => Acquisition::Overlong,
            Kind::ClockReversed => Acquisition::ClockReversed,
            Kind::Window => Acquisition::Window {
                started: u64::from_le_bytes(self.started),
                completed: u64::from_le_bytes(self.started)
                    + u32::from_le_bytes(self.elapsed) as u64,
            },
        }
    }
}
