//! Reports compared pairs with no claim and inspects pinned vendor code.
pub mod command;
pub mod inspect;

use crate::findings::{Findings, RunReport};
use crate::harness::Result;

#[derive(Default)]
pub struct Reviewer;

impl RunReport for Reviewer {
    fn report(&self, findings: &Findings<'_>) -> Result<()> {
        println!(
            "{}",
            serde_json::json!({
                "schema": 1, "analysis_resources": findings.analysis_resources, "suite": findings.suite,
            })
        );
        for (vendor, production) in findings.unclaimed {
            println!(
                "{} compared pair {vendor} -> {production} has no claim",
                findings.suite
            );
        }
        Ok(())
    }
}
