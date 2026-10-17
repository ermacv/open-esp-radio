//! Append-only JSON Lines logs: one record per line, and a log trimmed to
//! its newest records once it grows past a size.
//!
//! Two readers with distinct promises: [`read_strict`] for records that
//! decide something (what a board carries, who owns what), which fails on
//! a line it cannot parse, because skipping a damaged newest record would
//! return an older "known" state; and [`read_lossy`] for telemetry and
//! history views, which skips such lines (a newer writer's records).

use std::{fs, io::Write as _, path::Path};

use serde::{Deserialize, Serialize};

/// Append `value` as one line of `path`.
pub fn append(path: &Path, value: &impl Serialize) -> crate::Result<()> {
    let mut line = serde_json::to_vec(value)?;
    line.push(b'\n');
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(&line)?;
    Ok(())
}

/// Every record of `path`, for a decision; none when it does not exist. A
/// line that does not parse is an error naming it.
pub fn read_strict<T: for<'a> Deserialize<'a>>(path: &Path) -> crate::Result<Vec<T>> {
    let Some(text) = text(path)? else {
        return Ok(Vec::new());
    };
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(index, line)| {
            serde_json::from_str(line).map_err(|error| {
                format!(
                    "{}:{}: unreadable record: {error}",
                    path.display(),
                    index + 1
                )
                .into()
            })
        })
        .collect()
}

/// Every readable record of `path`, for a telemetry or history view; none
/// when it does not exist. Lines that do not parse are skipped. Never use
/// it for a record that decides something: see [`read_strict`].
pub fn read_lossy<T: for<'a> Deserialize<'a>>(path: &Path) -> crate::Result<Vec<T>> {
    Ok(text(path)?
        .map(|text| {
            text.lines()
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect()
        })
        .unwrap_or_default())
}

fn text(path: &Path) -> crate::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// Keep only the newest `retained` readable records of `path`; trimming
/// drops lines this build cannot read.
pub fn retain_newest<T: for<'a> Deserialize<'a> + Serialize>(
    path: &Path,
    retained: usize,
) -> crate::Result<()> {
    let records = read_lossy::<T>(path)?;
    let start = records.len().saturating_sub(retained);
    let mut text = Vec::new();
    for record in &records[start..] {
        text.extend(serde_json::to_vec(record)?);
        text.push(b'\n');
    }
    crate::atomic_write(path, &text)
}
