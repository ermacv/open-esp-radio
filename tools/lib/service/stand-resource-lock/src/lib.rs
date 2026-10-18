//! The lock of a shared fixture resource (a Bluetooth adapter, a host
//! radio, an OpenWrt host's boot): a `flock` lock file
//! (`oer_process::lock::FileLock`) per resource key in the stand file's lock
//! directory (`open-esp-radio/leases` in the XDG cache directory).
//!
//! A grant orders access; the lock file excludes every other process of the
//! host from the fixture while it is used, including one of a private
//! arbiter. A lock is taken only once the grant that ordered it is held, and
//! released before it. Boards are excluded by their device lock
//! (`oer-device-lock`), which the arbiter takes for every leased board.
#![forbid(unsafe_code)]

use std::{
    io::{Read as _, Seek as _, SeekFrom, Write as _},
    path::{Path, PathBuf},
};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

use oer_process::lock::{FileLock, Mode};

/// Another process holds the lock.
#[derive(Debug)]
pub struct Busy(String);

impl std::fmt::Display for Busy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Busy {}

/// An exclusive lock file naming its holder, released when dropped.
struct LockFile(FileLock);

impl LockFile {
    fn try_acquire(path: &Path, what: &str) -> Result<Self> {
        let Some(lock) = FileLock::try_acquire(path, Mode::Exclusive)? else {
            let mut owner = String::new();
            std::fs::File::open(path)?.read_to_string(&mut owner)?;
            let owner = owner.trim();
            return Err(Busy(format!(
                "{what} is held by {} ({})",
                if owner.is_empty() {
                    "another process"
                } else {
                    owner
                },
                path.display()
            ))
            .into());
        };
        // The guard exists before the fallible owner record, so every path
        // after the acquisition releases it.
        let lock = Self(lock);
        let mut file = lock.0.file();
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        writeln!(
            file,
            "pid={} command={}",
            std::process::id(),
            std::env::args().collect::<Vec<_>>().join(" ")
        )?;
        file.flush()?;
        Ok(lock)
    }
}

/// A fixture resource's lock, by its key (a Bluetooth adapter, a radio, an
/// OpenWrt host's boot).
pub struct ResourceLock {
    _file: LockFile,
}

impl ResourceLock {
    /// Lock the resource `key` now, or fail with [`Busy`].
    pub fn try_acquire(key: &str) -> Result<Self> {
        Self::try_acquire_in(&oer_stand_file::paths::locks()?, key)
    }

    /// Lock the resource `key`, waiting while a process outside the
    /// arbiter's order still holds it.
    pub fn acquire(key: &str) -> Result<Self> {
        wait_while_busy(|| Self::try_acquire(key))
    }

    /// [`Self::try_acquire`] with the lock files in `directory`: a test's
    /// own.
    pub fn try_acquire_in(directory: &Path, key: &str) -> Result<Self> {
        Ok(Self {
            _file: LockFile::try_acquire(
                &resource_path(directory, key),
                &format!("the HIL fixture resource `{key}`"),
            )?,
        })
    }
}

fn resource_path(directory: &Path, key: &str) -> PathBuf {
    directory
        .join(format!(
            "resource-{}",
            oer_durable::sha256_bytes(key.as_bytes())
        ))
        .join("fixture.lock")
}

/// Retry `lock` while a process outside the arbiter (for example a checkout
/// without it) still holds what it locks; the wait is reported once.
pub fn wait_while_busy<T>(mut lock: impl FnMut() -> Result<T>) -> Result<T> {
    let mut reported = false;
    loop {
        match lock() {
            Err(error) if error.is::<Busy>() => {
                if !reported {
                    eprintln!("stand: waiting for a lock held outside the queue: {error}");
                    reported = true;
                }
                oer_process::sleep(std::time::Duration::from_secs(1))?;
            }
            result => return result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_resource_is_held_by_one_owner_until_it_is_dropped() {
        let directory = tempfile::tempdir().unwrap();
        let held = ResourceLock::try_acquire_in(directory.path(), "bluetooth:/sys/x").unwrap();
        assert!(
            ResourceLock::try_acquire_in(directory.path(), "bluetooth:/sys/x")
                .err()
                .unwrap()
                .is::<Busy>()
        );
        assert!(ResourceLock::try_acquire_in(directory.path(), "local-radio:/sys/y").is_ok());
        drop(held);
        assert!(ResourceLock::try_acquire_in(directory.path(), "bluetooth:/sys/x").is_ok());
    }

    #[test]
    fn waiting_for_a_lock_ends_once_it_is_free_or_fails_otherwise() {
        let mut attempts = 0;
        let locked = wait_while_busy(|| {
            attempts += 1;
            if attempts == 1 {
                Err(Busy(String::from("held")).into())
            } else {
                Ok(attempts)
            }
        })
        .unwrap();
        assert_eq!(locked, 2);
        assert!(wait_while_busy(|| -> Result<()> { Err("other".into()) }).is_err());
    }
}
