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
    /// The waker vtables of the image.
    WakerVtables,
    /// The functions whose type can be a static field's function pointer.
    FieldType,
    /// Every task poll of the image, for the executor's call of a dequeued
    /// task's `poll_fn`.
    TaskPolls,
    /// Neither the image's functions nor its concrete DWARF instances
    /// contain a task poll: no task can reach the executor's dequeue call.
    NoTasks,
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
    /// The executor invariant: a task header's `poll_fn` matches the storage
    /// it heads, and a pointer erased to `()` or to bytes is read back as its
    /// own type. Every fact found from the image's types rests on it: a waker
    /// vtable or a field's function reaches a site only if
    /// no reinterpreted memory puts another function there. The points-to
    /// analysis of the interrupt-reachable sites is to prove it.
    ExecutorInvariant,
}

impl std::fmt::Display for Assumption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::ExecutorInvariant => {
                "the executor invariant (a task header's poll_fn matches its storage, and an \
                 erased pointer is read back as its own type)"
            }
        })
    }
}

impl Fact {
    /// What a resolution from this fact assumes: the interrupt table lists
    /// every handler outright; the facts found from types rest on the
    /// executor invariant.
    pub fn assumption(self) -> Option<Assumption> {
        match self {
            Self::InterruptTable | Self::StageTwoEntry => None,
            Self::WakerVtables | Self::FieldType | Self::TaskPolls | Self::NoTasks => {
                Some(Assumption::ExecutorInvariant)
            }
        }
    }

    /// Whether an empty set from this fact proves the site reaches nothing:
    /// the interrupt table lists every handler. No tasks proves the executor's
    /// dequeue call unreachable. Waker vtables and field types are found,
    /// not listed, so their empty set is a hole.
    pub fn proves_empty(self) -> bool {
        matches!(self, Self::InterruptTable | Self::NoTasks)
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

    /// Add every resolution and exit of `other`.
    pub fn extend(&mut self, other: Resolutions) {
        self.exits.extend(other.exits);
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
