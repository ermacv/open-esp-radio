use std::{fmt, path::PathBuf, str::FromStr};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Provider {
    LinuxNet,
}

impl Provider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LinuxNet => "linux-net",
        }
    }

    pub(crate) fn runtime_contract(self) -> &'static str {
        match self {
            Self::LinuxNet => {
                "schema=15 station_ap=ht20,ht40,he20 legacy_ap=dsss client=ht,non-ht client_aqm=1 observer=20,40 managed=1 rfkill=restore"
            }
        }
    }

    pub(crate) fn artifact_specs(self) -> &'static [ArtifactSpec] {
        match self {
            Self::LinuxNet => &LINUX_NET_ARTIFACTS,
        }
    }

    pub(crate) fn policy_path(self) -> &'static str {
        match self {
            Self::LinuxNet => "/etc/sudoers.d/open-radio-net",
        }
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output.write_str(self.as_str())
    }
}

impl FromStr for Provider {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "linux-net" => Ok(Self::LinuxNet),
            _ => Err("provider must be linux-net".to_owned()),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactRole {
    NetworkLauncher,
    NetworkHelper,
    ProbeLauncher,
    ProbeHelper,
    Hostapd,
    HostapdProvenance,
}

impl ArtifactRole {
    pub(crate) fn is_launcher(self) -> bool {
        matches!(self, Self::NetworkLauncher | Self::ProbeLauncher)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ArtifactSpec {
    pub(crate) role: ArtifactRole,
    pub(crate) source: &'static str,
    pub(crate) name: &'static str,
    pub(crate) target: &'static str,
    pub(crate) mode: u32,
}

const LINUX_NET_ARTIFACTS: [ArtifactSpec; 6] = [
    ArtifactSpec {
        role: ArtifactRole::NetworkLauncher,
        source: "target/debug/open-radio-net-launcher",
        name: "open-radio-net-launcher",
        target: "/usr/local/libexec/open-radio-net-launcher",
        mode: 0o555,
    },
    ArtifactSpec {
        role: ArtifactRole::NetworkHelper,
        source: "hil/host/linux-net/open-radio-net",
        name: "open-radio-net",
        target: "/usr/local/sbin/open-radio-net",
        mode: 0o555,
    },
    ArtifactSpec {
        role: ArtifactRole::ProbeLauncher,
        source: "target/debug/open-radio-probe-launcher",
        name: "open-radio-probe-launcher",
        target: "/usr/local/libexec/open-radio-probe-launcher",
        mode: 0o555,
    },
    ArtifactSpec {
        role: ArtifactRole::ProbeHelper,
        source: "target/debug/open-radio-probe",
        name: "open-radio-probe",
        target: "/usr/local/libexec/open-radio-probe",
        mode: 0o555,
    },
    ArtifactSpec {
        role: ArtifactRole::Hostapd,
        source: "target/hil/hostapd/hostapd",
        name: "open-radio-hostapd",
        target: "/usr/local/libexec/open-radio-hostapd",
        mode: 0o555,
    },
    ArtifactSpec {
        role: ArtifactRole::HostapdProvenance,
        source: "target/hil/hostapd/provenance.json",
        name: "open-radio-hostapd.json",
        target: "/usr/local/libexec/open-radio-hostapd.json",
        mode: 0o444,
    },
];

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactState {
    Missing,
    PresentUnverified,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedArtifact {
    pub role: ArtifactRole,
    pub source: PathBuf,
    pub target: PathBuf,
    pub state: ArtifactState,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstallPlan {
    pub schema: u32,
    pub provider: Provider,
    pub artifacts: Vec<PlannedArtifact>,
    pub build_steps: Vec<String>,
    pub authorization_boundary: String,
    pub activation_boundary: String,
    pub readonly_verification: Vec<String>,
    pub automatic_hardware_checks: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceIdentity {
    pub commit: String,
    pub dirty: bool,
    pub workspace_state_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub role: ArtifactRole,
    pub file_name: String,
    pub target: PathBuf,
    pub size_bytes: u64,
    pub sha256: String,
    pub mode: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Bundle {
    pub schema: u32,
    pub provider: Provider,
    pub generation: String,
    pub operator: String,
    pub source: SourceIdentity,
    pub runtime_contract: String,
    pub artifacts: Vec<Artifact>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstallState {
    Planned,
    Prepared,
    Activated,
    SoftwareVerified,
    RolledBack,
    RecoveryRequired,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InstallResult {
    pub schema: u32,
    pub provider: Provider,
    pub transaction: String,
    pub generation: String,
    pub source: SourceIdentity,
    pub artifacts: Vec<Artifact>,
    pub runtime_contract: String,
    pub policy_sha256: String,
    pub previous_generation: Option<String>,
    pub checks: Vec<String>,
    pub state: InstallState,
    pub changed: bool,
    pub activation_committed: bool,
    pub software_verified: bool,
    pub hardware_acceptance_performed: bool,
    pub primary_error: Option<String>,
    pub recovery_error: Option<String>,
}
