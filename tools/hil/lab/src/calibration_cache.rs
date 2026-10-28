//! Board-bound opaque PHY calibration records stored outside the checkout.
//!
//! Compatibility belongs to the target's format/schema/algorithm/profile;
//! physical replay validation remains the firmware's responsibility.
use crate::Result;
use oer_esp32s31_phy_probe::*;
use oer_lab_model::DeviceId;
use oer_os_process::lock::{FileLock, Mode};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::Read as _,
    os::unix::fs::{DirBuilderExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
};

pub const MAX_FILE_BYTES: usize = 8192;
const FILE_SCHEMA: u16 = 1;

/// The USB/stand identity and the target-derived radio identity are distinct.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CacheKey {
    chip: String,
    board: DeviceId,
    target: CalibrationCacheTarget,
}

impl CacheKey {
    pub fn new(chip: &str, board: DeviceId, target: CalibrationCacheTarget) -> Result<Self> {
        if chip.is_empty()
            || chip.len() > 32
            || !chip
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || target.identity.is_empty()
        {
            return Err("invalid calibration cache key".into());
        }
        Ok(Self {
            chip: chip.into(),
            board,
            target,
        })
    }

    pub fn target(&self) -> &CalibrationCacheTarget {
        &self.target
    }
}

/// Producing image and run, rather than the image currently consuming the file.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub image_commit: String,
    pub application_sha256: String,
    pub run_id: String,
    /// UTC seconds since the Unix epoch. Age does not invalidate a record.
    pub captured_unix_seconds: u64,
}

impl Provenance {
    fn valid(&self) -> bool {
        hex_string(&self.image_commit, 40)
            && hex_string(&self.application_sha256, 64)
            && !self.run_id.is_empty()
            && self.run_id.len() <= 128
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CachedRecord {
    file_schema: u16,
    key: CacheKey,
    metadata: CalibrationCacheMetadata,
    provenance: Provenance,
    boot_id: u64,
    ticket: u32,
    temperature: CalibrationTemperature,
    sha256: String,
    bytes_hex: String,
}

impl CachedRecord {
    pub fn metadata(&self) -> CalibrationCacheMetadata {
        self.metadata
    }
    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }
    pub fn bytes(&self) -> Result<Vec<u8>> {
        Ok(hex::decode(&self.bytes_hex)?)
    }

    fn valid(&self) -> bool {
        let Ok(bytes) = self.bytes() else {
            return false;
        };
        self.file_schema == FILE_SCHEMA
            && self.boot_id != 0
            && self.ticket != 0
            && self.provenance.valid()
            && compatible(&self.key.target, self.metadata)
            && self.metadata.valid_length()
            && bytes.len() == usize::from(self.metadata.length)
            && calibration_cache_crc32c(&bytes) == self.metadata.crc32c
            && oer_os_durable::sha256_bytes(&bytes) == self.sha256
    }
}

/// A bounded download, constructible only from a successful full registration.
pub struct SnapshotDownload {
    target: CalibrationCacheTarget,
    boot_id: u64,
    ticket: u32,
    metadata: CalibrationCacheMetadata,
    temperature: Option<CalibrationTemperature>,
    bytes: Vec<u8>,
}

impl SnapshotDownload {
    pub fn new(
        boot_id: u64,
        target: CalibrationCacheTarget,
        status: &CalibrationStartStatus,
    ) -> Result<Self> {
        let CalibrationStartState::Ready {
            capture,
            snapshot: Some(metadata),
            ..
        } = status.state
        else {
            return Err("calibration has no successful full snapshot".into());
        };
        if boot_id == 0
            || status.ticket == 0
            || capture.epoch != 1
            || !matches!(
                capture.lifecycle,
                CaptureLifecycle::Cold | CaptureLifecycle::Ieee802154
            )
            || !capture.full_calibration_performed
            || !matches!(
                capture.path,
                CalibrationPath::FullForCache | CalibrationPath::FullAfterRejectedCache
            )
            || !metadata.valid_length()
            || !compatible(&target, metadata)
        {
            return Err("calibration is not eligible for full snapshot promotion".into());
        }
        Ok(Self {
            target,
            boot_id,
            ticket: status.ticket,
            metadata,
            temperature: None,
            bytes: Vec::new(),
        })
    }

    pub fn next_request(&self) -> Option<ReadCalibrationSnapshot> {
        let first = u16::try_from(self.bytes.len()).ok()?;
        let remaining = self.metadata.length.checked_sub(first)?;
        (remaining != 0).then_some(ReadCalibrationSnapshot {
            ticket: self.ticket,
            first,
            count: remaining.min(CALIBRATION_CACHE_PAGE as u16) as u8,
        })
    }

    pub fn push(&mut self, page: CalibrationSnapshot) -> Result<()> {
        let expected = self
            .next_request()
            .ok_or("snapshot download already complete")?;
        if page.ticket != expected.ticket
            || page.first != expected.first
            || page.metadata != self.metadata
            || page.bytes.len() != usize::from(expected.count)
            || self
                .temperature
                .is_some_and(|temperature| temperature != page.temperature)
        {
            return Err("stale, truncated or inconsistent calibration snapshot page".into());
        }
        self.temperature = Some(page.temperature);
        self.bytes.extend_from_slice(&page.bytes);
        Ok(())
    }

    pub fn finish(self) -> Result<VerifiedFull> {
        if self.bytes.len() != usize::from(self.metadata.length)
            || calibration_cache_crc32c(&self.bytes) != self.metadata.crc32c
        {
            return Err("incomplete or corrupt calibration snapshot".into());
        }
        Ok(VerifiedFull { download: self })
    }
}

/// No public constructor or deserializer: only a verified full download can be promoted.
pub struct VerifiedFull {
    download: SnapshotDownload,
}

#[derive(Debug)]
pub enum Loaded {
    Snapshot(Box<CachedRecord>),
    Miss(CalibrationCacheMiss),
}

/// One last successful full record per board/chip/physical identity.
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn host() -> Result<Self> {
        Ok(Self::at(oer_os_durable::xdg::path(
            oer_os_durable::xdg::Base::Data,
            "phy-calibration",
        )?))
    }
    /// Explicit root for a stand instance or hermetic test.
    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }
    pub fn path(&self, key: &CacheKey) -> PathBuf {
        self.root
            .join(&key.chip)
            .join(key.board.compact())
            .join(hex::encode(&key.target.identity))
            .join("last-full.json")
    }

    /// Reads at most 8 KiB plus one sentinel byte, before JSON parsing.
    pub fn load(&self, key: &CacheKey) -> Loaded {
        let file = match File::open(self.path(key)) {
            Ok(file) => file,
            Err(error) => {
                return Loaded::Miss(if error.kind() == std::io::ErrorKind::NotFound {
                    CalibrationCacheMiss::Missing
                } else {
                    CalibrationCacheMiss::Unreadable
                });
            }
        };
        if !file.metadata().is_ok_and(|metadata| metadata.is_file()) {
            return Loaded::Miss(CalibrationCacheMiss::Unreadable);
        }
        let mut bytes = Vec::new();
        if file
            .take((MAX_FILE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .is_err()
        {
            return Loaded::Miss(CalibrationCacheMiss::Unreadable);
        }
        if bytes.len() > MAX_FILE_BYTES {
            return Loaded::Miss(CalibrationCacheMiss::InvalidFile);
        }
        let record: CachedRecord = match serde_json::from_slice(&bytes) {
            Ok(record) => record,
            Err(_) => return Loaded::Miss(CalibrationCacheMiss::InvalidFile),
        };
        if !record.valid() {
            return Loaded::Miss(CalibrationCacheMiss::InvalidFile);
        }
        if &record.key != key {
            return Loaded::Miss(CalibrationCacheMiss::Incompatible);
        }
        Loaded::Snapshot(Box::new(record))
    }

    /// A failed write is a persistence error, independent of successful PHY registration.
    /// The board lease serializes runs; this per-key lock also excludes other checkouts.
    pub fn promote(
        &self,
        key: &CacheKey,
        full: &VerifiedFull,
        provenance: Provenance,
    ) -> Result<PathBuf> {
        self.promote_with_archive(key, full, provenance, None)
    }

    /// Keep the exact producing record in a run artifact before advancing last-full.
    pub fn promote_archived(
        &self,
        key: &CacheKey,
        full: &VerifiedFull,
        provenance: Provenance,
        artifact: &Path,
    ) -> Result<PathBuf> {
        self.promote_with_archive(key, full, provenance, Some(artifact))
    }

    fn promote_with_archive(
        &self,
        key: &CacheKey,
        full: &VerifiedFull,
        provenance: Provenance,
        artifact: Option<&Path>,
    ) -> Result<PathBuf> {
        let full = &full.download;
        if key.target != full.target || !provenance.valid() {
            return Err("snapshot identity or producing provenance mismatch".into());
        }
        let record = CachedRecord {
            file_schema: FILE_SCHEMA,
            key: key.clone(),
            metadata: full.metadata,
            provenance,
            boot_id: full.boot_id,
            ticket: full.ticket,
            temperature: full
                .temperature
                .ok_or("snapshot has no temperature provenance")?,
            sha256: oer_os_durable::sha256_bytes(&full.bytes),
            bytes_hex: hex::encode(&full.bytes),
        };
        let bytes = serde_json::to_vec_pretty(&record)?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err("calibration cache file exceeds bound".into());
        }
        let path = self.path(key);
        let directory = path.parent().ok_or("cache path has no parent")?;
        private_directory(&self.root)?;
        let mut current = self.root.clone();
        for component in directory.strip_prefix(&self.root)?.components() {
            current.push(component);
            private_directory(&current)?;
        }
        let _lock = FileLock::try_acquire(&directory.join("promotion.lock"), Mode::Exclusive)?
            .ok_or("calibration cache promotion already in progress")?;
        if let Some(artifact) = artifact {
            if artifact == path || artifact.exists() {
                return Err("calibration cache artifact already exists".into());
            }
            oer_os_durable::atomic_write_private(artifact, &bytes)?;
        }
        oer_os_durable::atomic_write_private(&path, &bytes)?;
        Ok(path)
    }
}

fn private_directory(path: &Path) -> Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() {
        return Err("cache directory is not a directory".into());
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

pub fn compatible(target: &CalibrationCacheTarget, metadata: CalibrationCacheMetadata) -> bool {
    target.format == metadata.format
        && target.schema == metadata.schema
        && target.algorithm_revision == metadata.algorithm_revision
        && target.profile == metadata.profile
}

fn hex_string(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests;
