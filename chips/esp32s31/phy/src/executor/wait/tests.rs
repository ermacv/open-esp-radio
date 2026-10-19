use super::*;

fn completed(recorder: &mut Recorder, scope: Scope, kind: Kind, requested: u64, elapsed: u64) {
    recorder.observe(
        scope,
        kind,
        Event::Started {
            requested_micros: requested,
        },
    );
    recorder.observe(
        scope,
        kind,
        Event::Completed {
            elapsed_micros: elapsed,
            lateness_micros: elapsed - requested,
        },
    );
}

#[test]
fn direct_i2c_nested_i2c_and_settling_are_separate() {
    let mut recorder = Recorder::default();
    completed(&mut recorder, Scope::Dcode, Kind::Completion, 1, 3);
    completed(&mut recorder, Scope::Rfpll, Kind::BusBusy, 1, 5);
    completed(&mut recorder, Scope::Rfpll, Kind::Completion, 1, 7);
    completed(&mut recorder, Scope::Rfpll, Kind::Settle, 20, 35);
    recorder.pll_lock(false);
    recorder.pll_lock(true);
    assert!(recorder.is_complete());
    let report = recorder.report();
    assert_eq!(report.i2c.timing.count, 1);
    assert_eq!(report.i2c.timing.elapsed_micros, 3);
    assert_eq!(report.rfpll_i2c.bus_busy, 1);
    assert_eq!(report.rfpll_i2c.timing.count, 2);
    assert_eq!(report.rfpll_i2c.timing.requested_micros, 2);
    assert_eq!(report.rfpll_i2c.timing.elapsed_micros, 12);
    assert_eq!(report.rfpll_i2c.timing.maximum_lateness_micros, 6);
    assert_eq!(
        report.rfpll_settle,
        Timing {
            count: 1,
            requested_micros: 20,
            elapsed_micros: 35,
            maximum_lateness_micros: 15,
        }
    );
    assert_eq!((report.pll_locked, report.pll_unlocked), (1, 1));
}

#[test]
fn cancellation_and_inconsistent_deadlines_are_incomplete() {
    for events in [
        std::vec![Event::Started {
            requested_micros: 1
        }],
        std::vec![Event::Completed {
            elapsed_micros: 5,
            lateness_micros: 4
        }],
        std::vec![
            Event::Started {
                requested_micros: 10
            },
            Event::Completed {
                elapsed_micros: 9,
                lateness_micros: 0
            }
        ],
        std::vec![
            Event::Started {
                requested_micros: 1
            },
            Event::Completed {
                elapsed_micros: 5,
                lateness_micros: 5
            }
        ],
    ] {
        let mut recorder = Recorder::default();
        for event in events {
            recorder.observe(Scope::Dcode, Kind::Completion, event);
        }
        assert!(!recorder.is_complete());
    }
}

#[test]
fn scopes_cannot_complete_each_others_waits() {
    let mut recorder = Recorder::default();
    recorder.observe(
        Scope::Dcode,
        Kind::Completion,
        Event::Started {
            requested_micros: 1,
        },
    );
    recorder.observe(
        Scope::Rfpll,
        Kind::Completion,
        Event::Completed {
            elapsed_micros: 2,
            lateness_micros: 1,
        },
    );
    assert!(!recorder.is_complete());
}

#[test]
fn count_overflow_invalidates_evidence_without_wrapping() {
    let mut recorder = Recorder::default();
    for _ in 0..=u16::MAX {
        completed(&mut recorder, Scope::Rfpll, Kind::Settle, 1, 1);
    }
    assert!(!recorder.is_complete());
    assert_eq!(recorder.report().rfpll_settle.count, u16::MAX);
    assert_eq!(
        recorder.report().rfpll_settle.elapsed_micros,
        u32::from(u16::MAX)
    );
    let mut recorder = Recorder::default();
    for _ in 0..=u16::MAX {
        recorder.pll_lock(false);
    }
    assert!(!recorder.is_complete());
    assert_eq!(recorder.report().pll_unlocked, u16::MAX);
}

mod delays {
    use core::{
        cell::Cell,
        pin::pin,
        task::{Context, Poll, Waker},
    };
    use std::vec::Vec;

    use oer_time::{Clock, Duration, Instant};
    use oer_time_virtual::VirtualClock;

    use super::super::{Event, Kind, PhyShortDelay, delay, delay_observed};

    std::thread_local! {
        static SETTLED: Cell<u32> = const { Cell::new(0) };
    }

    struct Short;

    impl PhyShortDelay for Short {
        const MAX_MICROS: u32 = 20;

        fn settle_micros(micros: u32) -> bool {
            SETTLED.with(|settled| settled.set(settled.get() + micros));
            true
        }
    }

    fn poll<F: core::future::Future>(future: core::pin::Pin<&mut F>) -> Poll<F::Output> {
        future.poll(&mut Context::from_waker(Waker::noop()))
    }

    #[test]
    fn a_short_settle_completes_at_once_without_the_timer() {
        let clock: VirtualClock = VirtualClock::new();
        SETTLED.with(|settled| settled.set(0));
        assert_eq!(
            poll(pin!(delay::<Short, _>(&clock, Kind::Settle, 20))),
            Poll::Ready(())
        );
        assert_eq!(SETTLED.with(Cell::get), 20);
        assert_eq!(clock.next_deadline(), None);
    }

    #[test]
    fn backoff_and_long_settles_wait_on_the_timer_from_the_call() {
        for (kind, micros) in [
            (Kind::Settle, 21),
            (Kind::Completion, 1),
            (Kind::BusBusy, 1),
        ] {
            let clock: VirtualClock = VirtualClock::starting_at(Instant::from_micros(100));
            SETTLED.with(|settled| settled.set(0));
            let mut wait = pin!(delay::<Short, _>(&clock, kind, micros));
            clock.advance(Duration::from_micros(micros - 1)).unwrap();
            assert_eq!(poll(wait.as_mut()), Poll::Pending);
            assert_eq!(
                clock.next_deadline(),
                Some(Instant::from_micros(100 + micros))
            );
            clock.advance(Duration::from_micros(1)).unwrap();
            assert_eq!(poll(wait.as_mut()), Poll::Ready(()));
            assert_eq!(SETTLED.with(Cell::get), 0);
        }
    }

    #[test]
    fn a_deadline_past_the_timer_range_never_completes() {
        let clock: VirtualClock = VirtualClock::starting_at(Instant::from_micros(u64::MAX - 1));
        let mut wait = pin!(delay::<Short, _>(&clock, Kind::Completion, 2));
        assert_eq!(poll(wait.as_mut()), Poll::Pending);
        clock.advance_to(Instant::from_micros(u64::MAX));
        assert_eq!(poll(wait.as_mut()), Poll::Pending);
    }

    #[test]
    fn an_observed_wait_reports_its_start_elapsed_time_and_lateness() {
        let clock: VirtualClock = VirtualClock::starting_at(Instant::from_micros(10));
        let mut events = Vec::new();
        {
            let mut wait = pin!(delay_observed::<Short, _>(
                &clock,
                Kind::Completion,
                5,
                true,
                |event| events.push(event),
            ));
            assert_eq!(poll(wait.as_mut()), Poll::Pending);
            clock.advance(Duration::from_micros(7)).unwrap();
            assert_eq!(poll(wait.as_mut()), Poll::Ready(()));
        }
        assert_eq!(
            events,
            [
                Event::Started {
                    requested_micros: 5
                },
                Event::Completed {
                    elapsed_micros: 7,
                    lateness_micros: 2
                }
            ]
        );
        assert_eq!(clock.now(), Instant::from_micros(17));
    }

    #[test]
    fn a_disabled_observation_reports_nothing() {
        let clock: VirtualClock = VirtualClock::new();
        let mut observed = 0;
        assert_eq!(
            poll(pin!(delay_observed::<Short, _>(
                &clock,
                Kind::Settle,
                1,
                false,
                |_| observed += 1,
            ))),
            Poll::Ready(())
        );
        assert_eq!(observed, 0);
    }
}
