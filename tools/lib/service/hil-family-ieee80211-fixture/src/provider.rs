//! The Wi-Fi fixture provider: the laboratory checks a plan's Wi-Fi
//! requirements need, and the AP fixture a repetition runs against.

use std::path::Path;

use oer_hil_lab::config::{LabConfig, StationFixtureConfig};
use oer_hil_scenario::Plan;
use oer_hil_scenario_catalog::requirements::Requirements;
use oer_hil_workload::{
    family::FixtureProvider,
    fixture::{Fixtures, cleanup},
};

use crate::{Error, Result, prepared::Prepared};

/// The provider the runner's registry lists.
pub static PROVIDER: WifiFixtures = WifiFixtures;

/// Prepares and checks the station fixture AP, the laptop and OpenWrt
/// clients, monitors and captures a plan requires.
pub struct WifiFixtures;

impl FixtureProvider for WifiFixtures {
    fn admit(&self, lab: &LabConfig, required: Requirements) -> Result<()> {
        crate::local::network_helper::require_for(lab, required)
    }

    fn precondition(&self, lab: &LabConfig, plan: &Plan) -> Result<()> {
        if !plan.requirements.station_network {
            return Ok(());
        }
        lab.station_fixture.require_phy(lab.fixture_phy(plan.wifi))
    }

    fn check(&self, lab: &LabConfig, plan: &Plan) -> Result<()> {
        let required = plan.requirements;
        if let StationFixtureConfig::OpenWrt(config) = &lab.station_fixture
            && config.read_only
            && (required.station_control || required.openwrt_client || required.openwrt_tx_monitor)
        {
            return Err(Error::new(
                "scenario requires mutations forbidden by read-only OpenWrt fixture",
            )
            .into());
        }
        if required.air_observer && lab.air_observer.is_none() {
            return Err(Error::new("scenario requires the independent [air_observer]").into());
        }
        if (required.non_ht_member || required.legacy_bss)
            && !matches!(lab.station_fixture, StationFixtureConfig::OpenWrt(_))
        {
            return Err(
                Error::new("induced BSS protection requires the OpenWrt station fixture").into(),
            );
        }
        if required.legacy_bss && lab.legacy_bss.is_none() {
            return Err(
                Error::new("scenario requires the [legacy_bss] laboratory capability").into(),
            );
        }
        if required.probe_load {
            if !Path::new("/usr/local/libexec/open-radio-probe").is_file() {
                return Err(Error::new(
                    "probe load requires cargo stand fixture install --provider linux-net",
                )
                .into());
            }
            oer_toolchain::require_program(std::ffi::OsStr::new("tshark"))?;
        }
        if required.station_network {
            cleanup::require_healthy()?;
        }
        crate::local::network_helper::require_for(lab, required)?;
        if required.station_network {
            match &lab.station_fixture {
                StationFixtureConfig::OpenWrt(config) => {
                    let phy = lab.fixture_phy(plan.wifi);
                    crate::openwrt::ap::probe(
                        config,
                        crate::openwrt::ap::Profile::new(
                            config,
                            phy,
                            plan.wifi.management_frame_protection,
                            plan.wifi.access_point_security,
                            plan.wifi.access_point_beacon,
                        ),
                    )
                    .map_err(Error::context)?;
                    // Both directions consume remote counters and command-line capture tools.
                    if required.station_udp_rx_capture || required.station_udp_tx_capture {
                        crate::openwrt::evidence::doctor_tools(config)?;
                    }
                }
                StationFixtureConfig::LocalLinux(config) => {
                    crate::local::ap::check(config, &lab.station, lab.fixture_phy(plan.wifi))
                        .map_err(Error::context)?;
                    if required.station_udp_rx_capture || required.station_udp_tx_capture {
                        oer_toolchain::require_program(std::ffi::OsStr::new("dumpcap"))?;
                    }
                }
                StationFixtureConfig::External(_) if required.station_control => {
                    return Err(Error::new("scenario requires a controllable AP fixture").into());
                }
                StationFixtureConfig::External(_) => {}
            }
        }
        if required.laptop_client {
            crate::local::client::doctor()?;
        }
        if required.openwrt_client {
            let StationFixtureConfig::OpenWrt(config) = &lab.station_fixture else {
                return Err(Error::new("scenario requires an OpenWrt client").into());
            };
            crate::openwrt::client::doctor(&lab.access_point, config)?;
        }
        if required.openwrt_tx_monitor {
            let StationFixtureConfig::OpenWrt(config) = &lab.station_fixture else {
                return Err(Error::new("scenario requires an OpenWrt monitor").into());
            };
            crate::openwrt::tx_monitor::doctor(config)?;
        }
        if required.laptop_air_monitor {
            crate::local::air_monitor::doctor()?;
        }
        Ok(())
    }

    fn prepare(
        &self,
        lab: &LabConfig,
        plan: &Plan,
        output: &Path,
        fixtures: &mut Fixtures,
    ) -> Result<()> {
        fixtures.insert(Prepared::start(lab, plan, output)?)
    }

    /// The station fixture AP (stopped and restarted when the scenario
    /// controls it) and the captures its traffic evidence takes.
    fn exercise(&self, lab: &LabConfig, plan: &Plan, output: &Path) -> Result<()> {
        exercise(lab, plan, output)
    }
}

fn exercise(lab: &LabConfig, plan: &Plan, output: &Path) -> Result<()> {
    let required = plan.requirements;
    let prepared = Prepared::start(lab, plan, output)?;
    if required.station_control {
        let mut ap = prepared.ap()?;
        ap.stop()?;
        ap.restart()?;
    }
    if let StationFixtureConfig::OpenWrt(config) = &lab.station_fixture {
        if required.station_udp_rx_capture || required.station_udp_tx_capture {
            crate::openwrt::evidence::check_capture(config)?;
        }
        if required.laptop_air_monitor {
            crate::local::air_monitor::check_without_device(config, output)?;
        }
    }
    if required.station_udp_rx_capture
        && let Some(observer) = crate::openwrt::air_monitor::Capture::start(
            lab,
            None,
            std::time::Duration::from_secs(1),
            output,
        )?
    {
        observer.finish()?;
    }
    Ok(())
}
