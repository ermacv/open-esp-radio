//! An image owns capture storage, its clock, lifecycle and context. The agent
//! serves only bounded encoded records and aggregate counters.
use crate::mailbox::Reply;
use oer_protocols_hil_protocol::capture::*;

#[cfg(oer_trace)]
pub mod atomic;
pub mod collector;

/// No radio or platform dependency is needed to serve an image's capture.
/// Reads must observe one immutable cut; live captures refuse page reads.
pub trait Reader {
    fn describe(&self) -> Result<Description, Error>;
    fn records(&self, request: PageRequest) -> Result<Records, Error>;
    fn counters(&self, request: PageRequest) -> Result<Counters, Error>;
}

crate::requests! {
    /// Mounted by an image together with its capture reader.
    pub enum Request(sessions = false) {
        Describe(Describe), Records(ReadRecords), Counters(ReadCounters),
    }
    publishes [Refused]
}

pub enum Answer {
    Description(Description),
    Records(Records),
    Counters(Counters),
    Refused(Refused),
}

impl Request {
    pub fn serve(self, reader: &impl Reader) -> Answer {
        let result = match self {
            Self::Describe(_) => reader.describe().map(Answer::Description),
            Self::Records(ReadRecords(request)) => reader.records(request).map(Answer::Records),
            Self::Counters(ReadCounters(request)) => reader.counters(request).map(Answer::Counters),
        };
        result.unwrap_or_else(|error| Answer::Refused(Refused(error)))
    }
}

impl Answer {
    /// Answers the request through its `reply`; observation points never
    /// wait for IO.
    pub fn reply<T>(&self, reply: Reply<'_, T>) {
        match self {
            Self::Description(value) => reply.answer(value),
            Self::Records(value) => reply.answer(value),
            Self::Counters(value) => reply.answer(value),
            Self::Refused(value) => reply.answer(value),
        }
    }
}

#[cfg(test)]
mod tests;
