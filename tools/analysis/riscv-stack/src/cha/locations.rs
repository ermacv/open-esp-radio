//! PC-specific storage fields and register aliases, prepared once per audit.
use super::{Field, TypeFacts};
use crate::dwarf::locations::Place;
use oer_analysis_elf::dwarf::gimli;
use oer_analysis_riscv_lift::{RiscvDecoder, is_base_instruction};
use oer_analysis_riscv_model::*;
use oer_analysis_riscv_program::facts::ImageFacts;
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct Locations {
    /// Equal register values at a memory operand, proved by moves since
    /// the last CFG merge. Only nontrivial equivalences are retained.
    pub(super) aliases: BTreeMap<(u32, u8), u32>,
}

impl Locations {
    /// Function-pointer fields at a machine memory operand. Registers named
    /// by DWARF describe their values at this PC, so no ABI/register-layout
    /// guess or scope extension is needed.
    pub(super) fn fields(
        &self,
        types: &TypeFacts,
        pc: u32,
        base: u8,
        displacement: i32,
        control: &mut dyn RunControl,
    ) -> Result<BTreeSet<Field>> {
        let mut fields = BTreeSet::new();
        let Some(debug) = types.locations.as_ref() else {
            return Ok(fields);
        };
        let aliases = self.aliases.get(&(pc, base)).copied().unwrap_or(1 << base);
        for register in 0..32_u8 {
            if aliases & (1 << register) == 0 {
                continue;
            }
            for location in debug.registers.get(&register).into_iter().flatten() {
                control.checkpoint(1)?;
                if pc < location.start || pc >= location.end {
                    continue;
                }
                let Some(ty) = debug.variable_type(location.owner) else {
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

    pub(super) fn field(
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
    pub(super) fn value_field(
        &self,
        types: &TypeFacts,
        pc: u32,
        register: u8,
        control: &mut dyn RunControl,
    ) -> Result<Option<Field>> {
        let mut found = BTreeSet::new();
        let Some(debug) = types.locations.as_ref() else {
            return Ok(None);
        };
        for location in debug.registers.get(&register).into_iter().flatten() {
            control.checkpoint(1)?;
            if pc < location.start
                || pc >= location.end
                || location.place != Place::Register(register)
            {
                continue;
            }
            let Some(ty) = debug.variable_type(location.owner) else {
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

    pub(super) fn read_aliases(
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
