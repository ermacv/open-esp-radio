//! The phase timer of a running coexistence schedule.
//!
//! The vendor schedule re-arms one `esp_timer` under its schedule lock: every
//! phase change disarms it and arms it again for the new phase, and its
//! callback runs `coex_schm_timeout_process`. [`CoexScheduleExecutor`] keeps
//! that timer as data. Every phase change yields a [`CoexPhaseTimer`] command
//! for the runtime's timer task and a generation; an expiry that reports an
//! older generation lost a race with a later phase change and does not step
//! the schedule, as the disarmed vendor timer would not have fired.

use crate::{CoexPhaseStep, CoexSchedule, CoexScheduleIdle, CoexStatusType};

/// What the runtime's phase timer does after a phase change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoexPhaseTimer {
    /// Fire once after `micros` and report `generation` to
    /// [`CoexScheduleExecutor::expire`].
    Arm { generation: u32, micros: u32 },
    /// Stay disarmed until the next phase change.
    Disarm,
}

/// One phase change: the step to publish and the timer to program.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexPhaseChange {
    pub step: CoexPhaseStep,
    pub timer: CoexPhaseTimer,
}

/// The result of one phase-timer expiry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoexExpiry {
    /// A later phase change disarmed this expiry; nothing changed.
    Stale,
    /// The last phase stays current; the timer stays disarmed.
    Idle,
    /// The schedule stepped to the next phase.
    Changed(CoexPhaseChange),
}

/// A coexistence schedule with its phase timer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexScheduleExecutor {
    schedule: CoexSchedule,
    generation: u32,
    armed: Option<u32>,
}

impl CoexScheduleExecutor {
    /// `coex_schm_init` with a disarmed timer.
    pub const fn new() -> Self {
        Self {
            schedule: CoexSchedule::new(),
            generation: 0,
            armed: None,
        }
    }

    pub const fn schedule(&self) -> &CoexSchedule {
        &self.schedule
    }

    /// Whether an expiry of the current generation is pending.
    pub const fn armed(&self) -> bool {
        self.armed.is_some()
    }

    /// Publish status bits ([`CoexSchedule::set_status_bits`]); a returned
    /// change is the restart the change caused.
    pub fn set_status_bits(&mut self, kind: CoexStatusType, bits: u16) -> Option<CoexPhaseChange> {
        let step = self.schedule.set_status_bits(kind, bits)?;
        Some(self.change(step))
    }

    /// Withdraw status bits ([`CoexSchedule::clear_status_bits`]); the phase
    /// and the timer stay as they are.
    pub fn clear_status_bits(&mut self, kind: CoexStatusType, bits: u16) {
        self.schedule.clear_status_bits(kind, bits);
    }

    /// [`CoexSchedule::set_interval`]; it takes effect at the next phase.
    pub fn set_interval(&mut self, interval: u32) {
        self.schedule.set_interval(interval);
    }

    /// [`CoexSchedule::set_flexible_period`].
    pub fn set_flexible_period(&mut self, period: u8) {
        self.schedule.set_flexible_period(period);
    }

    /// Begin the phases again at phase 0 ([`CoexSchedule::restart`]).
    pub fn restart(&mut self) -> CoexPhaseChange {
        match self.schedule.restart() {
            Ok(step) => self.change(step),
            Err(CoexScheduleIdle::LastPhase) => unreachable!("a restart always takes a step"),
        }
    }

    /// The phase timer of `generation` fired.
    pub fn expire(&mut self, generation: u32) -> CoexExpiry {
        if self.armed != Some(generation) {
            return CoexExpiry::Stale;
        }
        self.armed = None;
        match self.schedule.timeout() {
            Ok(step) => CoexExpiry::Changed(self.change(step)),
            Err(CoexScheduleIdle::LastPhase) => CoexExpiry::Idle,
        }
    }

    fn change(&mut self, step: CoexPhaseStep) -> CoexPhaseChange {
        // Every phase change disarms the timer before it may arm it again.
        self.generation = self.generation.wrapping_add(1);
        let timer = match step.timer_micros {
            Some(micros) => {
                self.armed = Some(self.generation);
                CoexPhaseTimer::Arm {
                    generation: self.generation,
                    micros,
                }
            }
            None => {
                self.armed = None;
                CoexPhaseTimer::Disarm
            }
        };
        CoexPhaseChange { step, timer }
    }
}

impl Default for CoexScheduleExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
