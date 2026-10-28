//! Hardware-free assembly and interpretation of frozen radio-health pages.
use oer_esp32s31_instrumentation::health::*;
use oer_esp32s31_instrumentation_hil::*;
use serde::{Deserialize, Serialize};

type Result<T> = std::result::Result<T, &'static str>;
#[derive(Clone, Debug)]
pub struct Pages<T> {
    identity: HealthIdentity,
    total: u16,
    values: Vec<T>,
}
impl<T> Pages<T> {
    pub fn new(identity: HealthIdentity, total: u16) -> Self {
        Self {
            identity,
            total,
            values: Vec::new(),
        }
    }
    pub fn next(&self, limit: usize) -> Option<HealthPageRequest> {
        if limit == 0 || limit > usize::from(u8::MAX) {
            return None;
        }
        let remaining = usize::from(self.total).checked_sub(self.values.len())?;
        (remaining != 0).then_some(HealthPageRequest {
            identity: self.identity,
            first: self.values.len() as u16,
            count: remaining.min(limit) as u8,
        })
    }
    pub fn append(
        &mut self,
        identity: HealthIdentity,
        first: u16,
        total: u16,
        values: impl IntoIterator<Item = T>,
        requested: HealthPageRequest,
    ) -> Result<()> {
        if identity != self.identity
            || requested.identity != self.identity
            || total != self.total
            || usize::from(first) != self.values.len()
            || first != requested.first
        {
            return Err("health page identity or continuity changed");
        }
        let values: Vec<T> = values.into_iter().collect();
        if values.len() != usize::from(requested.count)
            || self.values.len() + values.len() > usize::from(total)
        {
            return Err("health page length changed");
        }
        self.values.extend(values);
        Ok(())
    }
    pub fn finish(self) -> Result<Vec<T>> {
        if self.values.len() != usize::from(self.total) {
            Err("missing health page")
        } else {
            Ok(self.values)
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CounterDelta {
    pub kind: CounterKind,
    pub before: CounterValue,
    pub after: CounterValue,
    /// Modulo only; multiple complete wraps are indeterminate.
    pub modulo_increment: Option<u32>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Snapshot {
    pub config: BeginHealth,
    pub started: HealthStarted,
    pub window: HealthWindow,
    pub finished: HealthFinished,
    pub counters: Vec<Counter>,
    pub signals: Vec<Signal>,
    pub tracking: Vec<Tracking>,
    pub measurements: Vec<Measurement>,
    pub deltas: Vec<CounterDelta>,
    pub complete: bool,
}
impl Snapshot {
    #[allow(clippy::too_many_arguments)]
    pub fn assemble(
        config: BeginHealth,
        started: HealthStarted,
        window: HealthWindow,
        finished: HealthFinished,
        counters: Vec<Counter>,
        signals: Vec<Signal>,
        tracking: Vec<Tracking>,
        measurements: Vec<Measurement>,
    ) -> Result<Self> {
        if !config.valid()
            || started.observation == 0
            || started.boot == 0
            || started.capacity == 0
            || usize::from(started.capacity) > MAX_RECORDS
            || started.storage_bytes as usize > PSRAM_BUDGET
            || window.identity != finished.identity
            || finished.identity.observation != started.observation
            || finished.identity.epoch == 0
            || window.finished_micros < window.started_micros
            || finished
                .counts
                .total()
                .is_none_or(|total| total > usize::from(started.capacity))
            || counters.len() != usize::from(finished.counts.counters)
            || signals.len() != usize::from(finished.counts.signals)
            || tracking.len() != usize::from(finished.counts.tracking)
            || measurements.len() != usize::from(finished.counts.measurements)
        {
            return Err("inconsistent health snapshot identity, budget or counts");
        }
        let mut deltas = Vec::new();
        for (index, before) in counters
            .iter()
            .enumerate()
            .filter(|(_, c)| c.cut == Cut::Before)
        {
            if counters[..index]
                .iter()
                .any(|c| c.cut == Cut::Before && c.kind == before.kind)
            {
                return Err("duplicate counter cut");
            }
            let mut after = counters
                .iter()
                .filter(|c| c.cut == Cut::After && c.kind == before.kind);
            let Some(after_cut) = after.next() else {
                return Err("counter has no after cut");
            };
            if after.next().is_some()
                || !before.read.valid()
                || !after_cut.read.valid()
                || before.read.last_micros > after_cut.read.first_micros
                || before.epoch != finished.identity.epoch
                || after_cut.epoch != before.epoch
            {
                return Err("counter reset, epoch or read bracket discontinuity");
            }
            let delta = after_cut.value.delta_since(before.value, true);
            if delta.is_none()
                && !matches!(
                    (before.value, after_cut.value),
                    (
                        CounterValue::SignedScaled40(_),
                        CounterValue::SignedScaled40(_)
                    )
                )
            {
                return Err("counter width changed or value exceeds its width");
            }
            deltas.push(CounterDelta {
                kind: before.kind,
                before: before.value,
                after: after_cut.value,
                modulo_increment: delta,
            });
        }
        if deltas.len() * 2 != counters.len() {
            return Err("unpaired counter cut");
        }
        if signals
            .iter()
            .any(|s| !s.read.valid() || s.channel != config.channel)
            || measurements.iter().any(|m| !m.read.valid())
        {
            return Err("invalid signal or measurement bracket");
        }
        // A retained overflow/failure stays incomplete even when every available page arrived.
        let complete = finished.complete()
            && window.rx_cleaned_up
            && !window.tracking_failed
            && !counters.is_empty();
        Ok(Self {
            config,
            started,
            window,
            finished,
            counters,
            signals,
            tracking,
            measurements,
            deltas,
            complete,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pages_reject_repeats_skips_stale_epochs_and_false_lengths() {
        let id = HealthIdentity {
            observation: 1,
            epoch: 2,
        };
        let mut pages = Pages::<u8>::new(id, 3);
        assert!(pages.next(0).is_none());
        assert!(pages.next(256).is_none());
        let first = pages.next(2).expect("first");
        assert!(pages.append(id, 1, 3, [1, 2], first).is_err());
        assert!(
            pages
                .append(HealthIdentity { epoch: 3, ..id }, 0, 3, [1, 2], first)
                .is_err()
        );
        assert!(pages.append(id, 0, 3, [1], first).is_err());
        pages.append(id, 0, 3, [1, 2], first).expect("first page");
        assert!(pages.append(id, 0, 3, [1, 2], first).is_err());
        assert!(pages.clone().finish().is_err());
        let last = pages.next(2).expect("last");
        pages.append(id, 2, 3, [3], last).expect("last page");
        assert!(pages.next(2).is_none());
        assert_eq!(pages.finish(), Ok(vec![1, 2, 3]));
    }
    #[test]
    fn overflow_cannot_become_complete_when_all_retained_pages_arrive() {
        let identity = HealthIdentity {
            observation: 1,
            epoch: 1,
        };
        let config = BeginHealth {
            channel: 1,
            peer: [2; 6],
            peer_rate_kbps: Some(1000),
            peer_power_dbm: Some(20),
        };
        let started = HealthStarted {
            observation: 1,
            boot: 3,
            capacity: 1024,
            storage_bytes: 100_000,
        };
        let window = HealthWindow {
            identity,
            started_micros: 10,
            finished_micros: 20,
            rx_cleaned_up: true,
            tracking_failed: false,
        };
        let finished = HealthFinished {
            identity,
            counts: Counts {
                counters: 2,
                ..Counts::default()
            },
            dropped: 1,
            failed: false,
            rx_cleaned_up: true,
            rf_closed: true,
        };
        let before = Counter {
            cut: Cut::Before,
            epoch: 1,
            read: Bracket {
                first_micros: 10,
                last_micros: 11,
            },
            kind: CounterKind::Mpdu,
            value: CounterValue::Modulo16(65535),
        };
        let after = Counter {
            cut: Cut::After,
            read: Bracket {
                first_micros: 19,
                last_micros: 20,
            },
            value: CounterValue::Modulo16(1),
            ..before
        };
        let snapshot = Snapshot::assemble(
            config,
            started,
            window,
            finished,
            vec![before, after],
            vec![],
            vec![],
            vec![],
        )
        .expect("partial report");
        assert!(!snapshot.complete);
        assert_eq!(snapshot.deltas[0].modulo_increment, Some(2));
        assert!(
            Snapshot::assemble(
                config,
                started,
                window,
                finished,
                vec![before, Counter { epoch: 2, ..after }],
                vec![],
                vec![],
                vec![]
            )
            .is_err()
        );
    }
}
