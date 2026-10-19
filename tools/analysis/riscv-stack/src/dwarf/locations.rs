//! Optional PC-specific variable/parameter locations, collected by read_debug.
//! Attribute inheritance and piece acceptance retain the typed consumer's
//! rules; machine-register aliases are prepared separately from instructions.
use super::{Entry, Reader, Unit, dwarf_error, next_operation, origin_reference, reference};
use oer_analysis_elf::dwarf::gimli;
use oer_analysis_riscv_model::*;
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
        keep: &mut Ledger<'_>,
        control: &mut dyn RunControl,
    ) -> Result<()> {
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
                    keep,
                    control,
                )?;
            }
        }
        Ok(())
    }
}

/// Per-unit lexical state; its reservations are released at the unit boundary.
pub(super) struct LocationWalk<'f, 'm> {
    facts: &'f mut LocalLocations,
    scopes: Vec<(isize, Vec<(u32, u32)>)>,
    scratch: Ledger<'m>,
}

impl<'f, 'm> LocationWalk<'f, 'm> {
    pub(super) fn new(
        facts: &'f mut LocalLocations,
        memory: &'m WorkingMemory,
        control: &mut dyn RunControl,
    ) -> Result<Self> {
        Ok(Self {
            facts,
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
