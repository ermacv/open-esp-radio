//! Class-hierarchy analysis from the image's types: a call through a function
//! pointer loaded from a static reaches every function whose address the
//! image takes and whose signature can be the pointer's type.
//!
//! The pointer's type is the type of the static's field the load reads, from
//! the DWARF: the global variable at the load's address (merged and split
//! globals included) and its type's members at the load's offset, `enum` and
//! `Option` variants included, down to a pointer to a subroutine type. The
//! candidates are matched per parameter and result, never by a signature's
//! spelling: every function a relocation names other than as a call or jump
//! target (an address taken) with the subroutine type's parameter count whose
//! parameters and result no known fact contradicts. For a Rust-ABI pointer a
//! fact is a byte size or a type name (behind typedefs and qualifiers); for a
//! pointer of a foreign ABI (`extern "C"`) only a byte size, since C and Rust
//! name one type differently, and a taken function the DWARF does not
//! describe is a candidate too. An unknown size or name excludes nothing.
//!
//! A Rust-ABI pointer holds only Rust functions (another ABI's needs `unsafe`,
//! as does a transmute, which is not seen), and the soundness of excluding by
//! name rests on rustc naming one type identically in every unit and crate:
//! the debuginfo name is computed from the type's path and generic arguments
//! by one rule. A test holds the toolchain to it across crates and codegen
//! units. A site whose load address or field is unknown stays unresolved.
use crate::dwarf::{Entry, Reader, Unit, reference, text};
use crate::{Analysis, Fact, Resolutions, TransferKind};
use oer_tool_analysis_elf::dwarf::gimli;
use oer_tool_analysis_riscv_model::{Ledger, Result, RunControl};
use std::collections::{BTreeMap, BTreeSet};

/// One type's name and layout.
#[derive(Debug)]
struct Type {
    tag: gimli::DwTag,
    name: Option<String>,
    size: Option<u64>,
    /// Members (variants' members included) by offset, and their types.
    members: Vec<(u64, usize)>,
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
struct Field {
    subroutine: usize,
    foreign: bool,
}

/// What the image's DWARF states about types, globals and functions.
#[derive(Debug, Default)]
pub struct TypeFacts {
    types: BTreeMap<usize, Type>,
    /// Pieces of global variables: address, bytes (`None` for the whole
    /// variable), offset in the variable, the variable's type.
    globals: Vec<(u32, Option<u64>, u64, usize)>,
    /// Signatures of subprograms and subroutine types, by DIE.
    signatures: BTreeMap<usize, Signature>,
    /// Each function's entry and the DIE that states its signature.
    functions: BTreeMap<u32, usize>,
    /// Subprograms by their symbol: the linkage name, or the name of one
    /// without (`#[no_mangle]`). A function merged into another keeps only
    /// such an entry, without an address.
    named: BTreeMap<String, Vec<usize>>,
    /// Addresses that several functions share, with their symbols: function
    /// merging aliases a merged function to the one it keeps, with that
    /// function's size. A symbol of size zero (a linker script's or an
    /// assembly label) names no function of its own.
    merged: BTreeMap<u32, Vec<String>>,
    /// Subprograms of units that state no types at all, such as the
    /// precompiled `core` built with limited debuginfo: their parameters are
    /// unknown, not absent.
    untyped: BTreeSet<usize>,
    /// The ELF function symbols of each address, those of size zero left out.
    symbols: BTreeMap<u32, Vec<String>>,
    /// The parameter counts of each trait method, `(trait, method)`, that the
    /// typed implementations in the image state. A trait fixes its methods'
    /// parameter count whatever the implementing type.
    trait_arities: BTreeMap<(String, String), BTreeSet<usize>>,
}

/// The trait and method of a trait implementation's symbol,
/// `<Type as path::Trait>::method`, by its demangled name.
fn trait_method(symbol: &str) -> Option<(String, String)> {
    let demangled = oer_tool_analysis_elf::demangle(symbol);
    let name = crate::dwarf::plain(&demangled);
    // A legacy symbol ends in its hash.
    let name = match name.rsplit_once("::h") {
        Some((head, hash))
            if hash.len() == 16 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()) =>
        {
            head
        }
        _ => name.as_str(),
    };
    let inner = name.strip_prefix('<')?;
    let mut depth = 0_i32;
    let mut close = None;
    let mut split = None;
    for (at, character) in inner.char_indices() {
        match character {
            '<' | '(' | '[' => depth += 1,
            '>' if depth == 0 => {
                close = Some(at);
                break;
            }
            '>' | ')' | ']' => depth -= 1,
            ' ' if depth == 0 && inner[at..].starts_with(" as ") => split = Some(at),
            _ => {}
        }
    }
    let (close, split) = (close?, split?);
    let method = inner[close + 1..].strip_prefix("::")?;
    if method.is_empty() || method.contains(':') {
        return None;
    }
    let trait_path: String = inner[split + 4..close]
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    Some((trait_path, method.to_owned()))
}

/// One unit's walk state of [`TypeWalk`].
#[derive(Default)]
struct UnitTypes {
    /// The type, subprogram or subroutine type each depth belongs to.
    owners: Vec<(isize, usize, gimli::DwTag)>,
    /// Whether the unit states any type: a unit with limited debuginfo
    /// names its functions without their parameters or result.
    typed: bool,
    subprograms: Vec<usize>,
    symbols: BTreeMap<usize, String>,
}

/// [`TypeFacts`] under construction by the one pass of
/// [`crate::dwarf::read`]: what each entry states, then, at the end, what
/// the whole image's entries state together.
pub(crate) struct TypeWalk {
    facts: TypeFacts,
    unit: UnitTypes,
    /// Typed subprograms by their symbol, for the trait methods' arities.
    typed_symbols: Vec<(usize, String)>,
    /// A function's entry and the DIEs that may state its signature: its
    /// own, then its abstract origin or specification.
    entries: Vec<(u32, usize, Option<usize>)>,
}

impl TypeWalk {
    /// The walk over the DWARF of `file`, with its function symbols.
    pub(crate) fn new(
        file: &oer_tool_analysis_elf::Elf<'_>,
        keep: &mut Ledger<'_>,
        control: &mut dyn RunControl,
    ) -> Result<Self> {
        let mut facts = TypeFacts::default();
        let mut sized: BTreeMap<u32, Vec<String>> = BTreeMap::new();
        for symbol in file.symbols() {
            control.checkpoint(1)?;
            if symbol.kind == oer_tool_analysis_elf::SymbolKind::Text
                && symbol.size > 0
                && let Ok(address) = u32::try_from(symbol.address)
            {
                let name = keep.string(symbol.name, control)?;
                let names = keep.entry(&mut sized, address & !1, Vec::new, control)?;
                keep.push(names, name, control)?;
            }
        }
        for (&address, symbols) in &sized {
            if symbols.len() > 1 {
                let mut copy = Vec::new();
                for symbol in symbols {
                    let symbol = keep.string(symbol, control)?;
                    keep.push(&mut copy, symbol, control)?;
                }
                keep.insert(&mut facts.merged, address, copy, control)?;
            }
        }
        facts.symbols = sized;
        Ok(Self {
            facts,
            unit: UnitTypes::default(),
            typed_symbols: Vec::new(),
            entries: Vec::new(),
        })
    }

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
                state.typed = true;
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
                }
            }
            gimli::DW_TAG_variable => {
                let inside_function = state
                    .owners
                    .iter()
                    .any(|(_, _, tag)| *tag == gimli::DW_TAG_subprogram);
                if let (false, Some(gimli::AttributeValue::Exprloc(expression)), Some(ty)) = (
                    inside_function,
                    entry.attr_value(gimli::DW_AT_location),
                    type_of,
                ) {
                    facts.add_global(&expression, unit.encoding(), ty, keep, control)?;
                }
            }
            gimli::DW_TAG_subprogram => {
                scratch.push(&mut state.owners, (depth, offset, tag), control)?;
                scratch.push(&mut state.subprograms, offset, control)?;
                let symbol = entry
                    .attr_value(gimli::DW_AT_linkage_name)
                    .or_else(|| entry.attr_value(gimli::DW_AT_MIPS_linkage_name))
                    .and_then(|value| text(dwarf, unit, value))
                    .or_else(|| name.clone());
                if let Some(symbol) = symbol {
                    let own = scratch.string(&symbol, control)?;
                    scratch.insert(&mut state.symbols, offset, own, control)?;
                    let key = keep.string(&symbol, control)?;
                    let offsets = keep.entry(&mut facts.named, key, Vec::new, control)?;
                    keep.push(offsets, offset, control)?;
                }
                keep.insert(
                    &mut facts.signatures,
                    offset,
                    Signature {
                        parameters: Vec::new(),
                        result: type_of,
                    },
                    control,
                )?;
                let low = match entry.attr_value(gimli::DW_AT_low_pc) {
                    Some(gimli::AttributeValue::Addr(low)) => u32::try_from(low).ok(),
                    _ => None,
                };
                let origin = entry
                    .attr_value(gimli::DW_AT_abstract_origin)
                    .or_else(|| entry.attr_value(gimli::DW_AT_specification))
                    .and_then(|value| reference(unit, value));
                if let Some(low) = low {
                    scratch.push(&mut self.entries, (low, offset, origin), control)?;
                }
            }
            gimli::DW_TAG_formal_parameter => {
                state.typed = true;
                if let Some(&(_, owner, gimli::DW_TAG_subprogram | gimli::DW_TAG_subroutine_type)) =
                    state.owners.last()
                    && let (Some(signature), Some(ty)) = (facts.signatures.get_mut(&owner), type_of)
                {
                    keep.push(&mut signature.parameters, ty, control)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// The end of one unit: its subprograms are typed or not.
    pub(crate) fn end_unit(
        &mut self,
        keep: &mut Ledger<'_>,
        scratch: &mut Ledger<'_>,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        let mut state = std::mem::take(&mut self.unit);
        if state.typed {
            for offset in state.subprograms {
                if let Some(symbol) = state.symbols.remove(&offset) {
                    scratch.push(&mut self.typed_symbols, (offset, symbol), control)?;
                }
            }
        } else {
            for offset in state.subprograms {
                keep.add(&mut self.facts.untyped, offset, control)?;
            }
        }
        Ok(())
    }

    /// The type facts of the whole image.
    pub(crate) fn finish(
        self,
        keep: &mut Ledger<'_>,
        control: &mut dyn RunControl,
    ) -> Result<TypeFacts> {
        let mut facts = self.facts;
        for (offset, symbol) in self.typed_symbols {
            control.checkpoint(1)?;
            if let (Some(method), Some(signature)) =
                (trait_method(&symbol), facts.signatures.get(&offset))
                && !signature.parameters.is_empty()
            {
                keep.admit(method.0.len() + method.1.len(), control)?;
                let arities =
                    keep.entry(&mut facts.trait_arities, method, BTreeSet::new, control)?;
                keep.add(arities, signature.parameters.len(), control)?;
            }
        }
        // A concrete instance lists its parameters through its abstract
        // origin when it lists none with types of its own.
        for (low, own, origin) in self.entries {
            control.checkpoint(1)?;
            let die = match origin {
                Some(origin)
                    if facts
                        .signatures
                        .get(&own)
                        .is_none_or(|own| own.parameters.is_empty() && own.result.is_none()) =>
                {
                    origin
                }
                _ => own,
            };
            if !facts.functions.contains_key(&low) {
                keep.insert(&mut facts.functions, low, die, control)?;
            }
        }
        Ok(facts)
    }
}

impl TypeFacts {
    /// Record a global variable's location: an address, perhaps plus an
    /// offset into a block LLVM merged globals into, perhaps in pieces.
    fn add_global(
        &mut self,
        expression: &gimli::Expression<Reader<'_>>,
        encoding: gimli::Encoding,
        ty: usize,
        keep: &mut Ledger<'_>,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        let mut operations = (*expression).operations(encoding);
        let mut pending: Option<u32> = None;
        let mut at = 0u64;
        let mut pieces = Vec::new();
        while let Ok(Some(operation)) = operations.next() {
            match operation {
                gimli::Operation::Address { address } => pending = u32::try_from(address).ok(),
                gimli::Operation::PlusConstant { value } => {
                    pending =
                        pending.and_then(|address| address.checked_add(u32::try_from(value).ok()?));
                }
                gimli::Operation::Piece { size_in_bits, .. } => {
                    let bytes = size_in_bits / 8;
                    if let Some(address) = pending.take() {
                        pieces.push((address, Some(bytes), at));
                    }
                    at += bytes;
                }
                _ => {}
            }
        }
        if let Some(address) = pending {
            pieces.push((address, None, 0));
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

    /// The shape of the function entered at `entry`; `None` without a DWARF
    /// signature.
    fn function_shape(&self, entry: u32) -> Option<Shape> {
        let die = self.functions.get(&entry)?;
        Some(self.shape(self.signatures.get(die)?))
    }

    /// Whether the function at `entry` can be the field's: one of its shapes
    /// matches. Function merging puts several functions, perhaps of other
    /// types, at one address, so an address several functions share has the
    /// shape of each one's subprograms too, and a function without any is a
    /// candidate whatever the field.
    fn can_be(&self, entry: u32, wanted: &Shape, field: Field) -> bool {
        let names = !field.foreign;
        if self
            .functions
            .get(&entry)
            .is_some_and(|die| self.untyped.contains(die))
        {
            return self.untyped_can_be(entry, wanted.0.len());
        }
        let own = self.function_shape(entry);
        let Some(symbols) = self.merged.get(&entry) else {
            return match own {
                Some(shape) => shape_matches(&shape, wanted, names),
                None => field.foreign,
            };
        };
        if own.is_some_and(|shape| shape_matches(&shape, wanted, names)) {
            return true;
        }
        symbols.iter().any(|symbol| match self.named.get(symbol) {
            None => true,
            Some(dies) => dies.iter().any(|die| {
                self.untyped.contains(die)
                    || self.signatures.get(die).is_some_and(|signature| {
                        shape_matches(&self.shape(signature), wanted, names)
                    })
            }),
        })
    }

    /// Whether a function of a unit without types, whose parameters the DWARF
    /// leaves unknown, can take `arity` parameters: only a trait method's
    /// count, which the trait fixes and the image's typed implementations of
    /// the same method state, excludes it.
    fn untyped_can_be(&self, entry: u32, arity: usize) -> bool {
        let Some(symbols) = self.symbols.get(&entry) else {
            return true;
        };
        symbols.iter().any(|symbol| {
            match trait_method(symbol).and_then(|method| self.trait_arities.get(&method)) {
                Some(arities) if arities.len() == 1 => arities.contains(&arity),
                _ => true,
            }
        })
    }

    /// The candidates of a call through the field: every taken function
    /// whose shape nothing known contradicts (another parameter count, or a
    /// parameter or result of another known size or name) and, for a foreign
    /// ABI, every taken function the DWARF does not describe.
    fn candidates(&self, field: Field, taken: &BTreeSet<u32>) -> Option<BTreeSet<u32>> {
        let wanted = self.shape(self.signatures.get(&field.subroutine)?);
        Some(
            taken
                .iter()
                .copied()
                .filter(|&function| self.can_be(function, &wanted, field))
                .collect(),
        )
    }
}

/// The targets of each unresolved call through a function pointer a static's
/// field holds: the candidates among the functions `taken` lists.
pub fn function_pointer_resolutions(
    analysis: &Analysis,
    types: &TypeFacts,
    taken: &BTreeSet<u32>,
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<Resolutions> {
    let mut scratch = Ledger::new(keep.memory(), control)?;
    let mut functions = BTreeSet::new();
    for &address in taken {
        if analysis.functions.contains_key(&address) {
            scratch.add(&mut functions, address, control)?;
        }
    }
    let taken = functions;
    let mut resolutions = Resolutions::new();
    for facts in analysis.functions.values() {
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
            // The candidates are a subset of the taken functions.
            let _candidates = keep
                .memory()
                .reserve((taken.len() * size_of::<u32>()) as u64, control.position())?;
            if let Some(targets) = types.candidates(field, &taken) {
                keep.admit(Resolutions::admission(targets.len()), control)?;
                resolutions.add(transfer.site, Fact::FieldType, targets);
            }
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
    fn a_trait_implementation_names_its_trait_and_method() {
        let method = |trait_path: &str, method: &str| Some((trait_path.into(), method.into()));
        assert_eq!(
            trait_method("<u32 as core::fmt::Display>::fmt"),
            method("core::fmt::Display", "fmt")
        );
        assert_eq!(
            trait_method("<alloc::vec::Vec<u8> as core::ops::index::Index<usize>>::index"),
            method("core::ops::index::Index<usize>", "index")
        );
        assert_eq!(
            trait_method(
                "<core::fmt::builders::PadAdapter as core::fmt::Write>::write_str::h0123456789abcdef"
            ),
            method("core::fmt::Write", "write_str")
        );
        // An inherent method or a free function names no trait.
        assert_eq!(trait_method("<core::fmt::Formatter>::pad"), None);
        assert_eq!(trait_method("core::fmt::write"), None);
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
}
