//! Each recorded frame, reachable or not, is bounded by the move limit.
//! Missing records belong to the separate coverage check.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use oer_chip_profile::policy::{FrameException, StackPolicy};

use super::{Image, Result};
use crate::{CheckedElf, interrupts::Required};

pub const RUNTIME_REPORT: &str = "runtime-frames.txt";
pub const BOOTSTRAP_REPORT: &str = "bootstrap-frames.txt";

/// One ELF's recorded frames and the exceptions that were actually applied.
pub struct FrameAudit<'a> {
    limit: u64,
    measured: usize,
    above: Vec<(String, u64, bool)>,
    exceptions: Vec<(&'a FrameException, u64)>,
    errors: Vec<String>,
}

impl<'a> FrameAudit<'a> {
    pub fn of(image: &Image, policy: &'a StackPolicy, elf: CheckedElf) -> Self {
        let exceptions = match elf {
            CheckedElf::Runtime => &policy.frame_exceptions.runtime,
            CheckedElf::Bootstrap => &policy.frame_exceptions.bootstrap,
        };
        let mut names: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
        for function in &image.functions {
            for alias in &function.names {
                names
                    .entry(oer_analysis_elf::demangle(alias))
                    .or_default()
                    .insert(function.address);
            }
        }
        let mut audit = Self {
            limit: policy.max_move_bytes,
            measured: image.stack_sizes.len(),
            above: Vec::new(),
            exceptions: Vec::new(),
            errors: Vec::new(),
        };
        let mut excepted = BTreeSet::new();
        for exception in exceptions {
            let addresses = names.get(&exception.function);
            let count = addresses.map_or(0, BTreeSet::len);
            let Some(&address) = addresses
                .filter(|addresses| addresses.len() == 1)
                .and_then(BTreeSet::first)
            else {
                audit.errors.push(format!(
                    "frame exception `{}` matches {count} functions (expected exactly one)",
                    exception.function
                ));
                continue;
            };
            let Some(&bytes) = image.stack_sizes.get(&address) else {
                audit.errors.push(format!(
                    "frame exception `{}` has no compiler frame record to except",
                    exception.function
                ));
                continue;
            };
            audit.exceptions.push((exception, bytes));
            if bytes <= audit.limit {
                audit.errors.push(format!(
                    "stale frame exception `{}`: frame {bytes} bytes is at or below the {}-byte limit; remove the exception",
                    exception.function, audit.limit
                ));
            }
            if bytes > exception.ceiling_bytes {
                audit.errors.push(format!(
                    "frame `{}` is {bytes} bytes, above its exception ceiling of {} bytes",
                    exception.function, exception.ceiling_bytes
                ));
            }
            if !excepted.insert(address) {
                audit.errors.push(format!(
                    "multiple frame exceptions name the same implementation: `{}`",
                    exception.function
                ));
            }
        }
        for (&address, &bytes) in &image.stack_sizes {
            if bytes > audit.limit {
                let name = image
                    .names
                    .get(&address)
                    .cloned()
                    .unwrap_or_else(|| format!("{address:#010x}"));
                audit.above.push((name, bytes, excepted.contains(&address)));
            }
        }
        audit
    }

    /// Diagnostic relaxes only unexcepted frames. Invalid exceptions and
    /// exceeded ceilings fail under either policy.
    pub fn check(&self, required: Required) -> Result<Vec<String>> {
        let mut errors = self.errors.clone();
        let mut warnings = Vec::new();
        for (name, bytes, excepted) in &self.above {
            if !excepted {
                let problem = format!(
                    "frame `{name}` is {bytes} bytes, above the {}-byte move limit",
                    self.limit
                );
                match required {
                    Required::Product => errors.push(problem),
                    Required::Diagnostic => warnings.push(problem),
                }
            }
        }
        if errors.is_empty() {
            Ok(warnings)
        } else {
            Err(errors.join("\n").into())
        }
    }

    pub fn render(&self) -> String {
        let mut out = format!(
            "Function frames (bytes): limit {}, recorded {}, above limit {}\nframe\tstatus\tfunction\n",
            self.limit,
            self.measured,
            self.above.len()
        );
        for (name, bytes, excepted) in &self.above {
            let status = if *excepted { "exception" } else { "over limit" };
            let _ = writeln!(out, "{bytes}\t{status}\t{name}");
        }
        out.push_str("\nFrame exceptions (bytes)\nframe\tceiling\tspare\tfunction\treason\n");
        for (exception, bytes) in &self.exceptions {
            let spare = i128::from(exception.ceiling_bytes) - i128::from(*bytes);
            let _ = writeln!(
                out,
                "{bytes}\t{}\t{spare}\t{}\t{}",
                exception.ceiling_bytes, exception.function, exception.reason
            );
        }
        for error in &self.errors {
            let _ = writeln!(out, "error: {error}");
        }
        out
    }
}

pub(crate) fn audit(
    image: &Image,
    policy: &StackPolicy,
    elf: CheckedElf,
    required: Required,
    output: &std::path::Path,
) -> Result<Vec<String>> {
    let audit = FrameAudit::of(image, policy, elf);
    let report = match elf {
        CheckedElf::Runtime => RUNTIME_REPORT,
        CheckedElf::Bootstrap => BOOTSTRAP_REPORT,
    };
    let result = audit.check(required);
    let mut report_text = audit.render();
    match &result {
        Ok(warnings) => {
            for warning in warnings {
                let _ = writeln!(report_text, "warning: {warning}");
            }
        }
        Err(error) => {
            let _ = writeln!(report_text, "failed: {error}");
        }
    }
    std::fs::write(output.join(report), report_text)?;
    result
}
