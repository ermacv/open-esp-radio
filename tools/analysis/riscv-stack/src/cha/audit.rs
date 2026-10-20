//! One audit of the prepared address uses, shared by both callback resolvers.
use super::locations::Locations;
use super::{Field, TypeFacts};
use crate::Analysis;
use oer_analysis_elf::rv32::Role;
use oer_analysis_riscv_lift::{RiscvDecoder, is_base_instruction};
use oer_analysis_riscv_model::*;
use oer_analysis_riscv_program::facts::{ImageFacts, addresses::TargetValue};
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
    InvalidPcRelativePair,
    UnresolvedTarget,
    RegisterClobbered,
    IncomingPath,
}

impl UnknownUse {
    /// The fact a later analysis would need to discharge this class.
    pub fn missing_fact(self) -> &'static str {
        match self {
            Self::NonExactAddress => "exact entry value without addend, odd bit or interior offset",
            Self::UnsupportedRelocation => "semantics of the address relocation",
            Self::MissingStorageType => "one typed function-pointer field at the static word",
            Self::UnpairedMaterialization => {
                "a proven materialization pair (absolute HI20/LO12 pairs require adjacency)"
            }
            Self::ForeignCode => "instruction owner in the address use's ELF",
            Self::UnsupportedMaterialization => "exact address in the same destination register",
            Self::ControlFlow => "all uses across the control-flow boundary",
            Self::UnknownWrite => "one typed field at the word store's PC and memory operand",
            Self::EscapedValue => "type-preserving destination of the register read",
            Self::IncompleteLifetime => "all reads through the end of the register's lifetime",
            Self::NoStorage => "at least one proven typed storage use",
            Self::InvalidPcRelativePair => {
                "one known HI20 selected by the low label's section and identity"
            }
            Self::UnresolvedTarget => "resolved RV32 target of the normalized materialization",
            Self::RegisterClobbered => "the AUIPC value reaches ADDI without a base-register write",
            Self::IncomingPath => {
                "every incoming path executes the materialization before this use"
            }
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
    /// Concrete stores encountered before the proof completed or failed.
    pub stores: Vec<TypedStore>,
    /// The use/instruction at which a proof stopped being known.
    pub barrier: Option<u32>,
}

/// A typed word store observed in the materialized register's lifetime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TypedStore {
    pub site: u32,
    pub base: u8,
    pub displacement: i32,
    pub field: super::FunctionPointerType,
}

/// The normalizer's result for every original PC-relative low, including
/// pairs whose actual target is data rather than a function. All indexes
/// refer to this audit's original ImageFacts observations.
#[derive(Debug)]
pub struct PcRelativeUse {
    pub low: usize,
    pub high: Option<usize>,
    pub target: Option<u32>,
    pub functions: BTreeSet<u32>,
    /// No inferred callback owner exists when a pair/target is unknown.
    /// Such a use conservatively prevents type exclusion for the image.
    pub unknown: Option<UnknownUse>,
}

/// Prepared once for both resolvers and diagnostics. Borrowing the image,
/// analysis and types prevents mixing DIE/section identities across ELFs.
pub struct FunctionPointerAudit<'a, 'elf> {
    pub(super) image: &'a ImageFacts<'elf>,
    pub(super) analysis: &'a Analysis,
    pub(super) types: &'a TypeFacts,
    pub(super) locations: Locations,
    pub(super) uses: BTreeMap<u32, Vec<AddressUseAudit>>,
    pc_relative: Vec<PcRelativeUse>,
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

    pub fn pc_relative_uses(&self) -> &[PcRelativeUse] {
        &self.pc_relative
    }

    /// The shared storage-signature comparison, also available to diagnostic
    /// consumers explaining an exclusion. Unknown type slots agree.
    pub fn compatible_storage(
        &self,
        field: super::FunctionPointerType,
        storage: super::FunctionPointerType,
        control: &mut dyn RunControl,
    ) -> Result<bool> {
        control.checkpoint(1)?;
        Ok(compatible(self.types, field, storage))
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
        let unbound = self.pc_relative.iter().any(|use_| use_.unknown.is_some());
        if unbound {
            // An orphan/unresolved pair has no callback target to attach its
            // uncertainty to. No function in the joint catalog may be excluded.
            for &function in self.analysis.functions.keys() {
                control.checkpoint(1)?;
                keep.add(&mut targets, function, control)?;
            }
            return Ok(targets);
        }
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
                                possible |= self.compatible_storage(field, storage, control)?;
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
    let mut locations = Locations::default();
    locations.read_aliases(image, keep, control)?;
    let mut uses = BTreeMap::new();
    let mut pc_relative = Vec::new();
    let mut paired_highs = BTreeSet::new();
    super::references::visit(
        image,
        keep.memory(),
        control,
        &mut |low, high, normalized, control| {
            let low_index = low.index as usize;
            let mut observation = PcRelativeUse {
                low: low_index,
                high: high.map(|high| high.index as usize),
                target: None,
                functions: BTreeSet::new(),
                unknown: None,
            };
            if let Some(high) = high {
                let high_index = high.index as usize;
                scratch.add(&mut paired_highs, high_index, control)?;
                let original = &image.address_uses()[high_index];
                let normalized_value = normalized
                    .addend
                    .map(|addend| normalized.target.offset.wrapping_add_signed(addend));
                if let TargetValue::Resolved(value) = original.value
                    && normalized_value == Some(value)
                    && let Ok(target) = u32::try_from(value)
                {
                    observation.target = Some(target);
                    if let Some(projected) = crate::relocations::function_use(
                        original,
                        analysis,
                        crate::relocations::FunctionUsePolicy::TypedStorage,
                    )? {
                        for function in [projected.function, projected.containing]
                            .into_iter()
                            .flatten()
                        {
                            keep.add(&mut observation.functions, function, control)?;
                        }
                        for &function in &observation.functions {
                            let high_use = AddressUse {
                                index: high_index,
                                site: original.relocation.at as u32,
                                section: original.relocation.section,
                                role: original.role,
                                exact: projected.exact && projected.function == Some(function),
                            };
                            let low_use = AddressUse {
                                index: low_index,
                                site: low.offset as u32,
                                section: low.section as usize,
                                role: Role::PcRelativeLow,
                                exact: high_use.exact,
                            };
                            let proof = if high_use.exact {
                                code_use(
                                    image,
                                    analysis,
                                    types,
                                    &locations,
                                    function,
                                    high_use,
                                    low_use,
                                    Some(low),
                                    &mut scratch,
                                    control,
                                )?
                            } else {
                                Proof::unknown(UnknownUse::NonExactAddress, high_use.site)
                            };
                            record(&mut uses, function, high_index, &proof, true, keep, control)?;
                            record(&mut uses, function, low_index, &proof, false, keep, control)?;
                        }
                    }
                } else {
                    observation.unknown = Some(UnknownUse::UnresolvedTarget);
                }
            } else {
                observation.unknown = Some(UnknownUse::InvalidPcRelativePair);
            }
            keep.push(&mut pc_relative, observation, control)?;
            Ok(())
        },
    )?;
    let mut entries: BTreeMap<u32, Vec<AddressUse>> = BTreeMap::new();
    for (index, address_use) in image.address_uses().iter().enumerate() {
        control.checkpoint(1)?;
        // LO12 names the AUIPC label, not a callback. Paired high uses
        // were already audited through the normalizer's actual target.
        if address_use.role == Role::PcRelativeLow || paired_highs.contains(&index) {
            continue;
        }
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
                Proof::unknown(UnknownUse::NonExactAddress, entry.site)
            } else if entry.role == Role::Word {
                match types.field(entry.site) {
                    Some(field) => Proof {
                        fields: BTreeSet::from([field]),
                        ..Proof::default()
                    },
                    None => Proof::unknown(UnknownUse::MissingStorageType, entry.site),
                }
            } else if let Some(low) = low {
                code_use(
                    image,
                    analysis,
                    types,
                    &locations,
                    function,
                    entry,
                    low,
                    None,
                    &mut scratch,
                    control,
                )?
            } else if entry.role == Role::Other {
                Proof::unknown(UnknownUse::UnsupportedRelocation, entry.site)
            } else {
                Proof::unknown(UnknownUse::UnpairedMaterialization, entry.site)
            };
            for entry in std::iter::once(entry).chain(low) {
                record(
                    &mut uses,
                    function,
                    entry.index,
                    &proof,
                    false,
                    keep,
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
        pc_relative,
    })
}

#[derive(Default)]
struct Proof {
    fields: BTreeSet<Field>,
    reason: Option<UnknownUse>,
    stores: Vec<TypedStore>,
    barrier: Option<u32>,
}

impl Proof {
    fn unknown(reason: UnknownUse, site: u32) -> Self {
        Self::default().stop(reason, site)
    }
    fn stop(mut self, reason: UnknownUse, site: u32) -> Self {
        self.reason = Some(reason);
        self.barrier = Some(site);
        self
    }
}

fn record(
    uses: &mut BTreeMap<u32, Vec<AddressUseAudit>>,
    function: u32,
    index: usize,
    proof: &Proof,
    merge_high: bool,
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<()> {
    let records = keep.entry(uses, function, Vec::new, control)?;
    let reason = proof
        .reason
        .or_else(|| proof.fields.is_empty().then_some(UnknownUse::NoStorage));
    // A high relocation can feed multiple lows. Retain it once, and never
    // let a successful low erase an unknown read of the same high value.
    let mut at = None;
    if merge_high {
        for (position, entry) in records.iter().enumerate() {
            control.checkpoint(1)?;
            if entry.index == index {
                at = Some(position);
                break;
            }
        }
    }
    let entry = if let Some(at) = at {
        &mut records[at]
    } else {
        keep.push(
            records,
            AddressUseAudit {
                index,
                evidence: StorageEvidence::Typed(BTreeSet::new()),
                stores: Vec::new(),
                barrier: None,
            },
            control,
        )?;
        records.last_mut().unwrap()
    };
    if let Some(reason) = reason {
        if !matches!(entry.evidence, StorageEvidence::Unknown(_)) {
            entry.evidence = StorageEvidence::Unknown(reason);
            entry.barrier = proof.barrier;
        }
    } else if let StorageEvidence::Typed(fields) = &mut entry.evidence {
        for &field in &proof.fields {
            keep.add(fields, field, control)?;
        }
    }
    for &store in &proof.stores {
        if !entry.stores.contains(&store) {
            keep.push(&mut entry.stores, store, control)?;
        }
    }
    Ok(())
}

/// The same deliberately small proof for absolute and PC-relative address
/// materializations. Pair identity and base-register provenance are separate:
/// each instruction on the straight-line path must preserve the upper value,
/// and no other incoming path may bypass it or enter its subsequent lifetime.
#[allow(clippy::too_many_arguments)]
fn code_use(
    image: &ImageFacts<'_>,
    analysis: &Analysis,
    types: &TypeFacts,
    locations: &Locations,
    function: u32,
    high: AddressUse,
    low: AddressUse,
    low_reference: Option<&FunctionRelocation>,
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<Proof> {
    let mut proof = Proof::default();
    macro_rules! unknown {
        ($reason:ident, $site:expr) => {
            return Ok(proof.stop(UnknownUse::$reason, $site))
        };
    }
    let Some((_, facts)) = analysis.functions.range(..=high.site).next_back() else {
        unknown!(ForeignCode, high.site);
    };
    let Ok(local) = image
        .functions()
        .binary_search_by_key(&facts.function.address, |f| f.address)
    else {
        unknown!(ForeignCode, high.site);
    };
    let owner = &image.functions()[local];
    if owner.section != high.section || low.section != high.section {
        unknown!(ForeignCode, low.site);
    }
    if let Some(&site) = locations.unclosed.get(&(owner.section, owner.address)) {
        unknown!(ControlFlow, site);
    }
    let incoming = |site| {
        locations.entries.contains(&(owner.section, site))
            || image
                .functions()
                .binary_search_by_key(&site, |f| f.address)
                .is_ok()
    };
    let code = crate::image::function_bytes(image.file(), owner)?;
    let bytes_at = |site: u32| code.get(site.wrapping_sub(owner.address) as usize..);
    let Some(bytes) = bytes_at(high.site) else {
        unknown!(UnsupportedMaterialization, high.site);
    };
    let Some(first) = RiscvDecoder.decode(bytes) else {
        unknown!(UnsupportedMaterialization, high.site);
    };
    let SemanticOp::Upper {
        dest: high_register,
        value,
        pc_relative,
    } = RiscvDecoder.lift(bytes)
    else {
        unknown!(UnsupportedMaterialization, high.site);
    };
    if first.length != 4 || high_register == 0 || pc_relative != (high.role == Role::PcRelativeHigh)
    {
        unknown!(UnsupportedMaterialization, high.site);
    }
    let mut site = high.site + 4;
    if site > low.site {
        unknown!(UnsupportedMaterialization, low.site);
    }
    while site < low.site {
        control.checkpoint(1)?;
        if incoming(site) {
            unknown!(IncomingPath, site);
        }
        let Some(bytes) = bytes_at(site) else {
            unknown!(UnsupportedMaterialization, site);
        };
        let Some(decoded) = RiscvDecoder.decode(bytes) else {
            unknown!(UnsupportedMaterialization, site);
        };
        if !is_base_instruction(bytes) || site + u32::from(decoded.length) > low.site {
            unknown!(UnsupportedMaterialization, site);
        }
        if decoded.flow != InstructionFlow::Next {
            unknown!(ControlFlow, site);
        }
        let op = RiscvDecoder.lift(bytes);
        if op.destination_register() == Some(high_register) {
            unknown!(RegisterClobbered, site);
        }
        if op.reads_register(high_register) {
            unknown!(EscapedValue, site);
        }
        site += u32::from(decoded.length);
    }
    if incoming(low.site) {
        unknown!(IncomingPath, low.site);
    }
    let Some(bytes) = bytes_at(low.site) else {
        unknown!(UnsupportedMaterialization, low.site);
    };
    let Some(second) = RiscvDecoder.decode(bytes) else {
        unknown!(UnsupportedMaterialization, low.site);
    };
    let op = RiscvDecoder.lift(bytes);
    if pc_relative
        && low_reference.is_none_or(|reference| {
            RiscvDecoder.value_relocation(reference, op) != ValueRelocation::LowerPcRelative
        })
    {
        unknown!(UnsupportedMaterialization, low.site);
    }
    let SemanticOp::Integer {
        op: IntegerOp::Add,
        dest: register,
        left: Operand::Register(base),
        right: Operand::Immediate(immediate),
    } = op
    else {
        unknown!(UnsupportedMaterialization, low.site);
    };
    let upper = if pc_relative {
        value.wrapping_add(high.site)
    } else {
        value
    };
    if base != high_register
        || register != high_register
        || upper.wrapping_add(immediate) != function
    {
        unknown!(UnsupportedMaterialization, low.site);
    }
    site = low.site + u32::from(second.length);
    while let Some(bytes) = bytes_at(site) {
        if bytes.is_empty() {
            break;
        }
        control.checkpoint(1)?;
        if incoming(site) {
            unknown!(IncomingPath, site);
        }
        let Some(decoded) = RiscvDecoder.decode(bytes) else {
            unknown!(IncompleteLifetime, site);
        };
        if !is_base_instruction(bytes) || decoded.flow != InstructionFlow::Next {
            unknown!(ControlFlow, site);
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
                        unknown!(UnknownWrite, site);
                    };
                    keep.add(&mut proof.fields, field, control)?;
                    keep.push(
                        &mut proof.stores,
                        TypedStore {
                            site,
                            base,
                            displacement,
                            field,
                        },
                        control,
                    )?;
                }
                _ => {
                    unknown!(EscapedValue, site);
                }
            }
        }
        if op.destination_register() == Some(register) {
            return Ok(if proof.fields.is_empty() {
                proof.stop(UnknownUse::NoStorage, site)
            } else {
                proof
            });
        }
        site += u32::from(decoded.length);
    }
    unknown!(IncompleteLifetime, site);
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
