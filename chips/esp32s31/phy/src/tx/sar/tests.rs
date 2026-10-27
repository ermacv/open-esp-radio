use super::*;
use oer_time_virtual::VirtualClock;
use std::{cell::RefCell, vec::Vec};

struct Hardware<'a> {
    clock: &'a VirtualClock,
    ready_at: Option<Instant>,
    events: RefCell<Vec<(&'static str, Instant)>>,
    io_delay: u64,
}
impl Hardware<'_> {
    fn record(&self, name: &'static str) {
        self.events.borrow_mut().push((name, self.clock.now()));
    }
}
impl Io for Hardware<'_> {
    fn arm_tone(&mut self) {
        self.record("arm");
        self.clock
            .advance(Duration::from_micros(self.io_delay))
            .unwrap();
    }
    fn trigger(&mut self) {
        self.record("trigger");
        self.clock
            .advance(Duration::from_micros(self.io_delay))
            .unwrap();
    }
    fn ready(&mut self) -> bool {
        self.record("ready");
        self.ready_at.is_some_and(|at| self.clock.now() >= at)
    }
    fn clear_tone(&mut self) {
        self.record("clear");
    }
    fn sample(&mut self) -> u16 {
        self.record("sample");
        73
    }
}
fn hardware(clock: &VirtualClock, ready_at: Option<u64>) -> Hardware<'_> {
    Hardware {
        clock,
        ready_at: ready_at.map(Instant::from_micros),
        events: RefCell::new(Vec::new()),
        io_delay: 0,
    }
}
#[test]
fn vendor_minimum_waits_are_measured_from_the_actual_edge() {
    let clock = VirtualClock::starting_at(Instant::EPOCH);
    let mut io = hardware(&clock, Some(0));
    let mut engine = Engine::new(true);
    assert_eq!(
        engine.advance(&clock, &mut io),
        Event::WaitAt(Instant::from_micros(1))
    );
    // Early/duplicate events do not consume a minimum wait or issue I/O.
    assert_eq!(
        engine.advance(&clock, &mut io),
        Event::WaitAt(Instant::from_micros(1))
    );
    clock.advance(Duration::from_micros(20)).unwrap();
    assert_eq!(
        engine.advance(&clock, &mut io),
        Event::WaitAt(Instant::from_micros(22))
    );
    clock.advance(Duration::from_micros(1)).unwrap();
    assert_eq!(
        engine.advance(&clock, &mut io),
        Event::WaitAt(Instant::from_micros(22))
    );
    assert_eq!(io.events.borrow().len(), 2);
    clock.advance(Duration::from_micros(1)).unwrap();
    let done = engine.advance(&clock, &mut io);
    assert_eq!(
        done,
        Event::Complete(Ok(Sample {
            value: 73,
            readiness: Readiness {
                elapsed_micros: 0,
                observations: 1,
                ready: true,
            }
        }))
    );
    assert_eq!(
        io.events
            .borrow()
            .iter()
            .map(|(s, _)| *s)
            .collect::<Vec<_>>(),
        ["arm", "trigger", "ready", "clear", "sample"]
    );
    assert_eq!(engine.advance(&clock, &mut io), done);
    assert_eq!(io.events.borrow().len(), 5);
}
#[test]
fn delayed_ready_sample_succeeds_and_delayed_unready_sample_consumes_no_result() {
    for ready in [true, false] {
        let clock = VirtualClock::starting_at(Instant::EPOCH);
        let mut io = hardware(&clock, ready.then_some(50));
        let mut engine = Engine::new(false);
        engine.advance(&clock, &mut io);
        clock.advance(Duration::from_micros(1)).unwrap();
        engine.advance(&clock, &mut io);
        clock.advance(Duration::from_micros(100)).unwrap();
        let readiness = Readiness {
            elapsed_micros: 98,
            observations: 1,
            ready,
        };
        let result = if ready {
            Ok(Sample {
                value: 73,
                readiness,
            })
        } else {
            Err(Error::NotReady(readiness))
        };
        assert_eq!(engine.advance(&clock, &mut io), Event::Complete(result));
        assert_eq!(io.events.borrow().len(), if ready { 4 } else { 3 });
    }
}
#[test]
fn not_ready_rearms_at_one_microsecond_and_stops_at_the_limit() {
    let clock = VirtualClock::starting_at(Instant::EPOCH);
    let mut io = hardware(&clock, None);
    let mut engine = Engine::new(true);
    let result = loop {
        match engine.advance(&clock, &mut io) {
            Event::WaitAt(at) => {
                clock
                    .advance(at.saturating_duration_since(clock.now()))
                    .unwrap();
            }
            Event::Complete(result) => break result,
        }
    };
    assert_eq!(
        result,
        Err(Error::NotReady(Readiness {
            elapsed_micros: 40,
            observations: 41,
            ready: false
        }))
    );
    assert!(
        !io.events
            .borrow()
            .iter()
            .any(|(name, _)| *name == "sample" || *name == "clear")
    );
}
#[test]
fn deadline_overflow_issues_no_edge() {
    let clock = VirtualClock::starting_at(Instant::from_micros(u64::MAX));
    let mut io = hardware(&clock, Some(0));
    assert_eq!(
        Engine::new(true).advance(&clock, &mut io),
        Event::Complete(Err(Error::DeadlineOverflow))
    );
    assert!(io.events.borrow().is_empty());
}

#[test]
fn minimum_wait_starts_after_the_mmio_edge_finishes() {
    let clock = VirtualClock::starting_at(Instant::EPOCH);
    let mut io = hardware(&clock, Some(0));
    io.io_delay = 5;
    let mut engine = Engine::new(false);
    assert_eq!(
        engine.advance(&clock, &mut io),
        Event::WaitAt(Instant::from_micros(6))
    );
    clock.advance(Duration::from_micros(1)).unwrap();
    assert_eq!(
        engine.advance(&clock, &mut io),
        Event::WaitAt(Instant::from_micros(13))
    );
    assert_eq!(clock.now(), Instant::from_micros(11));
}
#[test]
fn ready_exactly_at_the_limit_succeeds() {
    let clock = VirtualClock::starting_at(Instant::EPOCH);
    let mut io = hardware(&clock, Some(43));
    let mut engine = Engine::new(false);
    loop {
        match engine.advance(&clock, &mut io) {
            Event::WaitAt(at) => {
                clock
                    .advance(at.saturating_duration_since(clock.now()))
                    .unwrap();
            }
            Event::Complete(result) => {
                assert_eq!(
                    result.unwrap().readiness,
                    Readiness {
                        elapsed_micros: 40,
                        observations: 41,
                        ready: true
                    }
                );
                break;
            }
        }
    }
}
