//! Durable host files shared by every repository tool: atomic replacement,
//! content digests (remembered across processes by [`digests`]),
//! JSON Lines logs ([`jsonl`]), wall-clock timestamps and the user's state directories ([`xdg`]).
#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write as _},
    os::unix::fs::OpenOptionsExt as _,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use sha2::{Digest, Sha256};

pub mod digests;
pub mod inputs;
pub mod jsonl;
pub mod xdg;

/// The error type of every durable file operation.
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Distinguishes concurrent temporary files written by one process.
pub static UNIQUE_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Replace `path` atomically with `value` as pretty JSON and a final newline.
pub fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    atomic_write(path, &bytes)
}

/// Replace `path` atomically with `bytes`: a reader sees the old or the new
/// content, never a part, and the new content survives a crash once this
/// returns. Creates the parent directory.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    atomic_write_mode(path, bytes, 0o666)
}

/// The durable replacement of [`atomic_write`], with an owner-only file.
/// The caller owns directory permissions; the temporary is private from creation.
pub fn atomic_write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    atomic_write_mode(path, bytes, 0o600)
}

fn atomic_write_mode(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("path has no parent: {}", path.display()))?;
    fs::create_dir_all(parent)?;
    let counter = UNIQUE_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let file_name = path
        .file_name()
        .ok_or_else(|| format!("path has no file name: {}", path.display()))?
        .to_string_lossy();
    let temporary = parent.join(format!(".{file_name}.tmp-{}-{counter}", std::process::id()));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(mode)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.flush()?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    result.map_err(|error| after_cleanup(error, &temporary, fs::remove_file(&temporary)))
}

/// The refusal of a replacement whose temporary was then removed: `error`
/// itself when nothing is left behind (the temporary is gone, or was never
/// made), else `error` with the temporary that stays and why.
fn after_cleanup(
    error: Box<dyn std::error::Error + Send + Sync>,
    temporary: &Path,
    cleanup: std::io::Result<()>,
) -> Box<dyn std::error::Error + Send + Sync> {
    match cleanup {
        Ok(()) => error,
        Err(cleanup) if cleanup.kind() == std::io::ErrorKind::NotFound => error,
        Err(cleanup) => format!(
            "{error}; its temporary {} was not removed: {cleanup}",
            temporary.display()
        )
        .into(),
    }
}

/// The SHA-256 of the file at `path`, as lowercase hex.
pub fn sha256_file(path: &Path) -> Result<String> {
    sha256_reader(&mut File::open(path)?)
}

/// The SHA-256 of everything `reader` gives, as lowercase hex.
fn sha256_reader(reader: &mut impl Read) -> Result<String> {
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let bytes = buffer.get(..read).ok_or_else(|| {
            format!(
                "a read reported {read} bytes into a buffer of {}",
                buffer.len()
            )
        })?;
        digest.update(bytes);
    }
    Ok(format!("{:x}", digest.finalize()))
}

/// The SHA-256 of `bytes`, as lowercase hex.
pub fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Wall-clock milliseconds since the Unix epoch; zero for a clock set
/// before it.
pub fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

/// Wall-clock seconds since the Unix epoch; zero for a clock set before it.
pub fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_json_replaces_the_file_and_leaves_no_temporary() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested/state.json");
        atomic_json(&path, &serde_json::json!({"a": 1})).unwrap();
        atomic_json(&path, &serde_json::json!({"a": 2})).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "{\n  \"a\": 2\n}\n");
        let names: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(names, ["state.json"]);
    }

    #[test]
    fn file_and_byte_digests_agree() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("file");
        fs::write(&path, b"abc").unwrap();
        let expected = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(sha256_bytes(b"abc"), expected);
        assert_eq!(sha256_file(&path).unwrap(), expected);
    }

    #[test]
    fn a_read_that_reports_more_than_it_was_lent_is_refused() {
        struct Overreport;
        impl Read for Overreport {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                Ok(buffer.len() + 1)
            }
        }
        let error = sha256_reader(&mut Overreport).unwrap_err().to_string();
        assert_eq!(error, "a read reported 65537 bytes into a buffer of 65536");
    }

    #[test]
    fn a_failed_replacement_names_the_temporary_it_could_not_remove() {
        // The file's own name fits a directory entry and its temporary's does
        // not, so the temporary is neither made nor removable.
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("n".repeat(255));
        let error = atomic_write(&path, b"bytes").unwrap_err().to_string();
        let temporary = format!(".{}.tmp-{}-", "n".repeat(255), std::process::id());
        assert!(
            error.contains("; its temporary ")
                && error.contains(&temporary)
                && error.contains(" was not removed: "),
            "{error}"
        );
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_failed_replacement_that_leaves_nothing_reports_its_own_failure_alone() {
        let directory = tempfile::tempdir().unwrap();
        let blocked = directory.path().join("blocked");
        fs::create_dir(&blocked).unwrap();
        let error = atomic_write(&blocked, b"bytes").unwrap_err();
        assert!(error.downcast_ref::<std::io::Error>().is_some(), "{error}");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
    }
}

#[cfg(test)]
mod private_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    #[test]
    fn private_replacement_restricts_an_existing_file_and_failed_rename_cleans_up() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("record");
        fs::write(&path, b"old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        atomic_write_private(&path, b"new").unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let blocked = directory.path().join("blocked");
        fs::create_dir(&blocked).unwrap();
        assert!(atomic_write_private(&blocked, b"cannot rename over directory").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
    }
}
