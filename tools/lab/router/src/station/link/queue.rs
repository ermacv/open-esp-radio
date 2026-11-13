//! The parse of the AP's two `aqm` debugfs files into the client
//! protocol's queue record (`oer_tools_lab_client::protocol::station_link::
//! queue`, where each value's meaning and SOURCE are named).
//!
//! Every field is found by the name the file's own header line gives it,
//! never by its position. A column or name this parser does not know stays
//! in the verbatim lines and is listed as unknown; a known one the file
//! lacks is listed as missing. Neither becomes a value: it is unavailable,
//! never zero.

use std::collections::BTreeMap;

use oer_tools_lab_client::protocol::station_link::queue::{
    Counts, Observed, RadioEntry, RadioQueues, Raw, Scope, StationQueues, TidQueue,
};

use crate::ssh::Result;

/// The station file's columns this parser knows, as its header names them.
const STATION_COLUMNS: [&str; 12] = [
    "tid",
    "ac",
    "backlog-bytes",
    "backlog-packets",
    "new-flows",
    "drops",
    "marks",
    "overlimit",
    "collisions",
    "tx-bytes",
    "tx-packets",
    "flags",
];

/// The radio file's names this parser knows, and what each counts.
const RADIO_NAMES: [(&str, Counts); 9] = [
    ("fq_flows_cnt", Counts::RadioFlowSlots),
    ("fq_backlog", Counts::RadioQueuedPackets),
    ("fq_overlimit", Counts::RadioOvermemoryDropOperations),
    ("fq_overmemory", Counts::RadioDropOperations),
    ("fq_collisions", Counts::RadioCollidedEnqueues),
    ("fq_memory_usage", Counts::RadioMemoryUsage),
    ("fq_memory_limit", Counts::RadioMemoryLimit),
    ("fq_limit", Counts::RadioPacketLimit),
    ("fq_quantum", Counts::RadioQuantum),
];

/// The longest text the phy's `aqm` file gives: mac80211 prints it into a
/// 200-byte buffer with `scnprintf`, which keeps 199 characters and cuts
/// the rest, a row's number included.
///
/// SOURCE: Linux `net/mac80211/debugfs.c::aqm_read`, `char buf[200]`.
const RADIO_TEXT_MAX: usize = 200 - 1;

/// The longest text the station's `aqm` file gives: one buffer of
/// `AQM_TXQ_ENTRY_LEN * (IEEE80211_NUM_TIDS + 2)` bytes filled by
/// `scnprintf`. Its 17 TID queues' rows at their widest (ten digits per
/// counter, every flag) take 135 characters each beside the 108 of its
/// header, more than the buffer holds: the file can be cut too.
///
/// SOURCE: Linux `net/mac80211/debugfs_sta.c::sta_aqm_read`,
/// `AQM_TXQ_ENTRY_LEN` 130, `IEEE80211_NUM_TIDS` 16
/// (`include/linux/ieee80211.h`), `txq[IEEE80211_NUM_TIDS + 1]`
/// (`include/net/mac80211.h`).
const STATION_TEXT_MAX: usize = 130 * (16 + 2) - 1;

/// Whether a file's text, `header` and `rows` as the kernel printed them,
/// filled the kernel's buffer of `max` characters: a whole file ends with
/// its last row's newline, so only a cut one reaches `max` without it, and
/// its last row is then cut.
fn filled(header: &str, rows: &[&str], max: usize) -> bool {
    let printed = rows.iter().fold(header.len(), |length, row| {
        length.saturating_add(row.len() + 1)
    });
    !rows.is_empty() && printed == max
}

/// The router's boot clock as `/proc/uptime`'s first field prints it,
/// seconds with two decimals (`508749.69`), in milliseconds: 10 ms steps.
///
/// SOURCE: Linux `fs/proc/uptime.c::uptime_proc_show`, `"%lu.%02lu"` of
/// `ktime_get_boottime_ts64`, the clock of `iw`'s `associated at
/// [boottime]`.
pub fn parse_clock(text: &str) -> Result<u64> {
    let invalid = || format!("the router's clock is unreadable: `{text}`");
    let (seconds, hundredths) = text.trim().split_once('.').ok_or_else(invalid)?;
    if hundredths.len() != 2 || !hundredths.bytes().all(|digit| digit.is_ascii_digit()) {
        return Err(invalid());
    }
    let seconds = seconds.parse::<u64>().map_err(|_| invalid())?;
    let hundredths = hundredths.parse::<u64>().map_err(|_| invalid())?;
    seconds
        .checked_mul(1000)
        .and_then(|ms| ms.checked_add(hundredths * 10))
        .ok_or_else(invalid)
}

/// A section's `clock: <uptime>` line, then the file's header and rows.
fn split_read<'a>(file: &str, lines: &[&'a str]) -> Result<Option<(u64, &'a str, Vec<&'a str>)>> {
    let Some((clock, rest)) = lines.split_first() else {
        return Ok(None);
    };
    let clock = clock
        .trim()
        .strip_prefix("clock:")
        .ok_or_else(|| format!("the router's {file} read has no clock"))?;
    let observed_at_ms = parse_clock(clock)?;
    let (header, rows) = rest
        .split_first()
        .ok_or_else(|| format!("the router's {file} file has no header"))?;
    if rows.iter().any(|row| row.trim().starts_with("clock:")) {
        return Err(format!("the router read more than one {file} file"));
    }
    Ok(Some((observed_at_ms, header, rows.to_vec())))
}

/// The header's column names; each once.
fn columns<'a>(file: &str, header: &'a str) -> Result<Vec<&'a str>> {
    let columns = header.split_whitespace().collect::<Vec<_>>();
    for (at, column) in columns.iter().enumerate() {
        if columns.iter().skip(at + 1).any(|other| other == column) {
            return Err(format!("the router's {file} header names `{column}` twice"));
        }
    }
    Ok(columns)
}

/// A row's fields: words, a parenthesised group with its spaces as one
/// (`0x6(RUN AMPDU NO-AMSDU)`).
fn fields(row: &str) -> Option<Vec<String>> {
    let mut fields = Vec::new();
    let mut open: Option<String> = None;
    for word in row.split_whitespace() {
        let field = match open.take() {
            Some(mut group) => {
                group.push(' ');
                group.push_str(word);
                group
            }
            None => word.to_owned(),
        };
        if field.matches('(').count() > field.matches(')').count() {
            open = Some(field);
        } else {
            fields.push(field);
        }
    }
    open.is_none().then_some(fields)
}

fn number<T: std::str::FromStr>(file: &str, column: &str, text: &str) -> Result<T> {
    text.parse()
        .map_err(|_| format!("the router's {file} `{column}` is not a number: `{text}`"))
}

/// Parse the station's `aqm` section: absent when the router had no
/// readable file. A row that does not match its header, a TID twice or a
/// known column that reads no number refuses the observation.
pub fn parse_station(lines: &[&str]) -> Result<Option<StationQueues>> {
    const FILE: &str = "station aqm";
    let Some((observed_at_ms, header, rows)) = split_read(FILE, lines)? else {
        return Ok(None);
    };
    let columns = columns(FILE, header)?;
    if !columns.contains(&"tid") {
        return Err("the router's station aqm header names no `tid`".into());
    }
    let cut_row = filled(header, &rows, STATION_TEXT_MAX)
        .then(|| rows.last().map(|row| (*row).to_owned()))
        .flatten();
    let whole = match (&cut_row, rows.split_last()) {
        (Some(_), Some((_, whole))) => whole,
        _ => &rows[..],
    };
    let mut tids: Vec<TidQueue> = Vec::new();
    for row in whole {
        let fields = fields(row)
            .filter(|fields| fields.len() == columns.len())
            .ok_or_else(|| {
                format!("the router's station aqm row does not match its header: `{row}`")
            })?;
        let tid = columns
            .iter()
            .zip(&fields)
            .find(|(column, _)| **column == "tid")
            .map(|(_, text)| number::<u8>(FILE, "tid", text))
            .ok_or("the router's station aqm row has no `tid`")??;
        if tids.iter().any(|queue| queue.tid == tid) {
            return Err(format!("the router's station aqm names TID {tid} twice"));
        }
        let scope = Scope::StationTid { tid };
        let value = |counts, column: &str, text: &str| -> Result<Option<Observed>> {
            Ok(Some(Observed::new(
                number(FILE, column, text)?,
                counts,
                scope,
                observed_at_ms,
            )))
        };
        let mut queue = TidQueue {
            tid,
            ac: None,
            backlog_bytes: None,
            backlog_packets: None,
            new_flows: None,
            drops: None,
            marks: None,
            overlimit: None,
            collisions: None,
            tx_bytes: None,
            tx_packets: None,
            flags: None,
            unknown: BTreeMap::new(),
        };
        for (column, text) in columns.iter().copied().zip(&fields) {
            match column {
                "tid" => {}
                "ac" => queue.ac = Some(number(FILE, column, text)?),
                "backlog-bytes" => queue.backlog_bytes = value(Counts::QueuedBytes, column, text)?,
                "backlog-packets" => {
                    queue.backlog_packets = value(Counts::QueuedPackets, column, text)?;
                }
                "new-flows" => queue.new_flows = value(Counts::FlowActivations, column, text)?,
                "drops" => queue.drops = value(Counts::CodelDrops, column, text)?,
                "marks" => queue.marks = value(Counts::EcnMarks, column, text)?,
                "overlimit" => {
                    queue.overlimit = value(Counts::OverlimitDropOperations, column, text)?;
                }
                "collisions" => queue.collisions = value(Counts::CollidedEnqueues, column, text)?,
                "tx-bytes" => queue.tx_bytes = value(Counts::DequeuedBytes, column, text)?,
                "tx-packets" => queue.tx_packets = value(Counts::DequeuedPackets, column, text)?,
                "flags" => queue.flags = Some(text.clone()),
                unknown => {
                    queue.unknown.insert(unknown.to_owned(), text.clone());
                }
            }
        }
        tids.push(queue);
    }
    Ok(Some(StationQueues {
        raw: Raw {
            observed_at_ms,
            header: (*header).to_owned(),
            lines: rows.iter().map(|row| (*row).to_owned()).collect(),
        },
        unknown_columns: columns
            .iter()
            .filter(|column| !STATION_COLUMNS.contains(column))
            .map(|column| (*column).to_owned())
            .collect(),
        missing_columns: STATION_COLUMNS
            .iter()
            .filter(|column| !columns.contains(column))
            .map(|column| (*column).to_owned())
            .collect(),
        cut_row,
        tids,
    }))
}

/// Parse the radio's `aqm` section: absent when the router had no
/// readable file. A row that does not match its header, a name twice or a
/// known name whose value reads no number refuses the observation.
pub fn parse_radio(lines: &[&str]) -> Result<Option<RadioQueues>> {
    const FILE: &str = "radio aqm";
    let Some((observed_at_ms, header, rows)) = split_read(FILE, lines)? else {
        return Ok(None);
    };
    let columns = columns(FILE, header)?;
    let position = |name: &str| columns.iter().position(|column| *column == name);
    let name_at = position("name").ok_or("the router's radio aqm header names no `name`")?;
    let (access_at, value_at) = (position("access"), position("value"));
    let cut = filled(header, &rows, RADIO_TEXT_MAX)
        .then(|| rows.last().copied())
        .flatten();
    let whole = match (cut, rows.split_last()) {
        (Some(_), Some((_, whole))) => whole,
        _ => &rows[..],
    };
    let mut values = BTreeMap::new();
    let mut unknown_names = Vec::new();
    for row in whole {
        let fields = row.split_whitespace().collect::<Vec<_>>();
        if fields.len() != columns.len() {
            return Err(format!(
                "the router's radio aqm row does not match its header: `{row}`"
            ));
        }
        let field = |at: Option<usize>| at.and_then(|at| fields.get(at)).copied();
        let name = field(Some(name_at)).ok_or("the router's radio aqm row has no name")?;
        let counts = RADIO_NAMES
            .iter()
            .find(|(known, _)| *known == name)
            .map(|(_, counts)| *counts);
        let value = match (counts, field(value_at)) {
            (Some(counts), Some(text)) => Some(Observed::new(
                number(FILE, name, text)?,
                counts,
                Scope::Radio,
                observed_at_ms,
            )),
            _ => None,
        };
        if counts.is_none() {
            unknown_names.push(name.to_owned());
        }
        let entry = RadioEntry {
            access: field(access_at).map(str::to_owned),
            value,
        };
        if values.insert(name.to_owned(), entry).is_some() {
            return Err(format!("the router's radio aqm names `{name}` twice"));
        }
    }
    // The cut row's name: the known names its text could begin, or the text
    // itself. None of them is missing: the file named it, the buffer cut it.
    let cut_names: Vec<String> = cut
        .map(|row| {
            let partial = row.split_whitespace().nth(name_at).unwrap_or_default();
            let known = RADIO_NAMES
                .iter()
                .map(|(name, _)| *name)
                .filter(|name| !partial.is_empty() && name.starts_with(partial))
                .filter(|name| !values.contains_key(*name))
                .map(str::to_owned)
                .collect::<Vec<_>>();
            if known.is_empty() {
                vec![row.trim().to_owned()]
            } else {
                known
            }
        })
        .unwrap_or_default();
    Ok(Some(RadioQueues {
        raw: Raw {
            observed_at_ms,
            header: (*header).to_owned(),
            lines: rows.iter().map(|row| (*row).to_owned()).collect(),
        },
        unknown_columns: columns
            .iter()
            .filter(|column| !["access", "name", "value"].contains(column))
            .map(|column| (*column).to_owned())
            .collect(),
        unknown_names,
        missing_names: RADIO_NAMES
            .iter()
            .filter(|(name, _)| {
                !values.contains_key(*name) && !cut_names.iter().any(|cut| cut == name)
            })
            .map(|(name, _)| (*name).to_owned())
            .collect(),
        cut_names,
        values,
    }))
}

#[cfg(test)]
mod tests;
