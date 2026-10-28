//! Pinned vendor inspection presented from Blobray operations.
use crate::{
    harness::{Result, invalid},
    registers::Registers,
};
use oer_analysis_blobray_application as app;
use oer_analysis_blobray_domain::{ImageRegion, LibraryFunction, RegisterAccess};
use oer_analysis_riscv_lift::RiscvDecoder;
use oer_analysis_riscv_model::{
    AnalysisLimited, AnalysisProfile, CallAbi, FunctionRecord, MemoryKind, WorkingMemory,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
};
mod corpus;
mod format;
mod query;
pub use corpus::Corpus;
use corpus::Metadata;
pub use query::{fields, prints, show, xref};

pub const WORD: u32 = 4;

struct Budget {
    memory: WorkingMemory,
    control: app::in_process::Limits,
}
impl Budget {
    fn new() -> Result<Self> {
        let policy = AnalysisProfile::BlobrayCli.limits();
        Ok(Self {
            memory: WorkingMemory::new(policy.working_memory_bytes)?,
            control: app::in_process::Limits::new(
                policy.work_units,
                std::time::Duration::from_millis(policy.timeout_ms),
            ),
        })
    }
    fn report(&self, operation: &str, limited: AnalysisLimited) -> Result<()> {
        let profile = AnalysisProfile::BlobrayCli;
        let resources = oer_analysis_riscv_model::AnalysisResources::new(
            profile,
            profile.limits(),
            oer_analysis_riscv_model::AnalysisUsage {
                work_units: self.control.used(),
                peak_admitted_bytes: self.memory.peak(),
            },
            limited,
        );
        eprintln!(
            "{}",
            serde_json::json!({"operation": operation, "analysis_resources": resources})
        );
        Ok(())
    }
}

fn assumptions(names: &[String]) -> app::library::Assumptions<'_> {
    app::library::Assumptions {
        functions: names,
        abi: Some(CallAbi::RiscvInteger),
        call_summaries: true,
        ..Default::default()
    }
}

pub fn parse_address(text: &str) -> std::result::Result<u32, String> {
    let parsed = match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => u32::from_str_radix(&hex.replace('_', ""), 16),
        None => text.replace('_', "").parse(),
    };
    parsed.map_err(|error| format!("{text}: {error}"))
}

pub fn parse_offset(text: &str) -> std::result::Result<i32, String> {
    let (negative, magnitude) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let value =
        i32::try_from(parse_address(magnitude)?).map_err(|error| format!("{text}: {error}"))?;
    Ok(if negative { -value } else { value })
}

pub fn load() -> Result<Corpus> {
    let corpus = Corpus::load(&oer_os_process::built_root())?;
    for id in &corpus.missing {
        eprintln!("{id}: not in this checkout, skipped");
    }
    Ok(corpus)
}

/// The Blobray executable `cargo vendor-check scenario` passes to every
/// command; inspection reads the pinned artifacts itself and ignores it.
#[derive(clap::Args)]
pub struct Runner {
    #[arg(long, hide = true)]
    binary: Option<std::path::PathBuf>,
}

/// The reviewer commands a scenario binary offers beside its scenarios.
#[derive(clap::Subcommand)]
pub enum Command {
    /// Every read and write of the pinned vendor code to the addresses from
    /// `start` up to `end`, one word when `end` is omitted, with the bits each
    /// store clears, sets or takes from a computed value.
    Xref {
        #[arg(value_parser = parse_address)]
        start: u32,
        #[arg(value_parser = parse_address)]
        end: Option<u32>,
        #[command(flatten)]
        runner: Runner,
    },
    /// One pinned vendor function, annotated, from every artifact defining it.
    Show {
        function: String,
        #[command(flatten)]
        runner: Runner,
    },
    /// Every read and write of the pinned vendor code to a structure field
    /// reached through pointers whose last offsets are `offsets`, from any
    /// argument or symbol: `0x34 0` is the word at offset 0 of the pointer
    /// stored at offset 0x34.
    Fields {
        #[arg(required = true, allow_hyphen_values = true, value_parser = parse_offset)]
        offsets: Vec<i32>,
        #[command(flatten)]
        runner: Runner,
    },
    /// Every print of the pinned vendor code that passes bits of the
    /// addresses from `start` up to `end` (one word when omitted) to a
    /// format conversion, with the format text up to that conversion.
    Prints {
        #[arg(value_parser = parse_address)]
        start: u32,
        #[arg(value_parser = parse_address)]
        end: Option<u32>,
        #[command(flatten)]
        runner: Runner,
    },
}

pub fn run(command: Command) -> Result<()> {
    let corpus = load()?;
    let text = match command {
        Command::Xref { start, end, .. } => {
            xref(&corpus, start, end.unwrap_or(start.saturating_add(WORD)))?
        }
        Command::Prints { start, end, .. } => {
            prints(&corpus, start, end.unwrap_or(start.saturating_add(WORD)))?
        }
        Command::Fields { offsets, .. } => fields(&corpus, &offsets)?,
        Command::Show { function, .. } => show(&corpus, &function)?
            .ok_or_else(|| invalid(format!("no pinned artifact defines {function}")))?,
    };
    print!("{text}");
    Ok(())
}

#[cfg(test)]
mod tests;
