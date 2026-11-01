//! The copy report: every direct call or tail jump into a memory copy
//! entry ([`copy_entry`]) in the final machine code, with the byte count
//! the caller passes in `a2` where the value analysis gives it exactly,
//! and the roots of the panic check that reach it.
//!
//! The compiler's `large-assignments` lint and the large-values report see
//! MIR values only; LLVM creates further copies (by-value arguments,
//! returns, spills of aggregates) that only the machine code shows. This
//! report reads that code: each site is named by its caller and by the
//! DWARF inline chain at the call, innermost first, so a copy inlined from
//! another crate names where it came from. Sites of a constant size of at
//! least [`REPORT_BYTES`] are listed one by one, largest first; constant
//! smaller ones are counted; variable ones are listed per caller. Each
//! listed site and caller names the roots (each hart's task entry and task
//! roots, each interrupt level and exception) whose bound reaches its
//! caller, and each root has its totals. The report decides nothing: it is
//! not a check and fails no build.
//!
//! [`RUNTIME_JSON`] holds every site with its return address, its caller's
//! code range and its roots, and the copy entries' code ranges, for a host
//! program that weights the sites by a PC profile ([`read`]).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use oer_image_stack::{Analysis, TransferKind};
use serde::{Deserialize, Serialize};

use crate::Result;
use crate::panics::Root;

/// The runtime ELF's copy report.
pub const RUNTIME_REPORT: &str = "runtime-copies.txt";
/// The runtime ELF's copy sites, machine-readable ([`Copies`]).
pub const RUNTIME_JSON: &str = "runtime-copies.json";
/// The version of [`Copies`] that [`read`] accepts.
pub const SCHEMA: u32 = 1;

/// Constant copies of at least this many bytes are listed one by one.
pub const REPORT_BYTES: u32 = 512;

/// Inline frames printed for each listed site.
const FRAMES: usize = 4;

/// The demangled memory copy entries: `compiler_builtins`' and their C
/// symbols.
pub fn copy_entry(name: &str) -> bool {
    let name = strip_hash(name);
    matches!(
        name,
        "memcpy" | "memmove" | "compiler_builtins::mem::memcpy" | "compiler_builtins::mem::memmove"
    )
}

/// `name` without a legacy-mangling `::h<16 hex>` suffix.
fn strip_hash(name: &str) -> &str {
    match name.rsplit_once("::h") {
        Some((head, hash)) if hash.len() == 16 && hash.bytes().all(|b| b.is_ascii_hexdigit()) => {
            head
        }
        _ => name,
    }
}

/// One copy site: a call or tail jump at `site` in `caller` into `entry`,
/// copying `bytes` where `a2` is exact there.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Site {
    pub caller: u32,
    pub site: u32,
    pub entry: u32,
    pub bytes: Option<u32>,
    /// A tail jump: the entry returns to the caller's caller.
    pub tail: bool,
}

/// The copy sites in every function of `analysis` that is not itself a
/// copy entry.
pub fn sites(analysis: &Analysis, names: &BTreeMap<u32, String>) -> Vec<Site> {
    let entry = |address: u32| names.get(&address).is_some_and(|name| copy_entry(name));
    let mut sites = Vec::new();
    for (&caller, facts) in &analysis.functions {
        if entry(caller) {
            continue;
        }
        for transfer in &facts.transfers {
            let Some(target) = transfer.target.filter(|&target| entry(target)) else {
                continue;
            };
            // A tail jump keeps the caller's arguments too; the value
            // analysis records exact ones at direct calls.
            let call = transfer.kind == TransferKind::Call;
            let bytes = call
                .then(|| facts.call_arguments.get(&transfer.site))
                .flatten()
                .and_then(|arguments| arguments[2]);
            sites.push(Site {
                caller,
                site: transfer.site,
                entry: target,
                bytes,
                tail: !call,
            });
        }
    }
    sites
}

/// Where a site's source is: the inline chain at an address, innermost
/// first, each `file:line` relative to the workspace root when under it.
pub trait Locate {
    fn locate(&self, address: u32) -> Vec<String>;
}

impl Locate for oer_analysis_elf::dwarf::Symbolizer {
    fn locate(&self, address: u32) -> Vec<String> {
        self.frames(u64::from(address))
            .map(|frames| {
                frames
                    .into_iter()
                    .map(|frame| match (frame.file, frame.line) {
                        (Some(file), Some(line)) => format!("{file}:{line}"),
                        (Some(file), None) => file,
                        _ => frame.function.unwrap_or_else(|| "?".to_owned()),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// The labels of the roots of `roots` whose bound reaches `caller`.
fn reaching(roots: &[Root], caller: u32) -> Vec<String> {
    roots
        .iter()
        .filter(|root| root.reached.contains(&caller))
        .map(|root| root.label.clone())
        .collect()
}

/// The roots column of a line: the labels, `-` for none, `?` when the
/// roots were not analysed.
fn roots_column(roots: Option<&[Root]>, caller: u32) -> String {
    match roots {
        None => "?".to_owned(),
        Some(roots) => {
            let labels = reaching(roots, caller);
            if labels.is_empty() {
                "-".to_owned()
            } else {
                labels.join(",")
            }
        }
    }
}

/// The source of a site: the workspace's frames, `core` and dependency
/// frames only where none is the workspace's, at most [`FRAMES`].
fn source(locate: &impl Locate, site: u32, root: &Path) -> Vec<String> {
    let root = format!("{}/", root.display());
    let frames: Vec<String> = locate.locate(site);
    let own: Vec<String> = frames
        .iter()
        .filter_map(|frame| frame.strip_prefix(&root))
        .take(FRAMES)
        .map(str::to_owned)
        .collect();
    if own.is_empty() {
        frames.into_iter().take(FRAMES).collect()
    } else {
        own
    }
}

/// One root's (or no root's) share of the sites.
#[derive(Default)]
struct Totals {
    sites: usize,
    constant: usize,
    constant_bytes: u64,
    variable: usize,
}

impl Totals {
    fn add(&mut self, site: &Site) {
        self.sites += 1;
        match site.bytes {
            Some(bytes) => {
                self.constant += 1;
                self.constant_bytes += u64::from(bytes);
            }
            None => self.variable += 1,
        }
    }
}

/// The report over `sites`, naming functions from `names` and sources
/// through `locate`, with paths shown relative to `root`, and the roots
/// of `roots` that reach each site; `None` when the roots were not
/// analysed.
pub fn render(
    sites: &[Site],
    names: &BTreeMap<u32, String>,
    locate: &impl Locate,
    root: &Path,
    roots: Option<&[Root]>,
) -> String {
    let name = |address: u32| {
        names.get(&address).map_or_else(
            || format!("{address:#010x}"),
            |name| strip_hash(name).to_owned(),
        )
    };
    let mut listed: Vec<&Site> = sites
        .iter()
        .filter(|site| site.bytes.is_some_and(|bytes| bytes >= REPORT_BYTES))
        .collect();
    listed.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.site.cmp(&b.site)));
    let small = sites
        .iter()
        .filter(|site| site.bytes.is_some_and(|bytes| bytes < REPORT_BYTES))
        .count();
    let mut variable: BTreeMap<(String, u32), usize> = BTreeMap::new();
    for site in sites.iter().filter(|site| site.bytes.is_none()) {
        *variable
            .entry((name(site.caller), site.caller))
            .or_default() += 1;
    }
    let listed_bytes: u64 = listed
        .iter()
        .filter_map(|site| site.bytes)
        .map(u64::from)
        .sum();
    let mut out = String::new();
    let _ = writeln!(
        out,
        "memory copy call sites in the machine code (memcpy, memmove)"
    );
    let _ = writeln!(
        out,
        "sites\t{}\nconstant >= {REPORT_BYTES} B\t{} ({listed_bytes} B in all)\nconstant < {REPORT_BYTES} B\t{small}\nvariable\t{}",
        sites.len(),
        listed.len(),
        sites.len() - listed.len() - small,
    );
    match roots {
        None => {
            let _ = writeln!(out, "\nroots: not analysed");
        }
        Some(roots) => {
            let _ = writeln!(
                out,
                "\nby root (sites its bound reaches; constant bytes summed once per site):\nroot\tsites\tconstant\tconstant B\tvariable"
            );
            let mut unreached = Totals::default();
            let mut totals: Vec<Totals> = roots.iter().map(|_| Totals::default()).collect();
            for site in sites {
                let mut any = false;
                for (root, totals) in roots.iter().zip(&mut totals) {
                    if root.reached.contains(&site.caller) {
                        totals.add(site);
                        any = true;
                    }
                }
                if !any {
                    unreached.add(site);
                }
            }
            for (label, totals) in roots
                .iter()
                .map(|root| root.label.as_str())
                .zip(&totals)
                .chain([("-", &unreached)])
            {
                let _ = writeln!(
                    out,
                    "{label}\t{}\t{}\t{}\t{}",
                    totals.sites, totals.constant, totals.constant_bytes, totals.variable
                );
            }
        }
    }
    let _ = writeln!(
        out,
        "\nconstant >= {REPORT_BYTES} B (bytes, site, caller, roots, source innermost first):"
    );
    for site in listed {
        let _ = writeln!(
            out,
            "{}\t{:#010x}\t{}\t{}\t{}",
            site.bytes.unwrap_or_default(),
            site.site,
            name(site.caller),
            roots_column(roots, site.caller),
            source(locate, site.site, root).join(" <- "),
        );
    }
    let mut variable: Vec<((String, u32), usize)> = variable.into_iter().collect();
    variable.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let _ = writeln!(out, "\nvariable size (sites, caller, roots):");
    for ((caller, address), count) in variable {
        let _ = writeln!(out, "{count}\t{caller}\t{}", roots_column(roots, address));
    }
    out
}

/// A function's code range and name.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Code {
    pub name: String,
    pub address: u32,
    pub size: u32,
}

impl Code {
    /// Whether `address` lies in the code.
    pub fn contains(&self, address: u32) -> bool {
        address
            .checked_sub(self.address)
            .is_some_and(|offset| offset < self.size)
    }
}

/// One copy site of [`Copies`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CopySite {
    pub site: u32,
    /// The address the call returns to, the interrupted `ra` of a sample
    /// in the entry; `None` for a tail jump.
    pub returns: Option<u32>,
    /// The copy entry's name.
    pub entry: String,
    pub caller: Code,
    /// The bytes copied, where the value analysis gives `a2` exactly.
    pub bytes: Option<u32>,
    /// The inline chain at the site, innermost first.
    pub source: Vec<String>,
    /// The labels of the roots whose bound reaches the caller.
    pub roots: Vec<String>,
}

/// An ELF's copy sites: [`RUNTIME_JSON`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Copies {
    pub schema: u32,
    /// The copy entries' code.
    pub entries: Vec<Code>,
    /// The labels of the analysed roots; `None` when none were.
    pub roots: Option<Vec<String>>,
    pub sites: Vec<CopySite>,
}

/// The length of the instruction at `address` in an executable segment
/// of `elf`.
fn instruction_length(elf: &oer_analysis_elf::Elf<'_>, address: u32) -> Option<u32> {
    let half = elf
        .segments()
        .filter(|segment| segment.executable)
        .find_map(|segment| segment.bytes(u64::from(address), 2))?;
    Some(if half.first()? & 3 == 3 { 4 } else { 2 })
}

/// The machine-readable sites of `image`.
pub fn copies(
    image: &crate::stack::Image,
    sites: &[Site],
    locate: &impl Locate,
    root: &Path,
    roots: Option<&[Root]>,
) -> Result<Copies> {
    let elf = oer_analysis_elf::Elf::parse(&image.elf)?;
    let code = |address: u32| Code {
        name: image.names.get(&address).map_or_else(
            || format!("{address:#010x}"),
            |name| strip_hash(name).to_owned(),
        ),
        address,
        size: image
            .analysis
            .functions
            .get(&address)
            .map_or(0, |facts| facts.function.size),
    };
    let entries = sites
        .iter()
        .map(|site| site.entry)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .map(code)
        .collect();
    let sites = sites
        .iter()
        .map(|site| CopySite {
            site: site.site,
            returns: (!site.tail)
                .then(|| instruction_length(&elf, site.site))
                .flatten()
                .and_then(|length| site.site.checked_add(length)),
            entry: code(site.entry).name,
            caller: code(site.caller),
            bytes: site.bytes,
            source: source(locate, site.site, root),
            roots: roots.map_or_else(Vec::new, |roots| reaching(roots, site.caller)),
        })
        .collect();
    Ok(Copies {
        schema: SCHEMA,
        entries,
        roots: roots.map(|roots| roots.iter().map(|root| root.label.clone()).collect()),
        sites,
    })
}

/// The copy sites a build wrote to `path` ([`RUNTIME_JSON`]).
pub fn read(path: &Path) -> Result<Copies> {
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let copies: Copies =
        serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", path.display()))?;
    if copies.schema != SCHEMA {
        return Err(format!(
            "{}: schema {}, this reader takes {SCHEMA}",
            path.display(),
            copies.schema
        )
        .into());
    }
    Ok(copies)
}

/// Write the reports of `image` into `output`, naming the roots of `roots`
/// that reach each site; `None` when the roots were not analysed.
pub(crate) fn write(
    image: &crate::stack::Image,
    root: &Path,
    output: &Path,
    roots: Option<&[Root]>,
) -> Result<()> {
    let symbolizer = oer_analysis_elf::dwarf::Symbolizer::shared(image.elf.clone())?;
    let sites = sites(&image.analysis, &image.names);
    std::fs::write(
        output.join(RUNTIME_REPORT),
        render(&sites, &image.names, &symbolizer, root, roots),
    )?;
    let copies = copies(image, &sites, &symbolizer, root, roots)?;
    std::fs::write(
        output.join(RUNTIME_JSON),
        serde_json::to_vec_pretty(&copies)?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
