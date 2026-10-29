//! Drain and validate an immutable instrumentation cut. Interpretation of
//! records stays with the domain dictionary selected by its schema key.
use crate::{Result, SerialCapture};
use oer_hil_protocol::{Key, capture::*};
use std::time::Duration;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CaptureError {
    Device(Error),
    Identity,
    Schema,
    NotFrozen,
    Page,
    MissingPages,
    DuplicateCounter,
    Encoding,
}
impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "instrumentation capture: {self:?}")
    }
}
impl std::error::Error for CaptureError {}

#[derive(Clone, Debug)]
pub struct FrozenCapture {
    pub description: Description,
    pub records: Vec<EncodedRecord>,
    pub counters: Vec<Counter>,
}

impl FrozenCapture {
    pub fn complete(&self) -> bool {
        self.description.frozen
            && self.records.len() as u64 == u64::from(self.description.records)
            && self.counters.len() == usize::from(self.description.counters)
            && self.description.failure.is_none()
            && self.description.dropped == 0
            && self.counters.iter().all(|counter| !counter.saturated)
    }

    pub fn decode<T: serde::de::DeserializeOwned>(
        &self,
        schema: Key,
    ) -> std::result::Result<Vec<T>, CaptureError> {
        if self.description.schema != schema {
            return Err(CaptureError::Schema);
        }
        self.records
            .iter()
            .map(|record| postcard::from_bytes(record).map_err(|_| CaptureError::Encoding))
            .collect()
    }
}

/// Validates each requested page before accepting any of it. A failed page
/// leaves the already accepted prefix intact and can never complete a cut.
pub struct Pages {
    capture: FrozenCapture,
}
impl Pages {
    pub fn new(
        description: Description,
        identity: Identity,
        schema: Key,
    ) -> std::result::Result<Self, CaptureError> {
        if identity.capture == 0 || description.identity != identity {
            return Err(CaptureError::Identity);
        }
        if description.schema != schema {
            return Err(CaptureError::Schema);
        }
        if !description.frozen {
            return Err(CaptureError::NotFrozen);
        }
        Ok(Self {
            capture: FrozenCapture {
                description,
                records: Vec::new(),
                counters: Vec::new(),
            },
        })
    }
    pub fn next_records(&self) -> Option<PageRequest> {
        self.next(
            self.capture.records.len(),
            self.capture.description.records,
            RECORDS_PER_PAGE,
        )
    }
    pub fn next_counters(&self) -> Option<PageRequest> {
        self.next(
            self.capture.counters.len(),
            u32::from(self.capture.description.counters),
            COUNTERS_PER_PAGE,
        )
    }
    fn next(&self, first: usize, total: u32, limit: usize) -> Option<PageRequest> {
        let first = u32::try_from(first).ok()?;
        let count = total.checked_sub(first)?.min(limit as u32);
        (count != 0).then_some(PageRequest {
            identity: self.capture.description.identity,
            first,
            count: count as u8,
        })
    }
    pub fn records(
        &mut self,
        requested: PageRequest,
        page: Records,
    ) -> std::result::Result<(), CaptureError> {
        if Some(requested) != self.next_records()
            || page.identity != requested.identity
            || page.first != requested.first
            || page.total != self.capture.description.records
            || page.records.len() != usize::from(requested.count)
        {
            return Err(CaptureError::Page);
        }
        self.capture.records.extend(page.records);
        Ok(())
    }
    pub fn counters(
        &mut self,
        requested: PageRequest,
        page: Counters,
    ) -> std::result::Result<(), CaptureError> {
        if Some(requested) != self.next_counters()
            || page.identity != requested.identity
            || page.first != requested.first
            || page.total != self.capture.description.counters
            || page.counters.len() != usize::from(requested.count)
        {
            return Err(CaptureError::Page);
        }
        let mut kinds = std::collections::BTreeSet::new();
        if self
            .capture
            .counters
            .iter()
            .chain(&page.counters)
            .any(|counter| !kinds.insert(counter.kind))
        {
            return Err(CaptureError::DuplicateCounter);
        }
        self.capture.counters.extend(page.counters);
        Ok(())
    }
    pub fn finish(self) -> std::result::Result<FrozenCapture, CaptureError> {
        if self.capture.records.len() as u64 != u64::from(self.capture.description.records)
            || self.capture.counters.len() != usize::from(self.capture.description.counters)
        {
            return Err(CaptureError::MissingPages);
        }
        Ok(self.capture)
    }
}

impl SerialCapture {
    /// Boot is pinned by the exchange; the assembler pins capture, epoch,
    /// schema, counts and page continuity. The owner releases the cut later.
    pub fn read_instrumentation(
        &self,
        boot: u64,
        identity: Identity,
        schema: Key,
        timeout: Duration,
    ) -> Result<FrozenCapture> {
        let description = self.request_boot(boot, 0, Describe, timeout)?;
        let mut pages = Pages::new(description, identity, schema)?;
        while let Some(request) = pages.next_records() {
            pages.records(
                request,
                self.request_boot(boot, 0, ReadRecords(request), timeout)?,
            )?;
        }
        while let Some(request) = pages.next_counters() {
            pages.counters(
                request,
                self.request_boot(boot, 0, ReadCounters(request), timeout)?,
            )?;
        }
        Ok(pages.finish()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn description() -> Description {
        Description {
            identity: Identity {
                capture: 1,
                epoch: 2,
            },
            schema: EVENT_SCHEMA,
            frozen: true,
            records: 3,
            counters: 0,
            dropped: 0,
            failure: None,
        }
    }
    fn page(request: PageRequest) -> Records {
        Records {
            identity: request.identity,
            first: request.first,
            total: 3,
            records: (0..request.count).map(|_| EncodedRecord::new()).collect(),
        }
    }
    #[test]
    fn pages_reject_stale_repeated_skipped_and_partial_responses() {
        let desc = description();
        assert!(matches!(
            Pages::new(
                desc,
                Identity {
                    epoch: 3,
                    ..desc.identity
                },
                desc.schema
            ),
            Err(CaptureError::Identity)
        ));
        assert!(matches!(
            Pages::new(desc, desc.identity, Key([0; 8])),
            Err(CaptureError::Schema)
        ));
        let mut pages = Pages::new(desc, desc.identity, desc.schema).unwrap();
        let first = pages.next_records().unwrap();
        let mut invalid = page(first);
        invalid.first = 1;
        assert_eq!(pages.records(first, invalid), Err(CaptureError::Page));
        pages.records(first, page(first)).unwrap();
        assert_eq!(pages.records(first, page(first)), Err(CaptureError::Page));
        assert!(matches!(pages.finish(), Err(CaptureError::MissingPages)));
    }
    #[test]
    fn history_loss_survives_a_successful_drain() {
        let desc = Description {
            dropped: 1,
            failure: Some(Error::RecordsFull),
            ..description()
        };
        let mut pages = Pages::new(desc, desc.identity, desc.schema).unwrap();
        while let Some(request) = pages.next_records() {
            pages.records(request, page(request)).unwrap();
        }
        let captured = pages.finish().unwrap();
        assert!(!captured.complete());
        assert_eq!(captured.records.len(), 3);
    }
    #[test]
    fn malformed_pages_cannot_replace_a_valid_prefix_or_counter_bank() {
        let desc = Description {
            records: 0,
            counters: 2,
            ..description()
        };
        let mut pages = Pages::new(desc, desc.identity, desc.schema).unwrap();
        let request = pages.next_counters().unwrap();
        let counter = Counter {
            kind: 1,
            value: 9,
            saturated: false,
        };
        let mut response = Counters {
            identity: desc.identity,
            first: 0,
            total: 2,
            counters: [counter, counter].into_iter().collect(),
        };
        assert_eq!(
            pages.counters(request, response.clone()),
            Err(CaptureError::DuplicateCounter)
        );
        response.counters.last_mut().unwrap().kind = 2;
        response.identity.epoch = 3;
        assert_eq!(
            pages.counters(request, response.clone()),
            Err(CaptureError::Page)
        );
        response.identity = desc.identity;
        pages.counters(request, response).unwrap();
        assert!(pages.finish().unwrap().complete());
        assert!(matches!(
            Pages::new(
                Description {
                    frozen: false,
                    ..desc
                },
                desc.identity,
                desc.schema
            ),
            Err(CaptureError::NotFrozen)
        ));
    }
}
