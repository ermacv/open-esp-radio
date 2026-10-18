//! The `[bluetooth]` scenario table: Bluetooth LE workloads and their images.
//!
//! A workload implies its firmware image.

use std::path::Path;

use oer_hil_lab::config::LabConfig;
use oer_hil_protocol::DeviceImageKeys;
use oer_hil_protocol::bluetooth;
use oer_hil_scenario::{AirUse, PeerImage, Plan, ScenarioFamily};
use oer_hil_scenario_catalog::{bounded, requirements::Requirements};
use oer_hil_schema::image::ImageClass;
use oer_hil_workload::{context::Context, family::Workload, fixture::Fixtures};
use serde::{Deserialize, Serialize};

use crate::{
    Result,
    fixture::bluetooth::{self as fixture, model::Adapter},
    workload::bluetooth::{self as workload, secure_gatt::IrqSampling},
};

// Unit variants of an internally tagged enum would ignore unknown keys;
// empty struct variants keep `deny_unknown_fields` effective.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum BluetoothScenario {
    /// Three plaintext ATT connection cycles on one Trouble/Controller epoch.
    /// With `trace`, on the image that records the Bluetooth LE trace, whose
    /// connection steps the workload then checks.
    Gatt {
        #[serde(default, skip_serializing_if = "core::ops::Not::not")]
        trace: bool,
    },
    /// Scannable non-connectable advertising found by an active scanner
    /// through its scan response.
    ScannableAdvertising {},
    /// High duty cycle directed advertising times out; low duty cycle
    /// directed advertising connects its target.
    DirectedAdvertising {},
    /// Automated Numeric Comparison, bonded reconnect and an explicit
    /// terminal fault.
    SecureGatt {
        #[serde(default)]
        shutdown: SecureGattShutdown,
        /// Boundary-only sampling keeps IRQ watermark scans out of traffic;
        /// it is a timing comparison, not memory-stress evidence.
        #[serde(default)]
        irq_sampling: IrqSampling,
    },
    Dtm {
        boots: u8,
        minimum_packets: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        quiet_cycles: Option<u16>,
    },
    /// Direct Test Mode with the reference peer: the board under test
    /// receives LE 1M, 2M and Coded (S=8 and S=2) and transmits LE 1M and
    /// 2M, with a silence control on each receiver.
    DtmPeer { minimum_packets: u16 },
}

/// Mutually exclusive terminal proofs; neither substitutes for the other.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SecureGattShutdown {
    #[default]
    BondLoadFailure,
    HciReadFailure,
}

impl ScenarioFamily for BluetoothScenario {
    fn validate(&self) -> Result<()> {
        match self {
            Self::Dtm {
                boots,
                minimum_packets,
                quiet_cycles,
            } => {
                bounded(*boots, 1, 10, "boots")?;
                bounded(*minimum_packets, 1, 160, "minimum_packets")?;
                if let Some(cycles) = quiet_cycles {
                    bounded(*cycles, 1, 1000, "quiet_cycles")?;
                }
                Ok(())
            }
            Self::DtmPeer { minimum_packets } => {
                bounded(*minimum_packets, 1, 1000, "minimum_packets")
            }
            _ => Ok(()),
        }
    }

    /// The catalog image the reference peer board must carry, when the
    /// scenario uses the peer; [`crate::fixture::dtm_peer`] drives the
    /// Direct Test Mode peer.
    fn peer_image(&self) -> Option<PeerImage> {
        match self {
            Self::DtmPeer { .. } => Some(crate::fixture::dtm_peer::DTM_PEER_IMAGE),
            _ => None,
        }
    }

    fn plan(&self) -> Plan {
        Plan {
            requirements: Requirements {
                bluetooth_adapter: self.adapter_preflight().is_some(),
                peer: self.peer_image().is_some(),
                ..Requirements::default()
            },
            checks: match self {
                Self::Gatt { trace: true } => vec!["bluetooth.trace.connection-steps"],
                _ => Vec::new(),
            },
            ..Plan::target_only(self.image())
        }
    }

    /// Whether an image of [`Self::image`] reporting `capabilities` runs this
    /// workload: the advertising workloads drive the image's Controller over
    /// raw HCI.
    fn served_by(&self, capabilities: &DeviceImageKeys) -> bool {
        match self {
            Self::Gatt { .. } | Self::SecureGatt { .. } => true,
            Self::ScannableAdvertising {} | Self::DirectedAdvertising {} | Self::DtmPeer { .. } => {
                capabilities.has::<bluetooth::Hci>()
            }
            Self::Dtm { .. } => capabilities.has::<bluetooth::Dtm>(),
        }
    }

    /// Bluetooth LE connections and Direct Test Mode hop across the band.
    fn air_use(&self) -> Vec<AirUse> {
        vec![AirUse::Band2G4]
    }
}

impl Workload for BluetoothScenario {
    /// The Linux adapter must offer what the workload drives.
    fn precondition(&self, lab: &LabConfig) -> Result<()> {
        let Some(preflight) = self.adapter_preflight() else {
            return Ok(());
        };
        preflight(
            lab.bluetooth_adapter
                .ok_or("missing [bluetooth] adapter in the stand file")?,
        )
    }

    fn run(&self, output: &Path, context: &Context<'_>, _fixtures: &Fixtures) -> Result<()> {
        match self {
            Self::Gatt { trace } => workload::gatt::run(output, context, *trace),
            Self::ScannableAdvertising {} => workload::scannable::run(output, context),
            Self::DirectedAdvertising {} => workload::directed::run(output, context),
            Self::SecureGatt {
                shutdown,
                irq_sampling,
            } => workload::secure_gatt::run(output, context, *shutdown, *irq_sampling),
            Self::Dtm {
                boots,
                minimum_packets,
                quiet_cycles,
            } => workload::run(*boots, *minimum_packets, *quiet_cycles, output, context),
            Self::DtmPeer { minimum_packets } => {
                workload::dtm_peer::run(*minimum_packets, output, context)
            }
        }
    }
}

impl BluetoothScenario {
    pub fn image(&self) -> ImageClass {
        match self {
            Self::Gatt { trace: false } => ImageClass::BluetoothGatt,
            Self::Gatt { trace: true } => ImageClass::DiagnosticBluetoothGattTrace,
            Self::SecureGatt { .. } => ImageClass::BluetoothSecureGatt,
            Self::Dtm { .. }
            | Self::DtmPeer { .. }
            | Self::ScannableAdvertising {}
            | Self::DirectedAdvertising {} => ImageClass::BluetoothDtm,
        }
    }

    /// The Linux adapter capabilities this workload needs, or `None` when it
    /// uses no Linux adapter.
    pub fn adapter_preflight(&self) -> Option<fn(Adapter) -> Result<()>> {
        Some(match self {
            Self::Gatt { .. }
            | Self::SecureGatt { .. }
            | Self::ScannableAdvertising {}
            | Self::DirectedAdvertising {} => fixture::att::preflight,
            Self::Dtm { .. } => fixture::preflight,
            Self::DtmPeer { .. } => return None,
        })
    }
}

#[cfg(test)]
mod tests;
