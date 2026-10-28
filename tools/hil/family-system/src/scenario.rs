//! The `[system]` scenario table: radio-free workloads and their images.

use std::path::Path;

use oer_hil_model::schema::image::ImageClass;
use oer_hil_protocol::{Key, Message};
use oer_hil_scenario::workload::context::Context;
use oer_hil_scenario::workload::{family::Workload, fixture::Fixtures};
use oer_hil_scenario::{AirUse, Plan, ScenarioFamily};
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
    /// A host-requested reset retains HilRequested on its exclusive radio-free
    /// image, and the next boot reports the acknowledged request's identity.
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

    fn required_keys(&self) -> &'static [Key] {
        use oer_hil_protocol::system;
        const WATCHDOG: &[Key] = &[
            system::WatchdogTest::KEY,
            system::WatchdogArmed::KEY,
            system::PanicHook::KEY,
        ];
        const PANIC: &[Key] = &[system::InjectPanic::KEY, system::PanicInjected::KEY];
        const USB: &[Key] = &[
            system::DisableUsb::KEY,
            system::UsbDisabled::KEY,
            system::UsbJtagOff::KEY,
        ];
        match self {
            Self::BootSmoke {} => &[],
            Self::Watchdog {} => WATCHDOG,
            Self::PanicReset {} => PANIC,
            Self::StandRecovery {} => USB,
        }
    }

    fn forbidden_keys(&self) -> &'static [Key] {
        use oer_hil_protocol::system;
        match self {
            Self::Watchdog {} => &[system::UsbJtagOff::KEY],
            Self::PanicReset {} => &[system::PanicHook::KEY, system::UsbJtagOff::KEY],
            Self::BootSmoke {} | Self::StandRecovery {} => &[],
        }
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
    fn each_workload_requires_the_service_it_drives() {
        use oer_hil_protocol::system;
        assert!(parse("kind = 'boot-smoke'").required_keys().is_empty());
        for (text, endpoint) in [
            ("kind = 'watchdog'", system::WatchdogTest::KEY),
            ("kind = 'panic-reset'", system::InjectPanic::KEY),
            ("kind = 'stand-recovery'", system::DisableUsb::KEY),
        ] {
            assert!(parse(text).required_keys().contains(&endpoint), "{text}");
        }
    }

    #[test]
    fn a_workload_names_no_image() {
        assert!(
            toml::from_str::<SystemScenario>("kind = 'boot-smoke'\nimage = 'boot-smoke'").is_err()
        );
    }
}
