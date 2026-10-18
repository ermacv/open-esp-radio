//! Compare two linked RISC-V images function by function, modulo placement.
//!
//! Symbols and sections come from [`oer_elf`]; each function's listing comes
//! from [`oer_riscv_lift::listing`], which decodes the instructions and
//! replaces every address they form by the location the image names.
//!
//! Moving code between crates or adding a crate changes symbol hashes and,
//! under fat LTO, the order functions are laid out in, so the bytes of every
//! section differ even when no function's code changed. This comparison
//! replaces every address an instruction forms (branch and jump targets,
//! `auipc`/`lui` pairs with their use, `.word` data and pointers) by the
//! containing symbol and offset, drops legacy mangling hashes, and compares
//! each function's normalized instruction list by name. Identical-code-folded
//! functions, which keep either of their names, are unified through reviewed
//! aliases; reviewed scheduling ties are listed explicitly. It is the gate for
//! pure code moves between crates; `cargo fw compare` calls
//! [`compare_elf`] and `compare images` builds both sides with the HIL image
//! classes first (`oer_hil_image::compare_images`).
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::Path,
};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// One named symbol of an image.
#[derive(Clone, Debug)]
struct Symbol {
    address: u64,
    size: u64,
    name: String,
}

/// Allocated section contents by start address.
struct Sections(Vec<(u64, u64)>);

impl Sections {
    fn of(elf: &oer_elf::Elf<'_>) -> Self {
        Self(
            elf.sections()
                .filter(|section| section.allocated && !section.nobits)
                .map(|section| (section.address, section.size))
                .collect(),
        )
    }

    fn contains(&self, address: u64) -> bool {
        self.0
            .iter()
            .any(|(start, size)| *start <= address && address - start < *size)
    }
}

/// What a reviewer supplies to a comparison.
#[derive(Clone, Debug, Default)]
pub struct Review {
    pub aliases: Aliases,
    /// Functions whose code may differ (reviewed scheduling ties).
    pub allowed: BTreeSet<String>,
    /// Print the instruction diff of every differing or one-sided function
    /// whose name contains one of these.
    pub show: Vec<String>,
}

/// Reviewed renames applied to every symbol name before comparison.
#[derive(Clone, Debug, Default)]
pub struct Aliases(pub Vec<(String, String)>);

fn clean(name: &str, aliases: &Aliases) -> String {
    let mut name = name.to_owned();
    if let Some(index) = name.rfind("::h")
        && name.len() == index + 19
        && name[index + 3..].bytes().all(|b| b.is_ascii_hexdigit())
    {
        name.truncate(index);
    }
    // LLVM numbers local clones (`.123`, `.llvm.456`) per build; the
    // numbering is not part of the code, so clones share a name and are
    // compared as a sorted set of bodies.
    // The demangler shows an unrecognized suffix as `name (.123)`.
    while let Some(stem) = name.strip_suffix(')')
        && let Some((stem, suffix)) = stem.rsplit_once(" (.")
        && suffix.split('.').all(|part| {
            part == "llvm" || (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        })
    {
        name.truncate(stem.len());
    }
    while let Some((stem, number)) = name.rsplit_once('.')
        && !number.is_empty()
        && number.bytes().all(|b| b.is_ascii_digit())
    {
        name.truncate(stem.strip_suffix(".llvm").unwrap_or(stem).len());
    }
    for (from, to) in &aliases.0 {
        name = name.replace(from.as_str(), to);
    }
    name
}

/// Whether a symbol names code or data: local labels (`.L`) and RISC-V
/// mapping symbols (`$x`, `$d`), which mark code and data runs at every
/// function, name nothing and must not stand for a folded address.
fn names_a_location(name: &str) -> bool {
    !name.is_empty() && !name.starts_with(".L") && !name.starts_with('$')
}

struct Image {
    symbols: Vec<Symbol>,
    starts: Vec<u64>,
    sections: Sections,
}

impl Image {
    fn of(elf: &oer_elf::Elf<'_>, aliases: &Aliases) -> Self {
        let mut symbols = elf
            .symbols()
            .filter(|symbol| {
                symbol.defined
                    && names_a_location(symbol.name)
                    && !matches!(
                        symbol.kind,
                        oer_elf::SymbolKind::Section | oer_elf::SymbolKind::File
                    )
            })
            .map(|symbol| Symbol {
                address: symbol.address,
                size: symbol.size,
                name: clean(&symbol.demangled(), aliases),
            })
            .collect::<Vec<_>>();
        symbols.sort_by(|a, b| a.address.cmp(&b.address).then(a.name.cmp(&b.name)));
        // Folded functions share an address; the smallest name stands for all.
        let mut canonical: HashMap<u64, String> = HashMap::new();
        for symbol in &symbols {
            canonical
                .entry(symbol.address)
                .or_insert_with(|| symbol.name.clone());
        }
        for symbol in &mut symbols {
            symbol.name = canonical[&symbol.address].clone();
        }
        let starts = symbols.iter().map(|s| s.address).collect();
        Self {
            symbols,
            starts,
            sections: Sections::of(elf),
        }
    }
}

impl oer_riscv_lift::listing::Placement for Image {
    fn locate(&self, address: u64) -> String {
        let mut index = self.starts.partition_point(|start| *start <= address);
        while index > 0 {
            index -= 1;
            let symbol = &self.symbols[index];
            if symbol.address <= address && address < symbol.address + symbol.size.max(1) {
                return format!("{}+{:#x}", symbol.name, address - symbol.address);
            }
            if symbol.size != 0 {
                break;
            }
        }
        if self.sections.contains(address) {
            "data".into()
        } else {
            format!("?{address:#x}")
        }
    }

    fn contains(&self, address: u64) -> bool {
        self.sections.contains(address)
    }
}

/// Normalized instruction lists of every function, by name. A name that
/// occurs in several places keeps each body.
fn functions(elf: &oer_elf::Elf<'_>, image: &Image) -> Result<BTreeMap<String, Vec<Vec<String>>>> {
    use oer_riscv_lift::listing::{Placement, normalize};
    let mut functions: BTreeMap<String, Vec<Vec<String>>> = BTreeMap::new();
    for function in elf.functions()? {
        let section = elf.section(function.section)?;
        let start = usize::try_from(u64::from(function.address) - section.address)?;
        let bytes = section
            .data
            .get(start..start + function.size as usize)
            .ok_or_else(|| format!("{} exceeds its section", function.label()))?;
        let name = image.locate(u64::from(function.address));
        functions
            .entry(name.trim_end_matches("+0x0").to_owned())
            .or_default()
            .push(normalize(u64::from(function.address), bytes, image));
    }
    for bodies in functions.values_mut() {
        bodies.sort();
    }
    Ok(functions)
}

/// The outcome of one comparison.
#[derive(Debug, Default)]
pub struct Comparison {
    pub common: usize,
    pub differing: Vec<String>,
    pub only_old: Vec<String>,
    pub only_new: Vec<String>,
    /// One-image names whose bodies match a one-image name of the other side.
    pub paired_by_body: usize,
}

impl Comparison {
    pub fn equivalent(&self, allowed: &BTreeSet<String>) -> bool {
        self.differing.iter().all(|name| allowed.contains(name))
            && self.only_old.len() == self.paired_by_body
            && self.only_new.len() == self.paired_by_body
    }
}

fn compare_functions(
    old: &BTreeMap<String, Vec<Vec<String>>>,
    new: &BTreeMap<String, Vec<Vec<String>>>,
) -> Comparison {
    let names_old = old.keys().cloned().collect::<BTreeSet<_>>();
    let names_new = new.keys().cloned().collect::<BTreeSet<_>>();
    let common = names_old
        .intersection(&names_new)
        .cloned()
        .collect::<Vec<_>>();
    let only_old = names_old
        .difference(&names_new)
        .cloned()
        .collect::<Vec<_>>();
    let only_new = names_new
        .difference(&names_old)
        .cloned()
        .collect::<Vec<_>>();
    let differing = common
        .iter()
        .filter(|name| old[*name] != new[*name])
        .cloned()
        .collect();
    let mut left = only_old.iter().map(|n| &old[n]).collect::<Vec<_>>();
    let mut right = only_new.iter().map(|n| &new[n]).collect::<Vec<_>>();
    left.sort();
    right.sort();
    let paired_by_body = left.iter().zip(&right).filter(|(a, b)| a == b).count();
    Comparison {
        common: common.len(),
        differing,
        only_old,
        only_new,
        paired_by_body,
    }
}

/// Compare two ELF images and print the result; fails unless every function
/// is equivalent or listed in `review.allowed`.
pub fn compare_elf(old: &Path, new: &Path, review: &Review) -> Result<Comparison> {
    let (old_bytes, new_bytes) = (std::fs::read(old)?, std::fs::read(new)?);
    let (old_elf, new_elf) = (
        oer_elf::Elf::executable(&old_bytes)?,
        oer_elf::Elf::executable(&new_bytes)?,
    );
    let image_old = Image::of(&old_elf, &review.aliases);
    let image_new = Image::of(&new_elf, &review.aliases);
    let old_functions = functions(&old_elf, &image_old)?;
    let new_functions = functions(&new_elf, &image_new)?;
    let comparison = compare_functions(&old_functions, &new_functions);
    println!(
        "{}: {} common functions, {} differ, {} only old, {} only new ({} paired by identical body)",
        new.display(),
        comparison.common,
        comparison.differing.len(),
        comparison.only_old.len(),
        comparison.only_new.len(),
        comparison.paired_by_body
    );
    for name in comparison.differing.iter().take(40) {
        let reviewed = if review.allowed.contains(name) {
            " (reviewed tie)"
        } else {
            ""
        };
        println!("  differs: {name}{reviewed}");
    }
    for name in comparison.only_old.iter().take(10) {
        println!("  only old: {name}");
    }
    for name in comparison.only_new.iter().take(10) {
        println!("  only new: {name}");
    }
    let shown = comparison
        .differing
        .iter()
        .chain(&comparison.only_old)
        .chain(&comparison.only_new)
        .filter(|name| review.show.iter().any(|pattern| name.contains(pattern)))
        .collect::<BTreeSet<_>>();
    let none = Vec::new();
    for name in shown {
        println!("--- {name}");
        let old = old_functions.get(name).unwrap_or(&none);
        let new = new_functions.get(name).unwrap_or(&none);
        for index in 0..old.len().max(new.len()) {
            let empty = Vec::new();
            let (old, new) = (
                old.get(index).unwrap_or(&empty),
                new.get(index).unwrap_or(&empty),
            );
            if old == new {
                continue;
            }
            if old.len().max(new.len()) > 1 {
                println!("  body {index}:");
            }
            for (mark, line) in diff(old, new) {
                println!("  {mark} {line}");
            }
        }
    }
    Ok(comparison)
}

/// A line diff by longest common subsequence: ' ' kept, '-' old, '+' new.
fn diff<'a>(old: &'a [String], new: &'a [String]) -> Vec<(char, &'a str)> {
    let mut common = vec![vec![0u32; new.len() + 1]; old.len() + 1];
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            common[i][j] = if old[i] == new[j] {
                common[i + 1][j + 1] + 1
            } else {
                common[i + 1][j].max(common[i][j + 1])
            };
        }
    }
    let (mut i, mut j, mut lines) = (0, 0, Vec::new());
    while i < old.len() || j < new.len() {
        if i < old.len() && j < new.len() && old[i] == new[j] {
            lines.push((' ', old[i].as_str()));
            i += 1;
            j += 1;
        } else if j < new.len() && (i == old.len() || common[i][j + 1] >= common[i + 1][j]) {
            lines.push(('+', new[j].as_str()));
            j += 1;
        } else {
            lines.push(('-', old[i].as_str()));
            i += 1;
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(lines: &[&str]) -> Vec<Vec<String>> {
        vec![lines.iter().map(|l| (*l).to_owned()).collect()]
    }

    #[test]
    fn legacy_hashes_are_dropped_and_aliases_applied() {
        let aliases = Aliases(vec![("oer_chip_a_pac::Mac".into(), "Mac".into())]);
        assert_eq!(
            clean("oer_chip_a_pac::Mac::set::h0123456789abcdef", &aliases),
            "Mac::set"
        );
        assert_eq!(clean("plain::h0123", &aliases), "plain::h0123");
    }

    #[test]
    fn llvm_clone_numbers_are_dropped() {
        let none = Aliases::default();
        assert_eq!(
            clean("<u8 as Debug>::fmt.1234", &none),
            "<u8 as Debug>::fmt"
        );
        assert_eq!(clean("inner.llvm.98765.12", &none), "inner");
        assert_eq!(clean("v1.2::name", &none), "v1.2::name");
        assert_eq!(clean("log::LOGGER (.4696)", &none), "log::LOGGER");
        assert_eq!(
            clean("descriptor_head (.llvm.12)", &none),
            "descriptor_head"
        );
        assert_eq!(clean("f (.cold)", &none), "f (.cold)");
    }

    #[test]
    fn labels_and_mapping_symbols_name_no_location() {
        assert!(names_a_location("core::fmt::write"));
        assert!(!names_a_location("$x"));
        assert!(!names_a_location("$d.12"));
        assert!(!names_a_location(".LBB0_1"));
        assert!(!names_a_location(""));
    }

    #[test]
    fn a_symbol_of_an_unsized_label_locates_the_addresses_after_it() {
        use oer_riscv_lift::listing::Placement;
        let symbol = |address, size, name: &str| Symbol {
            address,
            size,
            name: name.into(),
        };
        let symbols = vec![
            symbol(0x4000_0000, 0x20, "f"),
            symbol(0x4080_1000, 0, "LABEL"),
            symbol(0x4080_1000, 0x10, "LABEL"),
        ];
        let image = Image {
            starts: symbols.iter().map(|s| s.address).collect(),
            symbols,
            sections: Sections(vec![(0x4080_0000, 0x2000)]),
        };
        assert_eq!(image.locate(0x4000_0004), "f+0x4");
        assert_eq!(image.locate(0x4080_1004), "LABEL+0x4");
        assert_eq!(image.locate(0x4080_1800), "data");
        assert_eq!(image.locate(0x5000_0000), "?0x50000000");
    }

    #[test]
    fn a_diff_keeps_common_lines_and_marks_the_rest() {
        let lines = |text: &[&str]| text.iter().map(|l| (*l).to_owned()).collect::<Vec<_>>();
        let (old, new) = (lines(&["a", "b", "c"]), lines(&["a", "x", "c"]));
        assert_eq!(
            diff(&old, &new),
            [(' ', "a"), ('+', "x"), ('-', "b"), (' ', "c")]
        );
    }

    #[test]
    fn equivalence_allows_reviewed_ties_and_body_paired_renames() {
        let old = BTreeMap::from([
            ("a".to_owned(), body(&["addi a0,a0,1"])),
            ("tie".to_owned(), body(&["addi s8,s8,1", "addi s9,s9,-1"])),
            ("old_name".to_owned(), body(&["ret"])),
        ]);
        let new = BTreeMap::from([
            ("a".to_owned(), body(&["addi a0,a0,1"])),
            ("tie".to_owned(), body(&["addi s9,s9,-1", "addi s8,s8,1"])),
            ("new_name".to_owned(), body(&["ret"])),
        ]);
        let comparison = compare_functions(&old, &new);
        assert_eq!(comparison.differing, ["tie"]);
        assert_eq!(comparison.paired_by_body, 1);
        assert!(!comparison.equivalent(&BTreeSet::new()));
        assert!(comparison.equivalent(&BTreeSet::from(["tie".to_owned()])));
    }

    #[test]
    fn a_changed_body_is_not_equivalent() {
        let old = BTreeMap::from([("f".to_owned(), body(&["li a0,1"]))]);
        let new = BTreeMap::from([("f".to_owned(), body(&["li a0,2"]))]);
        assert!(!compare_functions(&old, &new).equivalent(&BTreeSet::new()));
    }
}
