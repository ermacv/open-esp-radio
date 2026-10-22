//! Backend custody conversions; none accepts a P1 reference as stored payload.
use super::*;

/// Durable session owner, fixed before the invariant body loan is introduced.
pub struct OwnedSession<P: PoolPath, S> {
    slot: Slot<'static, DtmSessionKind, Child<P, 1>>,
    epoch: u64,
    state: PhantomData<S>,
}
impl<P: PoolPath, S> OwnedSession<P, S> {
    /// Wrap one of the backend's originally minted session slots.
    pub fn new(slot: Slot<'static, DtmSessionKind, Child<P, 1>>, epoch: u64) -> Self {
        Self {
            slot,
            epoch,
            state: PhantomData,
        }
    }
    /// Preserve original slot identity while changing backend-proved state.
    pub fn transition<T>(self) -> OwnedSession<P, T> {
        OwnedSession::new(self.slot, self.epoch)
    }
    /// Original resource index.
    pub fn index(&self) -> u8 {
        self.slot.index()
    }
    /// Original installation epoch.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    /// Lend a complete owner under the backend's private body authority.
    pub fn lend<'p, B: SlotOwner<P::Identity>>(
        self,
        loan: &BodyLoan<'p, P::Identity, B>,
    ) -> Session<'p, P, S> {
        Session {
            slot: loan.lend_slot(self.slot),
            epoch: self.epoch,
            state: PhantomData,
        }
    }
    /// Retain a returned whole owner in permanent queue storage.
    pub fn retain<'p, B: SlotOwner<P::Identity>>(
        session: Session<'p, P, S>,
        loan: &BodyLoan<'p, P::Identity, B>,
    ) -> Self {
        Self::new(loan.reclaim_slot(session.slot), session.epoch)
    }
}
/// Owned DTM command. The supported configuration is moved, never reconstructed.
pub enum OwnedCommand<P: PoolPath, C: LeCapabilities> {
    /// Start TX.
    Transmit(OwnedSession<P, Idle>, Supported<C, Transmit>),
    /// Start RX.
    Receive(OwnedSession<P, Idle>, Supported<C, Receive>),
    /// End a running test.
    End(OwnedSession<P, Running>),
}
impl<P: PoolPath, C: LeCapabilities> OwnedCommand<P, C> {
    /// Original session resource index, for actual-owner trace capture.
    pub fn index(&self) -> u8 {
        match self {
            Self::Transmit(s, _) | Self::Receive(s, _) => s.index(),
            Self::End(s) => s.index(),
        }
    }
    /// Session epoch used at the P2 publication boundary.
    pub fn epoch(&self) -> u64 {
        match self {
            Self::Transmit(s, _) | Self::Receive(s, _) => s.epoch(),
            Self::End(s) => s.epoch(),
        }
    }
}
/// Entire admitted request, with no reference into a P1 future.
pub struct OwnedRequest<P: PoolPath, X, C: LeCapabilities> {
    /// Correlation.
    pub id: LeOperationId,
    /// Indivisible command and terminal capacity reservation.
    pub capacity: Permit<'static, CommandCapacity, Child<P, 0>>,
    /// Owned operation and resource lease.
    pub command: OwnedCommand<P, C>,
    /// Complete context, with representation fixed before the body loan.
    pub context: X,
}
impl<'p, P: PoolPath, X, C: LeCapabilities> Request<'p, P, X, C> {
    /// Inspect the session epoch without consuming or mutating the request.
    pub fn epoch(&self) -> u64 {
        let Command::DirectTest(command) = &self.command;
        match command {
            DtmCommand::Transmit(s, _) | DtmCommand::Receive(s, _) => s.epoch(),
            DtmCommand::End(s) => s.epoch(),
        }
    }
    /// Move the entire request across the backend's private queue boundary.
    pub fn retain<B: SlotOwner<P::Identity>>(
        self,
        loan: &BodyLoan<'p, P::Identity, B>,
    ) -> OwnedRequest<P, X, C> {
        let Command::DirectTest(command) = self.command;
        let command = match command {
            DtmCommand::Transmit(s, c) => OwnedCommand::Transmit(OwnedSession::retain(s, loan), c),
            DtmCommand::Receive(s, c) => OwnedCommand::Receive(OwnedSession::retain(s, loan), c),
            DtmCommand::End(s) => OwnedCommand::End(OwnedSession::retain(s, loan)),
        };
        OwnedRequest {
            id: self.id,
            capacity: loan.reclaim_permit(self.capacity),
            command,
            context: self.context.into_owned(loan),
        }
    }
}
