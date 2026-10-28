//! A diagnostic owner's append-only storage. Publication freezes all pages.
use oer_hil_protocol::{base::RejectReason, health::*};

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
}
const _: () = const {
    if core::mem::size_of::<Store>() > PSRAM_BUDGET || MAX_RECORDS > u16::MAX as usize {
        panic!("health storage must fit its budget and protocol counters");
    }
};

/// Refused before appending: the exact category count cannot represent a record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CountOverflow {
    pub kind: CountKind,
    pub count: u16,
    pub stored: usize,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CountKind {
    Counter,
    Signal,
    Tracking,
    Measurement,
}

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
    pub fn push(&mut self, record: Record) -> core::result::Result<(), CountOverflow> {
        if !matches!(self.state, State::Armed | State::Capturing) {
            return Ok(());
        }
        let (kind, count) = match record {
            Record::Counter(_) => (CountKind::Counter, &mut self.counts.counters),
            Record::Signal(_) => (CountKind::Signal, &mut self.counts.signals),
            Record::Tracking(_) => (CountKind::Tracking, &mut self.counts.tracking),
            Record::Measurement(_) => (CountKind::Measurement, &mut self.counts.measurements),
        };
        // A full store has the existing counted-loss policy. Otherwise check
        // the category count before committing either the record or its count.
        let next = if self.records.is_full() {
            *count
        } else {
            count.checked_add(1).ok_or(CountOverflow {
                kind,
                count: *count,
                stored: self.records.len(),
            })?
        };
        if self.records.push(record).is_err() {
            self.dropped = self.dropped.saturating_add(1);
        } else {
            *count = next;
        }
        Ok(())
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
    fn page(
        &self,
        request: HealthPageRequest,
        limit: usize,
        total: u16,
    ) -> Result<impl Iterator<Item = &Record>> {
        if self.state != State::Frozen || request.identity != self.identity {
            return Err(RejectReason::InvalidState);
        }
        request
            .range(usize::from(total), limit)
            .ok_or(RejectReason::InvalidConfiguration)?;
        Ok(self.records.iter())
    }
    pub fn counters(&self, request: HealthPageRequest) -> Result<CounterPage> {
        let values = self.page(request, COUNTER_PAGE, self.counts.counters)?;
        Ok(CounterPage {
            identity: self.identity,
            first: request.first,
            total: self.counts.counters,
            records: values
                .filter_map(|r| {
                    if let Record::Counter(v) = r {
                        Some(*v)
                    } else {
                        None
                    }
                })
                .skip(usize::from(request.first))
                .take(usize::from(request.count))
                .collect(),
        })
    }
    pub fn signals(&self, request: HealthPageRequest) -> Result<SignalPage> {
        let values = self.page(request, SIGNAL_PAGE, self.counts.signals)?;
        Ok(SignalPage {
            identity: self.identity,
            first: request.first,
            total: self.counts.signals,
            records: values
                .filter_map(|r| {
                    if let Record::Signal(v) = r {
                        Some(*v)
                    } else {
                        None
                    }
                })
                .skip(usize::from(request.first))
                .take(usize::from(request.count))
                .collect(),
        })
    }
    pub fn tracking(&self, request: HealthPageRequest) -> Result<TrackingPage> {
        let values = self.page(request, DETAIL_PAGE, self.counts.tracking)?;
        Ok(TrackingPage {
            identity: self.identity,
            first: request.first,
            total: self.counts.tracking,
            records: values
                .filter_map(|r| {
                    if let Record::Tracking(v) = r {
                        Some(*v)
                    } else {
                        None
                    }
                })
                .skip(usize::from(request.first))
                .take(usize::from(request.count))
                .collect(),
        })
    }
    pub fn measurements(&self, request: HealthPageRequest) -> Result<MeasurementPage> {
        let values = self.page(request, DETAIL_PAGE, self.counts.measurements)?;
        Ok(MeasurementPage {
            identity: self.identity,
            first: request.first,
            total: self.counts.measurements,
            records: values
                .filter_map(|r| {
                    if let Record::Measurement(v) = r {
                        Some(*v)
                    } else {
                        None
                    }
                })
                .skip(usize::from(request.first))
                .take(usize::from(request.count))
                .collect(),
        })
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

#[cfg(test)]
mod tests {
    use super::*;
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
        let page = HealthPageRequest {
            identity,
            first: 0,
            count: 1,
        };
        assert!(store.tracking(page).is_err());
        for _ in 0..MAX_RECORDS + 1 {
            store.push(record()).unwrap();
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
        store.push(record()).unwrap();
        assert_eq!(
            store.tracking(page).expect("read").total,
            MAX_RECORDS as u16
        );
        assert!(
            store
                .tracking(HealthPageRequest { count: 3, ..page })
                .is_err()
        );
        assert!(
            store
                .tracking(HealthPageRequest {
                    first: MAX_RECORDS as u16,
                    ..page
                })
                .is_err()
        );
        store.release(identity).expect("release");
        assert!(store.tracking(page).is_err());
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
    fn category_overflow_refuses_before_mutating_storage() {
        let mut store = Store::new();
        store.begin(config(), 7).unwrap();
        store.counts.tracking = u16::MAX;
        assert_eq!(
            store.push(record()),
            Err(CountOverflow {
                kind: CountKind::Tracking,
                count: u16::MAX,
                stored: 0
            })
        );
        assert!(store.records.is_empty());
        assert_eq!(store.counts.tracking, u16::MAX);
    }
}
