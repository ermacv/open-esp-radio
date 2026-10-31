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
//! ends supervision with [`LivenessError::Overdue`]; the supervisor's client
//! stays quarantined (an echo taken at or after the deadline is never
//! counted, and every later round reports the same overdue beat), and the
//! image records it ([`LivenessError::diagnostic`]) and resets.
//!
//! What a completed round proves: the responder's hart ran its executor,
//! the responder's task was polled, and both signals crossed the harts,
//! before the deadline as the supervisor's clock read it after taking the
//! echo. It proves nothing about the health of other drivers
//! or tasks on that hart. When the supervisor's own executor stops, nothing
//! supervises; the RTC watchdog its executor feeds is then the only bound.
//! The period and the deadline are the image's stated configuration
//! ([`Period`]), not measured latencies.

use core::{
    future::{Future, poll_fn},
    pin::pin,
    task::Poll,
};

use oer_espressif_interrupt_table::signal::{ForeignContext, IrqSignal, SignalExecutor};
use oer_espressif_staged_runtime::panic::FaultReason;
use oer_memory::{InPlace, RequestSlot};
use oer_time::{Duration, Instant, TimeOverflow, Timer};

use super::obligation::{Obligations, Scan, Watch};
use super::{PeerClient, PeerError, PeerServer, PostError};

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

/// A watched channel's request whose deadline passed before its client took
/// the answer: never waited for, abandoned, expired or answered late. The
/// seat cannot tell a forgotten wait from a slow server.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RequestOverdue {
    /// The channel's seat ([`Watch::channel`]).
    pub channel: u32,
    /// The request's deadline.
    pub deadline: Instant,
}

/// Why supervision ended. Every variant is terminal for the image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LivenessError {
    /// The responder did not echo a beat by its deadline.
    Overdue(Overdue),
    /// A watched channel's request passed its deadline unanswered.
    RequestOverdue(RequestOverdue),
    /// The supervisor was polled outside its executor's context.
    Foreign(ForeignContext),
    /// The slot still held an earlier beat when the next was posted.
    Occupied {
        /// The beat that could not be posted.
        sequence: u32,
    },
    /// The supervisor waited with no beat posted.
    NotPosted {
        /// The beat it waited for.
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
    /// echo, 3 time range, 4 no beat posted), the sequence and the echo;
    /// `PeerUnanswered` for a watched request, with its channel, its
    /// deadline in microseconds (low, high word) and 2, the supervisor's
    /// mark (a client's own expiry records 0).
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
            Self::RequestOverdue(RequestOverdue { channel, deadline }) => {
                let micros = deadline.as_micros();
                (
                    FaultReason::PeerUnanswered,
                    [channel, micros as u32, (micros >> 32) as u32, 2],
                )
            }
            Self::Foreign(_) => (FaultReason::SignalContext, [0, 0, 0, 0]),
            Self::Occupied { sequence } => (FaultReason::LivenessBroken, [1, sequence, 0, 0]),
            Self::WrongEcho { sequence, echo } => {
                (FaultReason::LivenessBroken, [2, sequence, echo, 0])
            }
            Self::Time(TimeOverflow) => (FaultReason::LivenessBroken, [3, 0, 0, 0]),
            Self::NotPosted { sequence } => (FaultReason::LivenessBroken, [4, sequence, 0, 0]),
        }
    }
}

/// The supervising side, a task of executor `C` on one hart; it supervises
/// the responder, a task of executor `S` on another, and the `N` watched
/// channels of executor `C`'s clients.
#[must_use = "dropping the supervisor ends supervision and retains the seat"]
pub struct Supervisor<S: SignalExecutor, C: SignalExecutor, const N: usize> {
    client: PeerClient<Beat, S, C>,
    obligations: &'static Obligations<N>,
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
/// `request` signal, the supervisor executor's `reply` signal, the stated
/// `period` and the obligation table of executor `C`'s watched channels;
/// `None` when the slot or the table was claimed before.
pub fn pair<S: SignalExecutor, C: SignalExecutor, const N: usize>(
    slot: &'static RequestSlot<Beat, (), InPlace>,
    request: &'static IrqSignal<S>,
    reply: &'static IrqSignal<C>,
    period: Period,
    obligations: &'static Obligations<N>,
) -> Option<(Supervisor<S, C, N>, Responder<S, C>)> {
    if !obligations.claim() {
        return None;
    }
    let (client, server) = super::claim(slot, request, reply, None)?;
    Some((
        Supervisor {
            client,
            obligations,
            period,
            sequence: 0,
            completed: 0,
        },
        Responder { server },
    ))
}

impl<S: SignalExecutor, C: SignalExecutor, const N: usize> Supervisor<S, C, N> {
    /// Issue the next seat of this supervisor's table, for one channel of
    /// executor `C` ([`super::pair`]); `None` once all `N` are issued.
    pub fn watch(&self) -> Option<Watch<C>> {
        self.obligations.issue()
    }

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
        match self.client.post(Beat { sequence, echo: 0 }, deadline) {
            Ok(()) => {}
            Err(PostError::Occupied(_)) => return Err(LivenessError::Occupied { sequence }),
            // The beat that expired earlier stays overdue.
            Err(PostError::Expired { deadline, .. }) => {
                return Err(LivenessError::Overdue(Overdue {
                    sequence: self.sequence,
                    deadline,
                    completed: self.completed,
                }));
            }
        }
        self.sequence = sequence;
        let obligations = self.obligations;
        let beat = match watched(obligations, timer, self.client.answer(timer)).await? {
            Ok(beat) => beat,
            Err(PeerError::Expired { deadline }) => {
                return Err(LivenessError::Overdue(Overdue {
                    sequence,
                    deadline,
                    completed: self.completed,
                }));
            }
            Err(PeerError::NotPosted) => return Err(LivenessError::NotPosted { sequence }),
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
            if let Err(error) = watched(self.obligations, timer, timer.wait_until(next)).await {
                return error;
            }
        }
    }
}

/// Drive `inner` to its end while watching `obligations`: a watched request
/// whose deadline the clock reaches first ends the wait with
/// [`LivenessError::RequestOverdue`]. The task wakes at the earliest
/// watched deadline and at every write to a seat, so a newly posted request
/// with an earlier deadline is watched from its post on.
async fn watched<T: Timer, F: Future, const N: usize>(
    obligations: &Obligations<N>,
    timer: &T,
    inner: F,
) -> Result<F::Output, LivenessError> {
    let mut inner = pin!(inner);
    loop {
        // Registered before the scan, so a write after it wakes the task.
        poll_fn(|cx| {
            obligations.register(cx);
            Poll::Ready(())
        })
        .await;
        let _changed = obligations.take_changed();
        let earliest = match obligations.scan(timer.now()) {
            Scan::Overdue { channel, deadline } => {
                return Err(LivenessError::RequestOverdue(RequestOverdue {
                    channel,
                    deadline,
                }));
            }
            Scan::Pending(earliest) => earliest,
        };
        let mut due = pin!(async {
            match earliest {
                Some(deadline) => timer.wait_until(deadline).await,
                None => core::future::pending::<()>().await,
            }
        });
        let finished = poll_fn(|cx| {
            obligations.register(cx);
            if let Poll::Ready(output) = inner.as_mut().poll(cx) {
                return Poll::Ready(Some(output));
            }
            if obligations.take_changed() || due.as_mut().poll(cx).is_ready() {
                return Poll::Ready(None);
            }
            Poll::Pending
        })
        .await;
        if let Some(output) = finished {
            return Ok(output);
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
