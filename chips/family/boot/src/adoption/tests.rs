use super::*;
use std::vec;
#[path = "../../tests/support/adoption.rs"]
#[allow(
    dead_code,
    reason = "the staged runner uses the shared fake mapping events"
)]
mod support;
use oer_chips_family_drivers_watchdog::{LOCK, WRITE_KEY};
use oer_portable_time::Duration;
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
            Op::Action(oer_chips_family_drivers_watchdog::RtcStageAction::ResetRtc),
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
        (
            InitError::DebugStatusPublished,
            vec![Op::Cpu, Op::Console, Op::Clocks, Op::Cores, Op::Debug],
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
fn the_adoption_gives_the_integer_rate_and_fails_an_unsupported_cpu_clock() {
    struct Clock(CpuRate);
    impl CpuClock for Clock {
        fn sample(self) -> CpuRate {
            self.0
        }
    }
    let rate = |hz| CpuHz::new(hz).expect("a test rate is nonzero");
    for hz in [40_000_000, 160_000_000, 320_000_000] {
        assert_eq!(
            adopt_cpu_clock(Clock(CpuRate::Integer(rate(hz)))),
            Ok(rate(hz))
        );
    }
    for (root, fractional) in [(0, true), (1, true), (2, false), (3, false), (2, true)] {
        let failure = adopt_cpu_clock(Clock(CpuRate::Unsupported { root, fractional }));
        assert_eq!(
            failure,
            Err(crate::BootFailure::Clock(
                crate::ClockFailure::UnsupportedCpuClock { root, fractional }
            ))
        );
        assert_eq!(
            failure.map(CpuHz::get).unwrap_err().reason(),
            "cpu-clock-unsupported"
        );
    }
    assert_eq!(CpuHz::new(0), None);
}

#[test]
fn an_unsupported_cpu_clock_stops_the_adoption_before_the_console() {
    let log = Log::new(None);
    assert_eq!(
        initialize(log.owners()).map(|ready| ready.cpu.get()).ok(),
        Some(160_000_000),
        "the adopted rate reaches the port's output"
    );
    let unsupported = CpuRate::Unsupported {
        root: 2,
        fractional: false,
    };
    let log = Log {
        cpu: unsupported,
        ..Log::new(None)
    };
    let failure = InitError::Boot(crate::BootFailure::Clock(
        crate::ClockFailure::UnsupportedCpuClock {
            root: 2,
            fractional: false,
        },
    ));
    assert!(matches!(initialize(log.owners()), Err(error) if error == failure));
    assert_eq!(
        log.operations(),
        [Op::Cpu],
        "nothing after the clock's sample"
    );
    assert_eq!(failure.reason(), "cpu-clock-unsupported");
}
