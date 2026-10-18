//! Production PHY source lines the compared observations depend on.
//!
//! Blobray reports which executed production instructions a compared
//! observation depends on. Debug line information of the probe ELF maps them
//! to production hardware source lines (PHY, HAL, PAC and MAC driver crates
//! of the chip, and the chip-neutral hardware crates it shares), including
//! inlined frames: a line is
//! observed when any of its executed instructions is. Every executed but
//! unobserved line is either reviewed by a decision below, with its reason, or
//! reported as untriaged in the evidence index. A decision that matches no
//! unobserved line fails the run, so the table cannot outlive the code it
//! describes.
use crate::harness::{Result, invalid};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Production hardware sources of the installed chip, relative to the
/// repository root.
fn hardware_scope() -> &'static str {
    crate::chip().hardware_scope
}

/// Every production hardware source tree attributed to the installed chip:
/// its own, then the chip-neutral ones it shares.
fn scopes() -> impl Iterator<Item = &'static str> {
    std::iter::once(hardware_scope()).chain(crate::chip().shared_scopes.iter().copied())
}

/// The repository path of a decision's file: below the chip's hardware
/// scope, or below the repository root when it names a shared tree.
fn decision_path(file: &str) -> PathBuf {
    if crate::chip()
        .shared_scopes
        .iter()
        .any(|scope| file.starts_with(scope))
    {
        PathBuf::from(file)
    } else {
        Path::new(hardware_scope()).join(file)
    }
}

/// Every place of `decisions` with its repository path, resolved once.
fn places(decisions: &[Decision]) -> Vec<(PathBuf, (&'static str, &'static str))> {
    decisions
        .iter()
        .flat_map(|d| d.places)
        .map(|place| (decision_path(place.0), *place))
        .collect()
}

/// A source line of one file, relative to the repository root.
pub type SourceLine = (PathBuf, u32);

/// A reviewed decision on executed lines no compared observation depends on.
/// Each place is a file below the chip's hardware scope (or a path of a shared
/// hardware tree from the repository root) and trimmed source text ending at
/// that line. A multiline anchor includes the preceding context, distinguishing
/// identical dispatch lines in different functions without a line-number pin.
#[derive(Clone, Copy, Debug)]
pub struct Decision {
    pub reason: &'static str,
    pub places: &'static [(&'static str, &'static str)],
}

/// Executed and observed production hardware lines.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Lines {
    pub executed: BTreeSet<SourceLine>,
    pub observed: BTreeSet<SourceLine>,
    /// Lines an emitted event depends on, compared or not.
    pub effect: BTreeSet<SourceLine>,
    /// Lines the memory state at a case end depends on.
    pub state: BTreeSet<SourceLine>,
}

impl Lines {
    pub fn extend(&mut self, other: &Lines) {
        self.executed.extend(other.executed.iter().cloned());
        self.observed.extend(other.observed.iter().cloned());
        self.effect.extend(other.effect.iter().cloned());
        self.state.extend(other.state.iter().cloned());
    }

    pub fn unobserved(&self) -> BTreeSet<SourceLine> {
        self.executed.difference(&self.observed).cloned().collect()
    }
}

/// Maps probe instructions to production hardware source lines.
pub struct LineMap {
    /// Every located line once, in order; instructions name them by index.
    table: Vec<SourceLine>,
    lines: BTreeMap<u32, Vec<u32>>,
}

impl LineMap {
    /// The lines within the chip's hardware scope of every instruction in `pcs` of `elf`.
    pub fn new(elf: &[u8], pcs: &BTreeSet<u32>, root: &Path) -> Result<Self> {
        let symbolizer = oer_elf::dwarf::Symbolizer::new(elf)?;
        let root = root.canonicalize()?;
        let mut lines = BTreeMap::new();
        for pc in pcs {
            let mut located = vec![];
            let frames = symbolizer
                .frames(u64::from(*pc))
                .map_err(|e| invalid(format!("debug information at {pc:#x}: {e}")))?;
            for frame in frames {
                let (Some(path), Some(line)) = (frame.file, frame.line) else {
                    continue;
                };
                if let Some(file) = scopes().find_map(|scope| {
                    Path::new(&path)
                        .strip_prefix(root.join(scope))
                        .ok()
                        .map(|relative| Path::new(scope).join(relative))
                }) {
                    located.push((file, line));
                }
            }
            lines.insert(*pc, located);
        }
        Ok(Self::from_lines(lines))
    }

    fn from_lines(located: BTreeMap<u32, Vec<SourceLine>>) -> Self {
        let table: Vec<SourceLine> = located
            .values()
            .flatten()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let lines = located
            .into_iter()
            .map(|(pc, lines)| {
                let indices = lines
                    .iter()
                    .map(|line| table.binary_search(line).unwrap() as u32)
                    .collect();
                (pc, indices)
            })
            .collect();
        Self { table, lines }
    }

    /// Lines of `executed` instructions, observed when any of `observed` is.
    pub fn lines(
        &self,
        instructions: &blobray_application::in_process::ObservedInstructions,
    ) -> Lines {
        // Mark line indices first, so each line is cloned once per set.
        let mut marks = vec![[false; 4]; self.table.len()];
        for pc in &instructions.executed {
            let Some(indices) = self.lines.get(pc) else {
                continue;
            };
            let flags = [
                true,
                instructions.observed.contains(pc),
                instructions.effect.contains(pc),
                instructions.state.contains(pc),
            ];
            for index in indices {
                for (mark, flag) in marks[*index as usize].iter_mut().zip(flags) {
                    *mark |= flag;
                }
            }
        }
        // The table is ordered, so each set is built from a sorted sequence.
        let set = |kind: usize| -> BTreeSet<SourceLine> {
            marks
                .iter()
                .zip(&self.table)
                .filter(|(marks, _)| marks[kind])
                .map(|(_, line)| line.clone())
                .collect()
        };
        Lines {
            executed: set(0),
            observed: set(1),
            effect: set(2),
            state: set(3),
        }
    }
}

/// Trimmed text of source lines, read once per file.
#[derive(Default)]
pub struct Sources {
    files: BTreeMap<PathBuf, Vec<String>>,
}

impl Sources {
    fn text(&mut self, root: &Path, (file, line): &SourceLine) -> Result<&str> {
        if !self.files.contains_key(file) {
            let text = std::fs::read_to_string(root.join(file))?;
            self.files.insert(
                file.clone(),
                text.lines().map(|l| l.trim().to_owned()).collect(),
            );
        }
        self.files[file]
            .get(*line as usize - 1)
            .map(String::as_str)
            .ok_or_else(|| invalid(format!("{}:{line} is outside its file", file.display())))
    }

    fn matches(&mut self, root: &Path, line: &SourceLine, anchor: &str) -> Result<bool> {
        // Load the file and retain the same out-of-file diagnostic as a
        // single-line anchor. Matching context never reviews its earlier lines.
        self.text(root, line)?;
        let expected: Vec<_> = anchor.lines().map(str::trim).collect();
        let end = line.1 as usize;
        let Some(start) = end
            .checked_sub(expected.len())
            .filter(|_| !expected.is_empty())
        else {
            return Ok(false);
        };
        Ok(self.files[&line.0][start..end]
            .iter()
            .map(String::as_str)
            .eq(expected))
    }

    /// Split `unobserved` into (reviewed, untriaged) under `decisions`.
    pub fn classify(
        &mut self,
        root: &Path,
        decisions: &[Decision],
        unobserved: &BTreeSet<SourceLine>,
    ) -> Result<(BTreeSet<SourceLine>, BTreeSet<SourceLine>)> {
        let places = places(decisions);
        let mut reviewed = BTreeSet::new();
        let mut untriaged = BTreeSet::new();
        for line in unobserved {
            let mut matched = false;
            for (path, (_, source)) in &places {
                if line.0 == *path && self.matches(root, line, source)? {
                    matched = true;
                    break;
                }
            }
            if matched {
                reviewed.insert(line.clone());
            } else {
                untriaged.insert(line.clone());
            }
        }
        Ok((reviewed, untriaged))
    }

    /// Every place of every decision must still match an unobserved line.
    pub fn check(
        &mut self,
        root: &Path,
        decisions: &[Decision],
        unobserved: &BTreeSet<SourceLine>,
    ) -> Result<()> {
        let places = places(decisions);
        let mut matched = BTreeSet::new();
        for line in unobserved {
            for (path, place) in &places {
                if line.0 == *path && self.matches(root, line, place.1)? {
                    matched.insert(*place);
                }
            }
        }
        let stale: Vec<String> = decisions
            .iter()
            .flat_map(|d| d.places)
            .filter(|place| !matched.contains(*place))
            .map(|(file, source)| format!("`{source}` in {file}"))
            .collect();
        if stale.is_empty() {
            Ok(())
        } else {
            Err(invalid(format!(
                "observation decisions match no unobserved line; each line is observed or \
                 no longer executed: {}",
                stale.join(", ")
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "crates/hardware/test/phy/src/example.rs";
    const PLACE: &str = "phy/src/example.rs";

    fn root() -> tempfile::TempDir {
        crate::install(&crate::chip::TEST);
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join(FILE);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, "fn f() {\n    let unused = 1;\n    write();\n}\n").unwrap();
        root
    }

    fn line(number: u32) -> SourceLine {
        (PathBuf::from(FILE), number)
    }

    const DECIDED: &[Decision] = &[Decision {
        reason: "test",
        places: &[(PLACE, "let unused = 1;")],
    }];

    #[test]
    fn decisions_review_their_lines_and_leave_the_rest_untriaged() {
        let root = root();
        let unobserved = BTreeSet::from([line(2), line(3)]);
        let (reviewed, untriaged) = Sources::default()
            .classify(root.path(), DECIDED, &unobserved)
            .unwrap();
        assert_eq!(reviewed, BTreeSet::from([line(2)]));
        assert_eq!(untriaged, BTreeSet::from([line(3)]));
    }

    #[test]
    fn context_distinguishes_identical_lines_and_does_not_review_the_context() {
        let root = root();
        std::fs::write(root.path().join(FILE),
            "fn first() {\n    match detail {\n    }\n}\n\nfn second() {\n    match detail {\n    }\n}\n",
        ).unwrap();
        let decisions = &[Decision {
            reason: "only the second dispatch is covered by this stop goal",
            places: &[(PLACE, "fn second() {\nmatch detail {")],
        }];
        let mut sources = Sources::default();
        let unobserved = BTreeSet::from([line(2), line(6), line(7)]);
        let (reviewed, untriaged) = sources
            .classify(root.path(), decisions, &unobserved)
            .unwrap();
        assert_eq!(reviewed, BTreeSet::from([line(7)]));
        assert_eq!(untriaged, BTreeSet::from([line(2), line(6)]));
        sources.check(root.path(), decisions, &unobserved).unwrap();
        assert!(
            sources
                .check(root.path(), decisions, &BTreeSet::from([line(2)]))
                .is_err()
        );
        assert!(
            sources
                .check(root.path(), decisions, &BTreeSet::from([line(6)]))
                .is_err()
        );
    }

    #[test]
    fn a_decision_whose_line_is_observed_or_not_executed_is_stale() {
        let root = root();
        let mut sources = Sources::default();
        sources
            .check(root.path(), DECIDED, &BTreeSet::from([line(2)]))
            .unwrap();
        assert!(
            sources
                .check(root.path(), DECIDED, &BTreeSet::from([line(3)]))
                .is_err()
        );
        assert!(
            sources
                .check(root.path(), DECIDED, &BTreeSet::new())
                .is_err()
        );
    }

    #[test]
    fn decisions_name_shared_trees_from_the_repository_root() {
        crate::install(&crate::chip::TEST);
        assert_eq!(decision_path(PLACE), PathBuf::from(FILE));
        assert_eq!(
            decision_path("crates/hardware/shared-test/src/lib.rs"),
            PathBuf::from("crates/hardware/shared-test/src/lib.rs")
        );
    }

    #[test]
    fn a_line_is_observed_when_any_of_its_instructions_is() {
        let map = LineMap::from_lines(BTreeMap::from([
            (0x10, vec![line(2)]),
            (0x14, vec![line(2), line(3)]),
        ]));
        let lines = map.lines(&blobray_application::in_process::ObservedInstructions {
            executed: BTreeSet::from([0x10, 0x14]),
            observed: BTreeSet::from([0x10]),
            ..Default::default()
        });
        assert_eq!(lines.executed, BTreeSet::from([line(2), line(3)]));
        assert_eq!(lines.observed, BTreeSet::from([line(2)]));
        assert_eq!(lines.unobserved(), BTreeSet::from([line(3)]));
    }
}
