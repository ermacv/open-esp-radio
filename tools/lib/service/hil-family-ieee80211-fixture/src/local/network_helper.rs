//! One versioned contract for the root-owned laptop radio helper.

use oer_process::CommandExt as _;

use crate::Result;

pub use oer_hil_lab::NETWORK_HELPER as PATH;
const REQUIRED_CAPABILITIES: &str = "schema=15 station_ap=ht20,ht40,he20 legacy_ap=dsss client=ht,non-ht client_aqm=1 observer=20,40 managed=1 rfkill=restore";

/// Validate the installed command protocol before a selected run can flash or
/// reset the DUT. System-only and remote-only workloads do not need this helper.
pub fn require_for(
    lab: &oer_hil_lab::config::LabConfig,
    required: oer_hil_scenario_catalog::requirements::Requirements,
) -> Result<()> {
    if required.local_radio()
        || (required.station_network
            && matches!(
                lab.station_fixture,
                oer_hil_lab::config::StationFixtureConfig::LocalLinux(_)
            ))
    {
        doctor()?;
    }
    Ok(())
}

pub fn doctor() -> Result<()> {
    let output = oer_process::command("sudo")
        .args(["-n", PATH, "capabilities"])
        .supervised_output()?;
    if !output.status.success() {
        return Err(crate::Error::new(format!(
            "laptop radio helper is unavailable through non-interactive sudo: {}",
            output.status
        ))
        .into());
    }
    require_capabilities(&String::from_utf8(output.stdout)?)
}

fn require_capabilities(capabilities: &str) -> Result<()> {
    if capabilities.trim() != REQUIRED_CAPABILITIES {
        return Err(crate::Error::new(format!(
            "installed laptop radio helper is incompatible: expected `{REQUIRED_CAPABILITIES}`, got `{}`; reinstall it from this checkout",
            capabilities.trim()
        )).into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
