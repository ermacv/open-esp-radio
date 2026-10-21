//! Content fingerprints collected at the reads that supply a computation.

use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
};

/// A file reader, optionally recording the SHA-256 of the bytes it returns.
/// Ordinary loaders need no digest; a tracked computation shares one reader
/// with its nested loaders so newly introduced inputs are recorded too.
#[derive(Default)]
pub struct Inputs {
    digests: Option<BTreeMap<PathBuf, String>>,
}

impl Inputs {
    pub fn tracked() -> Self {
        Self {
            digests: Some(BTreeMap::new()),
        }
    }

    pub fn read(&mut self, path: &Path) -> io::Result<Vec<u8>> {
        let bytes = std::fs::read(path)?;
        if let Some(digests) = &mut self.digests {
            let path = std::path::absolute(path)?;
            let digest = crate::sha256_bytes(&bytes);
            if digests
                .get(&path)
                .is_some_and(|previous| previous != &digest)
            {
                return Err(io::Error::other(format!(
                    "analysis input changed while being read: {}",
                    path.display()
                )));
            }
            digests.insert(path, digest);
        }
        Ok(bytes)
    }

    pub fn read_to_string(&mut self, path: &Path) -> io::Result<String> {
        String::from_utf8(self.read(path)?)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    pub fn into_digests(self) -> BTreeMap<PathBuf, String> {
        self.digests.unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_the_bytes_returned_and_refuses_a_changed_repeated_read() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("input");
        std::fs::write(&path, b"first").unwrap();
        let mut inputs = Inputs::tracked();
        assert_eq!(inputs.read(&path).unwrap(), b"first");
        assert_eq!(inputs.read_to_string(&path).unwrap(), "first");
        std::fs::write(&path, b"second").unwrap();
        assert!(inputs.read(&path).is_err());
        assert_eq!(inputs.into_digests()[&path], crate::sha256_bytes(b"first"));
    }
}
