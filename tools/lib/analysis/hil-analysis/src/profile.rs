//! Symbolized reports of a repetition's program-counter profile, through
//! the one DWARF symbolizer, `oer_elf::dwarf`.
//!
//! A capture that armed a profile leaves `profile.json`: the request, the
//! target's status and each hart's raw `(pc, ra)` samples. The report
//! names each sample by the function containing it in the run's own image,
//! the outermost frame of the inline chain at its program counter, and for
//! the most sampled functions the innermost inlined frame and the callers
//! the return addresses name. A return address names the caller only while
//! the sampled function has not made a call of its own, so it is exact for
//! leaf functions.

use std::{collections::BTreeMap, path::Path};

use oer_elf::dwarf::Symbolizer;
use serde::Deserialize;

use crate::Result;

#[derive(Deserialize)]
struct Record {
    status: Option<oer_hil_protocol::telemetry::ProfileStatus>,
    #[serde(default)]
    samples: Vec<Vec<(u32, u32)>>,
    error: Option<String>,
}

/// One address's frames, innermost first.
fn frames(symbolizer: Option<&Symbolizer>, address: u32) -> Vec<String> {
    symbolizer
        .and_then(|symbolizer| symbolizer.frames(u64::from(address)).ok())
        .into_iter()
        .flatten()
        .filter_map(|frame| frame.function)
        .collect()
}

/// The function containing `address`: the outermost frame of its inline
/// chain, or the address when the image does not describe it.
fn containing(loader: Option<&Symbolizer>, address: u32) -> String {
    frames(loader, address)
        .pop()
        .unwrap_or_else(|| format!("{address:#010x}"))
}

/// The report of `profile.json` at `path`, symbolized against the ELF bytes
/// `elf`, with the `top` most sampled functions of each hart.
pub fn report(path: &Path, elf: Option<&[u8]>, top: usize) -> Result<String> {
    let record: Record = serde_json::from_slice(&std::fs::read(path)?)?;
    if let Some(error) = record.error {
        return Ok(format!("profile not drained: {error}\n"));
    }
    let status = record.status.ok_or("profile.json has no status")?;
    if status.open {
        return Ok("profile window still open when drained: no samples\n".into());
    }
    let loader = elf.and_then(|elf| Symbolizer::new(elf).ok());
    let loader = loader.as_ref();
    let mut text = String::new();
    if loader.is_none() {
        text.push_str("the run's image is not available: addresses are not symbolized\n");
    }
    for (hart, samples) in record.samples.iter().enumerate() {
        if samples.is_empty() {
            continue;
        }
        text.push_str(&format!(
            "hart {hart}: {} samples over {:.2} s every {} us, {} dropped\n",
            samples.len(),
            f64::from(status.window_us) / 1e6,
            status.period_us,
            status.overflow.get(hart).copied().unwrap_or(0)
        ));
        let mut functions = BTreeMap::<String, Vec<(u32, u32)>>::new();
        for &(pc, ra) in samples {
            functions
                .entry(containing(loader, pc))
                .or_default()
                .push((pc, ra));
        }
        let mut ranked = functions.into_iter().collect::<Vec<_>>();
        ranked.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(&b.0)));
        for (function, hits) in ranked.iter().take(top) {
            let share = hits.len() as f64 * 100.0 / samples.len() as f64;
            text.push_str(&format!("  {share:5.1}% {:>7}  {function}\n", hits.len()));
            let inner = most_common(hits.iter().filter_map(|&(pc, _)| {
                let chain = frames(loader, pc);
                (chain.len() > 1).then(|| chain[0].clone())
            }));
            if let Some((name, count)) = inner {
                text.push_str(&format!("                  inlined: {name} ({count})\n"));
            }
            let callers = ranked_names(hits.iter().map(|&(_, ra)| containing(loader, ra)));
            if let Some(callers) = callers {
                text.push_str(&format!("                  called from: {callers}\n"));
            }
        }
    }
    Ok(text)
}

fn most_common(names: impl Iterator<Item = String>) -> Option<(String, usize)> {
    let mut counts = BTreeMap::<String, usize>::new();
    for name in names {
        *counts.entry(name).or_default() += 1;
    }
    counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
}

/// The three most frequent names with their shares, or `None` for none.
fn ranked_names(names: impl Iterator<Item = String>) -> Option<String> {
    let mut counts = BTreeMap::<String, usize>::new();
    let mut total = 0;
    for name in names {
        *counts.entry(name).or_default() += 1;
        total += 1;
    }
    let mut ranked = counts.into_iter().collect::<Vec<_>>();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    (!ranked.is_empty()).then(|| {
        ranked
            .iter()
            .take(3)
            .map(|(name, count)| format!("{name} {:.0}%", *count as f64 * 100.0 / total as f64))
            .collect::<Vec<_>>()
            .join(", ")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(value: serde_json::Value) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), value.to_string()).unwrap();
        file
    }

    fn status(open: bool) -> serde_json::Value {
        serde_json::json!({"armed": true, "open": open, "harts": "Both", "period_us": 1999,
            "window_us": 2_000_000, "capacity": 8192, "samples": [3, 0], "overflow": [1, 0]})
    }

    #[test]
    fn samples_are_ranked_by_function_with_their_callers() {
        let file = write(serde_json::json!({"schema": 1, "status": status(false),
            "samples": [[[16, 32], [16, 32], [48, 64]], []]}));
        let text = report(file.path(), None, 5).unwrap();
        assert!(
            text.contains("hart 0: 3 samples over 2.00 s every 1999 us, 1 dropped"),
            "{text}"
        );
        assert!(text.contains(" 66.7%       2  0x00000010"), "{text}");
        assert!(text.contains("called from: 0x00000020 100%"), "{text}");
        assert!(!text.contains("hart 1"), "{text}");
    }

    #[test]
    fn an_undrained_or_open_profile_says_so() {
        let failed = write(serde_json::json!({"schema": 1, "error": "link ended"}));
        assert_eq!(
            report(failed.path(), None, 5).unwrap(),
            "profile not drained: link ended\n"
        );
        let open = write(serde_json::json!({"schema": 1, "status": status(true), "samples": []}));
        assert!(report(open.path(), None, 5).unwrap().contains("still open"));
    }
}
