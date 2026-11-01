//! The shared RF actor's periodic PHY tracking ticks.
//!
//! The actor's observer, in the radio executor on the boot hart, is the
//! counter's one writer. The temperature extremes, the last temperature
//! and the longest tick belong to an epoch (the parts' epoch): the first
//! tick of a new epoch starts them afresh. Counts wrap at 2^32.

use core::sync::atomic::{AtomicU32, Ordering, fence};

use super::{Concurrent, bump};

/// What one tick did.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Tick {
    pub ran: Ran,
    /// The PHY's temperature after a tracked tick, in sensor units.
    pub temperature: Option<i16>,
    pub micros: u32,
}

/// Whether a tick tracked.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Ran {
    Tracked(Branches),
    #[default]
    NotDue,
    Unavailable,
}

/// The branches one tracked tick committed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Branches {
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

/// One reading: the ticks' wrapping counts since boot, by what they did,
/// their microseconds, and the extremes of the epoch it was read in.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TrackingSample {
    /// Tracked ticks, and those that were not due or found the PHY
    /// unavailable.
    pub ticks: u32,
    pub not_due: u32,
    pub unavailable: u32,
    /// The tracked ticks' committed branches.
    pub inhibited: u32,
    pub rfpll_corrected: u32,
    pub wifi_i2c_changed: u32,
    pub wifi_tx_power: u32,
    pub bluetooth_ieee802154_tx_power: u32,
    pub calibration_common: u32,
    pub calibration_transmit: u32,
    /// Every tick's microseconds.
    pub tick_us: u32,
    /// The epoch's last, lowest and highest temperatures, and its longest
    /// tick (saturating one below `u32::MAX`).
    pub temperature_last: Option<i16>,
    pub temperature_min: Option<i16>,
    pub temperature_max: Option<i16>,
    pub longest_tick_us: Option<u32>,
}

/// The ticks' counts and the current epoch's extremes.
pub struct TrackingCounter {
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

impl TrackingCounter {
    pub const fn new() -> Self {
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

    /// Count `tick` in `epoch`; only the counter's writer calls it.
    pub fn observe(&self, tick: Tick, epoch: u32) {
        let sequence = self.sequence.load(Ordering::Relaxed);
        self.sequence
            .store(sequence.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        if self.epoch.load(Ordering::Relaxed) != epoch {
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
            let [low, high] = value.to_le_bytes();
            let word = PRESENT | u32::from(u16::from_le_bytes([low, high]));
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

    fn bump(&self, count: usize, by: u32) {
        if let Some(count) = self.counts.get(count) {
            bump(count, by);
        }
    }

    /// The counts now and the extremes of `epoch`.
    ///
    /// # Errors
    /// [`Concurrent`] while the writer writes.
    pub fn sample(&self, epoch: u32) -> Result<TrackingSample, Concurrent> {
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
        let [
            ticks,
            not_due,
            unavailable,
            inhibited,
            rfpll_corrected,
            wifi_i2c_changed,
            wifi_tx_power,
            bluetooth_ieee802154_tx_power,
            calibration_common,
            calibration_transmit,
            tick_us,
        ] = counts;
        Ok(TrackingSample {
            ticks,
            not_due,
            unavailable,
            inhibited,
            rfpll_corrected,
            wifi_i2c_changed,
            wifi_tx_power,
            bluetooth_ieee802154_tx_power,
            calibration_common,
            calibration_transmit,
            tick_us,
            temperature_last: temperature(last),
            temperature_min: temperature(min),
            temperature_max: temperature(max),
            longest_tick_us: longest.checked_sub(1),
        })
    }
}

impl Default for TrackingCounter {
    fn default() -> Self {
        Self::new()
    }
}

/// The temperature a word holds.
fn temperature(word: u32) -> Option<i16> {
    let [low, high, _, _] = word.to_le_bytes();
    (word & PRESENT != 0).then_some(i16::from_le_bytes([low, high]))
}

#[cfg(test)]
mod tests;
