#![forbid(unsafe_code)]
//! The image bundle format: everything one image build made, in one directory, described
//! by its `bundle.json`.
//!
//! A bundle holds the runtime ELF, the encoded flash contents (application,
//! bootloader, partition table and, where the chip selects slots by OTA
//! data, the OTA selection), the gate reports, the effective lock files and
//! `source-inputs.json`. [`ImageBundle::segments`] is what a flash writes:
//! the files at the chip's flash map offsets, the OTA selection last.
//!
//! A bundle is **published** whole: a builder fills a staging directory, a
//! sibling of its output ([`staging_directory`]), and
//! [`ImageBundle::publish`] records the SHA-256 and length of every flash
//! file in the manifest and moves the staging directory into place by
//! rename. A build that fails never publishes, so the previously published
//! bundle stays as it was. [`ImageBundle::load`] verifies every flash file
//! against the manifest's digest, and [`ImageBundle::snapshot`] reads the
//! flash files once into owned bytes, checked against the same digests:
//! what a write hashes and writes.

use std::{
    collections::BTreeSet,
    num::NonZeroU32,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

pub use oer_chip_profile::{Boot, FlashMap};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The bundle's description, beside its files.
pub const MANIFEST: &str = "bundle.json";
/// Format of [`MANIFEST`].
pub const SCHEMA: u32 = 5;

/// Every file name a bundle uses.
pub mod files {
    pub const RUNTIME_ELF: &str = "runtime.elf";
    pub const RUNTIME_BIN: &str = "runtime.bin";
    pub const BOOTSTRAP_ELF: &str = "bootstrap.elf";
    pub const APPLICATION: &str = "application.bin";
    pub const BOOTLOADER: &str = "bootloader.bin";
    pub const PARTITIONS: &str = "partitions.bin";
    pub const OTADATA: &str = "otadata.bin";
    pub const SOURCE_INPUTS: &str = "source-inputs.json";
    pub const RUNTIME_LOCK: &str = "runtime-Cargo.lock";
    pub const BOOTSTRAP_LOCK: &str = "bootstrap-Cargo.lock";
    pub const PLACEMENT_REPORT: &str = "placement.txt";
    pub const BUILD_LOG: &str = "build.log";
    /// The results of a build's requested checks when one of them failed:
    /// the build then leaves no bundle, but its checks stay machine-readable.
    pub const CHECKS: &str = "checks.json";
}

/// Which ELF of an image a check examined.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CheckedElf {
    /// The runtime (application) ELF.
    Runtime,
    /// A staged boot's bootstrap ELF.
    Bootstrap,
}

/// The result of one requested check. Only [`CheckResult::Passed`] and
/// [`CheckResult::NotApplicable`] let a build succeed.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "result", content = "reason")]
pub enum CheckResult {
    Passed,
    /// The check ran and the image does not hold the property.
    Failed(String),
    /// The chip data or policy the check needs is absent: the property was
    /// not shown, whether or not it applies. Fails a requested check.
    MissingContract(String),
    /// The property does not apply to this image, for the explicit reason
    /// given (never because data is missing).
    NotApplicable(String),
}

impl CheckResult {
    /// Whether a build that requested the check may succeed with it.
    pub fn passes(&self) -> bool {
        matches!(self, Self::Passed | Self::NotApplicable(_))
    }
}

impl std::fmt::Display for CheckResult {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Passed => write!(formatter, "passed"),
            Self::Failed(reason) => write!(formatter, "failed: {reason}"),
            Self::MissingContract(what) => write!(formatter, "missing contract: {what}"),
            Self::NotApplicable(reason) => write!(formatter, "not applicable: {reason}"),
        }
    }
}

/// One requested check of one ELF and its result.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CheckRecord {
    pub elf: CheckedElf,
    pub check: String,
    pub result: CheckResult,
}

/// One build's outputs.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ImageBundle {
    schema: u32,
    /// The bundle's directory; not recorded, it is where the manifest lies.
    #[serde(skip)]
    pub directory: PathBuf,
    pub chip: String,
    pub rust_target: String,
    pub boot: Boot,
    /// Where the flash contents go.
    pub flash: FlashMap,
    /// The staged boot's packed runtime and bootstrap ELF.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staged: Option<Staged>,
    /// Whether the bundle holds the OTA selection (`otadata.bin`).
    pub otadata: bool,
    /// Each effective lock file: the committed catalog it copies, and its
    /// file in the bundle.
    pub locks: Vec<Lock>,
    /// The gate reports, by file name.
    pub reports: Vec<String>,
    /// The seed the runtime's link order was shuffled by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout_seed: Option<NonZeroU32>,
    /// The reviewed ROM summaries the stack analysis applied.
    pub rom_summaries: BTreeSet<String>,
    /// Every requested check of every ELF with its result; empty when the
    /// build requested none.
    pub checks: Vec<CheckRecord>,
    /// What the gates passed with: partial or conditional bounds.
    pub warnings: Vec<String>,
    /// Every flash file's digest, in [`ImageBundle::segments`] order, as
    /// [`ImageBundle::publish`] recorded them.
    pub flash_files: Vec<FlashFile>,
}

/// A flash file of a published bundle: what its bytes were when it was
/// published.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FlashFile {
    /// Its file name in the bundle.
    pub file: String,
    pub sha256: String,
    pub bytes: u64,
}

/// One flash region of a [`Snapshot`]: its bytes, owned.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnapshotSegment {
    pub offset: u32,
    pub description: &'static str,
    pub data: Vec<u8>,
    /// The SHA-256 of `data`.
    pub sha256: String,
}

/// A bundle's flash contents read once into owned bytes, verified against
/// its manifest: what a write hashes and writes, whatever later happens to
/// the bundle's files.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Snapshot {
    /// The chip the bundle is for.
    pub chip: String,
    /// How a written image starts.
    pub start: oer_chip_profile::Start,
    /// Every segment in write order.
    pub segments: Vec<SnapshotSegment>,
}

impl Snapshot {
    /// Write each segment into the private directory `directory` as its own
    /// file; the offset and file of each, in write order: what a writer that
    /// programs files (OpenOCD) reads.
    pub fn materialize(&self, directory: &Path) -> Result<Vec<(u32, PathBuf)>> {
        self.segments
            .iter()
            .enumerate()
            .map(|(index, segment)| {
                let file = directory.join(format!("{index}-{:#x}.bin", segment.offset));
                std::fs::write(&file, &segment.data)?;
                Ok((segment.offset, file))
            })
            .collect()
    }
}

/// The staging directory a build of the bundle in `output` fills before
/// [`ImageBundle::publish`] moves it into place: a sibling of `output`, so
/// the move is a rename within one file system.
pub fn staging_directory(output: &Path) -> Result<PathBuf> {
    sibling(output, "staging")
}

/// `output`'s hidden sibling `.<name>.<suffix>`.
fn sibling(output: &Path, suffix: &str) -> Result<PathBuf> {
    let name = output
        .file_name()
        .ok_or_else(|| format!("{} names no directory", output.display()))?
        .to_string_lossy();
    let parent = output
        .parent()
        .ok_or_else(|| format!("{} has no parent", output.display()))?;
    Ok(parent.join(format!(".{name}.{suffix}")))
}

/// The staged boot's own files.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Staged {
    /// The bootstrap's package, which embeds the packed runtime.
    pub bootstrap_package: String,
}

/// An effective lock file of a build.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Lock {
    /// The committed `Cargo.lock`, relative to the repository root.
    pub committed: PathBuf,
    /// Its effective copy's file name in the bundle.
    pub file: String,
}

/// One region a flash writes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Segment {
    pub offset: u32,
    pub file: PathBuf,
    pub description: &'static str,
}

impl ImageBundle {
    /// An empty bundle of `profile`'s chip in `directory`, which a pipeline
    /// fills: no files yet, no gate run.
    pub fn new(directory: &Path, profile: &oer_chip_profile::Profile, flash: FlashMap) -> Self {
        Self {
            schema: SCHEMA,
            directory: directory.to_owned(),
            chip: profile.id.clone(),
            rust_target: profile.rust_target.clone(),
            boot: profile.boot,
            flash,
            staged: None,
            otadata: false,
            locks: Vec::new(),
            reports: Vec::new(),
            layout_seed: None,
            rom_summaries: BTreeSet::new(),
            checks: Vec::new(),
            warnings: Vec::new(),
            flash_files: Vec::new(),
        }
    }

    /// The bundle published in `directory`, every flash file verified
    /// against the digest its manifest recorded.
    pub fn load(directory: &Path) -> Result<Self> {
        let path = directory.join(MANIFEST);
        let value: serde_json::Value = serde_json::from_slice(
            &std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?,
        )
        .map_err(|error| format!("{}: {error}", path.display()))?;
        if value["schema"].as_u64() != Some(u64::from(SCHEMA)) {
            return Err(format!(
                "{} has schema {}; this tool reads schema {SCHEMA}: build the bundle again",
                path.display(),
                value["schema"]
            )
            .into());
        }
        let mut bundle: Self = serde_json::from_value(value)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        bundle.directory = directory.to_owned();
        for segment in bundle.segments() {
            if !segment.file.is_file() {
                return Err(format!(
                    "the bundle in {} lacks its {}",
                    directory.display(),
                    segment.description
                )
                .into());
            }
            let recorded = bundle.recorded(&segment)?;
            if oer_durable::sha256_file(&segment.file)? != recorded.sha256 {
                return Err(bundle.changed(&segment).into());
            }
        }
        Ok(bundle)
    }

    /// The manifest's digest of `segment`'s file.
    fn recorded(&self, segment: &Segment) -> Result<&FlashFile> {
        let name = segment
            .file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.flash_files
            .iter()
            .find(|file| file.file == name)
            .ok_or_else(|| {
                format!(
                    "the manifest in {} records no digest of its {}",
                    self.directory.display(),
                    segment.description
                )
                .into()
            })
    }

    fn changed(&self, segment: &Segment) -> String {
        format!(
            "the {} of the bundle in {} is not the file it was published with",
            segment.description,
            self.directory.display()
        )
    }

    /// The bundle's flash contents, each file read once into owned bytes and
    /// verified against the manifest's digest: the one source of what a
    /// write hashes and writes.
    pub fn snapshot(&self) -> Result<Snapshot> {
        let segments = self
            .segments()
            .into_iter()
            .map(|segment| {
                let data = std::fs::read(&segment.file)
                    .map_err(|error| format!("{}: {error}", segment.file.display()))?;
                let sha256 = oer_durable::sha256_bytes(&data);
                let recorded = self.recorded(&segment)?;
                if sha256 != recorded.sha256 || data.len() as u64 != recorded.bytes {
                    return Err(self.changed(&segment).into());
                }
                Ok(SnapshotSegment {
                    offset: segment.offset,
                    description: segment.description,
                    data,
                    sha256,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Snapshot {
            chip: self.chip.clone(),
            start: self.flash.start,
            segments,
        })
    }

    /// Publish this bundle, built in its staging directory
    /// ([`staging_directory`] of `output`), as the bundle in `output`:
    /// record every flash file's digest and the manifest in the staging
    /// directory, then move it into place by rename, replacing the bundle
    /// published there before. A caller that fails before this leaves the
    /// published bundle as it was.
    pub fn publish(mut self, output: &Path) -> Result<Self> {
        if self.directory != staging_directory(output)? {
            return Err(format!(
                "the bundle in {} is not staged for {}",
                self.directory.display(),
                output.display()
            )
            .into());
        }
        self.flash_files = self
            .segments()
            .iter()
            .map(|segment| {
                Ok(FlashFile {
                    file: segment
                        .file
                        .file_name()
                        .ok_or("a flash file has no name")?
                        .to_string_lossy()
                        .into_owned(),
                    sha256: oer_durable::sha256_file(&segment.file)?,
                    bytes: std::fs::metadata(&segment.file)?.len(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        oer_durable::atomic_json(&self.directory.join(MANIFEST), &self)?;
        // The replaced bundle moves aside first: a rename does not replace a
        // non-empty directory.
        let replaced = sibling(output, "replaced")?;
        remove_directory(&replaced)?;
        if output.exists() {
            std::fs::rename(output, &replaced)?;
        }
        std::fs::rename(&self.directory, output)?;
        remove_directory(&replaced)?;
        self.directory = output.to_owned();
        Ok(self)
    }

    /// The bundle's file `name`.
    pub fn path(&self, name: &str) -> PathBuf {
        self.directory.join(name)
    }

    pub fn runtime_elf(&self) -> PathBuf {
        self.path(files::RUNTIME_ELF)
    }

    /// The encoded ESP application image.
    pub fn application(&self) -> PathBuf {
        self.path(files::APPLICATION)
    }

    pub fn bootloader(&self) -> PathBuf {
        self.path(files::BOOTLOADER)
    }

    pub fn partitions(&self) -> PathBuf {
        self.path(files::PARTITIONS)
    }

    /// The OTA selection of the first application slot, when the chip's
    /// bootloader selects slots by OTA data.
    pub fn otadata(&self) -> Option<PathBuf> {
        self.otadata.then(|| self.path(files::OTADATA))
    }

    /// The staged boot's packed runtime.
    pub fn runtime_bin(&self) -> Option<PathBuf> {
        self.staged.as_ref().map(|_| self.path(files::RUNTIME_BIN))
    }

    /// The staged boot's bootstrap ELF.
    pub fn bootstrap_elf(&self) -> Option<PathBuf> {
        self.staged
            .as_ref()
            .map(|_| self.path(files::BOOTSTRAP_ELF))
    }

    pub fn source_inputs(&self) -> PathBuf {
        self.path(files::SOURCE_INPUTS)
    }

    /// The effective lock file that copies the committed `committed`.
    pub fn lock(&self, committed: &Path) -> Option<PathBuf> {
        self.locks
            .iter()
            .find(|lock| lock.committed == committed)
            .map(|lock| self.path(&lock.file))
    }

    /// The bundle of this bundle's flash files published in `output`, with
    /// the bytes of `application` (an archived copy of this bundle's) as its
    /// application: what a run flashes, so it writes exactly the bytes it
    /// archived even when a later build replaces this bundle. The ELF files,
    /// reports and locks stay here.
    pub fn with_application(&self, application: &Path, output: &Path) -> Result<Self> {
        if std::fs::read(application)? != std::fs::read(self.application())? {
            return Err(format!(
                "{} is not the application of the bundle in {}",
                application.display(),
                self.directory.display()
            )
            .into());
        }
        let staging = staging_directory(output)?;
        remove_directory(&staging)?;
        std::fs::create_dir_all(&staging)?;
        let mut bundle = Self::new_like(self, &staging);
        std::fs::copy(application, bundle.application())?;
        for file in [
            Some(files::BOOTLOADER),
            Some(files::PARTITIONS),
            self.otadata.then_some(files::OTADATA),
        ]
        .into_iter()
        .flatten()
        {
            std::fs::copy(self.path(file), staging.join(file))?;
        }
        bundle.otadata = self.otadata;
        bundle.publish(output)
    }

    fn new_like(other: &Self, directory: &Path) -> Self {
        Self {
            schema: SCHEMA,
            directory: directory.to_owned(),
            chip: other.chip.clone(),
            rust_target: other.rust_target.clone(),
            boot: other.boot,
            flash: other.flash.clone(),
            staged: None,
            otadata: false,
            locks: Vec::new(),
            reports: Vec::new(),
            layout_seed: other.layout_seed,
            rom_summaries: BTreeSet::new(),
            checks: Vec::new(),
            warnings: Vec::new(),
            flash_files: Vec::new(),
        }
    }

    /// What a flash writes, in order: the bootloader, the partition table,
    /// the application, and the OTA selection last, so an interrupted write
    /// leaves the previous selection pointing at an image whose checksum no
    /// longer validates instead of a half-written one.
    pub fn segments(&self) -> Vec<Segment> {
        let mut segments = vec![
            Segment {
                offset: self.flash.bootloader,
                file: self.bootloader(),
                description: "bootloader",
            },
            Segment {
                offset: self.flash.partition_table,
                file: self.partitions(),
                description: "partition table",
            },
            Segment {
                offset: self.flash.application,
                file: self.application(),
                description: "application",
            },
        ];
        if let Some(file) = self.otadata() {
            segments.push(Segment {
                offset: self.flash.otadata,
                file,
                description: "OTA selection",
            });
        }
        segments
    }
}

/// Remove the directory `path` and everything in it, if it exists.
pub fn remove_directory(path: &Path) -> Result<()> {
    match std::fs::remove_dir_all(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests;
