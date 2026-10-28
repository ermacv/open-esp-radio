//! Facts an image's DWARF states: the chain of functions inlined at an
//! address, what a function returns ([`Returned`]), where every
//! copy of a function lies, inlined or not, and the types, globals and
//! signatures of [`TypeFacts`].
//!
//! [`read`] reads the return types, the copies and the type facts in one
//! pass over the debug sections borrowed from the ELF's bytes (a compressed
//! one is an error: `oer-analysis-elf` reads none), and admits everything it
//! keeps or builds on the way in the analysis's working memory; what only
//! the pass needs borrows its strings from the sections. Inline chains are
//! addr2line's, over sections shared with the same bytes
//! ([`Symbolizer::shared`]): the caches it grows as it answers are the
//! library's own, which the working memory does not account.
use crate::cha::{TypeFacts, TypeWalk};
use oer_analysis_elf::dwarf::{DebugSections, SliceReader, Symbolizer, gimli};
use oer_analysis_riscv_model::{
    CallBehavior, Callee, Error, ErrorCode, Ledger, Result, RunControl,
};
use std::collections::BTreeMap;
use std::rc::Rc;

pub(crate) mod locations;
use locations::{LocalLocations, LocationWalk};

/// Additional metadata retained by the shared DWARF pass. The callback audit
/// can use local/parameter storage locations without another DWARF walk.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DebugMetadata {
    #[default]
    Standard,
    TypedLocations,
}

pub(crate) type Reader<'d> = SliceReader<'d>;
pub(crate) type Unit<'d> = gimli::Unit<Reader<'d>>;
pub(crate) type Entry<'d> = gimli::DebuggingInformationEntry<Reader<'d>>;

/// The debug information of one ELF.
pub struct Dwarf {
    symbolizer: Symbolizer,
    /// What the function entered at each address returns, where the DWARF
    /// states it.
    returns: BTreeMap<u32, Return>,
    /// Every concrete subprogram entry at this address agrees on
    /// DW_AT_noreturn=true, directly or through its declaration/origin.
    /// Missing or conflicting facts answer false.
    noreturn: BTreeMap<u32, bool>,
    /// Each named subprogram's path, by its offset in `.debug_info`.
    paths: BTreeMap<usize, String>,
    /// Each copy's start and the offset of the subprogram it copies.
    copies: Vec<(u32, usize)>,
}

/// What the DWARF states a function returns.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Returned<'a> {
    /// A type, by its qualified name, such as `core::task::wake::RawWaker`;
    /// empty for an unnamed one, such as a pointer.
    Type(&'a str),
    /// Nothing: the subprogram, or the one it names by origin, lies in a
    /// unit that describes types and names no return type.
    Nothing,
    /// The DWARF does not say: no subprogram enters there, its unit
    /// describes no types (a library built with line tables only, such as
    /// the precompiled `core`), its origin chain is broken, or two
    /// descriptions of the entry disagree.
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Return {
    Type(String),
    Nothing,
    Unknown,
}

pub(crate) fn invalid(message: impl Into<String>) -> Error {
    Error::new(ErrorCode::Integrity, message.into())
}

pub(crate) fn dwarf_error(error: gimli::Error) -> Error {
    invalid(format!("DWARF: {error}"))
}

/// Shared expression decoding and work/error accounting. Global addresses
/// and typed local pieces deliberately retain separate interpretation rules.
pub(crate) fn next_operation<'d>(
    operations: &mut gimli::OperationIter<Reader<'d>>,
    control: &mut dyn RunControl,
) -> Result<Option<gimli::Operation<Reader<'d>>>> {
    let operation = operations.next().map_err(dwarf_error)?;
    if operation.is_some() {
        control.checkpoint(1)?;
    }
    Ok(operation)
}

/// A demangled name without the crate disambiguators v0 symbols carry
/// (`core[1a2b]::task` reads `core::task`).
pub(crate) fn plain(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut depth = 0_u32;
    for c in name.chars() {
        match c {
            '[' => depth += 1,
            ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out
}

/// The bytes of the string an attribute names, borrowed from the sections.
pub(crate) fn bytes<'d>(
    dwarf: &gimli::Dwarf<Reader<'d>>,
    unit: &Unit<'d>,
    value: gimli::AttributeValue<Reader<'d>>,
) -> Option<&'d [u8]> {
    dwarf.attr_string(unit, value).ok().map(|name| name.slice())
}

/// The string an attribute names, lossily decoded.
pub(crate) fn text<'d>(
    dwarf: &gimli::Dwarf<Reader<'d>>,
    unit: &Unit<'d>,
    value: gimli::AttributeValue<Reader<'d>>,
) -> Option<String> {
    bytes(dwarf, unit, value).map(|name| String::from_utf8_lossy(name).into_owned())
}

/// The offset in `.debug_info` a reference attribute names.
pub(crate) fn reference(
    unit: &Unit<'_>,
    value: gimli::AttributeValue<Reader<'_>>,
) -> Option<usize> {
    match value {
        gimli::AttributeValue::UnitRef(offset) => offset
            .to_debug_info_offset(&unit.header)
            .map(|offset| offset.0),
        gimli::AttributeValue::DebugInfoRef(offset) => Some(offset.0),
        _ => None,
    }
}

/// Select a source DIE identity, preferring a present abstract_origin to a
/// specification. This does not inherit any attribute: each consumer keeps
/// its own rules for following this reference.
pub(crate) fn origin_reference(unit: &Unit<'_>, entry: &Entry<'_>) -> Option<usize> {
    entry
        .attr_value(gimli::DW_AT_abstract_origin)
        .or_else(|| entry.attr_value(gimli::DW_AT_specification))
        .and_then(|value| reference(unit, value))
}

/// Read the debug information of `elf`: its DWARF facts and type facts,
/// admitted in `keep`, which the caller holds while it keeps them.
/// `TypedLocations` additionally prepares PC-specific local/parameter
/// locations in TypeFacts for the common callback audit. `Standard`
/// skips that collector and allocates no local-location records or scopes.
pub fn read(
    elf: &Rc<[u8]>,
    metadata: DebugMetadata,
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<(Dwarf, TypeFacts)> {
    let memory = keep.memory();
    let file = oer_analysis_elf::Elf::parse(elf).map_err(|_| invalid("invalid ELF"))?;
    let sections = DebugSections::load(&file).map_err(|error| invalid(error.to_string()))?;
    let dwarf = sections
        .dwarf()
        .map_err(|error| invalid(error.to_string()))?;
    let mut scratch = Ledger::new(memory, control)?;
    let mut returns = Returns::default();
    let mut copies = Copies::default();
    let mut types = TypeWalk::default();
    let mut locations = (metadata == DebugMetadata::TypedLocations).then(LocalLocations::default);
    let mut units = dwarf.units();
    while let Some(header) = units.next().map_err(dwarf_error)? {
        let unit = dwarf.unit(header).map_err(dwarf_error)?;
        returns.begin_unit(&mut scratch, control)?;
        types.begin_unit();
        let mut location_walk = locations
            .as_mut()
            .map(|facts| LocationWalk::new(facts, memory, control))
            .transpose()?;
        let mut entries = unit.entries();
        while let Some(entry) = entries.next_dfs().map_err(dwarf_error)? {
            control.checkpoint(1)?;
            let Some(offset) = entry
                .offset()
                .to_debug_info_offset(&unit.header)
                .map(|offset| offset.0)
            else {
                continue;
            };
            let depth = entry.depth();
            returns.entry(&dwarf, &unit, entry, depth, offset, &mut scratch, control)?;
            copies.entry(&dwarf, &unit, entry, offset, keep, &mut scratch, control)?;
            types.entry(
                &dwarf,
                &unit,
                entry,
                depth,
                offset,
                keep,
                &mut scratch,
                control,
            )?;
            if let Some(locations) = &mut location_walk {
                locations.entry(&dwarf, &unit, entry, offset, keep, control)?;
            }
        }
    }
    copies.resolve_origins(control)?;
    let mut types = types.finish();
    types.locations = locations;
    types.source = Some(elf.clone());
    let symbolizer = Symbolizer::shared(elf.clone()).map_err(|error| invalid(error.to_string()))?;
    let mut out = Dwarf {
        symbolizer,
        returns: BTreeMap::new(),
        noreturn: BTreeMap::new(),
        paths: copies.paths,
        copies: copies.copies,
    };
    returns.finish(&mut out, keep, control)?;
    drop(scratch);
    Ok((out, types))
}

/// Where a subprogram entry's description continues.
#[derive(Clone, Copy)]
enum Link {
    None,
    To(usize),
    /// An origin attribute that names no entry.
    Invalid,
}

/// A subprogram entry, abstract or concrete: its return type's offset, its
/// origin and its unit.
struct Subprogram {
    returns: Option<usize>,
    noreturn: Option<bool>,
    origin: Link,
    unit: usize,
}

/// Every concrete subprogram's entry and what it returns, its own return
/// type or, through `DW_AT_abstract_origin` and `DW_AT_specification`, its
/// origin's.
#[derive(Default)]
struct Returns<'d> {
    /// The named namespaces and types the current entry lies in: depth and
    /// offset.
    scope: Vec<(isize, usize)>,
    /// Each named namespace and type: the scope it lies in and its name.
    nodes: BTreeMap<usize, (Option<usize>, &'d [u8])>,
    /// The named types among them.
    types: Vec<usize>,
    /// Every subprogram entry, by its offset in `.debug_info`.
    subprograms: BTreeMap<usize, Subprogram>,
    /// Each concrete subprogram's entry address and offset.
    concrete: Vec<(u32, usize)>,
    /// Whether each unit read so far describes any type.
    typed_units: Vec<bool>,
}

impl<'d> Returns<'d> {
    fn begin_unit(&mut self, scratch: &mut Ledger<'_>, control: &mut dyn RunControl) -> Result<()> {
        self.scope.clear();
        scratch.push(&mut self.typed_units, false, control)
    }

    #[allow(clippy::too_many_arguments)]
    fn entry(
        &mut self,
        dwarf: &gimli::Dwarf<Reader<'d>>,
        unit: &Unit<'d>,
        entry: &Entry<'d>,
        depth: isize,
        offset: usize,
        scratch: &mut Ledger<'_>,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        self.scope.retain(|(at, _)| *at < depth);
        let tag = entry.tag();
        let unit_index = self.typed_units.len().saturating_sub(1);
        if matches!(
            tag,
            gimli::DW_TAG_base_type
                | gimli::DW_TAG_structure_type
                | gimli::DW_TAG_union_type
                | gimli::DW_TAG_enumeration_type
                | gimli::DW_TAG_pointer_type
                | gimli::DW_TAG_array_type
                | gimli::DW_TAG_subroutine_type
                | gimli::DW_TAG_typedef
        ) && let Some(typed) = self.typed_units.last_mut()
        {
            *typed = true;
        }
        let named = matches!(
            tag,
            gimli::DW_TAG_namespace
                | gimli::DW_TAG_structure_type
                | gimli::DW_TAG_union_type
                | gimli::DW_TAG_enumeration_type
                | gimli::DW_TAG_base_type
                | gimli::DW_TAG_typedef
        );
        if named {
            if let Some(name) = entry
                .attr_value(gimli::DW_AT_name)
                .and_then(|value| bytes(dwarf, unit, value))
            {
                let parent = self.scope.last().map(|&(_, parent)| parent);
                scratch.insert(&mut self.nodes, offset, (parent, name), control)?;
                if tag != gimli::DW_TAG_namespace {
                    scratch.push(&mut self.types, offset, control)?;
                }
                // Nested types live in the type's scope.
                scratch.push(&mut self.scope, (depth, offset), control)?;
            }
        } else if tag == gimli::DW_TAG_subprogram {
            let returns = entry
                .attr_value(gimli::DW_AT_type)
                .and_then(|returns| reference(unit, returns));
            // As `origin_reference`: a present abstract origin wins, and one
            // that names no entry does not fall through.
            let origin = match entry
                .attr_value(gimli::DW_AT_abstract_origin)
                .or_else(|| entry.attr_value(gimli::DW_AT_specification))
            {
                None => Link::None,
                Some(value) => reference(unit, value).map_or(Link::Invalid, Link::To),
            };
            let subprogram = Subprogram {
                returns,
                noreturn: entry
                    .attr_value(gimli::DW_AT_noreturn)
                    .map(|value| value == gimli::AttributeValue::Flag(true)),
                origin,
                unit: unit_index,
            };
            scratch.insert(&mut self.subprograms, offset, subprogram, control)?;
            let Some(gimli::AttributeValue::Addr(low)) = entry.attr_value(gimli::DW_AT_low_pc)
            else {
                return Ok(());
            };
            let Ok(low) = u32::try_from(low) else {
                return Ok(());
            };
            scratch.push(&mut self.concrete, (low, offset), control)?;
        }
        Ok(())
    }

    /// The qualified name of the type at `offset`: its scope's names and
    /// its own, joined by `::`.
    fn qualified(&self, offset: usize) -> String {
        let mut parts = Vec::new();
        let mut at = Some(offset);
        while let Some(node) = at.and_then(|offset| self.nodes.get(&offset)) {
            parts.push(String::from_utf8_lossy(node.1));
            at = node.0;
        }
        parts.reverse();
        parts.join("::")
    }

    /// What the subprogram entry at `offset` returns: the first return type
    /// along its origin chain, or nothing where the chain ends in a typed
    /// unit without one.
    fn resolve(
        &self,
        offset: usize,
        keep: &mut Ledger<'_>,
        control: &mut dyn RunControl,
    ) -> Result<Return> {
        let mut at = offset;
        // Stop at a reference to no subprogram, a broken link or a cycle.
        for _ in 0..=self.subprograms.len() {
            control.checkpoint(1)?;
            let Some(subprogram) = self.subprograms.get(&at) else {
                return Ok(Return::Unknown);
            };
            if let Some(returns) = subprogram.returns {
                let name = if self.types.binary_search(&returns).is_ok() {
                    keep.string(&self.qualified(returns), control)?
                } else {
                    String::new()
                };
                return Ok(Return::Type(name));
            }
            match subprogram.origin {
                Link::To(next) => at = next,
                Link::Invalid => return Ok(Return::Unknown),
                Link::None => {
                    return Ok(if self.typed_units.get(subprogram.unit) == Some(&true) {
                        Return::Nothing
                    } else {
                        Return::Unknown
                    });
                }
            }
        }
        Ok(Return::Unknown)
    }

    /// DWARF 5 sections 2.13.2 and 3.3.8 allow declaration/origin facts
    /// to be omitted on a concrete subprogram. A present attribute wins;
    /// false or malformed flags never fall through to an inherited true.
    fn resolve_noreturn(&self, offset: usize, control: &mut dyn RunControl) -> Result<bool> {
        let mut at = offset;
        for _ in 0..=self.subprograms.len() {
            control.checkpoint(1)?;
            let Some(subprogram) = self.subprograms.get(&at) else {
                return Ok(false);
            };
            if let Some(proof) = subprogram.noreturn {
                return Ok(proof);
            }
            match subprogram.origin {
                Link::To(next) => at = next,
                Link::None | Link::Invalid => return Ok(false),
            }
        }
        Ok(false)
    }

    fn finish(
        self,
        out: &mut Dwarf,
        keep: &mut Ledger<'_>,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        for &(low, offset) in &self.concrete {
            let proof = self.resolve_noreturn(offset, control)?;
            let proof = proof && out.noreturn.get(&low).copied().unwrap_or(true);
            keep.insert(&mut out.noreturn, low, proof, control)?;
            let returned = self.resolve(offset, keep, control)?;
            // Descriptions of one entry that disagree say nothing.
            let returned = match out.returns.get(&low) {
                Some(earlier) if *earlier != returned => Return::Unknown,
                _ => returned,
            };
            keep.insert(&mut out.returns, low, returned, control)?;
        }
        Ok(())
    }
}

impl CallBehavior for Dwarf {
    fn noreturn(&self, callee: Callee<'_>) -> bool {
        matches!(callee, Callee::Address(address) if self.noreturn.get(&address) == Some(&true))
    }
}

/// Every subprogram's and inlined subroutine's copy and the paths of named
/// subprograms, for [`Dwarf::instances`].
#[derive(Default)]
struct Copies {
    paths: BTreeMap<usize, String>,
    copies: Vec<(u32, usize)>,
    /// Abstract subprograms can name their declaration through specification.
    origins: BTreeMap<usize, usize>,
}

impl Copies {
    #[allow(clippy::too_many_arguments)]
    fn entry<'d>(
        &mut self,
        dwarf: &gimli::Dwarf<Reader<'d>>,
        unit: &Unit<'d>,
        entry: &Entry<'d>,
        offset: usize,
        keep: &mut Ledger<'_>,
        scratch: &mut Ledger<'_>,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        let tag = entry.tag();
        if tag != gimli::DW_TAG_subprogram && tag != gimli::DW_TAG_inlined_subroutine {
            return Ok(());
        }
        if let Some(value) = entry.attr_value(gimli::DW_AT_linkage_name)
            && let Some(linkage) = text(dwarf, unit, value)
        {
            let path = keep.string(&plain(&oer_analysis_elf::demangle(&linkage)), control)?;
            keep.insert(&mut self.paths, offset, path, control)?;
        }
        // A copy names its subprogram by origin, or is one itself.
        let origin = if let Some(origin) = origin_reference(unit, entry) {
            scratch.insert(&mut self.origins, offset, origin, control)?;
            origin
        } else {
            offset
        };
        let start = match entry.attr_value(gimli::DW_AT_low_pc) {
            Some(gimli::AttributeValue::Addr(low)) => Some(low),
            _ => {
                let mut ranges = dwarf.die_ranges(unit, entry).map_err(dwarf_error)?;
                ranges.next().map_err(dwarf_error)?.map(|range| range.begin)
            }
        };
        if let Some(start) = start
            && start != 0
            && let Ok(start) = u32::try_from(start)
        {
            keep.push(&mut self.copies, (start, origin), control)?;
        }
        Ok(())
    }

    fn resolve_origins(&mut self, control: &mut dyn RunControl) -> Result<()> {
        for (_, origin) in &mut self.copies {
            // Stop at a named subprogram, a missing reference or a cycle.
            for _ in 0..=self.origins.len() {
                control.checkpoint(1)?;
                if self.paths.contains_key(origin) {
                    break;
                }
                let Some(next) = self.origins.get(origin) else {
                    break;
                };
                *origin = *next;
            }
        }
        Ok(())
    }
}

impl Dwarf {
    /// The functions inlined at `address`, innermost first, then the
    /// function they are inlined in, by their demangled paths; empty where
    /// the DWARF covers no code.
    pub fn inline_chain(&self, address: u32) -> Result<Vec<String>> {
        Ok(self
            .symbolizer
            .frames(u64::from(address))
            .map_err(|error| invalid(error.to_string()))?
            .into_iter()
            .filter_map(|frame| frame.function)
            .map(|name| plain(&name))
            .collect())
    }

    /// What the function entered at `function` returns.
    pub fn returned(&self, function: u32) -> Returned<'_> {
        match self.returns.get(&function) {
            Some(Return::Type(name)) => Returned::Type(name),
            Some(Return::Nothing) => Returned::Nothing,
            Some(Return::Unknown) | None => Returned::Unknown,
        }
    }

    /// Where every copy of each function named in `functions` (by its
    /// demangled path, as [`Dwarf::inline_chain`] names functions) lies: the
    /// entry of each out-of-line copy and the first address of each inlined
    /// one.
    pub fn instances(&self, functions: &[&str]) -> BTreeMap<String, Vec<u32>> {
        let mut found: BTreeMap<String, Vec<u32>> = functions
            .iter()
            .map(|function| ((*function).to_owned(), Vec::new()))
            .collect();
        for &(start, origin) in &self.copies {
            if let Some(name) = self.paths.get(&origin)
                && let Some(starts) = found.get_mut(name)
            {
                starts.push(start);
            }
        }
        for starts in found.values_mut() {
            starts.sort_unstable();
            starts.dedup();
        }
        found
    }
}

#[cfg(test)]
mod tests;
