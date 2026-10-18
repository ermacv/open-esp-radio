//! Function extraction from a static archive of RISC-V relocatable objects.

use oer_vendor_provenance::fingerprint::{self, Function};
use sha2::{Digest, Sha256};

/// One archive revision with every defined function it contains.
#[derive(Debug)]
pub struct Revision {
    /// Caller-chosen label, such as a commit identifier.
    pub label: String,
    /// SHA-256 of the complete archive bytes.
    pub sha256: String,
    /// Defined functions in archive member order, with their fingerprints.
    pub functions: Vec<Function>,
}

pub type Error = Box<dyn std::error::Error + Send + Sync>;

/// Read every defined sized function of every relocatable member.
pub fn read_archive(label: &str, bytes: &[u8]) -> Result<Revision, Error> {
    if !bytes.starts_with(b"!<arch>\n") {
        return Err(format!("{label}: not a static archive").into());
    }
    Ok(Revision {
        label: label.to_owned(),
        sha256: hex(&Sha256::digest(bytes)),
        functions: fingerprint::functions(bytes)?,
    })
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
