//! Custody of an accepted TX operation, from admission to its terminal.

use core::marker::PhantomData;
use oer_radio_port::{PoolBuilder, PoolPath, PoolSet, Root, Slot, Slots};

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
pub struct TerminalAccess<'id, Kind, Path: PoolPath = Root> {
    proof: PhantomData<Authority<'id, Kind, Path>>,
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

impl<'id, C, Kind, Path: PoolPath, M, O> Completed<C, Slot<'id, Kind, Path>, M, O> {
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
