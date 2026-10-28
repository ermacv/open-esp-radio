//! Exact core Waker identities and pointer-sized vtable projections.
use super::{TypeFacts, WakerType};
use crate::dwarf::locations::Place;
use oer_analysis_elf::dwarf::gimli;
use oer_analysis_riscv_model::{Result, RunControl};

impl TypeFacts {
    pub(crate) fn identify_waker(&mut self, id: usize, qualified: &str) {
        if let Some(ty) = self.types.get_mut(&id) {
            ty.waker = match qualified {
                "core::task::wake::Waker" => Some(WakerType::Waker),
                "core::task::wake::RawWaker" => Some(WakerType::Raw),
                "core::task::wake::RawWakerVTable" => Some(WakerType::Vtable),
                "core::option::Option<core::task::wake::Waker>" => Some(WakerType::Option),
                _ => None,
            };
        }
    }

    fn named_member(&self, id: usize, name: &str) -> Option<(u64, usize)> {
        let (_, ty) = self.resolve(id)?;
        if ty.tag != gimli::DW_TAG_structure_type || ty.has_variants {
            return None;
        }
        let &at = ty.named_members.get(name)?;
        let mut matches = ty.members.iter().filter(|(offset, _)| *offset == at);
        let &member = matches.next()?;
        matches.next().is_none().then_some(member)
    }

    /// Byte offset of the complete vtable pointer, with the niche requiring
    /// a separate machine proof of Some. No source-language field order is ABI.
    fn waker_projection(&self, id: usize) -> Option<(u64, bool)> {
        let (_, ty) = self.resolve(id)?;
        match ty.waker? {
            WakerType::Waker => {
                let (at, raw) = self.named_member(id, "waker")?;
                if self.resolve(raw)?.1.waker != Some(WakerType::Raw) {
                    return None;
                }
                let (within, _) = self.waker_projection(raw)?;
                let offset = at.checked_add(within)?;
                (offset.checked_add(4)? <= self.size(id)?).then_some((offset, false))
            }
            WakerType::Raw => {
                let (at, pointer) = self.named_member(id, "vtable")?;
                let (_, pointer_ty) = self.resolve(pointer)?;
                if pointer_ty.tag != gimli::DW_TAG_pointer_type || self.size(pointer)? != 4 {
                    return None;
                }
                let (_, table) = self.resolve(pointer_ty.target?)?;
                if table.waker != Some(WakerType::Vtable)
                    || table.has_variants
                    || table.tag != gimli::DW_TAG_structure_type
                    || table.size != Some(16)
                {
                    return None;
                }
                for (index, name) in ["clone", "wake", "wake_by_ref", "drop"]
                    .into_iter()
                    .enumerate()
                {
                    let (offset, field) = self.named_member(pointer_ty.target?, name)?;
                    let (_, field_ty) = self.resolve(field)?;
                    if offset != (index as u64).checked_mul(4)?
                        || self.size(field)? != 4
                        || field_ty.tag != gimli::DW_TAG_pointer_type
                        || self.resolve(field_ty.target?)?.1.tag != gimli::DW_TAG_subroutine_type
                    {
                        return None;
                    }
                }
                (at.checked_add(4)? <= self.size(id)?).then_some((at, false))
            }
            WakerType::Option => {
                // Accept only the described two-variant null niche. The
                // discriminator is the vtable word, None is zero, and the
                // default variant contains exactly one Waker. Other enum
                // layouts do not supply an initialized Waker field.
                if ty.tag != gimli::DW_TAG_structure_type
                    || !ty.has_variants
                    || ty.variants.len() != 2
                    || ty.variants.iter().any(|variant| !variant.supported)
                {
                    return None;
                }
                let none = ty.variants.iter().find(|v| v.value == Some(0))?;
                let some = ty.variants.iter().find(|v| v.value.is_none())?;
                let &[(none_at, none_ty)] = none.members.as_slice() else {
                    return None;
                };
                let (_, empty) = self.resolve(none_ty)?;
                if empty.tag != gimli::DW_TAG_structure_type
                    || empty.has_variants
                    || !empty.members.is_empty()
                    || none_at.checked_add(self.size(none_ty)?)? > self.size(id)?
                {
                    return None;
                }
                let &[(at, payload)] = some.members.as_slice() else {
                    return None;
                };
                let (_, payload_ty) = self.resolve(payload)?;
                if payload_ty.has_variants || payload_ty.tag != gimli::DW_TAG_structure_type {
                    return None;
                }
                let &[(within, waker)] = payload_ty.members.as_slice() else {
                    return None;
                };
                if self.resolve(waker)?.1.waker != Some(WakerType::Waker)
                    || at.checked_add(self.size(payload)?)? > self.size(id)?
                    || within.checked_add(self.size(waker)?)? > self.size(payload)?
                {
                    return None;
                }
                let (field, _) = self.waker_projection(waker)?;
                let offset = at.checked_add(within)?.checked_add(field)?;
                let (_, Some((discriminant, discr_ty))) = ty.discriminant? else {
                    return None;
                };
                (discriminant == offset
                    && self.size(discr_ty)? == 4
                    && offset.checked_add(4)? <= self.size(id)?)
                .then_some((offset, true))
            }
            WakerType::Vtable => None,
        }
    }

    /// A single concrete variable's same register piece covers every byte
    /// through the transfer. Adjacent location-list entries may join, but
    /// neither a gap nor a different owner may extend a location's lifetime.
    pub(crate) fn waker_register(
        &self,
        register: u8,
        start: u32,
        end: u32,
        control: &mut dyn RunControl,
    ) -> Result<Option<bool>> {
        let Some(debug) = &self.locations else {
            return Ok(None);
        };
        let Some(locations) = debug.registers.get(&register) else {
            return Ok(None);
        };
        let mut niche = None;
        for location in locations {
            control.checkpoint(1)?;
            if start >= end
                || location.start > start
                || location.end <= start
                || location.place != Place::Register(register)
            {
                continue;
            }
            let Some(ty) = debug.variable_type(location.owner) else {
                continue;
            };
            let Some((offset, option)) = self.waker_projection(ty) else {
                continue;
            };
            if location.offset != offset || location.bytes.or_else(|| self.size(ty)) != Some(4) {
                continue;
            }
            let mut covered = location.end;
            while covered < end {
                let mut next = covered;
                for part in locations {
                    control.checkpoint(1)?;
                    if part.owner == location.owner
                        && part.offset == offset
                        && part.bytes.or_else(|| self.size(ty)) == Some(4)
                        && part.place == location.place
                        && part.start <= covered
                        && part.end > next
                    {
                        next = part.end;
                    }
                }
                if next == covered {
                    break;
                }
                covered = next;
            }
            if covered >= end {
                // An ordinary Waker witness needs no niche proof even if
                // an overlapping Option parameter describes the same value.
                if !option {
                    return Ok(Some(false));
                }
                niche = Some(true);
            }
        }
        Ok(niche)
    }
}

#[cfg(test)]
mod tests;
