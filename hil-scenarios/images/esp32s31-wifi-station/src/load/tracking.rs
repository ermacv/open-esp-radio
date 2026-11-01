//! The shared RF actor's periodic PHY tracking ticks within a window.
//!
//! The actor reports each tick through `oer_trace::observe!`, so only an
//! `oer_trace` build counts them. Their one writer is the image's observer
//! in the radio executor (P2) on the boot hart; the load server on hart 1
//! samples them at the window's ends, as it samples the parts: a sequence
//! number brackets each write, and a sample taken while the writer wrote
//! is taken again. The temperature extremes, the last temperature and the
//! longest tick belong to an epoch (the parts' epoch, which starts at the
//! window's start): the first tick of a new epoch starts them afresh.
//! Counts wrap at 2^32.

use core::sync::atomic::{AtomicU32, Ordering, fence};

use oer_services_load::Tracking;

/// What one tick did, as the counter takes it.
#[cfg_attr(
    not(any(oer_trace, test)),
    allow(dead_code, reason = "only a trace build counts the tracking ticks")
)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Tick {
    pub ran: Ran,
    /// The PHY's temperature after a tracked tick, in sensor units.
    pub temperature: Option<i16>,
    pub micros: u32,
}

#[cfg_attr(
    not(any(oer_trace, test)),
    allow(dead_code, reason = "only a trace build counts the tracking ticks")
)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum Ran {
    Tracked(Branches),
    #[default]
    NotDue,
    Unavailable,
}

/// The branches one tracked tick committed.
#[cfg_attr(
    not(any(oer_trace, test)),
    allow(dead_code, reason = "only a trace build counts the tracking ticks")
)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Branches {
    pub inhibited: bool,
    pub rfpll_corrected: bool,
    pub wifi_i2c_changed: bool,
    pub wifi_tx_power: bool,
    pub bluetooth_ieee802154_tx_power: bool,
    pub calibration_common: bool,
    pub calibration_transmit: bool,
}

/// The counts by kind, in this order.
const TICKS: usize = 0;
const NOT_DUE: usize = 1;
const UNAVAILABLE: usize = 2;
const INHIBITED: usize = 3;
const RFPLL: usize = 4;
const WIFI_I2C: usize = 5;
const WIFI_TX_POWER: usize = 6;
const BLE_TX_POWER: usize = 7;
const CALIBRATION_COMMON: usize = 8;
const CALIBRATION_TRANSMIT: usize = 9;
const TICK_US: usize = 10;
const COUNTS: usize = 11;

/// A temperature word: bit 16 present, bits 0..16 the value's two's
/// complement.
const PRESENT: u32 = 1 << 16;

/// The ticks' counts and the current epoch's extremes.
pub(crate) struct TrackingCounter {
    sequence: AtomicU32,
    counts: [AtomicU32; COUNTS],
    /// The epoch the words below belong to.
    epoch: AtomicU32,
    last: AtomicU32,
    min: AtomicU32,
    max: AtomicU32,
    /// The longest tick plus one; zero without one.
    longest: AtomicU32,
}

/// The writer was writing while the counter was sampled.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Concurrent;

/// One reading of the counter.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Sample {
    counts: [u32; COUNTS],
    last: Option<i16>,
    min: Option<i16>,
    max: Option<i16>,
    longest_us: Option<u32>,
}

impl TrackingCounter {
    pub(crate) const fn new() -> Self {
        Self {
            sequence: AtomicU32::new(0),
            counts: [const { AtomicU32::new(0) }; COUNTS],
            epoch: AtomicU32::new(0),
            last: AtomicU32::new(0),
            min: AtomicU32::new(0),
            max: AtomicU32::new(0),
            longest: AtomicU32::new(0),
        }
    }

    /// Count `tick` in `epoch`; only the observer in the radio executor
    /// calls it.
    #[cfg(any(oer_trace, test))]
    pub(crate) fn observe(&self, tick: Tick, epoch: u32) {
        let sequence = self.sequence.load(Ordering::Relaxed);
        self.sequence
            .store(sequence.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        let fresh = self.epoch.load(Ordering::Relaxed) != epoch;
        if fresh {
            for word in [&self.last, &self.min, &self.max, &self.longest] {
                word.store(0, Ordering::Relaxed);
            }
            self.epoch.store(epoch, Ordering::Relaxed);
        }
        match tick.ran {
            Ran::Tracked(branches) => {
                self.bump(TICKS, 1);
                for (taken, count) in [
                    (branches.inhibited, INHIBITED),
                    (branches.rfpll_corrected, RFPLL),
                    (branches.wifi_i2c_changed, WIFI_I2C),
                    (branches.wifi_tx_power, WIFI_TX_POWER),
                    (branches.bluetooth_ieee802154_tx_power, BLE_TX_POWER),
                    (branches.calibration_common, CALIBRATION_COMMON),
                    (branches.calibration_transmit, CALIBRATION_TRANSMIT),
                ] {
                    self.bump(count, u32::from(taken));
                }
            }
            Ran::NotDue => self.bump(NOT_DUE, 1),
            Ran::Unavailable => self.bump(UNAVAILABLE, 1),
        }
        self.bump(TICK_US, tick.micros);
        if let Some(value) = tick.temperature {
            let word = PRESENT | u32::from(value as u16);
            self.last.store(word, Ordering::Relaxed);
            let keep = |extreme: &AtomicU32, better: fn(i16, i16) -> bool| {
                if temperature(extreme.load(Ordering::Relaxed))
                    .is_none_or(|kept| better(value, kept))
                {
                    extreme.store(word, Ordering::Relaxed);
                }
            };
            keep(&self.min, |new, kept| new < kept);
            keep(&self.max, |new, kept| new > kept);
        }
        let longest = tick.micros.saturating_add(1);
        if longest > self.longest.load(Ordering::Relaxed) {
            self.longest.store(longest, Ordering::Relaxed);
        }
        self.sequence
            .store(sequence.wrapping_add(2), Ordering::Release);
    }

    #[cfg(any(oer_trace, test))]
    fn bump(&self, count: usize, by: u32) {
        if let Some(count) = self.counts.get(count) {
            count.store(
                count.load(Ordering::Relaxed).wrapping_add(by),
                Ordering::Relaxed,
            );
        }
    }

    /// The counts now and the extremes of `epoch`.
    pub(crate) fn sample(&self, epoch: u32) -> Result<Sample, Concurrent> {
        let sequence = self.sequence.load(Ordering::Acquire);
        if sequence & 1 != 0 {
            return Err(Concurrent);
        }
        let counts = self
            .counts
            .each_ref()
            .map(|count| count.load(Ordering::Relaxed));
        let kept = self.epoch.load(Ordering::Relaxed) == epoch;
        let [last, min, max, longest] =
            [&self.last, &self.min, &self.max, &self.longest].map(|word| {
                if kept {
                    word.load(Ordering::Relaxed)
                } else {
                    0
                }
            });
        fence(Ordering::Acquire);
        if self.sequence.load(Ordering::Relaxed) != sequence {
            return Err(Concurrent);
        }
        Ok(Sample {
            counts,
            last: temperature(last),
            min: temperature(min),
            max: temperature(max),
            longest_us: longest.checked_sub(1),
        })
    }
}

/// The temperature a word holds.
fn temperature(word: u32) -> Option<i16> {
    (word & PRESENT != 0).then_some(word as u16 as i16)
}

/// The ticks between the window's ends `start` and `end`; the extremes are
/// the end's, of the window's epoch. `available` says whether the image
/// counts them.
pub(crate) fn window(start: &Sample, end: &Sample, available: bool) -> Tracking {
    let count = |kind: usize| {
        let at = |sample: &Sample| sample.counts.get(kind).copied().unwrap_or(0);
        at(end).wrapping_sub(at(start))
    };
    Tracking {
        available,
        ticks: count(TICKS),
        not_due: count(NOT_DUE),
        unavailable: count(UNAVAILABLE),
        inhibited: count(INHIBITED),
        rfpll_corrected: count(RFPLL),
        wifi_i2c_changed: count(WIFI_I2C),
        wifi_tx_power: count(WIFI_TX_POWER),
        bluetooth_ieee802154_tx_power: count(BLE_TX_POWER),
        calibration_common: count(CALIBRATION_COMMON),
        calibration_transmit: count(CALIBRATION_TRANSMIT),
        temperature_last: end.last,
        temperature_min: end.min,
        temperature_max: end.max,
        tick_us: count(TICK_US),
        longest_tick_us: end.longest_us,
    }
}

/// The ticks of the shared RF actor.
#[cfg(target_arch = "riscv32")]
pub(crate) static COUNTER: TrackingCounter = TrackingCounter::new();

/// Whether this build counts the ticks.
#[cfg(target_arch = "riscv32")]
pub(crate) const AVAILABLE: bool = cfg!(oer_trace);

/// The counter's tick of the actor's observation.
#[cfg(all(target_arch = "riscv32", oer_trace))]
pub(crate) fn tick(observed: oer_esp32s31_wifi::observation::TrackingTick) -> Tick {
    use oer_esp32s31_wifi::observation::TickRan;
    Tick {
        ran: match observed.ran {
            TickRan::Tracked(branches) => Ran::Tracked(Branches {
                inhibited: branches.inhibited,
                rfpll_corrected: branches.rfpll_corrected,
                wifi_i2c_changed: branches.wifi_i2c_changed,
                wifi_tx_power: branches.wifi_tx_power,
                bluetooth_ieee802154_tx_power: branches.bluetooth_ieee802154_tx_power,
                calibration_common: branches.calibration_common,
                calibration_transmit: branches.calibration_transmit,
            }),
            TickRan::NotDue => Ran::NotDue,
            TickRan::Unavailable => Ran::Unavailable,
        },
        temperature: observed.temperature,
        micros: observed.micros,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracked(temperature: i16, micros: u32, branches: Branches) -> Tick {
        Tick {
            ran: Ran::Tracked(branches),
            temperature: Some(temperature),
            micros,
        }
    }

    #[test]
    fn a_window_counts_its_ticks_and_branches_and_keeps_its_epoch_s_extremes() {
        let counter = TrackingCounter::new();
        // Before the window, in epoch 0: counted, but not the window's.
        counter.observe(tracked(90, 5_000, Branches::default()), 0);
        let start = counter.sample(1).unwrap();
        assert_eq!(start.min, None, "epoch 1 has no tick yet");
        let corrected = Branches {
            rfpll_corrected: true,
            calibration_transmit: true,
            ..Branches::default()
        };
        counter.observe(tracked(40, 300, corrected), 1);
        counter.observe(
            Tick {
                ran: Ran::NotDue,
                temperature: None,
                micros: 2,
            },
            1,
        );
        counter.observe(
            tracked(
                -7,
                1_200,
                Branches {
                    wifi_i2c_changed: true,
                    ..Branches::default()
                },
            ),
            1,
        );
        counter.observe(tracked(12, 100, Branches::default()), 1);
        counter.observe(
            Tick {
                ran: Ran::Unavailable,
                temperature: None,
                micros: 0,
            },
            1,
        );
        let end = counter.sample(1).unwrap();
        assert_eq!(
            window(&start, &end, true),
            Tracking {
                available: true,
                ticks: 3,
                not_due: 1,
                unavailable: 1,
                inhibited: 0,
                rfpll_corrected: 1,
                wifi_i2c_changed: 1,
                wifi_tx_power: 0,
                bluetooth_ieee802154_tx_power: 0,
                calibration_common: 0,
                calibration_transmit: 1,
                temperature_last: Some(12),
                temperature_min: Some(-7),
                temperature_max: Some(40),
                tick_us: 1_602,
                longest_tick_us: Some(1_200),
            }
        );
    }

    #[test]
    fn a_window_without_a_tick_reports_no_temperature_or_longest_tick() {
        let counter = TrackingCounter::new();
        counter.observe(tracked(30, 10, Branches::default()), 3);
        let start = counter.sample(4).unwrap();
        let end = counter.sample(4).unwrap();
        let window = window(&start, &end, false);
        assert_eq!(window, Tracking::default());
        assert_eq!(
            (window.temperature_last, window.longest_tick_us),
            (None, None)
        );
    }

    #[test]
    fn a_sample_during_a_write_is_refused() {
        let counter = TrackingCounter::new();
        counter.sequence.store(1, Ordering::Relaxed);
        assert_eq!(counter.sample(0), Err(Concurrent));
    }

    #[test]
    fn the_extreme_temperatures_of_the_type_round_trip() {
        let counter = TrackingCounter::new();
        counter.observe(tracked(i16::MIN, 0, Branches::default()), 1);
        counter.observe(tracked(i16::MAX, u32::MAX, Branches::default()), 1);
        let sample = counter.sample(1).unwrap();
        assert_eq!(
            (sample.min, sample.max, sample.last),
            (Some(i16::MIN), Some(i16::MAX), Some(i16::MAX))
        );
        assert_eq!(
            sample.longest_us,
            Some(u32::MAX - 1),
            "the longest saturates"
        );
    }
}
