//! Counter-only collector for a producer context separate from the reader.
//! Admission never waits. Freeze refuses an in-flight producer; observations
//! after a successful cut are ignored. The image owns its lifetime and cut.
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use oer_hil_protocol::capture::{Counter, Error};
use oer_trace::{Event, Observer};
struct Slot {
    kind: AtomicU32,
    count: AtomicU32,
    saturated: AtomicBool,
}
impl Slot {
    const fn new() -> Self {
        Self {
            kind: AtomicU32::new(0),
            count: AtomicU32::new(0),
            saturated: AtomicBool::new(false),
        }
    }
}
pub struct AtomicCollector<const K: usize> {
    state: AtomicU8,
    slots: [Slot; K],
    failed: AtomicU8,
}
impl<const K: usize> Default for AtomicCollector<K> {
    fn default() -> Self {
        Self::new()
    }
}
impl<const K: usize> AtomicCollector<K> {
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(0),
            slots: [const { Slot::new() }; K],
            failed: AtomicU8::new(0),
        }
    }
    pub fn failure(&self) -> Option<Error> {
        match self.failed.load(Ordering::Acquire) {
            0 => None,
            1 => Some(Error::Borrowed),
            _ => Some(Error::CounterKindsFull),
        }
    }
    pub fn counter(&self, kind: u16) -> Counter {
        self.slots
            .iter()
            .find(|slot| slot.kind.load(Ordering::Acquire) == u32::from(kind).saturating_add(1))
            .map_or(
                Counter {
                    kind,
                    value: 0,
                    saturated: false,
                },
                |slot| Counter {
                    kind,
                    value: slot.count.load(Ordering::Acquire),
                    saturated: slot.saturated.load(Ordering::Acquire),
                },
            )
    }
    pub fn freeze(&self) -> Result<heapless::Vec<Counter, K>, Error> {
        self.state
            .compare_exchange(0, 2, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| Error::Borrowed)?;
        let mut counters = heapless::Vec::new();
        for slot in &self.slots {
            let kind = slot.kind.load(Ordering::Acquire);
            if kind != 0 {
                counters
                    .push(self.counter(kind.saturating_sub(1) as u16))
                    .map_err(|_| Error::CounterKindsFull)?;
            }
        }
        Ok(counters)
    }
}
impl<const K: usize> Observer for AtomicCollector<K> {
    fn event<E: Event + Copy + 'static>(&self, _: E) {
        match self
            .state
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => {}
            Err(2) => return,
            Err(_) => {
                self.failed.store(1, Ordering::Release);
                return;
            }
        }
        let kind = u32::from(E::KIND.raw()).saturating_add(1);
        if let Some(slot) = self.slots.iter().find(|slot| matches!(slot.kind.load(Ordering::Relaxed), value if value == kind || value == 0)) {
            slot.kind.store(kind, Ordering::Release);
            let count = slot.count.load(Ordering::Relaxed);
            match count.checked_add(1) {
                Some(next) => slot.count.store(next, Ordering::Release),
                None => slot.saturated.store(true, Ordering::Release),
            }
        } else { self.failed.store(2, Ordering::Release); }
        self.state.store(0, Ordering::Release);
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use oer_trace::{Channel, Domain, Kind};
    #[derive(Clone, Copy)]
    struct Seen;
    impl Event for Seen {
        const KIND: Kind = Kind::new(Domain::Platform, 99);
        const CHANNEL: Channel = match Channel::new(Domain::Platform, 0) {
            Ok(channel) => channel,
            Err(_) => panic!("constant channel"),
        };
        fn encode(&self) -> [u32; 2] {
            [0, 0]
        }
        fn decode(_: [u32; 2]) -> Option<Self> {
            Some(Self)
        }
    }
    #[test]
    fn frozen_counter_cut_is_immutable_and_exhaustion_is_visible() {
        let bank = AtomicCollector::<1>::new();
        bank.event(Seen);
        bank.event(Seen);
        let cut = bank.freeze().unwrap();
        bank.event(Seen);
        assert_eq!(cut[0].value, 2);
        assert_eq!(bank.counter(Seen::KIND.raw()).value, 2);
        let empty = AtomicCollector::<0>::new();
        empty.event(Seen);
        assert!(empty.failure().is_some());
    }
    #[test]
    fn preempted_producer_refuses_freeze_without_waiting_and_saturates() {
        let bank = AtomicCollector::<1>::new();
        bank.event(Seen);
        bank.slots[0].count.store(u32::MAX, Ordering::Relaxed);
        bank.event(Seen);
        assert!(bank.counter(Seen::KIND.raw()).saturated);
        bank.state.store(1, Ordering::Relaxed);
        assert_eq!(bank.freeze(), Err(Error::Borrowed));
        bank.state.store(0, Ordering::Relaxed);
        assert!(bank.freeze().is_ok());
    }
}
