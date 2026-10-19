//! Facts an image's DWARF states: the chain of functions inlined at an
//! address, the type a function returns, by its qualified name, where every
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

pub(crate) type Reader<'d> = SliceReader<'d>;
pub(crate) type Unit<'d> = gimli::Unit<Reader<'d>>;
pub(crate) type Entry<'d> = gimli::DebuggingInformationEntry<Reader<'d>>;

/// The debug information of one ELF.
pub struct Dwarf {
    symbolizer: Symbolizer,
    /// A function's entry address and the qualified name of its return type.
    returns: BTreeMap<u32, String>,
    /// Every concrete subprogram entry at this address explicitly agrees
    /// on DW_AT_noreturn=true. Missing or conflicting facts answer false.
    noreturn: BTreeMap<u32, bool>,
    /// Each named subprogram's path, by its offset in `.debug_info`.
    paths: BTreeMap<usize, String>,
    /// Each copy's start and the offset of the subprogram it copies.
    copies: Vec<(u32, usize)>,
}

pub(crate) fn invalid(message: impl Into<String>) -> Error {
    Error::new(ErrorCode::Integrity, message.into())
}

pub(crate) fn dwarf_error(error: gimli::Error) -> Error {
    invalid(format!("DWARF: {error}"))
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

/// Read the debug information of `elf`: its DWARF facts and type facts,
/// admitted in `keep`, which the caller holds while it keeps them.
pub fn read(
    elf: &Rc<[u8]>,
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
    let mut types = TypeWalk::new(&file, keep, control)?;
    let mut units = dwarf.units();
    while let Some(header) = units.next().map_err(dwarf_error)? {
        let unit = dwarf.unit(header).map_err(dwarf_error)?;
        returns.scope.clear();
        types.begin_unit();
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
            copies.entry(&dwarf, &unit, entry, offset, keep, control)?;
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
        }
        types.end_unit(keep, &mut scratch, control)?;
    }
    let types = types.finish(keep, control)?;
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

/// Every subprogram's entry and the qualified name of its return type.
#[derive(Default)]
struct Returns<'d> {
    /// The named namespaces and types the current entry lies in: depth and
    /// offset.
    scope: Vec<(isize, usize)>,
    /// Each named namespace and type: the scope it lies in and its name.
    nodes: BTreeMap<usize, (Option<usize>, &'d [u8])>,
    /// The named types among them.
    types: Vec<usize>,
    /// Each subprogram's entry and its return type's offset.
    returned: Vec<(u32, usize)>,
    noreturn: BTreeMap<u32, bool>,
}

impl<'d> Returns<'d> {
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
            let Some(gimli::AttributeValue::Addr(low)) = entry.attr_value(gimli::DW_AT_low_pc)
            else {
                return Ok(());
            };
            let Ok(low) = u32::try_from(low) else {
                return Ok(());
            };
            let noreturn =
                entry.attr_value(gimli::DW_AT_noreturn) == Some(gimli::AttributeValue::Flag(true));
            let proof = scratch.entry(&mut self.noreturn, low, || noreturn, control)?;
            *proof &= noreturn;
            if let Some(target) = entry
                .attr_value(gimli::DW_AT_type)
                .and_then(|returns| reference(unit, returns))
            {
                scratch.push(&mut self.returned, (low, target), control)?;
            }
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

    fn finish(
        self,
        out: &mut Dwarf,
        keep: &mut Ledger<'_>,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        for (&low, &proof) in &self.noreturn {
            keep.insert(&mut out.noreturn, low, proof, control)?;
        }
        for &(low, target) in &self.returned {
            control.checkpoint(1)?;
            if self.types.binary_search(&target).is_ok() {
                let name = keep.string(&self.qualified(target), control)?;
                keep.insert(&mut out.returns, low, name, control)?;
            }
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
}

impl Copies {
    fn entry<'d>(
        &mut self,
        dwarf: &gimli::Dwarf<Reader<'d>>,
        unit: &Unit<'d>,
        entry: &Entry<'d>,
        offset: usize,
        keep: &mut Ledger<'_>,
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
        let origin = entry
            .attr_value(gimli::DW_AT_abstract_origin)
            .or_else(|| entry.attr_value(gimli::DW_AT_specification))
            .and_then(|value| reference(unit, value))
            .unwrap_or(offset);
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

    /// The qualified name of the type the function entered at `function`
    /// returns, such as `core::task::wake::RawWaker`.
    pub fn return_type(&self, function: u32) -> Option<&str> {
        self.returns.get(&function).map(String::as_str)
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
