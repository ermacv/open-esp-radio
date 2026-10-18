//! What a scenario's verdicts depend on in its compiled production probe.
//!
//! Three layers: the repository source files of every executed production
//! instruction (from its line information, inlined frames included), the
//! declaration file of every named data object the execution read, and the
//! content of every probe data range the execution read. Files outside the
//! repository (the standard library, registry crates) are pinned by the
//! toolchain and the lock files, which the evidence records globally. A
//! dependency that cannot be attributed makes the scenario fall back to its
//! probe's whole source closure, stated in the result.
use crate::harness::{Result, invalid};
use oer_elf::dwarf::{Dwarf, Symbolizer, gimli};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// The dependencies of one scenario's execution of its probe image.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dependencies {
    /// Repository-relative source files executed code or read named data
    /// come from.
    pub files: BTreeSet<PathBuf>,
    /// SHA-256 of the sorted multiset of the probe data ranges the
    /// execution read, independent of their addresses.
    pub read_data: String,
    /// Why the files cannot stand for the execution, when they cannot.
    pub fallback: Option<String>,
}

/// `path` relative to the canonical repository `root`, when inside it.
fn relative(path: &Path, root: &Path) -> Option<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let canonical = absolute.canonicalize().ok()?;
    canonical.strip_prefix(root).ok().map(Path::to_path_buf)
}

/// The declaration file of every data variable DWARF places at a fixed
/// address, by that address.
fn declarations(dwarf: &Dwarf) -> Result<BTreeMap<u64, PathBuf>> {
    use gimli::Reader as _;
    let mut files = BTreeMap::new();
    let mut units = dwarf.units();
    while let Some(header) = units.next()? {
        let unit = dwarf.unit(header)?;
        let mut entries = unit.entries();
        while let Some(entry) = entries.next_dfs()? {
            if entry.tag() != gimli::DW_TAG_variable {
                continue;
            }
            let Some(gimli::AttributeValue::Exprloc(expression)) =
                entry.attr_value(gimli::DW_AT_location)
            else {
                continue;
            };
            let mut operations = expression.operations(unit.encoding());
            let Ok(Some(gimli::Operation::Address { address })) = operations.next() else {
                continue;
            };
            let Some(gimli::AttributeValue::FileIndex(index)) =
                entry.attr_value(gimli::DW_AT_decl_file)
            else {
                continue;
            };
            let Some(program) = &unit.line_program else {
                continue;
            };
            let header = program.header();
            let Some(file) = header.file(index) else {
                continue;
            };
            let mut path = PathBuf::new();
            if let Some(directory) = file.directory(header) {
                path.push(
                    dwarf
                        .attr_string(&unit, directory)?
                        .to_string_lossy()?
                        .as_ref(),
                );
            }
            path.push(
                dwarf
                    .attr_string(&unit, file.path_name())?
                    .to_string_lossy()?
                    .as_ref(),
            );
            files.insert(address, path);
        }
    }
    Ok(files)
}

/// Contiguous byte ranges covering `reads`, as `[start, end)`.
fn merge(reads: &BTreeSet<(u32, u8)>) -> Vec<(u64, u64)> {
    let mut ranges: Vec<(u64, u64)> = vec![];
    for &(address, width) in reads {
        let (start, end) = (u64::from(address), u64::from(address) + u64::from(width));
        match ranges.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => ranges.push((start, end)),
        }
    }
    ranges
}

/// The repository files of `executed` instructions, whose source paths
/// `locate` returns, and the first instruction without any source line that
/// `declared` does not place in a probe declaration.
fn attribute(
    executed: &[u32],
    root: &Path,
    mut locate: impl FnMut(u32) -> Result<Vec<PathBuf>>,
    declared: impl Fn(u32) -> bool,
) -> Result<(BTreeSet<PathBuf>, Option<u32>)> {
    let mut files = BTreeSet::new();
    let mut unattributed = None;
    for &pc in executed {
        let paths = locate(pc)?;
        if paths.is_empty() && !declared(pc) {
            unattributed.get_or_insert(pc);
        }
        files.extend(paths.iter().filter_map(|path| relative(path, root)));
    }
    Ok((files, unattributed))
}

/// The address ranges of the functions the image's probe catalog declares.
fn declared_ranges(file: &oer_elf::Elf<'_>) -> Result<Vec<(u64, u64)>> {
    let Some(section) = file.section_by_name(oer_probe_codegen::SECTION) else {
        return Ok(vec![]);
    };
    let catalog: oer_probe_codegen::Catalog = serde_json::from_slice(section.data)?;
    let names: BTreeSet<&str> = catalog.entries.iter().map(|e| e.symbol.as_str()).collect();
    Ok(file
        .symbols()
        .filter(|s| s.kind == oer_elf::SymbolKind::Text && s.size > 0)
        .filter(|s| names.contains(s.name))
        .map(|s| (s.address, s.address + s.size))
        .collect())
}

/// SHA-256 of `regions` as a sorted multiset: each length-prefixed.
pub fn content_digest(mut regions: Vec<Vec<u8>>) -> String {
    regions.sort();
    let mut hash = Sha256::new();
    for region in &regions {
        hash.update((region.len() as u64).to_le_bytes());
        hash.update(region);
    }
    format!("{:x}", hash.finalize())
}

/// The dependencies of `executed` instructions and `reads` of ordinary
/// memory in probe image `elf`, attributed within repository `root`.
pub fn of(
    elf: &[u8],
    executed: &BTreeSet<u32>,
    reads: &BTreeSet<(u32, u8)>,
    root: &Path,
) -> Result<Dependencies> {
    let root = root.canonicalize()?;
    let file = oer_elf::Elf::parse(elf)?;
    let dwarf = oer_elf::dwarf::load(&file)?;
    let mut result = Dependencies::default();
    let mut fallback = |reason: String| {
        if result.fallback.is_none() {
            result.fallback = Some(reason);
        }
    };
    let symbolizer = Symbolizer::new(elf)?;
    // Only the probe's own code: the executions also run the linked vendor
    // image, whose instructions are not production's.
    let text: Vec<(u64, u64)> = file
        .sections()
        .filter(|s| s.executable && !s.nobits)
        .map(|s| (s.address, s.address + s.size))
        .collect();
    let probe: Vec<u32> = executed
        .iter()
        .copied()
        .filter(|pc| {
            text.iter()
                .any(|(s, e)| *s <= u64::from(*pc) && u64::from(*pc) < *e)
        })
        .collect();
    // A probe declaration, such as a naked assembly entry, may carry no
    // line information; the probe crates are in the evidence's global
    // group, so its instructions need no file of their own.
    let declared = declared_ranges(&file)?;
    let in_declaration = |pc: u32| {
        declared
            .iter()
            .any(|(s, e)| *s <= u64::from(pc) && u64::from(pc) < *e)
    };
    let (files, unattributed) = attribute(
        &probe,
        &root,
        |pc| {
            Ok(symbolizer
                .frames(u64::from(pc))
                .map_err(|e| invalid(format!("debug information at {pc:#x}: {e}")))?
                .into_iter()
                .filter_map(|frame| frame.file.map(PathBuf::from))
                .collect())
        },
        in_declaration,
    )?;
    result.files = files;
    if let Some(pc) = unattributed {
        fallback(format!("executed instruction {pc:#x} has no source line"));
    }
    let declarations = declarations(&dwarf)?;
    let named: Vec<(u64, u64, String)> = file
        .symbols()
        .filter(|s| s.kind == oer_elf::SymbolKind::Data && s.size > 0)
        // Compiler-generated anonymous constants carry no declaration: the
        // content digest stands for them.
        .filter(|s| !(s.name.starts_with(".L") || s.name.starts_with("anon.")))
        .map(|s| (s.address, s.address + s.size, s.name.to_owned()))
        .collect();
    // Initialized data: allocated, neither code nor thread-local, with bytes.
    let data: Vec<(u64, u64, &[u8])> = file
        .sections()
        .filter(|s| s.allocated && !s.executable && !s.tls && !s.nobits)
        .map(|s| (s.address, s.address + s.size, s.data))
        .collect();
    let mut regions = vec![];
    for (start, end) in merge(reads) {
        let Some((base, _, bytes)) = data.iter().find(|(s, e, _)| *s <= start && end <= *e) else {
            continue;
        };
        let offset = (start - base) as usize;
        let Some(region) = bytes.get(offset..offset + (end - start) as usize) else {
            continue;
        };
        regions.push(region.to_vec());
        for (symbol_start, symbol_end, name) in &named {
            if *symbol_start < end && start < *symbol_end {
                match declarations.get(symbol_start) {
                    Some(path) => {
                        if let Some(path) = relative(path, &root) {
                            result.files.insert(path);
                        }
                    }
                    None => fallback(format!("read data object {name} has no declaration file")),
                }
            }
        }
    }
    result.read_data = content_digest(regions);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_read_data_digest_depends_on_content_not_placement() {
        let a = content_digest(vec![vec![1, 2], vec![3]]);
        assert_eq!(a, content_digest(vec![vec![3], vec![1, 2]]));
        // A constant table another file defines changes only its bytes.
        assert_ne!(a, content_digest(vec![vec![1, 9], vec![3]]));
        assert_ne!(a, content_digest(vec![vec![1], vec![2, 3]]));
    }

    #[test]
    fn an_instruction_without_a_source_line_is_reported_for_fallback() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("crates/a")).unwrap();
        std::fs::write(root.join("crates/a/lib.rs"), "").unwrap();
        let locate = |pc| {
            Ok(match pc {
                0x10 => vec![root.join("crates/a/lib.rs")],
                // A registry crate outside the repository.
                0x14 => vec![PathBuf::from("/cargo/registry/core/src/lib.rs")],
                _ => vec![],
            })
        };
        let (files, unattributed) =
            attribute(&[0x10, 0x14, 0x18, 0x1c], &root, locate, |_| false).unwrap();
        assert_eq!(files, BTreeSet::from([PathBuf::from("crates/a/lib.rs")]));
        assert_eq!(unattributed, Some(0x18));
        // A line-less probe declaration (a naked entry) is the probe
        // crates', which the global group covers.
        let (_, unattributed) =
            attribute(&[0x10, 0x18, 0x1c], &root, locate, |pc| pc == 0x18).unwrap();
        assert_eq!(unattributed, Some(0x1c));
    }

    #[test]
    fn overlapping_and_adjacent_reads_merge() {
        let reads = BTreeSet::from([(0x10, 4), (0x12, 4), (0x16, 2), (0x20, 1)]);
        assert_eq!(merge(&reads), [(0x10, 0x18), (0x20, 0x21)]);
    }
}
