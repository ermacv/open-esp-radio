//! The owner's thresholds, verbatim rule and examples: issue #375.
use super::{Provenance, Summary};
use crate::Result;
use oer_image_checks::sizes::{Measurements, Section};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    path::Path,
};

/// All owner thresholds live here. Allocated stacks and individual RAM
/// sections have no growth allowance. See the README's link to rule #375.
pub const VALUE_GROWTH_BYTES: u64 = 64;
pub const BOUND_GROWTH_BYTES: u64 = 64;
pub const CODE_GROWTH_BYTES: u64 = 1024;

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
        .push_str("| Image | Code growth B | Verdict |\n| --- | ---: | --- |\n");
    let mut details = String::new();
    for (image, base) in base {
        let (verdict, code, shown) = compare(&base, &head[&image])?;
        result.verdict = result.verdict.max(verdict);
        let _ = writeln!(result.output, "| {image} | +{code} | {verdict} |");
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

fn compare(a: &Summary, b: &Summary) -> Result<(Verdict, u64, String)> {
    if a.schema != super::SCHEMA
        || b.schema != super::SCHEMA
        || a.type_min_bytes != b.type_min_bytes
        || !compatible(&a.provenance, &b.provenance)
        || a.elfs.keys().ne(b.elfs.keys())
    {
        return Err("incompatible size provenance: image, compiler, target, profile, flags, features, configuration or ELF set differs".into());
    }
    let mut diff = Diff::default();
    let mut code_growth = 0;
    for (elf, a) in &a.elfs {
        let b = &b.elfs[elf];
        code_growth += code_growth_of(a, b);
        diff.measurements(elf, a, b);
    }
    if code_growth > CODE_GROWTH_BYTES {
        diff.stop(format!(
            "code growth +{code_growth} B exceeds {CODE_GROWTH_BYTES} B"
        ));
    }
    Ok((diff.verdict, code_growth, diff.output))
}

fn code_growth_of(a: &Measurements, b: &Measurements) -> u64 {
    b.sections
        .iter()
        .filter(|(_, s)| s.executable)
        .map(|(name, s)| {
            s.bytes.saturating_sub(
                a.sections
                    .get(name)
                    .filter(|s| s.executable)
                    .map_or(0, |s| s.bytes),
            )
        })
        .sum()
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
    fn change(&mut self, elf: &str, kind: &str, name: &str, before: u64, after: u64, limit: u64) {
        if before == after {
            return;
        }
        self.verdict = self.verdict.max(Verdict::Report);
        let delta = i128::from(after) - i128::from(before);
        let _ = writeln!(
            self.output,
            "| {elf} | {kind} | `{name}` | {before} | {after} | {delta:+} |"
        );
        if after.saturating_sub(before) > limit {
            self.stop(format!(
                "{elf} {kind} `{name}` grows {delta:+} B (limit +{limit} B)"
            ));
        }
    }
    fn inventory<T>(
        &mut self,
        elf: &str,
        kind: &str,
        a: &BTreeMap<String, T>,
        b: &BTreeMap<String, T>,
    ) {
        for (label, from, to) in [("new", b, a), ("removed", a, b)] {
            let names: Vec<_> = from.keys().filter(|name| !to.contains_key(*name)).collect();
            if names.is_empty() {
                continue;
            }
            self.verdict = self.verdict.max(Verdict::Report);
            let _ = writeln!(self.output, "\n{label} {kind}s ({elf}):");
            for name in names {
                let _ = writeln!(self.output, "- {label} {kind} ({elf}): `{name}`");
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
                if after.is_some_and(is_ram) {
                    0
                } else {
                    u64::MAX
                },
            );
            if before != after {
                self.verdict = self.verdict.max(Verdict::Report);
                if before.map(|s| (&s.memory, s.executable, s.writable))
                    != after.map(|s| (&s.memory, s.executable, s.writable))
                {
                    let _ = writeln!(self.output, "section `{name}`: {:?} → {:?}", before, after);
                    if before.is_some() && after.is_some_and(is_ram) && !before.is_some_and(is_ram)
                    {
                        self.stop(format!("{elf} `{name}` moved into RAM"));
                    }
                }
            }
        }
        for name in keys(&a.allocated_stacks, &b.allocated_stacks) {
            let before = a.allocated_stacks.get(name);
            let after = b.allocated_stacks.get(name);
            if before != after {
                self.stop(format!(
                    "{elf} allocated stack `{name}`: {before:?} → {after:?}"
                ));
            }
        }
        self.values(elf, "type", &a.types, &b.types);
        // Retain multiplicity; compare every implementation in sorted order.
        for name in keys(&a.frames, &b.frames) {
            let before = a.frames.get(name).map_or(&[][..], Vec::as_slice);
            let after = b.frames.get(name).map_or(&[][..], Vec::as_slice);
            for index in 0..before.len().max(after.len()) {
                self.change(
                    elf,
                    "frame",
                    name,
                    before.get(index).copied().unwrap_or(0),
                    after.get(index).copied().unwrap_or(0),
                    VALUE_GROWTH_BYTES,
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
                BOUND_GROWTH_BYTES,
            );
            self.change(
                elf,
                "root maximum frame",
                name,
                before.map_or(0, |s| s.maximum_frame),
                after.map_or(0, |s| s.maximum_frame),
                VALUE_GROWTH_BYTES,
            );
            if before != after {
                self.verdict = self.verdict.max(Verdict::Report);
            }
            if let Some(after) = after {
                for hole in &after.holes {
                    if !before.is_some_and(|b| b.holes.contains(hole)) {
                        self.stop(format!(
                            "{elf} bound `{name}` new hole: {}+{:#x}: {}",
                            hole.function, hole.offset, hole.reason
                        ));
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
            self.change(
                elf,
                kind,
                name,
                a.get(name).copied().unwrap_or(0),
                b.get(name).copied().unwrap_or(0),
                VALUE_GROWTH_BYTES,
            );
        }
    }
}
fn is_ram(section: &Section) -> bool {
    matches!(section.memory.as_str(), "sram" | "psram")
}
fn keys<'a, T>(a: &'a BTreeMap<String, T>, b: &'a BTreeMap<String, T>) -> BTreeSet<&'a str> {
    a.keys().chain(b.keys()).map(String::as_str).collect()
}

#[cfg(test)]
mod tests;
