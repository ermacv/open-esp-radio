//! A diagnostic owner's append-only storage. Publication freezes all pages.
use crate::*;
use oer_hil_protocol::base::RejectReason;

type Result<T> = core::result::Result<T, RejectReason>;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    Idle,
    Armed,
    Capturing,
    Frozen,
}

pub struct Store {
    records: heapless::Vec<Record, MAX_RECORDS>,
    state: State,
    next: u32,
    identity: HealthIdentity,
    config: Option<BeginHealth>,
    counts: Counts,
    dropped: u32,
    failed: bool,
    window: Option<HealthWindow>,
    finished: Option<HealthFinished>,
    aggregates: [u32; 4],
    saturated: bool,
}
const _: () = const {
    if core::mem::size_of::<Store>() > PSRAM_BUDGET || MAX_RECORDS > u16::MAX as usize {
        panic!("health storage must fit its budget and protocol counters");
    }
};

impl Default for Store {
    fn default() -> Self {
        Self::new()
    }
}
impl Store {
    pub fn place(storage: &mut core::mem::MaybeUninit<Self>) -> &mut Self {
        oer_memory::init_in_place!(
            storage,
            Self {
                records: heapless::Vec::new(),
                state: State::Idle,
                next: 0,
                identity: HealthIdentity {
                    observation: 0,
                    epoch: 0
                },
                config: None,
                counts: Counts::default(),
                dropped: 0,
                failed: false,
                window: None,
                finished: None,
                aggregates: [0; 4],
                saturated: false,
            }
        )
    }
    pub const fn new() -> Self {
        Self {
            records: heapless::Vec::new(),
            state: State::Idle,
            next: 0,
            identity: HealthIdentity {
                observation: 0,
                epoch: 0,
            },
            config: None,
            counts: Counts {
                counters: 0,
                signals: 0,
                tracking: 0,
                measurements: 0,
            },
            dropped: 0,
            failed: false,
            window: None,
            finished: None,
            aggregates: [0; 4],
            saturated: false,
        }
    }
    pub fn begin(&mut self, config: BeginHealth, boot: u64) -> Result<HealthStarted> {
        if !config.valid() {
            return Err(RejectReason::InvalidConfiguration);
        }
        if self.state != State::Idle {
            return Err(RejectReason::InvalidState);
        }
        self.next = self.next.checked_add(1).ok_or(RejectReason::InvalidState)?;
        self.identity = HealthIdentity {
            observation: self.next,
            epoch: 0,
        };
        self.records.clear();
        self.counts = Counts::default();
        self.dropped = 0;
        self.failed = false;
        self.aggregates = [0; 4];
        self.saturated = false;
        self.window = None;
        self.finished = None;
        self.config = Some(config);
        self.state = State::Armed;
        Ok(HealthStarted {
            observation: self.next,
            boot,
            capacity: MAX_RECORDS as u16,
            storage_bytes: core::mem::size_of::<Self>() as u32,
        })
    }
    pub fn bind_epoch(&mut self, epoch: u32, channel: u8) -> Result<()> {
        if self.state != State::Armed
            || epoch == 0
            || self.config.map(|c| c.channel) != Some(channel)
        {
            return Err(RejectReason::InvalidState);
        }
        self.identity.epoch = epoch;
        self.state = State::Capturing;
        Ok(())
    }
    pub fn active(&self) -> bool {
        self.state != State::Idle
    }
    pub fn armed(&self) -> bool {
        self.state == State::Armed
    }
    pub fn validate_finish(&self, identity: HealthIdentity) -> Result<()> {
        if !matches!(self.state, State::Capturing | State::Armed) || identity != self.identity {
            Err(RejectReason::InvalidState)
        } else {
            Ok(())
        }
    }
    pub fn config(&self) -> Option<BeginHealth> {
        self.config
    }
    pub fn identity(&self) -> HealthIdentity {
        self.identity
    }
    pub fn validate_window(&self, identity: HealthIdentity) -> Result<()> {
        if self.state != State::Capturing || identity != self.identity || self.window.is_some() {
            Err(RejectReason::InvalidState)
        } else {
            Ok(())
        }
    }
    pub fn window_finished(&mut self, window: HealthWindow) -> Result<()> {
        self.validate_window(window.identity)?;
        self.failed |= !window.rx_cleaned_up
            || window.tracking_failed
            || window.finished_micros < window.started_micros;
        self.window = Some(window);
        Ok(())
    }
    pub fn fail(&mut self) {
        if matches!(self.state, State::Armed | State::Capturing) {
            self.failed = true;
        }
    }
    pub fn push(&mut self, record: Record) {
        if !matches!(self.state, State::Armed | State::Capturing) {
            return;
        }
        let count = match record {
            Record::Counter(_) => &mut self.counts.counters,
            Record::Signal(_) => &mut self.counts.signals,
            Record::Tracking(_) => &mut self.counts.tracking,
            Record::Measurement(_) => &mut self.counts.measurements,
        };
        if let Some(aggregate) = self.aggregates.get_mut(usize::from(record.kind())) {
            match aggregate.checked_add(1) {
                Some(next) => *aggregate = next,
                None => {
                    self.saturated = true;
                    self.failed = true;
                }
            }
        }
        if self.records.push(record).is_err() {
            self.dropped = self.dropped.saturating_add(1);
        } else {
            // Induction: begin resets records and counts together; only a
            // successful append increments one category. Thus each count is
            // at most records.len() <= MAX_RECORDS <= u16::MAX (const above).
            // Before this append it was strictly below that bound.
            *count = count.wrapping_add(1);
        }
    }
    /// Call after the already-started operation and RF cleanup have returned, even on failure.
    pub fn finish(&mut self, identity: HealthIdentity, rf_closed: bool) -> Result<HealthFinished> {
        self.validate_finish(identity)?;
        let rx_cleaned_up = self.window.is_some_and(|w| w.rx_cleaned_up);
        let result = HealthFinished {
            identity,
            counts: self.counts,
            dropped: self.dropped,
            failed: self.failed || self.window.is_none(),
            rx_cleaned_up,
            rf_closed,
        };
        self.finished = Some(result);
        self.state = State::Frozen;
        Ok(result)
    }
    pub fn release(&mut self, identity: HealthIdentity) -> Result<HealthReleased> {
        if self.state != State::Frozen || identity != self.identity {
            return Err(RejectReason::InvalidState);
        }
        self.records.clear();
        self.config = None;
        self.state = State::Idle;
        Ok(HealthReleased(identity))
    }
}

impl oer_hil_agent::capture::Reader for Store {
    fn describe(
        &self,
    ) -> core::result::Result<
        oer_hil_protocol::capture::Description,
        oer_hil_protocol::capture::Error,
    > {
        use oer_hil_protocol::capture::{Description, Error, Identity};
        if self.state == State::Idle {
            return Err(Error::InvalidState);
        }
        Ok(Description {
            identity: Identity {
                capture: self.identity.observation,
                epoch: self.identity.epoch,
            },
            schema: RECORD_SCHEMA,
            frozen: self.state == State::Frozen,
            records: self.records.len() as u32,
            counters: 4,
            dropped: self.dropped,
            failure: if self.saturated {
                Some(Error::CounterOverflow)
            } else if self.dropped != 0 {
                Some(Error::RecordsFull)
            } else {
                None
            },
        })
    }
    fn records(
        &self,
        request: oer_hil_protocol::capture::PageRequest,
    ) -> core::result::Result<oer_hil_protocol::capture::Records, oer_hil_protocol::capture::Error>
    {
        use oer_hil_protocol::capture::{
            EncodedRecord, Error, RECORD_BYTES, RECORDS_PER_PAGE, Records,
        };
        let description = self.describe()?;
        if request.identity != description.identity {
            return Err(Error::IdentityMismatch);
        }
        if !description.frozen {
            return Err(Error::InvalidState);
        }
        let range = request.range(self.records.len(), RECORDS_PER_PAGE)?;
        let mut records = heapless::Vec::new();
        for record in self.records.get(range).ok_or(Error::InvalidRange)? {
            let mut bytes = [0; RECORD_BYTES];
            let bytes = postcard::to_slice(record, &mut bytes).map_err(|_| Error::Encoding)?;
            records
                .push(EncodedRecord::from_slice(bytes).map_err(|_| Error::Encoding)?)
                .map_err(|_| Error::InvalidRange)?;
        }
        Ok(Records {
            identity: description.identity,
            first: request.first,
            total: description.records,
            records,
        })
    }
    fn counters(
        &self,
        request: oer_hil_protocol::capture::PageRequest,
    ) -> core::result::Result<oer_hil_protocol::capture::Counters, oer_hil_protocol::capture::Error>
    {
        use oer_hil_protocol::capture::{COUNTERS_PER_PAGE, Counter, Counters, Error};
        let description = self.describe()?;
        if request.identity != description.identity {
            return Err(Error::IdentityMismatch);
        }
        if !description.frozen {
            return Err(Error::InvalidState);
        }
        let range = request.range(self.aggregates.len(), COUNTERS_PER_PAGE)?;
        let mut counters = heapless::Vec::new();
        for (kind, value) in self
            .aggregates
            .iter()
            .enumerate()
            .skip(range.start)
            .take(range.len())
        {
            counters
                .push(Counter {
                    kind: kind as u16,
                    value: *value,
                    saturated: self.saturated,
                })
                .map_err(|_| Error::InvalidRange)?;
        }
        Ok(Counters {
            identity: description.identity,
            first: request.first,
            total: description.counters,
            counters,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_hil_agent::capture::Reader;
    use oer_hil_protocol::capture::{Identity, PageRequest};
    fn config() -> BeginHealth {
        BeginHealth {
            channel: 1,
            peer: [2; 6],
            peer_rate_kbps: None,
            peer_power_dbm: None,
        }
    }
    fn record() -> Record {
        Record::Tracking(Tracking {
            condition: None,
            operation: TrackingOperation::Tick,
            due: Due::NotDue,
            disposition: Disposition::Skipped,
            at_micros: 1,
            committed: 0,
        })
    }
    #[test]
    fn freeze_bounds_identity_overflow_and_release_are_enforced() {
        let mut store = Store::new();
        assert_eq!(store.begin(config(), 7).expect("arm").observation, 1);
        assert!(store.begin(config(), 7).is_err());
        store.bind_epoch(1, 1).expect("epoch");
        let identity = store.identity();
        let page = PageRequest {
            identity: Identity {
                capture: identity.observation,
                epoch: identity.epoch,
            },
            first: 0,
            count: 1,
        };
        assert!(store.records(page).is_err());
        for _ in 0..MAX_RECORDS + 1 {
            store.push(record());
        }
        store
            .window_finished(HealthWindow {
                identity,
                started_micros: 1,
                finished_micros: 2,
                rx_cleaned_up: true,
                tracking_failed: false,
            })
            .expect("window");
        let result = store.finish(identity, true).expect("freeze");
        assert_eq!(result.dropped, 1);
        assert_eq!(result.counts.total(), Some(MAX_RECORDS));
        assert!(!result.complete());
        let aggregate = store.counters(PageRequest { count: 4, ..page }).unwrap();
        assert_eq!(
            aggregate.counters.get(2).unwrap().value,
            (MAX_RECORDS as u32).saturating_add(1)
        );
        assert_eq!(
            store.describe().unwrap().failure,
            Some(oer_hil_protocol::capture::Error::RecordsFull)
        );
        store.push(record());
        assert_eq!(store.records(page).expect("read").total, MAX_RECORDS as u32);
        assert!(store.records(PageRequest { count: 3, ..page }).is_err());
        assert!(
            store
                .records(PageRequest {
                    first: MAX_RECORDS as u32,
                    ..page
                })
                .is_err()
        );
        store.release(identity).expect("release");
        assert!(store.records(page).is_err());
        assert_eq!(store.begin(config(), 7).expect("next").observation, 2);
        assert!(store.finish(identity, true).is_err());
    }
    #[test]
    fn failed_registration_can_be_frozen_without_inventing_an_epoch_or_cleanup() {
        let mut store = Store::new();
        store.begin(config(), 7).expect("arm");
        store.fail();
        let result = store
            .finish(store.identity(), false)
            .expect("failure evidence");
        assert_eq!(result.identity.epoch, 0);
        assert!(!result.complete());
    }
    #[test]
    fn full_store_keeps_exact_counts_and_preserves_aggregate_saturation() {
        let mut store = Store::new();
        store.begin(config(), 7).unwrap();
        // Tracking fills the exact-count capacity; rejected history continues
        // to contribute to the independent aggregate, including saturation.
        for _ in 0..MAX_RECORDS {
            store.push(record());
        }
        store.aggregates[2] = u32::MAX;
        store.push(record());
        assert_eq!(store.counts.tracking as usize, MAX_RECORDS);
        assert_eq!(store.records.len(), MAX_RECORDS);
        assert_eq!(store.aggregates[2], u32::MAX);
        assert!(store.saturated && store.failed);
        assert_eq!(store.dropped, 1);
    }
}
