//! Function-pointer types carried by concrete DWARF locations, rather than
//! only absolute globals. An unfamiliar location or address use is unknown;
//! it never licenses excluding a function by its declaration's signature.
use super::{Field, TypeFacts};
use crate::dwarf::{Reader, dwarf_error, invalid, reference};
use crate::{Analysis, Fact, Resolutions, TransferKind};
use oer_analysis_elf::dwarf::{DebugSections, gimli};
use oer_analysis_elf::rv32::{self, Role};
use oer_analysis_riscv_lift::RiscvDecoder;
use oer_analysis_riscv_model::*;
use oer_analysis_riscv_program::facts::ImageFacts;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Place {
    /// The value (or a piece of it) is in a register.
    Register(u8),
    /// Bytes of the value are stored at register + offset.
    Memory(u8, i64),
    /// A pointer's value is register + offset.
    Pointer(u8, i64),
    Unknown,
}

#[derive(Clone, Copy, Debug)]
struct Location {
    start: u32,
    end: u32,
    owner: usize,
    offset: u64,
    bytes: Option<u64>,
    place: Place,
}

#[derive(Default)]
struct Locations {
    /// A concrete/abstract variable's own type and its origin.
    variables: BTreeMap<usize, (Option<usize>, Option<usize>)>,
    registers: BTreeMap<u8, Vec<Location>>,
    /// Equal register values at a memory operand, proved by moves since
    /// the last CFG merge. Only nontrivial equivalences are retained.
    aliases: BTreeMap<(u32, u8), u32>,
}

impl Locations {
    fn variable_type(&self, mut owner: usize) -> Option<usize> {
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
    fn expression(
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
        while let Some(operation) = operations.next().map_err(dwarf_error)? {
            control.checkpoint(1)?;
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

    /// Function-pointer fields at a machine memory operand. Registers named
    /// by DWARF describe their values at this PC, so no ABI/register-layout
    /// guess or scope extension is needed.
    fn fields(
        &self,
        types: &TypeFacts,
        pc: u32,
        base: u8,
        displacement: i32,
        control: &mut dyn RunControl,
    ) -> Result<BTreeSet<Field>> {
        let mut fields = BTreeSet::new();
        let aliases = self.aliases.get(&(pc, base)).copied().unwrap_or(1 << base);
        for register in 0..32_u8 {
            if aliases & (1 << register) == 0 {
                continue;
            }
            for location in self.registers.get(&register).into_iter().flatten() {
                control.checkpoint(1)?;
                if pc < location.start || pc >= location.end {
                    continue;
                }
                let Some(ty) = self.variable_type(location.owner) else {
                    continue;
                };
                let Some((_, record)) = types.resolve(ty) else {
                    continue;
                };
                match location.place {
                    Place::Register(_) | Place::Pointer(_, _) => {
                        // A register piece of an aggregate is not a pointer to
                        // that aggregate. Only the complete pointer is a base.
                        if location.offset != 0
                            || location.bytes.is_some_and(|bytes| bytes != 4)
                            || record.tag != gimli::DW_TAG_pointer_type
                        {
                            continue;
                        }
                        let shift = match location.place {
                            Place::Pointer(_, shift) => shift,
                            _ => 0,
                        };
                        if let (Some(target), Ok(offset)) = (
                            record.target,
                            u64::try_from(i64::from(displacement) - shift),
                        ) {
                            types.fields_at(target, offset, 0, &mut fields);
                        }
                    }
                    Place::Memory(_, shift) => {
                        let Ok(at) = u64::try_from(i64::from(displacement) - shift) else {
                            continue;
                        };
                        let Some(bytes) = location.bytes.or_else(|| types.size(ty)) else {
                            continue;
                        };
                        if at.checked_add(4).is_some_and(|end| end <= bytes) {
                            types.fields_at(ty, location.offset + at, 0, &mut fields);
                        }
                    }
                    Place::Unknown => {}
                }
            }
        }
        Ok(fields)
    }

    fn field(
        &self,
        types: &TypeFacts,
        pc: u32,
        base: u8,
        displacement: i32,
        control: &mut dyn RunControl,
    ) -> Result<Option<Field>> {
        let found = self.fields(types, pc, base, displacement, control)?;
        Ok(if found.len() == 1 {
            found.first().copied()
        } else {
            None
        })
    }

    /// An actual call target may itself be one register piece of a typed
    /// aggregate (for example a guard consumed by value). This uses the
    /// call's PC and does not borrow a neighbouring inline scope.
    fn value_field(
        &self,
        types: &TypeFacts,
        pc: u32,
        register: u8,
        control: &mut dyn RunControl,
    ) -> Result<Option<Field>> {
        let mut found = BTreeSet::new();
        for location in self.registers.get(&register).into_iter().flatten() {
            control.checkpoint(1)?;
            if pc < location.start
                || pc >= location.end
                || location.place != Place::Register(register)
            {
                continue;
            }
            let Some(ty) = self.variable_type(location.owner) else {
                continue;
            };
            if location.bytes.or_else(|| types.size(ty)) != Some(4) {
                continue;
            }
            types.fields_at(ty, location.offset, 0, &mut found);
        }
        Ok(if found.len() == 1 {
            found.first().copied()
        } else {
            None
        })
    }

    fn read(
        image: &ImageFacts<'_>,
        keep: &mut Ledger<'_>,
        control: &mut dyn RunControl,
    ) -> Result<Self> {
        let sections =
            DebugSections::load(image.file()).map_err(|error| invalid(error.to_string()))?;
        let dwarf = sections
            .dwarf()
            .map_err(|error| invalid(error.to_string()))?;
        let mut locations = Self::default();
        let mut units = dwarf.units();
        while let Some(header) = units.next().map_err(dwarf_error)? {
            let unit = dwarf.unit(header).map_err(dwarf_error)?;
            let mut scopes: Vec<(isize, Vec<(u32, u32)>)> = Vec::new();
            let mut scratch = Ledger::new(keep.memory(), control)?;
            let mut entries = unit.entries();
            while let Some(entry) = entries.next_dfs().map_err(dwarf_error)? {
                control.checkpoint(1)?;
                let depth = entry.depth();
                scopes.retain(|(at, _)| *at < depth);
                let Some(owner) = entry
                    .offset()
                    .to_debug_info_offset(&unit.header)
                    .map(|at| at.0)
                else {
                    continue;
                };
                if matches!(
                    entry.tag(),
                    gimli::DW_TAG_subprogram
                        | gimli::DW_TAG_inlined_subroutine
                        | gimli::DW_TAG_lexical_block
                ) {
                    let mut ranges = dwarf.die_ranges(&unit, entry).map_err(dwarf_error)?;
                    let mut own = Vec::new();
                    while let Some(range) = ranges.next().map_err(dwarf_error)? {
                        if let (Ok(start), Ok(end)) =
                            (u32::try_from(range.begin), u32::try_from(range.end))
                        {
                            scratch.push(&mut own, (start, end), control)?;
                        }
                    }
                    if !own.is_empty() {
                        scratch.push(&mut scopes, (depth, own), control)?;
                    }
                }
                if !matches!(
                    entry.tag(),
                    gimli::DW_TAG_variable | gimli::DW_TAG_formal_parameter
                ) {
                    continue;
                }
                let ty = entry
                    .attr_value(gimli::DW_AT_type)
                    .and_then(|value| reference(&unit, value));
                let origin = entry
                    .attr_value(gimli::DW_AT_abstract_origin)
                    .or_else(|| entry.attr_value(gimli::DW_AT_specification))
                    .and_then(|value| reference(&unit, value));
                if ty.is_some() || origin.is_some() {
                    keep.insert(&mut locations.variables, owner, (ty, origin), control)?;
                }
                let Some(value) = entry.attr_value(gimli::DW_AT_location) else {
                    continue;
                };
                match value {
                    gimli::AttributeValue::Exprloc(expression) => {
                        if entry.attr_value(gimli::DW_AT_start_scope).is_none()
                            && let Some((_, ranges)) = scopes.last()
                        {
                            locations.expression(
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
                        if let Some(mut list) =
                            dwarf.attr_locations(&unit, value).map_err(dwarf_error)?
                        {
                            while let Some(location) = list.next().map_err(dwarf_error)? {
                                if let (Ok(start), Ok(end)) = (
                                    u32::try_from(location.range.begin),
                                    u32::try_from(location.range.end),
                                ) {
                                    locations.expression(
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
            }
        }
        locations.read_aliases(image, keep, control)?;
        Ok(locations)
    }

    fn read_aliases(
        &mut self,
        image: &ImageFacts<'_>,
        keep: &mut Ledger<'_>,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        let identity = || std::array::from_fn::<_, 32, _>(|register| 1_u32 << register);
        for function in image.functions() {
            let code = crate::image::function_bytes(image.file(), function)?;
            let mut merges = BTreeSet::new();
            let mut scratch = Ledger::new(keep.memory(), control)?;
            let mut at = 0;
            let mut unsupported_jump = false;
            while let Some(bytes) = code.get(at..) {
                control.checkpoint(1)?;
                let Some(decoded) = RiscvDecoder.decode(bytes) else {
                    break;
                };
                match decoded.flow {
                    InstructionFlow::Branch { displacement }
                    | InstructionFlow::Jump {
                        displacement,
                        link: false,
                    } => {
                        scratch.add(
                            &mut merges,
                            (function.address + at as u32).wrapping_add(displacement as u32),
                            control,
                        )?;
                    }
                    InstructionFlow::Indirect { link: false, .. } => unsupported_jump = true,
                    _ => {}
                }
                at += usize::from(decoded.length);
            }
            // An unknown intra-function jump may give a later block an
            // unseen predecessor; no alias deduction is made in this case.
            if unsupported_jump {
                continue;
            }
            let mut aliases = identity();
            at = 0;
            while let Some(bytes) = code.get(at..) {
                control.checkpoint(1)?;
                let pc = function.address + at as u32;
                let Some(decoded) = RiscvDecoder.decode(bytes) else {
                    break;
                };
                if merges.contains(&pc) {
                    aliases = identity();
                }
                let op = RiscvDecoder.lift(bytes);
                if let SemanticOp::Memory { base, .. } = op
                    && let Some(group) = aliases.get(base as usize).copied()
                    && group.count_ones() > 1
                {
                    keep.insert(&mut self.aliases, (pc, base), group, control)?;
                }
                let copied = match op {
                    SemanticOp::Integer {
                        op: IntegerOp::Add,
                        dest,
                        left: Operand::Register(source),
                        right: Operand::Immediate(0),
                    } if dest != 0 && dest < 32 && source != 0 => aliases
                        .get(source as usize)
                        .copied()
                        .map(|group| (dest, group)),
                    SemanticOp::Integer {
                        op: IntegerOp::Add,
                        dest,
                        left: Operand::Register(0),
                        right: Operand::Register(source),
                    }
                    | SemanticOp::Integer {
                        op: IntegerOp::Add,
                        dest,
                        left: Operand::Register(source),
                        right: Operand::Register(0),
                    } if dest != 0 && dest < 32 && source != 0 => aliases
                        .get(source as usize)
                        .copied()
                        .map(|group| (dest, group)),
                    _ => None,
                };
                if let Some(dest) = writes(op).filter(|dest| *dest != 0 && *dest < 32) {
                    for group in &mut aliases {
                        *group &= !(1 << dest);
                    }
                    if let Some(group) = aliases.get_mut(dest as usize) {
                        *group = 1 << dest;
                    }
                }
                if let Some((dest, source)) = copied {
                    let group = source | (1 << dest);
                    for (register, members) in aliases.iter_mut().enumerate() {
                        if group & (1 << register) != 0 {
                            *members = group;
                        }
                    }
                }
                if decoded.flow != InstructionFlow::Next || !base_instruction(bytes) {
                    aliases = identity();
                }
                at += usize::from(decoded.length);
            }
        }
        Ok(())
    }
}

/// Every address use either preserves an identified function-pointer type
/// or is unknown. Unknown is deliberately per function, including aliases:
/// one erased escape prevents a declaration type from excluding it anywhere.
#[derive(Default)]
struct Uses {
    fields: BTreeSet<Field>,
    unknown: bool,
}

#[derive(Clone, Copy)]
struct AddressUse {
    site: u32,
    role: Role,
    exact: bool,
}

fn base_instruction(bytes: &[u8]) -> bool {
    matches!(
        oer_analysis_riscv_lift::decode_instruction(bytes),
        Some((oer_analysis_riscv_decode::Instruction::Base(_), _))
    )
}

fn writes(op: SemanticOp) -> Option<u8> {
    match op {
        SemanticOp::Integer { dest, .. }
        | SemanticOp::Upper { dest, .. }
        | SemanticOp::Link { dest }
        | SemanticOp::Opaque { dest } => Some(dest),
        SemanticOp::Memory { dest, .. } => dest,
        _ => None,
    }
}

fn reads(op: SemanticOp, register: u8) -> bool {
    match op {
        SemanticOp::Integer { left, right, .. } => {
            [left, right].contains(&Operand::Register(register))
        }
        SemanticOp::Memory { base, source, .. } => base == register || source == Some(register),
        SemanticOp::Unsupported => true,
        _ => false,
    }
}

/// A deliberately small proof for an absolute HI20/LO12 address. Every
/// subsequent read must be a word store into one known typed field, until
/// the register is killed. Branches, calls and escapes are unknown.
#[allow(clippy::too_many_arguments)]
fn code_use(
    image: &ImageFacts<'_>,
    analysis: &Analysis,
    types: &TypeFacts,
    locations: &Locations,
    function: u32,
    high: AddressUse,
    low: AddressUse,
    control: &mut dyn RunControl,
) -> Result<Option<BTreeSet<Field>>> {
    if high.role != Role::AbsoluteHigh || low.role != Role::AbsoluteLow || low.site != high.site + 4
    {
        return Ok(None);
    }
    let Some((_, owner)) = analysis.functions.range(..=high.site).next_back() else {
        return Ok(None);
    };
    if image
        .functions()
        .binary_search_by_key(&owner.function.address, |function| function.address)
        .is_err()
    {
        // The joint analysis also contains ROM companions. Their section
        // indexes and bytes belong to another ELF.
        return Ok(None);
    }
    let code = crate::image::function_bytes(image.file(), &owner.function)?;
    let at = high.site.wrapping_sub(owner.function.address) as usize;
    let Some(bytes) = code.get(at..) else {
        return Ok(None);
    };
    let Some(first) = RiscvDecoder.decode(bytes) else {
        return Ok(None);
    };
    let SemanticOp::Upper {
        dest: high_register,
        value,
        pc_relative: false,
    } = RiscvDecoder.lift(bytes)
    else {
        return Ok(None);
    };
    if first.length != 4 {
        return Ok(None);
    }
    let Some(bytes) = code.get(at + 4..) else {
        return Ok(None);
    };
    let Some(second) = RiscvDecoder.decode(bytes) else {
        return Ok(None);
    };
    let SemanticOp::Integer {
        op: IntegerOp::Add,
        dest: register,
        left: Operand::Register(base),
        right: Operand::Immediate(immediate),
    } = RiscvDecoder.lift(bytes)
    else {
        return Ok(None);
    };
    if base != high_register
        || register != high_register
        || value.wrapping_add(immediate) != function
    {
        return Ok(None);
    }
    let mut site = low.site + u32::from(second.length);
    let mut fields = BTreeSet::new();
    while let Some(bytes) = code.get(site.wrapping_sub(owner.function.address) as usize..) {
        control.checkpoint(1)?;
        let Some(decoded) = RiscvDecoder.decode(bytes) else {
            return Ok(None);
        };
        if !base_instruction(bytes) || decoded.flow != InstructionFlow::Next {
            return Ok(None);
        }
        let op = RiscvDecoder.lift(bytes);
        if reads(op, register) {
            match op {
                SemanticOp::Memory {
                    kind: MemoryKind::Store,
                    base,
                    displacement,
                    width: 4,
                    source: Some(source),
                    ..
                } if source == register && base != register => {
                    let mut field = locations.field(types, site, base, displacement, control)?;
                    if field.is_none() {
                        field = locations.field(
                            types,
                            site + u32::from(decoded.length),
                            base,
                            displacement,
                            control,
                        )?;
                    }
                    let Some(field) = field else { return Ok(None) };
                    fields.insert(field);
                }
                _ => return Ok(None),
            }
        }
        if writes(op) == Some(register) {
            return Ok(Some(fields));
        }
        site += u32::from(decoded.length);
    }
    Ok(None)
}

fn address_uses(
    image: &ImageFacts<'_>,
    analysis: &Analysis,
    types: &TypeFacts,
    locations: &Locations,
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<BTreeMap<u32, Uses>> {
    let mut symbols = BTreeMap::new();
    for symbol in image.file().symbols() {
        if let Ok(address) = u32::try_from(symbol.address)
            && analysis.functions.contains_key(&address)
        {
            keep.insert(&mut symbols, symbol.index, address, control)?;
        }
    }
    let mut uses: BTreeMap<u32, Vec<AddressUse>> = BTreeMap::new();
    for section in image.file().sections().filter(|section| section.allocated) {
        for relocation in image
            .file()
            .relocations(section.index)
            .map_err(|error| invalid(error.to_string()))?
        {
            control.checkpoint(1)?;
            let role = rv32::kind(relocation.r_type).role;
            if role == Role::Hint || rv32::transfers(relocation.r_type) {
                continue;
            }
            let target = image
                .file()
                .target_address(&relocation)
                .map_err(|error| invalid(error.to_string()))?;
            let Ok(target) = u32::try_from(target) else {
                continue;
            };
            let mut function = match relocation.target {
                oer_analysis_elf::Target::Symbol(symbol) => {
                    symbols.get(&symbol).copied().unwrap_or(target & !1)
                }
                _ => target & !1,
            };
            if !analysis.functions.contains_key(&function)
                && let Some((&entry, owner)) = analysis.functions.range(..=target).next_back()
                && u64::from(target) < u64::from(entry) + u64::from(owner.function.size)
            {
                // A section/local label or arithmetic addend may name the
                // middle of code. It still prevents excluding the function.
                function = entry;
            }
            if !analysis.functions.contains_key(&function) {
                continue;
            }
            let entries = keep.entry(&mut uses, function, Vec::new, control)?;
            keep.push(
                entries,
                AddressUse {
                    site: relocation.at as u32,
                    role,
                    exact: target == function,
                },
                control,
            )?;
        }
    }
    let mut out = BTreeMap::new();
    for (function, mut entries) in uses {
        entries.sort_by_key(|entry| entry.site);
        let mut proof = Uses::default();
        let mut at = 0;
        while let Some(entry) = entries.get(at).copied() {
            control.checkpoint(1)?;
            let fields = if entry.exact && entry.role == Role::Word {
                types.field(entry.site).map(|field| BTreeSet::from([field]))
            } else if entry.exact
                && let Some(low) = entries.get(at + 1).copied().filter(|low| low.exact)
            {
                if let Some(fields) = code_use(
                    image, analysis, types, locations, function, entry, low, control,
                )? {
                    at += 1;
                    Some(fields)
                } else {
                    None
                }
            } else {
                None
            };
            match fields {
                Some(fields) => {
                    for field in fields {
                        keep.add(&mut proof.fields, field, control)?;
                    }
                }
                None => proof.unknown = true,
            }
            at += 1;
        }
        keep.insert(&mut out, function, proof, control)?;
    }
    Ok(out)
}

fn compatible(types: &TypeFacts, a: Field, b: Field) -> bool {
    let (Some(a_signature), Some(b_signature)) = (
        types.signatures.get(&a.subroutine),
        types.signatures.get(&b.subroutine),
    ) else {
        return true;
    };
    let a_shape = types.shape(a_signature);
    let b_shape = types.shape(b_signature);
    super::shape_matches(&a_shape, &b_shape, !a.foreign && !b.foreign)
}

/// Resolve function fields held by PC-specific typed pointer/aggregate
/// locations. Candidates come from every address use's storage type, never
/// a declaration signature alone. One unsupported use keeps its function
/// in every site's candidate set. Empty sets do not prove an unreachable
/// call. The existing type/executor assumption still applies to memory
/// reconstructed through an erased pointer.
pub fn typed_function_pointer_resolutions(
    image: &ImageFacts<'_>,
    analysis: &Analysis,
    types: &TypeFacts,
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<Resolutions> {
    let mut scratch = Ledger::new(keep.memory(), control)?;
    let locations = Locations::read(image, &mut scratch, control)?;
    let uses = address_uses(image, analysis, types, &locations, &mut scratch, control)?;
    let mut resolutions = Resolutions::new();
    for facts in analysis.functions.values() {
        if image
            .functions()
            .binary_search_by_key(&facts.function.address, |function| function.address)
            .is_err()
        {
            continue;
        }
        for transfer in &facts.transfers {
            if transfer.target.is_some()
                || !matches!(transfer.kind, TransferKind::Call | TransferKind::Tail)
            {
                continue;
            }
            let code = crate::image::function_bytes(image.file(), &facts.function)?;
            let register = code
                .get((transfer.site - facts.function.address) as usize..)
                .and_then(|bytes| RiscvDecoder.decode(bytes))
                .and_then(|decoded| decoded.flow.through_register())
                .filter(|(_, displacement, _)| *displacement == 0)
                .map(|(register, _, _)| register);
            let field = match register {
                Some(register) => locations.value_field(types, transfer.site, register, control)?,
                None => None,
            };
            let mut fields = BTreeSet::new();
            if let Some(field) = field {
                scratch.add(&mut fields, field, control)?;
            } else if let Some(loads) = facts.target_loads.get(&transfer.site) {
                let mut complete = true;
                for load in loads {
                    let mut field = load.address.and_then(|address| types.field(address));
                    if field.is_none() {
                        field =
                            locations.field(types, load.site, load.base, load.offset, control)?;
                    }
                    if let Some(field) = field {
                        scratch.add(&mut fields, field, control)?;
                    } else {
                        complete = false;
                        break;
                    }
                }
                if !complete {
                    continue;
                }
            }
            if fields.is_empty() {
                continue;
            }
            let mut targets = BTreeSet::new();
            for (&function, proof) in &uses {
                control.checkpoint(1)?;
                if proof.unknown
                    || fields.iter().any(|&field| {
                        proof
                            .fields
                            .iter()
                            .any(|&stored| compatible(types, field, stored))
                    })
                {
                    scratch.add(&mut targets, function, control)?;
                }
            }
            keep.admit(Resolutions::admission(targets.len()), control)?;
            resolutions.add(transfer.site, Fact::FieldType, targets);
        }
    }
    Ok(resolutions)
}

#[cfg(test)]
mod tests;
