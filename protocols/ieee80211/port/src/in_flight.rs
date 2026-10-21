//! Custody of an accepted TX operation, from admission to its terminal.

use core::marker::PhantomData;
use oer_radio_port::{
    BodyLoan, LoanedContext, PoolBuilder, PoolPath, PoolSet, PortContext, Root, Slot, SlotIdentity,
    SlotOwner, Slots,
};

/// Permission to lower one preparation before sealing its accepted operation.
/// Only the admission boundary constructs it. Its invariant preparation tag
/// prevents another implementation from borrowing a backend's conversion.
pub struct Admission<P> {
    proof: PhantomData<fn(P) -> P>,
}

impl<P> Admission<P> {
    pub(crate) const fn new() -> Self {
        Self { proof: PhantomData }
    }
}

/// Owners lowered before the indivisible operation is sealed.
pub struct AdmissionOwners<C, T, M, E> {
    /// Opaque owned context.
    pub context: C,
    /// Owned capacity, preserving its identity and path.
    pub ticket: T,
    /// Complete owned MPDU or aggregate.
    pub payload: M,
    /// Backend execution metadata.
    pub execution: E,
}

/// Backend-private owned payload conversion at the terminal boundary.
/// Concrete implementations retain all fields and reconstruct only their
/// public body loan; the complete operation stays sealed during conversion.
pub trait OwnedPayload<Id: SlotIdentity, S: SlotOwner<Id>> {
    /// The same payload under the consumer's invariant body loan.
    type Lent<'p>
    where
        Self: 'p,
        Id: 'p,
        S: 'p;
    /// Lend the complete payload using the concrete backend's private authority.
    fn lend<'p>(self, loan: &BodyLoan<'p, Id, S>) -> Self::Lent<'p>
    where
        Self: 'p,
        Id: 'p,
        S: 'p;
}

/// Backend-private retention of a complete public payload in its owned queue.
pub trait BodyPayload<'p, Id: SlotIdentity, S: SlotOwner<Id>> {
    /// Complete queue representation selected before the body loan.
    type Owned;
    /// End only the public loan, preserving every owned field.
    fn into_owned(self, loan: &BodyLoan<'p, Id, S>) -> Self::Owned;
}

/// Tickets and the unique consumer authority for their terminals. The install
/// gives the authority to the event consumer, never to the execution backend.
pub struct TransmitPool<Kind, const N: usize>(PhantomData<fn() -> Kind>);

impl<Kind, const N: usize> PoolSet for TransmitPool<Kind, N> {
    type Pools<'id, Path: PoolPath> = ([Slot<'id, Kind, Path>; N], TerminalAccess<'id, Kind, Path>);

    fn build<'id, Path: PoolPath>(builder: PoolBuilder<'id, Path>) -> Self::Pools<'id, Path> {
        (
            builder.into_pools::<Slots<Kind, N>>(),
            TerminalAccess { proof: PhantomData },
        )
    }
}

type Authority<'id, Kind, Path> = fn(&'id (), Kind, Path) -> (&'id (), Kind, Path);

/// Sole permission to recover completed owners of this installation and ticket
/// pool. Completing an operation does not grant permission to unpack it.
pub struct TerminalAccess<'id, Kind, Path: PoolPath = Root<oer_radio_port::LocalId<'id>>> {
    proof: PhantomData<Authority<'id, Kind, Path>>,
}

impl<Kind, Path: PoolPath> TerminalAccess<'static, Kind, Path> {
    /// Lend the unique consumer authority without copying it.
    pub fn lend<'p, S: SlotOwner<Path::Identity>>(
        self,
        _: &BodyLoan<'p, Path::Identity, S>,
    ) -> TerminalAccess<'p, Kind, Path> {
        TerminalAccess { proof: PhantomData }
    }
}

/// A backend's execution projection. Its concrete view lends only the data
/// needed to execute the payload: no owning MPDU, body or replaceable owner.
/// The backend keeps the projection's fields private and exposes operations
/// on borrowed storage. In particular, the view must not contain a public
/// `&mut Self` or offer an owner extraction/replacement operation.
///
/// Implementations must also reject mutable-owner coercion (`DerefMut`) and
/// owning iteration or `TxMpdu::into_parts` on the view. The runtime's
/// `execution_projection` UI pair checks these boundaries for both its single
/// MPDU and aggregate on host and RV32. A new backend supplies the same pair
/// for its concrete views: this open trait does not seal their implementations.
pub trait ExecutionPayload {
    /// Narrow execution borrow; it cannot outlive the accepted operation.
    type Execution<'a>
    where
        Self: 'a;
    /// Lend the execution view without releasing ownership of this payload.
    fn execution(&mut self) -> Self::Execution<'_>;
}

/// One accepted operation. Context, ticket and payload are indivisible until
/// a terminal consumes it. There is no mutable access to any of these owners.
/// Queueing, publication, withdrawal and quarantine move this whole value.
pub struct InFlight<C, T, M> {
    context: C,
    ticket: T,
    payload: M,
}

impl<C, T, M> InFlight<C, T, M> {
    /// Accept exactly these owners together at the interface's admission
    /// boundary. Callers assemble an attempt before this point; the backend
    /// enqueues this value and never reconstructs it from a side table.
    pub(crate) const fn admit(context: C, ticket: T, payload: M) -> Self {
        Self {
            context,
            ticket,
            payload,
        }
    }

    /// Inspect execution metadata without exposing a replaceable owner.
    pub const fn payload(&self) -> &M {
        &self.payload
    }

    /// Inspect the reserved capacity without releasing or replacing it.
    pub const fn ticket(&self) -> &T {
        &self.ticket
    }

    /// Execute through the backend's narrow storage projection.
    pub fn execute(&mut self) -> M::Execution<'_>
    where
        M: ExecutionPayload,
    {
        self.payload.execution()
    }

    /// End this operation once, returning its entire owner with its outcome:
    /// a single MPDU's [`SingleOutcome`](crate::SingleOutcome) or an
    /// aggregate's [`AggregateOutcome`](crate::AggregateOutcome).
    pub const fn complete<O>(self, outcome: O) -> Completed<C, T, M, O> {
        Completed {
            operation: self,
            outcome,
        }
    }
}

/// The terminal carries the accepted operation itself. There is no public
/// constructor from independently supplied context, ticket, MPDU or body.
pub struct Completed<C, T, M, O> {
    operation: InFlight<C, T, M>,
    outcome: O,
}

impl<C, T, M, O> Completed<C, T, M, O> {
    /// The terminal's observation, without releasing any owner.
    pub const fn outcome(&self) -> &O {
        &self.outcome
    }
}

impl<C, Kind, Path: PoolPath, M, O> Completed<C, Slot<'static, Kind, Path>, M, O> {
    /// Reintroduce the public body loan on a whole terminal. Context, ticket
    /// and payload cannot be extracted or independently replaced here.
    pub fn lend<'p, S, X>(
        self,
        loan: &BodyLoan<'p, Path::Identity, S>,
    ) -> Completed<X, Slot<'p, Kind, Path>, M::Lent<'p>, O>
    where
        S: SlotOwner<Path::Identity> + 'p,
        Path::Identity: 'p,
        M: OwnedPayload<Path::Identity, S> + 'p,
        X: LoanedContext<'p, Path::Identity, Owned = C>,
    {
        let InFlight {
            context,
            ticket,
            payload,
        } = self.operation;
        Completed {
            operation: InFlight::admit(
                X::restore(PortContext::from_owned(context, loan)),
                loan.lend_slot(ticket),
                payload.lend(loan),
            ),
            outcome: self.outcome,
        }
    }
}

impl<'id, C, Kind, Path: PoolPath, M, O> Completed<C, Slot<'id, Kind, Path>, M, O> {
    /// Retain a whole refused terminal without opening it at a queue boundary.
    pub fn into_owned<S>(
        self,
        loan: &BodyLoan<'id, Path::Identity, S>,
    ) -> Completed<C::Owned, Slot<'static, Kind, Path>, M::Owned, O>
    where
        S: SlotOwner<Path::Identity>,
        C: LoanedContext<'id, Path::Identity>,
        M: BodyPayload<'id, Path::Identity, S>,
    {
        let InFlight {
            context,
            ticket,
            payload,
        } = self.operation;
        Completed {
            operation: InFlight::admit(
                context.enclose().into_owned(loan),
                loan.reclaim_slot(ticket),
                payload.into_owned(loan),
            ),
            outcome: self.outcome,
        }
    }

    /// Recover the owners using the consumer authority for this ticket pool.
    /// A backend can create and forward terminals, but cannot reopen them.
    pub fn into_parts(
        self,
        _: &mut TerminalAccess<'id, Kind, Path>,
    ) -> TerminalParts<C, Slot<'id, Kind, Path>, M, O> {
        let InFlight {
            context,
            ticket,
            payload,
        } = self.operation;
        TerminalParts {
            context,
            ticket,
            payload,
            outcome: self.outcome,
        }
    }
}

/// Owners recovered by consuming a terminal. Their higher layer may now
/// classify, renew, retry or release them according to the returned outcome.
pub struct TerminalParts<C, T, M, O> {
    /// Original opaque operation context.
    pub context: C,
    /// Original admission and terminal capacity.
    pub ticket: T,
    /// One MPDU or the whole submitted aggregate, including external bodies.
    pub payload: M,
    /// Observation which ended this operation.
    pub outcome: O,
}

/// One returned MPDU. A fresh MPDU carries its memory and optional body
/// together; a Sent MPDU retains its body until explicit renewal.
pub trait TxMpdu {
    /// The backend's exhaustive buffer/Sent owner enumeration.
    type Buffer;
    /// External body owner.
    type Body;
    /// Consume a returned or cancelled MPDU, recovering its resources.
    fn into_parts(self) -> (Self::Buffer, Option<Self::Body>);
}
