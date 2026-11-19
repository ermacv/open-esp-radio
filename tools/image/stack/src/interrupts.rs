//! An image's interrupt table: for each binding its source, CPU line,
//! level, handler class, hart and handler, read from the link's facts; and
//! its level tasks, a table of their own: for each its line, level, hart and
//! handler, with no source and no class. Each table is the slice an exported
//! symbol holds (its pointer word relocated to the entries, then its length);
//! every non-zero handler word carries the relocation that names it, and a
//! zero word is an entry the build left out.
use oer_tools_analysis_riscv_model::{Error, ErrorCode, Result, RunControl};
use oer_tools_analysis_riscv_program::facts::ImageFacts;

/// Where one field lies in a table entry, in bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Field {
    pub offset: u32,
    pub size: u32,
}

/// The layout of a chip's table entry: the platform's contract, which its
/// own build asserts beside the binding type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TableLayout {
    /// Bytes per entry.
    pub entry: u32,
    pub source: Field,
    pub line: Field,
    pub level: Field,
    pub class: Field,
    pub core: Field,
    /// Offset of the handler's address word.
    pub handler: u32,
}

/// The layout of a chip's level task: the platform's contract, which its
/// own build asserts beside the task's type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskLayout {
    /// Bytes per task.
    pub entry: u32,
    pub line: Field,
    pub level: Field,
    pub core: Field,
    /// Offset of the handler's address word.
    pub handler: u32,
}

/// What an entry's handler may do, as the image declares it: the first
/// three by the interrupt table's class byte
/// (`oer_chips_family_platform_interrupt_table::Class`, whose discriminants
/// these are), a level task by its own table.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Class {
    /// Strict code throughout: no task poll, no waker call, never on the
    /// level of its core's executor.
    Hardware,
    /// An executor's interrupt: strict up to its task polls.
    Executor,
    /// A hardware handler that breaks the one named rule.
    Exempt(Violation),
    /// A level's one task: strict code as [`Class::Hardware`], alone on its
    /// level of its core. No class byte names it: the task table is its
    /// class.
    LevelTask,
}

/// The one hardware rule an exempt entry breaks.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Violation {
    /// It sits on the level of its core's executor.
    ExecutorLevel,
    /// It calls a waker.
    WakerCall,
}

/// The byte of `oer_chips_family_platform_interrupt_table::Class::LevelTask`,
/// which the image exports in its task table and never in the source table.
const LEVEL_TASK_BYTE: u32 = 4;

impl Class {
    fn of(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Hardware),
            1 => Some(Self::Executor),
            2 => Some(Self::Exempt(Violation::ExecutorLevel)),
            3 => Some(Self::Exempt(Violation::WakerCall)),
            _ => None,
        }
    }
}

impl std::fmt::Display for Class {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(match self {
            Self::Hardware => "hardware",
            Self::Executor => "executor",
            Self::Exempt(Violation::ExecutorLevel) => "exempt (shares its core's executor level)",
            Self::Exempt(Violation::WakerCall) => "exempt (calls a waker)",
            Self::LevelTask => "level task",
        })
    }
}

/// What an entry binds: a source routed to its line, with the class its
/// handler declares, or a level's task, whose line has no source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Kind {
    Routed { source: u32, class: Class },
    Task,
}

/// One entry of the tables: an interrupt table's binding or a level task.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TableEntry {
    pub kind: Kind,
    /// The CPU line whose vector calls `handler`: a routed source's, or the
    /// task's own.
    pub line: u32,
    pub level: u32,
    pub core: u32,
    /// The handler's address; `None` for an entry the build left out.
    pub handler: Option<u32>,
}

impl TableEntry {
    /// The entry's class: its handler's declared one, or a level task's.
    pub fn class(&self) -> Class {
        match self.kind {
            Kind::Routed { class, .. } => class,
            Kind::Task => Class::LevelTask,
        }
    }
}

impl std::fmt::Display for TableEntry {
    /// The entry as a report names it: by its source, or as the level task
    /// of its line.
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.kind {
            Kind::Routed { source, .. } => write!(out, "source {source}"),
            Kind::Task => write!(out, "the level task of line {}", self.line),
        }
    }
}

/// The entries of the table `symbol` holds in `image`.
pub fn interrupt_table(
    image: &ImageFacts<'_>,
    symbol: &str,
    layout: &TableLayout,
    control: &mut dyn RunControl,
) -> Result<Vec<TableEntry>> {
    let invalid = |message: String| Error::new(ErrorCode::Integrity, message);
    slice(
        image,
        symbol,
        layout.entry,
        layout.handler,
        control,
        |entry, field, handler| {
            let class = field(entry, layout.class)?;
            // A level task has no source: its class's byte is no element of
            // the source table, whose elements are each a routed source.
            if class == LEVEL_TASK_BYTE {
                return Err(invalid(format!(
                    "the source table's entry at {entry:#010x} carries the level task's class"
                )));
            }
            Ok(TableEntry {
                kind: Kind::Routed {
                    source: field(entry, layout.source)?,
                    class: Class::of(class).ok_or_else(|| {
                        invalid(format!(
                            "the entry at {entry:#010x} has no handler class {class}"
                        ))
                    })?,
                },
                line: field(entry, layout.line)?,
                level: field(entry, layout.level)?,
                core: field(entry, layout.core)?,
                handler,
            })
        },
    )
}

/// The level tasks the table `symbol` holds in `image`.
pub fn level_task_table(
    image: &ImageFacts<'_>,
    symbol: &str,
    layout: &TaskLayout,
    control: &mut dyn RunControl,
) -> Result<Vec<TableEntry>> {
    slice(
        image,
        symbol,
        layout.entry,
        layout.handler,
        control,
        |entry, field, handler| {
            Ok(TableEntry {
                kind: Kind::Task,
                line: field(entry, layout.line)?,
                level: field(entry, layout.level)?,
                core: field(entry, layout.core)?,
                handler,
            })
        },
    )
}

/// The elements of the slice `symbol` holds in `image`, `entry_bytes`
/// apart, each with its handler word at `handler_offset`, made by `make`
/// from the element's address, a reader of its fields and its handler.
fn slice(
    image: &ImageFacts<'_>,
    symbol: &str,
    entry_bytes: u32,
    handler_offset: u32,
    control: &mut dyn RunControl,
    make: impl Fn(u32, &dyn Fn(u32, Field) -> Result<u32>, Option<u32>) -> Result<TableEntry>,
) -> Result<Vec<TableEntry>> {
    let invalid = |message: String| Error::new(ErrorCode::Integrity, message);
    let slice = image
        .file()
        .symbols()
        .find(|candidate| candidate.name == symbol)
        .ok_or_else(|| Error::new(ErrorCode::NotFound, format!("no `{symbol}` in the image")))?;
    let at = u32::try_from(slice.address).map_err(|_| invalid("table beyond RV32".into()))?;
    let word = |address: u32| {
        image
            .read_only_word(address)
            .ok_or_else(|| invalid(format!("{address:#010x} is not read-only data")))
    };
    let first = word(at)?;
    let length = word(at + 4)?;
    // An empty slice's pointer is the element type's dangling constant,
    // which no relocation names; it is never read.
    if length == 0 {
        return Ok(Vec::new());
    }
    if image.word(at) != Some(first) {
        return Err(invalid(format!(
            "`{symbol}`'s pointer at {at:#010x} has no relocation"
        )));
    }
    let field = |entry: u32, field: Field| -> Result<u32> {
        let address = entry + field.offset;
        if address % 4 + field.size > 4 {
            return Err(invalid(format!(
                "a field at {address:#010x} spans two words"
            )));
        }
        let value = word(address & !3)? >> (8 * (address % 4));
        Ok(match field.size {
            4 => value,
            size => value & ((1 << (8 * size)) - 1),
        })
    };
    // The entries grow as they are read: a corrupt length fails at the
    // first word outside the table, never on the budget or an allocation.
    let mut entries = Vec::new();
    for index in 0..length {
        // Each entry reads its handler and at most five fields.
        control.checkpoint(6)?;
        let entry = first + index * entry_bytes;
        let address = entry + handler_offset;
        let handler = word(address)?;
        if handler != 0 && image.word(address) != Some(handler) {
            return Err(invalid(format!(
                "the handler word at {address:#010x} has no relocation"
            )));
        }
        entries.push(make(entry, &field, (handler != 0).then_some(handler & !1))?);
    }
    Ok(entries)
}
