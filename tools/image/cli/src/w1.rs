//! Image `w1`: its stand identity (the BSSID and channel of the stand
//! router's AP after the profile, `oer_lab_router::router`, and the stand's SSID)
//! and the check of the report block it prints after its exit.
//!
//! The BSSID is never printed whole: it is an `oer_lab_router::mac::Mac`, and every
//! message shows only its last two octets (`xx:xx:xx:xx:F6:D0`); the SSID is
//! never shown at all. Both go only into the environment of the image's
//! runtime build.
//!
//! The host recomputes every gate of the block from its other lines and
//! compares the result with the image's `w1.gate` lines and its
//! `OER-RESULT:` line; any disagreement is exit code 3, like a missing or
//! malformed block.

use std::fmt;

use oer_lab_router::mac::Mac;

/// The image these parameters belong to.
pub const IMAGE: &str = "w1";
/// The runtime build's variable of the channel.
pub const ENV_CHANNEL: &str = "OER_W1_CHANNEL";
/// The runtime build's variable of the stand's BSSID.
pub const ENV_BSSID: &str = "OER_W1_BSSID";
/// The runtime build's variable of the stand's SSID, in hex.
pub const ENV_SSID: &str = "OER_W1_SSID_HEX";
/// The runtime build's variable of the stand's regulatory country (the
/// router radio's UCI `country`).
pub const ENV_COUNTRY: &str = "OER_W1_COUNTRY";
/// Every variable of the runtime build.
pub const ENV: [&str; 4] = [ENV_CHANNEL, ENV_BSSID, ENV_SSID, ENV_COUNTRY];
/// The first line of the report block.
pub const BEGIN: &str = "OER-W1-BEGIN";
/// The last line of the report block.
pub const END: &str = "OER-W1-END";
/// The marker of the image's verdict line.
const RESULT_MARKER: &str = "OER-RESULT: ";
/// The interrupt stack the `irq_stack` gate requires to stay free.
pub const MIN_IRQ_STACK_FREE: u64 = 4096;
/// The longest gap list the image prints.
const MAX_GAPS: usize = 64;
/// The longest tracking tick list the image prints.
const MAX_TICKS: usize = 32;
/// The survey's table size.
const MAX_BSS: usize = 64;
/// The longest lag list the image prints.
const MAX_LAGS: usize = 64;
/// The longest TSF pair list the image prints.
const MAX_TSF_PAIRS: usize = 64;
/// The hold cycles of a run: `forward`, then `reverse`.
const CYCLES: [(&str, &str); 2] = [("0", "forward"), ("1", "reverse")];
/// The frames after a release whose descriptor index `w1.own` lists.
const MAX_FIRST_AFTER: usize = 16;
/// The reasons of `w1.own discards=`: the runner's drops (the unit never
/// reaches the application), then the application's prefix refusals.
const DISCARDS: [&str; 15] = [
    "empty",
    "too_long",
    "chained",
    "oversized",
    "overload",
    "rx_failure",
    "mic_failure",
    "quarantined",
    "invalid",
    "prefix_chain",
    "bounds",
    "output_small",
    "metadata",
    "ignored",
    "unsupported",
];
/// The runner's drops among [`DISCARDS`].
const RUNNER_DROPS: [&str; 5] = ["empty", "too_long", "chained", "oversized", "overload"];
/// Microseconds per time unit.
const TU_US: u64 = 1024;

/// The stand's SSID, 1 to 32 bytes; its `Debug` shows only its length and
/// it has no `Display`.
#[derive(Clone, Eq, PartialEq)]
pub struct Ssid(Vec<u8>);

impl Ssid {
    pub fn new(bytes: &[u8]) -> Result<Self, String> {
        if bytes.is_empty() || bytes.len() > 32 {
            return Err(format!("the SSID is {} bytes, not 1 to 32", bytes.len()));
        }
        Ok(Self(bytes.to_vec()))
    }

    /// Two lower-case hex digits per byte, for the runtime build only.
    fn hex(&self) -> String {
        self.0.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}

impl fmt::Debug for Ssid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Ssid(<{} bytes>)", self.0.len())
    }
}

/// The full address, colon-separated upper case, for the runtime build's
/// environment only.
fn unmasked(mac: &Mac) -> String {
    mac.octets()
        .iter()
        .map(|octet| format!("{octet:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// The two letters of `country`.
pub fn country_text(country: [u8; 2]) -> String {
    country.iter().copied().map(char::from).collect()
}

/// The stand image `w1` is built for and checked against.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    /// The router AP's channel after the profile.
    pub channel: u8,
    /// The router AP's BSSID: the stand.
    pub bssid: Mac,
    /// The SSID the stand's beacons carry.
    pub ssid: Ssid,
    /// The router radio's regulatory country.
    pub country: [u8; 2],
}

impl Config {
    /// The variables of the runtime build.
    pub fn runtime_env(&self) -> [(&'static str, String); 4] {
        [
            (ENV_CHANNEL, self.channel.to_string()),
            (ENV_BSSID, unmasked(&self.bssid)),
            (ENV_SSID, self.ssid.hex()),
            (ENV_COUNTRY, country_text(self.country)),
        ]
    }
}

/// Why a report could not be read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReportError {
    /// No `OER-W1-BEGIN` line.
    Missing,
    Malformed(String),
}

impl fmt::Display for ReportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => write!(formatter, "no {BEGIN} block"),
            Self::Malformed(reason) => write!(formatter, "malformed report: {reason}"),
        }
    }
}

fn malformed(reason: impl Into<String>) -> ReportError {
    ReportError::Malformed(reason.into())
}

/// The kind of a value.
#[derive(Clone, Copy)]
enum Kind {
    /// A decimal number up to the bound.
    Number(u64),
    /// A decimal number up to the bound, or `-`.
    NumberOrDash(u64),
    /// `0x` and hex digits, up to `u32::MAX`.
    Hex,
    /// [`Kind::Hex`], or `-`.
    HexOrDash,
    /// Any token.
    Text,
    /// `xx:xx:xx:xx:HH:HH`.
    Masked,
    /// `<min>..<max>` or `-`.
    Range {
        signed: bool,
    },
    /// A decimal `i32`, or `-`.
    SignedOrDash,
    /// A decimal number, or `<at_least>..<at_most>` when a histogram
    /// bucket holds the threshold.
    NumberOrRange,
    OneOf(&'static [&'static str]),
    /// A descriptor index up to `u32::MAX`, `x` (unknown) or `-`.
    IndexOrDash,
    /// `-`, or up to [`MAX_FIRST_AFTER`] descriptor indices (each up to
    /// `u32::MAX`, or `x`) joined by `,`.
    IndexList,
    /// `-`, or `<reason>:<count>` of distinct [`DISCARDS`] reasons, each
    /// count from 1 to `u32::MAX`, joined by `,`.
    Discards,
}

const U8: Kind = Kind::Number(u8::MAX as u64);
const U16: Kind = Kind::Number(u16::MAX as u64);
const U32: Kind = Kind::Number(u32::MAX as u64);
const U64: Kind = Kind::Number(u64::MAX);
const U32_OR_DASH: Kind = Kind::NumberOrDash(u32::MAX as u64);
const FLAG_OR_DASH: Kind = Kind::OneOf(&["0", "1", "-"]);
const CYCLE: Kind = Kind::OneOf(&["0", "1"]);
/// The phases of a run, as the image names them.
const PHASES: &[&str] = &["bringup", "first_frame", "window", "stress", "hold", "stop"];

struct Key {
    name: &'static str,
    kind: Kind,
    optional: bool,
}

const fn key(name: &'static str, kind: Kind) -> Key {
    Key {
        name,
        kind,
        optional: false,
    }
}

#[derive(Clone, Copy)]
enum Count {
    Once,
    Exactly(usize),
    AtMost(usize),
    Any,
}

struct Spec {
    keyword: &'static str,
    keys: &'static [Key],
    /// Every number key that `keys` does not name is accepted too.
    open: bool,
    count: Count,
}

const GATE_NAMES: [&str; 13] = [
    "beacons",
    "channel",
    "exit",
    "walker_restart",
    "ownership",
    "irq_stack",
    "service_errors",
    "unserviced",
    "timer_wake",
    "base_writes",
    "reserved",
    "owner_bit",
    "walker_on_detached",
];

/// The hold fill window's bounds, in ms (the image's `hold_fill_ms`).
const HOLD_FILL_MS: core::ops::RangeInclusive<u64> = 3_000..=10_000;

macro_rules! numbers {
    ($($name:literal),* $(,)?) => {
        &[$(key($name, U64)),*]
    };
}

const SPECS: &[Spec] = &[
    Spec {
        keyword: "w1.config",
        keys: &[
            key("channel", U8),
            key("applied_channel", U8),
            key("stand", Kind::Masked),
            key("ssid_matches", U64),
            key("window_ms", U64),
            key("cpu_hz", U64),
            key("arena", Kind::Text),
            key("slots", U64),
            key("mask", Kind::Hex),
        ],
        open: false,
        count: Count::Once,
    },
    Spec {
        keyword: "w1.bss",
        keys: &[
            key("idx", U64),
            key("who", Kind::OneOf(&["stand", "other"])),
            key("bssid", Kind::Masked),
            key("bi_tu", U16),
            key("ch", Kind::NumberOrDash(u8::MAX as u64)),
            key("beacons", U32),
            key("dup", U32),
            key("probe_rsp", U32),
            key("rssi", Kind::Range { signed: true }),
            key("no_rssi", U32),
            key("gaps_late", U32),
            key("max_gap_us", U32),
            key("missed", U32),
            key("lag_us", Kind::Range { signed: false }),
            key("ssid_len", U8),
            key("ssid_ok", Kind::OneOf(&["0", "1"])),
            key("stamp_span_us", U32_OR_DASH),
            key("handoff_span_us", U32_OR_DASH),
        ],
        open: false,
        count: Count::AtMost(MAX_BSS),
    },
    Spec {
        keyword: "w1.gap",
        keys: &[
            key("stamp", U32),
            key("gap_us", U32),
            key("missed", Kind::NumberOrDash(u32::MAX as u64)),
        ],
        open: false,
        count: Count::AtMost(MAX_GAPS),
    },
    Spec {
        keyword: "w1.gaps",
        keys: &[key("dropped", U32)],
        open: false,
        count: Count::Once,
    },
    Spec {
        keyword: "w1.tick",
        keys: &[
            key("start", U32),
            key("end", U32),
            key("ok", Kind::OneOf(&["0", "1"])),
        ],
        open: false,
        count: Count::AtMost(MAX_TICKS),
    },
    Spec {
        keyword: "w1.rx",
        keys: numbers!(
            "units",
            "bytes",
            "mac_irqs",
            "power_irqs",
            "unserviced_mac",
            "unserviced_pwr",
            "doorbell_unmasks",
            "ignored_unmasks",
            "chained",
            "oversized",
            "empty",
            "too_long",
            "overload",
            "admission_blocks",
            "stage_capacity_blocks",
            "service_passes",
            "service_errors",
            "capacity_drains",
            "frames_after_release",
        ),
        open: false,
        count: Count::Once,
    },
    Spec {
        keyword: "w1.hold",
        keys: &[
            key("cycle", CYCLE),
            key("fill_ms", U32),
            key("reached", U64),
            key("slots", U64),
            key("full", Kind::OneOf(&["0", "1"])),
            key("reason", Kind::OneOf(&["all", "stuck-at-last", "-"])),
            key("units", U64),
            key("mac_irqs", U64),
            key("stage_capacity_blocks", U64),
            key("admission_blocks", U64),
            key("end_released", U32_OR_DASH),
            key("end_detached", U32_OR_DASH),
            key("end_staged", U32_OR_DASH),
            key("end_observed", U32_OR_DASH),
            key("end_recycle_start", U32_OR_DASH),
            key("end_accepted_tail", U32_OR_DASH),
            key("end_next", Kind::HexOrDash),
            key("end_last", Kind::HexOrDash),
        ],
        open: false,
        count: Count::Exactly(CYCLES.len()),
    },
    Spec {
        keyword: "w1.exhaust",
        keys: &[
            key("cycle", CYCLE),
            key("snapshots", U64),
            key("blocked_us", U32_OR_DASH),
            key("next_low", Kind::HexOrDash),
            key("next_upper", FLAG_OR_DASH),
            key("last_low", Kind::HexOrDash),
            key("base", Kind::HexOrDash),
            key("walker", FLAG_OR_DASH),
            key("doorbell", FLAG_OR_DASH),
            key("buffer_full", Kind::NumberOrDash(u16::MAX as u64)),
            key("ring_base", Kind::HexOrDash),
            key("recycle_start", U32_OR_DASH),
            key("accepted_tail", U32_OR_DASH),
            key("observed", U32_OR_DASH),
            key("completed", U32_OR_DASH),
            key("remaining", U32_OR_DASH),
            key("reload_pending", FLAG_OR_DASH),
            key("released", U32_OR_DASH),
            key("detached", U32_OR_DASH),
            key("staged", U32_OR_DASH),
            key("next_after_doorbell", Kind::HexOrDash),
            key("last_after_doorbell", Kind::HexOrDash),
            key("desc_hash_changed", FLAG_OR_DASH),
        ],
        open: false,
        count: Count::Exactly(CYCLES.len()),
    },
    Spec {
        keyword: "w1.own",
        keys: &[
            key("cycle", CYCLE),
            key("order", Kind::OneOf(&["forward", "reverse"])),
            key("held", U64),
            key("owner_clear", U64),
            key("first_drop", U32_OR_DASH),
            key("hold_mismatch", U32),
            key("dwell_mismatch", U32),
            key("release_mismatch", U32),
            key("first_mismatch", Kind::IndexOrDash),
            key("first_after_release", Kind::IndexList),
            key("order_breaks", U32),
            key("chained", U32),
            key("oversized", U32),
            key("discards", Kind::Discards),
        ],
        open: false,
        count: Count::Exactly(CYCLES.len()),
    },
    Spec {
        keyword: "w1.prefix",
        keys: numbers!(
            "strict_rxend_refusable",
            "strict_dump_refusable",
            "admitted"
        ),
        open: true,
        count: Count::Once,
    },
    Spec {
        keyword: "w1.reload",
        keys: numbers!(
            "max_attempts",
            "limit_us",
            "observations",
            "reload_rerings",
            "rering_failures",
            "base_writes",
            "reserved_landings",
            "reserved_late_landings",
            "walker_on_detached",
        ),
        open: false,
        count: Count::Once,
    },
    Spec {
        keyword: "w1.continuation",
        keys: numbers!("longest_streak"),
        open: true,
        count: Count::Once,
    },
    Spec {
        keyword: "w1.teardown",
        keys: numbers!("mask_confirm_us", "drain_us", "drain_polls"),
        open: false,
        count: Count::Once,
    },
    Spec {
        keyword: "w1.exit",
        keys: &[key("cause", Kind::Text), key("end", Kind::Text)],
        open: false,
        count: Count::Once,
    },
    Spec {
        keyword: "w1.p2",
        keys: &[
            key("phase", Kind::OneOf(&["window", "stress", "hold"])),
            key("swi2_max_us", U64),
            key("swi2_p99_us", U64),
            key("actor_max_us", U64),
            key("actor_p99_us", U64),
            key("actor_over_bound", Kind::NumberOrRange),
            key("receive_max_us", U64),
            key("receive_p99_us", U64),
            key("mac_to_poll_max_us", U64),
            key("ring_to_doorbell_max_us", U64),
        ],
        open: false,
        count: Count::Any,
    },
    Spec {
        keyword: "w1.clock",
        keys: &[
            key("op", Kind::OneOf(&["acquire", "release", "abandon"])),
            key("clock", Kind::Text),
            key("phase", Kind::Text),
            key("count", U64),
            key("max_us", U64),
        ],
        open: false,
        count: Count::Any,
    },
    Spec {
        keyword: "w1.stack",
        keys: numbers!("irq_bytes", "irq_free", "min_free"),
        open: false,
        count: Count::Once,
    },
    Spec {
        keyword: "w1.timer",
        keys: &[
            key("cmp", Kind::OneOf(&["0", "1"])),
            key("phase", Kind::OneOf(&["window", "stress"])),
            key("early", U64),
            key("late_count", U64),
            key("late_max_us", U64),
            key("late_total_us", U64),
            key("missed", U64),
            key("unmatched", U64),
            key("coalesced", U64),
            key("overlapping", U64),
            key("armed_at_end", U64),
            key("waits", U64),
            key("woke_early", U64),
        ],
        open: false,
        count: Count::Any,
    },
    Spec {
        keyword: "w1.wake",
        keys: &[
            key("cmp", Kind::OneOf(&["0", "1"])),
            key("phase", Kind::OneOf(PHASES)),
            key("waits", U64),
            key("woke_early", U64),
        ],
        open: false,
        count: Count::Any,
    },
    Spec {
        keyword: "w1.lag",
        keys: &[
            key("i", U64),
            key("stamp", U32),
            key("lag_us", U32),
            key("since_prev_us", U32_OR_DASH),
            key("until_next_us", U32_OR_DASH),
        ],
        open: false,
        count: Count::AtMost(MAX_LAGS),
    },
    Spec {
        keyword: "w1.lagfit",
        keys: &[
            key("n", U64),
            key("median_us", U32_OR_DASH),
            key("max_us", U32_OR_DASH),
            key("next_arrival_matches", U64),
            key("tolerance_us", U32),
        ],
        open: false,
        count: Count::Once,
    },
    Spec {
        keyword: "w1.tsf",
        keys: &[key("i", U64), key("tsf", U32), key("handoff", U32)],
        open: false,
        count: Count::AtMost(MAX_TSF_PAIRS),
    },
    Spec {
        keyword: "w1.tsffit",
        keys: &[
            key("n", U64),
            key("bss", U32_OR_DASH),
            key("median_us", U32_OR_DASH),
            key("p90_us", U32_OR_DASH),
            key("max_us", U32_OR_DASH),
            key("slope_ppm", Kind::SignedOrDash),
        ],
        open: false,
        count: Count::Once,
    },
    Spec {
        keyword: "w1.survey",
        keys: numbers!(
            "frames",
            "header_refusals",
            "data",
            "ignored_management",
            "table_full",
            "off_channel",
        ),
        open: false,
        count: Count::Once,
    },
    Spec {
        keyword: "w1.verdict",
        keys: numbers!("beacons", "bi_us", "window_us", "required"),
        open: false,
        count: Count::Once,
    },
    Spec {
        keyword: "w1.gate",
        keys: &[
            key("name", Kind::OneOf(&GATE_NAMES)),
            key("result", Kind::OneOf(&["pass", "fail", "unexercised"])),
            Key {
                name: "reason",
                kind: Kind::Text,
                optional: true,
            },
        ],
        open: false,
        count: Count::AtMost(GATE_NAMES.len()),
    },
];

/// The validated `key=value` pairs of one line, in their order.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Fields {
    keyword: &'static str,
    pairs: Vec<(String, String)>,
}

impl Fields {
    fn get(&self, key: &str) -> Result<&str, ReportError> {
        self.pairs
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
            .ok_or_else(|| malformed(format!("{} has no {key}=", self.keyword)))
    }

    /// The decimal number `key`.
    pub fn number(&self, key: &str) -> Result<u64, ReportError> {
        let value = self.get(key)?;
        value
            .parse()
            .map_err(|_| malformed(format!("{} {key}={value} is no number", self.keyword)))
    }

    /// The number `key`, or `None` for `-`.
    fn number_or_dash(&self, key: &str) -> Result<Option<u64>, ReportError> {
        if self.get(key)? == "-" {
            return Ok(None);
        }
        self.number(key).map(Some)
    }

    /// The signed number `key`, or `None` for `-`.
    fn signed_or_dash(&self, key: &str) -> Result<Option<i64>, ReportError> {
        let value = self.get(key)?;
        if value == "-" {
            return Ok(None);
        }
        signed(value)
            .map(Some)
            .ok_or_else(|| malformed(format!("{} {key}={value} is no number", self.keyword)))
    }

    /// The token `key`.
    pub fn text(&self, key: &str) -> Result<&str, ReportError> {
        self.get(key)
    }

    /// The lower bound of the range `key`, or `None` for `-`.
    fn range_min(&self, key: &str) -> Result<Option<i64>, ReportError> {
        let value = self.get(key)?;
        if value == "-" {
            return Ok(None);
        }
        value
            .split_once("..")
            .and_then(|(min, _)| min.parse().ok())
            .map(Some)
            .ok_or_else(|| malformed(format!("{} {key}= is no range", self.keyword)))
    }
}

fn decimal(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// `0x` and hex digits, up to `u32::MAX`.
fn hex(text: &str) -> bool {
    text.strip_prefix("0x").is_some_and(|digits| {
        !digits.is_empty()
            && digits.bytes().all(|byte| byte.is_ascii_hexdigit())
            && u32::from_str_radix(digits, 16).is_ok()
    })
}

/// The `<reason>:<count>` pairs of a `discards=` list other than `-`;
/// `None` for an unknown or repeated reason or a count outside 1 to
/// `u32::MAX`.
fn discards(list: &str) -> Option<Vec<(&str, u64)>> {
    let mut pairs: Vec<(&str, u64)> = Vec::new();
    for pair in list.split(',') {
        let (reason, count) = pair.split_once(':')?;
        let count = decimal(count).filter(|&count| (1..=u64::from(u32::MAX)).contains(&count))?;
        if !DISCARDS.contains(&reason) || pairs.iter().any(|(seen, _)| *seen == reason) {
            return None;
        }
        pairs.push((reason, count));
    }
    Some(pairs)
}

/// A descriptor index up to `u32::MAX`, or `x`.
fn index(text: &str) -> bool {
    text == "x" || decimal(text).is_some_and(|number| number <= u64::from(u32::MAX))
}

fn signed(text: &str) -> Option<i64> {
    let (negative, digits) = match text.strip_prefix('-') {
        Some(digits) => (true, digits),
        None => (false, text),
    };
    let magnitude = i64::try_from(decimal(digits)?).ok()?;
    Some(if negative { -magnitude } else { magnitude })
}

/// Validate `value` as `kind`; a masked BSSID comes back in upper case.
fn validate(kind: Kind, value: &str) -> Result<String, String> {
    let valid = match kind {
        Kind::Number(bound) => decimal(value).is_some_and(|number| number <= bound),
        Kind::NumberOrDash(bound) => {
            value == "-" || decimal(value).is_some_and(|number| number <= bound)
        }
        Kind::Hex => hex(value),
        Kind::HexOrDash => value == "-" || hex(value),
        Kind::SignedOrDash => {
            value == "-" || signed(value).is_some_and(|number| i32::try_from(number).is_ok())
        }
        Kind::Text => !value.is_empty(),
        Kind::Masked => {
            let octets = value.strip_prefix("xx:xx:xx:xx:").map(|rest| rest.split(':'));
            let valid = octets.is_some_and(|octets| {
                let octets = octets.collect::<Vec<_>>();
                octets.len() == 2
                    && octets.iter().all(|octet| {
                        octet.len() == 2 && octet.bytes().all(|byte| byte.is_ascii_hexdigit())
                    })
            });
            if !valid {
                // Never repeat what may be an unmasked address.
                return Err("is no masked BSSID xx:xx:xx:xx:HH:HH".to_owned());
            }
            return Ok(value.to_ascii_uppercase().replace('X', "x"));
        }
        Kind::Range { signed: true } => {
            value == "-"
                || value.split_once("..").is_some_and(|(min, max)| {
                    matches!((signed(min), signed(max)), (Some(min), Some(max)) if min <= max)
                })
        }
        Kind::Range { signed: false } => {
            value == "-"
                || value.split_once("..").is_some_and(|(min, max)| {
                    matches!(
                        (decimal(min), decimal(max)),
                        (Some(min), Some(max)) if min <= max && max <= u64::from(u32::MAX)
                    )
                })
        }
        Kind::NumberOrRange => {
            decimal(value).is_some()
                || value.split_once("..").is_some_and(|(min, max)| {
                    matches!((decimal(min), decimal(max)), (Some(min), Some(max)) if min <= max)
                })
        }
        Kind::OneOf(values) => values.contains(&value),
        Kind::IndexOrDash => value == "-" || index(value),
        Kind::IndexList => {
            value == "-"
                || (value.split(',').count() <= MAX_FIRST_AFTER && value.split(',').all(index))
        }
        Kind::Discards => value == "-" || discards(value).is_some(),
    };
    if valid {
        Ok(value.to_owned())
    } else {
        Err(format!("= `{value}` is invalid"))
    }
}

/// Read one block line against the specs.
fn parse_line(line: &str) -> Result<Fields, ReportError> {
    let mut tokens = line.split_whitespace();
    let keyword = tokens.next().unwrap_or_default();
    let spec = SPECS
        .iter()
        .find(|spec| spec.keyword == keyword)
        .ok_or_else(|| malformed(format!("unknown line `{keyword}`")))?;
    let mut pairs: Vec<(String, String)> = Vec::new();
    for token in tokens {
        let (name, value) = token
            .split_once('=')
            .filter(|(name, _)| !name.is_empty())
            .ok_or_else(|| malformed(format!("{keyword}: `{token}` is no key=value")))?;
        if pairs.iter().any(|(existing, _)| existing == name) {
            return Err(malformed(format!("{keyword}: {name}= twice")));
        }
        let kind = match spec.keys.iter().find(|key| key.name == name) {
            Some(key) => key.kind,
            None if spec.open => U64,
            None => return Err(malformed(format!("{keyword}: unknown key {name}="))),
        };
        let value =
            validate(kind, value).map_err(|error| malformed(format!("{keyword} {name}{error}")))?;
        pairs.push((name.to_owned(), value));
    }
    for key in spec.keys {
        if !key.optional && !pairs.iter().any(|(name, _)| name == key.name) {
            return Err(malformed(format!("{keyword} has no {}=", key.name)));
        }
    }
    Ok(Fields {
        keyword: spec.keyword,
        pairs,
    })
}

/// One beacon gap of the stand.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Gap {
    /// The frame stamp (µs, low 32 bits) of the beacon that ended the gap.
    pub stamp: u32,
    pub gap_us: u32,
    pub missed: Option<u32>,
}

/// One window unit of the lag list: its receive stamp and its handoff lag
/// (the handoff's low 32 bits less the stamp, wrapping).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Lag {
    pub stamp: u32,
    pub lag_us: u32,
}

/// The handoff delays of a lag list, as `w1.lagfit` reports them. The
/// stamp clock runs at the handoff clock's rate, so a lag is the unit's
/// delay plus one offset; the smallest lag stands for the offset (`delay =
/// lag − min lag`, taken relative to the first lag so a wrap is harmless).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LagFit {
    /// The lower median delay.
    pub median_us: u32,
    pub max_us: u32,
    /// Units whose corrected handoff lies 0 to `tolerance_us` after the
    /// next unit's stamp: handed off only once the next unit arrived.
    pub next_arrival_matches: u64,
}

/// The fit of `lags` at `tolerance_us`; `None` without a unit.
pub fn lag_fit(lags: &[Lag], tolerance_us: u32) -> Option<LagFit> {
    let first = lags.first()?;
    let relative = |lag: &Lag| i64::from(lag.lag_us.wrapping_sub(first.lag_us) as i32);
    let floor = lags.iter().map(relative).min()?;
    let delay = |lag: &Lag| relative(lag) - floor;
    let mut delays = lags.iter().map(delay).collect::<Vec<_>>();
    delays.sort_unstable();
    let narrow = |value: i64| u32::try_from(value).unwrap_or(u32::MAX);
    let median_us = narrow(*delays.get((delays.len() - 1) / 2)?);
    let max_us = narrow(*delays.last()?);
    let next_arrival_matches = lags
        .windows(2)
        .filter(|pair| match pair {
            [unit, next] => {
                let after = delay(unit) - i64::from(next.stamp.wrapping_sub(unit.stamp));
                (0..=i64::from(tolerance_us)).contains(&after)
            }
            _ => false,
        })
        .count() as u64;
    Some(LagFit {
        median_us,
        max_us,
        next_arrival_matches,
    })
}

/// One stand beacon: its TSF and its handoff, both µs, low 32 bits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TsfPair {
    pub tsf: u32,
    pub handoff: u32,
}

/// The handoff times of beacons against their TSF with the linear trend
/// removed: least squares of handoff on TSF (integer arithmetic, both
/// relative to the first pair), residuals floored to µs and taken less the
/// smallest one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TsfFit {
    /// The lower median, the 90th percentile (rank ⌈0.9·n⌉) and the maximum
    /// of residual − min residual.
    pub median_us: u32,
    pub p90_us: u32,
    pub max_us: u32,
    /// (slope − 1) · 10⁶, floored.
    pub slope_ppm: i32,
}

/// The fit of `pairs`; `None` below two pairs or without a TSF spread.
pub fn tsf_fit(pairs: &[TsfPair]) -> Option<TsfFit> {
    let first = pairs.first()?;
    let point = |pair: &TsfPair| {
        (
            i128::from(pair.tsf.wrapping_sub(first.tsf) as i32),
            i128::from(pair.handoff.wrapping_sub(first.handoff) as i32),
        )
    };
    let n = i128::try_from(pairs.len()).ok()?;
    let (mut sx, mut sy, mut sxx, mut sxy) = (0_i128, 0_i128, 0_i128, 0_i128);
    for (x, y) in pairs.iter().map(point) {
        sx += x;
        sy += y;
        sxx += x * x;
        sxy += x * y;
    }
    let dxx = n * sxx - sx * sx;
    let dxy = n * sxy - sx * sy;
    if n < 2 || dxx == 0 {
        return None;
    }
    let denominator = n * dxx;
    let intercept = sy * dxx - dxy * sx;
    let residuals = pairs
        .iter()
        .map(point)
        .map(|(x, y)| (n * dxx * y - intercept - n * dxy * x).div_euclid(denominator))
        .collect::<Vec<_>>();
    let floor = *residuals.iter().min()?;
    let mut spread = residuals
        .iter()
        .map(|residual| u32::try_from(residual - floor).unwrap_or(u32::MAX))
        .collect::<Vec<_>>();
    spread.sort_unstable();
    let count = spread.len();
    let slope_ppm = ((dxy - dxx) * 1_000_000).div_euclid(dxx);
    Some(TsfFit {
        median_us: *spread.get((count - 1) / 2)?,
        p90_us: *spread.get((count * 9).div_ceil(10).checked_sub(1)?)?,
        max_us: *spread.last()?,
        slope_ppm: i32::try_from(slope_ppm).unwrap_or(if slope_ppm < 0 {
            i32::MIN
        } else {
            i32::MAX
        }),
    })
}

/// One periodic PHY tracking tick, SYSTIMER µs, low 32 bits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Tick {
    pub start: u32,
    pub end: u32,
    pub ok: bool,
}

/// A gate.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum GateName {
    Beacons,
    Channel,
    Exit,
    WalkerRestart,
    Ownership,
    IrqStack,
    ServiceErrors,
    Unserviced,
    TimerWake,
    BaseWrites,
    Reserved,
    OwnerBit,
    WalkerOnDetached,
}

impl GateName {
    pub const ALL: [Self; 13] = [
        Self::Beacons,
        Self::Channel,
        Self::Exit,
        Self::WalkerRestart,
        Self::Ownership,
        Self::IrqStack,
        Self::ServiceErrors,
        Self::Unserviced,
        Self::TimerWake,
        Self::BaseWrites,
        Self::Reserved,
        Self::OwnerBit,
        Self::WalkerOnDetached,
    ];

    /// The name in a `w1.gate` line.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Beacons => "beacons",
            Self::Channel => "channel",
            Self::Exit => "exit",
            Self::WalkerRestart => "walker_restart",
            Self::Ownership => "ownership",
            Self::IrqStack => "irq_stack",
            Self::ServiceErrors => "service_errors",
            Self::Unserviced => "unserviced",
            Self::TimerWake => "timer_wake",
            Self::BaseWrites => "base_writes",
            Self::Reserved => "reserved",
            Self::OwnerBit => "owner_bit",
            Self::WalkerOnDetached => "walker_on_detached",
        }
    }

    /// The name in an `OER-RESULT:` reason: `walker-restart`.
    pub fn phrase(self) -> String {
        self.as_str().replace('_', "-")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GateResult {
    Pass,
    Fail,
    Unexercised,
}

impl GateResult {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Unexercised => "unexercised",
        }
    }
}

/// A `w1.gate` line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImageGate {
    pub name: GateName,
    pub result: GateResult,
    pub reason: Option<String>,
}

/// The report block of one run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Report {
    pub config: Fields,
    pub bss: Vec<Fields>,
    pub gaps: Vec<Gap>,
    pub gaps_dropped: u64,
    pub ticks: Vec<Tick>,
    pub rx: Fields,
    /// One per hold cycle, in cycle order.
    pub hold: Vec<Fields>,
    /// One per hold cycle, in cycle order.
    pub exhaust: Vec<Fields>,
    /// One per hold cycle, in cycle order.
    pub own: Vec<Fields>,
    pub prefix: Fields,
    pub reload: Fields,
    pub continuation: Fields,
    pub teardown: Fields,
    pub exit: Fields,
    pub p2: Vec<Fields>,
    pub clock: Vec<Fields>,
    pub stack: Fields,
    pub timer: Vec<Fields>,
    pub wake: Vec<Fields>,
    /// The first window units' lags, in order.
    pub lags: Vec<Lag>,
    pub lagfit: Fields,
    /// The first stand beacons' TSF and handoff, in order.
    pub tsf: Vec<TsfPair>,
    pub tsffit: Fields,
    pub survey: Fields,
    pub verdict: Fields,
    /// One per gate, in [`GateName::ALL`] order.
    pub gates: Vec<ImageGate>,
}

impl Report {
    /// Read the block between `OER-W1-BEGIN v=1` and `OER-W1-END
    /// lines=<n>` out of the console `lines`; anything before or after it
    /// is ignored.
    pub fn parse<S: AsRef<str>>(lines: &[S]) -> Result<Self, ReportError> {
        let first_token = |line: &S| line.as_ref().split_whitespace().next() == Some(BEGIN);
        let begin = lines
            .iter()
            .position(first_token)
            .ok_or(ReportError::Missing)?;
        let Some((head, body)) = lines.get(begin..).and_then(<[S]>::split_first) else {
            return Err(ReportError::Missing);
        };
        if body.iter().any(first_token) {
            return Err(malformed(format!("two {BEGIN} lines")));
        }
        let head = head.as_ref().split_whitespace().collect::<Vec<_>>();
        if head != [BEGIN, "v=1"] {
            return Err(malformed(format!("{BEGIN} is not version v=1")));
        }
        let end = body
            .iter()
            .position(|line| line.as_ref().split_whitespace().next() == Some(END))
            .ok_or_else(|| malformed(format!("no {END} line")))?;
        let (block, rest) = body.split_at_checked(end).unwrap_or((body, &[]));
        let tail = rest
            .first()
            .map(|line| line.as_ref().split_whitespace().collect::<Vec<_>>())
            .unwrap_or_default();
        let count = match tail.as_slice() {
            [_, count] => count.strip_prefix("lines=").and_then(decimal),
            _ => None,
        }
        .ok_or_else(|| malformed(format!("{END} has no lines=<count>")))?;
        if u64::try_from(end).ok() != Some(count) {
            return Err(malformed(format!(
                "{END} counts {count} lines, the block has {end}"
            )));
        }
        let mut parsed: Vec<Fields> = Vec::new();
        for line in block {
            parsed.push(parse_line(line.as_ref())?);
        }
        for spec in SPECS {
            let found = parsed
                .iter()
                .filter(|fields| fields.keyword == spec.keyword)
                .count();
            let fits = match spec.count {
                Count::Once => found == 1,
                Count::Exactly(count) => found == count,
                Count::AtMost(most) => found <= most,
                Count::Any => true,
            };
            if !fits {
                return Err(malformed(format!(
                    "{} {found} lines of {}",
                    spec.keyword,
                    match spec.count {
                        Count::Once => "1".to_owned(),
                        Count::Exactly(count) => count.to_string(),
                        Count::AtMost(most) => format!("at most {most}"),
                        Count::Any => "any".to_owned(),
                    }
                )));
            }
        }
        let all = |keyword: &str| {
            parsed
                .iter()
                .filter(|fields| fields.keyword == keyword)
                .cloned()
                .collect::<Vec<_>>()
        };
        let one = |keyword: &str| all(keyword).pop().unwrap_or_default();
        let narrow = |value: u64| u32::try_from(value).map_err(|_| malformed("value exceeds u32"));
        let mut gaps = Vec::new();
        for gap in all("w1.gap") {
            gaps.push(Gap {
                stamp: narrow(gap.number("stamp")?)?,
                gap_us: narrow(gap.number("gap_us")?)?,
                missed: gap.number_or_dash("missed")?.map(narrow).transpose()?,
            });
        }
        let mut ticks = Vec::new();
        for tick in all("w1.tick") {
            ticks.push(Tick {
                start: narrow(tick.number("start")?)?,
                end: narrow(tick.number("end")?)?,
                ok: tick.text("ok")? == "1",
            });
        }
        let lag_lines = all("w1.lag");
        let mut lags = Vec::new();
        for (index, line) in lag_lines.iter().enumerate() {
            if line.number("i")? != index as u64 {
                return Err(malformed(format!("w1.lag i= is not {index}")));
            }
            let lag = Lag {
                stamp: narrow(line.number("stamp")?)?,
                lag_us: narrow(line.number("lag_us")?)?,
            };
            let since_prev = lags
                .last()
                .map(|previous: &Lag| u64::from(lag.stamp.wrapping_sub(previous.stamp)));
            if line.number_or_dash("since_prev_us")? != since_prev {
                return Err(malformed(format!(
                    "w1.lag i={index} since_prev_us= disagrees with the stamps"
                )));
            }
            if let Some(previous) = index
                .checked_sub(1)
                .and_then(|previous| lag_lines.get(previous))
                && previous.number_or_dash("until_next_us")? != since_prev
            {
                return Err(malformed(format!(
                    "w1.lag i={} until_next_us= disagrees with the stamps",
                    index - 1
                )));
            }
            lags.push(lag);
        }
        if let Some(last) = lag_lines.last()
            && last.number_or_dash("until_next_us")?.is_some()
        {
            return Err(malformed("the last w1.lag has an until_next_us="));
        }
        let lagfit = one("w1.lagfit");
        if lagfit.number("n")? != lags.len() as u64 {
            return Err(malformed("w1.lagfit n= is not the w1.lag count"));
        }
        let mut tsf = Vec::new();
        for (index, line) in all("w1.tsf").iter().enumerate() {
            if line.number("i")? != index as u64 {
                return Err(malformed(format!("w1.tsf i= is not {index}")));
            }
            tsf.push(TsfPair {
                tsf: narrow(line.number("tsf")?)?,
                handoff: narrow(line.number("handoff")?)?,
            });
        }
        let tsffit = one("w1.tsffit");
        if tsffit.number("n")? != tsf.len() as u64 {
            return Err(malformed("w1.tsffit n= is not the w1.tsf count"));
        }
        let mut gates = Vec::new();
        for name in GateName::ALL {
            let lines = all("w1.gate")
                .into_iter()
                .filter(|gate| gate.text("name").is_ok_and(|text| text == name.as_str()))
                .collect::<Vec<_>>();
            let [gate] = lines.as_slice() else {
                return Err(malformed(format!(
                    "{} w1.gate lines of {}",
                    lines.len(),
                    name.as_str()
                )));
            };
            let result = match gate.text("result")? {
                "pass" => GateResult::Pass,
                "fail" => GateResult::Fail,
                _ => GateResult::Unexercised,
            };
            let reason = gate.text("reason").ok().map(str::to_owned);
            if (result == GateResult::Pass) != reason.is_none() {
                return Err(malformed(format!(
                    "gate {} {}: a reason= belongs to every result but pass",
                    name.as_str(),
                    result.as_str()
                )));
            }
            gates.push(ImageGate {
                name,
                result,
                reason,
            });
        }
        for keyword in ["w1.hold", "w1.exhaust", "w1.own"] {
            for (line, (cycle, order)) in all(keyword).iter().zip(CYCLES) {
                if line.text("cycle")? != cycle {
                    return Err(malformed(format!("{keyword} cycle= is not {cycle}")));
                }
                if keyword == "w1.own" && line.text("order")? != order {
                    return Err(malformed(format!(
                        "w1.own cycle={cycle} order= is not {order}"
                    )));
                }
                if keyword == "w1.hold" {
                    let fill_ms = line.number("fill_ms")?;
                    if !HOLD_FILL_MS.contains(&fill_ms) {
                        return Err(malformed(format!(
                            "w1.hold cycle={cycle} fill_ms={fill_ms} is outside {}..={}",
                            HOLD_FILL_MS.start(),
                            HOLD_FILL_MS.end()
                        )));
                    }
                    let (reached, slots) = (line.number("reached")?, line.number("slots")?);
                    let consistent = match (line.text("full")?, line.text("reason")?) {
                        ("1", "all") => reached == slots,
                        ("1", "stuck-at-last") => reached.checked_add(1) == Some(slots),
                        ("0", "-") => reached < slots,
                        _ => false,
                    };
                    if !consistent {
                        return Err(malformed(format!(
                            "w1.hold cycle={cycle} full=/reason= disagree with reached={reached} slots={slots}"
                        )));
                    }
                }
            }
        }
        let bss = all("w1.bss");
        if bss
            .iter()
            .filter(|bss| bss.text("who").is_ok_and(|who| who == "stand"))
            .count()
            > 1
        {
            return Err(malformed("more than one w1.bss who=stand"));
        }
        Ok(Self {
            config: one("w1.config"),
            bss,
            gaps,
            gaps_dropped: one("w1.gaps").number("dropped")?,
            ticks,
            rx: one("w1.rx"),
            hold: all("w1.hold"),
            exhaust: all("w1.exhaust"),
            own: all("w1.own"),
            prefix: one("w1.prefix"),
            reload: one("w1.reload"),
            continuation: one("w1.continuation"),
            teardown: one("w1.teardown"),
            exit: one("w1.exit"),
            p2: all("w1.p2"),
            clock: all("w1.clock"),
            stack: one("w1.stack"),
            timer: all("w1.timer"),
            wake: all("w1.wake"),
            lags,
            lagfit,
            tsf,
            tsffit,
            survey: one("w1.survey"),
            verdict: one("w1.verdict"),
            gates,
        })
    }

    /// The survey entry of the stand AP.
    pub fn stand(&self) -> Option<&Fields> {
        self.bss
            .iter()
            .find(|bss| bss.text("who").is_ok_and(|who| who == "stand"))
    }
}

/// Why no beacon count can be required.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequiredError {
    ZeroInterval,
}

/// The beacons a window of `window_us` must hold at the beacon interval
/// `bi_us`: ⌊8·window_us / (10·bi_us)⌋, 80 % of the expected count.
pub fn required_beacons(window_us: u64, bi_us: u64) -> Result<u64, RequiredError> {
    if bi_us == 0 {
        return Err(RequiredError::ZeroInterval);
    }
    let required = 8 * u128::from(window_us) / (10 * u128::from(bi_us));
    // At most 0.8 · window_us, so it fits.
    Ok(u64::try_from(required).unwrap_or(u64::MAX))
}

/// The stand's beacon figures the `beacons` gate rests on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BeaconFigures {
    pub beacons: u64,
    pub bi_us: u64,
    pub window_us: u64,
    /// `None` at a zero beacon interval.
    pub required: Option<u64>,
}

/// A gate as the host recomputes it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostGate {
    pub name: GateName,
    pub result: GateResult,
    pub reason: Option<&'static str>,
    /// The brief fixes the reason token, so the image's must match.
    pub bound_reason: bool,
}

/// Every gate recomputed from a report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Host {
    /// In [`GateName::ALL`] order.
    pub gates: Vec<HostGate>,
    pub beacons: Option<BeaconFigures>,
}

impl Host {
    pub fn passed(&self) -> bool {
        self.gates
            .iter()
            .all(|gate| gate.result == GateResult::Pass)
    }

    /// The overall reasons: `<gate> <reason>` per failed gate and `<gate>
    /// not exercised` per unexercised one.
    pub fn reasons(&self) -> Vec<String> {
        self.gates
            .iter()
            .filter_map(|gate| match gate.result {
                GateResult::Pass => None,
                GateResult::Fail => Some(format!(
                    "{} {}",
                    gate.name.phrase(),
                    gate.reason.unwrap_or("fail")
                )),
                GateResult::Unexercised => Some(format!("{} not exercised", gate.name.phrase())),
            })
            .collect()
    }

    #[cfg(test)]
    pub fn gate(&self, name: GateName) -> Option<&HostGate> {
        self.gates.iter().find(|gate| gate.name == name)
    }
}

fn gate(name: GateName, pass: bool, reason: &'static str, bound_reason: bool) -> HostGate {
    HostGate {
        name,
        result: if pass {
            GateResult::Pass
        } else {
            GateResult::Fail
        },
        reason: (!pass).then_some(reason),
        bound_reason,
    }
}

fn beacons_gate(report: &Report) -> Result<(HostGate, Option<BeaconFigures>), ReportError> {
    let window_us = report.verdict.number("window_us")?;
    let Some(stand) = report.stand() else {
        let reason = if report.survey.number("table_full")? > 0 {
            "stand-not-tracked"
        } else {
            "stand-absent"
        };
        return Ok((gate(GateName::Beacons, false, reason, true), None));
    };
    // The stand BSSID heard without the stand's SSID is another network.
    if stand.text("ssid_ok")? != "1" {
        return Ok((gate(GateName::Beacons, false, "stand-mismatch", true), None));
    }
    let beacons = stand
        .number("beacons")?
        .checked_sub(stand.number("dup")?)
        .ok_or_else(|| malformed("the stand's dup= exceeds its beacons="))?;
    let bi_us = stand.number("bi_tu")? * TU_US;
    let required = required_beacons(window_us, bi_us).ok();
    let figures = BeaconFigures {
        beacons,
        bi_us,
        window_us,
        required,
    };
    let gate = match required {
        None => gate(GateName::Beacons, false, "zero-interval", true),
        Some(required) => gate(GateName::Beacons, beacons >= required, "beacons", true),
    };
    Ok((gate, Some(figures)))
}

/// The walker restart, from the first hold cycle (`w1.hold cycle=0`): a
/// full hold (`full=1`: every staging lease held, or stuck one short with
/// the missing buffer at LAST), nothing received while it lasted (the
/// walker ran out), and frames after the release. How the ring recovered
/// (`w1.reload`, `w1.exhaust`) is reported, not gated.
fn walker_restart_gate(report: &Report) -> Result<HostGate, ReportError> {
    let hold = report
        .hold
        .first()
        .ok_or_else(|| malformed("no w1.hold cycle=0"))?;
    let full = hold.text("full")? == "1";
    let held_units = hold.number("units")?;
    let after = report.rx.number("frames_after_release")?;
    Ok(if !full {
        HostGate {
            name: GateName::WalkerRestart,
            result: GateResult::Unexercised,
            reason: Some("pool-not-exhausted"),
            bound_reason: true,
        }
    } else if held_units > 0 {
        gate(GateName::WalkerRestart, false, "received-while-held", true)
    } else {
        gate(
            GateName::WalkerRestart,
            after > 0,
            "no-frames-after-release",
            true,
        )
    })
}

/// The ownership experiment, both cycles (`w1.own`): a held lease whose
/// bytes changed (any `*_mismatch=`) or a chained drop in a release window
/// fails it, `foreign-write`; otherwise a cycle whose hold was not full
/// (`w1.hold full=0`) is unexercised, `pool-not-exhausted`, as is one whose
/// leases did not cover the ring (`first_drop=-`), `ring-order`.
fn ownership_gate(report: &Report) -> Result<HostGate, ReportError> {
    let mut written = false;
    let mut short = false;
    let mut unordered = false;
    for (own, hold) in report.own.iter().zip(&report.hold) {
        for key in [
            "hold_mismatch",
            "dwell_mismatch",
            "release_mismatch",
            "chained",
        ] {
            written |= own.number(key)? > 0;
        }
        short |= hold.text("full")? != "1";
        unordered |= own.number_or_dash("first_drop")?.is_none();
    }
    let unexercised = |reason| HostGate {
        name: GateName::Ownership,
        result: GateResult::Unexercised,
        reason: Some(reason),
        bound_reason: true,
    };
    Ok(if written {
        gate(GateName::Ownership, false, "foreign-write", true)
    } else if short {
        unexercised("pool-not-exhausted")
    } else if unordered {
        unexercised("ring-order")
    } else {
        gate(GateName::Ownership, true, "", true)
    })
}

/// The owner bit, both cycles (`w1.own`): every held lease's descriptor
/// carried the owner bit clear at the end of the hold (`owner_clear=` equals
/// `held=`); otherwise `owner-bit-set`.
fn owner_bit_gate(report: &Report) -> Result<HostGate, ReportError> {
    let mut clear = true;
    for own in &report.own {
        clear &= own.number("owner_clear")? == own.number("held")?;
    }
    Ok(gate(GateName::OwnerBit, clear, "owner-bit-set", false))
}

/// Whether a `w1.hold` line's drain end shows a hold stuck at LAST in a
/// ring of `count`: `slots − 1` buffers staged and detached, at most one
/// released, every descriptor observed and NEXT zero. (That LAST is the
/// descriptor without a lease only the image can tell.)
fn stuck_evidence(hold: &Fields, count: u64) -> Result<bool, ReportError> {
    let Some(one_short) = hold.number("slots")?.checked_sub(1) else {
        return Ok(false);
    };
    let next_zero = hold
        .text("end_next")?
        .strip_prefix("0x")
        .is_some_and(|digits| u32::from_str_radix(digits, 16) == Ok(0));
    Ok(hold.number_or_dash("end_staged")? == Some(one_short)
        && hold.number_or_dash("end_detached")? == Some(one_short)
        && hold
            .number_or_dash("end_released")?
            .is_some_and(|released| released <= 1)
        && hold.number_or_dash("end_observed")? == Some(count)
        && next_zero)
}

/// The ring's descriptor count, from `w1.config arena=<count>x<buffer>`.
fn ring_count(report: &Report) -> Result<u64, ReportError> {
    report
        .config
        .text("arena")?
        .split_once('x')
        .and_then(|(count, _)| decimal(count))
        .filter(|&count| count > 0)
        .ok_or_else(|| malformed("w1.config arena= is no <count>x<buffer>"))
}

/// The descriptor indices of a `first_after_release=` list (`None` for
/// `x`); empty for `-`.
fn indices(list: &str) -> Vec<Option<u64>> {
    if list == "-" {
        return Vec::new();
    }
    list.split(',').map(decimal).collect()
}

/// The descriptors skipped between consecutive known indices that move
/// forward by at most half a ring of `count`.
fn skipped(indices: &[Option<u64>], count: u64) -> u64 {
    if count == 0 {
        return 0;
    }
    indices
        .windows(2)
        .filter_map(|pair| match pair {
            [Some(first), Some(second)] => {
                let step = (second + count - first % count) % count;
                (step >= 2 && step <= count / 2).then(|| step - 1)
            }
            _ => None,
        })
        .sum()
}

/// Consecutive frames whose descriptors are not consecutive in a ring of
/// `count` (an unknown index breaks both of its pairs).
fn order_breaks(indices: &[Option<u64>], count: u64) -> u64 {
    indices
        .windows(2)
        .filter(|pair| match pair {
            [Some(first), Some(second)] => *second != (first + 1) % count,
            _ => true,
        })
        .count() as u64
}

/// No timer wait of the image returned before its deadline, on either
/// executor, in any phase (`w1.timer` and `w1.wake`).
fn timer_wake_gate(report: &Report) -> Result<HostGate, ReportError> {
    let mut early = 0_u64;
    for line in report.timer.iter().chain(&report.wake) {
        early = early.saturating_add(line.number("woke_early")?);
    }
    Ok(gate(GateName::TimerWake, early == 0, "woke-early", true))
}

/// Recompute every gate of `report`; the `channel` gate also requires the
/// applied channel to be `expected`'s.
pub fn recompute(report: &Report, expected: &Config) -> Result<Host, ReportError> {
    let (beacons, figures) = beacons_gate(report)?;
    let applied = report.config.number("applied_channel")?;
    let channel = report.config.number("channel")?;
    let unserviced = report
        .rx
        .number("unserviced_mac")?
        .checked_add(report.rx.number("unserviced_pwr")?)
        .ok_or_else(|| malformed("unserviced counts overflow"))?;
    let gates = vec![
        beacons,
        gate(
            GateName::Channel,
            applied == channel && applied == u64::from(expected.channel),
            "channel-mismatch",
            false,
        ),
        gate(
            GateName::Exit,
            report.exit.text("cause")? == "StopRequested" && report.exit.text("end")? == "Halted",
            "not-stopped",
            false,
        ),
        walker_restart_gate(report)?,
        ownership_gate(report)?,
        gate(
            GateName::IrqStack,
            report.stack.number("irq_free")? >= MIN_IRQ_STACK_FREE,
            "irq-stack-low",
            false,
        ),
        gate(
            GateName::ServiceErrors,
            report.rx.number("service_errors")? == 0,
            "service-errors",
            false,
        ),
        gate(
            GateName::Unserviced,
            unserviced == 0,
            "unserviced-interrupts",
            false,
        ),
        timer_wake_gate(report)?,
        gate(
            GateName::BaseWrites,
            report.reload.number("base_writes")? == 1,
            "base-writes",
            false,
        ),
        gate(
            GateName::Reserved,
            report.reload.number("reserved_landings")? <= 1
                && report.reload.number("reserved_late_landings")? == 0,
            "reserved-landing",
            false,
        ),
        owner_bit_gate(report)?,
        gate(
            GateName::WalkerOnDetached,
            report.reload.number("walker_on_detached")? == 0,
            "walker-on-detached",
            false,
        ),
    ];
    Ok(Host {
        gates,
        beacons: figures,
    })
}

/// The verdict line an image printed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImageResult {
    Pass { beacons: u64, required: u64 },
    Fail { reasons: Vec<String> },
}

/// Read `OER-RESULT: pass w1 beacons=<N> required=<R>` or `OER-RESULT:
/// fail w1 <reason>[; <reason>…]`.
pub fn parse_result(line: &str) -> Result<ImageResult, ReportError> {
    let rest = line
        .find(RESULT_MARKER)
        .map(|at| &line[at + RESULT_MARKER.len()..])
        .ok_or_else(|| malformed("no OER-RESULT: line"))?;
    let tokens = rest.split_whitespace().collect::<Vec<_>>();
    match tokens.as_slice() {
        ["pass", IMAGE, beacons, required] => {
            let beacons = beacons.strip_prefix("beacons=").and_then(decimal);
            let required = required.strip_prefix("required=").and_then(decimal);
            match (beacons, required) {
                (Some(beacons), Some(required)) => Ok(ImageResult::Pass { beacons, required }),
                _ => Err(malformed(
                    "OER-RESULT: pass w1 needs beacons=<N> required=<R>",
                )),
            }
        }
        ["fail", IMAGE, _, ..] => {
            let reasons = rest
                .trim()
                .strip_prefix("fail")
                .and_then(|rest| rest.trim_start().strip_prefix(IMAGE))
                .unwrap_or_default()
                .split(';')
                .map(|reason| reason.split_whitespace().collect::<Vec<_>>().join(" "))
                .collect::<Vec<_>>();
            if reasons.iter().any(String::is_empty) {
                return Err(malformed("an empty reason in OER-RESULT: fail w1"));
            }
            Ok(ImageResult::Fail { reasons })
        }
        _ => Err(malformed(
            "the OER-RESULT: line is neither `pass w1 beacons= required=` nor `fail w1 <reasons>`",
        )),
    }
}

/// A gap that overlaps a tracking tick.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Overlap {
    pub gap: usize,
    pub tick: usize,
}

/// The gaps that overlap a tracking tick. A gap spans `[stamp − gap_us,
/// stamp]` on the frame stamp clock, which `offset` (wrapping) maps onto
/// the ticks' clock; both are wrapping `u32` microseconds.
pub fn correlate(gaps: &[Gap], ticks: &[Tick], offset: u32) -> Vec<Overlap> {
    let mut overlaps = Vec::new();
    for (index, gap) in gaps.iter().enumerate() {
        let gap_start = gap.stamp.wrapping_add(offset).wrapping_sub(gap.gap_us);
        for (tick_index, tick) in ticks.iter().enumerate() {
            let tick_length = tick.end.wrapping_sub(tick.start);
            if tick.start.wrapping_sub(gap_start) <= gap.gap_us
                || gap_start.wrapping_sub(tick.start) <= tick_length
            {
                overlaps.push(Overlap {
                    gap: index,
                    tick: tick_index,
                });
            }
        }
    }
    overlaps
}

/// The outcome of a `w1` run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Outcome {
    /// The image and the host agree on a pass.
    Pass,
    /// The image and the host agree on a fail, for the host's reasons.
    Fail(Vec<String>),
    /// The image and the host disagree.
    Contradiction(Vec<String>),
    /// The block is missing or malformed.
    Unreadable(ReportError),
}

impl Outcome {
    /// 0 pass, 1 fail, 3 a contradiction or an unreadable block (2, no
    /// verdict at all, is the caller's).
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Pass => 0,
            Self::Fail(_) => 1,
            Self::Contradiction(_) | Self::Unreadable(_) => 3,
        }
    }
}

/// A checked run: its outcome and the host's summary lines (BSSIDs masked).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Checked {
    pub outcome: Outcome,
    pub summary: Vec<String>,
}

fn disagreements(
    report: &Report,
    host: &Host,
    result: &ImageResult,
    expected: &Config,
) -> Result<Vec<String>, ReportError> {
    let mut problems = Vec::new();
    let tolerance = narrow_u32(report.lagfit.number("tolerance_us")?)?;
    let fit = lag_fit(&report.lags, tolerance);
    let image_fit = (
        report.lagfit.number_or_dash("median_us")?,
        report.lagfit.number_or_dash("max_us")?,
        report.lagfit.number("next_arrival_matches")?,
    );
    let host_fit = (
        fit.map(|fit| u64::from(fit.median_us)),
        fit.map(|fit| u64::from(fit.max_us)),
        fit.map_or(0, |fit| fit.next_arrival_matches),
    );
    if image_fit != host_fit {
        problems.push(format!(
            "w1.lagfit median/max/next_arrival_matches {image_fit:?}, the host computes {host_fit:?}"
        ));
    }
    let stand_index = report
        .bss
        .iter()
        .find(|bss| bss.text("who").is_ok_and(|who| who == "stand"))
        .map(|bss| bss.number("idx"))
        .transpose()?;
    let tsf_bss = report.tsffit.number_or_dash("bss")?;
    if tsf_bss != stand_index {
        problems.push(format!(
            "w1.tsffit bss={tsf_bss:?}, the stand's index is {stand_index:?}"
        ));
    }
    let tsf_fit = tsf_fit(&report.tsf);
    let image_tsf = (
        report.tsffit.number_or_dash("median_us")?,
        report.tsffit.number_or_dash("p90_us")?,
        report.tsffit.number_or_dash("max_us")?,
        report.tsffit.signed_or_dash("slope_ppm")?,
    );
    let host_tsf = (
        tsf_fit.map(|fit| u64::from(fit.median_us)),
        tsf_fit.map(|fit| u64::from(fit.p90_us)),
        tsf_fit.map(|fit| u64::from(fit.max_us)),
        tsf_fit.map(|fit| i64::from(fit.slope_ppm)),
    );
    if image_tsf != host_tsf {
        problems.push(format!(
            "w1.tsffit median/p90/max/slope {image_tsf:?}, the host computes {host_tsf:?}"
        ));
    }
    let count = ring_count(report)?;
    for hold in &report.hold {
        if hold.text("reason")? == "stuck-at-last" && !stuck_evidence(hold, count)? {
            problems.push(format!(
                "w1.hold cycle={} reason=stuck-at-last, but its end_*= do not show a hold stuck at LAST",
                hold.text("cycle")?
            ));
        }
    }
    for own in &report.own {
        let image = own.number("order_breaks")?;
        let host = order_breaks(&indices(own.text("first_after_release")?), count);
        if image != host {
            problems.push(format!(
                "w1.own cycle={} order_breaks={image}, the host counts {host}",
                own.text("cycle")?
            ));
        }
    }
    let channel = report.config.number("channel")?;
    if channel != u64::from(expected.channel) {
        problems.push(format!(
            "the image was built for channel {channel}, the router's AP is on {} (a stale build?)",
            expected.channel
        ));
    }
    let stand = report.config.text("stand")?;
    if stand != expected.bssid.masked() {
        problems.push(format!(
            "the image expects {stand}, the router's AP is {} (a stale build?)",
            expected.bssid.masked()
        ));
    }
    let mut carrying = 0_u64;
    for bss in &report.bss {
        carrying += u64::from(bss.text("ssid_ok")? == "1");
    }
    let matches = report.config.number("ssid_matches")?;
    if matches != carrying {
        problems.push(format!(
            "w1.config ssid_matches={matches}, the w1.bss lines carry ssid_ok=1 {carrying} times"
        ));
    }
    for (image, host) in report.gates.iter().zip(&host.gates) {
        if image.result != host.result {
            problems.push(format!(
                "gate {}: the image says {}, the host {}",
                image.name.as_str(),
                image.result.as_str(),
                host.result.as_str()
            ));
        } else if host.bound_reason && image.reason.as_deref() != host.reason {
            problems.push(format!(
                "gate {}: the image's reason is {}, the host's {}",
                image.name.as_str(),
                image.reason.as_deref().unwrap_or("-"),
                host.reason.unwrap_or("-")
            ));
        }
    }
    if let Some(figures) = host.beacons {
        let verdict = &report.verdict;
        let mut compare = |name: &str, host: u64| -> Result<(), ReportError> {
            let image = verdict.number(name)?;
            if image != host {
                problems.push(format!("w1.verdict {name}={image}, the host counts {host}"));
            }
            Ok(())
        };
        compare("beacons", figures.beacons)?;
        compare("bi_us", figures.bi_us)?;
        if let Some(required) = figures.required {
            compare("required", required)?;
        }
    }
    match result {
        ImageResult::Pass { beacons, required } => {
            if !host.passed() {
                problems.push(format!(
                    "the image passes, the host fails: {}",
                    host.reasons().join("; ")
                ));
            } else if let Some(figures) = host.beacons
                && (Some(*required) != figures.required || *beacons != figures.beacons)
            {
                problems.push(format!(
                    "OER-RESULT: beacons={beacons} required={required}, the host counts beacons={} required={}",
                    figures.beacons,
                    figures.required.map_or_else(|| "-".to_owned(), |r| r.to_string())
                ));
            }
        }
        ImageResult::Fail { reasons } => {
            if host.passed() {
                problems.push(format!(
                    "the image fails ({}), the host passes",
                    reasons.join("; ")
                ));
            } else {
                // Every failed gate is one reason that starts with its name;
                // an unexercised one reads `<gate> not exercised`.
                let mut expected_gates = host
                    .gates
                    .iter()
                    .filter(|gate| gate.result != GateResult::Pass)
                    .collect::<Vec<_>>();
                expected_gates.sort_by_key(|gate| gate.name);
                let mut named = Vec::new();
                for reason in reasons {
                    let first = reason.split(' ').next().unwrap_or_default();
                    let Some(gate) = host.gates.iter().find(|gate| gate.name.phrase() == first)
                    else {
                        problems.push(format!("OER-RESULT: reason `{reason}` names no gate"));
                        continue;
                    };
                    if gate.result == GateResult::Unexercised
                        && *reason != format!("{} not exercised", gate.name.phrase())
                    {
                        problems.push(format!(
                            "OER-RESULT: reason `{reason}`, the host's `{} not exercised`",
                            gate.name.phrase()
                        ));
                    }
                    named.push(gate);
                }
                named.sort_by_key(|gate| gate.name);
                if named != expected_gates {
                    problems.push(format!(
                        "OER-RESULT: fail w1 {}, the host's reasons are {}",
                        reasons.join("; "),
                        host.reasons().join("; ")
                    ));
                }
            }
        }
    }
    Ok(problems)
}

fn narrow_u32(value: u64) -> Result<u32, ReportError> {
    u32::try_from(value).map_err(|_| malformed("value exceeds u32"))
}

fn summary(report: &Report, host: &Host, expected: &Config) -> Vec<String> {
    let mut lines = vec![format!(
        "expects {} on channel {}; the image applied channel {}; {} BSS carry the stand's SSID",
        expected.bssid.masked(),
        expected.channel,
        report.config.text("applied_channel").unwrap_or("-"),
        report.config.text("ssid_matches").unwrap_or("-")
    )];
    match (report.stand(), host.beacons) {
        (None, _) => lines.push("the stand AP was not heard".to_owned()),
        (Some(_), None) => {
            lines.push("the stand AP's BSSID was heard without the stand's SSID".to_owned())
        }
        (Some(_), Some(figures)) => lines.push(format!(
            "stand beacons {} of {} required in {} us at {} us intervals",
            figures.beacons,
            figures
                .required
                .map_or_else(|| "-".to_owned(), |required| required.to_string()),
            figures.window_us,
            figures.bi_us
        )),
    }
    for (host_gate, image_gate) in host.gates.iter().zip(&report.gates) {
        lines.push(format!(
            "gate {:<14} host {}{} image {}{}",
            host_gate.name.as_str(),
            host_gate.result.as_str(),
            host_gate
                .reason
                .map_or_else(String::new, |reason| format!(" ({reason})")),
            image_gate.result.as_str(),
            image_gate
                .reason
                .as_ref()
                .map_or_else(String::new, |reason| format!(" ({reason})")),
        ));
    }
    let count = ring_count(report).unwrap_or(0);
    for ((own, exhaust), hold) in report.own.iter().zip(&report.exhaust).zip(&report.hold) {
        let text = |fields: &Fields, key: &str| fields.text(key).unwrap_or("?").to_owned();
        let skipped = own
            .text("first_after_release")
            .map_or(0, |list| skipped(&indices(list), count));
        let drops = own
            .text("discards")
            .ok()
            .filter(|list| *list != "-")
            .and_then(discards)
            .map_or(0, |pairs| {
                pairs
                    .iter()
                    .filter(|(reason, _)| RUNNER_DROPS.contains(reason))
                    .map(|(_, count)| count)
                    .sum::<u64>()
            });
        lines.push(format!(
            "hold cycle {}: full {} ({}); {skipped} descriptor(s) skipped in the frames after the release, {drops} runner drop(s) in the window ({}){}",
            text(own, "cycle"),
            text(hold, "full"),
            text(hold, "reason"),
            text(own, "discards"),
            if skipped > drops {
                format!("; {} skip(s) not counted by any drop", skipped - drops)
            } else {
                String::new()
            },
        ));
        lines.push(format!(
            "hold cycle {} {}: {} held, first drop {}; changed at hold/dwell/release {}/{}/{} (first {}); after the release {} ({} order breaks); chained {} oversized {}; snapshot recycle_start {} accepted_tail {} BASE {}, doorbell read NEXT {} LAST {}, descriptors changed {}",
            text(own, "cycle"),
            text(own, "order"),
            text(own, "held"),
            text(own, "first_drop"),
            text(own, "hold_mismatch"),
            text(own, "dwell_mismatch"),
            text(own, "release_mismatch"),
            text(own, "first_mismatch"),
            text(own, "first_after_release"),
            text(own, "order_breaks"),
            text(own, "chained"),
            text(own, "oversized"),
            text(exhaust, "recycle_start"),
            text(exhaust, "accepted_tail"),
            text(exhaust, "base"),
            text(exhaust, "next_after_doorbell"),
            text(exhaust, "last_after_doorbell"),
            text(exhaust, "desc_hash_changed"),
        ));
    }
    let offset = report
        .stand()
        .and_then(|stand| stand.range_min("lag_us").ok().flatten())
        .and_then(|lag| u32::try_from(lag).ok());
    match offset {
        Some(offset) => {
            let overlaps = correlate(&report.gaps, &report.ticks, offset);
            lines.push(format!(
                "{} of {} gaps ({} dropped) overlap one of {} tracking ticks (offset {offset} us)",
                overlaps
                    .iter()
                    .map(|overlap| overlap.gap)
                    .collect::<std::collections::BTreeSet<_>>()
                    .len(),
                report.gaps.len(),
                report.gaps_dropped,
                report.ticks.len()
            ));
            for overlap in overlaps {
                if let (Some(gap), Some(tick)) =
                    (report.gaps.get(overlap.gap), report.ticks.get(overlap.tick))
                {
                    lines.push(format!(
                        "  gap {} us ending {} overlaps tick {}..{} ok={}",
                        gap.gap_us,
                        gap.stamp,
                        tick.start,
                        tick.end,
                        u8::from(tick.ok)
                    ));
                }
            }
        }
        None => lines.push("no stand handoff lag: gaps not correlated with ticks".to_owned()),
    }
    let tolerance = report
        .lagfit
        .number("tolerance_us")
        .ok()
        .and_then(|tolerance| u32::try_from(tolerance).ok());
    match tolerance.and_then(|tolerance| lag_fit(&report.lags, tolerance).map(|fit| (tolerance, fit))) {
        Some((tolerance, fit)) => lines.push(format!(
            "handoff delay over {} window units: median {} us, max {} us; {} handed off within {tolerance} us after the next unit arrived",
            report.lags.len(),
            fit.median_us,
            fit.max_us,
            fit.next_arrival_matches
        )),
        None => lines.push("no window unit: no handoff delay".to_owned()),
    }
    match tsf_fit(&report.tsf) {
        Some(fit) => lines.push(format!(
            "stand beacon handoff against TSF over {} beacons, trend removed: median {} us, p90 {} us, max {} us above the earliest; slope {} ppm",
            report.tsf.len(),
            fit.median_us,
            fit.p90_us,
            fit.max_us,
            fit.slope_ppm
        )),
        None => lines.push("fewer than two stand beacons: no TSF fit".to_owned()),
    }
    lines
}

/// Check a `w1` run from its console `lines` and its `result_line`.
pub fn check<S: AsRef<str>>(lines: &[S], result_line: &str, expected: &Config) -> Checked {
    let unreadable = |error| Checked {
        outcome: Outcome::Unreadable(error),
        summary: Vec::new(),
    };
    let report = match Report::parse(lines) {
        Ok(report) => report,
        Err(error) => return unreadable(error),
    };
    let result = match parse_result(result_line) {
        Ok(result) => result,
        Err(error) => return unreadable(error),
    };
    let host = match recompute(&report, expected) {
        Ok(host) => host,
        Err(error) => return unreadable(error),
    };
    let problems = match disagreements(&report, &host, &result, expected) {
        Ok(problems) => problems,
        Err(error) => return unreadable(error),
    };
    let summary = summary(&report, &host, expected);
    let outcome = if !problems.is_empty() {
        Outcome::Contradiction(problems)
    } else if host.passed() {
        Outcome::Pass
    } else {
        Outcome::Fail(host.reasons())
    };
    Checked { outcome, summary }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SSID: &str = "Made Up 'Lab' Net";

    fn expected() -> Config {
        Config {
            channel: 6,
            bssid: Mac::new([0x02, 0, 0, 0, 0xF6, 0xD0]).unwrap(),
            ssid: Ssid::new(SSID.as_bytes()).unwrap(),
            country: *b"DE",
        }
    }

    fn body() -> Vec<String> {
        [
            "w1.config channel=6 applied_channel=6 stand=xx:xx:xx:xx:F6:D0 ssid_matches=1 window_ms=10000 cpu_hz=160000000 arena=32x1664 slots=32 mask=0x19a879e0",
            "w1.bss idx=0 who=stand bssid=xx:xx:xx:xx:F6:D0 bi_tu=100 ch=6 beacons=98 dup=1 probe_rsp=0 rssi=-61..-40 no_rssi=0 gaps_late=2 max_gap_us=310000 missed=2 lag_us=120..900 ssid_len=8 ssid_ok=1 stamp_span_us=9932800 handoff_span_us=9932810",
            "w1.bss idx=1 who=other bssid=xx:xx:xx:xx:12:34 bi_tu=100 ch=- beacons=40 dup=0 probe_rsp=1 rssi=- no_rssi=40 gaps_late=0 max_gap_us=0 missed=0 lag_us=- ssid_len=0 ssid_ok=0 stamp_span_us=- handoff_span_us=-",
            "w1.gap stamp=1000000 gap_us=204800 missed=1",
            "w1.gap stamp=4294967000 gap_us=307200 missed=-",
            "w1.gaps dropped=0",
            "w1.tick start=900000 end=900200 ok=1",
            "w1.rx units=500 bytes=90000 mac_irqs=480 power_irqs=3 unserviced_mac=0 unserviced_pwr=0 doorbell_unmasks=470 ignored_unmasks=0 chained=0 oversized=0 empty=0 too_long=0 overload=0 admission_blocks=0 stage_capacity_blocks=0 service_passes=480 service_errors=0 capacity_drains=1 frames_after_release=12",
            "w1.hold cycle=0 fill_ms=5324 reached=32 slots=32 full=1 reason=all units=0 mac_irqs=0 stage_capacity_blocks=1 admission_blocks=0 end_released=0 end_detached=32 end_staged=32 end_observed=32 end_recycle_start=4 end_accepted_tail=3 end_next=0x0 end_last=0x17000",
            "w1.hold cycle=1 fill_ms=5324 reached=32 slots=32 full=1 reason=all units=0 mac_irqs=0 stage_capacity_blocks=1 admission_blocks=0 end_released=- end_detached=- end_staged=- end_observed=- end_recycle_start=- end_accepted_tail=- end_next=- end_last=-",
            "w1.exhaust cycle=0 snapshots=1 blocked_us=1000100 next_low=0x17000 next_upper=0 last_low=0x16fe0 base=0x2f017000 walker=1 doorbell=0 buffer_full=3 ring_base=0x2f017000 recycle_start=4 accepted_tail=3 observed=32 completed=0 remaining=32 reload_pending=0 released=1 detached=31 staged=31 next_after_doorbell=0x17030 last_after_doorbell=- desc_hash_changed=0",
            "w1.exhaust cycle=1 snapshots=9 blocked_us=1000200 next_low=0x0 next_upper=0 last_low=0x17024 base=0x2f017000 walker=1 doorbell=0 buffer_full=3 ring_base=0x2f017000 recycle_start=4 accepted_tail=3 observed=32 completed=0 remaining=- reload_pending=0 released=1 detached=31 staged=31 next_after_doorbell=- last_after_doorbell=- desc_hash_changed=0",
            "w1.own cycle=0 order=forward held=32 owner_clear=32 first_drop=4 hold_mismatch=0 dwell_mismatch=0 release_mismatch=0 first_mismatch=- first_after_release=4,5,6,7,8 order_breaks=0 chained=0 oversized=0 discards=-",
            "w1.own cycle=1 order=reverse held=32 owner_clear=32 first_drop=3 hold_mismatch=0 dwell_mismatch=0 release_mismatch=0 first_mismatch=- first_after_release=4,5,7,x,31,0 order_breaks=3 chained=0 oversized=1 discards=oversized:1,rx_failure:2",
            "w1.prefix fcs=0 strict_rxend_refusable=0 strict_dump_refusable=0 admitted=500",
            "w1.reload max_attempts=3 limit_us=20 observations=2 reload_rerings=0 rering_failures=0 base_writes=1 reserved_landings=1 reserved_late_landings=0 walker_on_detached=0",
            "w1.continuation stale_doorbell=2 longest_streak=1",
            "w1.teardown mask_confirm_us=4 drain_us=30 drain_polls=2",
            "w1.exit cause=StopRequested end=Halted",
            "w1.p2 phase=window swi2_max_us=20 swi2_p99_us=8 actor_max_us=40 actor_p99_us=10 actor_over_bound=0 receive_max_us=30 receive_p99_us=9 mac_to_poll_max_us=50 ring_to_doorbell_max_us=12",
            "w1.clock op=acquire clock=WifiMac phase=window count=1 max_us=3",
            "w1.stack irq_bytes=32768 irq_free=20000 min_free=4096",
            "w1.timer cmp=0 phase=stress early=0 late_count=1 late_max_us=4 late_total_us=4 missed=0 unmatched=0 coalesced=0 overlapping=0 armed_at_end=0 waits=135000 woke_early=0",
            "w1.wake cmp=1 phase=hold waits=3 woke_early=0",
            "w1.lag i=0 stamp=1000000 lag_us=51700 since_prev_us=- until_next_us=1500",
            "w1.lag i=1 stamp=1001500 lag_us=151200 since_prev_us=1500 until_next_us=100900",
            "w1.lag i=2 stamp=1102400 lag_us=50000 since_prev_us=100900 until_next_us=-",
            "w1.lagfit n=3 median_us=1700 max_us=101200 next_arrival_matches=2 tolerance_us=2000",
            "w1.tsf i=0 tsf=0 handoff=5000000",
            "w1.tsf i=1 tsf=102400 handoff=5102410",
            "w1.tsf i=2 tsf=204800 handoff=5205800",
            "w1.tsf i=3 tsf=307200 handoff=5307200",
            "w1.tsffit n=4 bss=0 median_us=208 p90_us=1099 max_us=1099 slope_ppm=966",
            "w1.survey frames=540 header_refusals=0 data=0 ignored_management=0 table_full=0 off_channel=0",
            "w1.verdict beacons=97 bi_us=102400 window_us=10000000 required=78",
            "w1.gate name=beacons result=pass",
            "w1.gate name=channel result=pass",
            "w1.gate name=exit result=pass",
            "w1.gate name=walker_restart result=pass",
            "w1.gate name=ownership result=pass",
            "w1.gate name=irq_stack result=pass",
            "w1.gate name=service_errors result=pass",
            "w1.gate name=unserviced result=pass",
            "w1.gate name=timer_wake result=pass",
            "w1.gate name=base_writes result=pass",
            "w1.gate name=reserved result=pass",
            "w1.gate name=owner_bit result=pass",
            "w1.gate name=walker_on_detached result=pass",
        ]
        .map(str::to_owned)
        .to_vec()
    }

    /// `body` framed as a console: a line before, BEGIN, END with its count.
    fn console(body: &[String]) -> Vec<String> {
        let mut lines = vec!["OER w1 start".to_owned(), "OER-W1-BEGIN v=1".to_owned()];
        lines.extend_from_slice(body);
        lines.push(format!("OER-W1-END lines={}", body.len()));
        lines
    }

    /// Set `key=value` on the first line starting with `prefix`.
    fn set(body: &mut [String], prefix: &str, key: &str, value: &str) {
        let line = body
            .iter_mut()
            .find(|line| line.starts_with(prefix))
            .expect("the fixture has the line");
        let tokens = line
            .split_whitespace()
            .map(|token| match token.split_once('=') {
                Some((name, _)) if name == key => format!("{key}={value}"),
                _ => token.to_owned(),
            })
            .collect::<Vec<_>>();
        *line = tokens.join(" ");
    }

    fn set_gate(body: &mut [String], name: &str, tail: &str) {
        let prefix = format!("w1.gate name={name} ");
        let line = body
            .iter_mut()
            .find(|line| line.starts_with(&prefix))
            .expect("the fixture has the gate");
        *line = format!("{prefix}{tail}");
    }

    const PASS: &str = "OER-RESULT: pass w1 beacons=97 required=78";

    fn host_of(body: &[String]) -> Host {
        recompute(&Report::parse(&console(body)).unwrap(), &expected()).unwrap()
    }

    fn host_gate(body: &[String], name: GateName) -> HostGate {
        *host_of(body).gate(name).unwrap()
    }

    #[test]
    fn an_ssid_is_1_to_32_bytes_and_never_shown() {
        assert!(Ssid::new(b"").is_err());
        assert!(Ssid::new(&[b'x'; 33]).is_err());
        let ssid = Ssid::new(b"Made Up Lab Net").unwrap();
        assert_eq!(format!("{ssid:?}"), "Ssid(<15 bytes>)");
    }

    #[test]
    fn the_runtime_build_gets_the_full_bssid_the_ssid_and_the_channel() {
        assert_eq!(
            expected().runtime_env(),
            [
                (ENV_CHANNEL, "6".to_owned()),
                (ENV_BSSID, "02:00:00:00:F6:D0".to_owned()),
                (ENV_SSID, "4d61646520557020274c616227204e6574".to_owned()),
                (ENV_COUNTRY, "DE".to_owned()),
            ]
        );
    }

    #[test]
    fn required_beacons_is_80_percent_floored() {
        assert_eq!(required_beacons(10_000_000, 100 * TU_US), Ok(78));
        assert_eq!(required_beacons(0, 102_400), Ok(0));
        assert_eq!(
            required_beacons(10_000_000, 0),
            Err(RequiredError::ZeroInterval)
        );
        assert_eq!(required_beacons(u64::MAX, 1), Ok(u64::MAX / 10 * 8 + 4));
        assert_eq!(required_beacons(u64::MAX, u64::MAX), Ok(0));
        assert_eq!(required_beacons(1, u64::MAX), Ok(0));
    }

    #[test]
    fn a_good_block_parses_and_passes() {
        let report = Report::parse(&console(&body())).unwrap();
        assert_eq!(report.bss.len(), 2);
        assert_eq!(report.gaps.len(), 2);
        assert_eq!(report.gaps[1].missed, None);
        assert_eq!(
            report.ticks,
            [Tick {
                start: 900_000,
                end: 900_200,
                ok: true
            }]
        );
        assert_eq!(report.config.text("stand"), Ok("xx:xx:xx:xx:F6:D0"));
        let checked = check(&console(&body()), PASS, &expected());
        assert_eq!(checked.outcome, Outcome::Pass, "{:?}", checked.summary);
        assert_eq!(checked.outcome.exit_code(), 0);
        assert!(
            checked
                .summary
                .iter()
                .all(|line| !line.contains("02:00:00:00")),
            "{:?}",
            checked.summary
        );
    }

    #[test]
    fn a_count_over_the_bound_is_a_number_or_a_bucket_range() {
        for value in ["3", "2..5", "0..0"] {
            let mut lines = body();
            set(&mut lines, "w1.p2", "actor_over_bound", value);
            let report = Report::parse(&console(&lines)).unwrap();
            assert_eq!(report.p2[0].text("actor_over_bound"), Ok(value));
        }
    }

    #[test]
    fn a_missing_or_unended_block_is_unreadable() {
        let lines = ["OER w1 start", PASS];
        assert_eq!(Report::parse(&lines), Err(ReportError::Missing));
        let checked = check(&lines, PASS, &expected());
        assert_eq!(checked.outcome.exit_code(), 3);

        let mut lines = console(&body());
        lines.pop();
        let error = Report::parse(&lines).unwrap_err();
        assert!(error.to_string().contains(END), "{error}");
        assert_eq!(check(&lines, PASS, &expected()).outcome.exit_code(), 3);

        let mut twice = console(&body());
        twice.extend(console(&body()));
        assert!(Report::parse(&twice).is_err());

        let mut version = console(&body());
        version[1] = "OER-W1-BEGIN v=2".to_owned();
        assert!(Report::parse(&version).is_err());
    }

    #[test]
    fn the_line_count_must_match() {
        let mut lines = console(&body());
        let last = lines.len() - 1;
        lines[last] = format!("OER-W1-END lines={}", body().len() + 1);
        let error = Report::parse(&lines).unwrap_err();
        assert!(error.to_string().contains("counts"), "{error}");
        lines[last] = "OER-W1-END".to_owned();
        assert!(Report::parse(&lines).is_err());
        lines[last] = "OER-W1-END lines=x".to_owned();
        assert!(Report::parse(&lines).is_err());
    }

    #[test]
    fn garbage_is_an_error_never_a_panic() {
        let garbage = [
            ("w1.rx", "units", "-1"),
            ("w1.rx", "units", "18446744073709551616"),
            ("w1.rx", "units", ""),
            ("w1.bss idx=0", "bi_tu", "65536"),
            ("w1.bss idx=0", "who", "maybe"),
            ("w1.bss idx=0", "rssi", "-40..-61"),
            ("w1.bss idx=0", "rssi", "x..y"),
            ("w1.bss idx=0", "lag_us", "5"),
            ("w1.bss idx=0", "bssid", "02:00:00:00:F6:D0"),
            ("w1.config", "mask", "19a879e0"),
            ("w1.config", "channel", "256"),
            ("w1.gap stamp=1000000", "stamp", "4294967296"),
            ("w1.tick", "ok", "2"),
            ("w1.p2", "actor_over_bound", "5..2"),
            ("w1.p2", "actor_over_bound", "-"),
            ("w1.p2", "actor_over_bound", "2.."),
            ("w1.gate name=beacons", "result", "maybe"),
            ("w1.gate name=beacons", "name", "nothing"),
            ("w1.hold", "reached", "-"),
            ("w1.hold", "units", "x"),
            ("w1.exhaust", "next_low", "17000"),
            ("w1.exhaust", "base", "0x"),
            ("w1.exhaust", "base", "0x100000000"),
            ("w1.exhaust", "next_upper", "2"),
            ("w1.exhaust", "buffer_full", "65536"),
            ("w1.exhaust", "snapshots", "-"),
            ("w1.wake", "phase", "report"),
            ("w1.wake", "cmp", "2"),
            ("w1.timer", "woke_early", "-1"),
            ("w1.bss idx=0", "stamp_span_us", "4294967296"),
            ("w1.bss idx=0", "handoff_span_us", "x"),
        ];
        for (prefix, key, value) in garbage {
            let mut lines = body();
            set(&mut lines, prefix, key, value);
            let result = Report::parse(&console(&lines));
            assert!(
                matches!(result, Err(ReportError::Malformed(_))),
                "{prefix} {key}={value}: {result:?}"
            );
        }
        // An unmasked address is never repeated in the error.
        let mut lines = body();
        set(&mut lines, "w1.bss idx=0", "bssid", "02:AB:CD:EF:F6:D0");
        let error = Report::parse(&console(&lines)).unwrap_err().to_string();
        assert!(!error.contains("AB:CD"), "{error}");

        for broken in [
            "w1.unknown x=1",
            "w1.rx units",
            "w1.rx =1",
            "w1.gaps dropped=1 dropped=1",
            "w1.gaps",
            "",
            "\u{fffd}\u{fffd}",
            "w1.gaps dropped=0 extra=1",
        ] {
            let mut lines = body();
            lines.push(broken.to_owned());
            assert!(Report::parse(&console(&lines)).is_err(), "{broken}");
        }
        // A missing, a doubled or a reasonless gate.
        let mut lines = body();
        lines.retain(|line| !line.starts_with("w1.gate name=exit"));
        assert!(Report::parse(&console(&lines)).is_err());
        let mut lines = body();
        lines.push("w1.gate name=exit result=pass".to_owned());
        assert!(Report::parse(&console(&lines)).is_err());
        let mut lines = body();
        set_gate(&mut lines, "exit", "result=fail");
        assert!(Report::parse(&console(&lines)).is_err());
        let mut lines = body();
        set_gate(&mut lines, "exit", "result=pass reason=x");
        assert!(Report::parse(&console(&lines)).is_err());
        // Two stand entries, too many gaps.
        let mut lines = body();
        let stand = lines[1].replace("idx=0", "idx=2");
        lines.insert(2, stand);
        assert!(Report::parse(&console(&lines)).is_err());
        let mut lines = body();
        for _ in 0..MAX_GAPS {
            lines.insert(3, "w1.gap stamp=1 gap_us=1 missed=0".to_owned());
        }
        assert!(Report::parse(&console(&lines)).is_err());
        // A truncated console of every prefix of the block never panics.
        let full = console(&body());
        for end in 0..full.len() {
            let _ = check(&full[..end], PASS, &expected());
        }
        // Results that are no verdict.
        for line in [
            "OER-RESULT: pass w1",
            "OER-RESULT: pass w0",
            "OER-RESULT: pass w1 beacons=x required=1",
            "OER-RESULT: fail w1",
            "OER-RESULT: fail w1 a;;b",
            "nothing",
        ] {
            assert!(parse_result(line).is_err(), "{line}");
        }
    }

    #[test]
    fn the_beacon_gate_counts_the_stands_beacons() {
        let pass = host_of(&body());
        assert_eq!(
            pass.beacons,
            Some(BeaconFigures {
                beacons: 97,
                bi_us: 102_400,
                window_us: 10_000_000,
                required: Some(78)
            })
        );
        let mut few = body();
        set(&mut few, "w1.bss idx=0", "beacons", "78");
        // 78 − 1 duplicate = 77 < 78.
        assert_eq!(host_gate(&few, GateName::Beacons).reason, Some("beacons"));
        set(&mut few, "w1.bss idx=0", "dup", "0");
        assert_eq!(host_gate(&few, GateName::Beacons).result, GateResult::Pass);
        let mut more_dup = body();
        set(&mut more_dup, "w1.bss idx=0", "dup", "99");
        let report = Report::parse(&console(&more_dup)).unwrap();
        assert!(recompute(&report, &expected()).is_err());
    }

    #[test]
    fn a_missing_stand_or_zero_interval_fails_the_beacon_gate() {
        let mut absent = body();
        absent.retain(|line| !line.contains("who=stand"));
        let gate = host_gate(&absent, GateName::Beacons);
        assert_eq!(
            (gate.result, gate.reason),
            (GateResult::Fail, Some("stand-absent"))
        );
        set(&mut absent, "w1.survey", "table_full", "3");
        assert_eq!(
            host_gate(&absent, GateName::Beacons).reason,
            Some("stand-not-tracked")
        );

        let mut zero = body();
        set(&mut zero, "w1.bss idx=0", "bi_tu", "0");
        let host = host_of(&zero);
        assert_eq!(
            host.gate(GateName::Beacons).unwrap().reason,
            Some("zero-interval")
        );
        assert_eq!(host.beacons.unwrap().required, None);
        // The image agreeing on the zero interval is a plain fail.
        set(&mut zero, "w1.verdict", "bi_us", "0");
        set_gate(&mut zero, "beacons", "result=fail reason=zero-interval");
        let checked = check(
            &console(&zero),
            "OER-RESULT: fail w1 beacons zero-interval",
            &expected(),
        );
        assert_eq!(
            checked.outcome,
            Outcome::Fail(vec!["beacons zero-interval".to_owned()])
        );
        assert_eq!(checked.outcome.exit_code(), 1);
    }

    /// The image's block and verdict when its stand BSSID was heard without
    /// the stand's SSID.
    fn mismatched() -> Vec<String> {
        let mut body = body();
        set(&mut body, "w1.bss idx=0", "ssid_ok", "0");
        set(&mut body, "w1.config", "ssid_matches", "0");
        set(&mut body, "w1.verdict", "beacons", "0");
        set(&mut body, "w1.verdict", "bi_us", "0");
        set(&mut body, "w1.verdict", "required", "0");
        set_gate(&mut body, "beacons", "result=fail reason=stand-mismatch");
        body
    }

    #[test]
    fn the_stand_is_its_bssid_carrying_the_stand_ssid() {
        // Present with the SSID: the good block passes.
        assert_eq!(
            check(&console(&body()), PASS, &expected()).outcome,
            Outcome::Pass
        );
        // Present without it: stand-mismatch, and the host agrees.
        let mismatch = mismatched();
        let gate = host_gate(&mismatch, GateName::Beacons);
        assert_eq!(
            (gate.result, gate.reason),
            (GateResult::Fail, Some("stand-mismatch"))
        );
        let checked = check(
            &console(&mismatch),
            "OER-RESULT: fail w1 beacons stand-mismatch",
            &expected(),
        );
        assert_eq!(
            checked.outcome,
            Outcome::Fail(vec!["beacons stand-mismatch".to_owned()])
        );
        assert!(
            checked
                .summary
                .iter()
                .any(|line| line.contains("without the stand's SSID")),
            "{:?}",
            checked.summary
        );
        // The image passing a mismatched stand is a contradiction.
        let mut passing = mismatched();
        set_gate(&mut passing, "beacons", "result=pass");
        assert_eq!(
            check(&console(&passing), PASS, &expected())
                .outcome
                .exit_code(),
            3
        );
        // Other BSSIDs carrying the stand's SSID do not stand in for an
        // absent stand: still stand-absent.
        let mut absent = body();
        absent.retain(|line| !line.contains("who=stand"));
        set(&mut absent, "w1.bss idx=1", "ssid_ok", "1");
        let gate = host_gate(&absent, GateName::Beacons);
        assert_eq!(gate.reason, Some("stand-absent"));
    }

    #[test]
    fn ssid_matches_counts_the_bss_lines_carrying_the_ssid() {
        let mut two = body();
        set(&mut two, "w1.bss idx=1", "ssid_ok", "1");
        set(&mut two, "w1.config", "ssid_matches", "2");
        let checked = check(&console(&two), PASS, &expected());
        assert_eq!(checked.outcome, Outcome::Pass);
        assert!(
            checked.summary[0].contains("2 BSS carry"),
            "{:?}",
            checked.summary
        );
        set(&mut two, "w1.config", "ssid_matches", "1");
        let Outcome::Contradiction(problems) = check(&console(&two), PASS, &expected()).outcome
        else {
            panic!("a miscounted ssid_matches passes");
        };
        assert!(problems[0].contains("ssid_matches=1"), "{problems:?}");
        // Each value is validated.
        for (prefix, key, value) in [
            ("w1.bss idx=0", "ssid_ok", "2"),
            ("w1.bss idx=0", "ssid_ok", "-"),
            ("w1.config", "ssid_matches", "x"),
        ] {
            let mut lines = body();
            set(&mut lines, prefix, key, value);
            assert!(Report::parse(&console(&lines)).is_err(), "{key}={value}");
        }
    }

    #[test]
    fn nothing_of_the_stand_is_shown_but_its_mask() {
        let config = Config {
            channel: 6,
            bssid: Mac::new([0x02, 0xAB, 0xCD, 0xEF, 0xF6, 0xD0]).unwrap(),
            ssid: Ssid::new(SSID.as_bytes()).unwrap(),
            country: *b"DE",
        };
        let mut shown = vec![format!("{config:?}"), format!("{config:#?}")];
        for (body, result) in [
            (body(), PASS),
            (mismatched(), "OER-RESULT: fail w1 beacons stand-mismatch"),
        ] {
            let checked = check(&console(&body), result, &config);
            shown.push(format!("{checked:?}"));
            shown.extend(checked.summary);
        }
        let shown = shown.join("\n");
        for secret in [SSID, "Made Up", "AB:CD", "ab:cd", "4d61646520"] {
            assert!(!shown.contains(secret), "{secret}: {shown}");
        }
    }

    #[test]
    fn the_channel_gate_compares_the_applied_channel() {
        let mut applied = body();
        set(&mut applied, "w1.config", "applied_channel", "7");
        assert_eq!(
            host_gate(&applied, GateName::Channel).result,
            GateResult::Fail
        );
        // The image passing it anyway is a contradiction.
        assert_eq!(
            check(&console(&applied), PASS, &expected())
                .outcome
                .exit_code(),
            3
        );
        // A build for another channel than the router's is a stale build.
        let mut stale = body();
        set(&mut stale, "w1.config", "channel", "7");
        set(&mut stale, "w1.config", "applied_channel", "7");
        let checked = check(&console(&stale), PASS, &expected());
        let Outcome::Contradiction(problems) = checked.outcome else {
            panic!("{:?}", checked.outcome);
        };
        assert!(problems.iter().any(|problem| problem.contains("stale")));
        // So is another stand mask.
        let mut stand = body();
        set(&mut stand, "w1.config", "stand", "xx:xx:xx:xx:f6:d1");
        let Outcome::Contradiction(problems) = check(&console(&stand), PASS, &expected()).outcome
        else {
            panic!("a stale stand passes");
        };
        assert!(problems[0].contains("xx:xx:xx:xx:F6:D1"), "{problems:?}");
        // The image's lower-case mask is the same mask.
        let mut lower = body();
        set(&mut lower, "w1.config", "stand", "xx:xx:xx:xx:f6:d0");
        assert_eq!(
            check(&console(&lower), PASS, &expected()).outcome,
            Outcome::Pass
        );
    }

    #[test]
    fn the_exit_stack_service_and_unserviced_gates() {
        let mut exit = body();
        set(&mut exit, "w1.exit", "cause", "Fault");
        assert_eq!(host_gate(&exit, GateName::Exit).result, GateResult::Fail);
        let mut end = body();
        set(&mut end, "w1.exit", "end", "Abandoned");
        assert_eq!(host_gate(&end, GateName::Exit).result, GateResult::Fail);

        let mut stack = body();
        set(&mut stack, "w1.stack", "irq_free", "4096");
        assert_eq!(
            host_gate(&stack, GateName::IrqStack).result,
            GateResult::Pass
        );
        set(&mut stack, "w1.stack", "irq_free", "4095");
        assert_eq!(
            host_gate(&stack, GateName::IrqStack).result,
            GateResult::Fail
        );

        let mut errors = body();
        set(&mut errors, "w1.rx", "service_errors", "1");
        assert_eq!(
            host_gate(&errors, GateName::ServiceErrors).result,
            GateResult::Fail
        );

        for key in ["unserviced_mac", "unserviced_pwr"] {
            let mut unserviced = body();
            set(&mut unserviced, "w1.rx", key, "1");
            assert_eq!(
                host_gate(&unserviced, GateName::Unserviced).result,
                GateResult::Fail
            );
        }
        let mut overflow = body();
        set(
            &mut overflow,
            "w1.rx",
            "unserviced_mac",
            &u64::MAX.to_string(),
        );
        set(&mut overflow, "w1.rx", "unserviced_pwr", "1");
        let report = Report::parse(&console(&overflow)).unwrap();
        assert!(recompute(&report, &expected()).is_err());
    }

    #[test]
    fn the_walker_restart_gate() {
        let mut unexercised = body();
        set(&mut unexercised, "w1.hold", "reached", "31");
        set(&mut unexercised, "w1.hold", "full", "0");
        set(&mut unexercised, "w1.hold", "reason", "-");
        let gate = host_gate(&unexercised, GateName::WalkerRestart);
        assert_eq!(
            (gate.result, gate.reason),
            (GateResult::Unexercised, Some("pool-not-exhausted"))
        );
        // A hold that was not full leaves the ownership gate unexercised too.
        for name in ["walker_restart", "ownership"] {
            set_gate(
                &mut unexercised,
                name,
                "result=unexercised reason=pool-not-exhausted",
            );
        }
        let checked = check(
            &console(&unexercised),
            "OER-RESULT: fail w1 walker-restart not exercised; ownership not exercised",
            &expected(),
        );
        assert_eq!(
            checked.outcome,
            Outcome::Fail(vec![
                "walker-restart not exercised".to_owned(),
                "ownership not exercised".to_owned()
            ])
        );
        assert_eq!(checked.outcome.exit_code(), 1);
        // The image wording it otherwise contradicts the host.
        let checked = check(
            &console(&unexercised),
            "OER-RESULT: fail w1 walker-restart unexercised; ownership not exercised",
            &expected(),
        );
        assert_eq!(checked.outcome.exit_code(), 3);

        // Frames while every lease was held: the walker never ran out.
        let mut received = body();
        set(&mut received, "w1.hold", "units", "2");
        let gate = host_gate(&received, GateName::WalkerRestart);
        assert_eq!(
            (gate.result, gate.reason),
            (GateResult::Fail, Some("received-while-held"))
        );
        set_gate(
            &mut received,
            "walker_restart",
            "result=fail reason=received-while-held",
        );
        assert_eq!(
            check(
                &console(&received),
                "OER-RESULT: fail w1 walker-restart received-while-held",
                &expected()
            )
            .outcome
            .exit_code(),
            1
        );
        // The reason is bound: another one contradicts the host.
        set_gate(&mut received, "walker_restart", "result=fail reason=held");
        assert_eq!(
            check(
                &console(&received),
                "OER-RESULT: fail w1 walker-restart held",
                &expected()
            )
            .outcome
            .exit_code(),
            3
        );

        let mut silent = body();
        set(&mut silent, "w1.rx", "frames_after_release", "0");
        assert_eq!(
            host_gate(&silent, GateName::WalkerRestart).reason,
            Some("no-frames-after-release")
        );
        // A hold that never reached the pool's size outranks both.
        set(&mut silent, "w1.hold", "units", "5");
        set(&mut silent, "w1.hold", "reached", "0");
        set(&mut silent, "w1.hold", "full", "0");
        set(&mut silent, "w1.hold", "reason", "-");
        assert_eq!(
            host_gate(&silent, GateName::WalkerRestart).result,
            GateResult::Unexercised
        );

        // The recovery path is report-only.
        let mut recovered = body();
        set(&mut recovered, "w1.reload", "reload_rerings", "1");
        set(&mut recovered, "w1.rx", "capacity_drains", "0");
        set(&mut recovered, "w1.exhaust", "snapshots", "0");
        assert_eq!(
            host_gate(&recovered, GateName::WalkerRestart).result,
            GateResult::Pass
        );
    }

    #[test]
    fn the_base_writes_gate_requires_one_base_write() {
        assert_eq!(
            host_gate(&body(), GateName::BaseWrites).result,
            GateResult::Pass
        );
        for writes in ["0", "2"] {
            let mut written = body();
            set(&mut written, "w1.reload", "base_writes", writes);
            let gate = host_gate(&written, GateName::BaseWrites);
            assert_eq!(
                (gate.result, gate.reason),
                (GateResult::Fail, Some("base-writes")),
                "base_writes={writes}"
            );
        }
    }

    #[test]
    fn the_reserved_gate_allows_one_cold_landing_and_no_late_one() {
        for (cold, late, pass) in [
            ("0", "0", true),
            ("1", "0", true),
            ("2", "0", false),
            ("1", "1", false),
            ("0", "1", false),
        ] {
            let mut landed = body();
            set(&mut landed, "w1.reload", "reserved_landings", cold);
            set(&mut landed, "w1.reload", "reserved_late_landings", late);
            let gate = host_gate(&landed, GateName::Reserved);
            let expected = if pass {
                (GateResult::Pass, None)
            } else {
                (GateResult::Fail, Some("reserved-landing"))
            };
            assert_eq!(
                (gate.result, gate.reason),
                expected,
                "reserved_landings={cold} reserved_late_landings={late}"
            );
        }
    }

    #[test]
    fn the_walker_on_detached_gate_requires_no_such_read() {
        for (count, pass) in [("0", true), ("1", false), ("3", false)] {
            let mut stopped = body();
            set(&mut stopped, "w1.reload", "walker_on_detached", count);
            let gate = host_gate(&stopped, GateName::WalkerOnDetached);
            let expected = if pass {
                (GateResult::Pass, None)
            } else {
                (GateResult::Fail, Some("walker-on-detached"))
            };
            assert_eq!(
                (gate.result, gate.reason),
                expected,
                "walker_on_detached={count}"
            );
        }
    }

    #[test]
    fn the_owner_bit_gate_requires_every_held_descriptor_clear() {
        for (cycle, clear, pass) in [(0, "32", true), (0, "31", false), (1, "0", false)] {
            let mut lines = body();
            set_cycle(&mut lines, "w1.own", cycle, "owner_clear", clear);
            let gate = host_gate(&lines, GateName::OwnerBit);
            let expected = if pass {
                (GateResult::Pass, None)
            } else {
                (GateResult::Fail, Some("owner-bit-set"))
            };
            assert_eq!(
                (gate.result, gate.reason),
                expected,
                "cycle={cycle} owner_clear={clear}"
            );
        }
        // The key is required.
        let mut missing = body();
        let line = missing
            .iter_mut()
            .find(|line| line.starts_with("w1.own cycle=0"))
            .unwrap();
        *line = line.replace(" owner_clear=32", "");
        assert!(Report::parse(&console(&missing)).is_err());
    }

    #[test]
    fn the_reload_line_is_strict() {
        // The keys of the removed BASE writers are refused.
        let mut old = body();
        let line = old
            .iter_mut()
            .find(|line| line.starts_with("w1.reload"))
            .unwrap();
        *line = "w1.reload max_attempts=3 limit_us=20 observations=2 base_repairs=0 exhausted_republications=1 republications_accepted=1".to_owned();
        assert!(Report::parse(&console(&old)).is_err());
        // Every key is required.
        for key in [
            "reload_rerings",
            "rering_failures",
            "base_writes",
            "reserved_landings",
            "reserved_late_landings",
            "walker_on_detached",
        ] {
            let mut missing = body();
            let line = missing
                .iter_mut()
                .find(|line| line.starts_with("w1.reload"))
                .unwrap();
            *line = line
                .split_whitespace()
                .filter(|token| !token.starts_with(&format!("{key}=")))
                .collect::<Vec<_>>()
                .join(" ");
            assert!(Report::parse(&console(&missing)).is_err(), "{key}");
        }
    }

    #[test]
    fn the_hold_fill_window_is_bounded() {
        for (fill, ok) in [
            ("3000", true),
            ("10000", true),
            ("2999", false),
            ("10001", false),
        ] {
            let mut hold = body();
            set(&mut hold, "w1.hold", "fill_ms", fill);
            assert_eq!(Report::parse(&console(&hold)).is_ok(), ok, "fill_ms={fill}");
        }
    }

    #[test]
    fn the_timer_wake_gate_fails_on_any_early_wake() {
        assert_eq!(
            host_gate(&body(), GateName::TimerWake).result,
            GateResult::Pass
        );
        for prefix in ["w1.timer", "w1.wake"] {
            let mut early = body();
            set(&mut early, prefix, "woke_early", "1");
            let gate = host_gate(&early, GateName::TimerWake);
            assert_eq!(
                (gate.result, gate.reason),
                (GateResult::Fail, Some("woke-early")),
                "{prefix}"
            );
            // The image passing it anyway is a contradiction.
            assert_eq!(
                check(&console(&early), PASS, &expected())
                    .outcome
                    .exit_code(),
                3
            );
            set_gate(&mut early, "timer_wake", "result=fail reason=woke-early");
            let checked = check(
                &console(&early),
                "OER-RESULT: fail w1 timer-wake woke-early",
                &expected(),
            );
            assert_eq!(
                checked.outcome,
                Outcome::Fail(vec!["timer-wake woke-early".to_owned()])
            );
        }
        // Without any timer or wake line nothing woke early.
        let mut none = body();
        none.retain(|line| !line.starts_with("w1.timer") && !line.starts_with("w1.wake"));
        assert_eq!(
            host_gate(&none, GateName::TimerWake).result,
            GateResult::Pass
        );
    }

    #[test]
    fn the_hold_exhaust_and_span_lines_parse_and_are_required() {
        let report = Report::parse(&console(&body())).unwrap();
        assert_eq!(report.hold.len(), 2);
        assert_eq!(report.hold[0].number("reached"), Ok(32));
        assert_eq!(report.exhaust[0].text("base"), Ok("0x2f017000"));
        assert_eq!(report.exhaust[0].text("next_after_doorbell"), Ok("0x17030"));
        assert_eq!(report.exhaust[1].text("last_after_doorbell"), Ok("-"));
        assert_eq!(report.own[1].text("order"), Ok("reverse"));
        assert_eq!(report.wake.len(), 1);
        assert_eq!(report.bss[0].number("stamp_span_us"), Ok(9_932_800));
        assert_eq!(report.bss[1].text("handoff_span_us"), Ok("-"));
        // A run without a snapshot prints every value as `-`.
        let mut none = body();
        let line = none
            .iter_mut()
            .find(|line| line.starts_with("w1.exhaust"))
            .unwrap();
        *line = line
            .split_whitespace()
            .map(|token| match token.split_once('=') {
                Some(("snapshots", _)) => "snapshots=0".to_owned(),
                Some(("cycle", _)) => token.to_owned(),
                Some((name, _)) => format!("{name}=-"),
                None => token.to_owned(),
            })
            .collect::<Vec<_>>()
            .join(" ");
        assert!(Report::parse(&console(&none)).is_ok());
        for keyword in ["w1.hold", "w1.exhaust", "w1.own"] {
            let mut missing = body();
            missing.retain(|line| !line.starts_with(keyword));
            assert!(Report::parse(&console(&missing)).is_err(), "{keyword}");
            let mut twice = body();
            let line = twice
                .iter()
                .find(|line| line.starts_with(keyword))
                .unwrap()
                .clone();
            twice.insert(1, line);
            assert!(Report::parse(&console(&twice)).is_err(), "{keyword}");
        }
        // The timer lines need their wait counts.
        let mut bare = body();
        for line in bare.iter_mut().filter(|line| line.starts_with("w1.timer")) {
            *line = line.replace(" woke_early=0", "");
        }
        assert!(Report::parse(&console(&bare)).is_err());
    }

    /// Set `key=value` on the `keyword cycle=<cycle>` line.
    fn set_cycle(body: &mut [String], keyword: &str, cycle: u8, key: &str, value: &str) {
        set(body, &format!("{keyword} cycle={cycle} "), key, value);
    }

    #[test]
    fn the_cycle_lines_come_once_per_cycle_in_order() {
        // One line per cycle, cycle 0 first, forward then reverse.
        for keyword in ["w1.hold", "w1.exhaust", "w1.own"] {
            let mut swapped = body();
            let first = swapped
                .iter()
                .position(|line| line.starts_with(&format!("{keyword} cycle=0 ")))
                .unwrap();
            swapped.swap(first, first + 1);
            assert!(Report::parse(&console(&swapped)).is_err(), "{keyword}");
            let mut single = body();
            single.remove(first);
            assert!(Report::parse(&console(&single)).is_err(), "{keyword}");
        }
        let mut reversed = body();
        set_cycle(&mut reversed, "w1.own", 0, "order", "reverse");
        assert!(Report::parse(&console(&reversed)).is_err());

        let sixteen = (0..16).map(|i| i.to_string()).collect::<Vec<_>>().join(",");
        let mut full = body();
        set_cycle(&mut full, "w1.own", 0, "first_after_release", &sixteen);
        set_cycle(&mut full, "w1.own", 0, "order_breaks", "0");
        assert!(Report::parse(&console(&full)).is_ok());
        for (key, value) in [
            ("first_after_release", format!("{sixteen},16")),
            ("first_after_release", "4,,5".to_owned()),
            ("first_after_release", "4,y".to_owned()),
            ("first_after_release", "".to_owned()),
            ("first_mismatch", "y".to_owned()),
            ("first_mismatch", "-1".to_owned()),
            ("first_drop", "x".to_owned()),
            ("order", "sideways".to_owned()),
            ("held", "-".to_owned()),
            ("chained", "-".to_owned()),
        ] {
            let mut lines = body();
            set_cycle(&mut lines, "w1.own", 1, key, &value);
            assert!(
                matches!(
                    Report::parse(&console(&lines)),
                    Err(ReportError::Malformed(_))
                ),
                "w1.own {key}={value}"
            );
        }
        for (key, value) in [
            ("next_after_doorbell", "17030"),
            ("last_after_doorbell", "0x"),
            ("desc_hash_changed", "2"),
            ("cycle", "2"),
        ] {
            let mut lines = body();
            set_cycle(&mut lines, "w1.exhaust", 1, key, value);
            assert!(
                Report::parse(&console(&lines)).is_err(),
                "w1.exhaust {key}={value}"
            );
        }
        let mut bare = body();
        for line in bare
            .iter_mut()
            .filter(|line| line.starts_with("w1.exhaust"))
        {
            *line = line.replace(" desc_hash_changed=0", "");
        }
        assert!(Report::parse(&console(&bare)).is_err());
    }

    #[test]
    fn the_ownership_gate() {
        assert_eq!(
            host_gate(&body(), GateName::Ownership).result,
            GateResult::Pass
        );
        // Oversized drops are reported, not gated.
        let mut oversized = body();
        set_cycle(&mut oversized, "w1.own", 0, "oversized", "4");
        assert_eq!(
            host_gate(&oversized, GateName::Ownership).result,
            GateResult::Pass
        );
        for cycle in [0, 1] {
            for key in [
                "hold_mismatch",
                "dwell_mismatch",
                "release_mismatch",
                "chained",
            ] {
                let mut written = body();
                set_cycle(&mut written, "w1.own", cycle, key, "1");
                let gate = host_gate(&written, GateName::Ownership);
                assert_eq!(
                    (gate.result, gate.reason),
                    (GateResult::Fail, Some("foreign-write")),
                    "cycle {cycle} {key}"
                );
                // The image passing it is a contradiction.
                assert_eq!(
                    check(&console(&written), PASS, &expected())
                        .outcome
                        .exit_code(),
                    3
                );
                set_gate(
                    &mut written,
                    "ownership",
                    "result=fail reason=foreign-write",
                );
                let checked = check(
                    &console(&written),
                    "OER-RESULT: fail w1 ownership foreign-write",
                    &expected(),
                );
                assert_eq!(
                    checked.outcome,
                    Outcome::Fail(vec!["ownership foreign-write".to_owned()])
                );
            }
        }

        let mut short = body();
        set_cycle(&mut short, "w1.own", 1, "held", "31");
        set_cycle(&mut short, "w1.own", 1, "owner_clear", "31");
        set_cycle(&mut short, "w1.hold", 1, "reached", "31");
        set_cycle(&mut short, "w1.hold", 1, "full", "0");
        set_cycle(&mut short, "w1.hold", 1, "reason", "-");
        let gate = host_gate(&short, GateName::Ownership);
        assert_eq!(
            (gate.result, gate.reason),
            (GateResult::Unexercised, Some("pool-not-exhausted"))
        );
        set_gate(
            &mut short,
            "ownership",
            "result=unexercised reason=pool-not-exhausted",
        );
        assert_eq!(
            check(
                &console(&short),
                "OER-RESULT: fail w1 ownership not exercised",
                &expected()
            )
            .outcome,
            Outcome::Fail(vec!["ownership not exercised".to_owned()])
        );
        let mut unordered = body();
        set_cycle(&mut unordered, "w1.own", 0, "first_drop", "-");
        let gate = host_gate(&unordered, GateName::Ownership);
        assert_eq!(
            (gate.result, gate.reason),
            (GateResult::Unexercised, Some("ring-order"))
        );
        // A short hold outranks the ring order, a write outranks both.
        set_cycle(&mut unordered, "w1.hold", 1, "reached", "0");
        set_cycle(&mut unordered, "w1.hold", 1, "full", "0");
        set_cycle(&mut unordered, "w1.hold", 1, "reason", "-");
        assert_eq!(
            host_gate(&unordered, GateName::Ownership).reason,
            Some("pool-not-exhausted")
        );
        set_cycle(&mut unordered, "w1.own", 1, "dwell_mismatch", "2");
        assert_eq!(
            host_gate(&unordered, GateName::Ownership).reason,
            Some("foreign-write")
        );

        // The walker restart reads the first cycle only.
        let mut second = body();
        set_cycle(&mut second, "w1.hold", 1, "units", "3");
        assert_eq!(
            host_gate(&second, GateName::WalkerRestart).result,
            GateResult::Pass
        );
        set_cycle(&mut second, "w1.hold", 0, "units", "3");
        assert_eq!(
            host_gate(&second, GateName::WalkerRestart).reason,
            Some("received-while-held")
        );
    }

    /// Mark cycle `cycle` of `body` stuck at LAST with the drain end of run
    /// 5's reverse cycle: the buffer without a lease (descriptor 17, the
    /// accepted tail) is the walker's frozen LAST unit, not released.
    fn stuck_like_run_5(body: &mut [String], cycle: u8) {
        set_cycle(body, "w1.hold", cycle, "reached", "31");
        set_cycle(body, "w1.hold", cycle, "reason", "stuck-at-last");
        set_cycle(body, "w1.own", cycle, "held", "31");
        set_cycle(body, "w1.own", cycle, "owner_clear", "31");
        for (key, value) in [
            ("end_released", "0"),
            ("end_detached", "31"),
            ("end_staged", "31"),
            ("end_observed", "32"),
            ("end_recycle_start", "18"),
            ("end_accepted_tail", "17"),
            ("end_next", "0x0"),
            ("end_last", "0x17f90"),
        ] {
            set_cycle(body, "w1.hold", cycle, key, value);
        }
    }

    #[test]
    fn a_hold_stuck_at_last_is_full() {
        // One short, stuck as in run 5: both gates take it as full.
        let mut stuck = body();
        for cycle in [0, 1] {
            stuck_like_run_5(&mut stuck, cycle);
        }
        assert_eq!(
            check(&console(&stuck), PASS, &expected()).outcome,
            Outcome::Pass
        );
        // A released buffer at LAST is stuck as well.
        set_cycle(&mut stuck, "w1.hold", 1, "end_released", "1");
        assert_eq!(
            check(&console(&stuck), PASS, &expected()).outcome,
            Outcome::Pass
        );
        // A drain end without the stuck shape contradicts the image's
        // reason.
        for (key, value) in [
            ("end_released", "2"),
            ("end_detached", "32"),
            ("end_staged", "30"),
            ("end_observed", "31"),
            ("end_next", "0x17e50"),
            ("end_next", "-"),
        ] {
            let mut lines = body();
            stuck_like_run_5(&mut lines, 1);
            set_cycle(&mut lines, "w1.hold", 1, key, value);
            let Outcome::Contradiction(problems) =
                check(&console(&lines), PASS, &expected()).outcome
            else {
                panic!("{key}={value} is a contradiction");
            };
            assert!(
                problems
                    .iter()
                    .any(|problem| problem.contains("stuck at LAST")),
                "{key}={value}: {problems:?}"
            );
        }
        // full= and reason= must agree with reached=.
        for (reached, full, reason) in [
            ("32", "1", "stuck-at-last"),
            ("31", "1", "all"),
            ("30", "1", "stuck-at-last"),
            ("32", "0", "-"),
            ("31", "0", "all"),
            ("31", "1", "-"),
        ] {
            let mut lines = body();
            set_cycle(&mut lines, "w1.hold", 1, "reached", reached);
            set_cycle(&mut lines, "w1.hold", 1, "full", full);
            set_cycle(&mut lines, "w1.hold", 1, "reason", reason);
            assert!(
                matches!(
                    Report::parse(&console(&lines)),
                    Err(ReportError::Malformed(_))
                ),
                "reached={reached} full={full} reason={reason}"
            );
        }
        for (key, value) in [
            ("full", "2"),
            ("reason", "stuck"),
            ("end_next", "0"),
            ("end_released", "x"),
        ] {
            let mut lines = body();
            set_cycle(&mut lines, "w1.hold", 0, key, value);
            assert!(Report::parse(&console(&lines)).is_err(), "{key}={value}");
        }
    }

    #[test]
    fn the_discards_are_strict_and_attribute_the_skips() {
        assert_eq!(
            discards("chained:2,rx_failure:1"),
            Some(vec![("chained", 2), ("rx_failure", 1)])
        );
        for broken in [
            "chained:0",
            "chained",
            "chained:1,chained:2",
            "lost:1",
            "chained:1,",
            "",
            "chained:4294967296",
        ] {
            let mut lines = body();
            set_cycle(&mut lines, "w1.own", 0, "discards", broken);
            assert!(Report::parse(&console(&lines)).is_err(), "{broken}");
        }
        // 29,31 and 30,0 each skip one descriptor; a backward step is no
        // skip.
        assert_eq!(skipped(&indices("29,31,0"), 32), 1);
        assert_eq!(skipped(&indices("29,30,0,1"), 32), 1);
        assert_eq!(skipped(&indices("5,3,x,9"), 32), 0);
        let mut lines = body();
        set_cycle(&mut lines, "w1.own", 0, "first_after_release", "29,31,0");
        set_cycle(&mut lines, "w1.own", 0, "order_breaks", "1");
        let summary = check(&console(&lines), PASS, &expected()).summary;
        assert!(
            summary
                .iter()
                .any(|line| line.contains("1 descriptor(s) skipped")
                    && line.contains("1 skip(s) not counted by any drop")),
            "{summary:?}"
        );
        set_cycle(&mut lines, "w1.own", 0, "discards", "too_long:1");
        let summary = check(&console(&lines), PASS, &expected()).summary;
        assert!(
            !summary.iter().any(|line| line.contains("not counted")),
            "{summary:?}"
        );
    }

    #[test]
    fn the_order_breaks_are_recomputed_from_the_list() {
        assert_eq!(order_breaks(&indices("-"), 32), 0);
        assert_eq!(order_breaks(&indices("30,31,0,1"), 32), 0);
        assert_eq!(order_breaks(&indices("4,5,7,x,31,0"), 32), 3);
        assert_eq!(
            check(&console(&body()), PASS, &expected()).outcome,
            Outcome::Pass
        );
        let mut miscounted = body();
        set_cycle(&mut miscounted, "w1.own", 1, "order_breaks", "2");
        let Outcome::Contradiction(problems) =
            check(&console(&miscounted), PASS, &expected()).outcome
        else {
            panic!("a miscount is a contradiction");
        };
        assert!(
            problems
                .iter()
                .any(|problem| problem.contains("order_breaks=2")),
            "{problems:?}"
        );
        // The ring size comes from the arena.
        let mut arena = body();
        set(&mut arena, "w1.config", "arena", "big");
        assert!(matches!(
            check(&console(&arena), PASS, &expected()).outcome,
            Outcome::Unreadable(_)
        ));
    }

    fn lags(lines: &[String]) -> Vec<Lag> {
        Report::parse(&console(lines)).unwrap().lags
    }

    #[test]
    fn the_lag_fit_takes_the_smallest_lag_as_the_offset() {
        let lags = lags(&body());
        assert_eq!(lags.len(), 3);
        assert_eq!(
            lag_fit(&lags, 2_000),
            Some(LagFit {
                median_us: 1_700,
                max_us: 101_200,
                next_arrival_matches: 2
            })
        );
        // A tighter tolerance drops the handoff 300 us after its successor.
        assert_eq!(lag_fit(&lags, 250).unwrap().next_arrival_matches, 1);
        // A handoff before the next unit's arrival is no match.
        let early = [
            Lag {
                stamp: 0,
                lag_us: 1_000,
            },
            Lag {
                stamp: 5_000,
                lag_us: 0,
            },
        ];
        assert_eq!(lag_fit(&early, 2_000).unwrap().next_arrival_matches, 0);
        // Lags across the 32-bit wrap are one offset.
        let wrapped = [
            Lag {
                stamp: 10,
                lag_us: u32::MAX - 9,
            },
            Lag {
                stamp: 20,
                lag_us: 20,
            },
        ];
        let fit = lag_fit(&wrapped, 2_000).unwrap();
        assert_eq!((fit.median_us, fit.max_us), (0, 30));
        assert_eq!(lag_fit(&[], 2_000), None);
        // No unit: every value `-`.
        let mut none = body();
        none.retain(|line| !line.starts_with("w1.lag "));
        set(&mut none, "w1.lagfit", "n", "0");
        set(&mut none, "w1.lagfit", "median_us", "-");
        set(&mut none, "w1.lagfit", "max_us", "-");
        set(&mut none, "w1.lagfit", "next_arrival_matches", "0");
        assert_eq!(
            check(&console(&none), PASS, &expected()).outcome,
            Outcome::Pass
        );
    }

    #[test]
    fn the_lag_lines_are_strict_and_the_fit_is_recomputed() {
        for (prefix, key, value) in [
            ("w1.lag i=1", "i", "2"),
            ("w1.lag i=1", "since_prev_us", "1499"),
            ("w1.lag i=0", "until_next_us", "-"),
            ("w1.lag i=0", "since_prev_us", "0"),
            ("w1.lag i=2", "until_next_us", "5"),
            ("w1.lag i=2", "lag_us", "-"),
            ("w1.lagfit", "n", "2"),
            ("w1.lagfit", "tolerance_us", "4294967296"),
        ] {
            let mut lines = body();
            set(&mut lines, prefix, key, value);
            assert!(
                matches!(
                    Report::parse(&console(&lines)),
                    Err(ReportError::Malformed(_))
                ),
                "{prefix} {key}={value}"
            );
        }
        let mut missing = body();
        missing.retain(|line| !line.starts_with("w1.lagfit"));
        assert!(Report::parse(&console(&missing)).is_err());
        let mut many = body();
        many.retain(|line| !line.starts_with("w1.lag"));
        for index in 0..=MAX_LAGS {
            let until = if index == MAX_LAGS { "-" } else { "1" };
            let since = if index == 0 { "-" } else { "1" };
            many.push(format!(
                "w1.lag i={index} stamp={index} lag_us=0 since_prev_us={since} until_next_us={until}"
            ));
        }
        many.push(format!(
            "w1.lagfit n={} median_us=0 max_us=0 next_arrival_matches=0 tolerance_us=2000",
            MAX_LAGS + 1
        ));
        assert!(Report::parse(&console(&many)).is_err());
        // An image fit the host does not recompute is a contradiction.
        for (key, value) in [
            ("median_us", "1701"),
            ("max_us", "-"),
            ("next_arrival_matches", "1"),
        ] {
            let mut lines = body();
            set(&mut lines, "w1.lagfit", key, value);
            let checked = check(&console(&lines), PASS, &expected());
            assert_eq!(checked.outcome.exit_code(), 3, "{key}={value}");
        }
        let summary = check(&console(&body()), PASS, &expected()).summary;
        assert!(
            summary
                .iter()
                .any(|line| line.contains("median 1700 us, max 101200 us; 2 handed off")),
            "{summary:?}"
        );
    }

    fn pairs(points: &[(u32, u32)]) -> Vec<TsfPair> {
        points
            .iter()
            .map(|&(tsf, handoff)| TsfPair { tsf, handoff })
            .collect()
    }

    #[test]
    fn the_tsf_fit_removes_the_trend_and_spreads_from_the_earliest() {
        // On the line handoff = tsf + c every residual is zero.
        let exact = pairs(&[(0, 100), (102_400, 102_500), (204_800, 204_900)]);
        assert_eq!(
            tsf_fit(&exact),
            Some(TsfFit {
                median_us: 0,
                p90_us: 0,
                max_us: 0,
                slope_ppm: 0
            })
        );
        // A clock 100 ppm fast, no jitter.
        let fast = pairs(&[(0, 0), (1_000_000, 1_000_100), (2_000_000, 2_000_200)]);
        assert_eq!(tsf_fit(&fast).unwrap().slope_ppm, 100);
        assert_eq!(tsf_fit(&fast).unwrap().max_us, 0);
        // One beacon 900 us late among ten on time: the spread shows it.
        let mut late = (0..10_u32)
            .map(|index| (index * 102_400, index * 102_400))
            .collect::<Vec<_>>();
        late[5].1 += 900;
        let fit = tsf_fit(&pairs(&late)).unwrap();
        assert!((800..=1_000).contains(&fit.max_us), "{fit:?}");
        assert!(fit.median_us <= 100, "{fit:?}");
        // The wrap of either clock is harmless.
        let wrapped = pairs(&[(u32::MAX - 50, u32::MAX - 10), (51, 91), (153, 193)]);
        assert_eq!(tsf_fit(&wrapped).unwrap().max_us, 0);
        // Fewer than two pairs, or no TSF spread: no fit.
        assert_eq!(tsf_fit(&pairs(&[(1, 2)])), None);
        assert_eq!(tsf_fit(&pairs(&[(1, 2), (1, 9)])), None);
        assert_eq!(tsf_fit(&[]), None);
    }

    #[test]
    fn the_tsf_lines_are_strict_and_the_fit_is_recomputed() {
        let report = Report::parse(&console(&body())).unwrap();
        assert_eq!(
            tsf_fit(&report.tsf),
            Some(TsfFit {
                median_us: 208,
                p90_us: 1_099,
                max_us: 1_099,
                slope_ppm: 966
            })
        );
        let summary = check(&console(&body()), PASS, &expected()).summary;
        assert!(
            summary.iter().any(|line| line.contains("over 4 beacons")),
            "{summary:?}"
        );
        for (prefix, key, value) in [
            ("w1.tsf i=1", "i", "3"),
            ("w1.tsf i=1", "tsf", "-"),
            ("w1.tsf i=1", "handoff", "4294967296"),
            ("w1.tsffit", "n", "3"),
            ("w1.tsffit", "slope_ppm", "2147483648"),
            ("w1.tsffit", "slope_ppm", "1.5"),
            ("w1.tsffit", "bss", "x"),
        ] {
            let mut lines = body();
            set(&mut lines, prefix, key, value);
            assert!(
                matches!(
                    Report::parse(&console(&lines)),
                    Err(ReportError::Malformed(_))
                ),
                "{prefix} {key}={value}"
            );
        }
        let mut missing = body();
        missing.retain(|line| !line.starts_with("w1.tsffit"));
        assert!(Report::parse(&console(&missing)).is_err());
        // A fit, a slope or a BSS the host does not reproduce contradicts it.
        for (key, value) in [
            ("median_us", "999"),
            ("p90_us", "-"),
            ("slope_ppm", "-1"),
            ("bss", "1"),
            ("bss", "-"),
        ] {
            let mut lines = body();
            set(&mut lines, "w1.tsffit", key, value);
            assert_eq!(
                check(&console(&lines), PASS, &expected())
                    .outcome
                    .exit_code(),
                3,
                "{key}={value}"
            );
        }
        // Without the stand: no pair, no index, no fit.
        let mut absent = body();
        absent.retain(|line| !line.starts_with("w1.tsf ") && !line.contains("who=stand"));
        set(&mut absent, "w1.tsffit", "n", "0");
        for key in ["bss", "median_us", "p90_us", "max_us", "slope_ppm"] {
            set(&mut absent, "w1.tsffit", key, "-");
        }
        let report = Report::parse(&console(&absent)).unwrap();
        let host = recompute(&report, &expected()).unwrap();
        let result = ImageResult::Fail {
            reasons: host.reasons(),
        };
        let problems = disagreements(&report, &host, &result, &expected()).unwrap();
        assert!(
            !problems.iter().any(|problem| problem.contains("w1.tsffit")),
            "{problems:?}"
        );
    }

    #[test]
    fn exit_codes() {
        assert_eq!(Outcome::Pass.exit_code(), 0);
        assert_eq!(Outcome::Fail(Vec::new()).exit_code(), 1);
        assert_eq!(Outcome::Contradiction(Vec::new()).exit_code(), 3);
        assert_eq!(Outcome::Unreadable(ReportError::Missing).exit_code(), 3);

        // A gate the image fails and the host passes.
        let mut gate = body();
        set_gate(&mut gate, "irq_stack", "result=fail reason=low");
        let checked = check(
            &console(&gate),
            "OER-RESULT: fail w1 irq-stack low",
            &expected(),
        );
        assert_eq!(checked.outcome.exit_code(), 3, "{:?}", checked.outcome);
        // A bound reason the image names otherwise.
        let mut reason = body();
        set(&mut reason, "w1.bss idx=0", "dup", "50");
        set(&mut reason, "w1.verdict", "beacons", "48");
        set_gate(&mut reason, "beacons", "result=fail reason=few");
        let fail = "OER-RESULT: fail w1 beacons few";
        assert_eq!(
            check(&console(&reason), fail, &expected())
                .outcome
                .exit_code(),
            3
        );
        set_gate(&mut reason, "beacons", "result=fail reason=beacons");
        assert_eq!(
            check(&console(&reason), fail, &expected())
                .outcome
                .exit_code(),
            1
        );
        // The verdict line counting otherwise.
        let mut verdict = body();
        set(&mut verdict, "w1.verdict", "required", "77");
        assert_eq!(
            check(&console(&verdict), PASS, &expected())
                .outcome
                .exit_code(),
            3
        );
        // The result line contradicting the gates.
        assert_eq!(
            check(
                &console(&body()),
                "OER-RESULT: fail w1 beacons beacons",
                &expected()
            )
            .outcome
            .exit_code(),
            3
        );
        assert_eq!(
            check(
                &console(&body()),
                "OER-RESULT: pass w1 beacons=96 required=78",
                &expected()
            )
            .outcome
            .exit_code(),
            3
        );
        // A fail that misses a failed gate, or names a passed one.
        let mut two = body();
        set(&mut two, "w1.rx", "service_errors", "2");
        set_gate(&mut two, "service_errors", "result=fail reason=errors");
        set(&mut two, "w1.stack", "irq_free", "100");
        set_gate(&mut two, "irq_stack", "result=fail reason=low");
        assert_eq!(
            check(
                &console(&two),
                "OER-RESULT: fail w1 service-errors 2",
                &expected()
            )
            .outcome
            .exit_code(),
            3
        );
        let both = check(
            &console(&two),
            "OER-RESULT: fail w1 irq-stack low; service-errors 2",
            &expected(),
        );
        assert_eq!(both.outcome.exit_code(), 1, "{:?}", both.outcome);
        assert_eq!(
            check(
                &console(&two),
                "OER-RESULT: fail w1 irq-stack low; service-errors 2; exit x",
                &expected()
            )
            .outcome
            .exit_code(),
            3
        );
    }

    #[test]
    fn gaps_overlapping_a_tick_are_found_across_the_wrap() {
        let ticks = [
            Tick {
                start: 900_000,
                end: 900_200,
                ok: true,
            },
            Tick {
                start: u32::MAX - 100,
                end: 50,
                ok: false,
            },
        ];
        let gaps = [
            // [795_200, 1_000_000] holds the first tick.
            Gap {
                stamp: 1_000_000,
                gap_us: 204_800,
                missed: Some(1),
            },
            // [400_000, 500_000]: before it.
            Gap {
                stamp: 500_000,
                gap_us: 100_000,
                missed: None,
            },
            // [2^32 − 1000, 2^32 + 9] wraps over the second tick.
            Gap {
                stamp: 10,
                gap_us: 1010,
                missed: None,
            },
            // Ends inside the first tick only after the offset.
            Gap {
                stamp: 899_950,
                gap_us: 10,
                missed: None,
            },
        ];
        assert_eq!(
            correlate(&gaps, &ticks, 0),
            [Overlap { gap: 0, tick: 0 }, Overlap { gap: 2, tick: 1 }]
        );
        assert_eq!(
            correlate(&gaps, &ticks, 100),
            [
                Overlap { gap: 0, tick: 0 },
                Overlap { gap: 2, tick: 1 },
                Overlap { gap: 3, tick: 0 }
            ]
        );
        // The offset itself wraps.
        let tick = [Tick {
            start: 5,
            end: 20,
            ok: true,
        }];
        let gap = [Gap {
            stamp: u32::MAX - 4,
            gap_us: 1,
            missed: None,
        }];
        assert_eq!(correlate(&gap, &tick, 15), [Overlap { gap: 0, tick: 0 }]);
        assert!(correlate(&gap, &tick, 0).is_empty());
    }
}
