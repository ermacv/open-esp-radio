//! Two builds of the same firmware compared by their MMIO behavior, function
//! by function: the register-access observations inside the given windows,
//! without what changes between builds of unchanged code: symbol hashes,
//! code layout, branch offsets, the addresses of loaded data, and a stored
//! address of a sized symbol that moved: a stored value matches an
//! identical access first, then the same symbol and offset, then the same
//! value. Any other stored image address compares by value.
use crate::in_process::Executable;
use crate::*;
use oer_analysis_elf::classify as elf;
use std::collections::BTreeMap;

/// One function's MMIO accesses in one build, every instance of its name
/// together.
#[derive(Default)]
struct Accesses {
    analyzed: bool,
    blocked: bool,
    /// Some analyzed instance is incomplete.
    incomplete: bool,
    accesses: Vec<MmioAccess>,
    unresolved: u64,
}

impl Accesses {
    fn state(&self) -> MmioFunctionState {
        match (self.blocked, self.analyzed) {
            (true, _) => MmioFunctionState::Blocked,
            (false, true) => MmioFunctionState::Analyzed,
            (false, false) => MmioFunctionState::Absent,
        }
    }
}

/// The name a function keeps across builds: demangled, without crate
/// disambiguators or legacy hashes.
fn stable_name(name: Option<&[u8]>) -> String {
    let name = String::from_utf8_lossy(name.unwrap_or_default());
    format!("{:#}", rustc_demangle::demangle(&name))
}

/// Whether the `width` bytes at `address` overlap a window, as
/// `register-accesses` selects them.
fn in_ranges(address: u32, width: u8, ranges: &[ImageRegion]) -> bool {
    ranges.iter().any(|r| {
        u64::from(address) < r.end().unwrap_or(0)
            && u64::from(r.start) < u64::from(address) + u64::from(width)
    })
}

fn operation(fact: &FunctionRecord) -> Option<(MmioOperation, Option<u8>)> {
    match fact {
        FunctionRecord::MemoryAccess { access, width, .. } => Some((
            match access {
                MemoryKind::Load => MmioOperation::Load,
                MemoryKind::Store => MmioOperation::Store,
                MemoryKind::LoadReserved => MmioOperation::LoadReserved,
                MemoryKind::StoreConditional => MmioOperation::StoreConditional,
                MemoryKind::Atomic => MmioOperation::Atomic,
                MemoryKind::FloatLoad => MmioOperation::FloatLoad,
                MemoryKind::FloatStore => MmioOperation::FloatStore,
            },
            Some(*width),
        )),
        FunctionRecord::Expression { .. } => Some((MmioOperation::ReadSelection, None)),
        _ => None,
    }
}

/// The MMIO access of one observation, or `None` for one outside the
/// windows or unresolved (`unresolved` then counts one outside the stack).
fn access(
    observation: crate::registers::Candidate<'_>,
    stored: Option<Vec<StoredBits>>,
    ranges: &[ImageRegion],
    symbols: &Symbols,
    unresolved: &mut u64,
) -> Option<MmioAccess> {
    let (operation, width) = operation(observation.fact)?;
    // The observations already overlap a window: the ranges select them.
    let location = match (observation.address, observation.indexed) {
        (Some(address), _) => MmioLocation::At { address },
        (None, Some(progression)) => MmioLocation::Indexed { progression },
        (None, None) => {
            // An access through the stack is never MMIO.
            let through_stack = matches!(
                observation.fact,
                FunctionRecord::MemoryAccess {
                    address: AbstractValue::EntryStack { .. },
                    ..
                }
            );
            if operation != MmioOperation::ReadSelection && !through_stack {
                *unresolved += 1;
            }
            return None;
        }
    };
    let stored = stored
        .into_iter()
        .flatten()
        .map(|run| MmioStoredBits {
            low: run.low,
            width: run.width,
            source: match run.source {
                StoredBitsSource::Constant { value } => MmioStoredSource::Constant {
                    value,
                    symbol: symbols
                        .at(value)
                        .filter(|_| run.low == 0 && run.width == 32)
                        .map(|(name, offset)| MmioSymbolOffset {
                            name: name.to_owned(),
                            offset,
                        }),
                },
                StoredBitsSource::EntryRegister { register, low } => {
                    MmioStoredSource::EntryRegister { register, low }
                }
                StoredBitsSource::Load {
                    address,
                    width,
                    low,
                    same_word,
                } => MmioStoredSource::Load {
                    address: address.filter(|&address| in_ranges(address, width, ranges)),
                    width,
                    low,
                    same_word,
                },
                StoredBitsSource::Unknown => MmioStoredSource::Unknown,
            },
        })
        .collect();
    Some(MmioAccess {
        location,
        operation,
        width,
        mask: observation.mask,
        stored,
    })
}

/// The symbols one input of a build defines, by address: what a stored
/// address names across builds.
#[derive(Default)]
struct Symbols(Vec<(u64, u64, String)>);

impl Symbols {
    /// Sort the symbols by address, charging the comparisons to `c`.
    fn sort(&mut self, c: &mut dyn RunControl) -> Result<()> {
        charge_sort(self.0.len(), c)?;
        self.0.sort_unstable();
        Ok(())
    }

    /// The symbol `address` lies in, and the offset into it.
    fn at(&self, address: u32) -> Option<(&str, u32)> {
        let address = u64::from(address);
        let after = self.0.partition_point(|&(start, _, _)| start <= address);
        let (start, end, name) = self.0[..after].last()?;
        (address < *end).then(|| (name.as_str(), (address - start) as u32))
    }
}

/// The sized symbols each static executable among `inputs` defines; none
/// for a relocatable input, whose addresses are relocations, not words.
fn symbols<'m>(
    inputs: &[Executable],
    memory: &'m WorkingMemory,
    control: &mut dyn RunControl,
    admitted: &mut Vec<MemoryReservation<'m>>,
) -> Result<Vec<Symbols>> {
    let mut out = Vec::new();
    for input in inputs {
        let inventory = crate::captured::inventory(input, memory, control)?;
        let mut symbols = Symbols::default();
        for object in &inventory.objects {
            let Some(elf) = object
                .elf
                .as_ref()
                .filter(|elf| elf::object_kind(elf.object_type) == elf::ObjectKind::Executable)
            else {
                continue;
            };
            for symbol in &elf.symbols {
                control.checkpoint(1)?;
                // A defined function or data object with a size.
                if symbol.size == 0
                    || !elf::symbol_section(symbol.raw_section, symbol.extended_section)
                        .is_defined()
                    || !matches!(
                        elf::symbol_kind(symbol.symbol_type),
                        elf::SymbolKind::Object | elf::SymbolKind::Function
                    )
                {
                    continue;
                }
                let name = stable_name(symbol.name.as_deref());
                admit(
                    memory,
                    admitted,
                    name.len() + size_of::<(u64, u64, String)>(),
                    control,
                )?;
                symbols
                    .0
                    .push((symbol.value, symbol.value + symbol.size, name));
            }
        }
        symbols.sort(control)?;
        out.push(symbols);
    }
    Ok(out)
}

/// What one build knows of every function, by stable name, how many it
/// blocks and why code was not analyzed. A build with no analyzed function
/// is an error: nothing of its behavior is known.
#[allow(clippy::too_many_arguments)]
fn collect<'m>(
    build: &str,
    inputs: &[Executable],
    assumptions: crate::library::Assumptions<'_>,
    ranges: &[ImageRegion],
    decoder: &dyn FunctionSemantics,
    memory: &'m WorkingMemory,
    control: &mut dyn RunControl,
    admitted: &mut Vec<MemoryReservation<'m>>,
) -> Result<(BTreeMap<String, Accesses>, u64, Vec<MmioGap>)> {
    let symbols = symbols(inputs, memory, control, admitted)?;
    let callees = crate::registers::Callees::capture(inputs, memory, control)?;
    let mut functions: BTreeMap<String, Accesses> = BTreeMap::new();
    let mut blocked = 0;
    let mut gaps = Vec::new();
    crate::library::analyze_library(
        inputs,
        assumptions,
        decoder,
        memory,
        control,
        &mut |outcome, c| match outcome {
            crate::library::LibraryOutcome::Analyzed(analyzed) => {
                let name = stable_name(analyzed.function.name.as_deref());
                admit(memory, admitted, name.len() + size_of::<Accesses>(), c)?;
                let entry = functions.entry(name).or_default();
                entry.analyzed = true;
                entry.incomplete |= !analyzed.complete();
                c.phase(RunPhase::AnalyzeValues)?;
                let facts = oer_analysis_blobray_analysis::navigation::Facts::new(
                    analyzed.records,
                    memory,
                    c,
                )?;
                let mut flow = crate::registers::ReadObservations::new(
                    &facts,
                    memory,
                    decoder,
                    ranges,
                    assumptions.abi,
                    &callees,
                    analyzed.function,
                    analyzed.summaries,
                );
                crate::registers::observe(
                    analyzed.records,
                    &facts,
                    memory,
                    ranges,
                    c,
                    &mut |mut o, c| {
                        // Unlocated candidates only contribute to the unresolved
                        // count. Their provenance never enters a comparison.
                        let stored = if o.address.is_some() || o.indexed.is_some() {
                            flow.stored(&o, c)?
                        } else {
                            None
                        };
                        if let FunctionRecord::MemoryAccess { width, .. } = o.fact {
                            o.mask = o.mask.or_else(|| {
                                stored.as_deref().and_then(|stored| {
                                    oer_analysis_blobray_analysis::registers::write_mask(
                                        *width, stored,
                                    )
                                })
                            });
                        }
                        let input = &symbols[analyzed.function.input as usize];
                        if let Some(access) =
                            access(o, stored, ranges, input, &mut entry.unresolved)
                        {
                            admit(memory, admitted, access_bytes(&access), c)?;
                            entry.accesses.push(access);
                        }
                        Ok(())
                    },
                )
            }
            crate::library::LibraryOutcome::Blocked { function, .. } => {
                blocked += 1;
                let name = stable_name(function.name.as_deref());
                admit(memory, admitted, name.len() + size_of::<Accesses>(), c)?;
                functions.entry(name).or_default().blocked = true;
                Ok(())
            }
            crate::library::LibraryOutcome::Gap {
                input,
                object,
                reason,
            } => {
                admit(memory, admitted, reason.len() + size_of::<MmioGap>(), c)?;
                gaps.push(MmioGap {
                    input,
                    object: object.cloned(),
                    reason: reason.to_owned(),
                });
                Ok(())
            }
        },
    )?;
    if !functions.values().any(|function| function.analyzed) {
        let why = match (gaps.first(), blocked) {
            (Some(gap), _) => gap.reason.clone(),
            (None, 0) => "it defines none".to_owned(),
            (None, blocked) => format!("its {blocked} functions are blocked"),
        };
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            format!("the {build} build has no analyzed function: {why}"),
        ));
    }
    Ok((functions, blocked, gaps))
}

/// The bytes one access holds: itself, its stored runs and the names of
/// the symbols they store.
fn access_bytes(access: &MmioAccess) -> usize {
    size_of::<MmioAccess>()
        + access.stored.len() * size_of::<MmioStoredBits>()
        + access
            .stored
            .iter()
            .map(|run| match &run.source {
                MmioStoredSource::Constant {
                    symbol: Some(symbol),
                    ..
                } => symbol.name.len(),
                _ => 0,
            })
            .sum::<usize>()
}

/// Charge sorting `count` items: about `count · log2 count` comparisons.
fn charge_sort(count: usize, c: &mut dyn RunControl) -> Result<()> {
    let log = usize::BITS - count.max(1).leading_zeros();
    c.checkpoint((count as u64).saturating_mul(u64::from(log)))
}

fn admit<'m>(
    memory: &'m WorkingMemory,
    admitted: &mut Vec<MemoryReservation<'m>>,
    bytes: usize,
    c: &mut dyn RunControl,
) -> Result<()> {
    c.checkpoint(1)?;
    admitted.push(memory.reserve(bytes as u64, c.position())?);
    Ok(())
}

/// `access` with every stored constant reduced by `reduce`.
fn keyed(
    access: &MmioAccess,
    reduce: impl Fn(u32, &Option<MmioSymbolOffset>) -> MmioStoredSource,
) -> MmioAccess {
    MmioAccess {
        stored: access
            .stored
            .iter()
            .map(|run| MmioStoredBits {
                source: match &run.source {
                    MmioStoredSource::Constant { value, symbol } => reduce(*value, symbol),
                    source => source.clone(),
                },
                ..run.clone()
            })
            .collect(),
        ..access.clone()
    }
}

/// Mark the unmatched accesses of `old` and `new` that equal ones of the
/// other side under `key`, as multisets.
/// The keys of each side's unmatched accesses, which it builds, admits in
/// `memory` and charges to `c`, are released when it returns.
fn pair(
    old: &[MmioAccess],
    new: &[MmioAccess],
    matched: &mut [Vec<bool>; 2],
    key: &dyn Fn(&MmioAccess) -> MmioAccess,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<()> {
    let mut keys = Vec::new();
    let mut open = |accesses: &[MmioAccess],
                    done: &[bool],
                    c: &mut dyn RunControl|
     -> Result<Vec<(MmioAccess, usize)>> {
        let open = done.iter().filter(|done| !**done).count();
        let bytes = open * size_of::<(MmioAccess, usize)>()
            + accesses
                .iter()
                .zip(done)
                .filter(|(_, done)| !**done)
                .map(|(access, _)| access_bytes(access))
                .sum::<usize>();
        admit(memory, &mut keys, bytes, c)?;
        let mut pending = Vec::with_capacity(open);
        for (i, access) in accesses.iter().enumerate() {
            if !done[i] {
                c.checkpoint(1)?;
                pending.push((key(access), i));
            }
        }
        charge_sort(pending.len(), c)?;
        pending.sort_unstable();
        Ok(pending)
    };
    let before = open(old, &matched[0], c)?;
    let after = open(new, &matched[1], c)?;
    let mut j = 0;
    for (access, i) in &before {
        c.checkpoint(1)?;
        while j < after.len() && after[j].0 < *access {
            j += 1;
        }
        if j < after.len() && after[j].0 == *access {
            matched[0][*i] = true;
            matched[1][after[j].1] = true;
            j += 1;
        }
    }
    Ok(())
}

/// The accesses `old` has and `new` lacks, and those `new` has and `old`
/// lacks, as multisets. Identical accesses match first; of the rest, a
/// stored whole word in the same symbol at the same offset matches (the
/// object moved between the builds), then the same stored value whatever
/// symbol it lands in. Matching by symbol before value keeps a value that
/// one moved object took over from another from stealing either's pair.
/// What it returns is admitted in `admitted`.
fn unmatched<'m>(
    old: &[MmioAccess],
    new: &[MmioAccess],
    memory: &'m WorkingMemory,
    admitted: &mut Vec<MemoryReservation<'m>>,
    c: &mut dyn RunControl,
) -> Result<(Vec<MmioAccess>, Vec<MmioAccess>)> {
    let _matched = memory.reserve((old.len() + new.len()) as u64, c.position())?;
    let mut matched = [vec![false; old.len()], vec![false; new.len()]];
    pair(old, new, &mut matched, &|access| access.clone(), memory, c)?;
    let by_symbol = |access: &MmioAccess| {
        keyed(access, |value, symbol| MmioStoredSource::Constant {
            value: if symbol.is_some() { 0 } else { value },
            symbol: symbol.clone(),
        })
    };
    pair(old, new, &mut matched, &by_symbol, memory, c)?;
    let by_value = |access: &MmioAccess| {
        keyed(access, |value, _| MmioStoredSource::Constant {
            value,
            symbol: None,
        })
    };
    pair(old, new, &mut matched, &by_value, memory, c)?;
    let mut rest = |accesses: &[MmioAccess],
                    done: &[bool],
                    c: &mut dyn RunControl|
     -> Result<Vec<MmioAccess>> {
        let mut out = Vec::new();
        for (access, _) in accesses.iter().zip(done).filter(|(_, done)| !**done) {
            admit(memory, admitted, access_bytes(access), c)?;
            out.push(access.clone());
        }
        Ok(out)
    };
    Ok((rest(old, &matched[0], c)?, rest(new, &matched[1], c)?))
}

/// Compare the MMIO behavior of the builds `old` and `new` inside `ranges`.
/// Each build is analyzed as `register-accesses` analyzes it; a function is
/// matched across builds by its stable name, and every instance of a name
/// is compared as one multiset of accesses.
#[allow(clippy::too_many_arguments)]
pub fn mmio_diff(
    old: &[Executable],
    new: &[Executable],
    assumptions: crate::library::Assumptions<'_>,
    ranges: &[ImageRegion],
    decoder: &dyn FunctionSemantics,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<MmioDiff> {
    crate::registers::validate_ranges(ranges)?;
    if ranges.is_empty() {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "an MMIO comparison requires at least one address window",
        ));
    }
    let mut admitted = Vec::new();
    let (mut before, blocked_before, gaps_before) = collect(
        "old",
        old,
        assumptions,
        ranges,
        decoder,
        memory,
        control,
        &mut admitted,
    )?;
    let (mut after, blocked_after, gaps_after) = collect(
        "new",
        new,
        assumptions,
        ranges,
        decoder,
        memory,
        control,
        &mut admitted,
    )?;
    let with_accesses = |map: &BTreeMap<String, Accesses>| {
        map.values().filter(|f| !f.accesses.is_empty()).count() as u64
    };
    let functions = [with_accesses(&before), with_accesses(&after)];
    let mut names: Vec<String> = Vec::new();
    for name in before.keys().chain(after.keys()) {
        admit(
            memory,
            &mut admitted,
            name.len() + size_of::<String>(),
            control,
        )?;
        names.push(name.clone());
    }
    charge_sort(names.len(), control)?;
    names.sort_unstable();
    names.dedup();
    let mut changes = Vec::new();
    let mut uncertain = Vec::new();
    for name in names {
        control.checkpoint(1)?;
        let mut old = before.remove(&name).unwrap_or_default();
        let mut new = after.remove(&name).unwrap_or_default();
        let state = [old.state(), new.state()];
        // A blocked function's accesses are unknown: only its state compares.
        let compared = !state.contains(&MmioFunctionState::Blocked);
        charge_sort(old.accesses.len(), control)?;
        old.accesses.sort_unstable();
        charge_sort(new.accesses.len(), control)?;
        new.accesses.sort_unstable();
        let (removed, added) = if compared {
            unmatched(&old.accesses, &new.accesses, memory, &mut admitted, control)?
        } else {
            (Vec::new(), Vec::new())
        };
        // A function with no MMIO accesses in either build has no MMIO
        // behavior to differ, however its presence differs.
        let mmio = !old.accesses.is_empty()
            || !new.accesses.is_empty()
            || old.unresolved != 0
            || new.unresolved != 0;
        let changed = !removed.is_empty()
            || !added.is_empty()
            || old.unresolved != new.unresolved
            || (state[0] != state[1] && (mmio || !compared));
        if !changed {
            let defined = !state.contains(&MmioFunctionState::Absent);
            if defined && (!compared || old.incomplete || new.incomplete) {
                admit(
                    memory,
                    &mut admitted,
                    size_of::<MmioUncertainFunction>(),
                    control,
                )?;
                uncertain.push(MmioUncertainFunction {
                    function: name,
                    state,
                    complete: [
                        state[0] == MmioFunctionState::Analyzed && !old.incomplete,
                        state[1] == MmioFunctionState::Analyzed && !new.incomplete,
                    ],
                });
            }
            continue;
        }
        changes.push(MmioFunctionChange {
            function: name,
            state,
            complete: [
                state[0] == MmioFunctionState::Analyzed && !old.incomplete,
                state[1] == MmioFunctionState::Analyzed && !new.incomplete,
            ],
            removed,
            added,
            unresolved: [old.unresolved, new.unresolved],
        });
    }
    admit(memory, &mut admitted, size_of_val(ranges), control)?;
    Ok(MmioDiff {
        ranges: ranges.to_vec(),
        functions,
        changes,
        uncertain,
        blocked: [blocked_before, blocked_after],
        gaps: [gaps_before, gaps_after],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store of `value` to `address`.
    fn store(address: u32, value: u32) -> MmioAccess {
        MmioAccess {
            location: MmioLocation::At { address },
            operation: MmioOperation::Store,
            width: Some(4),
            mask: None,
            stored: vec![MmioStoredBits {
                low: 0,
                width: 32,
                source: MmioStoredSource::Constant {
                    value,
                    symbol: None,
                },
            }],
        }
    }

    struct Units(u64);
    impl RunControl for Units {
        fn checkpoint(&mut self, units: u64) -> Result<()> {
            self.0 = self
                .0
                .checked_sub(units)
                .ok_or_else(|| Error::new(ErrorCode::ResourceLimited, "work units exhausted"))?;
            Ok(())
        }
    }

    #[test]
    fn sorting_the_symbol_table_is_charged_by_its_comparisons() {
        let table = || {
            Symbols(
                (0..1024u64)
                    .rev()
                    .map(|i| (4 * i, 4 * i + 4, format!("s{i}")))
                    .collect(),
            )
        };
        // One unit per symbol does not cover sorting 1024 of them.
        let error = table().sort(&mut Units(1024)).unwrap_err();
        assert_eq!(error.code, ErrorCode::ResourceLimited);
        let mut symbols = table();
        symbols.sort(&mut Units(1024 * 11)).unwrap();
        assert_eq!(symbols.at(8), Some(("s2", 0)));
    }

    #[test]
    fn the_comparison_admits_its_keys_and_charges_their_sort() {
        // Disjoint sides: every key is built, sorted and left unmatched.
        let old: Vec<_> = (0..512).map(|i| store(0x2000_0000 + 4 * i, 0)).collect();
        let new: Vec<_> = (0..512).map(|i| store(0x2000_0000 + 4 * i, 1)).collect();
        let compare = |memory: &WorkingMemory, control: &mut dyn RunControl| {
            let mut admitted = Vec::new();
            unmatched(&old, &new, memory, &mut admitted, control)
                .map(|(removed, added)| (removed.len(), added.len(), admitted.len()))
        };
        let memory = WorkingMemory::new(1 << 30).unwrap();
        let (removed, added, admitted) = compare(&memory, &mut || Ok(())).unwrap();
        assert_eq!((removed, added), (512, 512));
        assert_eq!(admitted, 1024, "each returned access is admitted");
        let needed = memory.peak();
        assert!(needed >= 1024 * size_of::<MmioAccess>() as u64);
        // Less memory than the keys and results need fails inside the
        // comparison.
        let error = compare(&WorkingMemory::new(needed - 1).unwrap(), &mut || Ok(())).unwrap_err();
        assert_eq!(error.code, ErrorCode::ResourceLimited, "{error:?}");
        // Sorting the keys is charged: the units that cover building and
        // walking every key (three passes of 1024 keys and 512 steps, and
        // 1024 results) do not cover sorting them.
        let error = compare(&memory, &mut Units(6000)).unwrap_err();
        assert_eq!(error.code, ErrorCode::ResourceLimited, "{error:?}");
    }
}
