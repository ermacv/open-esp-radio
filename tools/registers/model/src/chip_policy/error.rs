//! Typed load errors: each names the policy file, the entity and the rule.

use std::path::{Path, PathBuf};

/// A policy that does not load: the file, the entity in it and the rule it
/// breaks.
#[derive(Debug, thiserror::Error)]
#[error("{}: {entity}: {kind}", path.display())]
pub struct Error {
    pub path: PathBuf,
    /// The entity the rule concerns: `Partition.Part`, a word
    /// `INSTANCE.REGISTER[.FIELD]`, a domain or a transaction.
    pub entity: String,
    pub kind: ErrorKind,
}

impl Error {
    pub(crate) fn new(path: &Path, entity: impl Into<String>, kind: ErrorKind) -> Self {
        Self {
            path: path.to_owned(),
            entity: entity.into(),
            kind,
        }
    }
}

/// The rule a policy breaks.
#[derive(Debug, thiserror::Error)]
pub enum ErrorKind {
    #[error("cannot read the file: {0}")]
    Read(#[source] std::io::Error),
    #[error("invalid TOML or an unknown key: {0}")]
    Parse(String),
    #[error("requires schema = {expected}, found {found}")]
    Schema { expected: u32, found: u32 },
    #[error("is for chip {found:?}, the root is {expected:?}")]
    Chip { expected: String, found: String },
    #[error("invalid name: {0}")]
    InvalidName(String),
    #[error("{0} is declared twice")]
    Duplicate(String),
    #[error("needs a description or reason")]
    MissingDescription,
    #[error("sources are empty, empty strings or repeated")]
    InvalidSources,
    #[error("cites evidence source {0:?}, which no catalog defines")]
    UnknownSource(String),
    #[error("cannot allocate: {0}")]
    Allocation(#[source] std::collections::TryReserveError),
    #[error("names instance {0:?}, which is no MMIO instance of the root")]
    UnknownInstance(String),
    #[error("names {0:?}, which is no register of the instance or the root")]
    UnknownRegister(String),
    #[error("names {0:?}, which is no field of its register")]
    UnknownField(String),
    #[error("a partition holds at least one part")]
    EmptyPartition,
    #[error("a part or unclaimed entry selects no word")]
    EmptySelection,
    #[error("unclaimed: {0}")]
    UnclaimedForm(String),
    #[error("elements of {0} lie outside its template or select none")]
    ElementRange(String),
    #[error("is held by {0} already")]
    ClaimedTwice(String),
    #[error("is neither held by a part nor unclaimed")]
    Uncovered,
    #[error("triggers: {0}")]
    Triggers(String),
    #[error("observes: {0}")]
    Observes(String),
    #[error("flags: {0}")]
    Flags(String),
    #[error("shared part: {0}")]
    Shared(String),
    #[error("read-only part: {0}")]
    ReadOnly(String),
    #[error("domain: {0}")]
    Domain(String),
    #[error("reaches {0}, which is unclaimed")]
    ReachesUnclaimed(String),
    #[error("reaches words of more than one part: {0}")]
    SeveralOwners(String),
    #[error("field class: {0}")]
    FieldClass(String),
    #[error("register: {0}")]
    Register(String),
    #[error("write-one-to-clear: {0}")]
    WriteOneToClear(String),
    #[error("snapshot: {0}")]
    Snapshot(String),
    #[error("DMA address: {0}")]
    Pointer(String),
    #[error("write constraint: {0}")]
    WriteConstraint(String),
    #[error("transaction: {0}")]
    Transaction(String),
}
