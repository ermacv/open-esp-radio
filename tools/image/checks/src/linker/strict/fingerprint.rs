//! Bind the placement record to the final ELF and every byte of its analysis.

use crate::Result;
use oer_os_durable::{inputs::Inputs, sha256_file};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Fingerprint {
    pub elf_sha256: String,
    pub linker_sha256: String,
    pub inputs: BTreeMap<PathBuf, String>,
}

impl Fingerprint {
    pub(super) fn capture(elf: &Path, linker: &Path, inputs: Inputs) -> Result<Self> {
        Ok(Self {
            elf_sha256: sha256_file(elf)?,
            linker_sha256: sha256_file(linker)?,
            inputs: inputs.into_digests(),
        })
    }

    /// A record from another specification cannot validate this build even
    /// when its own files are unchanged. Missing files invalidate the record.
    pub fn matches(&self, elf: &Path, specification: &Path, linker: &Path) -> Result<bool> {
        if !self
            .inputs
            .contains_key(&std::path::absolute(specification)?)
        {
            return Ok(false);
        }
        for (path, expected) in std::iter::once((elf, &self.elf_sha256))
            .chain(std::iter::once((linker, &self.linker_sha256)))
            .chain(
                self.inputs
                    .iter()
                    .map(|(path, digest)| (path.as_path(), digest)),
            )
        {
            match sha256_file(path) {
                Ok(digest) if &digest == expected => {}
                Ok(_) => return Ok(false),
                Err(error)
                    if error
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
                {
                    return Ok(false);
                }
                Err(error) => return Err(error),
            }
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_bytes_match_and_each_changed_or_missing_file_invalidates() {
        let directory = tempfile::tempdir().unwrap();
        let paths: Vec<_> = [
            "final.elf",
            "linker",
            "specification",
            "chip",
            "policy",
            "base-policy",
            "summaries",
            "rom",
            "pins",
        ]
        .into_iter()
        .map(|name| directory.path().join(name))
        .collect();
        for path in &paths {
            std::fs::write(path, b"original").unwrap();
        }
        let mut inputs = Inputs::tracked();
        for path in &paths[2..] {
            inputs.read(path).unwrap();
        }
        let record = Fingerprint::capture(&paths[0], &paths[1], inputs).unwrap();
        let matches = || record.matches(&paths[0], &paths[2], &paths[1]).unwrap();
        assert!(matches());
        // mtime alone does not invalidate content-based reuse.
        std::fs::write(&paths[3], b"original").unwrap();
        assert!(matches());
        for path in &paths {
            std::fs::write(path, b"changed").unwrap();
            assert!(!matches(), "{}", path.display());
            std::fs::remove_file(path).unwrap();
            assert!(!matches(), "{}", path.display());
            std::fs::write(path, b"original").unwrap();
            assert!(matches());
        }
        let other_spec = directory.path().join("other-specification");
        std::fs::write(&other_spec, b"original").unwrap();
        assert!(!record.matches(&paths[0], &other_spec, &paths[1]).unwrap());
        let text = toml::to_string(&record).unwrap();
        let record: Fingerprint = toml::from_str(&text).unwrap();
        assert!(record.matches(&paths[0], &paths[2], &paths[1]).unwrap());
    }
}
