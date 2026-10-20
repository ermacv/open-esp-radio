//! Shared identities, errors, run control and working memory, and the
//! ISA-neutral function decoding and lifting contracts. No filesystem access.
//!
//! Executables are identified by the SHA-256 of their content. Physical
//! identities retain archive ordinals and ELF table section/index pairs; names
//! are lossless metadata, never identity keys.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fmt, str::FromStr};

mod navigation;
pub use navigation::*;
mod function;
pub use function::*;
mod resources;
pub use resources::*;
mod memory;
pub use memory::*;
mod ledger;
pub use ledger::*;
mod tables;
pub use tables::*;
mod resolutions;
pub use resolutions::*;
mod calls;
pub use calls::*;
mod semantics;
pub use semantics::*;
mod record_memory;
pub use record_memory::*;
mod image;
pub use image::*;
mod source;
pub mod symbol_name;
pub use source::*;

/// Codes shared by API errors and JSON diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorCode {
    InvalidRequest,
    NotFound,
    AlreadyExists,
    Busy,
    Incompatible,
    Integrity,
    SourceChanged,
    DigestMismatch,
    Io,
    Storage,
    DiskFull,
    WorkerExited,
    WorkerProtocol,
    DiagnosticChannel,
    Unavailable,
    Cancelled,
    TimedOut,
    ResourceLimited,
    RecoveryRequired,
    LinkBlocked,
    LinkFailed,
    NeedsExtent,
    Conflict,
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error, Serialize, Deserialize)]
#[error("{message}")]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<MemoryFailure>,
}

impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            memory: None,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

macro_rules! identity {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub fn allocated_bytes(&self) -> u64 {
                self.0.capacity() as u64
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl TryFrom<String> for $name {
            type Error = Error;
            fn try_from(value: String) -> Result<Self> {
                if value.len() != 64
                    || !value
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                {
                    return Err(Error::new(
                        ErrorCode::InvalidRequest,
                        concat!(
                            stringify!($name),
                            " requires 64 lowercase hexadecimal digits"
                        ),
                    ));
                }
                Ok(Self(value))
            }
        }
        impl FromStr for $name {
            type Err = Error;
            fn from_str(value: &str) -> Result<Self> {
                Self::try_from(value.to_owned())
            }
        }
        impl From<$name> for String {
            fn from(value: $name) -> String {
                value.0
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

identity!(ArtifactId);

impl ArtifactId {
    pub fn of_bytes_controlled(bytes: &[u8], control: &mut dyn RunControl) -> Result<Self> {
        use sha2::Digest;
        let mut digest = sha2::Sha256::new();
        for chunk in bytes.chunks(WORK_BLOCK) {
            control.checkpoint(1)?;
            digest.update(chunk);
        }
        format!("{:x}", digest.finalize()).parse()
    }
    pub fn of_bytes(bytes: &[u8]) -> Self {
        Self(format!("{:x}", Sha256::digest(bytes)))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize, Hash)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ObjectLocation {
    Standalone,
    ArchiveMember { ordinal: u64 },
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize, Hash)]
#[serde(deny_unknown_fields)]
pub struct ObjectId {
    pub artifact: ArtifactId,
    pub location: ObjectLocation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum SymbolTableKind {
    Static,
    Dynamic,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize, Hash)]
#[serde(deny_unknown_fields)]
pub struct SymbolId {
    pub object: ObjectId,
    pub table: SymbolTableKind,
    pub table_section: u32,
    pub index: u64,
}
