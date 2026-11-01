//! The copy report: every direct call or tail jump into a memory copy
//! entry ([`copy_entry`]) in the final machine code, with the byte count
//! the caller passes in `a2` where the value analysis gives it exactly.
//!
//! The compiler's `large-assignments` lint and the large-values report see
//! MIR values only; LLVM creates further copies (by-value arguments,
//! returns, spills of aggregates) that only the machine code shows. This
//! report reads that code: each site is named by its caller and by the
//! DWARF inline chain at the call, innermost first, so a copy inlined from
//! another crate names where it came from. Sites of a constant size of at
//! least [`REPORT_BYTES`] are listed one by one, largest first; constant
//! smaller ones are counted; variable ones are listed per caller. The
//! report decides nothing: it is not a check and fails no build.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use oer_image_stack::{Analysis, TransferKind};

use crate::Result;

/// The runtime ELF's copy report.
pub const RUNTIME_REPORT: &str = "runtime-copies.txt";

/// Constant copies of at least this many bytes are listed one by one.
pub const REPORT_BYTES: u32 = 512;

/// Inline frames printed for each listed site.
const FRAMES: usize = 4;

/// The demangled memory copy entries: `compiler_builtins`' and their C
/// symbols.
pub(crate) fn copy_entry(name: &str) -> bool {
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
            let bytes = (transfer.kind == TransferKind::Call)
                .then(|| facts.call_arguments.get(&transfer.site))
                .flatten()
                .and_then(|arguments| arguments[2]);
            sites.push(Site {
                caller,
                site: transfer.site,
                entry: target,
                bytes,
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

/// The report over `sites`, naming functions from `names` and sources
/// through `locate`, with paths shown relative to `root`.
pub fn render(
    sites: &[Site],
    names: &BTreeMap<u32, String>,
    locate: &impl Locate,
    root: &Path,
) -> String {
    let name = |address: u32| {
        names.get(&address).map_or_else(
            || format!("{address:#010x}"),
            |name| strip_hash(name).to_owned(),
        )
    };
    let root = format!("{}/", root.display());
    let relative = |location: String| match location.strip_prefix(&root) {
        Some(rest) => rest.to_owned(),
        None => location,
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
    let mut variable: BTreeMap<String, usize> = BTreeMap::new();
    for site in sites.iter().filter(|site| site.bytes.is_none()) {
        *variable.entry(name(site.caller)).or_default() += 1;
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
    let _ = writeln!(
        out,
        "\nconstant >= {REPORT_BYTES} B (bytes, site, caller, source innermost first):"
    );
    for site in listed {
        // The workspace's frames name the copy; `core` and dependency
        // frames only where none is the workspace's.
        let frames: Vec<String> = locate.locate(site.site);
        let own: Vec<String> = frames
            .iter()
            .filter(|frame| frame.starts_with(&root))
            .take(FRAMES)
            .cloned()
            .map(relative)
            .collect();
        let source = if own.is_empty() {
            frames.into_iter().take(FRAMES).collect()
        } else {
            own
        };
        let _ = writeln!(
            out,
            "{}\t{:#010x}\t{}\t{}",
            site.bytes.unwrap_or_default(),
            site.site,
            name(site.caller),
            source.join(" <- "),
        );
    }
    let mut variable: Vec<(String, usize)> = variable.into_iter().collect();
    variable.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let _ = writeln!(out, "\nvariable size (sites, caller):");
    for (caller, count) in variable {
        let _ = writeln!(out, "{count}\t{caller}");
    }
    out
}

/// Write the report of `image` into `output`.
pub(crate) fn write(image: &crate::stack::Image, root: &Path, output: &Path) -> Result<()> {
    let symbolizer = oer_analysis_elf::dwarf::Symbolizer::shared(image.elf.clone())?;
    let sites = sites(&image.analysis, &image.names);
    std::fs::write(
        output.join(RUNTIME_REPORT),
        render(&sites, &image.names, &symbolizer, root),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests;
