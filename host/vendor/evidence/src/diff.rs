//! What two versions of one evidence shard say differently.
//!
//! A shard's source digests and line numbers change with every edit of a
//! recorded file, while its verdicts, case counts, coverage and untriaged
//! locations change only when the comparison did. The summary reports the
//! second kind line by line and counts the first, so a reviewer sees whether
//! a regeneration changed evidence or only recorded that sources moved.
use oer_vendor_evidence_shard::{Entry, Index, Location, SourceLine};
use std::collections::{BTreeMap, BTreeSet};

/// One difference between the old and new shard.
#[derive(Debug, PartialEq, Eq)]
pub enum Difference {
    /// A claim's verdict, case count, coverage, observation or state.
    Entry {
        entry: String,
        field: &'static str,
        old: String,
        new: String,
    },
    /// A claim only one version lists.
    EntryOnly { entry: String, new: bool },
    /// An untriaged location only one version lists.
    Untriaged { location: String, new: bool },
    /// An unprojected state range only one version lists.
    Unprojected { range: String, new: bool },
    /// A recorded source only one version lists.
    Source { path: String, new: bool },
    /// Recorded sources whose digests changed.
    Digests(Vec<String>),
    /// The probe data the executions read changed.
    ReadData,
    /// Unobserved and observed production lines moved or changed, by file.
    Lines {
        path: String,
        old: usize,
        new: usize,
    },
}

impl std::fmt::Display for Difference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let only = |new: bool| if new { "added" } else { "removed" };
        match self {
            Difference::Entry {
                entry,
                field,
                old,
                new,
            } => write!(f, "{entry}: {field} {old} -> {new}"),
            Difference::EntryOnly { entry, new } => write!(f, "{entry}: claim {}", only(*new)),
            Difference::Untriaged { location, new } => {
                write!(f, "untriaged {location}: {}", only(*new))
            }
            Difference::Unprojected { range, new } => {
                write!(f, "unprojected {range}: {}", only(*new))
            }
            Difference::Source { path, new } => write!(f, "source {path}: {}", only(*new)),
            Difference::Digests(paths) => write!(f, "source digests changed: {}", paths.join(", ")),
            Difference::ReadData => write!(f, "the probe data the executions read changed"),
            Difference::Lines { path, old, new } => write!(
                f,
                "{path}: observed and unobserved lines changed ({old} -> {new} unobserved)"
            ),
        }
    }
}

fn entry_key(entry: &Entry) -> String {
    format!(
        "{}/{}::{} -> {}",
        entry.suite, entry.source, entry.symbol, entry.production
    )
}

fn location_key(location: &Location) -> String {
    format!(
        "{}+{:#x} {:?}",
        location.function, location.offset, location.kind
    )
}

/// The differences between `old` and `new`, evidence first.
pub fn differences(old: &Index, new: &Index) -> Vec<Difference> {
    let mut found = vec![];
    let entries = |index: &Index| -> BTreeMap<String, Entry> {
        index
            .entries
            .iter()
            .map(|e| (entry_key(e), e.clone()))
            .collect()
    };
    let (old_entries, new_entries) = (entries(old), entries(new));
    for (key, before) in &old_entries {
        let Some(after) = new_entries.get(key) else {
            found.push(Difference::EntryOnly {
                entry: key.clone(),
                new: false,
            });
            continue;
        };
        let mut field = |name: &'static str, a: String, b: String| {
            if a != b {
                found.push(Difference::Entry {
                    entry: key.clone(),
                    field: name,
                    old: a,
                    new: b,
                });
            }
        };
        field("verdict", before.verdict.clone(), after.verdict.clone());
        field("cases", before.cases.to_string(), after.cases.to_string());
        field(
            "reviews",
            before.reviews.len().to_string(),
            after.reviews.len().to_string(),
        );
        field(
            "coverage",
            format!("{:?}", before.coverage),
            format!("{:?}", after.coverage),
        );
        field(
            "observation",
            format!("{:?}", before.observation),
            format!("{:?}", after.observation),
        );
        field(
            "state",
            format!("{:?}", before.state),
            format!("{:?}", after.state),
        );
    }
    for key in new_entries.keys() {
        if !old_entries.contains_key(key) {
            found.push(Difference::EntryOnly {
                entry: key.clone(),
                new: true,
            });
        }
    }
    let set = |locations: &[Location]| -> BTreeSet<String> {
        locations.iter().map(location_key).collect()
    };
    let (before, after) = (set(&old.untriaged), set(&new.untriaged));
    found.extend(before.difference(&after).map(|l| Difference::Untriaged {
        location: l.clone(),
        new: false,
    }));
    found.extend(after.difference(&before).map(|l| Difference::Untriaged {
        location: l.clone(),
        new: true,
    }));
    let ranges = |index: &Index| -> BTreeSet<String> {
        index.unprojected.iter().map(|r| format!("{r:?}")).collect()
    };
    let (before, after) = (ranges(old), ranges(new));
    found.extend(before.difference(&after).map(|r| Difference::Unprojected {
        range: r.clone(),
        new: false,
    }));
    found.extend(after.difference(&before).map(|r| Difference::Unprojected {
        range: r.clone(),
        new: true,
    }));
    if old.dependence.read_data != new.dependence.read_data {
        found.push(Difference::ReadData);
    }
    let sources = |index: &Index| -> BTreeMap<String, String> {
        index
            .sources
            .iter()
            .map(|s| (s.path.display().to_string(), s.sha256.clone()))
            .collect()
    };
    let (before, after) = (sources(old), sources(new));
    for path in before.keys().filter(|p| !after.contains_key(*p)) {
        found.push(Difference::Source {
            path: path.clone(),
            new: false,
        });
    }
    for path in after.keys().filter(|p| !before.contains_key(*p)) {
        found.push(Difference::Source {
            path: path.clone(),
            new: true,
        });
    }
    let digests: Vec<String> = before
        .iter()
        .filter(|(path, digest)| after.get(*path).is_some_and(|d| d != *digest))
        .map(|(path, _)| path.clone())
        .collect();
    if !digests.is_empty() {
        found.push(Difference::Digests(digests));
    }
    let by_file = |lines: &[SourceLine]| -> BTreeMap<String, usize> {
        let mut counts = BTreeMap::new();
        for line in lines {
            *counts.entry(line.path.display().to_string()).or_default() += 1;
        }
        counts
    };
    if old.unobserved != new.unobserved || old.observed != new.observed {
        let (before, after) = (by_file(&old.unobserved), by_file(&new.unobserved));
        let changed = |path: &str| {
            let filter = |lines: &[SourceLine]| -> Vec<u32> {
                lines
                    .iter()
                    .filter(|l| l.path.display().to_string() == path)
                    .map(|l| l.line)
                    .collect()
            };
            filter(&old.unobserved) != filter(&new.unobserved)
                || filter(&old.observed) != filter(&new.observed)
        };
        let paths: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
        for path in paths {
            if changed(path) {
                found.push(Difference::Lines {
                    path: path.clone(),
                    old: before.get(path).copied().unwrap_or(0),
                    new: after.get(path).copied().unwrap_or(0),
                });
            }
        }
    }
    found
}

/// Whether a difference changes what the shard claims rather than where
/// its sources are.
pub fn is_evidence(difference: &Difference) -> bool {
    !matches!(
        difference,
        Difference::Digests(_) | Difference::Lines { .. } | Difference::Source { .. }
    )
}

/// The readable summary of `differences` under `scenario`.
pub fn render(scenario: &str, differences: &[Difference]) -> String {
    let mut text = format!("{scenario}:\n");
    if differences.is_empty() {
        text.push_str("  identical\n");
        return text;
    }
    let (evidence, recorded): (Vec<_>, Vec<_>) = differences.iter().partition(|d| is_evidence(d));
    for difference in &evidence {
        text.push_str(&format!("  {difference}\n"));
    }
    for difference in &recorded {
        text.push_str(&format!("  (recorded) {difference}\n"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_vendor_evidence_shard::{Dependence, LocationKind, SourceDigest};
    use std::path::PathBuf;

    fn shard() -> Index {
        Index {
            schema: 9,
            command: "x".into(),
            target: "chip-a".into(),
            scenario: "i2c".into(),
            inputs: Default::default(),
            sources: vec![SourceDigest {
                path: PathBuf::from("crates/a.rs"),
                sha256: "1".into(),
            }],
            dependence: Dependence::whole_closure("test"),
            entries: vec![Entry {
                suite: "i2c".into(),
                source: "archive".into(),
                symbol: "phy_x".into(),
                production: "open_x".into(),
                verdict: "MATCH".into(),
                cases: 4,
                reviews: vec![],
                coverage: None,
                observation: None,
                state: None,
            }],
            untriaged: vec![Location {
                function: "phy_x".into(),
                offset: 0x10,
                kind: LocationKind::Block,
            }],
            functions: vec![],
            unobserved: vec![SourceLine {
                path: PathBuf::from("crates/a.rs"),
                line: 10,
            }],
            observed: vec![],
            unprojected: vec![],
        }
    }

    #[test]
    fn evidence_changes_are_listed_and_recorded_ones_counted() {
        let old = shard();
        let mut new = shard();
        new.sources[0].sha256 = "2".into();
        new.unobserved[0].line = 12;
        assert!(differences(&old, &new).iter().all(|d| !is_evidence(d)));
        new.entries[0].verdict = "DIFF".into();
        new.entries[0].cases = 3;
        new.untriaged.push(Location {
            function: "phy_y".into(),
            offset: 4,
            kind: LocationKind::Taken,
        });
        let found = differences(&old, &new);
        let text = render("i2c", &found);
        assert!(text.contains("verdict MATCH -> DIFF"), "{text}");
        assert!(text.contains("cases 4 -> 3"), "{text}");
        assert!(text.contains("untriaged phy_y+0x4 Taken: added"), "{text}");
        assert!(
            text.contains("(recorded) source digests changed: crates/a.rs"),
            "{text}"
        );
        assert!(
            text.contains("(recorded) crates/a.rs: observed and unobserved lines changed"),
            "{text}"
        );
        assert_eq!(found.iter().filter(|d| is_evidence(d)).count(), 3);
        assert!(render("i2c", &differences(&old, &old)).contains("identical"));
    }
}
