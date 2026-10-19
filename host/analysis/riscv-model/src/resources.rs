//! Cooperative resource ports. These contain no platform clock or process policy.
use crate::*;

pub const DEFAULT_WORK_UNITS: u64 = 1_000_000_000;
pub const WORK_BLOCK: usize = 4096;
/// Wall-clock deadline of one operation.
pub const DEFAULT_TIMEOUT_MS: u64 = 900_000;
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunPhase {
    PrepareObject,
    PrepareSection,
    Execute,
    Compare,
    Materialize,
    Link,
    ValidateImage,
    AnalyzeFunction,
    AnalyzeValues,
    #[default]
    Starting,
    ReadCaptured,
    Members,
    Elf,
}

/// Physical position, not arbitrary input text. Ordinals are zero based.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunPosition {
    pub phase: RunPhase,
    pub input: Option<u64>,
    pub member: Option<u64>,
    pub table: Option<u64>,
    pub entry: Option<u64>,
    pub artifact: Option<[u8; 32]>,
}
impl RunPosition {
    pub fn artifact(&mut self, id: &ArtifactId) {
        let mut digest = [0; 32];
        for (i, byte) in digest.iter_mut().enumerate() {
            // ArtifactId has already validated lowercase hexadecimal encoding.
            *byte = u8::from_str_radix(&id.as_str()[i * 2..i * 2 + 2], 16).unwrap();
        }
        self.artifact = Some(digest);
    }
}

/// All costs are charged before work. A zero charge is still a checkpoint.
/// Closures adapt existing low-level callers; operations supply a metered
/// implementation. Byte work is charged in bounded blocks per operation.
pub trait RunControl {
    fn checkpoint(&mut self, units: u64) -> Result<()>;
    fn position(&self) -> RunPosition {
        RunPosition::default()
    }
    fn set_position(&mut self, _position: RunPosition) {}
    fn phase(&mut self, phase: RunPhase) -> Result<()> {
        let mut position = self.position();
        position.phase = phase;
        self.set_position(position);
        self.checkpoint(0)
    }
    fn bytes(&mut self, bytes: usize) -> Result<()> {
        self.checkpoint(bytes.div_ceil(WORK_BLOCK) as u64)
    }
}
impl<F: FnMut() -> Result<()>> RunControl for F {
    fn checkpoint(&mut self, _: u64) -> Result<()> {
        self()
    }
}

/// Bound human prose independently of lossless artifact metadata.
pub fn truncate_message(message: &mut String) {
    if message.len() > 1024 {
        let mut end = 1024;
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
        message.push_str(" [truncated]");
    }
}

/// Preserve typed errors transported through std I/O adapters.
pub fn storage_io(error: std::io::Error) -> Error {
    if let Some(typed) = error.get_ref().and_then(|e| e.downcast_ref::<Error>()) {
        return typed.clone();
    }
    let code = match error.kind() {
        std::io::ErrorKind::StorageFull | std::io::ErrorKind::QuotaExceeded => ErrorCode::DiskFull,
        _ => ErrorCode::Io,
    };
    Error::new(code, error.to_string())
}
