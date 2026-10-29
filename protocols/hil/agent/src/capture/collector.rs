//! Image-owned atomic event counters. Observation retains no event payload,
//! timestamp or history, and never borrows mutable state or waits for IO.
use super::Reader;
use core::sync::atomic::{AtomicU32, Ordering::Relaxed};
use oer_hil_protocol::capture::*;
use oer_trace::{Event, Kind, Observer};

/// The image declares the supported kinds, including supported zero counts.
/// Each slot is one saturating atomic counter; no registration or ring exists.
pub struct Collector<const K: usize> {
    kinds: [Kind; K],
    values: [AtomicU32; K],
    unlisted: AtomicU32,
}
impl<const K: usize> Collector<K> {
    pub const fn new(kinds: [Kind; K]) -> Result<Self, Error> {
        const {
            if K > u16::MAX as usize {
                panic!("counter bank fits wire count");
            }
        }
        // A duplicate kind would make the bank ambiguous. Image dictionaries
        // are fixed values, so reject duplicates in their static initializer.
        let mut remaining = kinds.as_slice();
        while let [kind, tail @ ..] = remaining {
            let mut other = tail;
            while let [candidate, rest @ ..] = other {
                if kind.raw() == candidate.raw() {
                    return Err(Error::InvalidState);
                }
                other = rest;
            }
            remaining = tail;
        }
        Ok(Self {
            kinds,
            values: [const { AtomicU32::new(0) }; K],
            unlisted: AtomicU32::new(0),
        })
    }
    /// The image resets a measurement window at its own operation boundary.
    pub fn clear(&self) {
        for value in &self.values {
            value.store(0, Relaxed);
        }
        self.unlisted.store(0, Relaxed);
    }
    /// A single live atomic reading; it carries no event payload or ordering.
    pub fn count<E: Event>(&self) -> u32 {
        self.value(E::KIND)
    }
    pub fn value(&self, kind: Kind) -> u32 {
        self.kinds
            .iter()
            .zip(&self.values)
            .find_map(|(candidate, value)| {
                (candidate.raw() == kind.raw()).then(|| value.load(Relaxed))
            })
            .unwrap_or(0)
    }
    /// Identity is supplied by the image's HIL endpoint, not observation points.
    pub fn reader(&self, identity: Identity) -> CounterReader<'_, K> {
        CounterReader {
            collector: self,
            identity,
        }
    }
}
fn increment(value: &AtomicU32) {
    let _ = value.fetch_update(Relaxed, Relaxed, |current| current.checked_add(1));
}
impl<const K: usize> Observer for Collector<K> {
    #[inline]
    fn event<E: Event + Copy + 'static>(&self, _: E) {
        if let Some(value) = self
            .kinds
            .iter()
            .zip(&self.values)
            .find_map(|(kind, value)| (kind.raw() == E::KIND.raw()).then_some(value))
        {
            increment(value);
        } else {
            increment(&self.unlisted);
        }
    }
}

/// Existing capture transport over a fixed bank of live atomic counts.
/// The generic service also serves independent RF experiment record readers;
/// this reader has zero records and no freeze/clock/history lifecycle.
pub struct CounterReader<'a, const K: usize> {
    collector: &'a Collector<K>,
    identity: Identity,
}
impl<const K: usize> CounterReader<'_, K> {
    fn validate(&self, identity: Identity) -> Result<(), Error> {
        if identity == self.identity {
            Ok(())
        } else {
            Err(Error::IdentityMismatch)
        }
    }
}
impl<const K: usize> Reader for CounterReader<'_, K> {
    fn describe(&self) -> Result<Description, Error> {
        let dropped = self.collector.unlisted.load(Relaxed);
        let overflow = self
            .collector
            .values
            .iter()
            .any(|value| value.load(Relaxed) == u32::MAX);
        Ok(Description {
            identity: self.identity,
            schema: oer_hil_protocol::Key::of::<Counter>("trace/counter"),
            frozen: true,
            records: 0,
            counters: K as u16,
            dropped,
            failure: if overflow {
                Some(Error::CounterOverflow)
            } else if dropped != 0 {
                Some(Error::CounterKindsFull)
            } else {
                None
            },
        })
    }
    fn records(&self, request: PageRequest) -> Result<Records, Error> {
        self.validate(request.identity)?;
        Err(Error::InvalidRange)
    }
    fn counters(&self, request: PageRequest) -> Result<Counters, Error> {
        self.validate(request.identity)?;
        let range = request.range(K, COUNTERS_PER_PAGE)?;
        let mut counters = heapless::Vec::new();
        for (kind, value) in self
            .collector
            .kinds
            .iter()
            .zip(&self.collector.values)
            .skip(range.start)
            .take(range.len())
        {
            let value = value.load(Relaxed);
            counters
                .push(Counter {
                    kind: kind.raw(),
                    value,
                    saturated: value == u32::MAX,
                })
                .map_err(|_| Error::InvalidRange)?;
        }
        Ok(Counters {
            identity: self.identity,
            first: request.first,
            total: K as u16,
            counters,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::tests::{ID, Step, request};
    #[test]
    fn counts_saturate_and_report_unlisted_kinds_without_panicking() {
        let collector = Collector::new([Step::KIND]).unwrap();
        collector
            .values
            .first()
            .unwrap()
            .store(u32::MAX - 1, Relaxed);
        collector.event(Step);
        collector.event(Step);
        assert_eq!(collector.count::<Step>(), u32::MAX);
        let reader = collector.reader(ID);
        assert_eq!(
            reader.describe().unwrap().failure,
            Some(Error::CounterOverflow)
        );
        assert!(reader.counters(request(0, 1)).unwrap().counters[0].saturated);
        collector.clear();
        assert_eq!(reader.describe().unwrap().failure, None);
        let empty = Collector::new([]).unwrap();
        empty.event(Step);
        assert_eq!(empty.reader(ID).describe().unwrap().dropped, 1);
        assert_eq!(
            empty.reader(ID).describe().unwrap().failure,
            Some(Error::CounterKindsFull)
        );
    }
    #[test]
    fn concurrent_observers_share_only_atomics_and_keep_every_count() {
        let collector = Collector::new([Step::KIND]).unwrap();
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    for _ in 0..1000 {
                        collector.event(Step);
                    }
                });
            }
        });
        assert_eq!(collector.count::<Step>(), 4000);
    }
    #[test]
    fn observation_point_evaluates_no_arguments_without_trace_and_counts_once_with_trace() {
        let observer = Collector::new([Step::KIND]).unwrap();
        let evaluated = core::cell::Cell::new(0u32);
        oer_trace::observe!(observer, {
            evaluated.set(evaluated.get() + 1);
            Step
        });
        assert_eq!(evaluated.get(), u32::from(cfg!(oer_trace)));
        assert_eq!(observer.count::<Step>(), u32::from(cfg!(oer_trace)));
    }
}
