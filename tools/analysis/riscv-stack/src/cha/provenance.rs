//! Function-pointer types carried by concrete DWARF locations, rather than
//! only absolute globals. An unfamiliar location or address use is unknown;
//! it never licenses excluding a function by its declaration's signature.
use super::{Field, TypeFacts};
use crate::dwarf::locations::Place;
use crate::{Analysis, Fact, Resolutions, TransferKind};
use oer_analysis_elf::dwarf::gimli;
use oer_analysis_elf::rv32::Role;
use oer_analysis_riscv_lift::{RiscvDecoder, is_base_instruction};
use oer_analysis_riscv_model::*;
use oer_analysis_riscv_program::facts::ImageFacts;
use std::collections::{BTreeMap, BTreeSet};

struct Locations<'a> {
    debug: &'a crate::dwarf::locations::LocalLocations,
    /// Equal register values at a memory operand, proved by moves since
    /// the last CFG merge. Only nontrivial equivalences are retained.
    aliases: BTreeMap<(u32, u8), u32>,
}

impl Locations<'_> {
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
            for location in self.debug.registers.get(&register).into_iter().flatten() {
                control.checkpoint(1)?;
                if pc < location.start || pc >= location.end {
                    continue;
                }
                let Some(ty) = self.debug.variable_type(location.owner) else {
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
        for location in self.debug.registers.get(&register).into_iter().flatten() {
            control.checkpoint(1)?;
            if pc < location.start
                || pc >= location.end
                || location.place != Place::Register(register)
            {
                continue;
            }
            let Some(ty) = self.debug.variable_type(location.owner) else {
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
                if let Some(dest) = op
                    .destination_register()
                    .filter(|dest| *dest != 0 && *dest < 32)
                {
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
                if decoded.flow != InstructionFlow::Next || !is_base_instruction(bytes) {
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
        if !is_base_instruction(bytes) || decoded.flow != InstructionFlow::Next {
            return Ok(None);
        }
        let op = RiscvDecoder.lift(bytes);
        if op.reads_register(register) {
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
        if op.destination_register() == Some(register) {
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
    let mut uses: BTreeMap<u32, Vec<AddressUse>> = BTreeMap::new();
    for address_use in image.address_uses() {
        control.checkpoint(1)?;
        let Some(projected) = crate::relocations::function_use(
            address_use,
            analysis,
            crate::relocations::FunctionUsePolicy::TypedStorage,
        )?
        else {
            continue;
        };
        let Some(function) = projected.function else {
            continue;
        };
        let entries = keep.entry(&mut uses, function, Vec::new, control)?;
        keep.push(
            entries,
            AddressUse {
                site: address_use.relocation.at as u32,
                role: address_use.role,
                exact: projected.exact,
            },
            control,
        )?;
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
/// `types` must come from `read_debug` with `DebugMetadata::TypedLocations`
/// for this image. This resolver reads prepared metadata and instructions;
/// it does not load or traverse DWARF.
pub fn typed_function_pointer_resolutions(
    image: &ImageFacts<'_>,
    analysis: &Analysis,
    types: &TypeFacts,
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<Resolutions> {
    let mut scratch = Ledger::new(keep.memory(), control)?;
    let debug = types.locations.as_ref().ok_or_else(|| {
        Error::new(
            ErrorCode::Incompatible,
            "typed locations were not requested by read_debug",
        )
    })?;
    let mut locations = Locations {
        debug,
        aliases: BTreeMap::new(),
    };
    locations.read_aliases(image, &mut scratch, control)?;
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
