//! Uniform session evidence for a managed station access-point fixture.

use std::{net::SocketAddrV4, path::Path, time::Duration};

use crate::{
    Result, local::evidence::LocalLinuxRxCapture, local::evidence::LocalLinuxRxEvidence,
    openwrt::evidence::OpenWrtRxCapture, openwrt::evidence::OpenWrtRxEvidence,
};
use oer_hil_lab::config::StationFixtureConfig;
use oer_hil_scenario_catalog::link::{HtGuardIntervalExpectation, PhyExpectation};

pub enum RxCapture {
    LocalLinux(Box<LocalLinuxRxCapture>),
    OpenWrt(Box<OpenWrtRxCapture>),
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "kebab-case", tag = "fixture")]
pub enum RxEvidence {
    LocalLinux(LocalLinuxRxEvidence),
    OpenWrt(OpenWrtRxEvidence),
}

impl RxCapture {
    pub fn start(
        fixture: &StationFixtureConfig,
        endpoint: SocketAddrV4,
        output: &Path,
        duration: Duration,
        phy: PhyExpectation,
        forced_guard_interval: HtGuardIntervalExpectation,
        maximum_idle_channel_utilization_255: Option<u8>,
    ) -> Result<Option<Self>> {
        let target = *endpoint.ip();
        let port = endpoint.port();
        match fixture {
            StationFixtureConfig::LocalLinux(config) => {
                if forced_guard_interval != HtGuardIntervalExpectation::Any {
                    return Err(
                        "OpenWrt fixed-GI mutation requires the managed OpenWrt fixture".into(),
                    );
                }
                if maximum_idle_channel_utilization_255.is_some() {
                    return Err(
                        "idle channel utilization requires an OpenWrt station fixture".into(),
                    );
                }
                Ok(Some(Self::LocalLinux(Box::new(
                    LocalLinuxRxCapture::start(config, target, port, duration, phy)?,
                ))))
            }
            StationFixtureConfig::OpenWrt(config) => {
                Ok(Some(Self::OpenWrt(Box::new(OpenWrtRxCapture::start(
                    config,
                    endpoint,
                    output,
                    duration,
                    phy,
                    forced_guard_interval,
                    maximum_idle_channel_utilization_255,
                )?))))
            }
            StationFixtureConfig::External(_) => {
                if forced_guard_interval != HtGuardIntervalExpectation::Any {
                    return Err(
                        "OpenWrt fixed-GI mutation requires the managed OpenWrt fixture".into(),
                    );
                }
                if maximum_idle_channel_utilization_255.is_some() {
                    return Err(
                        "idle channel utilization requires a managed OpenWrt station fixture"
                            .into(),
                    );
                }
                Ok(None)
            }
        }
    }

    pub fn finish(self) -> Result<RxEvidence> {
        match self {
            Self::LocalLinux(capture) => capture.finish().map(RxEvidence::LocalLinux),
            Self::OpenWrt(capture) => (*capture).finish().map(RxEvidence::OpenWrt),
        }
    }
}

impl RxEvidence {
    pub const fn wireless_packets(&self) -> u64 {
        match self {
            Self::LocalLinux(evidence) => evidence.udp_packets,
            Self::OpenWrt(evidence) => evidence.wireless_packets,
        }
    }

    pub fn require_ht40_downlink(&self) -> Result<()> {
        match self {
            Self::LocalLinux(evidence) => require_ht40_mcs7(
                "STA RX/AP TX",
                evidence.channel_width_mhz,
                &evidence.tx_bitrate,
            ),
            Self::OpenWrt(evidence) => require_ht40_mcs7(
                "STA RX/AP TX",
                evidence.channel_width_mhz,
                &evidence.tx_bitrate,
            ),
        }
    }
}

/// Require the HT40 MCS7 ceiling family rather than comparing throughput
/// samples produced at different widths or MCS values. The fixture snapshot
/// is only the peer's most recent rate, so guard interval remains reported
/// evidence instead of a whole-interval precondition.
pub fn require_ht40_mcs7(direction: &str, interface_width_mhz: u8, bitrate: &str) -> Result<()> {
    let fields = bitrate.split_whitespace().collect::<Vec<_>>();
    let observed_mbps = fields
        .first()
        .ok_or_else(|| format!("{direction} link snapshot omitted bitrate"))?
        .parse::<f64>()
        .map_err(|error| format!("invalid {direction} bitrate `{bitrate}`: {error}"))?;
    let mcs = fields
        .windows(2)
        .find_map(|pair| (pair[0] == "MCS").then_some(pair[1]))
        .ok_or_else(|| format!("{direction} link snapshot omitted MCS: `{bitrate}`"))?
        .parse::<u8>()
        .map_err(|error| format!("invalid {direction} MCS in `{bitrate}`: {error}"))?;
    let vector_width_mhz = fields
        .iter()
        .find_map(|field| field.strip_suffix("MHz"))
        .ok_or_else(|| format!("{direction} link snapshot omitted channel width: `{bitrate}`"))?
        .parse::<u8>()
        .map_err(|error| format!("invalid {direction} channel width in `{bitrate}`: {error}"))?;
    let mcs7_rate =
        (134.9..=135.1).contains(&observed_mbps) || (149.9..=150.1).contains(&observed_mbps);
    if interface_width_mhz != 40 || vector_width_mhz != 40 || mcs != 7 || !mcs7_rate {
        return Err(format!(
            "{direction} link precondition failed: required=MCS 7 40MHz (135.0 MBit/s long GI or 150.0 MBit/s short GI) observed_interface_width={interface_width_mhz} observed=`{bitrate}`"
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
