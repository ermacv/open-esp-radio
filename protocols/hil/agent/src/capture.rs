//! An image owns capture storage, its clock, lifecycle and context. The agent
//! serves only bounded encoded records and aggregate counters.
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

crate::__requests! {
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

impl crate::serve::Answer for Answer {
    fn write<'f>(&self, frame: crate::serve::Frame<'f>) -> crate::serve::Written<'f> {
        match self {
            Self::Description(value) => frame.write(value),
            Self::Records(value) => frame.write(value),
            Self::Counters(value) => frame.write(value),
            Self::Refused(value) => frame.write(value),
        }
    }
}

/// The capture's requests are observations of the image's reader: each is
/// answered in the console's step from one frozen cut.
impl<O: Reader> crate::serve::Serve<O> for Request {
    type Command = core::convert::Infallible;
    fn serve<'f>(
        self,
        observer: &mut O,
        frame: crate::serve::Frame<'f>,
    ) -> crate::serve::Served<'f, Self::Command> {
        use crate::serve::Answer as _;
        crate::serve::Served::Observed(Self::serve(self, observer).write(frame))
    }
}

#[cfg(test)]
mod tests;
