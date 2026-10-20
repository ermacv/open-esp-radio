//! Reaching definitions of indirect targets over the complete local graph.
//! A join keeps every producer, and an unknown incoming value loses the
//! proof. Register copies are followed; calls obey the integer psABI.
use crate::{
    Function, Load, Transfer,
    image::{self, Observation},
};
use oer_analysis_riscv_lift::RiscvDecoder;
use oer_analysis_riscv_model::*;
use oer_analysis_riscv_program::facts::ImageFacts;
use std::collections::{BTreeMap, BTreeSet};

/// An earlier register value equal to a callback load's address base.
/// Each witness dominates the load along the complete local graph. This is
/// value identity, never equality of sets of possible producers or values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegisterWitness {
    pub site: u32,
    pub register: u8,
}

/// Why backward transport of a load base stopped. Witnesses before this
/// boundary remain useful; no fact is asserted across the boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BaseBarrier {
    Entry,
    IncomingPaths,
    Definition,
    MemoryWrite,
    Call,
    Unsupported,
    Cycle,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadBase {
    pub witnesses: Vec<RegisterWitness>,
    pub barrier: (u32, BaseBarrier),
}

/// Keep the single-predecessor suffix of each proven target load's base.
/// CFG edges come from the common analyzer and instruction operations from
/// its shared semantic decoder. Joins and unclosed graphs remain barriers.
/// Memory contents are never forwarded, and a write or call stops transport
/// even when the register itself survives it.
pub(crate) fn bases(
    image: &ImageFacts<'_>,
    function: &Function,
    loads: &BTreeMap<u32, Vec<Load>>,
    observation: &Observation,
    work: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<BTreeMap<u32, LoadBase>> {
    let mut out = BTreeMap::new();
    if !observation.closed_graph {
        return Ok(out);
    }
    let code = image::function_bytes(image.file(), function)?;
    for load in loads.values().flatten() {
        control.checkpoint(1)?;
        if out.contains_key(&load.site) {
            continue;
        }
        let mut witnesses = Vec::new();
        let mut local = Ledger::new(work.memory(), control)?;
        let mut seen = BTreeSet::new();
        let (mut pc, mut register) = (load.site, load.base);
        let barrier = loop {
            control.checkpoint(1)?;
            if pc == function.address {
                break (pc, BaseBarrier::Entry);
            }
            if !local.add(&mut seen, (pc, register), control)? {
                break (pc, BaseBarrier::Cycle);
            }
            let Some(incoming) = observation.predecessors.get(&pc) else {
                break (pc, BaseBarrier::IncomingPaths);
            };
            let [from] = incoming.as_slice() else {
                break (pc, BaseBarrier::IncomingPaths);
            };
            let Some(bytes) = code.get(from.wrapping_sub(function.address) as usize..) else {
                break (*from, BaseBarrier::Unsupported);
            };
            let Some(decoded) = RiscvDecoder.decode(bytes) else {
                break (*from, BaseBarrier::Unsupported);
            };
            if matches!(
                decoded.flow,
                InstructionFlow::Jump { link: true, .. }
                    | InstructionFlow::Indirect { link: true, .. }
            ) {
                break (*from, BaseBarrier::Call);
            }
            let op = RiscvDecoder.lift(bytes);
            if op == SemanticOp::Unsupported {
                break (*from, BaseBarrier::Unsupported);
            }
            if let SemanticOp::Memory { kind, .. } = op
                && !matches!(
                    kind,
                    MemoryKind::Load | MemoryKind::LoadReserved | MemoryKind::FloatLoad
                )
            {
                break (*from, BaseBarrier::MemoryWrite);
            }
            if op.destination_register() == Some(register) {
                // A move supplies value identity. Other definitions, loads
                // included, need a separate provenance mechanism.
                match op {
                    SemanticOp::Integer {
                        op: IntegerOp::Add,
                        left: Operand::Register(source),
                        right: Operand::Immediate(0),
                        ..
                    }
                    | SemanticOp::Integer {
                        op: IntegerOp::Add,
                        left: Operand::Register(source),
                        right: Operand::Register(0),
                        ..
                    }
                    | SemanticOp::Integer {
                        op: IntegerOp::Add,
                        left: Operand::Register(0),
                        right: Operand::Register(source),
                        ..
                    } if source != 0 => register = source,
                    _ => break (*from, BaseBarrier::Definition),
                }
            }
            pc = *from;
            work.push(
                &mut witnesses,
                RegisterWitness { site: pc, register },
                control,
            )?;
        };
        work.insert(
            &mut out,
            load.site,
            LoadBase { witnesses, barrier },
            control,
        )?;
    }
    Ok(out)
}

pub(crate) fn loads(
    image: &ImageFacts<'_>,
    function: &Function,
    transfers: &[Transfer],
    observation: &Observation,
    work: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<BTreeMap<u32, Vec<Load>>> {
    let mut out = BTreeMap::new();
    if !observation.closed_graph {
        return Ok(out);
    }
    let code = image::function_bytes(image.file(), function)?;
    let bytes = |pc: u32| code.get(pc.wrapping_sub(function.address) as usize..);
    for transfer in transfers
        .iter()
        .filter(|transfer| transfer.target.is_none())
    {
        control.checkpoint(1)?;
        let Some(call_bytes) = bytes(transfer.site) else {
            continue;
        };
        let Some(decoded) = RiscvDecoder.decode(call_bytes) else {
            continue;
        };
        let Some((register, 0, _)) = decoded.flow.through_register() else {
            continue;
        };
        let mut local = Ledger::new(work.memory(), control)?;
        let mut pending = Vec::new();
        let mut seen = BTreeSet::new();
        let mut producers = BTreeMap::new();
        local.push(&mut pending, (transfer.site, register), control)?;
        let mut known = true;
        while let Some((pc, register)) = pending.pop() {
            control.checkpoint(1)?;
            if !local.add(&mut seen, (pc, register), control)? {
                continue;
            }
            if pc == function.address {
                known = false;
                break;
            }
            let Some(incoming) = observation
                .predecessors
                .get(&pc)
                .filter(|incoming| !incoming.is_empty())
            else {
                known = false;
                break;
            };
            for &from in incoming {
                control.checkpoint(1)?;
                let Some(bytes) = bytes(from) else {
                    known = false;
                    break;
                };
                let Some(decoded) = RiscvDecoder.decode(bytes) else {
                    known = false;
                    break;
                };
                let op = RiscvDecoder.lift(bytes);
                if matches!(
                    decoded.flow,
                    InstructionFlow::Jump { link: true, .. }
                        | InstructionFlow::Indirect { link: true, .. }
                ) && CallAbi::RiscvInteger.clobbered().contains(&register)
                {
                    known = false;
                    break;
                }
                // No described destination is not proof of preservation:
                // unsupported operations can have unknown writes.
                if op == SemanticOp::Unsupported {
                    known = false;
                    break;
                }
                if op.destination_register() == Some(register) {
                    match op {
                        SemanticOp::Memory {
                            kind: MemoryKind::Load,
                            width: 4,
                            base,
                            displacement,
                            ..
                        } => {
                            local.insert(
                                &mut producers,
                                from,
                                Load {
                                    site: from,
                                    exclusive_producer: incoming.as_slice() == [from],
                                    base,
                                    offset: displacement,
                                    address: None,
                                },
                                control,
                            )?;
                        }
                        SemanticOp::Integer {
                            op: IntegerOp::Add,
                            left: Operand::Register(source),
                            right: Operand::Immediate(0),
                            ..
                        } => {
                            local.push(&mut pending, (from, source), control)?;
                        }
                        _ => {
                            known = false;
                            break;
                        }
                    }
                } else {
                    local.push(&mut pending, (from, register), control)?;
                }
            }
            if !known {
                break;
            }
        }
        if known && !producers.is_empty() {
            let mut result = Vec::new();
            for producer in producers.into_values() {
                work.push(&mut result, producer, control)?;
            }
            work.insert(&mut out, transfer.site, result, control)?;
        }
    }
    Ok(out)
}
