//! Read-only, scenario-scoped preflight. Report independent failures together.

use crate::image;
use std::path::Path;

use serde::Serialize;

use crate::scenario::{Scenario, requirements};
use crate::{Result, fixture};

use oer_hil_session::config::LabConfig;

#[derive(Default, Serialize)]
struct Checks {
    checks: Vec<Check>,
}

#[derive(Serialize)]
struct Check {
    name: String,
    passed: bool,
    failure: Option<String>,
}

impl Checks {
    fn run(&mut self, name: impl Into<String>, check: impl FnOnce() -> Result<()>) -> Result<()> {
        oer_os_process::check_cancelled()?;
        let result = check();
        if let Err(error) = &result
            && oer_os_process::is_cancelled(&**error)
        {
            return result;
        }
        self.checks.push(Check {
            name: name.into(),
            passed: result.is_ok(),
            failure: result.err().map(|error| error.to_string()),
        });
        Ok(())
    }

    fn passed(&self) -> bool {
        self.checks.iter().all(|check| check.passed)
    }
}

pub(crate) fn run(root: &Path, lab: &LabConfig, scenarios: &[&Scenario]) -> Result<()> {
    let required = requirements(scenarios);
    let mut checks = Checks::default();
    checks.run("firmware-agent", || {
        let profile = oer_chip_profile::Profile::load(root, lab.chip())?;
        for scenario in scenarios {
            let recipe = profile
                .hil_image(scenario.image().id())
                .ok_or("unsupported HIL class")?;
            let manifest = root.join(&recipe.manifest);
            if !manifest.is_file() {
                return Err(format!("missing HIL image {}", manifest.display()).into());
            }
        }
        Ok(())
    })?;
    checks.run("serial-device", || fs_device_exists(&lab.dut.serial))?;
    for tool in oer_os_toolchain::Tool::ALL {
        checks.run(format!("tool-{}", tool.name()), || {
            oer_os_toolchain::require(tool)
        })?;
    }
    checks.run("source-dependencies", || {
        image::ensure_vendor_dependencies_absent(root)
    })?;
    for scenario in scenarios {
        checks.run(format!("fixture-{}", scenario.id()), || {
            fixture::preflight::check(lab, scenario)
        })?;
    }
    checks.run("resource-ownership", || {
        oer_hil_session::lock::FixtureLock::probe_for(lab, required)
    })?;
    crate::emit_json(
        &serde_json::json!({
            "schema": 1,
            "status": if checks.passed() { "passed" } else { "failed" },
            "cell_id": lab.cell_id(),
            "device_id": lab.dut.id,
            "stand_file": lab.path(),
            "requirements": required,
            "checks": checks.checks,
        }),
        true,
    )?;
    if checks.passed() {
        Ok(())
    } else {
        Err("HIL environment checks failed; see the JSON check report".into())
    }
}

fn fs_device_exists(path: &Path) -> Result<()> {
    path.exists()
        .then_some(())
        .ok_or_else(|| format!("serial device does not exist: {}", path.display()).into())
}

#[cfg(test)]
mod tests;
