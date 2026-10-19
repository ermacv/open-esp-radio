//! The build provenance of a run's images: firmware subjects, source
//! materials, parameters and the build environment.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use oer_hil_schema::image::ImageClass;

pub const BUILD_PROVENANCE_SCHEMA: u16 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceRebuildStatus {
    SourceSnapshot,
    CleanCommit,
    TrackedPatch,
    Incomplete,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceLimitation {
    RepositoryStateNotCaptured,
    SourceRemoteUnavailable,
    UntrackedContentNotArchived,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourceFileIdentity {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SourceMaterial {
    pub name: String,
    pub checkout_path: PathBuf,
    pub remote: Option<String>,
    pub commit: String,
    pub dirty: bool,
    pub workspace_sha256: String,
    pub rebuild_status: SourceRebuildStatus,
    pub tracked_patch_path: Option<PathBuf>,
    pub tracked_patch_size_bytes: Option<u64>,
    pub tracked_patch_sha256: Option<String>,
    pub untracked_files: Vec<SourceFileIdentity>,
    pub limitations: Vec<SourceLimitation>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BuildFileMaterial {
    pub name: String,
    pub path: PathBuf,
    pub archive_path: Option<PathBuf>,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BuildParameters {
    pub image: ImageClass,
    /// None is retained when decoding older bundles with no recorded selection.
    #[serde(default)]
    pub network: Option<String>,
    pub runtime_profile: String,
    pub target: String,
    pub runtime_features: String,
    /// The seed the runtime was linked with; absent for the natural order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout_seed: Option<std::num::NonZeroU32>,
    /// Runtime features added to or removed from the class's own; an image
    /// built with any is an experiment's, never its class's.
    #[serde(
        default,
        skip_serializing_if = "oer_hil_schema::image::FeatureDelta::is_empty"
    )]
    pub features: oer_hil_schema::image::FeatureDelta,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BuildSubjectRole {
    Application,
    BootstrapElf,
    RuntimeBin,
    RuntimeElf,
    Bootloader,
    PartitionTable,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BuildSubject {
    pub role: BuildSubjectRole,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub sha256: String,
}

/// One host tool that took part in a firmware build.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BuildTool {
    pub name: String,
    pub program: String,
    pub version: Option<String>,
}

/// Host tools and inherited settings under which a firmware image was built.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BuildEnvironment {
    pub tools: Vec<BuildTool>,
    pub inherited_rustflags: Option<String>,
    pub inherited_encoded_rustflags: Option<String>,
    pub cargo_incremental: String,
    pub source_date_epoch: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BuildReproducibility {
    Unverified,
    Verified,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BuildProvenance {
    pub schema: u16,
    pub build_id: String,
    pub build_type: String,
    pub parameters: BuildParameters,
    pub sources: Vec<SourceMaterial>,
    pub files: Vec<BuildFileMaterial>,
    pub environment: BuildEnvironment,
    pub subjects: Vec<BuildSubject>,
    pub source_reconstructable: bool,
    pub reproducibility: BuildReproducibility,
}
