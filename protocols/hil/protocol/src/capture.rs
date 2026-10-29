//! Generic, frozen capture pages. Schemas and their interpretation belong to
//! the image's observation dictionaries, never to this transport.
use crate::Key;
use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

pub const RECORD_BYTES: usize = 128;
pub const RECORDS_PER_PAGE: usize = 2;
pub const COUNTERS_PER_PAGE: usize = 16;
pub type EncodedRecord = heapless::Vec<u8, RECORD_BYTES>;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Identity {
    pub capture: u32,
    pub epoch: u32,
}

/// Capture refusals never change the operation being observed.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Error {
    InvalidState,
    IdentityMismatch,
    InvalidRange,
    RecordsFull,
    CounterKindsFull,
    CounterOverflow,
    Encoding,
    Borrowed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Description {
    pub identity: Identity,
    /// Hash of the record's path and full postcard schema.
    pub schema: Key,
    pub frozen: bool,
    pub records: u32,
    pub counters: u16,
    pub dropped: u32,
    pub failure: Option<Error>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Describe;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct PageRequest {
    pub identity: Identity,
    pub first: u32,
    pub count: u8,
}

impl PageRequest {
    pub fn range(self, total: usize, limit: usize) -> Result<core::ops::Range<usize>, Error> {
        let first = usize::try_from(self.first).map_err(|_| Error::InvalidRange)?;
        let count = usize::from(self.count);
        let end = first.checked_add(count).ok_or(Error::InvalidRange)?;
        if count == 0 || count > limit || end > total {
            return Err(Error::InvalidRange);
        }
        Ok(first..end)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ReadRecords(pub PageRequest);
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ReadCounters(pub PageRequest);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Records {
    pub identity: Identity,
    pub first: u32,
    pub total: u32,
    pub records: heapless::Vec<EncodedRecord, RECORDS_PER_PAGE>,
}

/// Counts are independent of history filtering and history capacity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Counter {
    pub kind: u16,
    pub value: u32,
    pub saturated: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Counters {
    pub identity: Identity,
    pub first: u32,
    pub total: u16,
    pub counters: heapless::Vec<Counter, COUNTERS_PER_PAGE>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Refused(pub Error);

/// Schema shared by image collectors and the host. Dictionary event kinds
/// keep their existing two-word encoding; timestamps are boot-local µs.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct EventRecord {
    pub kind: u16,
    pub at_micros: u64,
    pub words: [u32; 2],
}
pub const EVENT_SCHEMA: Key = Key::of::<EventRecord>("trace/event");

crate::messages! {
    endpoint Describe = "capture/describe" => Description;
    topic Description = "capture/description";
    endpoint ReadRecords = "capture/records/read" => Records;
    topic Records = "capture/records";
    endpoint ReadCounters = "capture/counters/read" => Counters;
    topic Counters = "capture/counters";
    topic Refused = "capture/refused";
}
