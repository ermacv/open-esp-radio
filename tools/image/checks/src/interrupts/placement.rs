//! Measurements only: the existing handler inventory and bounds keep their
//! meaning, and none of these totals participates in a gate's verdict.

use super::{InterruptStacks, Result, cached_hook_references, code::Role};
use crate::stack::Image;
use oer_analysis_elf::{Elf, SymbolKind};
use oer_analysis_riscv_stack::{Analysis, HartStack};
use oer_chip_profile::Profile;
use oer_espressif_facts::Region;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Write as _};

/// The full interrupt placement inventory in an image bundle.
pub const INTERRUPT_PLACEMENT_REPORT: &str = "interrupt-placement.txt";

/// One hart and priority; zero denotes its exception path.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Level {
    hart: u32,
    priority: u32,
}

impl fmt::Display for Level {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(out, "hart {} ", self.hart)?;
        match self.priority {
            0 => out.write_str("exception"),
            priority => write!(out, "P{priority}"),
        }
    }
}

#[derive(Debug, Default, Eq, PartialEq)]
struct Total {
    functions: usize,
    bytes: u64,
}

#[derive(Debug)]
struct CachedFunction {
    address: u32,
    size: u32,
    name: String,
    krate: String,
    levels: Vec<Level>,
    role: Role,
}

#[derive(Clone, Debug)]
struct DataObject {
    address: u64,
    size: u64,
    names: BTreeSet<String>,
}

#[derive(Debug)]
struct DataReference {
    function: u32,
    name: String,
    site: u32,
    target: u64,
    object: Option<(u64, u64)>,
}

#[derive(Debug)]
struct SramRange {
    name: String,
    start: u64,
    end: u64,
}

#[derive(Debug)]
struct Sram {
    region: Region,
    source: String,
    ranges: Vec<SramRange>,
    placed: u64,
}

impl Sram {
    fn of(elf: &Elf<'_>, profile: &Profile, region: Region) -> Self {
        let symbols = elf.addresses();
        let mut ranges = Vec::new();
        if let Some(placement) = &profile.facts.placement {
            for rule in &placement.range {
                if rule.region != "sram" {
                    continue;
                }
                let [start, end] = &rule.range;
                if let (Some(&start_at), Some(&end_at)) =
                    (symbols.get(start.as_str()), symbols.get(end.as_str()))
                {
                    ranges.push(SramRange {
                        name: format!("{start}..{end}"),
                        start: start_at,
                        end: end_at,
                    });
                }
            }
        }
        // Include allocated SRAM sections too: a section such as .hot.text
        // need not have its own placement range. NOLOAD stacks and DMA count.
        ranges.extend(
            elf.sections()
                .filter(|section| section.allocated && section.size > 0)
                .filter_map(|section| {
                    let end = section.address.checked_add(section.size)?;
                    (section.address < u64::from(region.end()) && end > u64::from(region.origin))
                        .then(|| SramRange {
                            name: section.name.to_owned(),
                            start: section.address,
                            end,
                        })
                }),
        );
        let placed = union_bytes(region, &ranges);
        Self {
            region,
            source: format!(
                "chips/{}/platform/chip.toml [memory].sram; application-owned SRAM_START..SRAM_SEG_END; ROM boot-stack side above the end is already excluded (source: chips/{}/layout/src/lib.rs, ESP-IDF memory map)",
                profile.facts.id, profile.facts.id
            ),
            ranges,
            placed,
        }
    }

    fn free(&self) -> u64 {
        u64::from(self.region.length) - self.placed
    }
}

/// Count the union, clipped to the usable contract region. In particular,
/// IRQ stacks nested inside critical BSS must not be subtracted twice.
fn union_bytes(region: Region, ranges: &[SramRange]) -> u64 {
    let mut ranges: Vec<_> = ranges
        .iter()
        .filter_map(|range| {
            let start = range.start.max(u64::from(region.origin));
            let end = range.end.min(u64::from(region.end()));
            (start < end).then_some((start, end))
        })
        .collect();
    ranges.sort_unstable();
    let mut end = u64::from(region.origin);
    let mut total = 0;
    for (start, next_end) in ranges {
        total += next_end.saturating_sub(start.max(end));
        end = end.max(next_end);
    }
    total
}

/// The placement measurements, kept separate from [`super::HandlerCode`]
/// and [`InterruptStacks::check`]. Counts use the resolved strict/task
/// partition, including exceptions and the independent panic path.
#[derive(Debug)]
pub struct InterruptPlacement {
    functions: Vec<CachedFunction>,
    references: Vec<DataReference>,
    objects: BTreeMap<(u64, u64), DataObject>,
    sram: Sram,
    code_summary: String,
    strict_totals: String,
    strict_paths: String,
}

impl InterruptPlacement {
    /// Unoccupied bytes in the SRAM contract, counting the union of placed ranges.
    pub fn free_sram(&self) -> u64 {
        self.sram.free()
    }

    /// Reuse the analysis's function sizes and the loaded ELF's relocations,
    /// symbols and section/range addresses; do not read an ELF from disk.
    pub fn of(image: &Image, stacks: &InterruptStacks, profile: &Profile) -> Result<Self> {
        let elf = Elf::parse(&image.elf)?;
        let (functions, _) = cached_functions(&image.analysis, stacks);
        let reached = stacks
            .harts
            .iter()
            .flat_map(|hart| &hart.levels)
            .flat_map(|level| level.bound.reached.iter().copied())
            .collect();
        let references =
            cached_hook_references(&elf, &image.functions, &reached, &stacks.contract.memory)?;
        let (references, objects) = cached_data(&elf, &references, stacks);
        Ok(Self {
            functions,
            references,
            objects,
            sram: Sram::of(&elf, profile, stacks.contract.memory.sram),
            code_summary: stacks
                .code_placement
                .summary(|address| stacks.name(address)),
            strict_totals: stacks.code_placement.totals(),
            strict_paths: stacks
                .code_placement
                .render_paths(|address| stacks.name(address)),
        })
    }

    fn data_bytes(&self) -> u64 {
        self.objects.values().map(|object| object.size).sum()
    }

    /// The code partition, data and SRAM lines following `handler code:`.
    pub fn summary(&self) -> String {
        format!(
            "{}cached handler data: {} references to {} objects, {} bytes\nSRAM: usable {}, placed {}, free {} bytes\n",
            self.code_summary,
            self.references.len(),
            self.objects.len(),
            self.data_bytes(),
            self.sram.region.length,
            self.sram.placed,
            self.sram.free(),
        )
    }

    /// Complete lists, with deterministic ordering and no truncation.
    pub fn render(&self) -> String {
        let mut out = self.summary();
        out.push_str(
            "\nCached handler functions (size descending)\nname\taddress\tsize\tlevels\tcrate\tclass\n",
        );
        let mut crates: BTreeMap<&str, Total> = BTreeMap::new();
        for function in &self.functions {
            let levels = function
                .levels
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(
                out,
                "{}\t{:#010x}\t{}\t{}\t{}\t{}",
                function.name,
                function.address,
                function.size,
                levels,
                function.krate,
                function.role
            );
            let total = crates.entry(&function.krate).or_default();
            total.functions += 1;
            total.bytes += u64::from(function.size);
        }
        out.push_str("\nCached handler code by crate\ncrate\tfunctions\tbytes\n");
        let mut crates: Vec<_> = crates.into_iter().collect();
        crates.sort_by(|(a, x), (b, y)| y.bytes.cmp(&x.bytes).then(a.cmp(b)));
        for (krate, total) in crates {
            let _ = writeln!(out, "{krate}\t{}\t{}", total.functions, total.bytes);
        }
        out.push('\n');
        out.push_str(&self.strict_totals);
        out.push('\n');
        out.push_str(&self.strict_paths);
        out.push_str("\nCached handler data references\nfunction\tfunction address\tsite\ttarget\tobject address\tobject size\tobject names\n");
        for reference in &self.references {
            let _ = write!(
                out,
                "{}\t{:#010x}\t{:#010x}\t{:#010x}\t",
                reference.name, reference.function, reference.site, reference.target
            );
            match reference.object.and_then(|key| self.objects.get(&key)) {
                Some(object) => {
                    let _ = writeln!(
                        out,
                        "{:#010x}\t{}\t{}",
                        object.address,
                        object.size,
                        object.names.iter().cloned().collect::<Vec<_>>().join(" | ")
                    );
                }
                None => out.push_str("?\t?\tno sized data symbol\n"),
            }
        }
        out.push_str("\nCached handler data objects\naddress\tsize\tnames\n");
        for object in self.objects.values() {
            let _ = writeln!(
                out,
                "{:#010x}\t{}\t{}",
                object.address,
                object.size,
                object.names.iter().cloned().collect::<Vec<_>>().join(" | ")
            );
        }
        let unknown = self
            .references
            .iter()
            .filter(|reference| reference.object.is_none())
            .count();
        let _ = writeln!(
            out,
            "\nData accounting: {unknown} references without a sized data symbol; object bytes cover sized objects only. Code targets are excluded. Aliases of the same address and size count once; interior addresses count the whole object. Only static references from resolved handler code are measured."
        );
        out.push_str("\nSRAM accounting\n");
        let _ = writeln!(
            out,
            "usable range: {:#010x}..{:#010x}\nboundary source: {}\nPlaced bytes are the union of placement ranges and allocated SRAM sections, including NOLOAD IRQ stacks and DMA; overlaps count once.\nname\tstart\tend",
            self.sram.region.origin,
            self.sram.region.end(),
            self.sram.source
        );
        for range in &self.sram.ranges {
            let _ = writeln!(
                out,
                "{}\t{:#010x}\t{:#010x}",
                range.name, range.start, range.end
            );
        }
        out
    }
}

fn level_reachability(harts: &[HartStack]) -> BTreeMap<Level, &BTreeSet<u32>> {
    harts
        .iter()
        .flat_map(|hart| {
            hart.levels.iter().chain([&hart.exception]).map(|level| {
                (
                    Level {
                        hart: hart.core,
                        priority: level.level,
                    },
                    &level.bound.reached,
                )
            })
        })
        .collect()
}

fn cached_functions(
    analysis: &Analysis,
    stacks: &InterruptStacks,
) -> (Vec<CachedFunction>, BTreeMap<Level, Total>) {
    let reached = level_reachability(&stacks.harts);
    let mut levels: BTreeMap<_, _> = reached
        .keys()
        .map(|&level| (level, Total::default()))
        .collect();
    let mut functions = Vec::new();
    let cached: BTreeSet<_> = stacks
        .handler_code
        .cached
        .iter()
        .copied()
        .chain(
            stacks
                .code_placement
                .functions
                .iter()
                .filter(|(_, function)| function.location.cached())
                .map(|(&address, _)| address),
        )
        .collect();
    // Both inventories obtained these addresses from this same analysis.
    for facts in cached
        .iter()
        .filter_map(|address| analysis.functions.get(address))
    {
        let (address, size) = (facts.function.address, facts.function.size);
        let name = stacks.name(address);
        let krate = name
            .split("::")
            .next()
            .unwrap_or(&name)
            .trim_start_matches('<')
            .to_owned();
        let membership: Vec<_> = reached
            .iter()
            .filter_map(|(&level, functions)| functions.contains(&address).then_some(level))
            .collect();
        for level in &membership {
            let total = levels.entry(*level).or_default();
            total.functions += 1;
            total.bytes += u64::from(size);
        }
        functions.push(CachedFunction {
            address,
            size,
            name,
            krate,
            levels: membership,
            role: stacks
                .code_placement
                .functions
                .get(&address)
                .map_or(Role::Strict, |function| function.role),
        });
    }
    functions.sort_by(|a, b| b.size.cmp(&a.size).then(a.address.cmp(&b.address)));
    (functions, levels)
}

fn cached_data(
    elf: &Elf<'_>,
    references: &[(u32, u32, u64)],
    stacks: &InterruptStacks,
) -> (Vec<DataReference>, BTreeMap<(u64, u64), DataObject>) {
    let mut symbols: BTreeMap<(u64, u64), DataObject> = BTreeMap::new();
    for symbol in elf
        .symbols()
        .filter(|symbol| symbol.defined && symbol.size > 0 && symbol.kind == SymbolKind::Data)
    {
        symbols
            .entry((symbol.address, symbol.size))
            .or_insert_with(|| DataObject {
                address: symbol.address,
                size: symbol.size,
                names: BTreeSet::new(),
            })
            .names
            .insert(symbol.demangled());
    }
    let code: Vec<_> = elf
        .sections()
        .filter(|section| section.executable)
        .collect();
    let mut objects = BTreeMap::new();
    let mut data = Vec::new();
    for &(function, site, target) in references {
        let object = symbols
            .values()
            .filter(|object| {
                target
                    .checked_sub(object.address)
                    .is_some_and(|offset| offset < object.size)
            })
            .min_by_key(|object| (object.size, object.address));
        // The hook's cache-safety test includes calls and function addresses.
        // Those are already measured as code, rather than as data objects.
        if object.is_none() && code.iter().any(|section| section.contains(target)) {
            continue;
        }
        let key = object.map(|object| (object.address, object.size));
        if let Some(object) = object {
            objects.insert((object.address, object.size), object.clone());
        }
        data.push(DataReference {
            function,
            name: stacks.name(function),
            site,
            target,
            object: key,
        });
    }
    (data, objects)
}

#[cfg(test)]
pub(super) mod tests;
