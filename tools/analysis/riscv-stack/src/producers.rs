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
                let dest = match op {
                    SemanticOp::Integer { dest, .. }
                    | SemanticOp::Upper { dest, .. }
                    | SemanticOp::Link { dest }
                    | SemanticOp::Opaque { dest } => Some(dest),
                    SemanticOp::Memory { dest, .. } => dest,
                    SemanticOp::Unsupported => {
                        known = false;
                        break;
                    }
                    _ => None,
                };
                if dest == Some(register) {
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
