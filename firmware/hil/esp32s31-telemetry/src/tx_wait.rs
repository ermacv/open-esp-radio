//! Append-only boot trace. Each writer reserves a unique slot and publishes
//! it with release ordering; readers never see a partially initialized entry.
//! No locks, interrupt masking, allocation or text formatting in the producer.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use oer_esp32s31_ieee80211_runtime::diagnostics::aggregate_tx::TxWaitSample;

macro_rules! record_fields {
    ($($field:ident),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
        pub struct Record { $(pub $field: u32,)+ }
        struct Slot { ready: AtomicBool, $($field: AtomicU32,)+ }
        impl Slot {
            const fn new() -> Self {
                Self { ready: AtomicBool::new(false), $($field: AtomicU32::new(0),)+ }
            }
            fn publish(&self, record: Record) {
                $(self.$field.store(record.$field, Ordering::Relaxed);)+
                self.ready.store(true, Ordering::Release);
            }
            fn read(&self) -> Option<Record> {
                self.ready.load(Ordering::Acquire).then(|| Record {
                    $($field: self.$field.load(Ordering::Relaxed),)+
                })
            }
        }
    };
}

record_fields!(
    deadline_wake,
    at_high,
    at_low,
    elapsed_us,
    lateness_us,
    sequence,
    queue,
    available,
    enabled,
    valid,
    done,
    timeout,
    collision,
    aifsn,
    backoff,
    priority,
    cca_force,
    cca_aux_force,
    mac_active_state,
    rx_hang,
    tx_hang,
    rx_tx_hang,
    rx_tx_panic,
    rx_mpdu_count,
    rx_signal_count,
    rx_end_count,
    rx_fcs_error_count,
    rx_abort_count
);

pub struct Trace {
    // Independent reservation budgets keep common 5 ms waits from hiding
    // rarer long waits later in the same boot. Slots are never overwritten.
    reserved: [AtomicUsize; 4],
    dropped: AtomicU32,
    slots: [Slot; 32],
}

impl Default for Trace {
    fn default() -> Self {
        Self::new()
    }
}

impl Trace {
    const SAMPLES_PER_BAND: usize = 8;

    pub const fn new() -> Self {
        Self {
            reserved: [const { AtomicUsize::new(0) }; 4],
            dropped: AtomicU32::new(0),
            slots: [const { Slot::new() }; 32],
        }
    }

    pub fn record(&self, sample: TxWaitSample) {
        let band = match sample.elapsed_micros {
            0..10_000 => 0,
            10_000..20_000 => 1,
            20_000..40_000 => 2,
            _ => 3,
        };
        let Ok(index) =
            self.reserved[band].fetch_update(Ordering::Relaxed, Ordering::Relaxed, |index| {
                (index < Self::SAMPLES_PER_BAND).then_some(index + 1)
            })
        else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let available = sample.snapshot.is_some();
        let state = sample.snapshot.unwrap_or_default();
        self.slots[band * Self::SAMPLES_PER_BAND + index].publish(Record {
            deadline_wake: sample.deadline_wake.into(),
            at_high: (sample.at_micros >> 32) as u32,
            at_low: sample.at_micros as u32,
            elapsed_us: u32::try_from(sample.elapsed_micros).unwrap_or(u32::MAX),
            lateness_us: u32::try_from(sample.timer_lateness_micros).unwrap_or(u32::MAX),
            sequence: sample.first_sequence.get().into(),
            queue: sample.queue.into(),
            available: available.into(),
            enabled: state.enabled.into(),
            valid: state.valid.into(),
            done: state.completion_pending.into(),
            timeout: state.timeout_pending.into(),
            collision: state.collision_pending.into(),
            aifsn: state.aifsn.into(),
            backoff: state.contention_window.into(),
            priority: state.scheduler_priority.into(),
            cca_force: state.cca_force.into(),
            cca_aux_force: state.cca_aux_force.into(),
            mac_active_state: state.mac_active_state.into(),
            rx_hang: state.hang.rx.into(),
            tx_hang: state.hang.tx.into(),
            rx_tx_hang: state.hang.rx_tx_hang,
            rx_tx_panic: state.hang.rx_tx_panic,
            rx_mpdu_count: state.rx_mpdu_count.into(),
            rx_signal_count: state.rx_signal_count.into(),
            rx_end_count: state.rx_end_count.into(),
            rx_fcs_error_count: state.rx_fcs_error_count.into(),
            rx_abort_count: state.rx_abort_count.into(),
        });
    }

    /// Entries are grouped by observed wait age, then reservation order.
    /// Use the recorded timestamp when reconstructing chronological order.
    pub fn records(&self) -> impl Iterator<Item = Record> + '_ {
        self.slots.iter().filter_map(Slot::read)
    }

    pub fn dropped(&self) -> u32 {
        self.dropped.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_ieee80211_mac::sequence::SequenceNumber;

    #[test]
    fn frequent_short_waits_cannot_exhaust_long_wait_capacity() {
        let trace = Trace::new();
        assert_eq!(trace.records().count(), 0);
        for sequence in 0..40 {
            trace.record(TxWaitSample {
                deadline_wake: true,
                at_micros: u64::from(sequence),
                elapsed_micros: 5_000,
                timer_lateness_micros: 3,
                first_sequence: SequenceNumber::new(sequence).unwrap(),
                queue: 2,
                snapshot: None,
            });
        }
        assert_eq!(trace.records().count(), 8);
        assert_eq!(trace.dropped(), 32);
        for (index, record) in trace.records().enumerate() {
            assert_eq!(record.sequence, index as u32);
            assert_eq!(record.available, 0);
            assert_eq!(record.lateness_us, 3);
        }
        for age in [10_000, 20_000, 40_000] {
            for sequence in 0..9 {
                trace.record(TxWaitSample {
                    deadline_wake: true,
                    at_micros: age + u64::from(sequence),
                    elapsed_micros: age,
                    timer_lateness_micros: 0,
                    first_sequence: SequenceNumber::new(sequence).unwrap(),
                    queue: 2,
                    snapshot: None,
                });
            }
            assert_eq!(
                trace
                    .records()
                    .filter(|r| u64::from(r.elapsed_us) == age)
                    .count(),
                8
            );
        }
        assert_eq!(trace.records().count(), 32);
        assert_eq!(trace.dropped(), 35);
    }

    #[test]
    fn available_snapshot_preserves_mac_activity_and_pending_completion() {
        let trace = Trace::new();
        let mut sample = TxWaitSample {
            deadline_wake: true,
            at_micros: u64::from(u32::MAX) + 27,
            elapsed_micros: 10_022,
            timer_lateness_micros: 22,
            first_sequence: SequenceNumber::new(598).unwrap(),
            queue: 2,
            snapshot: Some(Default::default()),
        };
        let snapshot = sample.snapshot.as_mut().unwrap();
        snapshot.mac_active_state = 3;
        snapshot.completion_pending = true;
        snapshot.hang.rx = 7;
        snapshot.hang.rx_tx_hang = 259;
        snapshot.rx_mpdu_count = 531;
        snapshot.rx_end_count = 937;
        trace.record(sample);
        let record = trace.records().next().unwrap();
        assert_eq!(record.available, 1);
        assert_eq!(record.mac_active_state, 3);
        assert_eq!(record.done, 1);
        assert_eq!(record.rx_hang, 7);
        assert_eq!(record.rx_tx_hang, 259);
        assert_eq!(record.rx_mpdu_count, 531);
        assert_eq!(record.rx_end_count, 937);
        assert_eq!(record.elapsed_us, 10_022);
        assert_eq!(
            (u64::from(record.at_high) << 32) | u64::from(record.at_low),
            u64::from(u32::MAX) + 27
        );
    }

    #[test]
    fn reader_skips_reserved_but_unpublished_slot() {
        let trace = Trace::new();
        trace.reserved[0].store(1, Ordering::Relaxed);
        assert_eq!(trace.records().count(), 0);
        trace.slots[0].publish(Record {
            sequence: 42,
            ..Record::default()
        });
        assert_eq!(trace.records().next().unwrap().sequence, 42);
    }
}
