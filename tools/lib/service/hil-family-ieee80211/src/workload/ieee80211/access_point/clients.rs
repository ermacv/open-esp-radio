//! Scoped ownership of AP client fixtures.

use oer_hil_workload::context::Context;
use std::net::Ipv4Addr;

use oer_hil_protocol::wifi::WifiAccessPointSecurity;

use crate::Result;
use crate::scenario::AccessPointClients;
use crate::workload::ieee80211::access_point::with_cleanup_errors;
use oer_hil_family_ieee80211_fixture::{
    local::client::{
        ClientNetwork, ClientPhy, ControlledClient, LaptopClientLinkEvidence,
        LaptopClientLinkObservation,
    },
    openwrt::client::ControlledOpenWrtClient,
    openwrt::client::{OpenWrtClientLinkEvidence, OpenWrtClientLinkObservation},
};

/// The primary client's link counters over one workload, from its own
/// driver: what it sent, retried and gave up on.
pub(super) enum PrimaryLinkObservation {
    OpenWrt(Box<OpenWrtClientLinkObservation>),
    Laptop(LaptopClientLinkObservation),
}

impl PrimaryLinkObservation {
    pub(super) fn finish(self) -> Result<ClientLinkEvidence> {
        Ok(match self {
            Self::OpenWrt(observation) => ClientLinkEvidence::OpenWrt(observation.finish()?),
            Self::Laptop(observation) => ClientLinkEvidence::Laptop(observation.finish()?),
        })
    }
}

/// What the primary client's driver counted on its link to the AP.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(tag = "client", rename_all = "kebab-case")]
pub(super) enum ClientLinkEvidence {
    OpenWrt(OpenWrtClientLinkEvidence),
    Laptop(LaptopClientLinkEvidence),
}

impl core::fmt::Display for ClientLinkEvidence {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::OpenWrt(evidence) => write!(
                f,
                "OpenWrt AP-client link: rx_packets={} rx_bytes={} rx_duration_us={:?} rx_bitrate={:?} tx_packets={} tx_bytes={} tx_bitrate={:?} retries={} failed={} tx_duration_us={} tid0_aqm_drops={}",
                evidence.rx_packets,
                evidence.rx_bytes,
                evidence.rx_duration_micros,
                evidence.rx_bitrate,
                evidence.tx_packets,
                evidence.tx_bytes,
                evidence.tx_bitrate,
                evidence.tx_retries,
                evidence.tx_failed,
                evidence.tx_duration_micros,
                evidence.tid0_aqm_drops,
            ),
            Self::Laptop(evidence) => write!(
                f,
                "laptop AP-client link: tx_packets={} retries={} failed={} interface_tx_dropped={} tid0_aqm_drops/overlimit={}/{} rx_packets={} rx_drop_misc={} tx_bitrate={:?}",
                evidence.tx_packets,
                evidence.tx_retries,
                evidence.tx_failed,
                evidence.interface_tx_dropped,
                evidence.tid0_aqm_drops,
                evidence.tid0_aqm_overlimit,
                evidence.rx_packets,
                evidence.rx_drop_misc,
                evidence.tx_bitrate,
            ),
        }
    }
}
use oer_hil_lab::config::StationFixtureConfig;
use oer_hil_scenario_catalog::link::HtGuardIntervalExpectation;

pub(super) enum ConnectedClients {
    Laptop {
        primary: ControlledClient,
        secondary: Option<ControlledOpenWrtClient>,
        management_capture: Option<
            Box<oer_hil_family_ieee80211_fixture::openwrt::tx_monitor::OpenWrtManagementCapture>,
        >,
    },
    OpenWrt {
        primary: ControlledOpenWrtClient,
    },
}

impl ConnectedClients {
    pub(super) fn openwrt_primary(&self) -> Option<&ControlledOpenWrtClient> {
        match self {
            Self::OpenWrt { primary } => Some(primary),
            Self::Laptop { .. } => None,
        }
    }

    pub(super) fn secondary(&self) -> Option<&ControlledOpenWrtClient> {
        match self {
            Self::Laptop { secondary, .. } => secondary.as_ref(),
            Self::OpenWrt { .. } => None,
        }
    }

    pub(super) fn traffic_target(&self, target: Ipv4Addr) -> Result<Ipv4Addr> {
        match self {
            Self::Laptop { .. } => Ok(target),
            Self::OpenWrt { primary } => primary.forward_address().ok_or_else(|| {
                "OpenWrt primary client omitted its wired forwarding address".into()
            }),
        }
    }

    pub(super) fn begin_primary_link_observation(&self) -> Result<PrimaryLinkObservation> {
        Ok(match self {
            Self::OpenWrt { primary } => {
                PrimaryLinkObservation::OpenWrt(Box::new(primary.begin_link_observation()?))
            }
            Self::Laptop { primary, .. } => {
                PrimaryLinkObservation::Laptop(primary.begin_link_observation()?)
            }
        })
    }

    pub(super) fn begin_secondary_link_observation(
        &self,
    ) -> Result<Option<OpenWrtClientLinkObservation>> {
        self.secondary()
            .map(ControlledOpenWrtClient::begin_link_observation)
            .transpose()
    }
}

pub(super) fn connect_clients(
    config: &super::Config,
    minimum_clients: u8,
    context: &Context<'_>,
    output: &std::path::Path,
) -> Result<ConnectedClients> {
    let security = config.security;
    let (openwrt_client_fixed_ht_mcs, openwrt_client_fixed_guard_interval) = match config.clients {
        AccessPointClients::OpenWrt {
            fixed_ht_mcs,
            fixed_guard_interval: true,
        } => (
            fixed_ht_mcs,
            config
                .link
                .map_or(HtGuardIntervalExpectation::Any, |link| link.guard_interval),
        ),
        AccessPointClients::OpenWrt { fixed_ht_mcs, .. } => {
            (fixed_ht_mcs, HtGuardIntervalExpectation::Any)
        }
        AccessPointClients::Laptop { .. } | AccessPointClients::LaptopAndOpenWrt { .. } => {
            (None, HtGuardIntervalExpectation::Any)
        }
    };
    let timeout = config.timeout;
    let openwrt_fixture = || -> Result<&oer_hil_lab::config::OpenWrtConfig> {
        match &context.lab.station_fixture {
            StationFixtureConfig::OpenWrt(fixture) => Ok(fixture),
            _ => Err("AP OpenWrt client requires the OpenWrt station fixture".into()),
        }
    };
    match config.clients {
        AccessPointClients::OpenWrt { .. } => Ok(ConnectedClients::OpenWrt {
            primary: ControlledOpenWrtClient::connect_primary(
                &context.lab.access_point,
                openwrt_fixture()?,
                security,
                openwrt_client_fixed_ht_mcs,
                openwrt_client_fixed_guard_interval,
            )?,
        }),
        AccessPointClients::Laptop { .. } | AccessPointClients::LaptopAndOpenWrt { .. } => {
            // Associate the observable OpenWrt peer first in two-client runs.
            // This gives debugfs evidence for the first BA bank and exercises
            // the laptop on the next independently allocated peer slot.
            let secondary = if minimum_clients >= 2 {
                Some(ControlledOpenWrtClient::connect(
                    &context.lab.access_point,
                    openwrt_fixture()?,
                    security,
                    openwrt_client_fixed_ht_mcs,
                    openwrt_client_fixed_guard_interval,
                )?)
            } else {
                None
            };
            if security == WifiAccessPointSecurity::Open {
                return Err("open AP qualification requires the controlled OpenWrt client".into());
            }
            let connect = || -> Result<_> {
                let capture = match &context.lab.station_fixture {
                    StationFixtureConfig::OpenWrt(config) if config.monitor_interface.is_some() => {
                        Some(Box::new(
                            oer_hil_family_ieee80211_fixture::openwrt::tx_monitor::OpenWrtManagementCapture::start(
                                config, output, timeout,
                            )?,
                        ))
                    }
                    _ => None,
                };
                let result = ControlledClient::connect(
                    &ClientNetwork::target_access_point(
                        &context.lab.access_point,
                        match config.clients.laptop_phy() {
                            Some(crate::scenario::access_point::LaptopPhy::NonHt) => {
                                ClientPhy::NonHt
                            }
                            _ => ClientPhy::Ht,
                        },
                    ),
                    &output.join("linux-client"),
                );
                match result {
                    Ok(primary) => Ok((primary, capture)),
                    Err(error) => {
                        let evidence = capture.map(|capture| capture.finish()).transpose().err();
                        Err(with_cleanup_errors(error, evidence, None, None, None))
                    }
                }
            };
            let (primary, management_capture) = match connect() {
                Ok(primary) => primary,
                Err(error) => {
                    let restore = secondary
                        .map(ControlledOpenWrtClient::restore)
                        .transpose()
                        .err();
                    return Err(with_cleanup_errors(error, restore, None, None, None));
                }
            };
            Ok(ConnectedClients::Laptop {
                primary,
                secondary,
                management_capture,
            })
        }
    }
}

pub(super) fn restore_clients(clients: ConnectedClients) -> Result<()> {
    match clients {
        ConnectedClients::OpenWrt { primary } => primary.restore(),
        ConnectedClients::Laptop {
            primary,
            secondary,
            management_capture,
        } => {
            let capture = management_capture
                .map(|capture| capture.finish())
                .transpose();
            let secondary = secondary.map(ControlledOpenWrtClient::restore).transpose();
            let primary = primary.restore();
            let restore = match (primary, secondary) {
                (Ok(()), Ok(_)) => Ok(()),
                (Err(primary), Ok(_)) => Err(primary),
                (Ok(()), Err(secondary)) => Err(secondary),
                (Err(primary), Err(secondary)) => Err(format!(
                    "primary client restore failed: {primary}; secondary client restore failed: {secondary}",
                )
                .into()),
            };
            match (restore, capture) {
                (Ok(()), Ok(_)) => Ok(()),
                (Err(error), Ok(_)) | (Ok(()), Err(error)) => Err(error),
                (Err(error), Err(capture)) => {
                    Err(format!("{error}; management capture failed: {capture}").into())
                }
            }
        }
    }
}
