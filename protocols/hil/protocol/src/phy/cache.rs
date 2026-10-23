//! Bounded, chip-neutral exchange of opaque calibration records.
//!
//! The format owner defines identity bytes and record encoding. A decoded
//! candidate still needs native PHY validation before any replay side effect.
use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

/// Maximum bytes in one transfer, independent of a chip's record size.
pub const CALIBRATION_CACHE_CAPACITY: usize = 272;
/// Transfer pages never put a complete record in a request/reply slot.
pub const CALIBRATION_CACHE_PAGE: usize = 32;
/// The deadline starts with the request; pages and status polls cannot extend it.
pub const CALIBRATION_CACHE_WAIT_MS: u16 = 500;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CalibrationCacheFormat(pub u32);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CalibrationCacheTarget {
    /// Identity bytes defined by `format`, derived by the target, never the host.
    pub identity: heapless::Vec<u8, 32>,
    pub format: CalibrationCacheFormat,
    pub schema: u16,
    pub algorithm_revision: u32,
    pub profile: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GetCalibrationCacheIdentity;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CalibrationCacheIdentity {
    pub target: CalibrationCacheTarget,
    pub active_ticket: Option<u32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum CalibrationSource {
    Full,
    HostCache,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StartCalibration {
    pub capture: super::PrepareCapture,
    pub source: CalibrationSource,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CalibrationCacheRequest {
    pub target: CalibrationCacheTarget,
    pub remaining_ms: u16,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CalibrationStarted {
    /// Nonzero, scoped to one attempt within the envelope's boot identity.
    pub ticket: u32,
    pub state: CalibrationStartState,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CalibrationCacheRequested {
    pub ticket: u32,
    pub request: CalibrationCacheRequest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CalibrationCacheMetadata {
    pub format: CalibrationCacheFormat,
    pub schema: u16,
    pub algorithm_revision: u32,
    pub profile: u32,
    pub length: u16,
    /// CRC32C (Castagnoli) over exactly `length` opaque record bytes.
    pub crc32c: u32,
}

impl CalibrationCacheMetadata {
    pub const fn valid_length(self) -> bool {
        self.length > 0 && self.length as usize <= CALIBRATION_CACHE_CAPACITY
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum CalibrationCacheMiss {
    Missing,
    Unreadable,
    InvalidFile,
    Incompatible,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum CalibrationCacheOffer {
    NoCache(CalibrationCacheMiss),
    Snapshot(CalibrationCacheMetadata),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct WriteCalibrationCachePage {
    pub ticket: u32,
    pub first: u16,
    pub bytes: heapless::Vec<u8, CALIBRATION_CACHE_PAGE>,
}

impl WriteCalibrationCachePage {
    /// Admission also requires an open cache window under the current boot.
    /// Reject duplicates, overlaps and gaps without modifying the inbox.
    pub fn range(&self, ticket: u32, next: u16) -> Option<core::ops::Range<usize>> {
        let first = usize::from(self.first);
        let end = first.checked_add(self.bytes.len())?;
        (ticket != 0
            && self.ticket == ticket
            && self.first == next
            && !self.bytes.is_empty()
            && end <= CALIBRATION_CACHE_CAPACITY)
            .then_some(first..end)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CalibrationCachePageWritten {
    pub ticket: u32,
    pub next: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CompleteCalibrationCache {
    pub ticket: u32,
    pub offer: CalibrationCacheOffer,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum CalibrationCacheRejection {
    Format,
    Schema,
    Algorithm,
    Profile,
    Length,
    Checksum,
    Decode,
}

/// Caller admission and native selection are separate evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum CalibrationCacheDecision {
    FullRequested,
    NoCache(CalibrationCacheMiss),
    TimedOut,
    Rejected(CalibrationCacheRejection),
    /// Structural decode succeeded; native validation has not been promised.
    CandidateSupplied,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CalibrationCacheCompleted {
    pub ticket: u32,
    pub decision: CalibrationCacheDecision,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GetCalibrationStartStatus {
    pub ticket: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CalibrationStartStatus {
    pub ticket: u32,
    pub state: CalibrationStartState,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum CalibrationStartState {
    AwaitingCache(CalibrationCacheRequest),
    Registering {
        decision: CalibrationCacheDecision,
    },
    Ready {
        decision: CalibrationCacheDecision,
        capture: super::CaptureReady,
        registration_us: u32,
        /// Only a successful full calibration is eligible for promotion.
        snapshot: Option<CalibrationCacheMetadata>,
    },
    Failed {
        decision: CalibrationCacheDecision,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ReadCalibrationSnapshot {
    pub ticket: u32,
    pub first: u16,
    pub count: u8,
}

impl ReadCalibrationSnapshot {
    /// Bounds an immutable export; its ticket and metadata remain stable.
    pub fn range(self, ticket: u32, total: u16) -> Option<core::ops::Range<usize>> {
        let first = usize::from(self.first);
        let count = usize::from(self.count);
        let end = first.checked_add(count)?;
        (ticket != 0
            && self.ticket == ticket
            && count > 0
            && count <= CALIBRATION_CACHE_PAGE
            && end <= usize::from(total)
            && usize::from(total) <= CALIBRATION_CACHE_CAPACITY)
            .then_some(first..end)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CalibrationSnapshot {
    pub ticket: u32,
    pub first: u16,
    pub metadata: CalibrationCacheMetadata,
    pub bytes: heapless::Vec<u8, CALIBRATION_CACHE_PAGE>,
}

pub fn calibration_cache_crc32c(bytes: &[u8]) -> u32 {
    crc::Crc::<u32>::new(&crc::CRC_32_ISCSI).checksum(bytes)
}

#[cfg(test)]
mod tests;
