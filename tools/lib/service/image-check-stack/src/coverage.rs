//! Compiler frame coverage: every function of an image (`oer-riscv-stack`'s
//! inventory) either has a `.stack_sizes` record or exactly one reviewed
//! reason why not. A review explains a missing record; it never supplies a
//! frame, so the bound of a root that reaches such a function still names it
//! when its machine code does not bound it either.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use oer_riscv_stack::Function;
use serde::Deserialize;

use crate::Result;

/// The coverage review file (`stack-coverage.toml`, schema 1).
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoveragePolicy {
    pub schema: u32,
    pub reviewed: Vec<CoverageReview>,
}

/// Exact symbols without a frame record, and why.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageReview {
    pub symbols: Vec<String>,
    pub category: CoverageCategory,
    pub source: String,
    pub reason: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum CoverageCategory {
    Assembly,
    VectorData,
    CompilerRuntime,
    /// A linker script's section boundary in executable memory: a global
    /// untyped label, which the stack analysis takes for a function.
    LinkerLabel,
}

/// The frame coverage of one image.
#[derive(Clone, Debug, Default)]
pub struct Coverage {
    /// Functions of the image, aliases counted once.
    pub functions: usize,
    /// Of those, the ones with a `.stack_sizes` record.
    pub measured: usize,
    /// Each unmeasured function's address, alias and the review that
    /// explains it.
    pub reviewed: Vec<(u32, String, CoverageCategory)>,
    /// Each unmeasured alias without exactly one review.
    pub unreviewed: Vec<String>,
}

impl CoveragePolicy {
    pub fn load(path: &Path) -> Result<Self> {
        let source = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let policy: Self =
            toml::from_str(&source).map_err(|error| format!("{}: {error}", path.display()))?;
        if policy.schema != 1
            || policy.reviewed.iter().any(|review| {
                review.symbols.is_empty()
                    || review.symbols.iter().any(|symbol| symbol.trim().is_empty())
                    || review.source.trim().is_empty()
                    || review.reason.trim().is_empty()
            })
        {
            return Err(format!(
                "{}: a coverage review needs schema 1, exact symbols, a source and a reason",
                path.display()
            )
            .into());
        }
        let mut names = BTreeSet::new();
        for review in &policy.reviewed {
            for symbol in &review.symbols {
                if !names.insert(without_crate_disambiguators(symbol)) {
                    return Err(format!("overlapping stack coverage review: {symbol}").into());
                }
            }
        }
        Ok(policy)
    }

    /// The coverage of `functions` by the frame records `sizes`. An image
    /// without any record fails: it was not compiled with
    /// `-Z emit-stack-sizes`.
    pub fn coverage(&self, functions: &[Function], sizes: &BTreeMap<u32, u64>) -> Result<Coverage> {
        if sizes.is_empty() {
            return Err(
                "the image has no `.stack_sizes` records; compile it with `-Z emit-stack-sizes`"
                    .into(),
            );
        }
        let mut coverage = Coverage {
            functions: functions.len(),
            ..Coverage::default()
        };
        for function in functions {
            if sizes.contains_key(&function.address) {
                coverage.measured += 1;
                continue;
            }
            for raw in &function.names {
                let name = demangled(raw);
                let stable = without_crate_disambiguators(&name);
                let reviews: Vec<&CoverageReview> = self
                    .reviewed
                    .iter()
                    .filter(|review| {
                        review
                            .symbols
                            .iter()
                            .any(|symbol| without_crate_disambiguators(symbol) == stable)
                    })
                    .collect();
                match reviews.as_slice() {
                    [review] => coverage
                        .reviewed
                        .push((function.address, name, review.category)),
                    _ => coverage.unreviewed.push(format!(
                        "{name} at {:#010x}: {} coverage reviews (expected one)",
                        function.address,
                        reviews.len()
                    )),
                }
            }
        }
        Ok(coverage)
    }
}

/// A symbol as reviews name it: demangled with its crate disambiguators.
fn demangled(raw: &str) -> String {
    rustc_demangle::try_demangle(raw)
        .map(|name| name.to_string())
        .unwrap_or_else(|_| raw.to_owned())
}

/// `name` without its v0 crate disambiguators: an unpadded hexadecimal
/// number of up to 16 digits in brackets after a crate name, which changes
/// with the consumer's build. The full definition, type arguments, closure
/// identity and const parameters stay.
pub(crate) fn without_crate_disambiguators(name: &str) -> String {
    let mut result = String::new();
    let mut rest = name;
    while let Some(open) = rest.find('[') {
        result.push_str(&rest[..open]);
        rest = &rest[open..];
        let disambiguator_end = rest.find(']').filter(|end| {
            (2..=17).contains(end)
                && rest.as_bytes()[1..*end].iter().all(u8::is_ascii_hexdigit)
                && rest[*end + 1..].starts_with("::")
                && result
                    .as_bytes()
                    .last()
                    .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
        });
        if let Some(end) = disambiguator_end {
            rest = &rest[end + 1..];
        } else {
            result.push('[');
            rest = &rest[1..];
        }
    }
    result.push_str(rest);
    result
}
