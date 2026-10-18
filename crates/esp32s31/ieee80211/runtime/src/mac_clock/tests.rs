use core::cell::Cell;

use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use oer_time::{Duration, Instant};
use oer_time_virtual::VirtualClock;

use super::{
    LocalTimeCounter, MAC_CLOCK_RELATION, MacClockReader, MacClockStorage, ReceptionTimer,
};

/// A MAC counter that reads the virtual monotonic time plus an offset, or
/// holds still while stopped.
struct Counter<'c> {
    clock: &'c VirtualClock,
    offset: Cell<u32>,
    stopped_at: Cell<Option<u32>>,
}

impl<'c> Counter<'c> {
    fn new(clock: &'c VirtualClock, offset: u32) -> Self {
        Self {
            clock,
            offset: Cell::new(offset),
            stopped_at: Cell::new(None),
        }
    }

    /// The raw value the counter reads at the monotonic instant `at`.
    fn raw_at(&self, at: Instant) -> u32 {
        (at.as_micros() as u32).wrapping_add(self.offset.get())
    }
}

impl LocalTimeCounter for &Counter<'_> {
    fn read(&self) -> u32 {
        self.stopped_at
            .get()
            .unwrap_or_else(|| self.raw_at(oer_time::Clock::now(self.clock)))
    }
}

fn at(micros: u64) -> Instant {
    Instant::from_micros(micros)
}

#[test]
fn a_reception_converts_to_its_monotonic_time() {
    let clock = VirtualClock::new();
    clock.advance_to(at(1_000_000));
    let counter = Counter::new(&clock, 2_596_124);
    let storage = MacClockStorage::<NoopRawMutex, _, _>::new(&clock);
    let mac = storage.start(&counter);
    mac.now().unwrap();
    let stamped = counter.raw_at(at(1_000_000));
    // The frame is handed off 400 us after its reception.
    clock.advance(Duration::from_micros(400)).unwrap();
    assert_eq!(mac.received_at(stamped), Some(at(1_000_000)));
    let sample = mac.sample().unwrap();
    assert_eq!(sample.generation, crate::clock_generation(0));
    assert_eq!(sample.target, at(1_000_400));
    assert_eq!(
        mac.stamp(stamped)
            .map(|stamp| MAC_CLOCK_RELATION.project(stamp.at, stamp.generation, &sample))
            .unwrap()
            .map(|projected| projected.at),
        Ok(at(1_000_000))
    );
}

#[test]
fn a_stamp_later_than_now_has_no_place() {
    let clock = VirtualClock::new();
    clock.advance_to(at(5_000));
    let counter = Counter::new(&clock, 0);
    let storage = MacClockStorage::<NoopRawMutex, _, _>::new(&clock);
    let mac = storage.start(&counter);
    mac.now().unwrap();
    assert_eq!(mac.received_at(counter.raw_at(at(6_000))), None);
}

#[test]
fn readings_carry_across_the_counter_wrap() {
    let clock = VirtualClock::new();
    clock.advance_to(at(10));
    let counter = Counter::new(&clock, u32::MAX - 100);
    let storage = MacClockStorage::<NoopRawMutex, _, _>::new(&clock);
    let mac = storage.start(&counter);
    let start = mac.now().unwrap().as_micros();
    clock.advance(Duration::from_micros(500)).unwrap();
    assert_eq!(mac.now().unwrap().as_micros(), start + 500);
    let stamped = counter.raw_at(at(300));
    assert_eq!(mac.received_at(stamped), Some(at(300)));
}

#[test]
fn a_wake_starts_a_generation_and_earlier_stamps_keep_their_sample() {
    let clock = VirtualClock::new();
    clock.advance_to(at(100_000));
    let counter = Counter::new(&clock, 1_000);
    let storage = MacClockStorage::<NoopRawMutex, _, _>::new(&clock);
    let mac = storage.start(&counter);
    mac.sample().unwrap();
    let before_sleep = counter.raw_at(at(100_000));
    // The counter stops for a 20 ms sleep, then runs on from where it held.
    counter.stopped_at.set(Some(counter.raw_at(at(100_050))));
    clock.advance_to(at(120_050));
    counter.stopped_at.set(None);
    counter.offset.set(1_000u32.wrapping_sub(20_000));
    mac.on_rf_wake();
    let after_wake = counter.raw_at(at(120_050));
    clock.advance(Duration::from_micros(30)).unwrap();
    assert_eq!(mac.sample().unwrap().generation, crate::clock_generation(1));
    // A frame from before the sleep converts in its own relation.
    assert_eq!(
        mac.stamp(before_sleep).unwrap().generation,
        crate::clock_generation(0)
    );
    assert_eq!(mac.received_at(before_sleep), Some(at(100_000)));
    // A frame after the wake converts in the new one.
    assert_eq!(
        mac.stamp(after_wake).unwrap().generation,
        crate::clock_generation(1)
    );
    assert_eq!(mac.received_at(after_wake), Some(at(120_050)));
}

#[test]
fn a_counter_that_went_back_across_a_wake_leaves_earlier_stamps_without_a_place() {
    let clock = VirtualClock::new();
    clock.advance_to(at(100_000));
    let counter = Counter::new(&clock, 3_000_000_000);
    let storage = MacClockStorage::<NoopRawMutex, _, _>::new(&clock);
    let mac = storage.start(&counter);
    mac.sample().unwrap();
    let before_sleep = counter.raw_at(at(100_000));
    clock.advance_to(at(150_000));
    // The counter restarted during the sleep.
    counter
        .offset
        .set(0u32.wrapping_sub(150_000).wrapping_add(7));
    mac.on_rf_wake();
    let wake = mac.sample().unwrap();
    assert_eq!(wake.generation, crate::clock_generation(1));
    // The timeline goes on at the monotonic projection of the last sample.
    assert_eq!(wake.source.as_micros(), 3_000_150_000);
    assert_eq!(mac.received_at(before_sleep), None);
    let after_wake = counter.raw_at(at(150_000));
    clock.advance(Duration::from_micros(10)).unwrap();
    assert_eq!(mac.received_at(after_wake), Some(at(150_000)));
}

#[test]
fn readings_any_time_apart_keep_one_generation() {
    let clock = VirtualClock::new();
    clock.advance_to(at(1_000));
    let counter = Counter::new(&clock, 77);
    let storage = MacClockStorage::<NoopRawMutex, _, _>::new(&clock);
    let mac = storage.start(&counter);
    mac.sample().unwrap();
    // Three counter wraps without a reading.
    let later = at(1_000 + (3 << 32) + 250);
    clock.advance_to(later);
    let stamped = counter.raw_at(later);
    clock.advance(Duration::from_micros(40)).unwrap();
    assert_eq!(mac.received_at(stamped), Some(later));
    assert_eq!(mac.sample().unwrap().generation, crate::clock_generation(0));
}

#[test]
fn a_counter_jump_outside_a_wake_breaks_the_relation() {
    let clock = VirtualClock::new();
    clock.advance_to(at(10_000));
    let counter = Counter::new(&clock, 0);
    let storage = MacClockStorage::<NoopRawMutex, _, _>::new(&clock);
    let mac = storage.start(&counter);
    mac.sample().unwrap();
    let before = counter.raw_at(at(10_000));
    clock.advance_to(at(11_000));
    counter.offset.set(5_000_000);
    let sample = mac.sample().unwrap();
    assert_eq!(sample.generation, crate::clock_generation(1));
    // The timeline goes on at the projection, not at the jumped counter.
    assert_eq!(sample.source.as_micros(), 11_000);
    assert_eq!(mac.received_at(before), None);
}

#[test]
fn a_new_radio_start_replaces_the_clock_and_its_handles() {
    let clock = VirtualClock::new();
    clock.advance_to(at(10_000));
    let counter = Counter::new(&clock, 1_000);
    let storage = MacClockStorage::<NoopRawMutex, _, _>::new(&clock);
    let first = storage.start(&counter);
    first.on_rf_wake();
    assert_eq!(
        first.sample().unwrap().generation,
        crate::clock_generation(1)
    );
    let stamped = counter.raw_at(at(10_000));

    // The radio restarts: the new clock is fresh, in a generation the
    // earlier start never used, the old handle reads nothing, and a stamp
    // of the earlier start converts only as a reading of the new counter.
    clock.advance_to(at(20_000));
    let second = storage.start(&counter);
    assert_eq!(first.sample(), None);
    assert_eq!(first.received_at(stamped), None);
    let sample = second.sample().unwrap();
    assert!(sample.generation.jump() > 1);
    assert_eq!(sample.target, at(20_000));
}

#[test]
fn tx_reader_follows_rf_generation_and_refuses_a_replaced_radio_start() {
    let clock = VirtualClock::new();
    let counter = Counter::new(&clock, 5_000);
    let storage = MacClockStorage::<NoopRawMutex, _, _>::new(&clock);
    let mac = storage.start(&counter);
    let timer = ReceptionTimer {
        timer: &clock,
        reception: mac,
    };
    let before_sleep = MacClockReader::sample(&timer).unwrap();
    assert_eq!(before_sleep.source.as_micros(), 5_000);
    assert_eq!(oer_time::Clock::now(&timer), at(0));

    // The power owner reports the stopped counter's wake. The TX reader
    // sees the updated generation while the monotonic timer keeps advancing.
    counter.stopped_at.set(Some(5_000));
    clock.advance_to(at(20_000));
    mac.on_rf_wake();
    let after_wake = MacClockReader::sample(&timer).unwrap();
    assert_eq!(after_wake.source, before_sleep.source);
    assert_ne!(after_wake.generation, before_sleep.generation);
    assert_eq!(after_wake, mac.snapshot().unwrap().sample());
    assert_eq!(oer_time::Clock::now(&timer), at(20_000));

    let restarted = storage.start(&counter);
    assert_eq!(MacClockReader::sample(&timer), None);
    let rebound = ReceptionTimer {
        timer: &clock,
        reception: restarted,
    };
    assert!(
        MacClockReader::sample(&rebound).unwrap().generation.jump() > after_wake.generation.jump()
    );
}

#[test]
fn tsf_epochs_are_never_reused_across_radio_starts() {
    let clock = VirtualClock::new();
    let counter = Counter::new(&clock, 0);
    let storage = MacClockStorage::<NoopRawMutex, _, _>::new(&clock);
    let first = storage.start(&counter);
    let a = first.tsf_epoch();
    let b = first.tsf_epoch();
    let second = storage.start(&counter);
    let c = second.tsf_epoch();
    assert!(a != b && b != c && a != c);
}

#[test]
fn a_tsf_sample_of_an_earlier_radio_start_is_refused_with_its_mac_clock() {
    use oer_ieee80211_lower_mac::{TsfProjectionError, TsfRelation, TsfSample, VifId, VifTsf};
    use oer_ieee80211_mac::tsf::TsfInstant;
    let clock = VirtualClock::new();
    clock.advance_to(at(10_000));
    let counter = Counter::new(&clock, 0);
    let storage = MacClockStorage::<NoopRawMutex, _, _>::new(&clock);
    let first = storage.start(&counter);
    let relation = TsfRelation::new(
        first.tsf_epoch(),
        oer_ieee80211_mac::tsf::TsfDuration::from_micros(1),
    );
    let sample = TsfSample {
        tsf: VifTsf::new(VifId(0), TsfInstant::from_micros(1_000_000)),
        local: first.sample().unwrap().stamp(),
        uncertainty: oer_ieee80211_lower_mac::Ieee80211Duration::from_micros(1),
        generation: relation.generation(),
    };
    let now = first.sample().unwrap().stamp();
    assert!(sample.tsf_at(now, relation.generation()).is_ok());

    // After a radio restart the MAC clock's stamps are of a new generation,
    // and the old handle has none: the TSF sample no longer converts.
    clock.advance_to(at(20_000));
    let second = storage.start(&counter);
    assert_eq!(first.sample(), None);
    let restarted = second.sample().unwrap().stamp();
    assert_eq!(
        sample.tsf_at(restarted, relation.generation()),
        Err(TsfProjectionError::StaleStamp)
    );
}

#[test]
fn a_counter_reading_pairs_the_raw_counter_with_monotonic_time_and_keeps_the_relation() {
    let clock = VirtualClock::new();
    clock.advance_to(at(10_000));
    let counter = Counter::new(&clock, 1_000);
    let storage = MacClockStorage::<NoopRawMutex, _, _>::new(&clock);
    let mac = storage.start(&counter);
    let before = mac.sample().unwrap();
    clock.advance_to(at(12_000));
    assert_eq!(
        mac.counter_reading(),
        Some((counter.raw_at(at(12_000)), at(12_000)))
    );
    // The reading neither starts a generation nor moves the timeline.
    assert_eq!(mac.sample().unwrap().generation, before.generation);
    // A replaced clock has no reading.
    let _later = storage.start(&counter);
    assert_eq!(mac.counter_reading(), None);
}
