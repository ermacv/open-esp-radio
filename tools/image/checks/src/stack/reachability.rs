//! Read the existing bounds, never call or reproduce their graph walk.
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use oer_analysis_riscv_stack::Bound;
use oer_chip_profile::policy::StackPolicy;

use super::Image;

pub const RUNTIME_REPORT: &str = "runtime-reachability.txt";
pub const BOOTSTRAP_REPORT: &str = "bootstrap-reachability.txt";
pub const INTERRUPT_REPORT: &str = "interrupt-reachability.txt";

/// Standalone probes have no post-build stack gate. Analyse each linked
/// boot entry (deduplicating aliases), without introducing a build verdict.
pub fn probe(
    root: &std::path::Path,
    profile: &oer_chip_profile::Profile,
    policy: &StackPolicy,
    elf: &std::path::Path,
    output: &std::path::Path,
) -> super::Result<()> {
    let image = Image::read(root, profile, elf)?;
    let report = Report::new(&image, policy);
    let (mut body, mut peak) = image.with_bounds(|keep, control| {
        let mut body = String::new();
        let mut entries = BTreeSet::new();
        for symbol in ["runtime_main", "_start_rust", "main", "_start"] {
            if let Some(address) = image.symbol(symbol)
                && entries.insert(address)
            {
                let bound =
                    image
                        .analysis
                        .bound_with(address, &image.resolutions, keep, control)?;
                body.push_str(&report.root(&format!("entry:{symbol}"), &bound));
            }
        }
        for (index, (&address, _)) in image
            .names
            .iter()
            .filter(|(_, name)| name.contains("TaskStorage<") && name.ends_with("::poll"))
            .enumerate()
        {
            let bound = image
                .analysis
                .bound_with(address, &image.resolutions, keep, control)?;
            body.push_str(&report.root(&format!("T{index}"), &bound));
        }
        Ok(body)
    })?;
    if image
        .contract
        .as_ref()
        .is_some_and(|contract| image.symbol(&contract.interrupts.table_symbol).is_some())
    {
        let stacks = crate::interrupts::interrupt_stacks_of(&image)?;
        peak = peak.max(stacks.witness_memory_peak);
        for hart in &stacks.harts {
            for level in &hart.levels {
                body.push_str(
                    &report.root(&format!("hart{}-P{}", hart.core, level.level), &level.bound),
                );
            }
            body.push_str(&report.root(
                &format!("hart{}-exception", hart.core),
                &hart.exception.bound,
            ));
        }
    } else {
        body.push_str("IRQ contexts: no staged interrupt table; no IRQ completeness claim.\n");
    }
    std::fs::create_dir_all(output)?;
    std::fs::write(
        output.join(RUNTIME_REPORT),
        format!("{}{body}", report.header(peak)),
    )?;
    Ok(())
}

/// Linked implementations selected by policy, plus explicit unmatched selectors.
pub struct Report<'a> {
    image: &'a Image,
    targets: BTreeMap<u32, BTreeSet<String>>,
    unmatched: Vec<&'a str>,
}

fn matches(pattern: &str, name: &str) -> bool {
    let mut parts = pattern.split('*');
    let first = parts.next().expect("split always has one item");
    let Some(mut remaining) = name.strip_prefix(first) else {
        return false;
    };
    let rest: Vec<_> = parts.collect();
    if rest.is_empty() {
        return remaining.is_empty();
    }
    for (index, part) in rest.iter().enumerate() {
        if index + 1 == rest.len() {
            return remaining.ends_with(part);
        }
        let Some(at) = remaining.find(part) else {
            return false;
        };
        remaining = &remaining[at + part.len()..];
    }
    true
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Verdict {
    Reachable,
    ProvenUnreachable,
    Unknown,
}

impl Verdict {
    fn of(reached: bool, bound: &Bound) -> Self {
        if reached {
            Self::Reachable
        } else if bound.unresolved.is_empty() {
            Self::ProvenUnreachable
        } else {
            Self::Unknown
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Reachable => "reachable",
            Self::ProvenUnreachable => "proven-unreachable",
            Self::Unknown => "unknown",
        }
    }
}

impl<'a> Report<'a> {
    pub fn new(image: &'a Image, policy: &'a StackPolicy) -> Self {
        let mut targets = BTreeMap::<u32, BTreeSet<String>>::new();
        let mut unmatched = Vec::new();
        // Inspect aliases as well as the display name, including ROM companions.
        let patterns: BTreeSet<_> = image
            .observed_functions
            .iter()
            .chain(&policy.observed_functions)
            .collect();
        for pattern in patterns {
            let mut found = false;
            for (&address, facts) in &image.analysis.functions {
                let names: BTreeSet<_> = facts
                    .function
                    .names
                    .iter()
                    .map(|name| oer_analysis_elf::demangle(name))
                    .filter(|name| matches(pattern, name))
                    .collect();
                if !names.is_empty() {
                    found = true;
                    targets.entry(address).or_default().extend(names);
                }
            }
            if !found {
                unmatched.push(pattern.as_str());
            }
        }
        Self {
            image,
            targets,
            unmatched,
        }
    }

    pub fn header(&self, memory_peak: u64) -> String {
        format!(
            "{}Reachability over the existing resolved stack bounds (report only)\n\
            Witnesses follow first-discovery parents; they are not shortest paths.\n\
            Unmatched selectors have no linked function entry; inlined bodies are not separate entries.\n\
            observed linked implementations: {}; unmatched selectors: {}\n\
            \n",
            crate::stack::budget::header(&self.image.resources, memory_peak),
            self.targets.len(),
            self.unmatched.len()
        )
    }

    pub fn root(&self, label: &str, bound: &Bound) -> String {
        let paths = self
            .targets
            .keys()
            .filter(|address| bound.reached.contains(address))
            .count();
        let mut out = format!(
            "root\t{label}\tverdict={}\tpaths={paths}\tholes={}\tentry={:#010x} {}\n",
            Verdict::of(paths != 0, bound).label(),
            bound.unresolved.len(),
            bound.root,
            self.image.function_at(bound.root)
        );
        for (&address, aliases) in &self.targets {
            let reached = bound.reached.contains(&address);
            let _ = writeln!(
                out,
                "  {}\t{address:#010x}\t{}",
                Verdict::of(reached, bound).label(),
                aliases.iter().cloned().collect::<Vec<_>>().join(" | ")
            );
            if reached {
                let witness = bound
                    .witness(address)
                    .expect("reached function has a first-discovery witness");
                let _ = writeln!(
                    out,
                    "    witness: {:#010x} {}",
                    bound.root,
                    self.image.function_at(bound.root)
                );
                for (callee, parent) in witness {
                    let _ = writeln!(
                        out,
                        "      {:#010x} --{:#010x}--> {callee:#010x} {}",
                        parent.caller,
                        parent.site,
                        self.image.function_at(callee)
                    );
                }
            }
        }
        for pattern in &self.unmatched {
            let _ = writeln!(
                out,
                "  {}\tlinked=0\tselector={pattern}",
                Verdict::of(false, bound).label()
            );
        }
        // Root holes apply to every unknown target; print the full list once.
        for &(site, reason) in &bound.unresolved {
            let _ = writeln!(
                out,
                "  hole\t{site:#010x}\t{reason}\t{}",
                self.image.function_at(site)
            );
        }
        for assumption in &bound.assumptions {
            let _ = writeln!(out, "  assumption\t{assumption}");
        }
        out.push('\n');
        out
    }
}

#[cfg(test)]
mod tests;
