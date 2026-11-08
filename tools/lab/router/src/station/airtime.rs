//! The AP radio's airtime queue limits: mac80211's Airtime Queue Limit
//! (AQL), which stops a station's dequeue to the driver once the estimated
//! airtime of its frames still pending reaches its limit. AQL never drops a
//! frame; it holds frames in mac80211's queue.
//!
//! Every value is read from and written to mac80211's debugfs on the
//! router.
//! - `aql_txq_limit` of the radio's PHY holds each access category's low and
//!   high limits in microseconds. A write `<ac> <low> <high>` also sets them
//!   on every station whose limits equal the old ones. A station that
//!   associates later starts with them (Linux v6.18.26
//!   `net/mac80211/debugfs.c` `aql_txq_limit_write`, `sta_info.c`
//!   `sta_info_alloc_link`).
//! - `aql_threshold` holds the total pending airtime above which every
//!   station is held to its low limit. The lab never writes it.
//! - The station's `aql` file holds its pending airtime and its own limits
//!   per access category.
//!
//! The router's end state after a lease has every access category at
//! mac80211's defaults ([`DEFAULT`]); [`restore_defaults`] writes and
//! confirms them.

use serde::{Deserialize, Serialize};

use super::mac_text;
use crate::{
    mac::Mac,
    router::quote,
    ssh::{Remote, Result},
};

/// An access category, as mac80211 numbers it in `aql_txq_limit`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum AccessCategory {
    Vo,
    Vi,
    Be,
    Bk,
}

impl AccessCategory {
    /// Every access category in mac80211's order.
    pub const ALL: [Self; 4] = [Self::Vo, Self::Vi, Self::Be, Self::Bk];

    /// mac80211's number of the access category.
    /// SOURCE: Linux v6.18.26 `include/net/mac80211.h`, `enum
    /// ieee80211_ac_numbers` (VO 0, VI 1, BE 2, BK 3).
    pub const fn number(self) -> u8 {
        match self {
            Self::Vo => 0,
            Self::Vi => 1,
            Self::Be => 2,
            Self::Bk => 3,
        }
    }

    /// The name mac80211's debugfs files print.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Vo => "VO",
            Self::Vi => "VI",
            Self::Be => "BE",
            Self::Bk => "BK",
        }
    }
}

/// One access category's low and high limits, in microseconds of estimated
/// airtime.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Limit {
    pub low_us: u32,
    pub high_us: u32,
}

/// mac80211's default limits of every access category.
/// SOURCE: Linux v6.18.26 `include/net/cfg80211.h`,
/// `IEEE80211_DEFAULT_AQL_TXQ_LIMIT_L` 5000 and
/// `IEEE80211_DEFAULT_AQL_TXQ_LIMIT_H` 12000.
pub const DEFAULT: Limit = Limit {
    low_us: 5000,
    high_us: 12000,
};

/// The highest high limit the lab writes: 100 ms of estimated airtime per
/// station and access category.
pub const MAX_HIGH_US: u32 = 100_000;

/// The limits of the four access categories, in [`AccessCategory::ALL`]'s
/// order.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub vo: Limit,
    pub vi: Limit,
    pub be: Limit,
    pub bk: Limit,
}

impl Limits {
    pub const fn get(&self, ac: AccessCategory) -> Limit {
        match ac {
            AccessCategory::Vo => self.vo,
            AccessCategory::Vi => self.vi,
            AccessCategory::Be => self.be,
            AccessCategory::Bk => self.bk,
        }
    }

    fn set(&mut self, ac: AccessCategory, limit: Limit) {
        match ac {
            AccessCategory::Vo => self.vo = limit,
            AccessCategory::Vi => self.vi = limit,
            AccessCategory::Be => self.be = limit,
            AccessCategory::Bk => self.bk = limit,
        }
    }
}

/// What the router reads back: the radio's limits and total threshold, and
/// one station's own limits and pending airtime when it was asked for.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Airtime {
    pub radio: Limits,
    pub threshold_us: u32,
    pub station: Option<StationAirtime>,
}

/// A station's own limits and its pending airtime per access category.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StationAirtime {
    pub limits: Limits,
    pub pending_us: Pending,
}

/// Pending estimated airtime per access category, in microseconds.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Pending {
    pub vo: u32,
    pub vi: u32,
    pub be: u32,
    pub bk: u32,
}

/// Refuse a limit the lab does not write: a zero low limit, a low limit
/// above the high one, or a high one above [`MAX_HIGH_US`].
pub fn check(limit: Limit) -> Result<()> {
    if limit.low_us == 0 || limit.low_us > limit.high_us || limit.high_us > MAX_HIGH_US {
        return Err(format!(
            "an airtime limit needs 0 < low <= high <= {MAX_HIGH_US} us, not {}/{} us",
            limit.low_us, limit.high_us
        ));
    }
    Ok(())
}

/// Set access category `ac`'s limits on the radio `radio` (its UCI name),
/// and on station `station`'s own limits when given, then read every limit
/// back. Result: what the router reads after the write; an error when a
/// written limit does not read back as written. Nothing is journaled here:
/// [`set_recorded`] journals around it.
pub fn set(
    remote: &mut dyn Remote,
    radio: &str,
    ac: AccessCategory,
    limit: Limit,
    station: Option<(&str, Mac)>,
) -> Result<Airtime> {
    check(limit)?;
    let write = format!("{} {} {}", ac.number(), limit.low_us, limit.high_us);
    let station_write = station.map(|_| format!("limit {write}"));
    let read = parse(&remote.run(&script(radio, &[write], station_write.as_slice(), station)?)?)?;
    confirm("radio's", ac, read.radio.get(ac), limit)?;
    if station.is_some() {
        let own = read
            .station
            .ok_or("the router printed no station limits after writing them")?;
        confirm("station's own", ac, own.limits.get(ac), limit)?;
    }
    Ok(read)
}

fn confirm(whose: &str, ac: AccessCategory, read: Limit, written: Limit) -> Result<()> {
    if read != written {
        return Err(format!(
            "the {whose} {} airtime limits read {}/{} us after writing {}/{} us",
            ac.label(),
            read.low_us,
            read.high_us,
            written.low_us,
            written.high_us
        ));
    }
    Ok(())
}

/// Read the radio's limits and threshold, and station `station`'s own
/// limits and pending airtime on the AP interface when given.
pub fn read(remote: &mut dyn Remote, radio: &str, station: Option<(&str, Mac)>) -> Result<Airtime> {
    parse(&remote.run(&script(radio, &[], &[], station)?)?)
}

/// Whether `radio`'s PHY has mac80211's `aql_txq_limit` debugfs file. A
/// router without it has no airtime limits to set or restore.
pub fn present(remote: &mut dyn Remote, radio: &str) -> Result<bool> {
    let answer = remote.run(&format!(
        "set -e\nphy=$(iwinfo nl80211 phyname {})\n\
         if test -e /sys/kernel/debug/ieee80211/$phy/aql_txq_limit; then echo present; else echo absent; fi\n",
        quote(radio)?
    ))?;
    match answer.trim() {
        "present" => Ok(true),
        "absent" => Ok(false),
        _ => Err("the router did not say whether its radio has airtime limits".to_owned()),
    }
}

/// Write [`DEFAULT`] to every access category of `radio` and confirm it
/// reads back for each. Nothing is journaled here: [`restore_recorded`]
/// journals around it.
pub fn restore_defaults(remote: &mut dyn Remote, radio: &str) -> Result<Airtime> {
    let writes = AccessCategory::ALL
        .iter()
        .map(|ac| format!("{} {} {}", ac.number(), DEFAULT.low_us, DEFAULT.high_us))
        .collect::<Vec<_>>();
    let read = parse(&remote.run(&script(radio, &writes, &[], None)?)?)?;
    for ac in AccessCategory::ALL {
        if read.radio.get(ac) != DEFAULT {
            return Err(format!(
                "the radio's {} airtime limits read {}/{} us after restoring {}/{} us",
                ac.label(),
                read.radio.get(ac).low_us,
                read.radio.get(ac).high_us,
                DEFAULT.low_us,
                DEFAULT.high_us
            ));
        }
    }
    Ok(read)
}

/// How far a journaled change got.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    /// About to write: nothing written yet.
    Intended,
    /// Written and read back as written.
    Confirmed,
    /// Written, or partly written, and not confirmed: the write or its
    /// read-back failed, with `reason`.
    Unconfirmed,
}

/// One access category's radio limits before and after a change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Change {
    pub ac: AccessCategory,
    pub before: Limit,
    pub after: Limit,
}

/// What a journaled change hands its journal at each phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Record<'a> {
    pub phase: Phase,
    pub changes: &'a [Change],
    /// The station's own limits were written too.
    pub station: bool,
    /// The router's end state restoring the defaults, not a lease's set.
    pub restored: bool,
    pub reason: Option<&'a str>,
}

/// Journals each phase of a change; an error refuses the phase.
pub type Recorder<'a> = &'a mut dyn FnMut(&Record<'_>) -> Result<()>;

/// [`set`], journaled: read the limits, journal the intent, write, then
/// journal the confirmed or unconfirmed result. When the intent cannot be
/// journaled nothing is written. A failed write or read-back is journaled
/// as unconfirmed before its error returns; the change is never retried.
pub fn set_recorded(
    remote: &mut dyn Remote,
    radio: &str,
    ac: AccessCategory,
    limit: Limit,
    station: Option<(&str, Mac)>,
    record: Recorder<'_>,
) -> Result<Airtime> {
    check(limit)?;
    let before = read(remote, radio, station)?;
    let changes = [Change {
        ac,
        before: before.radio.get(ac),
        after: limit,
    }];
    let entry = |phase, reason| Record {
        phase,
        changes: &changes,
        station: station.is_some(),
        restored: false,
        reason,
    };
    record(&entry(Phase::Intended, None)).map_err(|error| {
        format!("the change was not journaled, so nothing was written: {error}")
    })?;
    finish(set(remote, radio, ac, limit, station), &entry, record)
}

/// [`restore_defaults`], journaled the same way. A radio already at the
/// defaults, as read, is left unwritten and unjournaled.
pub fn restore_recorded(
    remote: &mut dyn Remote,
    radio: &str,
    record: Recorder<'_>,
) -> Result<Airtime> {
    let before = read(remote, radio, None)?;
    let changes = AccessCategory::ALL
        .into_iter()
        .filter(|ac| before.radio.get(*ac) != DEFAULT)
        .map(|ac| Change {
            ac,
            before: before.radio.get(ac),
            after: DEFAULT,
        })
        .collect::<Vec<_>>();
    if changes.is_empty() {
        return Ok(before);
    }
    let entry = |phase, reason| Record {
        phase,
        changes: &changes,
        station: false,
        restored: true,
        reason,
    };
    record(&entry(Phase::Intended, None)).map_err(|error| {
        format!("the restore was not journaled, so nothing was written: {error}")
    })?;
    finish(restore_defaults(remote, radio), &entry, record)
}

/// Journal the write's outcome: confirmed with its read-back, or
/// unconfirmed with its error, which is returned either way.
fn finish<'a>(
    outcome: Result<Airtime>,
    entry: &dyn Fn(Phase, Option<&'a str>) -> Record<'a>,
    record: Recorder<'_>,
) -> Result<Airtime> {
    match outcome {
        Ok(read) => {
            record(&entry(Phase::Confirmed, None)).map_err(|error| {
                format!(
                    "the write is confirmed, but the journal did not take the confirmation: {error}"
                )
            })?;
            Ok(read)
        }
        Err(error) => {
            // The reason outlives the entry only inside this call.
            let reason: &str = &error;
            let journaled = record(&Record {
                reason: Some(reason),
                ..entry(Phase::Unconfirmed, None)
            });
            Err(match journaled {
                Ok(()) => format!("the change is unconfirmed (journaled): {error}"),
                Err(journal) => format!(
                    "the change is unconfirmed and the journal did not take it ({journal}): {error}"
                ),
            })
        }
    }
}

/// The router's script: `writes` into the PHY's `aql_txq_limit` and
/// `station_writes` into the station's `aql`, in order, then the sections,
/// each after its `[name]` line.
fn script(
    radio: &str,
    writes: &[String],
    station_writes: &[String],
    station: Option<(&str, Mac)>,
) -> Result<String> {
    let mut text = format!(
        "set -e\nphy=$(iwinfo nl80211 phyname {})\nd=/sys/kernel/debug/ieee80211/$phy\n",
        quote(radio)?
    );
    let station_file = match station {
        Some((interface, mac)) => Some(format!(
            "\"$d/netdev:\"{}\"/stations/\"{}\"/aql\"",
            quote(interface)?,
            quote(&mac_text(mac))?
        )),
        None => None,
    };
    for write in writes {
        text.push_str(&format!("echo {} > \"$d/aql_txq_limit\"\n", quote(write)?));
    }
    if !station_writes.is_empty() {
        let file = station_file
            .as_deref()
            .ok_or("a station write names no station")?;
        for write in station_writes {
            text.push_str(&format!("echo {} > {file}\n", quote(write)?));
        }
    }
    text.push_str(
        "echo '[limits]'\ncat \"$d/aql_txq_limit\"\necho '[threshold]'\ncat \"$d/aql_threshold\"\n",
    );
    if let Some(file) = station_file {
        text.push_str(&format!("echo '[station]'\ncat {file}\n"));
    }
    Ok(text)
}

fn parse(text: &str) -> Result<Airtime> {
    let mut section = "";
    let (mut limits, mut threshold, mut station) = (Vec::new(), Vec::new(), Vec::new());
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        match line.trim() {
            "[limits]" | "[threshold]" | "[station]" => section = line.trim(),
            _ => match section {
                "[limits]" => limits.push(line),
                "[threshold]" => threshold.push(line),
                "[station]" => station.push(line),
                _ => return Err("the router printed airtime limits outside a section".to_owned()),
            },
        }
    }
    let threshold_us = match threshold.as_slice() {
        [value] => number(value.trim(), "the radio's airtime threshold")?,
        _ => return Err("the router printed no single airtime threshold".to_owned()),
    };
    Ok(Airtime {
        radio: parse_radio(&limits)?,
        threshold_us,
        station: if station.is_empty() {
            None
        } else {
            Some(parse_station(&station)?)
        },
    })
}

/// `aql_txq_limit`: a header line, then `<AC> <low> <high>` for VO, VI, BE
/// and BK, then a `BC/MC <limit>` line this reader leaves alone.
fn parse_radio(lines: &[&str]) -> Result<Limits> {
    let mut found = Found::default();
    for line in lines {
        let words = line.split_whitespace().collect::<Vec<_>>();
        let Some(ac) = AccessCategory::ALL
            .into_iter()
            .find(|ac| words.first() == Some(&ac.label()))
        else {
            continue;
        };
        let [_, low, high] = words.as_slice() else {
            return Err(format!(
                "the radio's {} airtime limits are not two numbers",
                ac.label()
            ));
        };
        let limit = Limit {
            low_us: number(low, "a low airtime limit")?,
            high_us: number(high, "a high airtime limit")?,
        };
        if found.slot(ac).replace(limit).is_some() {
            return Err(format!(
                "the radio printed {} airtime limits twice",
                ac.label()
            ));
        }
    }
    let mut limits = Limits {
        vo: DEFAULT,
        vi: DEFAULT,
        be: DEFAULT,
        bk: DEFAULT,
    };
    for ac in AccessCategory::ALL {
        let limit = found
            .slot(ac)
            .ok_or_else(|| format!("the radio printed no {} airtime limits", ac.label()))?;
        limits.set(ac, limit);
    }
    Ok(limits)
}

/// The limits read so far, per access category.
#[derive(Default)]
struct Found {
    vo: Option<Limit>,
    vi: Option<Limit>,
    be: Option<Limit>,
    bk: Option<Limit>,
}

impl Found {
    fn slot(&mut self, ac: AccessCategory) -> &mut Option<Limit> {
        match ac {
            AccessCategory::Vo => &mut self.vo,
            AccessCategory::Vi => &mut self.vi,
            AccessCategory::Be => &mut self.be,
            AccessCategory::Bk => &mut self.bk,
        }
    }
}

/// The station's `aql`: `Q depth: VO: <n> us VI: <n> us BE: <n> us BK: <n>
/// us` and `Q limit[low/high]: VO: <l>/<h> VI: … BK: <l>/<h>`.
fn parse_station(lines: &[&str]) -> Result<StationAirtime> {
    let (mut pending, mut limits) = (None, None);
    for line in lines {
        if let Some(rest) = line.trim().strip_prefix("Q depth:") {
            let [vo, vi, be, bk] =
                per_category(rest, |value| number(value, "a station's pending airtime"))?;
            pending = Some(Pending { vo, vi, be, bk });
        } else if let Some(rest) = line.trim().strip_prefix("Q limit[low/high]:") {
            let values = per_category(rest, |value| {
                let (low, high) = value
                    .split_once('/')
                    .ok_or_else(|| "a station's airtime limit is not low/high".to_owned())?;
                Ok(Limit {
                    low_us: number(low, "a station's low airtime limit")?,
                    high_us: number(high, "a station's high airtime limit")?,
                })
            })?;
            let [vo, vi, be, bk] = values;
            limits = Some(Limits { vo, vi, be, bk });
        }
    }
    match (pending, limits) {
        (Some(pending_us), Some(limits)) => Ok(StationAirtime { limits, pending_us }),
        _ => Err("the station's airtime file names no pending airtime or no limits".to_owned()),
    }
}

/// The four values after `VO:`, `VI:`, `BE:` and `BK:` in that order; a
/// trailing `us` is dropped.
fn per_category<T: Copy>(text: &str, value: impl Fn(&str) -> Result<T>) -> Result<[T; 4]> {
    let words = text
        .split_whitespace()
        .filter(|word| *word != "us")
        .collect::<Vec<_>>();
    let [vo, a, vi, b, be, c, bk, d] = words.as_slice() else {
        return Err("a station's airtime line does not name four access categories".to_owned());
    };
    for (label, ac) in [(vo, "VO:"), (vi, "VI:"), (be, "BE:"), (bk, "BK:")] {
        if *label != ac {
            return Err(format!(
                "a station's airtime line names `{label}` for `{ac}`"
            ));
        }
    }
    Ok([value(a)?, value(b)?, value(c)?, value(d)?])
}

fn number(text: &str, what: &str) -> Result<u32> {
    text.parse()
        .map_err(|_| format!("{what} is not a number: `{text}`"))
}

#[cfg(test)]
mod tests;
