//! Two harts read the one latch at once: their register accesses interleave
//! one by one, in every order a schedule picks, unlike a preempting context,
//! whose whole read lands between two accesses of the read it preempts.
//!
//! Each hart is a thread whose every access waits for its turn; a seeded
//! schedule names the hart of each global step. The model latch counts one
//! stride per step and copies the count at the strobe; a copy completes some
//! steps later, in strobe order, and VALID reads true only while no copy is
//! in flight. One variant queues every strobe, the other absorbs a strobe
//! that arrives while a copy is in flight into that copy.

use std::{
    sync::{Condvar, Mutex, MutexGuard},
    thread,
    vec::Vec,
};

use super::super::{CounterLatch, Unverified, read};

/// Which hart takes each global step.
#[derive(Clone, Copy, Debug)]
enum Schedule {
    /// The high bit of a seeded LCG.
    Seeded(u64),
    /// The harts take turns, one access each.
    Alternating,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Strobes {
    /// Every strobe starts its own copy.
    Queued,
    /// A strobe while a copy is in flight joins that copy.
    Absorbed,
}

/// One read as a hart saw it: the global steps around it and its result.
#[derive(Clone, Copy, Debug)]
struct Reading {
    hart: usize,
    start: u64,
    end: u64,
    result: Result<u64, Unverified>,
}

impl Reading {
    fn value(&self) -> u64 {
        self.result.unwrap_or_else(|unverified| unverified.floor)
    }
}

struct Sim {
    step: u64,
    base: u64,
    stride: u64,
    settle: u64,
    strobes: Strobes,
    /// Copies in flight: the count each copies and the step it completes at.
    in_flight: Vec<(u64, u64)>,
    latched: u64,
    /// Every completed copy: the step it completed at and its count.
    completed: Vec<(u64, u64)>,
    schedule: Schedule,
    turn: usize,
    done: [bool; 2],
    readings: Vec<Reading>,
}

impl Sim {
    fn count(&self) -> u64 {
        self.base + self.step * self.stride
    }

    /// One register access: the step advances, due copies complete in order.
    fn advance(&mut self) {
        self.step += 1;
        while let Some(&(count, at)) = self.in_flight.first() {
            if at > self.step {
                break;
            }
            self.in_flight.remove(0);
            self.latched = count;
            self.completed.push((self.step, count));
        }
    }

    fn strobe(&mut self) {
        if self.strobes == Strobes::Absorbed && !self.in_flight.is_empty() {
            return;
        }
        let at = self.step + self.settle;
        self.in_flight.push((self.count(), at));
    }

    /// The next step's hart.
    fn choose(&mut self) {
        self.turn = match &mut self.schedule {
            Schedule::Seeded(seed) => {
                *seed = seed
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                usize::from(*seed >> 63 == 1)
            }
            Schedule::Alternating => 1 - self.turn,
        };
    }
}

struct Shared {
    sim: Mutex<Sim>,
    turn: Condvar,
}

struct Hart<'a> {
    shared: &'a Shared,
    hart: usize,
}

impl Hart<'_> {
    /// Wait for this hart's turn (or the other hart's end), access, pass on.
    fn access<R>(&self, op: impl FnOnce(&mut Sim) -> R) -> R {
        let mut sim = self.lock();
        while sim.turn != self.hart && !sim.done[1 - self.hart] {
            sim = self.shared.turn.wait(sim).expect("the schedule lock");
        }
        sim.advance();
        let result = op(&mut sim);
        sim.choose();
        self.shared.turn.notify_all();
        result
    }

    fn lock(&self) -> MutexGuard<'_, Sim> {
        self.shared.sim.lock().expect("the schedule lock")
    }

    fn run(&self, reads: usize) {
        for _ in 0..reads {
            let start = self.lock().step;
            let result = read(self);
            let mut sim = self.lock();
            let end = sim.step;
            sim.readings.push(Reading {
                hart: self.hart,
                start,
                end,
                result,
            });
        }
        let mut sim = self.lock();
        sim.done[self.hart] = true;
        self.shared.turn.notify_all();
    }
}

impl CounterLatch for Hart<'_> {
    fn update(&self) {
        self.access(Sim::strobe);
    }
    fn valid(&self) -> bool {
        self.access(|sim| sim.in_flight.is_empty())
    }
    fn high(&self) -> u32 {
        self.access(|sim| (sim.latched >> 32) as u32)
    }
    fn low(&self) -> u32 {
        self.access(|sim| sim.latched as u32)
    }
}

/// Run two harts of `reads` reads each under `schedule`.
fn run(schedule: Schedule, base: u64, settle: u64, strobes: Strobes, reads: usize) -> Sim {
    let shared = Shared {
        sim: Mutex::new(Sim {
            step: 0,
            base,
            stride: 3,
            settle,
            strobes,
            in_flight: Vec::new(),
            latched: 0,
            completed: Vec::new(),
            schedule,
            turn: 0,
            done: [false; 2],
            readings: Vec::new(),
        }),
        turn: Condvar::new(),
    };
    shared.sim.lock().expect("the schedule lock").choose();
    thread::scope(|scope| {
        for hart in 0..2 {
            let shared = &shared;
            scope.spawn(move || Hart { shared, hart }.run(reads));
        }
    });
    shared.sim.into_inner().expect("the schedule lock")
}

/// The guarantees of one schedule's readings.
fn check(sim: &Sim, case: &str) {
    for reading in &sim.readings {
        // A verified read is one copy, whole, completed during the read.
        if let Ok(value) = reading.result {
            assert!(
                sim.completed
                    .iter()
                    .any(|&(at, count)| count == value && at > reading.start && at <= reading.end),
                "{case}: {reading:?} is no copy completed during the read"
            );
        }
        // A floor lies between the counts the latch held around the read.
        let value = reading.value();
        assert!(
            value <= sim.base + reading.end * sim.stride,
            "{case}: {reading:?} lies past the count at its end"
        );
    }
    // A read that starts after another returned, on either hart, never
    // returns less.
    for earlier in &sim.readings {
        for later in &sim.readings {
            if earlier.end <= later.start {
                assert!(
                    earlier.value() <= later.value(),
                    "{case}: {earlier:?} then {later:?} ran backward"
                );
            }
        }
    }
}

#[test]
fn reads_on_two_harts_interleaved_step_by_step_stay_whole_and_monotonic() {
    let mut verified = [0_usize; 2];
    let mut unverified = 0_usize;
    for strobes in [Strobes::Queued, Strobes::Absorbed] {
        // Below a 2^32 boundary, so HI and LO of different copies mix.
        for base in [0xffff_ff00, 0x7_ffff_ff80] {
            for settle in 1..=3 {
                for seed in 0..64 {
                    let sim = run(Schedule::Seeded(seed), base, settle, strobes, 4);
                    let case = std::format!("{strobes:?}/{base:#x}/{settle}/{seed}");
                    assert_eq!(sim.readings.len(), 8, "{case}");
                    check(&sim, &case);
                    for reading in &sim.readings {
                        match reading.result {
                            Ok(_) => verified[reading.hart] += 1,
                            Err(_) => unverified += 1,
                        }
                    }
                }
            }
        }
    }
    // The schedules interleave both harts' reads; each hart verifies most.
    assert!(verified.iter().all(|&count| count > 1000), "{verified:?}");
    assert!(unverified < verified[0] + verified[1], "{unverified}");
}

#[test]
fn a_hart_whose_peer_strobes_at_every_turn_returns_a_monotonic_floor() {
    // Strict alternation with an instant copy: the peer's strobe lands
    // between every pair of this hart's accesses. Its reads may exhaust
    // their rounds; each result still keeps the guarantees.
    for strobes in [Strobes::Queued, Strobes::Absorbed] {
        let sim = run(Schedule::Alternating, 0xffff_fff0, 1, strobes, 6);
        assert_eq!(sim.readings.len(), 12);
        check(&sim, &std::format!("alternating/{strobes:?}"));
    }
}
