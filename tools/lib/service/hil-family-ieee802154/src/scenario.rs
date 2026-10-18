//! The `[i]` scenario table: IEEE 802.15.4 diagnostics.

use std::path::Path;

use oer_hil_scenario::{AirUse, PeerImage, Plan, ScenarioFamily};
use oer_hil_scenario_catalog::bounded;
use oer_hil_schema::image::ImageClass;
use oer_hil_workload::{context::Context, family::Workload, fixture::Fixtures};
use serde::{Deserialize, Serialize};

use crate::{Result, workload::ieee802154};

/// One IEEE 802.15.4 diagnostic. Each kind implies its diagnostic image.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Ieee802154Scenario {
    EventStatus(Diagnostic),
    EdEvent(Diagnostic),
    AirCheck(AirCheck),
    PeerExchange(PeerExchange),
    BackgroundMaintenance(BackgroundMaintenance),
    ChannelEnergy(ChannelEnergy),
    LiveRssi(LiveRssi),
    ThreadExchange(ThreadExchange),
    RouteProbe(RouteProbe),
    TrackingHold(TrackingHold),
}

/// The RF hold of forced shared-PHY tracking ticks with the IEEE 802.15.4
/// client receiving ().
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TrackingHold {
    pub boots: u8,
    pub channel: u8,
    /// Timed ticks of each branch set per boot.
    pub ticks: u16,
}

/// The same-bit arrival and level-retrigger probe of the source-132 route.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RouteProbe {
    pub boots: u8,
    pub threshold_micros: u32,
    pub settle_micros: u32,
}

/// Channel energy and assessment against the reference peer's burst.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelEnergy {
    pub boots: u8,
    /// The channel the peer's burst occupies.
    pub channel: u8,
    /// A channel at least five channels (25 MHz) from the burst's.
    pub far_channel: u8,
    /// Assessments per step.
    pub samples: u8,
    pub energy_scan_micros: u32,
}

/// The live RSSI read against the reference peer's frames at two transmit
/// powers.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LiveRssi {
    pub boots: u8,
    pub channel: u8,
    /// Frames per power.
    pub frames: u8,
    /// The peer's transmit power of the strong series, in dBm.
    pub high_power_dbm: i8,
    /// The peer's transmit power of the weak series, in dBm.
    pub low_power_dbm: i8,
}

/// A Thread exchange between the device's OpenThread radio and the Thread
/// reference peer.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ThreadExchange {
    pub boots: u8,
    pub channel: u8,
    pub pan_id: u16,
    /// Datagrams per direction.
    pub datagrams: u8,
}

/// Background PHY maintenance of the running client.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BackgroundMaintenance {
    pub boots: u8,
    pub channel: u8,
    /// Tracking periods the session runs.
    pub periods: u8,
}

/// An exchange with the IEEE 802.15.4 reference peer.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PeerExchange {
    pub boots: u8,
    pub channel: u8,
    /// Frames per direction.
    pub frames: u8,
    /// Run the exchange with the device taking part in coexistence with
    /// Wi-Fi and the coexistence schedule running.
    #[serde(default, skip_serializing_if = "core::ops::Not::not")]
    pub wifi_coexistence: bool,
}

/// The single-device on-air check.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AirCheck {
    pub boots: u8,
    pub channel: u8,
    pub cycles: u8,
    pub energy_scan_micros: u32,
    pub receive_window_millis: u32,
    pub scheduled_lead_micros: u32,
    pub scheduled_window_micros: u32,
}

impl AirCheck {
    fn request(self) -> oer_hil_protocol::ieee802154::Ieee802154AirCheckRequest {
        oer_hil_protocol::ieee802154::Ieee802154AirCheckRequest {
            channel: self.channel,
            cycles: self.cycles,
            energy_scan_micros: self.energy_scan_micros,
            receive_window_millis: self.receive_window_millis,
            scheduled_lead_micros: self.scheduled_lead_micros,
            scheduled_window_micros: self.scheduled_window_micros,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Diagnostic {
    pub boots: u8,
    pub poll_limit: u32,
    pub timer_threshold: u32,
}

impl ScenarioFamily for Ieee802154Scenario {
    fn validate(&self) -> Result<()> {
        match self {
            Self::EventStatus(diagnostic) | Self::EdEvent(diagnostic) => {
                bounded(diagnostic.boots, 1, 20, "boots")?;
                bounded(diagnostic.poll_limit, 1, 1_000_000, "poll_limit")?;
                bounded(diagnostic.timer_threshold, 1, 1_000, "timer_threshold")
            }
            Self::TrackingHold(hold) => {
                bounded(hold.boots, 1, 20, "boots")?;
                bounded(hold.channel, 11, 26, "channel")?;
                bounded(hold.ticks, 1, 10, "ticks")
            }
            Self::BackgroundMaintenance(maintenance) => {
                bounded(maintenance.boots, 1, 20, "boots")?;
                bounded(maintenance.channel, 11, 26, "channel")?;
                bounded(maintenance.periods, 2, 30, "periods")
            }
            Self::PeerExchange(exchange) => {
                bounded(exchange.boots, 1, 20, "boots")?;
                bounded(exchange.channel, 11, 26, "channel")?;
                bounded(
                    exchange.frames,
                    1,
                    oer_hil_protocol::ieee802154::IEEE802154_SESSION_RECORDED_FRAMES as u8,
                    "frames",
                )
            }
            Self::ChannelEnergy(energy) => {
                bounded(energy.boots, 1, 20, "boots")?;
                bounded(energy.samples, 1, 16, "samples")?;
                for channel in [energy.channel, energy.far_channel] {
                    let request = oer_hil_protocol::ieee802154::Ieee802154SessionAssessRequest {
                        channel,
                        energy_scan_micros: energy.energy_scan_micros,
                    };
                    if !request.validate() {
                        return Err(
                            "IEEE 802.15.4 channel energy bounds are outside the wire contract"
                                .into(),
                        );
                    }
                }
                if energy.channel.abs_diff(energy.far_channel) < 5 {
                    return Err("the far channel is within 25 MHz of the burst's".into());
                }
                Ok(())
            }
            Self::LiveRssi(live) => {
                bounded(live.boots, 1, 20, "boots")?;
                bounded(live.channel, 11, 26, "channel")?;
                bounded(
                    live.frames,
                    1,
                    oer_hil_protocol::ieee802154::IEEE802154_SESSION_RECORDED_FRAMES as u8,
                    "frames",
                )?;
                if live.high_power_dbm <= live.low_power_dbm {
                    return Err("the strong series' power is not above the weak one's".into());
                }
                Ok(())
            }
            Self::RouteProbe(probe) => {
                bounded(probe.boots, 1, 20, "boots")?;
                let request = oer_hil_protocol::ieee802154::Ieee802154RouteProbeRequest {
                    threshold_micros: probe.threshold_micros,
                    settle_micros: probe.settle_micros,
                };
                if request.validate() {
                    Ok(())
                } else {
                    Err("IEEE 802.15.4 route probe timing is outside the wire contract".into())
                }
            }
            Self::ThreadExchange(exchange) => {
                bounded(exchange.boots, 1, 20, "boots")?;
                bounded(exchange.channel, 11, 26, "channel")?;
                bounded(exchange.pan_id, 0, 0xfffe, "pan_id")?;
                bounded(
                    exchange.datagrams,
                    1,
                    oer_hil_protocol::ieee802154::IEEE802154_THREAD_RECORDED_DATAGRAMS as u8,
                    "datagrams",
                )
            }
            Self::AirCheck(check) => {
                bounded(check.boots, 1, 20, "boots")?;
                if check.request().validate() {
                    Ok(())
                } else {
                    Err("IEEE 802.15.4 air check bounds are outside the wire contract".into())
                }
            }
        }
    }

    /// The catalog image the reference peer board must carry, if the
    /// scenario uses the peer.
    fn peer_image(&self) -> Option<PeerImage> {
        match self {
            Self::PeerExchange(_) | Self::ChannelEnergy(_) | Self::LiveRssi(_) => Some(PeerImage {
                name: crate::peer::PEER_IMAGE,
                reflash: crate::peer::PEER_REFLASH,
            }),
            Self::ThreadExchange(_) => Some(PeerImage {
                name: crate::thread_peer::THREAD_PEER_IMAGE,
                reflash: crate::thread_peer::THREAD_PEER_REFLASH,
            }),
            Self::EventStatus(_)
            | Self::EdEvent(_)
            | Self::RouteProbe(_)
            | Self::AirCheck(_)
            | Self::BackgroundMaintenance(_)
            | Self::TrackingHold(_) => None,
        }
    }

    fn plan(&self) -> Plan {
        let mut plan = Plan::target_only(match self {
            Self::EventStatus(_) => ImageClass::DiagnosticIeee802154EventStatus,
            Self::EdEvent(_) => ImageClass::DiagnosticIeee802154EdEvent,
            Self::RouteProbe(_) => ImageClass::DiagnosticIeee802154Route,
            Self::AirCheck(_) | Self::BackgroundMaintenance(_) => {
                ImageClass::DiagnosticIeee802154Radio
            }
            // Against the reference peer a failure's post-mortem holds the
            // MAC trace.
            Self::PeerExchange(_) | Self::ChannelEnergy(_) | Self::LiveRssi(_) => {
                ImageClass::DiagnosticIeee802154RadioTrace
            }
            Self::ThreadExchange(_) => ImageClass::DiagnosticIeee802154Thread,
            Self::TrackingHold(_) => ImageClass::DiagnosticIeee802154TrackingHold,
        });
        plan.requirements.peer = self.peer_image().is_some();
        plan
    }

    /// The MAC-foundation probes keep RF closed; every other diagnostic
    /// occupies its channels.
    fn air_use(&self) -> Vec<AirUse> {
        match self {
            Self::EventStatus(_) | Self::EdEvent(_) | Self::RouteProbe(_) => Vec::new(),
            Self::AirCheck(scenario) => vec![AirUse::Ieee802154Channel(scenario.channel)],
            Self::PeerExchange(scenario) => vec![AirUse::Ieee802154Channel(scenario.channel)],
            Self::ThreadExchange(scenario) => vec![AirUse::Ieee802154Channel(scenario.channel)],
            Self::BackgroundMaintenance(scenario) => {
                vec![AirUse::Ieee802154Channel(scenario.channel)]
            }
            Self::LiveRssi(scenario) => vec![AirUse::Ieee802154Channel(scenario.channel)],
            Self::TrackingHold(scenario) => vec![AirUse::Ieee802154Channel(scenario.channel)],
            Self::ChannelEnergy(scenario) => vec![
                AirUse::Ieee802154Channel(scenario.channel),
                AirUse::Ieee802154Channel(scenario.far_channel),
            ],
        }
    }
}

impl Workload for Ieee802154Scenario {
    fn run(&self, output: &Path, context: &Context<'_>, _fixtures: &Fixtures) -> Result<()> {
        match *self {
            Self::EventStatus(Diagnostic {
                boots,
                poll_limit,
                timer_threshold,
            }) => ieee802154::event_status::run(
                ieee802154::event_status::Config {
                    boots,
                    poll_limit,
                    timer_threshold,
                },
                output,
                context,
            ),
            Self::EdEvent(Diagnostic {
                boots,
                poll_limit,
                timer_threshold,
            }) => ieee802154::ed_event::run(
                ieee802154::ed_event::Config {
                    boots,
                    poll_limit,
                    timer_threshold,
                },
                output,
                context,
            ),
            Self::TrackingHold(hold) => ieee802154::tracking_hold::run(
                ieee802154::tracking_hold::Config {
                    boots: hold.boots,
                    channel: hold.channel,
                    ticks: hold.ticks,
                },
                output,
                context,
            ),
            Self::BackgroundMaintenance(maintenance) => ieee802154::background_maintenance::run(
                ieee802154::background_maintenance::Config {
                    boots: maintenance.boots,
                    channel: maintenance.channel,
                    periods: maintenance.periods,
                },
                output,
                context,
            ),
            Self::PeerExchange(exchange) => ieee802154::peer_exchange::run(
                ieee802154::peer_exchange::Config {
                    boots: exchange.boots,
                    channel: exchange.channel,
                    frames: exchange.frames,
                    wifi_coexistence: exchange.wifi_coexistence,
                },
                output,
                context,
            ),
            Self::ChannelEnergy(energy) => ieee802154::channel_energy::run(
                ieee802154::channel_energy::Config {
                    boots: energy.boots,
                    channel: energy.channel,
                    far_channel: energy.far_channel,
                    samples: energy.samples,
                    energy_scan_micros: energy.energy_scan_micros,
                },
                output,
                context,
            ),
            Self::LiveRssi(live) => ieee802154::live_rssi::run(
                ieee802154::live_rssi::Config {
                    boots: live.boots,
                    channel: live.channel,
                    frames: live.frames,
                    high_power_dbm: live.high_power_dbm,
                    low_power_dbm: live.low_power_dbm,
                },
                output,
                context,
            ),
            Self::RouteProbe(probe) => ieee802154::route_probe::run(
                ieee802154::route_probe::Config {
                    boots: probe.boots,
                    threshold_micros: probe.threshold_micros,
                    settle_micros: probe.settle_micros,
                },
                output,
                context,
            ),
            Self::ThreadExchange(exchange) => ieee802154::thread_exchange::run(
                ieee802154::thread_exchange::Config {
                    boots: exchange.boots,
                    channel: exchange.channel,
                    pan_id: exchange.pan_id,
                    datagrams: exchange.datagrams,
                },
                output,
                context,
            ),
            Self::AirCheck(check) => ieee802154::air_check::run(
                ieee802154::air_check::Config {
                    boots: check.boots,
                    request: check.request(),
                },
                output,
                context,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_imply_their_images_and_bound_their_polling() {
        for (kind, image) in [
            ("event-status", ImageClass::DiagnosticIeee802154EventStatus),
            ("ed-event", ImageClass::DiagnosticIeee802154EdEvent),
        ] {
            let scenario: Ieee802154Scenario = toml::from_str(&format!(
                "kind = '{kind}'\nboots = 1\npoll_limit = 10\ntimer_threshold = 2"
            ))
            .unwrap();
            scenario.validate().unwrap();
            assert_eq!(scenario.plan(), Plan::target_only(image));
            let excessive: Ieee802154Scenario = toml::from_str(&format!(
                "kind = '{kind}'\nboots = 1\npoll_limit = 0\ntimer_threshold = 2"
            ))
            .unwrap();
            assert!(excessive.validate().is_err());
        }
    }

    #[test]
    fn the_tracking_hold_needs_no_peer_and_bounds_its_ticks() {
        let table = "kind = 'tracking-hold'\nboots = 1\nchannel = 15\nticks = 3";
        let scenario: Ieee802154Scenario = toml::from_str(table).unwrap();
        scenario.validate().unwrap();
        let plan = scenario.plan();
        assert_eq!(plan.image, ImageClass::DiagnosticIeee802154TrackingHold);
        assert!(!plan.requirements.peer);
        for invalid in ["ticks = 0", "ticks = 11"] {
            let scenario: Ieee802154Scenario =
                toml::from_str(&table.replace("ticks = 3", invalid)).unwrap();
            assert!(scenario.validate().is_err(), "{invalid}");
        }
    }

    #[test]
    fn the_peer_exchange_needs_the_peer_and_bounds_its_frames() {
        let table = "kind = 'peer-exchange'\nboots = 1\nchannel = 15\nframes = 4";
        let scenario: Ieee802154Scenario = toml::from_str(table).unwrap();
        scenario.validate().unwrap();
        let plan = scenario.plan();
        assert_eq!(plan.image, ImageClass::DiagnosticIeee802154RadioTrace);
        assert!(plan.requirements.peer);
        for invalid in ["frames = 0", "frames = 17"] {
            let scenario: Ieee802154Scenario =
                toml::from_str(&table.replace("frames = 4", invalid)).unwrap();
            assert!(scenario.validate().is_err(), "{invalid}");
        }
    }

    #[test]
    fn the_live_rssi_cell_needs_the_peer_and_a_power_step() {
        let table = "kind = 'live-rssi'\nboots = 1\nchannel = 15\nframes = 8\nhigh_power_dbm = 20\nlow_power_dbm = -15";
        let scenario: Ieee802154Scenario = toml::from_str(table).unwrap();
        scenario.validate().unwrap();
        let plan = scenario.plan();
        assert_eq!(plan.image, ImageClass::DiagnosticIeee802154RadioTrace);
        assert!(plan.requirements.peer);
        for invalid in ["channel = 27", "frames = 0", "low_power_dbm = 20"] {
            let key = invalid.split(' ').next().unwrap();
            let original = table.lines().find(|line| line.starts_with(key)).unwrap();
            let scenario: Ieee802154Scenario =
                toml::from_str(&table.replace(original, invalid)).unwrap();
            assert!(scenario.validate().is_err(), "{invalid} was accepted");
        }
    }

    #[test]
    fn the_channel_energy_cell_needs_the_peer_and_a_far_channel() {
        let table = "kind = 'channel-energy'\nboots = 1\nchannel = 15\nfar_channel = 25\nsamples = 4\nenergy_scan_micros = 2000";
        let scenario: Ieee802154Scenario = toml::from_str(table).unwrap();
        scenario.validate().unwrap();
        let plan = scenario.plan();
        assert_eq!(plan.image, ImageClass::DiagnosticIeee802154RadioTrace);
        assert!(plan.requirements.peer);
        for invalid in [
            "far_channel = 18",
            "far_channel = 27",
            "samples = 0",
            "energy_scan_micros = 100",
        ] {
            let key = invalid.split(' ').next().unwrap();
            let original = table.lines().find(|line| line.starts_with(key)).unwrap();
            let scenario: Ieee802154Scenario =
                toml::from_str(&table.replace(original, invalid)).unwrap();
            assert!(scenario.validate().is_err(), "{invalid}");
        }
    }

    #[test]
    fn peer_scenarios_name_the_image_their_peer_must_carry() {
        let exchange: Ieee802154Scenario =
            toml::from_str("kind = 'peer-exchange'\nboots = 1\nchannel = 15\nframes = 4").unwrap();
        assert_eq!(
            exchange.peer_image().map(|image| image.name),
            Some(crate::peer::PEER_IMAGE)
        );
        let table =
            "kind = 'thread-exchange'\nboots = 1\nchannel = 15\npan_id = 0x4f45\ndatagrams = 2";
        let thread: Ieee802154Scenario = toml::from_str(table).unwrap();
        thread.validate().unwrap();
        assert_eq!(
            thread.peer_image().map(|image| image.name),
            Some(crate::thread_peer::THREAD_PEER_IMAGE)
        );
        let plan = thread.plan();
        assert_eq!(plan.image, ImageClass::DiagnosticIeee802154Thread);
        assert!(plan.requirements.peer);
        for invalid in ["datagrams = 0", "datagrams = 4"] {
            let scenario: Ieee802154Scenario =
                toml::from_str(&table.replace("datagrams = 2", invalid)).unwrap();
            assert!(scenario.validate().is_err(), "{invalid}");
        }
        let air_check: Ieee802154Scenario = toml::from_str("kind = 'air-check'\nboots = 1\nchannel = 15\ncycles = 2\nenergy_scan_micros = 5000\nreceive_window_millis = 200\nscheduled_lead_micros = 20000\nscheduled_window_micros = 50000").unwrap();
        assert_eq!(air_check.peer_image(), None);
        assert!(!air_check.plan().requirements.peer);
    }

    #[test]
    fn the_route_probe_implies_its_image_and_bounds_its_timing() {
        let table = "kind = 'route-probe'\nboots = 1\nthreshold_micros = 100\nsettle_micros = 2000";
        let scenario: Ieee802154Scenario = toml::from_str(table).unwrap();
        scenario.validate().unwrap();
        assert_eq!(
            scenario.plan(),
            Plan::target_only(ImageClass::DiagnosticIeee802154Route)
        );
        let invalid: Ieee802154Scenario =
            toml::from_str(&table.replace("settle_micros = 2000", "settle_micros = 399")).unwrap();
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn the_air_check_implies_its_image_and_bounds_its_request() {
        let table = "kind = 'air-check'\nboots = 1\nchannel = 15\ncycles = 2\nenergy_scan_micros = 5000\nreceive_window_millis = 200\nscheduled_lead_micros = 20000\nscheduled_window_micros = 50000";
        let scenario: Ieee802154Scenario = toml::from_str(table).unwrap();
        scenario.validate().unwrap();
        assert_eq!(
            scenario.plan(),
            Plan::target_only(ImageClass::DiagnosticIeee802154Radio)
        );
        let invalid: Ieee802154Scenario =
            toml::from_str(&table.replace("channel = 15", "channel = 27")).unwrap();
        assert!(invalid.validate().is_err());
    }
}
