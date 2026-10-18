//! Ordered startup artifact transfer; artifact semantics belong to the target.

use core::fmt;
use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

pub const STARTUP_ARTIFACT_CHUNK_MAX_LEN: usize = 160;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartupArtifactChunkError {
    EmptyArtifact,
    EmptyChunk,
    ChunkTooLarge,
    Range,
}

impl fmt::Display for StartupArtifactChunkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyArtifact => formatter.write_str("startup artifact must not be empty"),
            Self::EmptyChunk => formatter.write_str("startup artifact chunk must not be empty"),
            Self::ChunkTooLarge => formatter.write_str("startup artifact chunk is too large"),
            Self::Range => formatter.write_str("startup artifact chunk range is invalid"),
        }
    }
}

/// One ordered fragment of a target-defined, host-owned startup artifact.
///
/// The protocol deliberately assigns no meaning to the artifact bytes. The
/// selected target adapter owns the exact length, validation and conversion
/// into a typed runtime value. Chunks are contiguous and carry a digest of the
/// complete artifact so a receiver never treats a partial transfer as valid.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StartupArtifactChunk {
    total_length: u16,
    offset: u16,
    crc32c: u32,
    bytes: heapless::Vec<u8, STARTUP_ARTIFACT_CHUNK_MAX_LEN>,
}

impl StartupArtifactChunk {
    pub fn try_new(
        total_length: u16,
        offset: u16,
        crc32c: u32,
        bytes: &[u8],
    ) -> Result<Self, StartupArtifactChunkError> {
        let mut chunk = Self {
            total_length,
            offset,
            crc32c,
            bytes: heapless::Vec::new(),
        };
        chunk
            .bytes
            .extend_from_slice(bytes)
            .map_err(|_| StartupArtifactChunkError::ChunkTooLarge)?;
        chunk.validate()?;
        Ok(chunk)
    }

    pub fn validate(&self) -> Result<(), StartupArtifactChunkError> {
        if self.total_length == 0 {
            return Err(StartupArtifactChunkError::EmptyArtifact);
        }
        if self.bytes.is_empty() {
            return Err(StartupArtifactChunkError::EmptyChunk);
        }
        let end = usize::from(self.offset)
            .checked_add(self.bytes.len())
            .ok_or(StartupArtifactChunkError::Range)?;
        if end > usize::from(self.total_length) {
            return Err(StartupArtifactChunkError::Range);
        }
        Ok(())
    }

    pub const fn total_length(&self) -> u16 {
        self.total_length
    }

    pub const fn offset(&self) -> u16 {
        self.offset
    }

    pub const fn crc32c(&self) -> u32 {
        self.crc32c
    }

    pub fn bytes(&self) -> &[u8] {
        self.bytes.as_slice()
    }

    pub fn is_final(&self) -> bool {
        usize::from(self.offset) + self.bytes.len() == usize::from(self.total_length)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum StartupArtifactDisposition {
    /// No retained artifact was supplied; initialization created one.
    Created,
    /// The supplied artifact was accepted and restored.
    Restored,
    /// The supplied artifact was rejected and full initialization replaced it.
    Replaced,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StartupArtifactStatus {
    pub disposition: StartupArtifactDisposition,
    pub total_length: u16,
    pub initialization_elapsed_micros: u64,
}

/// Computes the transfer-integrity digest of one opaque startup artifact.
///
/// This checksum is not an identity or qualification hash. It only prevents
/// a receiver from accepting an incomplete or internally inconsistent UART
/// transfer.
pub fn startup_artifact_crc32c(bytes: &[u8]) -> u32 {
    crate::framing::crc32c(bytes)
}
