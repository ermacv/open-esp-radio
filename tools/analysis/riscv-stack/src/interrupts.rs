//! An image's interrupt table: for each peripheral source its handler, level,
//! handler class and core, read from the link's facts. The table is the slice an exported
//! symbol holds (its pointer word relocated to the entries, then its length);
//! every non-zero handler word carries the relocation that names it, and a
//! zero word is an entry the build left out.
use oer_analysis_riscv_model::{Error, ErrorCode, Result, RunControl};
use oer_analysis_riscv_program::facts::ImageFacts;

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
    pub level: Field,
    pub class: Field,
    pub core: Field,
    /// Offset of the handler's address word.
    pub handler: u32,
}

/// What an entry's handler may do, as the image declares it
/// (`oer_interrupt_table::Class`, whose discriminants these are).
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Class {
    /// Strict code throughout: no task poll, no waker call, never on the
    /// level of its core's executor.
    Hardware,
    /// An executor's interrupt: strict up to its task polls.
    Executor,
    /// A hardware handler on the level of its core's executor.
    Exempt,
}

impl Class {
    fn of(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Hardware),
            1 => Some(Self::Executor),
            2 => Some(Self::Exempt),
            _ => None,
        }
    }
}

impl std::fmt::Display for Class {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(match self {
            Self::Hardware => "hardware",
            Self::Executor => "executor",
            Self::Exempt => "exempt",
        })
    }
}

/// One entry of the table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TableEntry {
    pub source: u32,
    pub level: u32,
    pub class: Class,
    pub core: u32,
    /// The handler's address; `None` for an entry the build left out.
    pub handler: Option<u32>,
}

/// The entries of the table `symbol` holds in `image`.
pub fn interrupt_table(
    image: &ImageFacts<'_>,
    symbol: &str,
    layout: &TableLayout,
    control: &mut dyn RunControl,
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
        // Each entry reads its handler and four fields.
        control.checkpoint(5)?;
        let entry = first + index * layout.entry;
        let address = entry + layout.handler;
        let handler = word(address)?;
        if handler != 0 && image.word(address) != Some(handler) {
            return Err(invalid(format!(
                "the handler word at {address:#010x} has no relocation"
            )));
        }
        let class = field(entry, layout.class)?;
        entries.push(TableEntry {
            source: field(entry, layout.source)?,
            level: field(entry, layout.level)?,
            class: Class::of(class).ok_or_else(|| {
                invalid(format!(
                    "the entry at {entry:#010x} has no handler class {class}"
                ))
            })?,
            core: field(entry, layout.core)?,
            handler: (handler != 0).then_some(handler & !1),
        });
    }
    Ok(entries)
}
