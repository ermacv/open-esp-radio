//! Optional PC-specific variable/parameter locations, read by
//! [`crate::read_locations`] for the [`LocationQueries`] the instruction pass
//! built. Attribute inheritance and piece acceptance retain the typed
//! consumer's rules; machine-register aliases are prepared separately from
//! instructions.
use super::{Entry, Reader, Unit, dwarf_error, next_operation, origin_reference, reference};
use oer_tools_analysis_elf::dwarf::gimli;
use oer_tools_analysis_riscv_model::*;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Place {
    /// The value (or a piece of it) is in a register.
    Register(u8),
    /// Bytes of the value are stored at register + offset.
    Memory(u8, i64),
    /// A pointer's value is register + offset.
    Pointer(u8, i64),
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Location {
    pub(crate) start: u32,
    pub(crate) end: u32,
    pub(crate) owner: usize,
    pub(crate) offset: u64,
    pub(crate) bytes: Option<u64>,
    pub(crate) place: Place,
}

/// Where the typed-location consumers can ask, built from the instruction
/// pass before the locations are read ([`crate::location_queries`]). A
/// [`Location`] is kept only when [`Self::keeps`] it; every other one is
/// skipped by each consumer on its own conditions, so the answers are those
/// of the complete collection.
#[derive(Debug, Default)]
pub struct LocationQueries {
    /// PCs at which a register of any place may be asked (`fields`,
    /// `value_field`): sorted and unique once finished.
    points: Vec<u32>,
    /// Half-open PC ranges in which a complete register word may be asked
    /// with its continuation (`waker_register`): sorted, disjoint and
    /// non-adjacent once finished.
    windows: Vec<(u32, u64)>,
}

impl LocationQueries {
    pub(crate) fn point(
        &mut self,
        pc: u32,
        keep: &mut Ledger<'_>,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        keep.push(&mut self.points, pc, control)
    }

    /// An empty range asks nothing and is not retained.
    pub(crate) fn window(
        &mut self,
        start: u32,
        end: u64,
        keep: &mut Ledger<'_>,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        if u64::from(start) >= end {
            return Ok(());
        }
        keep.push(&mut self.windows, (start, end), control)
    }

    /// Sort the points and merge the windows in place; a union intersects a
    /// range exactly when one of its members does.
    pub(crate) fn finish(&mut self, control: &mut dyn RunControl) -> Result<()> {
        let charge = |count: usize| {
            (count as u64).saturating_mul(u64::from(usize::BITS - count.leading_zeros()))
        };
        control.checkpoint(charge(self.points.len()))?;
        self.points.sort_unstable();
        self.points.dedup();
        control.checkpoint(charge(self.windows.len()))?;
        self.windows.sort_unstable();
        // Sorted by start, a window overlapping or adjoining the last kept
        // one extends it.
        self.windows.dedup_by(|next, kept| {
            let joins = u64::from(next.0) <= kept.1;
            if joins {
                kept.1 = kept.1.max(next.1);
            }
            joins
        });
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn points(&self) -> &[u32] {
        &self.points
    }

    /// Queries of exactly `points` and `windows`, finished.
    #[cfg(test)]
    pub(crate) fn asking(points: &[u32], windows: &[(u32, u64)]) -> Self {
        let mut keep = crate::tests::ledger();
        let mut queries = Self::default();
        for &pc in points {
            queries.point(pc, &mut keep, &mut || Ok(())).unwrap();
        }
        for &(start, end) in windows {
            queries
                .window(start, end, &mut keep, &mut || Ok(()))
                .unwrap();
        }
        queries.finish(&mut || Ok(())).unwrap();
        queries
    }

    /// `location` is kept when its range `[start, end)` contains a point, or,
    /// for a register place whose piece is a complete word or the whole value,
    /// when it intersects a window. A point consumer reads only locations
    /// containing its PC. The window consumer reads only such register places
    /// (a piece of another size never has its four bytes), and only those
    /// intersecting its range `[pc, end)`: its first location contains `pc`,
    /// and each joined continuation starts at or before a covered PC below
    /// `end` and ends after a covered PC above `pc`.
    pub(crate) fn keeps(&self, location: &Location) -> bool {
        let at = self.points.partition_point(|&pc| pc < location.start);
        if self.points.get(at).is_some_and(|&pc| pc < location.end) {
            return true;
        }
        if !matches!(location.place, Place::Register(_))
            || location.bytes.is_some_and(|bytes| bytes != 4)
        {
            return false;
        }
        let at = self
            .windows
            .partition_point(|&(_, end)| end <= u64::from(location.start));
        self.windows
            .get(at)
            .is_some_and(|&(start, _)| start < location.end)
    }
}

#[derive(Debug, Default)]
pub(crate) struct LocalLocations {
    /// A concrete/abstract variable's own type and its origin.
    pub(crate) variables: BTreeMap<usize, (Option<usize>, Option<usize>)>,
    pub(crate) registers: BTreeMap<u8, Vec<Location>>,
}

impl LocalLocations {
    pub(crate) fn variable_type(&self, mut owner: usize) -> Option<usize> {
        for _ in 0..16 {
            let &(ty, origin) = self.variables.get(&owner)?;
            if ty.is_some() {
                return ty;
            }
            owner = origin?;
        }
        None
    }

    fn add(
        &mut self,
        location: Location,
        queries: &LocationQueries,
        keep: &mut Ledger<'_>,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        if !queries.keeps(&location) {
            return Ok(());
        }
        let register = match location.place {
            Place::Register(register)
            | Place::Memory(register, _)
            | Place::Pointer(register, _) => register,
            Place::Unknown => return Ok(()),
        };
        let locations = keep.entry(&mut self.registers, register, Vec::new, control)?;
        keep.push(locations, location, control)
    }

    /// Each supported expression is a sequence of independent, byte-aligned
    /// pieces. Empty/unsupported pieces still advance the object offset.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn expression(
        &mut self,
        expression: gimli::Expression<Reader<'_>>,
        encoding: gimli::Encoding,
        owner: usize,
        ranges: &[(u32, u32)],
        queries: &LocationQueries,
        keep: &mut Ledger<'_>,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        let mut operations = expression.operations(encoding);
        let mut place = Place::Unknown;
        let mut offset = 0_u64;
        let mut started = false;
        let mut valid = true;
        while let Some(operation) = next_operation(&mut operations, control)? {
            match operation {
                gimli::Operation::Register { register } if !started => {
                    place = u8::try_from(register.0)
                        .ok()
                        .filter(|r| *r < 32)
                        .map(Place::Register)
                        .unwrap_or(Place::Unknown);
                    started = true;
                }
                gimli::Operation::RegisterOffset {
                    register, offset, ..
                } if !started => {
                    place = u8::try_from(register.0)
                        .ok()
                        .filter(|r| *r < 32)
                        .map(|r| Place::Memory(r, offset))
                        .unwrap_or(Place::Unknown);
                    started = true;
                }
                gimli::Operation::StackValue => {
                    place = match place {
                        Place::Memory(register, offset) => Place::Pointer(register, offset),
                        _ => Place::Unknown,
                    };
                }
                gimli::Operation::Piece {
                    size_in_bits,
                    bit_offset,
                } => {
                    if size_in_bits % 8 != 0 || bit_offset.is_some_and(|bit| bit != 0) {
                        // Unsupported bit pieces also make all subsequent
                        // byte offsets ambiguous.
                        return Ok(());
                    }
                    let bytes = size_in_bits / 8;
                    if valid && size_in_bits % 8 == 0 && bit_offset.is_none_or(|bit| bit == 0) {
                        for &(start, end) in ranges {
                            control.checkpoint(1)?;
                            self.add(
                                Location {
                                    start,
                                    end,
                                    owner,
                                    offset,
                                    bytes: Some(bytes),
                                    place,
                                },
                                queries,
                                keep,
                                control,
                            )?;
                        }
                    }
                    offset = offset.saturating_add(bytes);
                    place = Place::Unknown;
                    started = false;
                    valid = true;
                }
                gimli::Operation::Nop => {}
                _ => valid = false,
            }
        }
        if valid && started {
            for &(start, end) in ranges {
                control.checkpoint(1)?;
                self.add(
                    Location {
                        start,
                        end,
                        owner,
                        offset,
                        bytes: None,
                        place,
                    },
                    queries,
                    keep,
                    control,
                )?;
            }
        }
        Ok(())
    }
}

/// Per-unit lexical state; its reservations are released at the unit boundary.
pub(super) struct LocationWalk<'f, 'q, 'm> {
    facts: &'f mut LocalLocations,
    queries: &'q LocationQueries,
    scopes: Vec<(isize, Vec<(u32, u32)>)>,
    scratch: Ledger<'m>,
}

impl<'f, 'q, 'm> LocationWalk<'f, 'q, 'm> {
    pub(super) fn new(
        facts: &'f mut LocalLocations,
        queries: &'q LocationQueries,
        memory: &'m WorkingMemory,
        control: &mut dyn RunControl,
    ) -> Result<Self> {
        Ok(Self {
            facts,
            queries,
            scopes: Vec::new(),
            scratch: Ledger::new(memory, control)?,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn entry<'d>(
        &mut self,
        dwarf: &gimli::Dwarf<Reader<'d>>,
        unit: &Unit<'d>,
        entry: &Entry<'d>,
        owner: usize,
        keep: &mut Ledger<'_>,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        let depth = entry.depth();
        self.scopes.retain(|(at, _)| *at < depth);
        if matches!(
            entry.tag(),
            gimli::DW_TAG_subprogram
                | gimli::DW_TAG_inlined_subroutine
                | gimli::DW_TAG_lexical_block
        ) {
            let mut ranges = dwarf.die_ranges(unit, entry).map_err(dwarf_error)?;
            let mut own = Vec::new();
            while let Some(range) = ranges.next().map_err(dwarf_error)? {
                control.checkpoint(1)?;
                if let (Ok(start), Ok(end)) = (u32::try_from(range.begin), u32::try_from(range.end))
                {
                    self.scratch.push(&mut own, (start, end), control)?;
                }
            }
            if !own.is_empty() {
                self.scratch.push(&mut self.scopes, (depth, own), control)?;
            }
        }
        if !matches!(
            entry.tag(),
            gimli::DW_TAG_variable | gimli::DW_TAG_formal_parameter
        ) {
            return Ok(());
        }
        let ty = entry
            .attr_value(gimli::DW_AT_type)
            .and_then(|value| reference(unit, value));
        let origin = origin_reference(unit, entry);
        if ty.is_some() || origin.is_some() {
            keep.insert(&mut self.facts.variables, owner, (ty, origin), control)?;
        }
        let Some(value) = entry.attr_value(gimli::DW_AT_location) else {
            return Ok(());
        };
        match value {
            gimli::AttributeValue::Exprloc(expression) => {
                if entry.attr_value(gimli::DW_AT_start_scope).is_none()
                    && let Some((_, ranges)) = self.scopes.last()
                {
                    self.facts.expression(
                        expression,
                        unit.encoding(),
                        owner,
                        ranges,
                        self.queries,
                        keep,
                        control,
                    )?;
                }
            }
            value => {
                if let Some(mut list) = dwarf.attr_locations(unit, value).map_err(dwarf_error)? {
                    while let Some(location) = list.next().map_err(dwarf_error)? {
                        control.checkpoint(1)?;
                        if let (Ok(start), Ok(end)) = (
                            u32::try_from(location.range.begin),
                            u32::try_from(location.range.end),
                        ) {
                            self.facts.expression(
                                location.data,
                                unit.encoding(),
                                owner,
                                &[(start, end)],
                                self.queries,
                                keep,
                                control,
                            )?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
