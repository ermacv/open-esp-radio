//! Operation ownership, correlation and backend lifecycle messages.

use crate::PortResult;
use core::{fmt::Debug, future::Future, ops::RangeInclusive};
use oer_time::RadioInstant;

/// One admitted operation: what a client's submission returns. It holds
/// the backend's token, which is neither `Clone` nor `Copy` and only the
/// backend mints, and the client that admitted it, so it names exactly one
/// operation, only its holder ends it, and it ends through the client that
/// admitted it ([`Self::cancel`]). It borrows that client (`'c`), so no
/// work outlives its phase and none can name the work of a later one.
#[must_use = "dropping a work forgoes ending it; its terminal still arrives"]
pub struct Work<'c, C: RadioPortClient> {
    token: C::Token,
    client: &'c C,
}

impl<'c, C: RadioPortClient> Work<'c, C> {
    /// Admitted work of `client`, which the backend's submission minted
    /// `token` for.
    pub const fn new(client: &'c C, token: C::Token) -> Self {
        Self { token, client }
    }

    /// The key a router matches the operation's terminal event by.
    pub fn id(&self) -> <C::Token as RadioWork>::Id {
        self.token.id()
    }

    /// End the operation through the client that admitted it. Its terminal
    /// event is the operation's own, which may be its natural end; a
    /// refusal as [`CancelError::NotRunning`] says the air ended it first,
    /// the world's outcome. Dropping the future may leave the work running
    /// or ended; its terminal arrives either way.
    pub fn cancel(self) -> impl Future<Output = PortResult<(), CancelError, C::Fault>> + 'c {
        self.client.end(self.token)
    }
}

/// A backend's token of one admitted operation ([`Work`]).
pub trait RadioWork {
    /// The key a router matches the operation's terminal event by. It
    /// grants nothing: ending the work takes the [`Work`] itself.
    type Id: Copy + Eq;

    /// The routing key of this operation.
    fn id(&self) -> Self::Id;
}

/// What every client handle of a running phase does beside its protocol's
/// own submissions, settings and permits: read the radio clock and end its
/// own work.
///
/// A client exists only for an interface the disabled port configured, and
/// its protocol's calls name only that interface. Its submissions return a
/// [`Work`] that borrows it and ends through it.
pub trait RadioPortClient {
    /// The backend's token of one admitted operation of this client.
    type Token: RadioWork;
    /// The clock domain of the port's radio instants.
    type Domain;
    /// The cause a poisoned backend reports.
    type Fault: Copy + Debug;

    /// The radio clock: the epoch of the port's scheduled work and receive
    /// timestamps, at the resolution its [`ClockInfo`](crate::ClockInfo)
    /// states.
    fn now(
        &self,
    ) -> impl Future<Output = PortResult<RadioInstant<Self::Domain>, ClockError, Self::Fault>> + '_;

    /// The backend's side of [`Work::cancel`]: end the operation `token`
    /// names, which this client admitted. Only a [`Work`] holds a token and
    /// nothing takes it out but `cancel`, which calls the client that
    /// admitted it, so a caller reaches this only through that `cancel`.
    fn end(
        &self,
        token: Self::Token,
    ) -> impl Future<Output = PortResult<(), CancelError, Self::Fault>> + '_;
}

/// Why the radio clock could not be read; nothing changed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ClockError {
    /// The backend could not take a reading now (a latch failed, the clock
    /// belongs to a replaced radio start); a later call may succeed.
    Unavailable,
    /// The radio epoch has no later instant: the clock cannot advance, and
    /// no later call succeeds. The port still settles the work it admitted.
    EpochExhausted,
}

/// Why a cancellation was refused; nothing changed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CancelError {
    /// No admitted work has this identity: it already ended and its terminal
    /// event was reported, or it never ran.
    NotRunning,
}

/// A lifecycle command; each admitted one ends with one [`LifecycleEvent`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LifecycleCommand {
    /// Start admitting work and receiving; ends with
    /// [`LifecycleEvent::Enabled`] or [`LifecycleEvent::Failed`].
    Enable,
    /// Stop admitting work, end the work in flight and stop receiving; ends
    /// with [`LifecycleEvent::Disabled`] after the terminal event of every
    /// admitted operation.
    Disable,
    /// Stop admitting work and let the work in flight complete; ends with
    /// [`LifecycleEvent::Quiesced`] after the last terminal event.
    /// [`Self::Enable`] resumes admission.
    Quiesce,
}

/// The terminal event of a lifecycle command.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LifecycleEvent {
    /// [`LifecycleCommand::Enable`] completed.
    Enabled,
    /// [`LifecycleCommand::Disable`] completed.
    Disabled,
    /// [`LifecycleCommand::Quiesce`] completed.
    Quiesced,
    /// An admitted command failed and left the port in the state it was in
    /// before the command. A failure that leaves the backend's state
    /// unknown is not reported here: [`crate::Poisoned`] follows instead.
    Failed {
        /// The command that failed.
        command: LifecycleCommand,
    },
}

/// Why a lifecycle command was refused; nothing changed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LifecycleError {
    /// The port is already in the requested state.
    AlreadyInState,
    /// The command does not apply in the port's current state, or the
    /// backend does not implement it.
    InvalidState,
    /// Another lifecycle command has not ended yet.
    Busy,
}

/// A caller-chosen correlation identity of one port's submissions.
///
/// Each port keeps its own identity type (a Wi-Fi `TxId`, an
/// IEEE 802.15.4 `RequestId`), so the identity of
/// one port cannot complete another's work; all of them are 32-bit values
/// with the same [`BACKEND_RESERVED`] range, which callers never choose and
/// a backend may use for work it submits on its own.
pub trait Correlation: Copy + Eq {
    /// The identity with this raw value.
    fn from_raw(raw: u32) -> Self;

    /// The raw value.
    fn raw(self) -> u32;

    /// Whether the identity lies in the backend's range.
    fn is_backend_reserved(self) -> bool {
        BACKEND_RESERVED.contains(&self.raw())
    }
}

/// Raw identities a caller never chooses: a backend may name its own work
/// with them, and a caller's allocator skips them.
pub const BACKEND_RESERVED: RangeInclusive<u32> = 0xFFFF_FF00..=u32::MAX;

/// A caller's allocator of correlation identities: consecutive values that
/// wrap before [`BACKEND_RESERVED`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CorrelationIds {
    next: u32,
}

impl Default for CorrelationIds {
    fn default() -> Self {
        Self::new()
    }
}

impl CorrelationIds {
    /// Identities from zero.
    pub const fn new() -> Self {
        Self::starting_at(0)
    }

    /// Identities from `first`; a reserved `first` starts at zero.
    pub const fn starting_at(first: u32) -> Self {
        Self {
            next: if first >= *BACKEND_RESERVED.start() {
                0
            } else {
                first
            },
        }
    }

    /// The next identity.
    #[allow(
        clippy::should_implement_trait,
        reason = "the allocator never ends and yields any identity type"
    )]
    pub fn next<I: Correlation>(&mut self) -> I {
        let id = self.next;
        self.next = match id.checked_add(1) {
            Some(next) if next < *BACKEND_RESERVED.start() => next,
            _ => 0,
        };
        I::from_raw(id)
    }
}
