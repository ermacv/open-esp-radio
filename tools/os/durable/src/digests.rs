//! SHA-256 digests of files remembered across processes.
//!
//! An immutable file (a finished run bundle's) is hashed once: its digest is
//! remembered for the file's device, inode, size, modification time and
//! status-change time. Any write to the file changes its status-change time,
//! which user code cannot set, so a changed file is always hashed again.
//! Hard links of one file share one entry. The cache is not authenticated:
//! a command whose results enter tracked files [`DigestCache::disable`]s it.

use std::{
    collections::HashMap,
    fs,
    os::unix::fs::MetadataExt as _,
    path::{Path, PathBuf},
    sync::Mutex,
};

use crate::Result;

/// Entries unseen this long are dropped when the cache is saved.
const MAX_AGE_SECONDS: u64 = 30 * 24 * 3600;

/// File digests remembered in one cache file, or none when disabled.
pub struct DigestCache(Mutex<Inner>);

struct Inner {
    path: Option<PathBuf>,
    /// Digest and the last time a process used it, by file identity.
    entries: HashMap<String, (String, u64)>,
    added: bool,
}

impl DigestCache {
    /// The cache kept in `path`, or a disabled one (every file hashed) for
    /// `None`. An unreadable cache file starts empty.
    pub fn open(path: Option<PathBuf>) -> Self {
        let entries = path
            .as_ref()
            .and_then(|path| fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self(Mutex::new(Inner {
            path,
            entries,
            added: false,
        }))
    }

    /// Hash every file for the rest of this cache's life and never save it.
    pub fn disable(&self) {
        self.lock().path = None;
    }

    /// The SHA-256 of the regular file at `path`, from the cache when its
    /// identity and times are unchanged.
    pub fn sha256_file(&self, path: &Path) -> Result<String> {
        let key = key(&fs::metadata(path)?);
        let enabled = {
            let mut inner = self.lock();
            let enabled = inner.path.is_some();
            if enabled && let Some((digest, seen)) = inner.entries.get_mut(&key) {
                *seen = crate::unix_seconds();
                let digest = digest.clone();
                inner.added = true;
                return Ok(digest);
            }
            enabled
        };
        let digest = crate::sha256_file(path)?;
        // A file written while it was hashed is not remembered.
        if enabled && self::key(&fs::metadata(path)?) == key {
            let mut inner = self.lock();
            inner
                .entries
                .insert(key, (digest.clone(), crate::unix_seconds()));
            inner.added = true;
        }
        Ok(digest)
    }

    /// Persist newly used digests, dropping entries unseen for a month.
    /// Failure only loses the cache.
    pub fn save(&self) {
        let mut inner = self.lock();
        let Some(path) = inner.path.clone().filter(|_| inner.added) else {
            return;
        };
        let oldest = crate::unix_seconds().saturating_sub(MAX_AGE_SECONDS);
        inner.entries.retain(|_, (_, seen)| *seen >= oldest);
        if crate::atomic_write(
            &path,
            &serde_json::to_vec(&inner.entries).unwrap_or_default(),
        )
        .is_ok()
        {
            inner.added = false;
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn key(metadata: &fs::Metadata) -> String {
    format!(
        "{:?}",
        (
            metadata.dev(),
            metadata.ino(),
            metadata.size(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ctime(),
            metadata.ctime_nsec(),
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_disabled_cache_hashes_every_time_and_saves_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("file");
        let cache_file = directory.path().join("sha256.json");
        fs::write(&file, b"one").unwrap();
        let cache = DigestCache::open(Some(cache_file.clone()));
        cache.disable();
        cache.sha256_file(&file).unwrap();
        cache.save();
        assert!(!cache_file.exists());
        let none = DigestCache::open(None);
        assert_eq!(
            none.sha256_file(&file).unwrap(),
            crate::sha256_bytes(b"one")
        );
    }
}
