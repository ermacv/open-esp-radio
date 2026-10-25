//! Typed load errors: each names the file, the entity and the rule.

use std::path::{Path, PathBuf};

/// A chip model that does not load: the file, the entity in it and the
/// rule it breaks.
#[derive(Debug, thiserror::Error)]
#[error("{}: {entity}: {kind}", path.display())]
pub struct Error {
    pub path: PathBuf,
    /// The entity the rule concerns: `TYPE`, `TYPE.REGISTER.FIELD`, an
    /// instance, an interrupt, a region or a layout.
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

/// The rule a chip model breaks.
#[derive(Debug, thiserror::Error)]
pub enum ErrorKind {
    #[error("cannot read the file: {0}")]
    Read(#[source] std::io::Error),
    #[error("invalid TOML or an unknown key: {0}")]
    Parse(String),
    #[error("requires schema = {expected}, found {found}")]
    Schema { expected: u32, found: u32 },
    #[error("include {0:?} is not a safe relative path")]
    UnsafePath(String),
    #[error("include {0:?} is listed twice")]
    DuplicateInclude(String),
    #[error("invalid name: {0}")]
    InvalidName(String),
    #[error("{0} is declared twice")]
    Duplicate(String),
    #[error("sources are empty, empty strings or repeated")]
    InvalidSources,
    #[error("cites evidence source {0:?}, which no catalog defines")]
    UnknownSource(String),
    #[error("needs a description")]
    MissingDescription,
    #[error("register geometry: {0}")]
    RegisterGeometry(String),
    #[error("field bits {bits:?}: {reason}")]
    FieldBits { bits: String, reason: String },
    #[error("field overlaps field {0}")]
    FieldOverlap(String),
    #[error("reserved mask: {0}")]
    Reserved(String),
    #[error("reset: {0}")]
    Reset(String),
    #[error("values or write constraint: {0}")]
    Values(String),
    #[error("pointer: {0}")]
    Pointer(String),
    #[error("interrupt pair: {0}")]
    InterruptPair(String),
    #[error("dim: {0}")]
    Dim(String),
    #[error("group: {0}")]
    Group(String),
    #[error("expansion exceeds the loader's {0} budget")]
    ExpansionLimit(&'static str),
    #[error("cannot allocate expanded records: {0}")]
    Allocation(#[source] std::collections::TryReserveError),
    #[error("word at offset {offset:#x} overlaps {other}")]
    WordOverlap { offset: u64, other: String },
    #[error("command register: {0}")]
    Command(String),
    #[error("port: {0}")]
    Port(String),
    #[error("{0} is allowed only in an analog-I2C type")]
    AnalogOnly(&'static str),
    #[error("read-safe: {0}")]
    ReadSafe(String),
    #[error("view: {0}")]
    View(String),
    #[error("review override names no register or field of the type")]
    ReviewUnknownEntity,
    #[error("review override repeats the default's {0}")]
    ReviewRepeatsDefault(&'static str),
    #[error("review has no {0} after its default and override")]
    ReviewIncomplete(&'static str),
    #[error("a hint is navigation, not a reviewed assertion")]
    ReviewHint,
    #[error("naming: {0}")]
    Naming(String),
    #[error("names type {0:?}, which no included file declares")]
    UnknownType(String),
    #[error("type {block_type} is a {found} type, not a {expected} one")]
    WrongSpace {
        block_type: String,
        expected: &'static str,
        found: &'static str,
    },
    #[error("extent: {0}")]
    Extent(String),
    #[error("{0} lies outside the instance's extent")]
    OutsideExtent(String),
    #[error("address range overlaps {0}")]
    AddressOverlap(String),
    #[error("interrupt source number {0} is used twice")]
    DuplicateSourceNumber(u16),
    #[error("names instance {0:?}, which the root does not declare")]
    UnknownInstance(String),
    #[error("interrupt output: {0}")]
    Output(String),
    #[error("interrupt coverage: {0}")]
    InterruptCoverage(String),
    #[error("interrupt source {0} is neither bound nor explicitly unbound")]
    MissingInterruptSource(u16),
    #[error("names no expanded MMIO field: {0}")]
    UnknownField(String),
    #[error(
        "{kind} field {field} has class {class} and width {width}; gates require one plain bit; resets require one plain, write-only or write-trigger bit; muxes require plain"
    )]
    ControlField {
        field: String,
        kind: &'static str,
        class: &'static str,
        width: u32,
    },
    #[error("control link: {0}")]
    Control(String),
    #[error("shared control: {0}")]
    SharedControl(String),
    #[error("gate group: {0}")]
    GateGroup(String),
    #[error("clock source: {0}")]
    ClockSource(String),
    #[error("analog-I2C route: {0}")]
    Route(String),
    #[error("analog-I2C block code {0:#04x} is used twice")]
    DuplicateCode(u8),
    #[error("memory layouts: {0}")]
    Layouts(#[source] Box<crate::Error>),
    #[error("layout placement: {0}")]
    Placement(String),
    #[error("RAM region: {0}")]
    RamRegion(String),
}
