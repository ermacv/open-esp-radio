//! One SAR/PWDET conversion, advanced by timer events with short I/O loans.
use oer_time::{Clock, Duration, Instant};

// MEASUREMENT: s31-a run 1792521908966-002e2465, 21,512 conversions,
// 100 forced tracking ticks, post-2-us-settle ready observations <= 20 us.
// Twice that maximum bounds only not-ready samples, never late ready samples.
pub(crate) const READY_LIMIT: Duration = Duration::from_micros(40);
const SAMPLE_INTERVAL: Duration = Duration::from_micros(1);

pub(crate) trait Io {
    fn arm_tone(&mut self);
    fn trigger(&mut self);
    fn ready(&mut self) -> bool;
    fn clear_tone(&mut self);
    fn sample(&mut self) -> u16;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Readiness {
    pub elapsed_micros: u64,
    pub observations: u16,
    pub ready: bool,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Sample {
    pub value: u16,
    pub readiness: Readiness,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    NotReady(Readiness),
    DeadlineOverflow,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    Arm,
    Trigger(Instant),
    Observe {
        at: Instant,
        started: Instant,
        deadline: Instant,
    },
    Complete(Result<Sample, Error>),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Event {
    Wait { at: Instant, delay: Duration },
    Complete(Result<Sample, Error>),
}

/// SOURCE: rev0 ROM `phy_pwdet_tone_start`: tone arm, minimum 1 us,
/// SAR trigger, minimum 2 us, ready observation, optional clear, result read.
/// Each event executes at most one bounded atom. Edge events return a fresh
/// relative minimum interval; the driver arms it without subtracting a clock.
/// HW17 (`research/hw-sar-hold.md`) qualifies retained
/// ready/result through 100 us. A ready sample succeeds even after the limit;
/// an unready sample then fails without consuming invalid data.
pub(crate) struct Engine {
    state: State,
    clear_tone: bool,
    observations: u16,
}
impl Engine {
    pub(crate) const fn new(clear_tone: bool) -> Self {
        Self {
            state: State::Arm,
            clear_tone,
            observations: 0,
        }
    }
    pub(crate) fn advance(&mut self, clock: &impl Clock, io: &mut impl Io) -> Event {
        let now = clock.now();
        match self.state {
            State::Arm => {
                let Some(_) = now.checked_add(SAMPLE_INTERVAL) else {
                    return self.complete(Err(Error::DeadlineOverflow));
                };
                io.arm_tone();
                let Some(at) = clock.now().checked_add(SAMPLE_INTERVAL) else {
                    return self.complete(Err(Error::DeadlineOverflow));
                };
                self.state = State::Trigger(at);
                Event::Wait {
                    at,
                    delay: SAMPLE_INTERVAL,
                }
            }
            State::Trigger(at) if now < at => Event::Wait {
                at,
                delay: Duration::from_micros(
                    at.saturating_duration_since(now)
                        .as_micros()
                        .saturating_add(1),
                ),
            },
            State::Trigger(_) => {
                let Some(at) = now.checked_add(Duration::from_micros(2)) else {
                    return self.complete(Err(Error::DeadlineOverflow));
                };
                let Some(_) = at.checked_add(READY_LIMIT) else {
                    return self.complete(Err(Error::DeadlineOverflow));
                };
                io.trigger();
                let Some(at) = clock.now().checked_add(Duration::from_micros(2)) else {
                    return self.complete(Err(Error::DeadlineOverflow));
                };
                let Some(deadline) = at.checked_add(READY_LIMIT) else {
                    return self.complete(Err(Error::DeadlineOverflow));
                };
                self.state = State::Observe {
                    at,
                    started: at,
                    deadline,
                };
                Event::Wait {
                    at,
                    delay: Duration::from_micros(2),
                }
            }
            State::Observe { at, .. } if now < at => Event::Wait {
                at,
                delay: Duration::from_micros(
                    at.saturating_duration_since(now)
                        .as_micros()
                        .saturating_add(1),
                ),
            },
            State::Observe {
                started, deadline, ..
            } => {
                let ready = io.ready();
                self.observations = self.observations.saturating_add(1);
                let readiness = Readiness {
                    elapsed_micros: clock.now().saturating_duration_since(started).as_micros(),
                    observations: self.observations,
                    ready,
                };
                if ready {
                    if self.clear_tone {
                        io.clear_tone();
                    }
                    let value = io.sample();
                    self.complete(Ok(Sample { value, readiness }))
                } else if clock.now() >= deadline {
                    self.complete(Err(Error::NotReady(readiness)))
                } else {
                    let at = clock.now().saturating_add(SAMPLE_INTERVAL).min(deadline);
                    self.state = State::Observe {
                        at,
                        started,
                        deadline,
                    };
                    Event::Wait {
                        at,
                        delay: SAMPLE_INTERVAL,
                    }
                }
            }
            State::Complete(result) => Event::Complete(result),
        }
    }
    fn complete(&mut self, result: Result<Sample, Error>) -> Event {
        self.state = State::Complete(result);
        Event::Complete(result)
    }
}

#[cfg(target_arch = "riscv32")]
impl Io for oer_esp32s31_hal::domain::transaction::PhyHardware {
    fn arm_tone(&mut self) {
        crate::hardware::arm_phy_power_detector_tone(self);
    }
    fn trigger(&mut self) {
        oer_esp32s31_hal::phy::power_detector::trigger_sar(self);
    }
    fn ready(&mut self) -> bool {
        oer_esp32s31_hal::phy::power_detector::sample_ready(self)
    }
    fn clear_tone(&mut self) {
        crate::hardware::clear_phy_power_detector_tone_arm(self);
    }
    fn sample(&mut self) -> u16 {
        oer_esp32s31_hal::phy::power_detector::sample_sar(self)
    }
}
#[cfg(test)]
mod tests;
