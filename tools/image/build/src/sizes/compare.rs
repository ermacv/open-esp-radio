//! The owner's size rule: `CLAUDE.md`, "Sizes".
use super::{Provenance, Summary};
use crate::Result;
use oer_image_checks::sizes::{Measurements, Section};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    path::Path,
};

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub enum Verdict {
    #[default]
    Same,
    Report,
    Stop,
}
impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Same => "same",
            Self::Report => "report",
            Self::Stop => "stop",
        })
    }
}

#[derive(Debug, Default)]
pub struct Comparison {
    pub verdict: Verdict,
    pub output: String,
}

pub fn compare_directories(base: &Path, head: &Path) -> Result<Comparison> {
    let base = summaries(base)?;
    let head = summaries(head)?;
    if base.keys().ne(head.keys()) {
        return Err("incompatible image sets in size output directories".into());
    }
    let mut result = Comparison::default();
    result
        .output
        .push_str("| Image | Net code growth B | Verdict |\n| --- | ---: | --- |\n");
    let mut details = String::new();
    for (image, base) in base {
        let (verdict, code, shown) = compare(&base, &head[&image])?;
        result.verdict = result.verdict.max(verdict);
        let _ = writeln!(result.output, "| {image} | {code:+} | {verdict} |");
        let _ = writeln!(details, "\n{image}\n{shown}");
    }
    result.output.push_str(&details);
    let _ = writeln!(result.output, "\noverall: {}", result.verdict);
    Ok(result)
}

fn summaries(directory: &Path) -> Result<BTreeMap<String, Summary>> {
    fn visit(directory: &Path, out: &mut BTreeMap<String, Summary>) -> Result<()> {
        if directory.join(super::FILE).is_file() {
            let summary = Summary::read(directory)?;
            let p = &summary.provenance;
            let key = format!(
                "{}/{} [{}; {}]",
                p.chip,
                p.binary,
                p.features.join(","),
                match p.purpose {
                    crate::bundle::Purpose::Analysis => "analysis",
                    crate::bundle::Purpose::Flash => "flash",
                }
            );
            if out.insert(key.clone(), summary).is_some() {
                return Err(format!("duplicate image summary: {key}").into());
            }
            return Ok(());
        }
        if directory.join(crate::bundle::MANIFEST).is_file() {
            return Err(format!(
                "missing sizes.json beside {}",
                directory.join(crate::bundle::MANIFEST).display()
            )
            .into());
        }
        let mut children = std::fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
        children.sort_by_key(|e| e.file_name());
        for child in children {
            if child.file_type()?.is_dir() && !child.file_name().to_string_lossy().starts_with('.')
            {
                visit(&child.path(), out)?;
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    visit(directory, &mut out)?;
    if out.is_empty() {
        return Err(format!("no sizes.json in {}", directory.display()).into());
    }
    Ok(out)
}

fn compatible(a: &Provenance, b: &Provenance) -> bool {
    a.chip == b.chip
        && a.package == b.package
        && a.binary == b.binary
        && a.target == b.target
        && a.profile == b.profile
        && a.profile_settings == b.profile_settings
        && a.features == b.features
        && a.default_features == b.default_features
        && a.purpose == b.purpose
        && a.layout_seed == b.layout_seed
        && a.configuration_sha256 == b.configuration_sha256
        && a.rustc == b.rustc
        && a.stack_policy == b.stack_policy
        && compiler_flags(&a.flags) == compiler_flags(&b.flags)
}

fn compiler_flags(
    flags: &BTreeMap<String, BTreeMap<String, String>>,
) -> BTreeMap<&str, BTreeMap<&str, &str>> {
    flags
        .iter()
        .map(|(elf, values)| {
            (
                elf.as_str(),
                values
                    .iter()
                    .filter(|(key, _)| {
                        !matches!(
                            key.as_str(),
                            "OPEN_RADIO_CPU0_TASK_STACK_BYTES" | "OPEN_RADIO_IRQ_STACK_BYTES"
                        )
                    })
                    .map(|(key, value)| (key.as_str(), value.as_str()))
                    .collect(),
            )
        })
        .collect()
}

fn compare(a: &Summary, b: &Summary) -> Result<(Verdict, i128, String)> {
    if a.schema != super::SCHEMA
        || b.schema != super::SCHEMA
        || a.type_min_bytes != b.type_min_bytes
        || !compatible(&a.provenance, &b.provenance)
        || a.elfs.keys().ne(b.elfs.keys())
    {
        return Err("incompatible size provenance: image, compiler, target, profile, flags, features, configuration or ELF set differs".into());
    }
    let mut diff = Diff::default();
    let code_growth = code_bytes(b) - code_bytes(a);
    for (elf, a) in &a.elfs {
        let b = &b.elfs[elf];
        diff.measurements(elf, a, b);
    }
    if !a.image_files.is_empty() || !b.image_files.is_empty() {
        diff.output.push_str(
            "\n| Image file | Base B | Head B | Delta B |\n| --- | ---: | ---: | ---: |\n",
        );
        for name in keys(&a.image_files, &b.image_files) {
            let before = a.image_files.get(name).copied().unwrap_or(0);
            let after = b.image_files.get(name).copied().unwrap_or(0);
            let _ = writeln!(
                diff.output,
                "| {name} | {before} | {after} | {:+} |",
                i128::from(after) - i128::from(before)
            );
            if before != after {
                diff.verdict = diff.verdict.max(Verdict::Report);
            }
        }
    }
    if a.embedded_payloads != b.embedded_payloads {
        let _ = writeln!(
            diff.output,
            "embedded payload copies (excluded from code): {:?} → {:?}",
            a.embedded_payloads, b.embedded_payloads
        );
    }
    Ok((diff.verdict, code_growth, diff.output))
}

fn code_bytes(summary: &Summary) -> i128 {
    let contents: i128 = summary
        .elfs
        .values()
        .flat_map(|m| m.sections.values())
        .map(|s| i128::from(s.image_content_bytes()))
        .sum();
    contents
        - summary
            .embedded_payloads
            .iter()
            .map(|p| i128::from(p.bytes))
            .sum::<i128>()
}

#[derive(Default)]
struct Diff {
    verdict: Verdict,
    output: String,
}
impl Diff {
    fn stop(&mut self, reason: String) {
        self.verdict = Verdict::Stop;
        let _ = writeln!(self.output, "STOP: {reason}");
    }
    /// Growth of a `stops` entry (an SRAM section or an allocated stack) stops;
    /// growth of any other entry is only reported in the table.
    fn change(&mut self, elf: &str, kind: &str, name: &str, before: u64, after: u64, stops: bool) {
        if before == after {
            return;
        }
        self.verdict = self.verdict.max(Verdict::Report);
        let delta = i128::from(after) - i128::from(before);
        let _ = writeln!(
            self.output,
            "| {elf} | {kind} | `{name}` | {before} | {after} | {delta:+} |"
        );
        if stops && after > before {
            self.stop(format!("{elf} {kind} `{name}` grows {delta:+} B"));
        }
    }
    fn inventory<T: std::fmt::Debug>(
        &mut self,
        elf: &str,
        kind: &str,
        a: &BTreeMap<String, T>,
        b: &BTreeMap<String, T>,
    ) {
        for (label, from, to) in [("new", b, a), ("removed", a, b)] {
            let names: Vec<_> = from
                .iter()
                .filter(|(name, _)| !to.contains_key(*name))
                .collect();
            if names.is_empty() {
                continue;
            }
            self.verdict = self.verdict.max(Verdict::Report);
            let _ = writeln!(self.output, "\n{label} {kind}s ({elf}):");
            for (name, bytes) in names {
                let _ = writeln!(
                    self.output,
                    "- {label} {kind} ({elf}): `{name}` ({bytes:?} B)"
                );
            }
        }
    }
    fn measurements(&mut self, elf: &str, a: &Measurements, b: &Measurements) {
        let _ = writeln!(
            self.output,
            "\n| ELF | Kind | Name | Base B | Head B | Delta B |\n| --- | --- | --- | ---: | ---: | ---: |"
        );
        for name in keys(&a.sections, &b.sections) {
            let before = a.sections.get(name);
            let after = b.sections.get(name);
            self.change(
                elf,
                "section",
                name,
                before.map_or(0, |s| s.bytes),
                after.map_or(0, |s| s.bytes),
                after.is_some_and(Section::is_sram),
            );
            self.change(
                elf,
                "linker filler",
                name,
                before.map_or(0, |s| s.filler.iter().map(|r| r.bytes).sum()),
                after.map_or(0, |s| s.filler.iter().map(|r| r.bytes).sum()),
                false,
            );
            if before != after {
                self.verdict = self.verdict.max(Verdict::Report);
                if before.map(|s| (&s.memory, s.class, s.nobits, s.executable, s.writable))
                    != after.map(|s| (&s.memory, s.class, s.nobits, s.executable, s.writable))
                {
                    let _ = writeln!(self.output, "section `{name}`: {:?} → {:?}", before, after);
                    if before.is_some()
                        && after.is_some_and(Section::is_sram)
                        && !before.is_some_and(Section::is_sram)
                    {
                        self.stop(format!("{elf} `{name}` moved into SRAM"));
                    }
                }
            }
        }
        for name in keys(&a.allocated_stacks, &b.allocated_stacks) {
            let before = a.allocated_stacks.get(name);
            let after = b.allocated_stacks.get(name);
            self.change(
                elf,
                "allocated stack",
                name,
                before.copied().unwrap_or(0),
                after.copied().unwrap_or(0),
                true,
            );
        }
        self.values(elf, "type", &a.types, &b.types);
        // Retain multiplicity; compare every implementation in sorted order.
        for name in keys(&a.frames, &b.frames) {
            let (Some(before), Some(after)) = (a.frames.get(name), b.frames.get(name)) else {
                continue;
            };
            if before != after {
                self.verdict = self.verdict.max(Verdict::Report);
            }
            if before.len() != after.len() {
                let _ = writeln!(
                    self.output,
                    "frame `{name}` implementations: {} → {}",
                    before.len(),
                    after.len()
                );
            }
            for index in 0..before.len().max(after.len()) {
                self.change(
                    elf,
                    "frame",
                    name,
                    before.get(index).copied().unwrap_or(0),
                    after.get(index).copied().unwrap_or(0),
                    false,
                );
            }
        }
        for name in keys(&a.bounds, &b.bounds) {
            let before = a.bounds.get(name);
            let after = b.bounds.get(name);
            self.change(
                elf,
                "bound",
                name,
                before.map_or(0, |s| s.bytes),
                after.map_or(0, |s| s.bytes),
                false,
            );
            self.change(
                elf,
                "root maximum frame",
                name,
                before.map_or(0, |s| s.maximum_frame),
                after.map_or(0, |s| s.maximum_frame),
                false,
            );
            if before != after {
                self.verdict = self.verdict.max(Verdict::Report);
            }
            if let Some(after) = after {
                let mut counts = BTreeMap::new();
                if let Some(before) = before {
                    for hole in &before.holes {
                        *counts
                            .entry((&hole.function, &hole.inline_chain, &hole.reason))
                            .or_insert(0usize) += 1;
                    }
                }
                for hole in &after.holes {
                    let remaining = counts
                        .entry((&hole.function, &hole.inline_chain, &hole.reason))
                        .or_default();
                    if *remaining != 0 {
                        *remaining -= 1;
                    } else {
                        self.verdict = self.verdict.max(Verdict::Report);
                        let _ = writeln!(
                            self.output,
                            "new hole: {}+{:#x}: {}; inline chain {:?} ({elf} bound `{name}`)",
                            hole.function, hole.offset, hole.reason, hole.inline_chain
                        );
                    }
                }
                if before.is_some_and(|b| {
                    b.complete != after.complete
                        || b.assumptions != after.assumptions
                        || b.root != after.root
                }) {
                    let _ = writeln!(
                        self.output,
                        "bound `{name}`: root `{}`, complete {}, assumptions {:?}",
                        after.root, after.complete, after.assumptions
                    );
                }
            }
        }
        self.inventory(elf, "function", &a.frames, &b.frames);
        self.inventory(elf, "type", &a.types, &b.types);
        let _ = writeln!(
            self.output,
            "analysis {elf}: work {} → {} / {}; peak memory {} → {} / {} B; partial functions {} → {}; partial bounds {} → {}",
            a.resources.work_units,
            b.resources.work_units,
            b.resources.work_limit,
            a.resources.peak_memory_bytes,
            b.resources.peak_memory_bytes,
            b.resources.memory_limit_bytes,
            a.resources.partial_functions,
            b.resources.partial_functions,
            a.resources.partial_bounds,
            b.resources.partial_bounds
        );
    }
    fn values(
        &mut self,
        elf: &str,
        kind: &str,
        a: &BTreeMap<String, u64>,
        b: &BTreeMap<String, u64>,
    ) {
        for name in keys(a, b) {
            let (Some(&before), Some(&after)) = (a.get(name), b.get(name)) else {
                continue;
            };
            self.change(elf, kind, name, before, after, false);
        }
    }
}
fn keys<'a, T>(a: &'a BTreeMap<String, T>, b: &'a BTreeMap<String, T>) -> BTreeSet<&'a str> {
    a.keys().chain(b.keys()).map(String::as_str).collect()
}

#[cfg(test)]
mod tests;
