#![no_std]
#![forbid(unsafe_code)]

//! Which channels a country permits, and at what power.
//!
//! A [`RegulatoryDomain`] holds the rules of one [`Country`]: frequency spans
//! with a power limit and flags. A protocol crate implements [`RadioChannel`]
//! for its channel type; [`RegulatoryDomain::permit`] returns the channel as a
//! [`Permitted`], the only value a tune, install, scan or transmission takes.
//! Without a domain nothing is permitted.
//!
//! ```compile_fail,E0451
//! # use oer_radio_regulatory::{Permitted, Initiate};
//! # fn make<C>(channel: C) -> Permitted<C> {
//! Permitted { channel, ceiling_mbm: 2000, initiate: Initiate::Allowed, indoor_only: false }
//! # }
//! ```

pub mod generated;

#[cfg(test)]
mod tests;

/// An ISO 3166-1 alpha-2 country code, two uppercase ASCII letters.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Country([u8; 2]);

/// A country code that is not two uppercase ASCII letters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidCountry;

impl Country {
    /// The country of `code`, when it is two uppercase ASCII letters.
    pub const fn new(code: [u8; 2]) -> Result<Self, InvalidCountry> {
        if code[0].is_ascii_uppercase() && code[1].is_ascii_uppercase() {
            Ok(Self(code))
        } else {
            Err(InvalidCountry)
        }
    }

    /// The two letters.
    pub const fn code(self) -> [u8; 2] {
        self.0
    }
}

/// A span of frequencies in kHz, `low` below `high`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct FrequencySpan {
    low_khz: u32,
    high_khz: u32,
}

impl FrequencySpan {
    /// The span from `low_khz` to `high_khz`, when it is not empty.
    pub const fn new(low_khz: u32, high_khz: u32) -> Option<Self> {
        if low_khz < high_khz {
            Some(Self { low_khz, high_khz })
        } else {
            None
        }
    }

    /// The lower edge.
    pub const fn low_khz(self) -> u32 {
        self.low_khz
    }

    /// The upper edge.
    pub const fn high_khz(self) -> u32 {
        self.high_khz
    }

    /// The width.
    pub const fn width_khz(self) -> u32 {
        self.high_khz - self.low_khz
    }
}

/// What a rule restricts besides power.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct RuleFlags(u8);

impl RuleFlags {
    /// No restriction.
    pub const NONE: Self = Self(0);
    /// No initiating radiation: listen only, never transmit first.
    pub const NO_IR: Self = Self(1);
    /// Dynamic frequency selection applies.
    pub const DFS: Self = Self(2);
    /// Indoor use only.
    pub const NO_OUTDOOR: Self = Self(4);

    /// Both sets of flags.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether every flag of `other` is set.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

/// One frequency span of a country with its limits.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Rule {
    span: FrequencySpan,
    max_bandwidth_khz: u32,
    max_eirp_mbm: i32,
    flags: RuleFlags,
}

impl Rule {
    /// A rule: the widest channel `max_bandwidth_khz`, the EIRP limit in mBm
    /// (hundredths of a dBm).
    pub const fn new(
        span: FrequencySpan,
        max_bandwidth_khz: u32,
        max_eirp_mbm: i32,
        flags: RuleFlags,
    ) -> Self {
        Self {
            span,
            max_bandwidth_khz,
            max_eirp_mbm,
            flags,
        }
    }

    /// The frequencies the rule covers.
    pub const fn span(&self) -> FrequencySpan {
        self.span
    }
}

/// Why a list of rules is not a domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InvalidDomain {
    /// Rule `index` starts below the end of the rule before it.
    Overlapping { index: usize },
}

/// The rules of one country: sorted by frequency and disjoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegulatoryDomain {
    country: Country,
    rules: &'static [Rule],
}

impl RegulatoryDomain {
    /// The domain of `country`, when `rules` are sorted and disjoint (touching
    /// spans are allowed).
    pub const fn new(country: Country, rules: &'static [Rule]) -> Result<Self, InvalidDomain> {
        let mut rest = rules;
        let mut index = 1;
        while let [previous, tail @ ..] = rest {
            if let [next, ..] = tail
                && next.span.low_khz < previous.span.high_khz
            {
                return Err(InvalidDomain::Overlapping { index });
            }
            rest = tail;
            index += 1;
        }
        Ok(Self { country, rules })
    }

    /// The country.
    pub const fn country(&self) -> Country {
        self.country
    }

    /// The rules, by ascending frequency.
    pub const fn rules(&self) -> &'static [Rule] {
        self.rules
    }

    /// The domain of `country` among the generated ones.
    pub fn of(country: Country) -> Option<Self> {
        generated::DOMAINS
            .iter()
            .copied()
            .find(|domain| domain.country == country)
    }

    /// `channel`, when the domain permits it.
    ///
    /// The channel's whole span must lie in rules that touch each other, and
    /// each of them must admit its width. The ceiling is the least EIRP of
    /// those rules. A NO-IR rule makes the channel [`Initiate::PassiveOnly`];
    /// a DFS rule refuses it.
    pub fn permit<C: RadioChannel>(&self, channel: C) -> Result<Permitted<C>, Refused> {
        let span = channel.span();
        let mut covered_to = span.low_khz;
        let mut ceiling_mbm = i32::MAX;
        let mut flags = RuleFlags::NONE;
        for rule in self.rules {
            if rule.span.high_khz <= covered_to {
                continue;
            }
            if rule.span.low_khz > covered_to {
                break;
            }
            if span.width_khz() > rule.max_bandwidth_khz {
                return Err(Refused::Bandwidth);
            }
            ceiling_mbm = ceiling_mbm.min(rule.max_eirp_mbm);
            flags = flags.union(rule.flags);
            covered_to = rule.span.high_khz;
            if covered_to >= span.high_khz {
                break;
            }
        }
        if covered_to < span.high_khz {
            return Err(Refused::NotAllocated);
        }
        if flags.contains(RuleFlags::DFS) {
            return Err(Refused::RadarDetectionRequired);
        }
        let initiate = if flags.contains(RuleFlags::NO_IR) {
            Initiate::PassiveOnly
        } else {
            Initiate::Allowed
        };
        Ok(Permitted {
            channel,
            ceiling_mbm,
            initiate,
            indoor_only: flags.contains(RuleFlags::NO_OUTDOOR),
        })
    }
}

/// A domain that permits the whole 2.4 GHz and 5 GHz range at 20 dBm, for the
/// tests of crates that take a [`Permitted`] channel.
#[cfg(feature = "test-support")]
pub const TEST_DOMAIN: RegulatoryDomain = {
    const RULES: &[Rule] = &[
        Rule::new(
            match FrequencySpan::new(2_400_000, 2_500_000) {
                Some(span) => span,
                None => panic!("test span"),
            },
            40_000,
            2000,
            RuleFlags::NONE,
        ),
        Rule::new(
            match FrequencySpan::new(5_100_000, 5_900_000) {
                Some(span) => span,
                None => panic!("test span"),
            },
            160_000,
            2000,
            RuleFlags::NONE,
        ),
    ];
    RegulatoryDomain {
        country: Country(*b"ZZ"),
        rules: RULES,
    }
};

/// A channel of some protocol, as frequencies.
pub trait RadioChannel: Copy {
    /// Every frequency the channel occupies.
    fn span(&self) -> FrequencySpan;
}

/// Whether a radio may start a transmission on a permitted channel.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Initiate {
    /// Transmit as the protocol allows.
    Allowed,
    /// Listen only: never transmit first, send no probe request or beacon.
    PassiveOnly,
}

/// Why a domain does not permit a channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Refused {
    /// A part of the channel lies outside every rule.
    NotAllocated,
    /// The channel is wider than a rule it lies in admits.
    Bandwidth,
    /// The channel is in a DFS band and no radar detection exists.
    RadarDetectionRequired,
}

/// A channel its domain permits. Only [`RegulatoryDomain::permit`] makes one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Permitted<C> {
    channel: C,
    ceiling_mbm: i32,
    initiate: Initiate,
    indoor_only: bool,
}

impl<C: Copy> Permitted<C> {
    /// The channel.
    pub const fn channel(&self) -> C {
        self.channel
    }

    /// Whether the radio may start a transmission on it.
    pub const fn initiate(&self) -> Initiate {
        self.initiate
    }

    /// Whether the rules allow the channel indoors only.
    pub const fn indoor_only(&self) -> bool {
        self.indoor_only
    }

    /// The rule's EIRP limit in mBm.
    pub const fn eirp_limit_mbm(&self) -> i32 {
        self.ceiling_mbm
    }

    /// The transmit power ceiling on this channel under `limits`.
    pub const fn tx_ceiling(&self, limits: PowerLimits) -> TxCeiling {
        let mut ceiling = self.ceiling_mbm.saturating_sub(limits.antenna_gain_mbm);
        if limits.chip_maximum_mbm < ceiling {
            ceiling = limits.chip_maximum_mbm;
        }
        if let Some(cap) = limits.owner_cap_mbm
            && cap < ceiling
        {
            ceiling = cap;
        }
        TxCeiling(ceiling)
    }
}

/// What besides the domain bounds transmit power, in mBm.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PowerLimits {
    /// The board's antenna gain, subtracted from the domain's EIRP.
    pub antenna_gain_mbm: i32,
    /// The most the chip conducts.
    pub chip_maximum_mbm: i32,
    /// A cap the owner sets, such as a laboratory limit.
    pub owner_cap_mbm: Option<i32>,
}

/// The conducted transmit power no transmission exceeds, in mBm.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct TxCeiling(i32);

impl TxCeiling {
    /// The ceiling in mBm.
    pub const fn mbm(self) -> i32 {
        self.0
    }

    /// The ceiling in quarter dBm, rounded down and saturated to `i8`.
    pub const fn quarter_dbm(self) -> i8 {
        let quarter = self.0.div_euclid(25);
        if quarter > i8::MAX as i32 {
            i8::MAX
        } else if quarter < i8::MIN as i32 {
            i8::MIN
        } else {
            quarter as i8
        }
    }

    /// The ceiling in whole dBm, rounded down and saturated to `i8`.
    pub const fn dbm(self) -> i8 {
        let dbm = self.0.div_euclid(100);
        if dbm > i8::MAX as i32 {
            i8::MAX
        } else if dbm < i8::MIN as i32 {
            i8::MIN
        } else {
            dbm as i8
        }
    }
}
