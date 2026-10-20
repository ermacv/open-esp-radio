//! Local callback field discovery; exclusion belongs to the shared audit.
use super::FunctionPointerAudit;
use crate::{Fact, Resolutions, TransferKind};
use oer_analysis_riscv_lift::RiscvDecoder;
use oer_analysis_riscv_model::*;
use std::collections::BTreeSet;

/// Resolve function fields held by PC-specific typed pointer/aggregate
/// locations. Candidates come from every address use's storage type, never
/// a declaration signature alone. One unsupported use keeps its function
/// in every site's candidate set. Empty sets do not prove an unreachable
/// call. The existing type/executor assumption still applies to memory
/// reconstructed through an erased pointer.
/// The audit must use types prepared by `read_debug` with
/// `DebugMetadata::TypedLocations` for its image. This resolver reads prepared
/// metadata and instructions; it does not load or traverse DWARF.
pub fn typed_function_pointer_resolutions(
    audit: &FunctionPointerAudit<'_, '_>,
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<Resolutions> {
    let image = audit.image;
    let analysis = audit.analysis;
    let types = audit.types;
    let locations = &audit.locations;
    let mut scratch = Ledger::new(keep.memory(), control)?;
    if types.locations.is_none() {
        return Err(Error::new(
            ErrorCode::Incompatible,
            "typed locations were not requested by read_debug",
        ));
    }
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
            let targets = audit.candidates(&fields, &mut scratch, control)?;
            keep.admit(Resolutions::admission(targets.len()), control)?;
            resolutions.add(transfer.site, Fact::FieldType, targets);
        }
    }
    Ok(resolutions)
}

#[cfg(test)]
mod tests;
