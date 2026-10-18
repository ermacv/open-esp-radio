//! The entries of jump tables, from the facts of one build. The analysis
//! locates a dispatch and its table's base; a build's facts (an object's
//! relocations, an image's relocated words and labels) say what the table
//! holds, as the image's memory says what its constants hold.
use crate::*;

/// A table's entries, admitted in the working memory of the operation that
/// asked for them.
pub struct TableWords<'m> {
    /// The entries in index order: code offsets of the dispatching function's
    /// section in an object, code addresses in an image.
    pub entries: Vec<u64>,
    pub reserved: MemoryReservation<'m>,
}

/// A build whose facts hold no table: every dispatch stays a gap.
pub struct NoTables;

impl TableEntries for NoTables {
    fn entries<'m>(
        &self,
        _: &AbstractValue,
        _: Option<u32>,
        _: u32,
        _: &'m WorkingMemory,
        _: &mut dyn RunControl,
    ) -> Result<Option<TableWords<'m>>> {
        Ok(None)
    }
}

/// Where the entries of a function's jump tables come from.
pub trait TableEntries {
    /// The entries of the table whose base is `base` (a value the analysis
    /// holds: a symbol of an object or an image address) for a function of
    /// `section`: `count` entries when a bounds check states the count,
    /// otherwise those the build's facts delimit. Reading stops at the first
    /// entry the facts do not name as code of `section`, so fewer entries
    /// than `count` prove no table. `None` when the facts hold no table
    /// there. Each entry is charged to `control` before it is read, and
    /// the entries are admitted in `memory` before they are allocated.
    fn entries<'m>(
        &self,
        base: &AbstractValue,
        count: Option<u32>,
        section: u32,
        memory: &'m WorkingMemory,
        control: &mut dyn RunControl,
    ) -> Result<Option<TableWords<'m>>>;
}
