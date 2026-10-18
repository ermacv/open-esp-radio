use core::sync::atomic::{AtomicU8, Ordering};

use embassy_sync::{blocking_mutex::raw::RawMutex, signal::Signal};

const NO_COMMAND: u8 = 0;
const RECONNECT_COMMAND: u8 = 1;
const DISCONNECT_COMMAND: u8 = 2;
const STOP_COMMAND: u8 = 3;
const NO_COMPLETION: u8 = 0;
const STOPPED_COMPLETION: u8 = 1;
const ENDED_COMPLETION: u8 = 2;
const FAULTED_COMPLETION: u8 = 3;
const NO_ENDPOINTS: u8 = 0;
const STATION_ENDPOINTS: u8 = 2;

/// Terminal acknowledgement published by the task which owns station
/// hardware.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum StationCompletion {
    /// The requested finite stop returned the exact quiescent owner.
    Stopped = STOPPED_COMPLETION,
    /// The runner returned its owner through another terminal edge.
    Ended = ENDED_COMPLETION,
    /// The task future was destroyed before it returned a quiescent owner.
    /// Lower hardware owners remain fail-closed; no reusable station frontier
    /// is claimed.
    Faulted = FAULTED_COMPLETION,
}

/// A new station epoch cannot be materialized after the preceding task lost
/// its hardware owner without proving quiescence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationControlError {
    /// A controller or task endpoint from the preceding epoch still exists.
    InUse,
    /// The previous task was cancelled or destroyed, so its static DMA or ISR
    /// resources cannot be reused through this control domain.
    Faulted,
}

impl StationCompletion {
    const fn decode(value: u8) -> Option<Self> {
        match value {
            STOPPED_COMPLETION => Some(Self::Stopped),
            ENDED_COMPLETION => Some(Self::Ended),
            FAULTED_COMPLETION => Some(Self::Faulted),
            _ => None,
        }
    }
}

/// Cooperative command accepted by a running station service.
///
/// Commands are ordered by severity. A pending `Stop` cannot be replaced by
/// a later reconnect request, while `Stop` may upgrade either weaker command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum StationCommand {
    /// End the current connected epoch and let the lifecycle rescan/reconnect.
    Reconnect = RECONNECT_COMMAND,
    /// Leave the controlled port and return the station owner to the caller.
    Disconnect = DISCONNECT_COMMAND,
    /// Stop the complete station service and return all recoverable owners.
    Stop = STOP_COMMAND,
}

impl StationCommand {
    const fn decode(value: u8) -> Option<Self> {
        match value {
            RECONNECT_COMMAND => Some(Self::Reconnect),
            DISCONNECT_COMMAND => Some(Self::Disconnect),
            STOP_COMMAND => Some(Self::Stop),
            _ => None,
        }
    }
}

/// Static command storage shared by one controller and one station runner.
///
/// This is deliberately separate from DMA and protocol resources. It can be
/// placed in ordinary static storage and reused after the previous runner has
/// returned its owner.
pub struct StationControlResources<M: RawMutex> {
    endpoints: AtomicU8,
    pending: AtomicU8,
    terminal: AtomicU8,
    completion: AtomicU8,
    wake: Signal<M, ()>,
    completion_wake: Signal<M, ()>,
}

impl<M: RawMutex> StationControlResources<M> {
    pub const fn new() -> Self {
        Self {
            endpoints: AtomicU8::new(NO_ENDPOINTS),
            pending: AtomicU8::new(NO_COMMAND),
            terminal: AtomicU8::new(NO_COMMAND),
            completion: AtomicU8::new(NO_COMPLETION),
            wake: Signal::new(),
            completion_wake: Signal::new(),
        }
    }

    /// Start one station session and create its single command consumer.
    ///
    /// An atomic endpoint lease prevents two runners from being constructed
    /// from the same command slot or terminal-completion waiter. This permits
    /// a cleanly returned static resource to start a later station epoch while
    /// still rejecting overlap and preserving sticky cancellation poison.
    pub fn split(
        &self,
    ) -> Result<(StationController<'_, M>, StationCommandReceiver<'_, M>), StationControlError>
    {
        if StationCompletion::decode(self.completion.load(Ordering::Acquire))
            == Some(StationCompletion::Faulted)
        {
            return Err(StationControlError::Faulted);
        }
        if self
            .endpoints
            .compare_exchange(
                NO_ENDPOINTS,
                STATION_ENDPOINTS,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return Err(StationControlError::InUse);
        }
        self.pending.store(NO_COMMAND, Ordering::Release);
        self.terminal.store(NO_COMMAND, Ordering::Release);
        self.completion.store(NO_COMPLETION, Ordering::Release);
        self.wake.reset();
        self.completion_wake.reset();
        let shared = self;
        Ok((
            StationController { resources: shared },
            StationCommandReceiver { resources: shared },
        ))
    }

    fn request(&self, command: StationCommand) -> bool {
        if self.completion.load(Ordering::Acquire) != NO_COMPLETION {
            return false;
        }
        let requested = command as u8;
        let mut observed = self.pending.load(Ordering::Acquire);
        loop {
            if observed >= requested {
                return false;
            }
            match self.pending.compare_exchange_weak(
                observed,
                requested,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.wake.signal(());
                    return true;
                }
                Err(actual) => observed = actual,
            }
        }
    }

    fn take_pending(&self) -> Option<StationCommand> {
        StationCommand::decode(self.pending.swap(NO_COMMAND, Ordering::AcqRel))
    }

    fn record_terminal(&self, command: StationCommand) {
        self.terminal.store(command as u8, Ordering::Release);
    }

    pub(super) fn take_terminal(&self) -> Option<StationCommand> {
        StationCommand::decode(self.terminal.swap(NO_COMMAND, Ordering::AcqRel))
    }

    pub(super) fn publish_completion(&self, completion: StationCompletion) {
        if self.completion.load(Ordering::Acquire) == FAULTED_COMPLETION {
            return;
        }
        self.completion.store(completion as u8, Ordering::Release);
        self.completion_wake.signal(());
    }

    fn release_endpoint(&self) {
        let previous = self.endpoints.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous != NO_ENDPOINTS);
    }
}

impl<M: RawMutex> Default for StationControlResources<M> {
    fn default() -> Self {
        Self::new()
    }
}

/// Unique application handle for cooperative station control.
///
/// Command publication uses a shared borrow, but waiting for terminal
/// completion requires a mutable borrow so the single-waiter Embassy signal
/// cannot accidentally be observed by concurrent stop futures.
pub struct StationController<'resources, M: RawMutex> {
    resources: &'resources StationControlResources<M>,
}

impl<M: RawMutex> Drop for StationController<'_, M> {
    fn drop(&mut self) {
        self.resources.release_endpoint();
    }
}

impl<'resources, M: RawMutex> StationController<'resources, M> {
    /// Request a fresh scan and association without destroying the service.
    pub fn request_reconnect(&self) -> bool {
        self.resources.request(StationCommand::Reconnect)
    }

    /// Request a finite disconnect and return from the station runner.
    pub fn request_disconnect(&self) -> bool {
        self.resources.request(StationCommand::Disconnect)
    }

    /// Request complete finite station shutdown.
    pub fn request_stop(&self) -> bool {
        self.resources.request(StationCommand::Stop)
    }

    /// Wait for a terminal task acknowledgement.
    ///
    /// [`StationCompletion::Stopped`] proves the owner returned
    /// through the controlled lifecycle edge. `Faulted` reports cancellation
    /// or destruction of the task future and permanently quarantines that
    /// owner; it deliberately does not claim that ISR, DMA or protocol tasks
    /// are stopped and exposes no recovery operation. Merely
    /// publishing [`request_stop`](Self::request_stop) proves neither outcome.
    pub async fn wait_completion(&mut self) -> StationCompletion {
        loop {
            if let Some(completion) =
                StationCompletion::decode(self.resources.completion.load(Ordering::Acquire))
            {
                return completion;
            }
            self.resources.completion_wake.wait().await;
        }
    }

    /// Request complete shutdown and wait for task acknowledgement.
    pub async fn stop(&mut self) -> StationCompletion {
        self.request_stop();
        self.wait_completion().await
    }

    pub(super) const fn resources(&self) -> &'resources StationControlResources<M> {
        self.resources
    }
}

/// Single-consumer command side supplied to the concrete lifecycle backend.
///
/// A connected backend must observe this future only at a cancellation-safe
/// edge such as [`crate::datapath::DatapathRunner::run_until`]. It records a
/// terminal command only when it returns `StaAttemptOutcome::Stopped`;
/// `Reconnect` normally becomes a disconnected epoch instead.
pub struct StationCommandReceiver<'resources, M: RawMutex> {
    resources: &'resources StationControlResources<M>,
}

impl<M: RawMutex> Drop for StationCommandReceiver<'_, M> {
    fn drop(&mut self) {
        if self.resources.completion.load(Ordering::Acquire) == NO_COMPLETION {
            self.resources
                .publish_completion(StationCompletion::Faulted);
        }
        self.resources.release_endpoint();
    }
}

impl<M: RawMutex> StationCommandReceiver<'_, M> {
    pub fn try_take(&mut self) -> Option<StationCommand> {
        self.resources.take_pending()
    }

    pub async fn wait(&mut self) -> StationCommand {
        loop {
            if let Some(command) = self.try_take() {
                return command;
            }
            self.resources.wake.wait().await;
        }
    }

    /// Return a command which arrived before the backend reached a safe edge.
    ///
    /// This is primarily useful for `Reconnect`: an early request must remain
    /// pending until the DATAPATH runner can end its epoch safely. A stronger
    /// command queued concurrently retains priority.
    pub fn defer(&mut self, command: StationCommand) -> bool {
        self.resources.request(command)
    }

    /// Preserve the reason for a terminal `Stopped` lifecycle edge.
    pub fn record_terminal(&mut self, command: StationCommand) {
        self.resources.record_terminal(command);
    }

    #[cfg(test)]
    pub(super) fn take_terminal(&mut self) -> Option<StationCommand> {
        self.resources.take_terminal()
    }
}
