//! Function-level comparison of two revisions of a vendor archive.
//!
//! Every function of the old revision is classified against the new one by
//! its relocation-normalized code fingerprint: unchanged under its name,
//! renamed (the same code under another name, as obfuscated releases do),
//! changed under its name, or removed, with the most similar unmatched new
//! function as a candidate. New functions that match nothing are added.
use crate::Result;
use crate::fingerprint::{Function, functions, similarity_ppm};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Candidates for a changed or removed function must be within this length
/// ratio of it, in instructions.
const LENGTH_RATIO: f64 = 2.0;

#[derive(Debug, PartialEq)]
pub enum Status {
    Unchanged,
    /// Same code; only external target names differ.
    ReferencesRenamed,
    Renamed {
        new: String,
    },
    Changed {
        similarity: f64,
    },
    Removed {
        closest: Option<(String, f64)>,
    },
    Added,
}

#[derive(Debug)]
pub struct Entry {
    pub name: String,
    pub member: String,
    pub status: Status,
}

/// Candidates whose instruction multisets overlap most are the only ones
/// compared by the costly ordered similarity.
const CANDIDATES: usize = 8;

/// Shared instructions of two sorted token multisets.
fn overlap(a: &[u64], b: &[u64]) -> usize {
    let mut others = b.iter().peekable();
    a.iter()
        .filter(|token| {
            // What `b` holds below this token is shared with nothing later.
            while others.next_if(|other| other < token).is_some() {}
            others.next_if_eq(token).is_some()
        })
        .count()
}

/// Similarity of two token sequences as a fraction.
fn similarity(a: &[u64], b: &[u64]) -> f64 {
    f64::from(similarity_ppm(a, b)) / 1e6
}

fn sorted(tokens: &[u64]) -> Vec<u64> {
    let mut tokens = tokens.to_vec();
    tokens.sort_unstable();
    tokens
}

fn closest<'a>(
    old: &Function,
    candidates: impl Iterator<Item = (&'a Function, &'a [u64])>,
) -> Option<(String, f64)> {
    let length = old.tokens.len() as f64;
    let multiset = sorted(&old.tokens);
    let mut ranked: Vec<(usize, &Function)> = candidates
        .filter(|(f, _)| {
            let other = f.tokens.len() as f64;
            other <= length * LENGTH_RATIO && length <= other * LENGTH_RATIO
        })
        .map(|(f, other)| (overlap(&multiset, other), f))
        .collect();
    ranked.sort_by_key(|(shared, _)| std::cmp::Reverse(*shared));
    ranked
        .into_iter()
        .take(CANDIDATES)
        .map(|(_, f)| (f.name.clone(), similarity(&old.tokens, &f.tokens)))
        .max_by(|a, b| a.1.total_cmp(&b.1))
}

/// Classify every function of `old` against `new`.
pub fn compare(old: &[Function], new: &[Function]) -> Vec<Entry> {
    let mut new_by_name: BTreeMap<&str, Vec<&Function>> = BTreeMap::new();
    let mut new_by_code: BTreeMap<&str, Vec<&Function>> = BTreeMap::new();
    for f in new {
        new_by_name.entry(&f.name).or_default().push(f);
        new_by_code.entry(&f.code).or_default().push(f);
    }
    let old_names: BTreeSet<&str> = old.iter().map(|f| f.name.as_str()).collect();
    let mut matched: BTreeSet<(&str, &str)> = BTreeSet::new();
    // Every old function with the status its name or its code gives it in
    // the new revision; one that has neither is removed, and its closest
    // candidate is looked for once every match is known.
    let mut found: Vec<(&Function, Option<Status>)> = vec![];
    for f in old {
        let same_name = new_by_name.get(f.name.as_str());
        let status =
            if let Some(same) = same_name.and_then(|list| list.iter().find(|n| n.code == f.code)) {
                matched.insert((&same.member, &same.name));
                Some(if same.named == f.named {
                    Status::Unchanged
                } else {
                    Status::ReferencesRenamed
                })
            } else if let Some(best) = same_name.and_then(|list| {
                list.iter().max_by(|a, b| {
                    similarity(&f.tokens, &a.tokens).total_cmp(&similarity(&f.tokens, &b.tokens))
                })
            }) {
                matched.insert((&best.member, &best.name));
                Some(Status::Changed {
                    similarity: similarity(&f.tokens, &best.tokens),
                })
            } else {
                new_by_code
                    .get(f.code.as_str())
                    .and_then(|list| list.iter().find(|n| !old_names.contains(n.name.as_str())))
                    .map(|renamed| {
                        matched.insert((&renamed.member, &renamed.name));
                        Status::Renamed {
                            new: renamed.name.clone(),
                        }
                    })
            };
        found.push((f, status));
    }
    let unmatched: Vec<&Function> = new
        .iter()
        .filter(|f| !matched.contains(&(f.member.as_str(), f.name.as_str())))
        .filter(|f| !old_names.contains(f.name.as_str()))
        .collect();
    let multisets: Vec<Vec<u64>> = unmatched.iter().map(|f| sorted(&f.tokens)).collect();
    let removed = |f: &Function| Status::Removed {
        closest: closest(
            f,
            unmatched
                .iter()
                .copied()
                .zip(multisets.iter().map(Vec::as_slice)),
        ),
    };
    let entry = |f: &Function, status| Entry {
        name: f.name.clone(),
        member: f.member.clone(),
        status,
    };
    found
        .into_iter()
        .map(|(f, status)| entry(f, status.unwrap_or_else(|| removed(f))))
        .chain(unmatched.iter().map(|f| entry(f, Status::Added)))
        .collect()
}

fn render(entries: &[Entry], all: bool) {
    for entry in entries {
        let line = match &entry.status {
            Status::Unchanged if !all => continue,
            Status::Unchanged => "unchanged".to_owned(),
            Status::ReferencesRenamed => "references-renamed".to_owned(),
            Status::Renamed { new } => format!("renamed\t{new}"),
            Status::Changed { similarity } => format!("changed\tsimilarity {similarity:.2}"),
            Status::Removed {
                closest: Some((name, score)),
            } => {
                format!("removed\tclosest {name} similarity {score:.2}")
            }
            Status::Removed { closest: None } => "removed".to_owned(),
            Status::Added => "added".to_owned(),
        };
        println!("{}\t{}\t{line}", entry.member, entry.name);
    }
}

/// The number of entries of each status that occurs, in the order of the
/// statuses' names.
fn summary(entries: &[Entry]) -> String {
    let name = |status: &Status| match status {
        Status::Unchanged => "unchanged",
        Status::ReferencesRenamed => "references-renamed",
        Status::Renamed { .. } => "renamed",
        Status::Changed { .. } => "changed",
        Status::Removed { .. } => "removed",
        Status::Added => "added",
    };
    let names: BTreeSet<&str> = entries.iter().map(|entry| name(&entry.status)).collect();
    names
        .iter()
        .map(|kind| {
            let count = entries
                .iter()
                .filter(|entry| name(&entry.status) == *kind)
                .count();
            format!("{kind}={count}")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Compare `old` with `new`, or every pinned artifact of `chip` with its
/// namesake in `baseline`.
pub fn run(
    root: &Path,
    chip: &str,
    old: Option<PathBuf>,
    new: Option<PathBuf>,
    baseline: Option<PathBuf>,
    all: bool,
) -> Result<()> {
    let pairs: Vec<(String, PathBuf, PathBuf)> = match (old, new, baseline) {
        (Some(old), Some(new), None) => vec![(new.display().to_string(), old, new)],
        (None, None, Some(baseline)) => oer_tools_pins_artifacts::artifacts::pinned(root, chip)?
            .into_iter()
            .filter(|a| !a.firmware)
            .filter_map(|a| {
                let name = a.path.file_name()?.to_owned();
                let old = baseline.join(name);
                let binary = std::fs::read(&a.path)
                    .is_ok_and(|bytes| oer_tools_analysis_elf::is_binary(&bytes));
                (binary && old.is_file()).then_some((a.id, old, a.path))
            })
            .collect(),
        _ => return Err("pass --old and --new, or --baseline".into()),
    };
    for (id, old, new) in pairs {
        let entries = compare(&read(&old)?, &read(&new)?);
        println!("# {id}: {}", summary(&entries));
        render(&entries, all);
    }
    Ok(())
}

/// Every function of the archive or ELF at `path`, fingerprinted.
pub fn read(path: &Path) -> Result<Vec<Function>> {
    Ok(functions(
        &std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?,
    )?)
}

#[cfg(test)]
mod tests;
