//! Caller-owned, single-context observation collector. A collector is not
//! Sync: the image lends it only within its chosen task/executor context.
use super::Reader;
use core::cell::{Cell, RefCell};
use oer_hil_protocol::capture::*;
use oer_trace::{Event, Observer};

pub trait Clock {
    fn micros(&self) -> u64;
}

pub struct Collector<C, const R: usize, const K: usize> {
    clock: C,
    state: RefCell<State<R, K>>,
    borrow_failure: Cell<bool>,
}

struct State<const R: usize, const K: usize> {
    identity: Option<Identity>,
    frozen: bool,
    mask: u64,
    records: heapless::Vec<EncodedRecord, R>,
    counters: heapless::Vec<Counter, K>,
    dropped: u32,
    failure: Option<Error>,
}

impl<C, const R: usize, const K: usize> Collector<C, R, K> {
    pub const fn new(clock: C) -> Self {
        const {
            if R > u32::MAX as usize || K > u16::MAX as usize {
                panic!("capture geometry fits the wire counts");
            }
        }
        Self {
            clock,
            state: RefCell::new(State {
                identity: None,
                frozen: false,
                mask: 0,
                records: heapless::Vec::new(),
                counters: heapless::Vec::new(),
                dropped: 0,
                failure: None,
            }),
            borrow_failure: Cell::new(false),
        }
    }

    /// Identity allocation and operation bracketing belong to the image.
    /// A capture cannot be replaced until its owner explicitly releases it.
    pub fn begin(&mut self, identity: Identity, mask: u64) -> Result<(), Error> {
        let state = self.state.get_mut();
        if state.identity.is_some() || identity.capture == 0 {
            return Err(Error::InvalidState);
        }
        state.identity = Some(identity);
        state.mask = mask;
        self.borrow_failure.set(false);
        Ok(())
    }

    /// Live aggregate query; history filtering and capacity never change it.
    pub fn counter(&self, kind: u16) -> Result<Counter, Error> {
        let state = self.state.try_borrow().map_err(|_| Error::Borrowed)?;
        Ok(state
            .counters
            .iter()
            .find(|counter| counter.kind == kind)
            .copied()
            .unwrap_or(Counter {
                kind,
                value: 0,
                saturated: false,
            }))
    }
    pub fn mark_failure(&mut self, error: Error) {
        self.state.get_mut().failure.get_or_insert(error);
    }
    /// Join a producer's immutable aggregate cut before freezing this collector.
    pub fn merge_counters(&mut self, counters: &[Counter]) -> Result<(), Error> {
        let state = self.state.get_mut();
        if state.identity.is_none() || state.frozen {
            return Err(Error::InvalidState);
        }
        for incoming in counters {
            if let Some(counter) = state
                .counters
                .iter_mut()
                .find(|counter| counter.kind == incoming.kind)
            {
                let value = counter.value.checked_add(incoming.value);
                counter.value = value.unwrap_or(u32::MAX);
                counter.saturated |= incoming.saturated || value.is_none();
                if counter.saturated {
                    state.failure.get_or_insert(Error::CounterOverflow);
                }
            } else if state.counters.push(*incoming).is_err() {
                state.failure.get_or_insert(Error::CounterKindsFull);
                return Err(Error::CounterKindsFull);
            } else if incoming.saturated {
                state.failure.get_or_insert(Error::CounterOverflow);
            }
        }
        Ok(())
    }

    pub fn freeze(&mut self) -> Result<Description, Error> {
        let state = self.state.get_mut();
        if state.identity.is_none() || state.frozen {
            return Err(Error::InvalidState);
        }
        state.frozen = true;
        self.describe()
    }

    pub fn release(&mut self, identity: Identity) -> Result<(), Error> {
        let state = self.state.get_mut();
        state.validate(identity)?;
        state.identity = None;
        state.frozen = false;
        state.records.clear();
        state.counters.clear();
        state.dropped = 0;
        state.failure = None;
        Ok(())
    }
}

impl<C: Clock, const R: usize, const K: usize> Collector<C, R, K> {
    /// Image-local admission reports a refused observation as a typed value.
    /// Owners call through Observer, which latches this failure and returns ().
    pub fn record<E: Event + Copy + 'static>(&self, event: E) -> Result<(), Error> {
        let Ok(mut state) = self.state.try_borrow_mut() else {
            self.borrow_failure.set(true);
            return Err(Error::Borrowed);
        };
        if state.identity.is_none() || state.frozen {
            return Err(Error::InvalidState);
        }
        let counted = state.count(E::KIND.raw());
        if let Err(error) = counted {
            state.failure.get_or_insert(error);
        }
        // History filtering never disables aggregate counts.
        if state.mask & E::CHANNEL.mask() == 0 {
            return counted;
        }
        let record = EventRecord {
            kind: E::KIND.raw(),
            at_micros: self.clock.micros(),
            words: event.encode(),
        };
        let mut bytes = [0; RECORD_BYTES];
        let result = postcard::to_slice(&record, &mut bytes)
            .map_err(|_| Error::Encoding)
            .and_then(|bytes| EncodedRecord::from_slice(bytes).map_err(|_| Error::Encoding))
            .and_then(|record| state.records.push(record).map_err(|_| Error::RecordsFull));
        if let Err(error) = result {
            state.failure.get_or_insert(error);
            match state.dropped.checked_add(1) {
                Some(next) => state.dropped = next,
                None => {
                    state.failure.get_or_insert(Error::CounterOverflow);
                }
            }
        }
        counted.and(result)
    }
}

impl<C: Clock, const R: usize, const K: usize> Observer for Collector<C, R, K> {
    fn event<E: Event + Copy + 'static>(&self, event: E) {
        let _ = self.record(event);
    }
}

impl<const R: usize, const K: usize> State<R, K> {
    fn count(&mut self, kind: u16) -> Result<(), Error> {
        if let Some(counter) = self
            .counters
            .iter_mut()
            .find(|counter| counter.kind == kind)
        {
            match counter.value.checked_add(1) {
                Some(next) => counter.value = next,
                None => {
                    counter.saturated = true;
                    return Err(Error::CounterOverflow);
                }
            }
            return Ok(());
        }
        self.counters
            .push(Counter {
                kind,
                value: 1,
                saturated: false,
            })
            .map_err(|_| Error::CounterKindsFull)
    }

    fn validate(&self, identity: Identity) -> Result<(), Error> {
        if self.identity != Some(identity) {
            return Err(Error::IdentityMismatch);
        }
        if !self.frozen {
            return Err(Error::InvalidState);
        }
        Ok(())
    }
}

impl<C, const R: usize, const K: usize> Reader for Collector<C, R, K> {
    fn describe(&self) -> Result<Description, Error> {
        let state = self.state.try_borrow().map_err(|_| Error::Borrowed)?;
        Ok(Description {
            identity: state.identity.ok_or(Error::InvalidState)?,
            schema: EVENT_SCHEMA,
            frozen: state.frozen,
            records: state.records.len() as u32,
            counters: state.counters.len() as u16,
            dropped: state.dropped,
            failure: if self.borrow_failure.get() {
                Some(Error::Borrowed)
            } else {
                state.failure
            },
        })
    }

    fn records(&self, request: PageRequest) -> Result<Records, Error> {
        let state = self.state.try_borrow().map_err(|_| Error::Borrowed)?;
        state.validate(request.identity)?;
        let range = request.range(state.records.len(), RECORDS_PER_PAGE)?;
        let mut records = heapless::Vec::new();
        for record in state.records.get(range).ok_or(Error::InvalidRange)? {
            records
                .push(record.clone())
                .map_err(|_| Error::InvalidRange)?;
        }
        Ok(Records {
            identity: request.identity,
            first: request.first,
            total: state.records.len() as u32,
            records,
        })
    }

    fn counters(&self, request: PageRequest) -> Result<Counters, Error> {
        let state = self.state.try_borrow().map_err(|_| Error::Borrowed)?;
        state.validate(request.identity)?;
        let range = request.range(state.counters.len(), COUNTERS_PER_PAGE)?;
        let counters =
            heapless::Vec::from_slice(state.counters.get(range).ok_or(Error::InvalidRange)?)
                .map_err(|_| Error::InvalidRange)?;
        Ok(Counters {
            identity: request.identity,
            first: request.first,
            total: state.counters.len() as u16,
            counters,
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::capture::tests::{ID, Step, Virtual};
    #[test]
    fn saturation_and_nested_observation_are_reported_without_panicking() {
        let mut collector = Collector::<_, 2, 1>::new(Virtual);
        collector.begin(ID, u64::MAX).unwrap();
        collector.event(Step);
        collector
            .state
            .get_mut()
            .counters
            .first_mut()
            .unwrap()
            .value = u32::MAX;
        collector.event(Step);
        assert!(
            collector
                .counters(crate::capture::tests::request(0, 1))
                .is_err()
        );
        let description = collector.freeze().unwrap();
        assert_eq!(description.failure, Some(Error::CounterOverflow));
        let counter = collector
            .counters(crate::capture::tests::request(0, 1))
            .unwrap()
            .counters
            .pop()
            .unwrap();
        assert_eq!((counter.value, counter.saturated), (u32::MAX, true));
        collector.release(ID).unwrap();
        collector.begin(ID, u64::MAX).unwrap();
        let borrow = collector.state.borrow_mut();
        collector.event(Step);
        drop(borrow);
        assert_eq!(collector.freeze().unwrap().failure, Some(Error::Borrowed));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod merge_tests {
    use super::*;
    #[test]
    fn immutable_aggregates_merge_by_kind_and_keep_saturation_and_capacity_failures() {
        let mut collector = Collector::<_, 0, 1>::new(());
        let identity = Identity {
            capture: 1,
            epoch: 0,
        };
        collector.begin(identity, 0).unwrap();
        collector
            .merge_counters(&[Counter {
                kind: 1,
                value: u32::MAX,
                saturated: false,
            }])
            .unwrap();
        collector
            .merge_counters(&[Counter {
                kind: 1,
                value: 1,
                saturated: false,
            }])
            .unwrap();
        let counter = collector.counter(1).unwrap();
        assert_eq!(counter.value, u32::MAX);
        assert!(counter.saturated);
        assert_eq!(
            collector.merge_counters(&[Counter {
                kind: 2,
                value: 1,
                saturated: false
            }]),
            Err(Error::CounterKindsFull)
        );
        let cut = collector.freeze().unwrap();
        assert_eq!(cut.failure, Some(Error::CounterOverflow));
        assert_eq!(collector.merge_counters(&[]), Err(Error::InvalidState));
    }
}
