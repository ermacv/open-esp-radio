//! How a bundle keeps an archived file, and its one reader.
//!
//! The runtime ELF, large and read only to symbolize, lies
//! deflate-compressed at `<path>.deflate`; what a run records of it (path,
//! size, digest) is the uncompressed file's. Every other archived file lies
//! at its path as it is.

use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, Read as _},
    path::{Path, PathBuf},
    sync::Mutex,
};

use sha2::{Digest as _, Sha256};

use crate::Result;

/// The archived files kept deflate-compressed.
pub const COMPRESSED: [&str; 1] = ["runtime.elf"];

/// The extension of a compressed archived file and its object.
pub const DEFLATE: &str = "deflate";

/// Whether the archived file `path` is kept compressed.
pub fn compressed(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| COMPRESSED.contains(&name))
}

/// Where the bytes of the archived file `path` lie: `path` itself, or
/// `<path>.deflate` for a [`COMPRESSED`] file.
pub fn stored_path(path: &Path) -> PathBuf {
    if compressed(path) {
        let mut stored = path.as_os_str().to_owned();
        stored.push(".");
        stored.push(DEFLATE);
        PathBuf::from(stored)
    } else {
        path.to_owned()
    }
}

/// The bytes of the archived file `path`, decompressed when it is kept
/// compressed.
pub fn read_archived(path: &Path) -> Result<Vec<u8>> {
    let stored = stored_path(path);
    let file = File::open(&stored).map_err(|error| format!("{}: {error}", stored.display()))?;
    let mut bytes = Vec::new();
    if compressed(path) {
        flate2::read::DeflateDecoder::new(io::BufReader::new(file))
            .read_to_end(&mut bytes)
            .map_err(|error| format!("{}: {error}", stored.display()))?;
    } else {
        io::BufReader::new(file).read_to_end(&mut bytes)?;
    }
    Ok(bytes)
}

/// The size and SHA-256 of an archived file's bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Identity {
    pub size_bytes: u64,
    pub sha256: String,
}

/// The [`Identity`] of the archived file `path`, decompressed when it is
/// kept compressed.
pub fn archived_identity(path: &Path) -> Result<Identity> {
    stored_identity(&stored_path(path), compressed(path))
}

/// The [`Identity`] of the bytes `stored` holds, deflate-compressed when
/// `compressed`.
pub fn stored_identity(stored: &Path, compressed: bool) -> Result<Identity> {
    if !compressed {
        return Ok(Identity {
            size_bytes: fs::metadata(stored)?.len(),
            sha256: oer_os_durable::sha256_file(stored)?,
        });
    }
    deflated_identity(io::BufReader::new(File::open(stored)?), stored)
}

/// The [`Identity`] of the bytes the deflate stream `compressed` (read from
/// `stored`) holds.
fn deflated_identity(compressed: impl io::Read, stored: &Path) -> Result<Identity> {
    struct Hashing(Sha256, u64);
    impl io::Write for Hashing {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.update(bytes);
            self.1 += bytes.len() as u64;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut hashing = Hashing(Sha256::new(), 0);
    io::copy(
        &mut flate2::read::DeflateDecoder::new(compressed),
        &mut hashing,
    )
    .map_err(|error| format!("{}: {error}", stored.display()))?;
    Ok(Identity {
        size_bytes: hashing.1,
        sha256: format!("{:x}", hashing.0.finalize()),
    })
}

/// The uncompressed [`Identity`] of a compressed file whose stored bytes
/// hash to `stored_sha256`, decompressing `stored` only the first time this
/// process meets those bytes: a run store links one runtime ELF into many
/// runs.
pub fn decompressed_identity(stored: &Path, stored_sha256: &str) -> Result<Identity> {
    static KNOWN: Mutex<BTreeMap<String, Identity>> = Mutex::new(BTreeMap::new());
    let known = |known: &BTreeMap<String, Identity>| known.get(stored_sha256).cloned();
    if let Some(identity) = known(&KNOWN.lock().unwrap_or_else(|error| error.into_inner())) {
        return Ok(identity);
    }
    // The stored bytes are read once: what is hashed is what is decompressed.
    let bytes = fs::read(stored)?;
    if oer_os_durable::sha256_bytes(&bytes) != stored_sha256 {
        return Err(format!("{} changed while it was read", stored.display()).into());
    }
    let identity = deflated_identity(&bytes[..], stored)?;
    KNOWN
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(stored_sha256.to_owned(), identity.clone());
    Ok(identity)
}
