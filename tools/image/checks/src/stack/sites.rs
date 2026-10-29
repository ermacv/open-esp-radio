//! The indirect sites each root's bound takes, read from the bound itself
//! ([`Bound::sites`]): where each site is, how many targets it reaches, the
//! fact that gives them and what that fact assumes. It never walks the call
//! graph or resolves a site again.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use oer_image_stack::{Assumption, Bound, CodeFact, Fact, SiteSource};
use serde::Serialize;

use super::{Image, Result, bound_state};

pub const RUNTIME_REPORT: &str = "runtime-indirect-sites.txt";
pub const RUNTIME_JSON: &str = "runtime-indirect-sites.json";
pub const BOOTSTRAP_REPORT: &str = "bootstrap-indirect-sites.txt";
pub const BOOTSTRAP_JSON: &str = "bootstrap-indirect-sites.json";

/// Where a site's targets come from: the machine code's own resolutions,
/// then the facts outside it ([`Fact`]).
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum Kind {
    JumpTable,
    ValueAnalysis,
    StageTwoEntry,
    WakerVtables,
    FieldType,
}

impl Kind {
    fn of_code(fact: CodeFact) -> Self {
        match fact {
            CodeFact::JumpTable => Self::JumpTable,
            CodeFact::ValueAnalysis => Self::ValueAnalysis,
        }
    }

    fn of_fact(fact: Fact) -> Self {
        match fact {
            Fact::StageTwoEntry => Self::StageTwoEntry,
            Fact::WakerVtables => Self::WakerVtables,
            Fact::FieldType => Self::FieldType,
        }
    }
}

/// One indirect site a root's bound takes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Site {
    pub site: u32,
    /// The function that holds the site, and the site's offset in it.
    pub function: String,
    pub offset: u32,
    /// The distinct functions (or exits) the site reaches.
    pub targets: usize,
    pub facts: Vec<Kind>,
    /// What the facts assume; empty when the targets are proven.
    pub assumptions: Vec<Assumption>,
}

/// One root's (or interrupt level's) taken indirect sites.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Root {
    pub label: String,
    pub root: String,
    /// The bound's state: `proven`, `conditional (…)` or `partial`.
    pub state: String,
    pub sites: Vec<Site>,
    /// The root's sites by fact; a site with two facts counts under both.
    pub by_fact: BTreeMap<Kind, usize>,
}

/// The distinct sites over every root.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct Totals {
    pub sites: usize,
    pub by_fact: BTreeMap<Kind, usize>,
    /// Sites by assumption; the sites without one are proven.
    pub by_assumption: BTreeMap<Assumption, usize>,
    pub proven: usize,
}

/// The indirect sites of one ELF's roots.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct Report {
    pub roots: Vec<Root>,
    pub totals: Totals,
}

impl Report {
    /// Add `bound`'s sites as the root `label`.
    pub fn add(&mut self, image: &Image, label: &str, bound: &Bound) {
        let mut sites = Vec::new();
        let mut by_fact = BTreeMap::new();
        for (&address, taken) in &bound.sites {
            let facts: Vec<Kind> = match &taken.source {
                SiteSource::Code(fact) => vec![Kind::of_code(*fact)],
                SiteSource::Facts(facts) => facts.iter().map(|&fact| Kind::of_fact(fact)).collect(),
            };
            for &kind in &facts {
                *by_fact.entry(kind).or_insert(0) += 1;
            }
            let (function, offset) = image
                .containing(address)
                .unwrap_or_else(|| (format!("{address:#010x}"), 0));
            sites.push(Site {
                site: address,
                function,
                offset,
                targets: taken.targets,
                facts,
                assumptions: taken.source.assumptions().into_iter().collect(),
            });
        }
        let root = image
            .containing(bound.root)
            .map_or_else(|| format!("{:#010x}", bound.root), |(name, _)| name);
        self.roots.push(Root {
            label: label.to_owned(),
            root,
            state: bound_state(
                bound.bytes,
                &bound.assumptions,
                !bound.level_drops.is_empty(),
            ),
            sites,
            by_fact,
        });
        self.totals = self.totals();
    }

    fn totals(&self) -> Totals {
        let distinct: BTreeMap<u32, &Site> = self
            .roots
            .iter()
            .flat_map(|root| root.sites.iter())
            .map(|site| (site.site, site))
            .collect();
        let mut totals = Totals {
            sites: distinct.len(),
            ..Totals::default()
        };
        for site in distinct.values() {
            for &kind in &site.facts {
                *totals.by_fact.entry(kind).or_insert(0) += 1;
            }
            for &assumption in &site.assumptions {
                *totals.by_assumption.entry(assumption).or_insert(0) += 1;
            }
            if site.assumptions.is_empty() {
                totals.proven += 1;
            }
        }
        totals
    }

    pub fn render(&self) -> String {
        let mut out = String::from(
            "Indirect sites each root's bound takes: the targets, the fact that gives them \
             and what it assumes.\nJumpTable and ValueAnalysis are the machine code's own; \
             the other facts come from outside it.\n",
        );
        for root in &self.roots {
            let _ = writeln!(
                out,
                "root {} (`{}`): {}; {} sites{}",
                root.label,
                root.root,
                root.state,
                root.sites.len(),
                counts(&root.by_fact)
            );
            if !root.sites.is_empty() {
                let _ = writeln!(out, "  site\ttargets\tfacts\tassumption\tfunction+offset");
            }
            for site in &root.sites {
                let facts: Vec<String> =
                    site.facts.iter().map(|kind| format!("{kind:?}")).collect();
                let assumptions: Vec<&str> = site
                    .assumptions
                    .iter()
                    .map(|assumption| assumption.name())
                    .collect();
                let _ = writeln!(
                    out,
                    "  {:#010x}\t{}\t{}\t{}\t{}+{:#x}",
                    site.site,
                    site.targets,
                    facts.join(","),
                    if assumptions.is_empty() {
                        "none".to_owned()
                    } else {
                        assumptions.join(",")
                    },
                    site.function,
                    site.offset
                );
            }
        }
        let totals = &self.totals;
        let _ = writeln!(
            out,
            "totals: {} distinct sites{}",
            totals.sites,
            counts(&totals.by_fact)
        );
        let mut assumptions: Vec<String> = totals
            .by_assumption
            .iter()
            .map(|(assumption, count)| format!("{} {count}", assumption.name()))
            .collect();
        assumptions.push(format!("none {}", totals.proven));
        let _ = writeln!(out, "by assumption: {}", assumptions.join(", "));
        out
    }

    /// Write the text and JSON reports into `output`.
    pub fn write(&self, output: &Path, text: &str, json: &str) -> Result<()> {
        std::fs::write(output.join(text), self.render())?;
        std::fs::write(output.join(json), serde_json::to_string_pretty(self)?)?;
        Ok(())
    }
}

fn counts(by_fact: &BTreeMap<Kind, usize>) -> String {
    if by_fact.is_empty() {
        return String::new();
    }
    let counts: Vec<String> = by_fact
        .iter()
        .map(|(kind, count)| format!("{kind:?} {count}"))
        .collect();
    format!(": {}", counts.join(", "))
}
