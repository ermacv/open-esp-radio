//! One audit of the prepared address uses, shared by both callback resolvers.
use super::locations::Locations;
use super::{Field, TypeFacts};
use crate::Analysis;
use oer_analysis_elf::rv32::Role;
use oer_analysis_riscv_lift::{RiscvDecoder, is_base_instruction};
use oer_analysis_riscv_model::*;
use oer_analysis_riscv_program::facts::ImageFacts;
use std::collections::{BTreeMap, BTreeSet};

/// The missing proof for one address use. These are analysis outcomes, not
/// function/address allowlists. A single unknown retains the whole function.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum UnknownUse {
    NonExactAddress,
    UnsupportedRelocation,
    MissingStorageType,
    UnpairedMaterialization,
    ForeignCode,
    UnsupportedMaterialization,
    ControlFlow,
    UnknownWrite,
    EscapedValue,
    IncompleteLifetime,
    NoStorage,
}

impl UnknownUse {
    /// The fact a later analysis would need to discharge this class.
    pub fn missing_fact(self) -> &'static str {
        match self {
            Self::NonExactAddress => "exact entry value without addend, odd bit or interior offset",
            Self::UnsupportedRelocation => "semantics of the address relocation",
            Self::MissingStorageType => "one typed function-pointer field at the static word",
            Self::UnpairedMaterialization => "adjacent absolute HI20/LO12 pair in one section",
            Self::ForeignCode => "instruction owner in the address use's ELF",
            Self::UnsupportedMaterialization => "exact address in the same destination register",
            Self::ControlFlow => "all uses across the control-flow boundary",
            Self::UnknownWrite => "one typed field at the word store's PC and memory operand",
            Self::EscapedValue => "type-preserving destination of the register read",
            Self::IncompleteLifetime => "all reads through the end of the register's lifetime",
            Self::NoStorage => "at least one proven typed storage use",
        }
    }
}

/// Every use has either concrete storage types or one specific unknown reason.
#[derive(Debug)]
pub enum StorageEvidence {
    Typed(BTreeSet<super::FunctionPointerType>),
    Unknown(UnknownUse),
}

/// A result tied to the original ImageFacts::address_uses entry. That entry
/// retains symbol identity, section, addend and raw target arithmetic.
#[derive(Debug)]
pub struct AddressUseAudit {
    pub index: usize,
    pub evidence: StorageEvidence,
}

/// Prepared once for both resolvers and diagnostics. Borrowing the image,
/// analysis and types prevents mixing DIE/section identities across ELFs.
pub struct FunctionPointerAudit<'a, 'elf> {
    pub(super) image: &'a ImageFacts<'elf>,
    pub(super) analysis: &'a Analysis,
    pub(super) types: &'a TypeFacts,
    pub(super) locations: Locations,
    pub(super) uses: BTreeMap<u32, Vec<AddressUseAudit>>,
}

#[derive(Clone, Copy)]
struct AddressUse {
    index: usize,
    site: u32,
    section: usize,
    role: Role,
    exact: bool,
}

impl FunctionPointerAudit<'_, '_> {
    /// Per-function results, including every merged symbol's uses. An empty
    /// slice means the address is taken but no storage evidence was found.
    pub fn uses(&self) -> impl Iterator<Item = (u32, &[AddressUseAudit])> {
        self.uses
            .iter()
            .map(|(&function, uses)| (function, uses.as_slice()))
    }

    pub(super) fn owns(&self, entry: u32) -> bool {
        self.image
            .functions()
            .binary_search_by_key(&entry, |f| f.address)
            .is_ok()
    }

    /// Both resolvers use this exact rule. No evidence and empty evidence
    /// are unknown; every proven storage type participates in the union.
    pub(super) fn candidates(
        &self,
        fields: &BTreeSet<Field>,
        keep: &mut Ledger<'_>,
        control: &mut dyn RunControl,
    ) -> Result<BTreeSet<u32>> {
        let mut targets = BTreeSet::new();
        for (&function, uses) in &self.uses {
            control.checkpoint(1)?;
            let mut possible = uses.is_empty();
            for use_ in uses {
                control.checkpoint(1)?;
                match &use_.evidence {
                    StorageEvidence::Unknown(_) => possible = true,
                    StorageEvidence::Typed(stored) => {
                        possible |= stored.is_empty();
                        for &storage in stored {
                            for &field in fields {
                                control.checkpoint(1)?;
                                possible |= compatible(self.types, field, storage);
                            }
                        }
                    }
                }
                if possible {
                    break;
                }
            }
            if possible {
                keep.add(&mut targets, function, control)?;
            }
        }
        Ok(targets)
    }
}

/// Audit the existing structural index without revisiting ELF/DWARF or
/// rebuilding their indexes. Optional local locations come from read_debug's
/// common walk. Without them, code stores conservatively remain unknown.
pub fn audit_function_addresses<'a, 'elf>(
    image: &'a ImageFacts<'elf>,
    analysis: &'a Analysis,
    types: &'a TypeFacts,
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<FunctionPointerAudit<'a, 'elf>> {
    let source = types
        .source
        .as_deref()
        .ok_or_else(|| Error::new(ErrorCode::Incompatible, "type facts have no source ELF"))?;
    if !std::ptr::eq(source, image.bytes()) {
        // Callers may prepare from an identical byte buffer. Charge comparison,
        // and never accept another ELF merely because its DIE offsets agree.
        control.bytes(source.len())?;
        if source != image.bytes() {
            return Err(Error::new(
                ErrorCode::Incompatible,
                "type facts belong to a different ELF",
            ));
        }
    }
    let mut scratch = Ledger::new(keep.memory(), control)?;
    let mut locations = Locations {
        aliases: BTreeMap::new(),
    };
    if types.locations.is_some() {
        locations.read_aliases(image, keep, control)?;
    }
    let mut uses = BTreeMap::new();
    let mut entries: BTreeMap<u32, Vec<AddressUse>> = BTreeMap::new();
    for (index, address_use) in image.address_uses().iter().enumerate() {
        control.checkpoint(1)?;
        // Keep taken functions even when the storage projection has no fact.
        if let Some(function) = crate::relocations::taken_address(address_use)
            && analysis.functions.contains_key(&function)
        {
            keep.entry(&mut uses, function, Vec::new, control)?;
        }
        let Some(projected) = crate::relocations::function_use(
            address_use,
            analysis,
            crate::relocations::FunctionUsePolicy::TypedStorage,
        )?
        else {
            continue;
        };
        // Arithmetic can relate the source symbol and result to different
        // functions. Neither may be silently dropped from the audit.
        let functions = BTreeSet::from_iter(
            [projected.function, projected.containing]
                .into_iter()
                .flatten(),
        );
        for function in functions {
            let into = scratch.entry(&mut entries, function, Vec::new, control)?;
            scratch.push(
                into,
                AddressUse {
                    index,
                    site: address_use.relocation.at as u32,
                    section: address_use.relocation.section,
                    role: address_use.role,
                    exact: projected.exact && projected.function == Some(function),
                },
                control,
            )?;
        }
    }
    for (function, mut entries) in entries {
        entries.sort_by_key(|entry| (entry.section, entry.site));
        let results = keep.entry(&mut uses, function, Vec::new, control)?;
        let mut at = 0;
        while let Some(entry) = entries.get(at).copied() {
            control.checkpoint(1)?;
            let low = entries.get(at + 1).copied().filter(|low| {
                entry.role == Role::AbsoluteHigh
                    && low.role == Role::AbsoluteLow
                    && low.section == entry.section
                    && low.site == entry.site + 4
            });
            let proof = if !entry.exact || low.is_some_and(|low| !low.exact) {
                Err(UnknownUse::NonExactAddress)
            } else if entry.role == Role::Word {
                types
                    .field(entry.site)
                    .map(|field| BTreeSet::from([field]))
                    .ok_or(UnknownUse::MissingStorageType)
            } else if let Some(low) = low {
                code_use(
                    image,
                    analysis,
                    types,
                    &locations,
                    function,
                    entry,
                    low,
                    &mut scratch,
                    control,
                )?
            } else if entry.role == Role::Other {
                Err(UnknownUse::UnsupportedRelocation)
            } else {
                Err(UnknownUse::UnpairedMaterialization)
            };
            // A materialization's two relocations each retain their outcome.
            for entry in std::iter::once(entry).chain(low) {
                let evidence = match &proof {
                    Ok(fields) if !fields.is_empty() => {
                        let mut stored = BTreeSet::new();
                        for &field in fields {
                            keep.add(&mut stored, field, control)?;
                        }
                        StorageEvidence::Typed(stored)
                    }
                    Ok(_) => StorageEvidence::Unknown(UnknownUse::NoStorage),
                    Err(reason) => StorageEvidence::Unknown(*reason),
                };
                keep.push(
                    results,
                    AddressUseAudit {
                        index: entry.index,
                        evidence,
                    },
                    control,
                )?;
            }
            at += if low.is_some() { 2 } else { 1 };
        }
    }
    Ok(FunctionPointerAudit {
        image,
        analysis,
        types,
        locations,
        uses,
    })
}

/// Existing deliberately small straight-line HI20/LO12 proof. Every read
/// must be a word store to a typed local field before the register is killed.
#[allow(clippy::too_many_arguments)]
fn code_use(
    image: &ImageFacts<'_>,
    analysis: &Analysis,
    types: &TypeFacts,
    locations: &Locations,
    function: u32,
    high: AddressUse,
    low: AddressUse,
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<std::result::Result<BTreeSet<Field>, UnknownUse>> {
    let Some((_, owner)) = analysis.functions.range(..=high.site).next_back() else {
        return Ok(Err(UnknownUse::ForeignCode));
    };
    let Ok(local) = image
        .functions()
        .binary_search_by_key(&owner.function.address, |function| function.address)
    else {
        return Ok(Err(UnknownUse::ForeignCode));
    };
    let owner = &image.functions()[local];
    if owner.section != high.section {
        return Ok(Err(UnknownUse::ForeignCode));
    }
    let code = crate::image::function_bytes(image.file(), owner)?;
    let at = high.site.wrapping_sub(owner.address) as usize;
    let Some(bytes) = code.get(at..) else {
        return Ok(Err(UnknownUse::UnsupportedMaterialization));
    };
    let Some(first) = RiscvDecoder.decode(bytes) else {
        return Ok(Err(UnknownUse::UnsupportedMaterialization));
    };
    let SemanticOp::Upper {
        dest: high_register,
        value,
        pc_relative: false,
    } = RiscvDecoder.lift(bytes)
    else {
        return Ok(Err(UnknownUse::UnsupportedMaterialization));
    };
    if first.length != 4 {
        return Ok(Err(UnknownUse::UnsupportedMaterialization));
    }
    let Some(bytes) = code.get(at + 4..) else {
        return Ok(Err(UnknownUse::UnsupportedMaterialization));
    };
    let Some(second) = RiscvDecoder.decode(bytes) else {
        return Ok(Err(UnknownUse::UnsupportedMaterialization));
    };
    let SemanticOp::Integer {
        op: IntegerOp::Add,
        dest: register,
        left: Operand::Register(base),
        right: Operand::Immediate(immediate),
    } = RiscvDecoder.lift(bytes)
    else {
        return Ok(Err(UnknownUse::UnsupportedMaterialization));
    };
    if base != high_register
        || register != high_register
        || register == 0
        || value.wrapping_add(immediate) != function
    {
        return Ok(Err(UnknownUse::UnsupportedMaterialization));
    }
    let mut site = low.site + u32::from(second.length);
    let mut fields = BTreeSet::new();
    while let Some(bytes) = code.get(site.wrapping_sub(owner.address) as usize..) {
        control.checkpoint(1)?;
        let Some(decoded) = RiscvDecoder.decode(bytes) else {
            return Ok(Err(UnknownUse::IncompleteLifetime));
        };
        if !is_base_instruction(bytes) || decoded.flow != InstructionFlow::Next {
            return Ok(Err(UnknownUse::ControlFlow));
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
                    let Some(field) = field else {
                        return Ok(Err(UnknownUse::UnknownWrite));
                    };
                    keep.add(&mut fields, field, control)?;
                }
                _ => return Ok(Err(UnknownUse::EscapedValue)),
            }
        }
        if op.destination_register() == Some(register) {
            return Ok(Ok(fields));
        }
        site += u32::from(decoded.length);
    }
    Ok(Err(UnknownUse::IncompleteLifetime))
}

fn compatible(types: &TypeFacts, a: Field, b: Field) -> bool {
    let (Some(a_signature), Some(b_signature)) = (
        types.signatures.get(&a.subroutine),
        types.signatures.get(&b.subroutine),
    ) else {
        return true;
    };
    super::shape_matches(
        &types.shape(a_signature),
        &types.shape(b_signature),
        !a.foreign && !b.foreign,
    )
}
