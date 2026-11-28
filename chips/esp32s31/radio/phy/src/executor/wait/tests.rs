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

    use crate::executor::test_clock::VirtualClock;
    use oer_portable_clock::Clock;
    use oer_portable_time::{Duration, Instant};

    use super::super::{Event, Kind, PhyShortDelay, delay, delay_observed};

    std::thread_local! {
        static SETTLED: Cell<u32> = const { Cell::new(0) };
    }

    struct Short;

    impl PhyShortDelay for Short {
        const MAX_MICROS: u32 = 20;

        fn settle<K: oer_portable_settle::SettleCounter + ?Sized>(
            _: &K,
            limit: oer_portable_settle::SettleLimit,
        ) {
            SETTLED.with(|settled| settled.set(settled.get() + limit.micros()));
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
            Poll::Ready(Ok(()))
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
            assert_eq!(poll(wait.as_mut()), Poll::Ready(Ok(())));
            assert_eq!(SETTLED.with(Cell::get), 0);
        }
    }

    #[test]
    fn a_deadline_past_the_timer_range_is_refused() {
        let clock: VirtualClock = VirtualClock::starting_at(Instant::from_micros(u64::MAX - 1));
        let mut wait = pin!(delay::<Short, _>(&clock, Kind::Completion, 2));
        assert_eq!(
            poll(wait.as_mut()),
            Poll::Ready(Err(super::super::PhyWaitRefused::Deadline))
        );
    }

    #[test]
    fn a_refused_wait_is_reported_as_refused() {
        let clock: VirtualClock = VirtualClock::starting_at(Instant::from_micros(u64::MAX - 1));
        let mut events = Vec::new();
        {
            let wait = pin!(delay_observed::<Short, _>(
                &clock,
                Kind::Completion,
                2,
                true,
                |event| events.push(event),
            ));
            assert_eq!(
                poll(wait),
                Poll::Ready(Err(super::super::PhyWaitRefused::Deadline))
            );
        }
        assert_eq!(
            events,
            [
                Event::Started {
                    requested_micros: 2
                },
                Event::Refused
            ]
        );
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
            assert_eq!(poll(wait.as_mut()), Poll::Ready(Ok(())));
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
            Poll::Ready(Ok(()))
        );
        assert_eq!(observed, 0);
    }

    /// A tally that keeps every wait it is told of.
    #[derive(Default)]
    struct Kept(
        core::cell::RefCell<Vec<(super::super::Waited, u64, u64)>>,
        core::cell::RefCell<Vec<(super::super::Polled, u32, bool, u64)>>,
    );

    impl super::super::WaitTally for Kept {
        const COUNTS: bool = true;
        fn waited(&self, kind: super::super::Waited, asked: u64, taken: u64) {
            self.0.borrow_mut().push((kind, asked, taken));
        }
        fn polled(&self, kind: super::super::Polled, reads: u32, ready: bool, taken: u64) {
            self.1.borrow_mut().push((kind, reads, ready, taken));
        }
    }

    #[test]
    fn a_counted_timer_names_each_wait_with_what_it_asked_and_took() {
        use super::super::{PhyTimer, Waited};
        let clock: VirtualClock = VirtualClock::starting_at(Instant::from_micros(100));
        let kept = Kept::default();
        let timer = PhyTimer::counted(&clock, &clock, &kept);
        assert_eq!(
            poll(pin!(delay::<Short, _>(&timer, Kind::Settle, 20))),
            Poll::Ready(Ok(()))
        );
        for (kind, micros, late) in [
            (Kind::Settle, 21, 4),
            (Kind::Completion, 1, 2),
            (Kind::BusBusy, 2, 0),
        ] {
            let mut wait = pin!(delay::<Short, _>(&timer, kind, micros));
            assert_eq!(poll(wait.as_mut()), Poll::Pending);
            clock.advance(Duration::from_micros(micros + late)).unwrap();
            assert_eq!(poll(wait.as_mut()), Poll::Ready(Ok(())));
        }
        assert_eq!(
            *kept.0.borrow(),
            [
                (Waited::SettleSpun, 20, 0),
                (Waited::SettleTimed, 21, 25),
                (Waited::Completion, 1, 3),
                (Waited::BusBusy, 2, 2),
            ]
        );
    }

    #[test]
    fn a_refused_wait_is_not_counted() {
        let clock: VirtualClock = VirtualClock::starting_at(Instant::from_micros(u64::MAX - 1));
        let kept = Kept::default();
        let timer = super::super::PhyTimer::counted(&clock, &clock, &kept);
        assert_eq!(
            poll(pin!(delay::<Short, _>(&timer, Kind::Completion, 2))),
            Poll::Ready(Err(super::super::PhyWaitRefused::Deadline))
        );
        assert!(kept.0.borrow().is_empty());
    }

    #[test]
    fn a_counted_poll_names_its_kind_reads_readiness_and_time() {
        use super::super::{PhyTimer, Polled, polled};
        let clock: VirtualClock = VirtualClock::starting_at(Instant::from_micros(100));
        let kept = Kept::default();
        let timer = PhyTimer::counted(&clock, &clock, &kept);
        let result = polled(
            &timer,
            Polled::PwdetReady,
            || {
                clock.advance(Duration::from_micros(7)).unwrap();
                5_u32
            },
            |reads| Some((*reads, true)),
        );
        assert_eq!(result, 5, "the poll's own result comes back unchanged");
        let unnamed = polled(&timer, Polled::DcIqReadiness, || 9_u32, |_| None);
        assert_eq!(unnamed, 9);
        polled(&timer, Polled::ToneSarReady, || (), |()| Some((1, false)));
        assert_eq!(
            *kept.1.borrow(),
            [
                (Polled::PwdetReady, 5, true, 7),
                (Polled::ToneSarReady, 1, false, 0),
            ],
            "a poll whose result names no reads is not counted"
        );
        assert!(kept.0.borrow().is_empty(), "a poll is not a wait");
    }

    /// A clock whose every read is counted, under a tally that does not count.
    struct Watched(Cell<u32>);

    impl Clock for Watched {
        fn now(&self) -> Instant {
            self.0.set(self.0.get() + 1);
            Instant::EPOCH
        }
    }

    impl super::super::WaitTally for Watched {
        const COUNTS: bool = false;
        fn waited(&self, _: super::super::Waited, _: u64, _: u64) {}
        fn polled(&self, _: super::super::Polled, _: u32, _: bool, _: u64) {}
    }

    #[test]
    fn an_uncounted_poll_reads_no_clock() {
        use super::super::{Polled, polled};
        let clock = Watched(Cell::new(0));
        let mut seen = false;
        let result = polled(
            &clock,
            Polled::DcIqReadiness,
            || 3_u8,
            |_| {
                seen = true;
                Some((1, true))
            },
        );
        assert_eq!(result, 3);
        assert_eq!(clock.0.get(), 0);
        assert!(!seen, "an uncounted poll does not ask what it read");
    }
}
