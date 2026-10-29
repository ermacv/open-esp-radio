//! Images expose atomic observation counters through this service. Independent
//! RF experiment readers also serve their own bounded data contracts.
use crate::console::{Console, Immediate};
use oer_hil_protocol::{RequestIdentity, capture::*};

#[cfg(oer_trace)]
pub mod atomic;
pub mod collector;

/// No radio or platform dependency is needed to serve an image's capture.
/// A counter reader loads live atomics; RF data readers enforce their own lifecycle.
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
    /// Pages use console backpressure; observation points never wait for IO.
    pub async fn publish<const N: usize, const M: usize, P: Immediate>(
        &self,
        console: &Console<N, M, P>,
        request: RequestIdentity,
    ) {
        macro_rules! send {
            ($value:expr) => {
                console
                    .publish_reliably(request.session_id, request.request_id, $value)
                    .await
            };
        }
        match self {
            Self::Description(value) => send!(value),
            Self::Records(value) => send!(value),
            Self::Counters(value) => send!(value),
            Self::Refused(value) => send!(value),
        };
    }
}

#[cfg(test)]
mod tests;
