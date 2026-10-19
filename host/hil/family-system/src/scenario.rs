//! The `[system]` scenario table: radio-free workloads and their images.

use std::path::Path;

use oer_hil_scenario::{AirUse, Plan, ScenarioFamily};
use oer_hil_schema::image::ImageClass;
use oer_hil_workload::context::Context;
use oer_hil_workload::{family::Workload, fixture::Fixtures};
use serde::{Deserialize, Serialize};

use crate::{Result, workload::system};

/// One radio-free workload. Each kind implies its firmware image.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum SystemScenario {
    // Unit variants of an internally tagged enum would ignore unknown keys;
    // empty struct variants keep `deny_unknown_fields` effective.
    /// Bootstrap, PSRAM mapping and runtime startup.
    BootSmoke {},
    /// SoC watchdog reset on its exclusive radio-free image.
    Watchdog {},
    /// The product panic entry records an intentional panic and resets the
    /// chip on its exclusive radio-free image.
    PanicReset {},
    /// The stand brings back a board whose image switched its USB
    /// Serial/JTAG off: its reset ladder and its download entry.
    StandRecovery {},
}

const BOOT_SMOKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

impl ScenarioFamily for SystemScenario {
    fn validate(&self) -> Result<()> {
        match self {
            Self::BootSmoke {}
            | Self::Watchdog {}
            | Self::PanicReset {}
            | Self::StandRecovery {} => Ok(()),
        }
    }

    fn plan(&self) -> Plan {
        Plan::target_only(match self {
            Self::BootSmoke {} => ImageClass::BootSmoke,
            Self::Watchdog {} => ImageClass::SystemWatchdog,
            Self::PanicReset {} => ImageClass::SystemPanicReset,
            Self::StandRecovery {} => ImageClass::DiagnosticUsbJtagOff,
        })
    }

    /// The watchdog image enables no radio; the other system images carry
    /// the product's radios.
    fn air_use(&self) -> Vec<AirUse> {
        match self {
            Self::Watchdog {} => Vec::new(),
            _ => vec![AirUse::Band2G4],
        }
    }
}

impl Workload for SystemScenario {
    fn run(&self, output: &Path, context: &Context<'_>, _fixtures: &Fixtures) -> Result<()> {
        match self {
            Self::BootSmoke {} => context.with_capture(output, |capture| {
                capture.wait_for_boot_smoke(BOOT_SMOKE_TIMEOUT)
            }),
            Self::Watchdog {} => system::watchdog::run(output, context),
            Self::PanicReset {} => system::panic_reset::run(output, context),
            Self::StandRecovery {} => system::stand_recovery::run(output, context),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> SystemScenario {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn each_workload_implies_its_image_and_needs_no_fixture() {
        for (text, image) in [
            ("kind = 'boot-smoke'", ImageClass::BootSmoke),
            ("kind = 'watchdog'", ImageClass::SystemWatchdog),
            ("kind = 'panic-reset'", ImageClass::SystemPanicReset),
            ("kind = 'stand-recovery'", ImageClass::DiagnosticUsbJtagOff),
        ] {
            let scenario = parse(text);
            scenario.validate().unwrap();
            assert_eq!(scenario.plan(), Plan::target_only(image));
        }
    }

    #[test]
    fn a_workload_names_no_image() {
        assert!(
            toml::from_str::<SystemScenario>("kind = 'boot-smoke'\nimage = 'boot-smoke'").is_err()
        );
    }
}
