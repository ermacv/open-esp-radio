//! Fixture preparation and restoration without accessing the device.
use std::path::Path;

use crate::Result;
use crate::scenario::{Families, Scenario};
use oer_hil_lab::config::LabConfig;
use oer_hil_workload::family::Registry as _;

pub(crate) fn check_without_device(
    root: &Path,
    lab: &LabConfig,
    scenario: &Scenario,
) -> Result<()> {
    let plan = scenario.plan();
    let resolved = lab.resolve(plan.wifi);
    let lab = &resolved;
    let required = plan.requirements;
    let _lease = oer_hil_lab::lock::FixtureLock::acquire_without_device(lab, required)?;
    let output = root.join("target/hil/fixture-checks").join(format!(
        "{}-{}",
        oer_durable::unix_millis(),
        scenario.id()
    ));
    std::fs::create_dir_all(&output)?;
    let cleanup = oer_hil_workload::fixture::cleanup::Scope::new(&output);
    let result = crate::fixture::preflight::check(lab, scenario).and_then(|()| {
        Families::FIXTURES
            .iter()
            .try_for_each(|provider| provider.exercise(lab, &plan, &output))
    });
    let records = cleanup.finish()?;
    let restored = records.iter().all(|record| record.failure.is_none());
    let report = serde_json::json!({"schema": 1, "scenario": scenario.id(), "device_accessed": false,
        "prepared": result.is_ok(), "restored": restored,
        "failure": result.as_ref().err().map(|error| error.to_string()), "cleanup": records});
    oer_durable::atomic_json(&output.join("result.json"), &report)?;
    crate::emit_json(
        &serde_json::json!({"report": report, "artifacts": output}),
        true,
    )?;
    result?;
    if !restored {
        return Err("fixture restoration failed; see fixture-check artifacts".into());
    }
    Ok(())
}
