//! Liveness supervision of a peer hart: a periodic request from a task on
//! one hart (the supervisor) to a task on another (the responder), each with
//! its own deadline, over one in-place request slot and the two executors'
//! signals ([`pair`]).
//!
//! The registration is durable. The image declares the slot as a static,
//! which [`pair`] claims once, and declares the supervisor as a task of its
//! own that never ends: it owns the seat and its timer, independently of
//! every other future of the image. Each round posts the next [`Beat`],
//! waits for its echo until the round's deadline on the supervisor
//! executor's timer, and starts the next round one period after this one
//! started (at once when that has passed). A beat not echoed by its deadline
//! ends supervision with [`LivenessError::Overdue`]; the beat stays posted,
//! and the image records it ([`LivenessError::diagnostic`]) and resets.
//!
//! What a completed round proves: the responder's hart ran its executor,
//! the responder's task was polled, and both signals crossed the harts,
//! within the deadline. It proves nothing about the health of other drivers
//! or tasks on that hart. When the supervisor's own executor stops, nothing
//! supervises; the RTC watchdog its executor feeds is then the only bound.
//! The period and the deadline are the image's stated configuration
//! ([`Period`]), not measured latencies.

use oer_espressif_interrupt_table::signal::{ForeignContext, IrqSignal, SignalExecutor};
use oer_espressif_staged_runtime::panic::FaultReason;
use oer_memory::{InPlace, RequestSlot};
use oer_time::{Duration, Instant, TimeOverflow, Timer};

use super::{PeerClient, PeerError, PeerServer};

/// One round's request: its sequence, which the responder echoes.
#[derive(Debug, Eq, PartialEq)]
pub struct Beat {
    sequence: u32,
    echo: u32,
}

/// How often the supervisor asks and how long each answer may take.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Period {
    period: Duration,
    deadline: Duration,
}

/// A refused [`Period`]: the deadline is zero or longer than the period.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeriodError;

impl Period {
    /// A round every `period`, each answered within `deadline`, which is
    /// nonzero and no longer than the period: one beat at a time is posted.
    ///
    /// # Errors
    ///
    /// [`PeriodError`] for a zero deadline or one past the period.
    pub const fn new(period: Duration, deadline: Duration) -> Result<Self, PeriodError> {
        if deadline.as_micros() == 0 || deadline.as_micros() > period.as_micros() {
            return Err(PeriodError);
        }
        Ok(Self { period, deadline })
    }

    /// The time between two rounds' starts.
    pub const fn period(&self) -> Duration {
        self.period
    }

    /// The time each answer may take.
    pub const fn deadline(&self) -> Duration {
        self.deadline
    }
}

/// A completed round.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Round {
    /// The beat's sequence, from 1, wrapping.
    pub sequence: u32,
    /// When the beat was posted.
    pub posted: Instant,
    /// When its echo was taken.
    pub answered: Instant,
}

/// A beat not echoed by its deadline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Overdue {
    /// The overdue beat's sequence.
    pub sequence: u32,
    /// Its deadline.
    pub deadline: Instant,
    /// The rounds completed before it, saturating.
    pub completed: u32,
}

/// Why supervision ended. Every variant is terminal for the image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LivenessError {
    /// The responder did not echo a beat by its deadline.
    Overdue(Overdue),
    /// The supervisor was polled outside its executor's context.
    Foreign(ForeignContext),
    /// The slot still held an earlier beat when the next was posted.
    Occupied {
        /// The beat that could not be posted.
        sequence: u32,
    },
    /// The responder answered with another sequence than the beat's.
    WrongEcho {
        /// The beat's sequence.
        sequence: u32,
        /// The echo it came back with.
        echo: u32,
    },
    /// A deadline or the next round's start lies past representable time.
    Time(TimeOverflow),
}

impl LivenessError {
    /// The retained reason and its four observation words: `LivenessOverdue`
    /// with the beat's sequence, its deadline in microseconds (low, high
    /// word) and the rounds completed before it; `SignalContext` for a
    /// foreign poll; `LivenessBroken` with a kind (1 seat occupied, 2 wrong
    /// echo, 3 time range), the sequence and the echo.
    pub const fn diagnostic(self) -> (FaultReason, [u32; 4]) {
        match self {
            Self::Overdue(Overdue {
                sequence,
                deadline,
                completed,
            }) => {
                let micros = deadline.as_micros();
                (
                    FaultReason::LivenessOverdue,
                    [sequence, micros as u32, (micros >> 32) as u32, completed],
                )
            }
            Self::Foreign(_) => (FaultReason::SignalContext, [0, 0, 0, 0]),
            Self::Occupied { sequence } => (FaultReason::LivenessBroken, [1, sequence, 0, 0]),
            Self::WrongEcho { sequence, echo } => {
                (FaultReason::LivenessBroken, [2, sequence, echo, 0])
            }
            Self::Time(TimeOverflow) => (FaultReason::LivenessBroken, [3, 0, 0, 0]),
        }
    }
}

/// The supervising side, a task of executor `C` on one hart; it supervises
/// the responder, a task of executor `S` on another.
#[must_use = "dropping the supervisor ends supervision and retains the seat"]
pub struct Supervisor<S: SignalExecutor, C: SignalExecutor> {
    client: PeerClient<Beat, S, C>,
    period: Period,
    sequence: u32,
    completed: u32,
}

/// The responding side, a task of executor `S`.
#[must_use = "dropping the responder leaves every later beat overdue"]
pub struct Responder<S: SignalExecutor, C: SignalExecutor> {
    server: PeerServer<Beat, S, C>,
}

/// Claim `slot`'s supervisor and responder, with the responder executor's
/// `request` signal, the supervisor executor's `reply` signal and the
/// stated `period`; `None` when the slot was claimed before.
pub fn pair<S: SignalExecutor, C: SignalExecutor>(
    slot: &'static RequestSlot<Beat, (), InPlace>,
    request: &'static IrqSignal<S>,
    reply: &'static IrqSignal<C>,
    period: Period,
) -> Option<(Supervisor<S, C>, Responder<S, C>)> {
    let (client, server) = super::pair(slot, request, reply)?;
    Some((
        Supervisor {
            client,
            period,
            sequence: 0,
            completed: 0,
        },
        Responder { server },
    ))
}

impl<S: SignalExecutor, C: SignalExecutor> Supervisor<S, C> {
    /// Run one round now: post the next beat and wait for its echo until
    /// the deadline on `timer`.
    ///
    /// # Errors
    ///
    /// [`LivenessError`]; after one the supervisor posts nothing more.
    pub async fn round<T: Timer>(&mut self, timer: &T) -> Result<Round, LivenessError> {
        let sequence = self.sequence.wrapping_add(1);
        let posted = timer.now();
        let deadline = posted
            .checked_add(self.period.deadline)
            .ok_or(LivenessError::Time(TimeOverflow))?;
        self.client
            .post(Beat { sequence, echo: 0 })
            .map_err(|_| LivenessError::Occupied { sequence })?;
        self.sequence = sequence;
        let beat = match self.client.answer(timer, deadline).await {
            Ok(beat) => beat,
            Err(PeerError::Expired) => {
                return Err(LivenessError::Overdue(Overdue {
                    sequence,
                    deadline,
                    completed: self.completed,
                }));
            }
            Err(PeerError::Foreign(error)) => return Err(LivenessError::Foreign(error)),
        };
        if beat.echo != sequence {
            return Err(LivenessError::WrongEcho {
                sequence,
                echo: beat.echo,
            });
        }
        self.completed = self.completed.saturating_add(1);
        Ok(Round {
            sequence,
            posted,
            answered: timer.now(),
        })
    }

    /// Supervise for good: a round every period, each reported to `observe`
    /// when it completes. Returns only why supervision ended.
    pub async fn supervise<T: Timer>(
        mut self,
        timer: &T,
        mut observe: impl FnMut(Round),
    ) -> LivenessError {
        loop {
            let round = match self.round(timer).await {
                Ok(round) => round,
                Err(error) => return error,
            };
            observe(round);
            let Some(next) = round.posted.checked_add(self.period.period) else {
                return LivenessError::Time(TimeOverflow);
            };
            timer.wait_until(next).await;
        }
    }
}

impl<S: SignalExecutor, C: SignalExecutor> Responder<S, C> {
    /// Echo every beat, for good, in the responder executor's context.
    ///
    /// # Errors
    ///
    /// Returns only when polled outside its executor's context.
    pub async fn serve(&mut self) -> ForeignContext {
        self.server.serve(|beat| beat.echo = beat.sequence).await
    }
}

#[cfg(test)]
mod tests;
