//! Targets that facts outside the machine code give the indirect sites the
//! analysis left unresolved, with the fact each set comes from.
//!
//! Every fact gives a superset of what its sites can reach, so the facts of
//! one site unite. An empty set resolves a site only where the fact is
//! complete for it ([`Fact::proves_empty`]); otherwise it is a hole
//! (a no-candidate hole), never a site that reaches nothing.
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Where a site's targets come from.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Fact {
    /// The image's static interrupt table: a dispatcher reaches the
    /// handlers of its level and core.
    InterruptTable,
    /// The image's one waker vtable, the executor's, by a census by type
    /// and by layout: every use of a typed waker clone function's address is
    /// that vtable's first word, and every row of four function pointers in
    /// data headed by a function the DWARF does not type is classified, so
    /// every waker the image builds carries it. It names no [`Assumption`];
    /// the census rests on the DWARF and the kept relocations being whole
    /// (a run-time vtable from an untyped clone function, or one in an
    /// executable section, is not found) and on memory safety (no unsafe
    /// code forges a vtable or a waker's vtable pointer).
    WakerVtables,
    /// The functions whose type can be a static field's function pointer.
    FieldType,
    /// The jump out of the bootstrap into the stage-two entry the stack
    /// policy names. The checks verify, in both ELFs, that the site jumps
    /// to the entry and that the entry installs its stack pointer before any
    /// stack access; the bootstrap's stack path ends there.
    StageTwoEntry,
}

/// What a resolution takes for granted that the analysis does not prove: a
/// bound resting on one is conditional, and names it.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub enum Assumption {
    /// The executor invariant: a pointer erased to `()` or to bytes is read
    /// back as its own type. Every fact found from the image's types but not
    /// enumerated whole rests on it: a field's function reaches a site only
    /// if no reinterpreted memory puts another function there. The points-to
    /// analysis of the interrupt-reachable sites is to prove it.
    ExecutorInvariant,
}

impl Assumption {
    /// The assumption's short name, for a report's label.
    pub fn name(self) -> &'static str {
        match self {
            Self::ExecutorInvariant => "ExecutorInvariant",
        }
    }
}

impl std::fmt::Display for Assumption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::ExecutorInvariant => {
                "the executor invariant (an erased pointer is read back as its own type)"
            }
        })
    }
}

impl Fact {
    /// What a resolution from this fact assumes: the interrupt table lists
    /// every handler outright and the waker vtable census classifies every
    /// vtable the DWARF and relocations show; the other facts found from
    /// types rest on the executor invariant.
    pub fn assumption(self) -> Option<Assumption> {
        match self {
            Self::InterruptTable | Self::StageTwoEntry | Self::WakerVtables => None,
            Self::FieldType => Some(Assumption::ExecutorInvariant),
        }
    }

    /// Whether an empty set from this fact proves the site reaches nothing:
    /// the interrupt table lists every handler. Waker vtables and field
    /// types are found, not listed, so their empty set is a hole.
    pub fn proves_empty(self) -> bool {
        matches!(self, Self::InterruptTable)
    }
}

/// One site's targets and the facts that gave them.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Resolution {
    pub targets: BTreeSet<u32>,
    pub facts: BTreeSet<Fact>,
}

impl Resolution {
    /// Whether the site reaches exactly `targets`: a nonempty set, or an
    /// empty one a fact proves.
    pub fn resolves(&self) -> bool {
        !self.targets.is_empty() || self.facts.iter().any(|fact| fact.proves_empty())
    }
}

/// The resolutions of an image's indirect sites, by site.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Resolutions {
    sites: BTreeMap<u32, Resolution>,
    /// Addresses a transfer leaves the analysed stack for good: the stack
    /// below the transfer belongs to the code it enters, not to this root.
    exits: BTreeSet<u32>,
    /// The direct calls by which an executor polls one of its tasks: each
    /// enters the task's code.
    task_polls: BTreeSet<u32>,
    /// The functions those calls enter: each task's root.
    task_roots: BTreeSet<u32>,
}

impl Resolutions {
    pub fn new() -> Self {
        Self::default()
    }

    /// The bytes [`Self::add`] of `targets` targets may request, for a
    /// builder to admit first: the site's entry, its fact and every target.
    pub fn admission(targets: usize) -> usize {
        size_of::<(u32, Resolution)>() + size_of::<Fact>() + targets * size_of::<u32>()
    }

    /// Add the `targets` that `fact` gives `site`, to those other facts give.
    pub fn add(&mut self, site: u32, fact: Fact, targets: impl IntoIterator<Item = u32>) {
        let resolution = self.sites.entry(site).or_default();
        resolution.facts.insert(fact);
        resolution.targets.extend(targets);
    }

    /// Declare `address` an exit: a transfer that reaches it ends its path.
    pub fn add_exit(&mut self, address: u32) {
        self.exits.insert(address);
    }

    pub fn is_exit(&self, address: u32) -> bool {
        self.exits.contains(&address)
    }

    /// Declare the call at `site` an executor's poll of a task.
    pub fn add_task_poll(&mut self, site: u32) {
        self.task_polls.insert(site);
    }

    /// The calls by which an executor polls its tasks.
    pub fn task_polls(&self) -> &BTreeSet<u32> {
        &self.task_polls
    }

    /// Declare the function at `address` a task's root: the function an
    /// executor's task poll calls.
    pub fn add_task_root(&mut self, address: u32) {
        self.task_roots.insert(address);
    }

    /// The roots of the tasks the executor polls.
    pub fn task_roots(&self) -> &BTreeSet<u32> {
        &self.task_roots
    }

    /// Add every resolution, exit, task poll and task root of `other`.
    pub fn extend(&mut self, other: Resolutions) {
        self.exits.extend(other.exits);
        self.task_polls.extend(other.task_polls);
        self.task_roots.extend(other.task_roots);
        for (site, resolution) in other.sites {
            let into = self.sites.entry(site).or_default();
            into.facts.extend(resolution.facts);
            into.targets.extend(resolution.targets);
        }
    }

    pub fn get(&self, site: u32) -> Option<&Resolution> {
        self.sites.get(&site)
    }

    pub fn iter(&self) -> impl Iterator<Item = (u32, &Resolution)> {
        self.sites
            .iter()
            .map(|(&site, resolution)| (site, resolution))
    }

    pub fn len(&self) -> usize {
        self.sites.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sites.is_empty()
    }
}
