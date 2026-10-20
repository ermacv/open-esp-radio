//! Function-pointer field discovery and shared storage-type matching.
//! Candidate exclusion is owned by the per-address-use audit, never by a
//! function declaration. Global and PC-specific field discovery stay distinct.
use crate::dwarf::{Entry, Reader, Unit, reference, text};
use crate::{Fact, Resolutions, TransferKind};
use oer_analysis_elf::dwarf::gimli;
use oer_analysis_riscv_model::{Ledger, Result, RunControl};
use std::collections::{BTreeMap, BTreeSet};

mod audit;
mod locations;
mod provenance;
pub use audit::{
    AddressUseAudit, FunctionPointerAudit, StorageEvidence, UnknownUse, audit_function_addresses,
};
pub use provenance::typed_function_pointer_resolutions;

/// One type's name and layout.
#[derive(Debug)]
struct Type {
    tag: gimli::DwTag,
    name: Option<String>,
    size: Option<u64>,
    /// Members (variants' members included) by offset, and their types.
    members: Vec<(u64, usize)>,
    named_members: BTreeMap<String, u64>,
    /// The type a typedef, qualifier or pointer refers to.
    target: Option<usize>,
}

/// A subprogram's or subroutine type's parameter types and result type.
#[derive(Debug, Default, Clone)]
struct Signature {
    parameters: Vec<usize>,
    result: Option<usize>,
}

/// One parameter's or result's type as matching sees it: its byte size and
/// the name rustc gives it, behind typedefs and qualifiers (`None` when the
/// DWARF leaves either unknown).
type Slot = (Option<u64>, Option<String>);

/// A signature's shape: its parameters' slots and its result's.
type Shape = (Vec<Slot>, Slot);

/// Whether a function's slot can be a field's: neither a known size nor a
/// known name differs. rustc names each type once, so a type never has two
/// names; an unknown size or name contradicts nothing.
fn slot_matches(function: &Slot, field: &Slot) -> bool {
    fn agree<T: PartialEq>(a: &Option<T>, b: &Option<T>) -> bool {
        match (a, b) {
            (Some(a), Some(b)) => a == b,
            _ => true,
        }
    }
    agree(&function.0, &field.0) && agree(&function.1, &field.1)
}

/// Whether a function's shape can be a field's; `names` compares type names
/// too, as only a Rust-ABI field may.
fn shape_matches(function: &Shape, field: &Shape, names: bool) -> bool {
    let slot = |function: &Slot, field: &Slot| {
        if names {
            slot_matches(function, field)
        } else {
            slot_matches(&(function.0, None), &(field.0, None))
        }
    };
    function.0.len() == field.0.len()
        && function.0.iter().zip(&field.0).all(|(a, b)| slot(a, b))
        && slot(&function.1, &field.1)
}

/// A function-pointer field: its subroutine type and whether its ABI is
/// foreign.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct FunctionPointerType {
    /// Subroutine DIE offset in this audit's ELF.
    pub subroutine: usize,
    /// Whether matching may compare sizes only.
    pub foreign: bool,
}

type Field = FunctionPointerType;

/// What the image's DWARF states about types and storage.
#[derive(Debug, Default)]
pub struct TypeFacts {
    /// Retain the source identity; DIE offsets are local to this ELF.
    pub(crate) source: Option<std::rc::Rc<[u8]>>,
    /// Optional prepared locals, with type DIE identities from this same ELF.
    pub(crate) locations: Option<crate::dwarf::locations::LocalLocations>,
    types: BTreeMap<usize, Type>,
    /// Pieces of global variables: address, bytes (`None` for the whole
    /// variable), offset in the variable, the variable's type.
    globals: Vec<(u32, Option<u64>, u64, usize)>,
    /// Storage subroutine signatures, by DIE.
    signatures: BTreeMap<usize, Signature>,
}

/// One unit's walk state of [`TypeWalk`].
#[derive(Default)]
struct UnitTypes {
    /// The type, subprogram or subroutine type each depth belongs to.
    owners: Vec<(isize, usize, gimli::DwTag)>,
}

/// Types collected by the common DWARF walk. No symbol or declaration index
/// is needed to prove storage provenance.
#[derive(Default)]
pub(crate) struct TypeWalk {
    facts: TypeFacts,
    unit: UnitTypes,
}

impl TypeWalk {
    pub(crate) fn begin_unit(&mut self) {
        self.unit = UnitTypes::default();
    }

    /// What one entry states.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn entry<'d>(
        &mut self,
        dwarf: &gimli::Dwarf<Reader<'d>>,
        unit: &Unit<'d>,
        entry: &Entry<'d>,
        depth: isize,
        offset: usize,
        keep: &mut Ledger<'_>,
        scratch: &mut Ledger<'_>,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        let facts = &mut self.facts;
        let state = &mut self.unit;
        state.owners.retain(|(at, ..)| *at < depth);
        let name = entry
            .attr_value(gimli::DW_AT_name)
            .and_then(|value| text(dwarf, unit, value));
        let size = entry
            .attr_value(gimli::DW_AT_byte_size)
            .and_then(|value| value.udata_value());
        let type_of = entry
            .attr_value(gimli::DW_AT_type)
            .and_then(|value| reference(unit, value));
        let tag = entry.tag();
        match tag {
            gimli::DW_TAG_structure_type
            | gimli::DW_TAG_union_type
            | gimli::DW_TAG_enumeration_type
            | gimli::DW_TAG_base_type
            | gimli::DW_TAG_array_type
            | gimli::DW_TAG_pointer_type
            | gimli::DW_TAG_subroutine_type
            | gimli::DW_TAG_typedef
            | gimli::DW_TAG_const_type
            | gimli::DW_TAG_volatile_type
            | gimli::DW_TAG_restrict_type
            | gimli::DW_TAG_atomic_type
            | gimli::DW_TAG_reference_type
            | gimli::DW_TAG_rvalue_reference_type
            | gimli::DW_TAG_class_type => {
                let name = match name {
                    Some(name) => Some(keep.string(&name, control)?),
                    None => None,
                };
                keep.insert(
                    &mut facts.types,
                    offset,
                    Type {
                        tag,
                        name,
                        size,
                        members: Vec::new(),
                        named_members: BTreeMap::new(),
                        target: type_of,
                    },
                    control,
                )?;
                if tag == gimli::DW_TAG_subroutine_type {
                    keep.insert(
                        &mut facts.signatures,
                        offset,
                        Signature {
                            parameters: Vec::new(),
                            result: type_of,
                        },
                        control,
                    )?;
                }
                scratch.push(&mut state.owners, (depth, offset, tag), control)?;
            }
            gimli::DW_TAG_member => {
                let location = match entry.attr_value(gimli::DW_AT_data_member_location) {
                    Some(value) => value.udata_value(),
                    None => Some(0),
                };
                // The innermost aggregate owning this member: a variant's
                // members belong to its enum.
                if let (Some(&(_, owner, _)), Some(location), Some(member)) = (
                    state.owners.iter().rev().find(|(_, _, tag)| {
                        matches!(
                            *tag,
                            gimli::DW_TAG_structure_type | gimli::DW_TAG_union_type
                        )
                    }),
                    location,
                    type_of,
                ) && let Some(owner) = facts.types.get_mut(&owner)
                {
                    keep.push(&mut owner.members, (location, member), control)?;
                    if let Some(name) = name {
                        let name = keep.string(&name, control)?;
                        keep.insert(&mut owner.named_members, name, location, control)?;
                    }
                }
            }
            gimli::DW_TAG_variable => {
                let inside_function = state
                    .owners
                    .iter()
                    .any(|(_, _, tag)| *tag == gimli::DW_TAG_subprogram);
                if let (Some(gimli::AttributeValue::Exprloc(expression)), Some(ty)) =
                    (entry.attr_value(gimli::DW_AT_location), type_of)
                {
                    // Function-local statics still use the global policy;
                    // local register/stack expressions use optional locations.
                    facts.add_global(
                        &expression,
                        unit.encoding(),
                        ty,
                        inside_function,
                        keep,
                        control,
                    )?;
                }
            }
            gimli::DW_TAG_subprogram => {
                scratch.push(&mut state.owners, (depth, offset, tag), control)?;
            }
            gimli::DW_TAG_formal_parameter => {
                if let Some(&(_, owner, gimli::DW_TAG_subroutine_type)) = state.owners.last()
                    && let (Some(signature), Some(ty)) = (facts.signatures.get_mut(&owner), type_of)
                {
                    keep.push(&mut signature.parameters, ty, control)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn finish(self) -> TypeFacts {
        self.facts
    }
}

impl TypeFacts {
    /// A named layout member, only when every copy of the type agrees.
    pub(crate) fn member_offset(
        &self,
        name: &str,
        member: &str,
        control: &mut dyn RunControl,
    ) -> Result<Option<u32>> {
        let mut found = None;
        for ty in self.types.values() {
            control.checkpoint(1)?;
            if ty.name.as_deref() != Some(name) {
                continue;
            }
            let Some(offset) = ty
                .named_members
                .get(member)
                .and_then(|at| u32::try_from(*at).ok())
            else {
                return Ok(None);
            };
            if found.is_some_and(|previous| previous != offset) {
                return Ok(None);
            }
            found = Some(offset);
        }
        Ok(found)
    }

    /// Whether a complete word is a named member of a statically typed
    /// object. Arrays retain their element type; pointers are never traversed.
    pub(crate) fn named_field_at(
        &self,
        address: u32,
        name: &str,
        member: &str,
        control: &mut dyn RunControl,
    ) -> Result<bool> {
        fn visit(
            facts: &TypeFacts,
            id: usize,
            at: u64,
            name: &str,
            member: &str,
            depth: usize,
            control: &mut dyn RunControl,
        ) -> Result<bool> {
            control.checkpoint(1)?;
            if depth > 32 {
                return Ok(false);
            }
            let Some((_, ty)) = facts.resolve(id) else {
                return Ok(false);
            };
            if ty.name.as_deref() == Some(name) && ty.named_members.get(member) == Some(&at) {
                return Ok(true);
            }
            if ty.tag == gimli::DW_TAG_array_type
                && let Some(element) = ty.target
                && let Some(size) = facts.size(element).filter(|size| *size != 0)
            {
                return visit(facts, element, at % size, name, member, depth + 1, control);
            }
            for &(offset, child) in &ty.members {
                control.checkpoint(1)?;
                if let Some(at) = at.checked_sub(offset)
                    && facts.size(child).is_some_and(|size| at < size)
                    && visit(facts, child, at, name, member, depth + 1, control)?
                {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        for &(start, bytes, offset, ty) in &self.globals {
            control.checkpoint(1)?;
            if let Some(at) = address.checked_sub(start)
                && bytes
                    .or_else(|| self.size(ty))
                    .is_some_and(|bytes| u64::from(at) + 4 <= bytes)
                && let Some(at) = offset.checked_add(u64::from(at))
                && visit(self, ty, at, name, member, 0, control)?
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Embassy macros erase a task pool to a `TaskPoolHolder` whose size
    /// and alignment come from `TaskPool<F, N>` at compile time. Recovering
    /// that erased layout rests on the executor invariant. The image must
    /// still state the storage's stride, its actual header offset and the
    /// holder's complete extent; an arbitrary byte buffer is not a pool.
    pub(crate) fn task_poll_field(
        &self,
        address: u32,
        poll: &str,
        control: &mut dyn RunControl,
    ) -> Result<bool> {
        if self.named_field_at(address, "TaskHeader", "poll_fn", control)? {
            return Ok(true);
        }
        let Some(storage) = poll
            .strip_prefix("<embassy_executor::raw::")
            .and_then(|name| name.strip_suffix(">::poll"))
        else {
            return Ok(false);
        };
        // LLVM's DWARF names an async future `async_fn_env`, while the
        // linkage name describes the same future as a closure. Compare the
        // two spellings without allocating a copy of every type name.
        fn same_storage(debug: &str, linkage: &str) -> bool {
            match debug.split_once("{async_fn_env#0}") {
                Some((before, after)) => {
                    linkage
                        .strip_prefix(before)
                        .and_then(|rest| rest.strip_prefix("{closure#0}"))
                        == Some(after)
                }
                None => debug == linkage,
            }
        }
        let mut layout = None;
        for ty in self.types.values() {
            control.checkpoint(1)?;
            if !ty
                .name
                .as_deref()
                .is_some_and(|name| same_storage(name, storage))
            {
                continue;
            }
            let (Some(size), Some(&raw)) = (
                ty.size.filter(|size| *size != 0),
                ty.named_members.get("raw"),
            ) else {
                return Ok(false);
            };
            let mut header = false;
            for &(offset, member) in &ty.members {
                control.checkpoint(1)?;
                header |= offset == raw
                    && self.resolve(member).is_some_and(|(_, member)| {
                        member.name.as_deref() == Some("TaskHeader")
                            && member.size.is_some_and(|bytes| {
                                raw.checked_add(bytes).is_some_and(|end| end <= size)
                            })
                    });
            }
            if !header
                || layout
                    .replace((size, raw))
                    .is_some_and(|previous| previous != (size, raw))
            {
                return Ok(false);
            }
        }
        let (Some((stride, raw)), Some(field)) = (
            layout,
            self.member_offset("TaskHeader", "poll_fn", control)?,
        ) else {
            return Ok(false);
        };
        let Some(field) = raw.checked_add(u64::from(field)) else {
            return Ok(false);
        };
        for &(start, bytes, offset, ty) in &self.globals {
            control.checkpoint(1)?;
            let Some((_, record)) = self.resolve(ty) else {
                continue;
            };
            if !record
                .name
                .as_deref()
                .is_some_and(|name| name.starts_with("TaskPoolHolder<"))
                || record.named_members.get("data") != Some(&0)
            {
                continue;
            }
            let Some(size) = record.size else {
                continue;
            };
            // A piece of a holder does not prove the whole pool's extent.
            if offset != 0 || bytes.is_some_and(|bytes| bytes != size) {
                continue;
            }
            let Some(at) = address
                .checked_sub(start)
                .map(u64::from)
                .and_then(|at| at.checked_sub(field))
            else {
                continue;
            };
            if at.is_multiple_of(stride) && at.checked_add(stride).is_some_and(|end| end <= size) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Record a global variable's location: an address, perhaps plus an
    /// offset into a block LLVM merged globals into, perhaps in pieces.
    fn add_global(
        &mut self,
        expression: &gimli::Expression<Reader<'_>>,
        encoding: gimli::Encoding,
        ty: usize,
        inside_function: bool,
        keep: &mut Ledger<'_>,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        let mut scratch = Ledger::new(keep.memory(), control)?;
        let mut operations = (*expression).operations(encoding);
        let mut pending: Option<u32> = None;
        let mut at = 0u64;
        let mut pieces = Vec::new();
        let mut static_location = true;
        while let Some(operation) = crate::dwarf::next_operation(&mut operations, control)? {
            static_location &= matches!(
                operation,
                gimli::Operation::Address { .. }
                    | gimli::Operation::PlusConstant { .. }
                    | gimli::Operation::Piece { .. }
            );
            match operation {
                gimli::Operation::Address { address } => pending = u32::try_from(address).ok(),
                gimli::Operation::PlusConstant { value } => {
                    pending =
                        pending.and_then(|address| address.checked_add(u32::try_from(value).ok()?));
                }
                gimli::Operation::Piece { size_in_bits, .. } => {
                    let bytes = size_in_bits / 8;
                    if let Some(address) = pending.take() {
                        scratch.push(&mut pieces, (address, Some(bytes), at), control)?;
                    }
                    at += bytes;
                }
                _ => {}
            }
        }
        if let Some(address) = pending {
            scratch.push(&mut pieces, (address, None, 0), control)?;
        }
        if inside_function && !static_location {
            return Ok(());
        }
        for (address, bytes, at) in pieces {
            keep.push(&mut self.globals, (address, bytes, at, ty), control)?;
        }
        Ok(())
    }

    /// The type behind typedefs and qualifiers.
    fn resolve(&self, mut offset: usize) -> Option<(usize, &Type)> {
        for _ in 0..16 {
            let ty = self.types.get(&offset)?;
            match (ty.tag, ty.target) {
                (
                    gimli::DW_TAG_typedef
                    | gimli::DW_TAG_const_type
                    | gimli::DW_TAG_volatile_type
                    | gimli::DW_TAG_restrict_type
                    | gimli::DW_TAG_atomic_type,
                    Some(target),
                ) => {
                    offset = target;
                }
                _ => return Some((offset, ty)),
            }
        }
        None
    }

    fn size(&self, offset: usize) -> Option<u64> {
        let (_, ty) = self.resolve(offset)?;
        ty.size.or(match ty.tag {
            // A C pointer or reference may omit its size: an address's.
            gimli::DW_TAG_pointer_type
            | gimli::DW_TAG_reference_type
            | gimli::DW_TAG_rvalue_reference_type => Some(4),
            // An empty tuple or struct may omit its size.
            gimli::DW_TAG_structure_type if ty.members.is_empty() => Some(0),
            _ => None,
        })
    }

    /// The function-pointer fields at byte `offset` of the type at `ty`.
    fn fields_at(&self, ty: usize, offset: u64, depth: u32, out: &mut BTreeSet<Field>) {
        if depth > 32 {
            return;
        }
        let Some((_, record)) = self.resolve(ty) else {
            return;
        };
        if offset == 0
            && record.tag == gimli::DW_TAG_pointer_type
            && let Some((subroutine, pointee)) =
                record.target.and_then(|target| self.resolve(target))
            && pointee.tag == gimli::DW_TAG_subroutine_type
        {
            let foreign = record
                .name
                .as_deref()
                .is_some_and(|name| name.contains("extern"));
            out.insert(Field {
                subroutine,
                foreign,
            });
            return;
        }
        for &(at, member) in &record.members {
            let size = self.size(member).unwrap_or(0);
            if offset >= at && (offset < at + size || (size == 0 && offset == at)) {
                self.fields_at(member, offset - at, depth + 1, out);
            }
        }
    }

    /// The function-pointer field a load from `address` reads, when exactly
    /// one global field there is one.
    fn field(&self, address: u32) -> Option<Field> {
        let mut found = BTreeSet::new();
        for &(start, bytes, at, ty) in &self.globals {
            let Some(bytes) = bytes.or_else(|| self.size(ty)) else {
                continue;
            };
            if address < start || u64::from(address - start) >= bytes {
                continue;
            }
            self.fields_at(ty, at + u64::from(address - start), 0, &mut found);
        }
        (found.len() == 1).then(|| *found.first().expect("one"))
    }

    /// A signature's shape.
    fn shape(&self, signature: &Signature) -> Shape {
        let slot = |ty: usize| -> Slot {
            let name = self.resolve(ty).and_then(|(_, ty)| ty.name.clone());
            (self.size(ty), name)
        };
        let parameters = signature.parameters.iter().map(|&ty| slot(ty)).collect();
        let result = match signature.result {
            Some(result) => slot(result),
            None => (Some(0), Some("()".into())),
        };
        (parameters, result)
    }
}

/// The targets of each unresolved call through a function pointer a static's
/// field holds. Uses the same audited candidate policy as the local resolver.
pub fn function_pointer_resolutions(
    audit: &FunctionPointerAudit<'_, '_>,
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<Resolutions> {
    let analysis = audit.analysis;
    let types = audit.types;
    let mut scratch = Ledger::new(keep.memory(), control)?;
    let mut resolutions = Resolutions::new();
    for facts in analysis.functions.values() {
        if !audit.owns(facts.function.address) {
            continue;
        }
        for transfer in &facts.transfers {
            if transfer.target.is_some()
                || !matches!(transfer.kind, TransferKind::Call | TransferKind::Tail)
            {
                continue;
            }
            let Some(load) = transfer.load else { continue };
            let address = load.address.or_else(|| {
                let base = facts.unresolved_registers.get(&transfer.site)?[load.base as usize]?;
                Some(base.wrapping_add(load.offset as u32))
            });
            let Some(field) = address.and_then(|address| types.field(address)) else {
                continue;
            };
            control.checkpoint(1)?;
            let targets = audit.candidates(&BTreeSet::from([field]), &mut scratch, control)?;
            keep.admit(Resolutions::admission(targets.len()), control)?;
            resolutions.add(transfer.site, Fact::FieldType, targets);
        }
    }
    Ok(resolutions)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(size: Option<u64>, name: Option<&str>) -> Slot {
        (size, name.map(String::from))
    }

    #[test]
    fn only_a_known_difference_excludes_a_function() {
        let unit = || slot(Some(0), Some("()"));
        let field: Shape = (vec![slot(Some(4), Some("u32"))], unit());
        let unknown: Shape = (vec![slot(None, None)], unit());
        let unnamed: Shape = (vec![slot(Some(4), None)], unit());
        let renamed: Shape = (vec![slot(Some(4), Some("i32"))], unit());
        let resized: Shape = (vec![slot(Some(8), None)], unit());
        let arity: Shape = (vec![], unit());
        assert!(shape_matches(&field, &field, true));
        assert!(shape_matches(&unknown, &field, true));
        assert!(shape_matches(&unnamed, &field, true));
        assert!(!shape_matches(&renamed, &field, true));
        assert!(!shape_matches(&resized, &field, true));
        assert!(!shape_matches(&arity, &field, true));
        // A foreign-ABI field compares sizes only: C names `u32` otherwise.
        assert!(shape_matches(&renamed, &field, false));
        assert!(!shape_matches(&resized, &field, false));
        assert!(!shape_matches(&arity, &field, false));
    }

    #[test]
    fn global_expression_rules_preserve_absolute_pieces_and_local_static_admission() {
        let collect = |bytes: &[u8], inside_function| {
            let expression = gimli::Expression(Reader::new(bytes, gimli::RunTimeEndian::Little));
            let encoding = gimli::Encoding {
                address_size: 4,
                format: gimli::Format::Dwarf32,
                version: 4,
            };
            let mut facts = TypeFacts::default();
            facts
                .add_global(
                    &expression,
                    encoding,
                    7,
                    inside_function,
                    &mut crate::tests::ledger(),
                    &mut || Ok(()),
                )
                .unwrap();
            facts.globals
        };
        // Address plus constant, including function-local statics.
        let merged = [0x03, 0, 0x20, 0, 0, 0x23, 8];
        assert_eq!(collect(&merged, false), [(0x2008, None, 0, 7)]);
        assert_eq!(collect(&merged, true), [(0x2008, None, 0, 7)]);
        // Missing pieces advance offsets. A final unbounded address retains
        // the global decoder's original whole-variable offset of zero.
        let pieces = [0x93, 4, 0x03, 0, 0x21, 0, 0, 0x93, 4, 0x03, 0, 0x22, 0, 0];
        assert_eq!(
            collect(&pieces, false),
            [(0x2100, Some(4), 4, 7), (0x2200, None, 0, 7)]
        );
        // Global bit-piece rules intentionally differ from typed locals.
        assert_eq!(
            collect(&[0x03, 0, 0x20, 0, 0, 0x9d, 7, 1], false),
            [(0x2000, Some(0), 0, 7)]
        );
        // An unsupported operation is ignored at global scope but rejects
        // a function-local expression as evidence of a static object.
        let nop = [0x03, 0, 0x20, 0, 0, 0x96];
        assert_eq!(collect(&nop, false), [(0x2000, None, 0, 7)]);
        assert!(collect(&nop, true).is_empty());
    }
}
