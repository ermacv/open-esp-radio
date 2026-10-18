//! Reviewer reports of the typed vendor-comparison scenarios.
//!
//! Nothing here decides a verdict or a recorded set: the engine hands each
//! claimed suite's [`Findings`] to [`Reviewer`], which writes the untriaged
//! locations, the function view and the closure gateways beside the run and
//! prints the compared pairs no claim names. [`inspect`] reads pinned vendor
//! code without running a scenario. The dependency runs from this crate to
//! the engine only, so an edit here leaves the evidence shards current.
pub mod inspect;
pub mod triage;

use oer_vendor_scenario_engine::coverage;
use oer_vendor_scenario_engine::findings::{Findings, RunReport};
use oer_vendor_scenario_engine::harness::Result;
use oer_vendor_scenario_engine::registers::Registers;

/// The report every scenario binary passes to the engine.
#[derive(Default)]
pub struct Reviewer;

impl RunReport for Reviewer {
    fn report(&self, findings: &Findings<'_>) -> Result<()> {
        let suite = findings.suite;
        for (vendor, production) in findings.unclaimed {
            println!("{suite} compared pair {vendor} -> {production} has no claim");
        }
        if let Some(untriaged) = &findings.untriaged {
            // A reviewer's aid beside the run: each untriaged location's code.
            let code = triage::Code::of(
                &untriaged.images,
                Registers::load(
                    &oer_process::built_root().join(oer_vendor_scenario_engine::chip().registers),
                )?,
                coverage::diagnostic(untriaged.decisions),
            )?;
            let path = triage::write(findings.directory, suite, &code, untriaged.listed)?;
            println!("{suite} untriaged locations: {}", path.display());
            let view = triage::functions(
                &code,
                untriaged.listed,
                untriaged.uncovered,
                untriaged.consequential,
                |location| coverage::reason(untriaged.decisions, location),
            );
            let path = findings.directory.join(format!("functions-{suite}.txt"));
            std::fs::write(&path, view)?;
            println!("{suite} untriaged functions: {}", path.display());
        }
        if !findings.gateways.is_empty() {
            let path = findings.directory.join(format!("gateways-{suite}.txt"));
            std::fs::write(&path, findings.gateways.join("\n") + "\n")?;
            println!("{suite} closure gateways: {}", path.display());
        }
        Ok(())
    }
}
