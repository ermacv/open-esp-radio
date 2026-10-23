//! The inventory run behind `cargo registers check inventory`: Blobray's
//! `register-accesses`, under the RISC-V integer calling convention and
//! with the constants callees return taken at their calls, over
//! every pinned vendor binary inside the publication's owned MMIO ranges,
//! compared with the register model ([`super`]). Reports are written before
//! returning an error for used reserved or absent bits; unknown bits remain separate.
use super::{self as inventory, Region};
use crate::Result;
use oer_analysis_blobray_application::{
    in_process::{Executable, Limits},
    library::{self, Assumptions},
};
use oer_analysis_blobray_domain::ImageRegion;
use oer_analysis_riscv_lift::RiscvDecoder;
use oer_analysis_riscv_model::{
    AnalysisProfile, AnalysisResources, AnalysisUsage, CallAbi, Error, ErrorCode, WorkingMemory,
};
use oer_os_process::Checkout;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

/// Every pinned vendor binary of `chip`, in pin order. Firmware outputs are
/// applications linked from these binaries, and sources are not code.
fn inputs(ctx: &Checkout, chip: &str) -> Result<(Vec<inventory::uses::Input>, Vec<Executable>)> {
    let mut inputs = Vec::new();
    let mut executables = Vec::new();
    for pinned in oer_tool_pins::artifacts::pinned(&ctx.root, chip)? {
        if pinned.firmware {
            continue;
        }
        let bytes = fs::read(&pinned.path)?;
        if !oer_analysis_elf::is_binary(&bytes) {
            continue;
        }
        let executable = Executable::new(bytes);
        inputs.push(inventory::uses::Input {
            role: pinned.id,
            sha256: executable.id().clone(),
        });
        executables.push(executable);
    }
    if inputs.is_empty() {
        return Err(format!("no pinned vendor binary for {chip}").into());
    }
    Ok((inputs, executables))
}

/// Analyze typed accesses and fold them into words as each record arrives.
fn accesses(
    chip: &str,
    signatures: &oer_analysis_blobray_application::signatures::Signatures,
    inputs: &[inventory::uses::Input],
    executables: &[Executable],
    regions: &[Region],
    declared: &std::collections::BTreeMap<u32, inventory::DeclaredWord>,
    arrays: &[Vec<u32>],
) -> Result<(inventory::uses::Report, AnalysisResources)> {
    eprintln!(
        "register-inventory: analyzing every function of {} vendor binaries",
        inputs.len()
    );
    // The same assumptions and resource bounds as the CLI invocation:
    // --abi riscv-integer --call-summaries --timeout-secs 3600.
    let assumptions = Assumptions {
        functions: &[],
        symbolic_stores: false,
        abi: Some(CallAbi::RiscvInteger),
        call_summaries: true,
        signatures: Some(signatures),
        read_only: &[],
        call_results: &[],
    };
    let ranges: Vec<_> = regions
        .iter()
        .map(|r| ImageRegion {
            start: r.start,
            length: u64::from(r.end - r.start),
        })
        .collect();
    let profile = AnalysisProfile::RegisterInventory;
    let limits = profile.limits();
    let memory = WorkingMemory::new(limits.working_memory_bytes)?;
    let mut control = Limits::new(limits.work_units, Duration::from_millis(limits.timeout_ms));
    let mut check =
        inventory::uses::Accumulator::new(chip, inputs, assumptions, regions, declared, arrays)?;
    let (summary, limited) = library::register_accesses(
        executables,
        assumptions,
        &ranges,
        &RiscvDecoder,
        &memory,
        &mut control,
        &mut |record, _| {
            check
                .observe(record)
                .map_err(|error| Error::new(ErrorCode::InvalidRequest, error.to_string()))
        },
    )?;
    let resources = AnalysisResources::new(
        profile,
        limits,
        AnalysisUsage {
            work_units: control.used(),
            peak_admitted_bytes: memory.peak(),
        },
        limited,
    );
    Ok((check.finish(summary), resources))
}

/// Build the inventory of `chip` into `output`, by default
/// `target/register-inventory/<chip>`.
pub fn run(ctx: &Checkout, chip: &str, output: Option<PathBuf>) -> Result<()> {
    oer_tool_pins::artifacts::project::Project::new(&ctx.root, chip)?;
    let directory = output.unwrap_or_else(|| ctx.root.join("target/register-inventory").join(chip));
    fs::create_dir_all(&directory)?;
    let (inputs, executables) = inputs(ctx, chip)?;
    let (registers, regions) = inventory::model(&ctx.root, chip)?;
    let signatures =
        oer_analysis_blobray_application::signatures::Signatures::parse(&fs::read_to_string(
            ctx.root
                .join("chips")
                .join(chip)
                .join("vendor/facts/idf-function-signatures.toml"),
        )?)?;
    let (mut report, resources) = accesses(
        chip,
        &signatures,
        &inputs,
        &executables,
        &regions,
        &inventory::declared_words(&registers),
        &inventory::arrays(&registers),
    )?;
    let cited: BTreeSet<inventory::Function> =
        oer_tool_pins::provenance::registered(&ctx.root, chip)?
            .into_iter()
            .map(|entry| (entry.artifact, entry.symbol))
            .collect();
    report.prioritize(&cited);
    write_report(&directory, chip, &report, &resources)
}

pub(super) fn write_report(
    directory: &Path,
    chip: &str,
    report: &inventory::uses::Report,
    resources: &AnalysisResources,
) -> Result<()> {
    let text = report.render(chip);
    fs::write(directory.join("report.txt"), &text)?;
    fs::write(
        directory.join("report.json"),
        serde_json::to_string_pretty(
            &serde_json::json!({"schema": 1, "analysis_resources": resources, "chip": chip, "check": report}),
        )?,
    )?;
    print!(
        "{}",
        text.lines()
            .take(5)
            .map(|l| format!("{l}\n"))
            .collect::<String>()
    );
    println!("report: {}", directory.join("report.txt").display());
    if !report.corpus.summary.signature_conflicts.is_empty() {
        return Err(format!(
            "{} signature conflicts; full report: {}",
            report.corpus.summary.signature_conflicts.len(),
            directory.join("report.json").display()
        )
        .into());
    }
    if !report.corpus.summary.signature_audit_incomplete.is_empty() {
        return Err(format!(
            "{} incomplete signature audits; full report: {}",
            report.corpus.summary.signature_audit_incomplete.len(),
            directory.join("report.json").display()
        )
        .into());
    }
    if report.violations() != 0 {
        return Err(format!(
            "vendor uses reserved or absent bits in {} MMIO words; see {}",
            report.violations(),
            directory.join("report.txt").display()
        )
        .into());
    }
    Ok(())
}
