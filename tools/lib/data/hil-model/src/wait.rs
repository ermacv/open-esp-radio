//! Waits derived from a contract, a protocol or a measurement, never a
//! literal.

use core::{fmt, num::NonZeroU32};

use serde::{Deserialize, Serialize};

use crate::{Overflow, SpanName, StandProfile, arithmetic};

/// A multiplier of at least one, as a ratio, so that a margin is exact and
/// reviewable.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "(u32, NonZeroU32)", into = "(u32, NonZeroU32)")]
pub struct Factor {
    numerator: u32,
    denominator: NonZeroU32,
}

/// A factor below one would shorten a bound.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidFactor;

impl fmt::Display for InvalidFactor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a factor is a ratio of at least one")
    }
}

impl std::error::Error for InvalidFactor {}

impl Factor {
    pub const ONE: Self = Self::times(NonZeroU32::MIN);

    /// A whole multiple, which is never below one.
    pub const fn times(multiple: NonZeroU32) -> Self {
        Self {
            numerator: multiple.get(),
            denominator: NonZeroU32::MIN,
        }
    }

    pub const fn new(numerator: u32, denominator: NonZeroU32) -> Result<Self, InvalidFactor> {
        if numerator < denominator.get() {
            return Err(InvalidFactor);
        }
        Ok(Self {
            numerator,
            denominator,
        })
    }

    /// `micros × factor`, rounded up.
    pub fn apply(self, micros: u64) -> Result<u64, Overflow> {
        let product = u128::from(micros) * u128::from(self.numerator);
        let rounded = product.div_ceil(u128::from(self.denominator.get()));
        u64::try_from(rounded).map_err(|_| Overflow)
    }
}

impl TryFrom<(u32, NonZeroU32)> for Factor {
    type Error = InvalidFactor;

    fn try_from((numerator, denominator): (u32, NonZeroU32)) -> Result<Self, Self::Error> {
        Self::new(numerator, denominator)
    }
}

impl From<Factor> for (u32, NonZeroU32) {
    fn from(factor: Factor) -> Self {
        (factor.numerator, factor.denominator)
    }
}

mod sealed {
    pub trait Sealed {}
}

/// A protocol interval typed by its protocol's unit. A [`Wait`] on a
/// protocol interval can only be made from one of these, never from a bare
/// number; a scenario's parameters hold them.
pub trait ProtocolQuantity: sealed::Sealed {
    fn interval_micros(&self) -> u64;
}

/// A protocol interval outside its protocol's range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OutOfRange {
    pub quantity: &'static str,
    pub value: u32,
}

impl fmt::Display for OutOfRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {} is outside its protocol's range",
            self.quantity, self.value
        )
    }
}

impl std::error::Error for OutOfRange {}

/// An IEEE 802.11 interval of at least one time unit of 1024 µs: a beacon
/// interval, a listen interval.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct TimeUnits(u32);

/// A Bluetooth LE connection interval in its 1.25 ms units, 6 to 3200 (7.5
/// ms to 4 s).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct ConnectionInterval(u32);

/// An IEEE 802.15.4 period of at least one symbol of the 2.4 GHz O-QPSK
/// PHY, 16 µs each: a backoff period, a turnaround, a superframe slot.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct Symbols(u32);

macro_rules! quantity {
    ($type:ident, $name:literal, $range:expr, $micros:expr) => {
        impl $type {
            pub const fn new(value: u32) -> Result<Self, OutOfRange> {
                let range = $range;
                if *range.start() <= value && value <= *range.end() {
                    Ok(Self(value))
                } else {
                    Err(OutOfRange {
                        quantity: $name,
                        value,
                    })
                }
            }

            pub const fn get(self) -> u32 {
                self.0
            }
        }

        impl TryFrom<u32> for $type {
            type Error = OutOfRange;

            fn try_from(value: u32) -> Result<Self, OutOfRange> {
                Self::new(value)
            }
        }

        impl From<$type> for u32 {
            fn from(quantity: $type) -> Self {
                quantity.0
            }
        }

        impl sealed::Sealed for $type {}

        impl ProtocolQuantity for $type {
            fn interval_micros(&self) -> u64 {
                u64::from(self.0) * $micros
            }
        }
    };
}

quantity!(TimeUnits, "802.11 time units", 1..=u32::MAX, 1_024);
quantity!(
    ConnectionInterval,
    "BLE connection interval",
    6..=3_200,
    1_250
);
quantity!(Symbols, "802.15.4 symbols", 1..=u32::MAX, 16);

#[derive(Clone, Debug, Eq, PartialEq)]
enum Source {
    Contract {
        ceiling_micros: u64,
        factor: Factor,
    },
    Protocol {
        interval_micros: u64,
        count: NonZeroU32,
    },
    Measured {
        span: SpanName,
    },
}

/// How long a scenario waits for something. Scenario and executor code
/// holds no duration literal: every wait names where its length comes
/// from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Wait(Source);

impl Wait {
    /// A contract's ceiling times a factor: the value of the port's own
    /// `CEILINGS` constant, which no backend may exceed (, owner
    /// decisions  and ).
    pub const fn contract(ceiling: oer_time::Duration, factor: Factor) -> Self {
        Self(Source::Contract {
            ceiling_micros: ceiling.as_micros(),
            factor,
        })
    }

    /// `count` protocol intervals, for example six BLE connection intervals.
    pub fn protocol<Q: ProtocolQuantity>(quantity: &Q, count: NonZeroU32) -> Self {
        Self(Source::Protocol {
            interval_micros: quantity.interval_micros(),
            count,
        })
    }

    /// A span measured on the stand ([`StandProfile`]).
    pub const fn measured(span: SpanName) -> Self {
        Self(Source::Measured { span })
    }
}

/// A stand span used although its samples are too few; every plan that
/// uses it carries the label.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Provisional {
    pub span: SpanName,
    pub reason: String,
}

/// A wait's length, and its label when it is provisional.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Resolved {
    pub micros: u64,
    pub provisional: Option<Provisional>,
}

/// Why a wait has no length.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WaitError {
    /// The stand profile has no such span.
    NoSpan(SpanName),
    Overflow,
}

impl From<Overflow> for WaitError {
    fn from(_: Overflow) -> Self {
        Self::Overflow
    }
}

impl fmt::Display for WaitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSpan(name) => write!(f, "the stand profile has no span `{name}`"),
            Self::Overflow => Overflow.fmt(f),
        }
    }
}

impl std::error::Error for WaitError {}

impl Wait {
    /// The wait's length in microseconds, with its label when provisional.
    pub fn micros(&self, profile: &StandProfile) -> Result<Resolved, WaitError> {
        match &self.0 {
            Source::Contract {
                ceiling_micros,
                factor,
            } => Ok(Resolved {
                micros: factor.apply(*ceiling_micros)?,
                provisional: None,
            }),
            Source::Protocol {
                interval_micros,
                count,
            } => Ok(Resolved {
                micros: arithmetic::mul(*interval_micros, u64::from(count.get()))?,
                provisional: None,
            }),
            Source::Measured { span } => profile.budget(span),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ratio(numerator: u32, denominator: u32) -> Factor {
        Factor::new(numerator, NonZeroU32::new(denominator).unwrap()).unwrap()
    }

    #[test]
    fn a_factor_below_one_is_rejected_and_an_overflow_is_reported() {
        assert_eq!(
            Factor::new(1, NonZeroU32::new(2).unwrap()),
            Err(InvalidFactor)
        );
        assert_eq!(ratio(3, 2).apply(1001), Ok(1502));
        assert_eq!(
            Factor::times(NonZeroU32::new(2).unwrap()).apply(u64::MAX),
            Err(Overflow)
        );
    }

    #[test]
    fn a_factor_below_one_is_refused_when_read() {
        assert_eq!(
            serde_json::from_str::<Factor>("[3,2]").unwrap(),
            ratio(3, 2)
        );
        assert!(serde_json::from_str::<Factor>("[1,2]").is_err());
        assert!(serde_json::from_str::<Factor>("[1,0]").is_err());
    }

    #[test]
    fn a_contract_wait_is_its_ceiling_times_its_factor() {
        #[allow(
            clippy::disallowed_methods,
            reason = "stands for a port's CEILINGS constant"
        )]
        let ceiling = oer_time::Duration::from_micros(4_000);
        let wait = Wait::contract(ceiling, Factor::times(NonZeroU32::new(2).unwrap()));
        assert_eq!(
            wait.micros(&StandProfile::empty()),
            Ok(Resolved {
                micros: 8_000,
                provisional: None
            })
        );
    }

    #[test]
    fn protocol_waits_come_from_typed_units_in_their_range() {
        let six = NonZeroU32::new(6).unwrap();
        let profile = StandProfile::empty();
        let micros = |wait: Wait| wait.micros(&profile).unwrap().micros;
        assert_eq!(
            micros(Wait::protocol(&ConnectionInterval::new(24).unwrap(), six)),
            180_000
        );
        assert_eq!(
            micros(Wait::protocol(&TimeUnits::new(100).unwrap(), six)),
            614_400
        );
        assert_eq!(
            micros(Wait::protocol(&Symbols::new(20).unwrap(), six)),
            1_920
        );
        let huge = Wait::protocol(&TimeUnits::new(u32::MAX).unwrap(), NonZeroU32::MAX);
        assert_eq!(huge.micros(&profile), Err(WaitError::Overflow));
    }

    #[test]
    fn protocol_units_outside_their_range_are_refused() {
        assert!(ConnectionInterval::new(5).is_err());
        assert!(ConnectionInterval::new(6).is_ok());
        assert!(ConnectionInterval::new(3_200).is_ok());
        assert!(ConnectionInterval::new(3_201).is_err());
        assert!(TimeUnits::new(0).is_err());
        assert!(Symbols::new(0).is_err());
        assert!(serde_json::from_str::<ConnectionInterval>("3201").is_err());
    }
}
