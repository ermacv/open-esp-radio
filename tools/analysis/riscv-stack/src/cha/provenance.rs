//! Local callback field discovery; exclusion belongs to the shared audit.
use super::FunctionPointerAudit;
use crate::{
    Fact, FunctionFacts, FunctionPointerType, Load, RegisterWitness, Resolutions, TransferKind,
};
use oer_analysis_riscv_lift::RiscvDecoder;
use oer_analysis_riscv_model::*;
use std::collections::BTreeSet;

/// The storage type of a callback load, with the PC-scoped register witness
/// used when its address base was transported from an earlier instruction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CallbackLoadField {
    pub field: FunctionPointerType,
    pub witness: Option<RegisterWitness>,
}

impl FunctionPointerAudit<'_, '_> {
    /// Evidence for the target load at `site` in this audit's function.
    /// This reads the same prepared facts used by the local resolver.
    pub fn load_field(
        &self,
        function: u32,
        site: u32,
        control: &mut dyn RunControl,
    ) -> Result<Option<CallbackLoadField>> {
        if !self.owns(function) {
            return Ok(None);
        }
        let Some(facts) = self.analysis.functions.get(&function) else {
            return Ok(None);
        };
        for load in facts.target_loads.values().flatten() {
            control.checkpoint(1)?;
            if load.site == site {
                return self.field_for_load(facts, load, control);
            }
        }
        Ok(None)
    }

    fn field_for_load(
        &self,
        facts: &FunctionFacts,
        load: &Load,
        control: &mut dyn RunControl,
    ) -> Result<Option<CallbackLoadField>> {
        let mut field = load.address.and_then(|address| self.types.field(address));
        if field.is_none() {
            let fields =
                self.locations
                    .fields(self.types, load.site, load.base, load.offset, control)?;
            if fields.len() > 1 {
                return Ok(None);
            }
            field = fields.first().copied();
        }
        if let Some(field) = field {
            return Ok(Some(CallbackLoadField {
                field,
                witness: None,
            }));
        }
        if let Some(base) = facts.load_bases.get(&load.site) {
            let mut found = None;
            for witness in &base.witnesses {
                control.checkpoint(1)?;
                for field in self.locations.fields(
                    self.types,
                    witness.site,
                    witness.register,
                    load.offset,
                    control,
                )? {
                    if found.is_some_and(|proof: CallbackLoadField| proof.field != field) {
                        return Ok(None);
                    }
                    found.get_or_insert(CallbackLoadField {
                        field,
                        witness: Some(*witness),
                    });
                }
            }
            return Ok(found);
        }
        Ok(None)
    }
}

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
                    if let Some(proof) = audit.field_for_load(facts, load, control)? {
                        scratch.add(&mut fields, proof.field, control)?;
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
