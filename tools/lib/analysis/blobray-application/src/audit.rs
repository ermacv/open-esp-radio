//! Final-image audit of statically resolved transfers into forbidden ranges.
use crate::in_process::Executable;
use crate::*;

/// Audit every executable section of `executable` for statically resolved
/// transfers into `ranges`, presenting each finding to `emit`.
pub fn audit_targets(
    executable: &Executable,
    ranges: &[ForbiddenTargetRange],
    decoder: &dyn FunctionSemantics,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    emit: &mut impl FnMut(&TargetAuditRecord, &mut dyn RunControl) -> Result<()>,
) -> Result<TargetAuditSummary> {
    if ranges.is_empty() || ranges.len() > 64 {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "audit requires 1..64 forbidden ranges",
        ));
    }
    for range in ranges {
        range.validate()?;
    }
    let mut position = RunPosition {
        phase: RunPhase::AnalyzeFunction,
        ..Default::default()
    };
    position.artifact(executable.id());
    control.set_position(position);
    control.checkpoint(0)?;
    let mut total = TargetAuditSummary::default();
    oer_riscv_program::executable_sections(
        executable.bytes(),
        memory,
        control,
        &mut |section, c| {
            let part = blobray_analysis::audit::audit_section(section, decoder, ranges, c, emit)?;
            total.sections += part.sections;
            total.executable_bytes += part.executable_bytes;
            total.embedded_data_bytes += part.embedded_data_bytes;
            total.instructions += part.instructions;
            total.unsupported_non_control += part.unsupported_non_control;
            total.unresolved_indirect += part.unresolved_indirect;
            total.coverage_gaps += part.coverage_gaps;
            total.forbidden_targets += part.forbidden_targets;
            Ok(())
        },
    )?;
    Ok(total)
}
