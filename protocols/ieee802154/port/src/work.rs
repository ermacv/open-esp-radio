//! The work an interface client admitted and how it ends.

use core::{fmt::Debug, future::Future};

use oer_radio_port::{ClockError, PortResult};
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
    /// timestamps, at the resolution its [`ClockInfo`](oer_radio_port::ClockInfo)
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

/// Why a cancellation was refused; nothing changed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CancelError {
    /// No admitted work has this identity: it already ended and its terminal
    /// event was reported, or it never ran.
    NotRunning,
}
