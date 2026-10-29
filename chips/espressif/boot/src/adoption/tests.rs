#![allow(clippy::unwrap_used)]
#![allow(
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::arithmetic_side_effects
)]
use super::*;
use std::vec;
#[path = "../../tests/support/adoption.rs"]
#[allow(
    dead_code,
    reason = "the staged runner uses the shared fake mapping events"
)]
mod support;
use oer_espressif_watchdog::{LOCK, WRITE_KEY};
use oer_time::Duration;
use support::{Log, Op};

#[test]
fn adoption_preserves_order_owners_watchdog_words_and_the_unarmed_window() {
    let log = Log::new(None);
    let mut ready = initialize(log.owners()).unwrap();
    assert_eq!(ready.clock.id, 12);
    assert_eq!(ready.resources.id, 15);
    assert!(core::ptr::eq(ready.clock.log, &log));
    assert_eq!(
        log.operations(),
        [
            Op::Cpu,
            Op::Console,
            Op::Clocks,
            Op::Cores,
            Op::Debug,
            Op::Timer0Clock,
            Op::Main1Clock,
            Op::Key(0, WRITE_KEY),
            Op::Running(0, false, false),
            Op::Latch(0),
            Op::Key(0, LOCK),
            Op::Key(1, WRITE_KEY),
            Op::Running(1, false, false),
            Op::Latch(1),
            Op::Key(1, LOCK),
            Op::Key(2, WRITE_KEY),
            Op::Hold(612_000),
            Op::Action(oer_espressif_watchdog::RtcStageAction::ResetRtc),
            Op::Feed,
            Op::Running(2, true, false),
            Op::Key(2, LOCK),
            Op::Finish,
        ]
    );
    let events = log.events.borrow();
    let clock_adoption = events.iter().find(|(_, op)| *op == Op::Clocks).unwrap().0;
    let rtc_enable = events
        .iter()
        .find(|(_, op)| *op == Op::Running(2, true, false))
        .unwrap()
        .0;
    assert_eq!(
        rtc_enable.checked_duration_since(clock_adoption),
        Some(Duration::from_micros(17))
    );
    drop(events);
    log.events.borrow_mut().clear();
    ready.watchdogs.rtc.feed();
    assert_eq!(
        log.operations(),
        [Op::Key(2, WRITE_KEY), Op::Feed, Op::Key(2, LOCK)]
    );
}

#[test]
fn refused_installation_stops_at_its_original_step_before_any_watchdog_write() {
    for (error, prefix) in [
        (InitError::ConsoleInstalled, vec![Op::Cpu, Op::Console]),
        (
            InitError::CoreControlInstalled,
            vec![Op::Cpu, Op::Console, Op::Clocks, Op::Cores],
        ),
    ] {
        let log = Log::new(Some(error));
        assert!(matches!(initialize(log.owners()), Err(actual) if actual == error));
        assert_eq!(log.operations(), prefix);
    }
    assert_eq!(InitError::ConsoleInstalled.reason(), "console-installed");
    assert_eq!(
        InitError::CoreControlInstalled.reason(),
        "core-control-installed"
    );
}

#[test]
fn cpu_adoption_preserves_integer_rate_and_refuses_fractional_or_unknown_rates() {
    use core::cell::Cell;
    struct Clock<'a> {
        sample: CpuRate,
        published: &'a Cell<Option<u32>>,
    }
    impl CpuClock for Clock<'_> {
        fn sample(&self) -> CpuRate {
            CpuRate {
                fractional: self.sample.fractional,
                integer_hz: self.sample.integer_hz,
            }
        }
        fn publish(self, rate_hz: Option<u32>) {
            self.published.set(rate_hz);
        }
    }
    for (fractional, integer_hz, expected) in [
        (false, Some(160_000_000), Some(160_000_000)),
        (true, Some(160_000_000), None),
        (false, None, None),
    ] {
        let published = Cell::new(Some(40_000_000));
        adopt_cpu_clock(Clock {
            sample: CpuRate {
                fractional,
                integer_hz,
            },
            published: &published,
        });
        assert_eq!(published.get(), expected);
    }
}
