//! A clock that moves only when the test moves it, and only when nothing
//! can happen without it.
//!
//! Every wait of the code under test goes through [`ManualClock::recv`] or
//! [`ManualClock::until`]; the stream's reader reports its events and the
//! descriptors it waits on to the clock ([`Observer`]); the test's writer
//! reports the writes and signals it has still to act on. The driver
//! ([`ManualClock::drive`]) advances time to the deadline of a blocked wait
//! only when nothing but time can move: the waiter has resumed from every
//! notification (it blocked having seen the latest change), no write or
//! signal is pending, and
//! the reader has ended, waits on a full channel, or waits in `poll` with
//! neither descriptor readable (checked by a zero-timeout `poll` of
//! duplicates, so a stop that was not delivered never counts as one).
//! Nothing in a test waits on the host's time: a step is an event, and the
//! number of steps a test allows is its controlled deadline.

use super::super::{Clock, Observer, ReaderEvent, Wait};
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use std::{
    os::fd::{BorrowedFd, OwnedFd},
    sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, atomic::AtomicBool, mpsc},
    time::Duration,
};

/// Whether either descriptor is readable now, without waiting.
fn readable(fds: &[OwnedFd]) -> bool {
    let mut polled: Vec<PollFd<'_>> = fds
        .iter()
        .map(|fd| PollFd::new(fd, PollFlags::IN))
        .collect();
    let zero = Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    matches!(poll(&mut polled, Some(&zero)), Ok(count) if count > 0)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Reader {
    Absent,
    Busy,
    Waiting,
    Sending,
}

#[derive(Debug)]
pub struct State {
    now: u64,
    generation: u64,
    /// A blocked wait: its deadline, and the generation it saw when it
    /// blocked. A change since (a notification it has not yet resumed
    /// from) keeps the system from being quiet.
    waiting: Option<(u64, u64)>,
    reader: Reader,
    written: u64,
    consumed: u64,
    delivered: u64,
    taken: u64,
    /// Duplicates of the reader's source and stop pipe while it runs.
    fds: Vec<OwnedFd>,
    /// The channel's receiving end is gone: a reader blocked on a full
    /// channel is about to fail its send, not stuck.
    channel_gone: bool,
    /// Signals sent to the writer it has not acted on yet.
    signals: u64,
    /// Writes the writer still has to make: it is not quiet before.
    writes_left: u64,
    released: bool,
}

#[derive(Clone)]
pub struct ManualClock(Arc<(Mutex<State>, Condvar)>);

impl ManualClock {
    pub fn new() -> Self {
        Self(Arc::new((
            Mutex::new(State {
                now: 0,
                generation: 0,
                waiting: None,
                reader: Reader::Absent,
                written: 0,
                consumed: 0,
                delivered: 0,
                taken: 0,
                fds: Vec::new(),
                channel_gone: false,
                signals: 0,
                writes_left: 0,
                released: false,
            }),
            Condvar::new(),
        )))
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.0.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn change(&self, update: impl FnOnce(&mut State)) {
        let mut state = self.lock();
        update(&mut state);
        state.generation += 1;
        self.0.1.notify_all();
    }

    /// A reader is about to start: it is busy until it reports waiting.
    pub fn expect_reader(&self) {
        self.change(|state| state.reader = Reader::Busy);
    }

    pub fn wrote(&self, bytes: u64) {
        self.change(|state| state.written += bytes);
    }

    pub fn read(&self, bytes: u64) {
        self.change(|state| state.consumed += bytes);
    }

    /// Wake whoever waits: something the clock does not see changed.
    pub fn poke(&self) {
        self.change(|_| {});
    }

    /// Every wait times out from now on: a failing test's guard frees the
    /// code under test.
    pub fn release(&self) {
        self.change(|state| {
            state.released = true;
            state.now = u64::MAX;
        });
    }

    /// Wait until `condition` holds or `until` passes.
    pub fn until(&self, until: u64, mut condition: impl FnMut(&State) -> bool) -> bool {
        let mut state = self.lock();
        loop {
            if condition(&state) {
                return true;
            }
            if state.now >= until {
                return false;
            }
            let generation = state.generation;
            state.waiting = Some((until, generation));
            self.0.1.notify_all();
            state = self
                .0
                .1
                .wait_while(state, |state| state.generation == generation)
                .unwrap_or_else(PoisonError::into_inner);
            state.waiting = None;
        }
    }

    /// The writer's step, outside the code under test and without a
    /// deadline: wait until every byte written so far was read (or the
    /// clock is released), then count `bytes` as written in the same
    /// change, so no quiet moment lies between the read and the write.
    pub fn wait_then_write(&self, bytes: u64) {
        let state = self.lock();
        let mut state = self
            .0
            .1
            .wait_while(state, |state| {
                state.consumed < state.written && !state.released
            })
            .unwrap_or_else(PoisonError::into_inner);
        state.written += bytes;
        state.writes_left = state.writes_left.saturating_sub(1);
        state.generation += 1;
        self.0.1.notify_all();
    }

    /// The writer will make `writes` writes before it waits for a signal.
    pub fn plan_writes(&self, writes: u64) {
        self.change(|state| state.writes_left = writes);
    }

    /// A signal to the writer is on its way: nothing is quiet until the
    /// writer has acted on it.
    pub fn signal_sent(&self) {
        self.change(|state| state.signals += 1);
    }

    pub fn signal_taken(&self) {
        self.change(|state| state.signals = state.signals.saturating_sub(1));
    }

    /// Wait until the reader has delivered `chunks` chunks or `until`.
    pub fn until_delivered(&self, chunks: u64, until: u64) -> bool {
        self.until(until, |state| state.delivered >= chunks)
    }

    /// Move time, a quiet step at a time, until `done`; at most `steps`
    /// steps. Whether `done` came.
    pub fn drive(&self, done: &AtomicBool, steps: usize) -> bool {
        let mut taken = 0;
        let mut state = self.lock();
        loop {
            if done.load(std::sync::atomic::Ordering::SeqCst) {
                return true;
            }
            // Nothing but time can move: no stop or signal in flight, and
            // the reader ended, waits in `poll` with no input pending and
            // no write to come, or waits on a full channel.
            let full =
                state.delivered.saturating_sub(state.taken) >= super::super::STREAM_DEPTH as u64;
            let quiet = state.signals == 0
                && match state.reader {
                    Reader::Absent => true,
                    Reader::Busy => false,
                    Reader::Waiting => state.writes_left == 0 && !readable(&state.fds),
                    Reader::Sending => full && !state.channel_gone,
                };
            // Every notified party has resumed and blocked again: the
            // waiter saw the latest change.
            let acknowledged = state
                .waiting
                .is_some_and(|(_, seen)| seen == state.generation);
            if let (Some((deadline, _)), true, true) = (state.waiting, quiet, acknowledged) {
                if taken == steps {
                    return false;
                }
                taken += 1;
                state.now = deadline;
                state.generation += 1;
                self.0.1.notify_all();
            }
            state = self.0.1.wait(state).unwrap_or_else(PoisonError::into_inner);
        }
    }
}

impl Clock for ManualClock {
    type Instant = u64;

    fn now(&self) -> u64 {
        self.lock().now
    }

    fn after(&self, from: u64, by: Duration) -> u64 {
        from.saturating_add(u64::try_from(by.as_micros()).unwrap_or(u64::MAX))
    }

    fn remaining(&self, until: u64) -> Duration {
        Duration::from_micros(until.saturating_sub(self.now()))
    }

    fn recv<T>(&self, channel: &mpsc::Receiver<T>, until: u64) -> Wait<T> {
        loop {
            let generation = self.lock().generation;
            match channel.try_recv() {
                Ok(message) => return Wait::Message(message),
                Err(mpsc::TryRecvError::Disconnected) => return Wait::Disconnected,
                Err(mpsc::TryRecvError::Empty) => {}
            }
            let mut state = self.lock();
            if state.now >= until {
                return Wait::Timeout;
            }
            if state.generation != generation {
                continue;
            }
            state.waiting = Some((until, generation));
            self.0.1.notify_all();
            state = self
                .0
                .1
                .wait_while(state, |state| state.generation == generation)
                .unwrap_or_else(PoisonError::into_inner);
            state.waiting = None;
        }
    }
}

impl Observer for ManualClock {
    fn watch(&self, source: BorrowedFd<'_>, stop: BorrowedFd<'_>) {
        self.keep(source, stop);
    }

    fn event(&self, event: ReaderEvent) {
        self.change(|state| match event {
            ReaderEvent::Waiting => state.reader = Reader::Waiting,
            ReaderEvent::Woke => state.reader = Reader::Busy,
            ReaderEvent::Sending => state.reader = Reader::Sending,
            ReaderEvent::Delivered => {
                state.delivered += 1;
                state.reader = Reader::Busy;
            }
            ReaderEvent::Taken => state.taken += 1,
            ReaderEvent::Stopping => {}
            ReaderEvent::Released => state.channel_gone = true,
            ReaderEvent::Ended => {
                state.reader = Reader::Absent;
                state.fds.clear();
                state.channel_gone = false;
            }
        });
    }
}

impl ManualClock {
    fn keep(&self, source: BorrowedFd<'_>, stop: BorrowedFd<'_>) {
        let source = source
            .try_clone_to_owned()
            .expect("a duplicate of the source");
        let stop = stop
            .try_clone_to_owned()
            .expect("a duplicate of the stop pipe");
        self.change(|state| state.fds = vec![source, stop]);
    }
}

impl std::fmt::Debug for ManualClock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:?}", *self.lock())
    }
}
