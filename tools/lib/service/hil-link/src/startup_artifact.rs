//! Host-owned persistence for one opaque HIL startup artifact.

use std::{
    fs::{self},
    io::ErrorKind,
    path::Path,
};

use oer_hil_protocol::{
    phy::STARTUP_ARTIFACT_CHUNK_MAX_LEN, phy::StartupArtifactChunk, phy::startup_artifact_crc32c,
};

use crate::Result;

pub fn load_if_present(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) if bytes.is_empty() => {
            Err(format!("startup artifact `{}` is empty", path.display()).into())
        }
        Ok(bytes) if bytes.len() > usize::from(u16::MAX) => Err(format!(
            "startup artifact `{}` exceeds the protocol length limit",
            path.display()
        )
        .into()),
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => {
            Err(format!("cannot read startup artifact `{}`: {error}", path.display()).into())
        }
    }
}

pub fn chunks(bytes: &[u8]) -> Result<Vec<StartupArtifactChunk>> {
    let total_length = u16::try_from(bytes.len())
        .map_err(|_| "startup artifact exceeds the protocol length limit")?;
    if total_length == 0 {
        return Err("startup artifact must not be empty".into());
    }
    let checksum = startup_artifact_crc32c(bytes);
    bytes
        .chunks(STARTUP_ARTIFACT_CHUNK_MAX_LEN)
        .enumerate()
        .map(|(index, part)| {
            let offset = u16::try_from(index * STARTUP_ARTIFACT_CHUNK_MAX_LEN)
                .map_err(|_| "startup artifact chunk offset overflow")?;
            StartupArtifactChunk::try_new(total_length, offset, checksum, part)
                .map_err(|error| format!("cannot frame startup artifact: {error}").into())
        })
        .collect()
}

pub struct Assembler {
    bytes: Vec<u8>,
    total_length: Option<usize>,
    expected_crc32c: u32,
}

impl Default for Assembler {
    fn default() -> Self {
        Self::new()
    }
}

impl Assembler {
    pub const fn new() -> Self {
        Self {
            bytes: Vec::new(),
            total_length: None,
            expected_crc32c: 0,
        }
    }

    pub fn push(&mut self, chunk: &StartupArtifactChunk) -> Result<Option<Vec<u8>>> {
        chunk
            .validate()
            .map_err(|error| format!("target returned an invalid startup artifact: {error}"))?;
        if chunk.offset() == 0 {
            self.bytes.clear();
            self.bytes.reserve_exact(usize::from(chunk.total_length()));
            self.total_length = Some(usize::from(chunk.total_length()));
            self.expected_crc32c = chunk.crc32c();
        }
        if self.total_length != Some(usize::from(chunk.total_length()))
            || self.expected_crc32c != chunk.crc32c()
            || self.bytes.len() != usize::from(chunk.offset())
        {
            return Err("target returned non-contiguous startup artifact chunks".into());
        }
        self.bytes.extend_from_slice(chunk.bytes());
        if !chunk.is_final() {
            return Ok(None);
        }
        if self.bytes.len() != self.total_length.unwrap_or_default()
            || startup_artifact_crc32c(&self.bytes) != self.expected_crc32c
        {
            return Err("target startup artifact checksum mismatch".into());
        }
        Ok(Some(std::mem::take(&mut self.bytes)))
    }
}

#[cfg(test)]
mod tests;
