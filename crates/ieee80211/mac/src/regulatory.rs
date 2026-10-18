//! The regulatory domain: where a radio may transmit, and at what power.
//!
//! A [`RegulatoryDomain`] is a country's rules for the 2.4 GHz band: the
//! 20 MHz channels it allows a station to transmit on, the widest channel
//! and the highest EIRP. [`RegulatoryDomain::legal`] turns a
//! [`WifiChannel`] into a [`LegalChannel`], the only value a radio start
//! takes: a channel every 20 MHz channel of which the domain allows, with
//! the domain's power ceiling. A lower ceiling (a lab's power cap) narrows
//! it with [`LegalChannel::capped`]; nothing raises it.
//!
//! The domain is a property of where the radio is, not of what the
//! hardware tunes to: a backend's own channel support is checked
//! separately, and a legal channel names no backend.
//!
//! The table holds only the domains a deployment uses today; a country
//! outside it is refused ([`UnlistedCountry`]), never given another
//! country's rules. The world domain (`00`) is a domain of its own, which
//! a caller names explicitly.
//!
//! Each entry carries its `SOURCE`. The values are those of the Linux
//! wireless regulatory database (wireless-regdb release 2026.09.03,
//! `db.txt`), which cites the regulation of each country; its power is the
//! maximum EIRP, and a channel is allowed when its whole 20 MHz lies in an
//! allowed frequency range without the `NO-IR` flag (no initiating
//! radiation, so no Probe Request).
//!
//! Host and application input (a country code) reaches this module; it
//! never panics on it.
#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]

use core::fmt;

use crate::channel::{WifiChannel, WifiChannelWidth};

/// An ISO 3166-1 alpha-2 country code in upper case, or `00` for the world
/// domain.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CountryCode([u8; 2]);

impl CountryCode {
    /// The world domain's code.
    pub const WORLD: Self = Self(*b"00");

    /// `code` as a country code: two ASCII letters in either case, or
    /// `00`.
    pub const fn new(code: [u8; 2]) -> Result<Self, CountryCodeError> {
        let [first, second] = code;
        if first == b'0' && second == b'0' {
            return Ok(Self::WORLD);
        }
        if first.is_ascii_alphabetic() && second.is_ascii_alphabetic() {
            Ok(Self([first.to_ascii_uppercase(), second.to_ascii_uppercase()]))
        } else {
            Err(CountryCodeError)
        }
    }

    /// The code in `text` (an OpenWrt UCI `country` value, for one): exactly
    /// two characters.
    pub const fn parse(text: &str) -> Result<Self, CountryCodeError> {
        match text.as_bytes() {
            [first, second] => Self::new([*first, *second]),
            _ => Err(CountryCodeError),
        }
    }

    /// The code's two upper-case ASCII bytes.
    pub const fn bytes(self) -> [u8; 2] {
        self.0
    }
}

impl fmt::Display for CountryCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [first, second] = self.0;
        write!(formatter, "{}{}", char::from(first), char::from(second))
    }
}

/// A text that is no two-letter country code nor `00`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CountryCodeError;

impl fmt::Display for CountryCodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("not a two-letter country code nor 00")
    }
}

impl core::error::Error for CountryCodeError {}

/// A country without rules in the table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UnlistedCountry(pub CountryCode);

impl fmt::Display for UnlistedCountry {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "no regulatory rules for country {}", self.0)
    }
}

impl core::error::Error for UnlistedCountry {}

/// A domain's rules for the 2.4 GHz band.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ghz2_4Rules {
    /// The 20 MHz channels a station may transmit on: `first..=last`.
    first_channel: u8,
    last_channel: u8,
    /// The widest channel allowed, in MHz.
    max_bandwidth_mhz: u16,
    /// The highest EIRP, in dBm.
    max_eirp_dbm: i8,
}

impl Ghz2_4Rules {
    /// The lowest channel a station may transmit on.
    pub const fn first_channel(&self) -> u8 {
        self.first_channel
    }

    /// The highest channel a station may transmit on.
    pub const fn last_channel(&self) -> u8 {
        self.last_channel
    }

    /// The widest channel allowed, in MHz.
    pub const fn max_bandwidth_mhz(&self) -> u16 {
        self.max_bandwidth_mhz
    }

    /// The highest EIRP, in dBm.
    pub const fn max_eirp_dbm(&self) -> i8 {
        self.max_eirp_dbm
    }

    const fn allows_20mhz(&self, channel: u8) -> bool {
        self.first_channel <= channel && channel <= self.last_channel
    }
}

/// One country's validated regulatory rules.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegulatoryDomain {
    country: CountryCode,
    ghz2_4: Ghz2_4Rules,
}

/// A domain of the table.
const fn domain(
    country: [u8; 2],
    first_channel: u8,
    last_channel: u8,
    max_eirp_dbm: i8,
) -> RegulatoryDomain {
    RegulatoryDomain {
        country: CountryCode(country),
        ghz2_4: Ghz2_4Rules {
            first_channel,
            last_channel,
            max_bandwidth_mhz: 40,
            max_eirp_dbm,
        },
    }
}

// The table: every domain with its source.

// SOURCE: wireless-regdb 2026.09.03 `db.txt`, `country 00`: rule
// `(2402 - 2472 @ 40), (20)` (channels 1-11, 20 dBm), with channels 12-13
// (`2457 - 2482 @ 20`) and 14 `NO-IR`: the intersection of the domains the
// database holds.
const WORLD: RegulatoryDomain = domain(*b"00", 1, 11, 20);
// SOURCE: wireless-regdb 2026.09.03 `db.txt`, `country DE`: rule
// `(2400 - 2483.5 @ 40), (100 mW)` (channels 1-13, 20 dBm EIRP), citing
// Bundesnetzagentur Vfg. 10/2013 (Allgemeinzuteilung WLAN 2,4 GHz) and EU
// decision 2006/771/EC as amended (ETSI EN 300 328).
const GERMANY: RegulatoryDomain = domain(*b"DE", 1, 13, 20);
// SOURCE: wireless-regdb 2026.09.03 `db.txt`, `country RU`: rule
// `(2400 - 2483.5 @ 40), (100mW)` (channels 1-13, 20 dBm EIRP), citing
// docs.cntd.ru document 1300597464 (the Russian allocation).
const RUSSIA: RegulatoryDomain = domain(*b"RU", 1, 13, 20);
// SOURCE: wireless-regdb 2026.09.03 `db.txt`, `country US`: rule
// `(2400 - 2472 @ 40), (30)` (channels 1-11, 30 dBm EIRP), citing 47 CFR
// Part 15 (15.247); channels 12-13 stay out for the restricted band above
// 2483.5 MHz (15.205).
const UNITED_STATES: RegulatoryDomain = domain(*b"US", 1, 11, 30);

impl RegulatoryDomain {
    /// The world domain (`00`): what every listed country allows.
    pub const WORLD: Self = WORLD;

    /// The rules of `country`; a country outside the table is refused.
    pub const fn for_country(country: CountryCode) -> Result<Self, UnlistedCountry> {
        match &country.0 {
            b"00" => Ok(WORLD),
            b"DE" => Ok(GERMANY),
            b"RU" => Ok(RUSSIA),
            b"US" => Ok(UNITED_STATES),
            _ => Err(UnlistedCountry(country)),
        }
    }

    /// The domain's country.
    pub const fn country(&self) -> CountryCode {
        self.country
    }

    /// The domain's 2.4 GHz rules.
    pub const fn ghz2_4(&self) -> Ghz2_4Rules {
        self.ghz2_4
    }

    /// `channel`, when the domain allows a station to transmit on every
    /// 20 MHz channel it occupies at its width, with the domain's power
    /// ceiling.
    pub const fn legal(&self, channel: WifiChannel) -> Result<LegalChannel, IllegalChannel> {
        let rules = self.ghz2_4;
        if channel.bandwidth_mhz() > rules.max_bandwidth_mhz {
            return Err(IllegalChannel::TooWide {
                country: self.country,
                channel,
            });
        }
        let primary = channel.primary();
        // A validated 40 MHz channel keeps its secondary in 1..=13.
        let secondary = match channel.width() {
            WifiChannelWidth::Mhz20 => primary,
            WifiChannelWidth::Mhz40Above => primary.saturating_add(4),
            WifiChannelWidth::Mhz40Below => primary.saturating_sub(4),
        };
        if !(rules.allows_20mhz(primary) && rules.allows_20mhz(secondary)) {
            return Err(IllegalChannel::Disallowed {
                country: self.country,
                channel,
            });
        }
        Ok(LegalChannel {
            channel,
            country: self.country,
            max_eirp_dbm: rules.max_eirp_dbm,
        })
    }
}

/// A channel the domain does not allow.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IllegalChannel {
    /// A 20 MHz channel the channel occupies is not allowed.
    Disallowed {
        country: CountryCode,
        channel: WifiChannel,
    },
    /// The channel is wider than the domain allows.
    TooWide {
        country: CountryCode,
        channel: WifiChannel,
    },
}

impl fmt::Display for IllegalChannel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disallowed { country, channel } => write!(
                formatter,
                "country {country} does not allow channel {} at {} MHz",
                channel.primary(),
                channel.bandwidth_mhz()
            ),
            Self::TooWide { country, channel } => write!(
                formatter,
                "country {country} does not allow {} MHz channels",
                channel.bandwidth_mhz()
            ),
        }
    }
}

impl core::error::Error for IllegalChannel {}

/// A 2.4 GHz channel a regulatory domain allows, with the power ceiling a
/// transmission on it keeps: only [`RegulatoryDomain::legal`] makes one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LegalChannel {
    channel: WifiChannel,
    country: CountryCode,
    max_eirp_dbm: i8,
}

impl LegalChannel {
    /// The channel.
    pub const fn channel(&self) -> WifiChannel {
        self.channel
    }

    /// The country whose rules allowed it.
    pub const fn country(&self) -> CountryCode {
        self.country
    }

    /// The power ceiling, in dBm EIRP: the domain's limit, or a lower cap.
    pub const fn max_eirp_dbm(&self) -> i8 {
        self.max_eirp_dbm
    }

    /// The same channel with its ceiling at most `cap_dbm`: the lower of
    /// the domain's limit and the cap. A cap never raises the ceiling.
    #[must_use]
    pub const fn capped(mut self, cap_dbm: i8) -> Self {
        if cap_dbm < self.max_eirp_dbm {
            self.max_eirp_dbm = cap_dbm;
        }
        self
    }
}

#[cfg(test)]
mod tests;
