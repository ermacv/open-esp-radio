//! Selection and grouped presentation of `register-accesses` records.
//!
//! A filter selects observations only. Blocked functions and gaps stay in the
//! output and the summary always counts the whole analysis, so a selection
//! cannot hide that the analysis was incomplete.
use blobray_domain::{
    IndexedAddress, RegisterAccess, RegisterMask, RegisterMaskKind, StoredBits, StoredBitsSource,
};
use oer_riscv_model::{FunctionRecord, MemoryKind, ObjectLocation, SymbolId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Which observations a `register-accesses` request keeps.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AccessFilter {
    /// Word addresses; an observation is kept when its address lies in one of
    /// these 32-bit words, or its indexed progression may reach one. Empty
    /// keeps every address, unresolved included.
    pub words: Vec<u32>,
    /// Function names; empty keeps every function.
    pub functions: Vec<String>,
}

impl AccessFilter {
    /// Whether `record` belongs in the output: every blocked function and gap,
    /// and each observation the filter selects.
    pub fn keeps(&self, record: &RegisterAccess) -> bool {
        match record {
            RegisterAccess::Observation {
                function,
                address,
                indexed,
                ..
            } => {
                let word = self.words.is_empty()
                    || address.is_some_and(|address| self.words.contains(&(address & !3)))
                    || indexed.is_some_and(|indexed| {
                        self.words.iter().any(|word| indexed.may_reach_word(*word))
                    });
                word && self.names(function.name.as_deref())
            }
            RegisterAccess::Blocked { function, .. } => self.names(function.name.as_deref()),
            RegisterAccess::Gap { .. } => true,
        }
    }

    fn names(&self, name: Option<&[u8]>) -> bool {
        self.functions.is_empty()
            || name.is_some_and(|name| {
                self.functions
                    .iter()
                    .any(|wanted| wanted.as_bytes() == name)
            })
    }
}

/// The key observations are grouped by.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GroupBy {
    /// One group per accessed 32-bit word, with the functions accessing it.
    Address,
    /// One group per function, with the words it accesses.
    Function,
}

/// One group of observations sharing an address or a function.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessGroup {
    /// The 32-bit word every observation of the group lies in; `None` groups
    /// unresolved addresses (address grouping) or is absent (function grouping).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word: Option<u32>,
    /// The function, as `input` and name; absent for address grouping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<GroupFunction>,
    pub entries: Vec<AccessEntry>,
}

/// A function by its input position, display name and symbol identity.
///
/// The symbol keeps functions apart that share a name: local functions of
/// different archive members, unnamed functions and names equal only after
/// lossy UTF-8 decoding. Groups order by input, then name, then symbol.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupFunction {
    pub input: u64,
    /// The symbol name, lossily decoded for display; empty when unnamed.
    pub name: String,
    pub symbol: SymbolId,
}

impl GroupFunction {
    /// `name (input 0, member 3)`; `<unnamed>` for a function without a name
    /// and no member for a standalone ELF.
    pub fn human(&self) -> String {
        let name = if self.name.is_empty() {
            "<unnamed>"
        } else {
            &self.name
        };
        match self.symbol.object.location {
            ObjectLocation::ArchiveMember { ordinal } => {
                format!("{name} (input {}, member {ordinal})", self.input)
            }
            _ => format!("{name} (input {})", self.input),
        }
    }
}

/// Observations of one group that agree on the other key, access and mask.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessEntry {
    /// The accessed word, for function grouping; `None` there is unresolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub word: Option<u32>,
    /// The progression of an unresolved address computed from one index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indexed: Option<IndexedAddress>,
    /// The accessing function, for address grouping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<GroupFunction>,
    /// `load`, `store`, `load-reserved`, `store-conditional`, `atomic`,
    /// `float-load`, `float-store`; `expression` for a masked load/store
    /// expression; `other` for any other fact.
    pub access: String,
    pub width: Option<u8>,
    pub mask: Option<RegisterMask>,
    /// For stores, where each run of the stored bits comes from; entries
    /// with different sources stay apart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stored: Option<Vec<StoredBits>>,
    pub count: u64,
}

/// A group: its word or its function.
type GroupKey = (Option<u32>, Option<GroupFunction>);
/// A group's entries with each entry's mask and count.
type Entries = BTreeMap<EntryKey, (Option<RegisterMask>, Option<Vec<StoredBits>>, u64)>;
type EntryKey = (
    Option<u32>,
    Option<IndexedAddress>,
    Option<GroupFunction>,
    String,
    Option<u8>,
    Option<(u8, u32)>,
    Option<Vec<StoredBits>>,
);

/// Aggregates selected observations into groups in key order.
#[derive(Clone, Debug)]
pub struct AccessGroups {
    by: GroupBy,
    groups: BTreeMap<GroupKey, Entries>,
}

impl AccessGroups {
    pub fn new(by: GroupBy) -> Self {
        Self {
            by,
            groups: BTreeMap::new(),
        }
    }

    /// Count `record` when it is an observation; other records are not grouped.
    pub fn add(&mut self, record: &RegisterAccess) {
        let RegisterAccess::Observation {
            function,
            fact,
            address,
            indexed,
            mask,
            stored,
            ..
        } = record
        else {
            return;
        };
        // The sources themselves key an entry: two runs with equal text, such
        // as loads of different widths, stay apart.
        let stored_key = stored.clone();
        let function = GroupFunction {
            input: function.input,
            name: function
                .name
                .as_deref()
                .map(|name| String::from_utf8_lossy(name).into_owned())
                .unwrap_or_default(),
            symbol: function.symbol.clone(),
        };
        let word = address.map(|address| address & !3);
        let (access, width) = access(fact);
        let mask_key = mask.map(|mask| (mask_kind(mask.kind), mask.bits));
        let (group, entry) = match self.by {
            GroupBy::Address => (
                (word, None),
                (
                    None,
                    *indexed,
                    Some(function),
                    access,
                    width,
                    mask_key,
                    stored_key,
                ),
            ),
            GroupBy::Function => (
                (None, Some(function)),
                (word, *indexed, None, access, width, mask_key, stored_key),
            ),
        };
        let slot = self
            .groups
            .entry(group)
            .or_default()
            .entry(entry)
            .or_insert((*mask, stored.clone(), 0));
        slot.2 += 1;
    }

    /// The groups in key order: addresses ascending with unresolved first, or
    /// functions by input and name.
    pub fn groups(&self) -> Vec<AccessGroup> {
        self.groups
            .iter()
            .map(|((word, function), entries)| AccessGroup {
                word: *word,
                function: function.clone(),
                entries: entries
                    .iter()
                    .map(
                        |(
                            (word, indexed, function, access, width, _, _),
                            (mask, stored, count),
                        )| {
                            AccessEntry {
                                word: *word,
                                indexed: *indexed,
                                function: function.clone(),
                                access: access.clone(),
                                width: *width,
                                mask: *mask,
                                stored: stored.clone(),
                                count: *count,
                            }
                        },
                    )
                    .collect(),
            })
            .collect()
    }

    /// One header line per group and one indented line per entry.
    pub fn human(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for group in self.groups() {
            lines.push(match (&group.function, group.word) {
                (Some(function), _) => function.human(),
                (None, Some(word)) => format!("{word:#010x}"),
                (None, None) => String::from("unresolved"),
            });
            for entry in &group.entries {
                let subject = match (&entry.function, entry.word, entry.indexed) {
                    (Some(function), _, _) => function.human(),
                    (None, Some(word), _) => format!("{word:#010x}"),
                    (None, None, Some(indexed)) => indexed_human(&indexed),
                    (None, None, None) => String::from("unresolved"),
                };
                let indexed = match (&entry.function, entry.indexed) {
                    (Some(_), Some(indexed)) => format!(" {}", indexed_human(&indexed)),
                    _ => String::new(),
                };
                let width = entry
                    .width
                    .map(|width| format!(" {width}B"))
                    .unwrap_or_default();
                let mask = entry
                    .mask
                    .map(|mask| match mask.kind {
                        RegisterMaskKind::ReadSelection => format!(" read {:#010x}", mask.bits),
                        RegisterMaskKind::WriteReplacement => {
                            format!(" replace {:#010x}", mask.bits)
                        }
                    })
                    .unwrap_or_default();
                let stored = entry
                    .stored
                    .as_deref()
                    .map(|runs| format!(" {}", stored_human(runs)))
                    .unwrap_or_default();
                lines.push(format!(
                    "  {subject}{indexed} {}{width}{mask}{stored} x{}",
                    entry.access, entry.count
                ));
            }
        }
        lines
    }
}

/// `indexed 0x2010f400+0x10*i` for an indexed progression, with `(i<6)` when
/// the index is bounded; a negative stride prints as `-0x7c*i`.
pub fn indexed_human(indexed: &IndexedAddress) -> String {
    let sign = if indexed.stride < 0 { '-' } else { '+' };
    let bound = indexed
        .count
        .map(|count| format!(" (i<{count})"))
        .unwrap_or_default();
    format!(
        "indexed {:#010x}{sign}{:#x}*i{bound}",
        indexed.base,
        indexed.stride.unsigned_abs()
    )
}

/// The runs of a stored value, high bits first: `[24:18]=arg0[6:0]` for an
/// argument's bits (`entry x8[5:4]` for another entry register),
/// `[17:0]=kept` for bits a read-modify-write keeps in place,
/// `=same word[9:2]` for the stored word's bits moved elsewhere, `=0x1b` for
/// fixed bits, `=load 4B 0x20109004[7:0]` for another loaded value with its
/// load width and `=?` for an unknown source.
pub fn stored_human(runs: &[StoredBits]) -> String {
    let range = |low: u8, width: u8| {
        if width == 1 {
            format!("[{low}]")
        } else {
            format!("[{}:{low}]", low + width - 1)
        }
    };
    runs.iter()
        .rev()
        .map(|run| {
            let source = match &run.source {
                StoredBitsSource::Constant { value } => format!("{value:#x}"),
                StoredBitsSource::EntryRegister { register, low }
                    if (10..18).contains(register) =>
                {
                    format!("arg{}{}", register - 10, range(*low, run.width))
                }
                StoredBitsSource::EntryRegister { register, low } => {
                    format!("entry x{register}{}", range(*low, run.width))
                }
                StoredBitsSource::Load {
                    same_word: true,
                    low,
                    ..
                } if *low == run.low => String::from("kept"),
                StoredBitsSource::Load {
                    address,
                    width,
                    low,
                    same_word,
                } => {
                    let word = match (same_word, address) {
                        (true, _) => String::from("same word"),
                        (false, Some(address)) => format!("load {width}B {address:#x}"),
                        (false, None) => format!("load {width}B"),
                    };
                    format!("{word}{}", range(*low, run.width))
                }
                StoredBitsSource::Unknown => String::from("?"),
            };
            format!("{}={source}", range(run.low, run.width))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn mask_kind(kind: RegisterMaskKind) -> u8 {
    match kind {
        RegisterMaskKind::ReadSelection => 0,
        RegisterMaskKind::WriteReplacement => 1,
    }
}

fn access(fact: &FunctionRecord) -> (String, Option<u8>) {
    match fact {
        FunctionRecord::MemoryAccess { access, width, .. } => {
            let name = match access {
                MemoryKind::Load => "load",
                MemoryKind::Store => "store",
                MemoryKind::LoadReserved => "load-reserved",
                MemoryKind::StoreConditional => "store-conditional",
                MemoryKind::Atomic => "atomic",
                MemoryKind::FloatLoad => "float-load",
                MemoryKind::FloatStore => "float-store",
            };
            (name.into(), Some(*width))
        }
        FunctionRecord::Expression { .. } => ("expression".into(), None),
        _ => ("other".into(), None),
    }
}

#[cfg(test)]
mod tests;
