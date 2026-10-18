//! Linear final-image scan with bounded local constant propagation, not a CFG proof.
use blobray_domain::*;
use oer_riscv_model::*;

pub fn audit_section(
    view: ExecutableSectionView<'_>,
    decoder: &dyn FunctionSemantics,
    ranges: &[ForbiddenTargetRange],
    control: &mut dyn RunControl,
    emit: &mut impl FnMut(&TargetAuditRecord, &mut dyn RunControl) -> Result<()>,
) -> Result<TargetAuditSummary> {
    let ExecutableSectionView {
        section,
        address,
        bytes,
        data_ranges,
    } = view;
    let mut summary = TargetAuditSummary {
        sections: 1,
        executable_bytes: bytes.len() as u64,
        ..Default::default()
    };
    let mut values = [None; 32];
    values[0] = Some(0);
    let mut offset = 0;
    let mut data_index = 0;
    while offset < bytes.len() {
        let pc = address.wrapping_add(offset as u32);
        let mut pos = control.position();
        pos.entry = Some(u64::from(pc));
        control.set_position(pos);
        control.checkpoint(1)?;
        while data_index < data_ranges.len()
            && data_ranges[data_index].start + data_ranges[data_index].length <= u64::from(pc)
        {
            control.checkpoint(1)?;
            data_index += 1;
        }
        let next_data = data_ranges.get(data_index);
        if let Some(data) = next_data.filter(|d| d.start <= u64::from(pc)) {
            let end = usize::try_from(data.start + data.length - u64::from(address))
                .ok()
                .filter(|end| *end <= bytes.len())
                .ok_or_else(|| Error::new(ErrorCode::Integrity, "data mapping outside section"))?;
            summary.embedded_data_bytes += (end - offset) as u64;
            values.fill(None);
            values[0] = Some(0);
            offset = end;
            continue;
        }
        let code_end = next_data.map_or(bytes.len(), |d| (d.start - u64::from(address)) as usize);
        let remaining = bytes
            .get(offset..code_end)
            .ok_or_else(|| Error::new(ErrorCode::Integrity, "invalid data mapping order"))?;
        if remaining.len() < 2 {
            return Err(Error::new(ErrorCode::Integrity, "truncated instruction"));
        }
        let width = if remaining[0] & 3 != 3 {
            2
        } else if remaining[0] & 0x1f != 0x1f {
            4
        } else {
            summary.coverage_gaps += 1;
            emit(
                &TargetAuditRecord::Gap {
                    section,
                    site: pc,
                    encoding: remaining[..remaining.len().min(4)].to_vec(),
                    reason: TargetAuditGapReason::InstructionLength,
                },
                control,
            )?;
            break;
        };
        if remaining.len() < width {
            return Err(Error::new(ErrorCode::Integrity, "truncated instruction"));
        }
        let instruction = &remaining[..width];
        let Some(decoded) = decoder.decode(instruction) else {
            // The backend owns opcode classification. An unsupported operation
            // never preserves stale constant-register facts.
            match decoder.unsupported_flow(instruction) {
                UnsupportedFlow::NonControl => summary.unsupported_non_control += 1,
                UnsupportedFlow::Indirect => summary.unresolved_indirect += 1,
                UnsupportedFlow::Unknown => {
                    summary.coverage_gaps += 1;
                    emit(
                        &TargetAuditRecord::Gap {
                            section,
                            site: pc,
                            encoding: instruction.to_vec(),
                            reason: TargetAuditGapReason::UnsupportedEncoding,
                        },
                        control,
                    )?;
                }
            }
            values.fill(None);
            values[0] = Some(0);
            offset += width;
            continue;
        };
        if usize::from(decoded.length) != width {
            return Err(Error::new(
                ErrorCode::Integrity,
                "instruction width disagreement",
            ));
        }
        summary.instructions += 1;
        let target = match decoded.flow {
            InstructionFlow::Jump { displacement, .. }
            | InstructionFlow::Branch { displacement } => {
                Some(pc.wrapping_add_signed(displacement))
            }
            flow @ (InstructionFlow::Indirect { .. } | InstructionFlow::Return { .. }) => {
                let (base, offset, _) = flow.through_register().unwrap_or_default();
                let target =
                    values[usize::from(base)].map(|n: u32| n.wrapping_add_signed(offset) & !1);
                if target.is_none() {
                    summary.unresolved_indirect += 1;
                }
                target
            }
            _ => None,
        };
        if let Some(target) = target {
            for range in ranges {
                control.checkpoint(1)?;
                if u64::from(target) >= u64::from(range.start) && u64::from(target) < range.end {
                    emit(
                        &TargetAuditRecord::Forbidden {
                            finding: ForbiddenTarget {
                                section,
                                site: pc,
                                target,
                                range: range.name.clone(),
                            },
                        },
                        control,
                    )?;
                    summary.forbidden_targets += 1;
                }
            }
        }
        let operand = |o| match o {
            Operand::Register(r) => values[usize::from(r)],
            Operand::Immediate(v) => Some(v),
        };
        match decoder.lift(instruction) {
            SemanticOp::Upper {
                dest,
                value,
                pc_relative,
            } => {
                values[usize::from(dest)] = Some(if pc_relative {
                    pc.wrapping_add(value)
                } else {
                    value
                })
            }
            SemanticOp::Integer {
                op,
                dest,
                left,
                right,
            } => {
                values[usize::from(dest)] = operand(left)
                    .zip(operand(right))
                    .map(|(a, b)| oer_riscv_analysis::fold_integer(op, a, b))
            }
            SemanticOp::Memory {
                dest: Some(dest), ..
            }
            | SemanticOp::Link { dest }
            | SemanticOp::Opaque { dest } => values[usize::from(dest)] = None,
            SemanticOp::Unsupported => values.fill(None),
            _ => (),
        }
        if !matches!(decoded.flow, InstructionFlow::Next) {
            values.fill(None);
        }
        values[0] = Some(0);
        offset += width;
    }
    Ok(summary)
}
