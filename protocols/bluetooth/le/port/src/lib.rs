#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]

//! Portable LE submission and resource custody on `oer-radio-port` lifecycle.
//!
//! DTM starts and End are synchronous admissions. Every command owns a capacity
//! bundle until its mandatory terminal is dequeued; an active test additionally
//! holds its unique session lease until End or confirmed withdrawal. P2 progresses
//! independently of `next_event`. The latter only removes already produced events.
//! Body loans and component paths survive every queue handover. Hardware owners
//! and conversion authority never appear in a portable body.
//!
//! Caller IDs use the shared allocator, skipping the backend-reserved range:
//! ```
//! use oer_bluetooth_le_port::LeOperationId;
//! use oer_radio_port::{Correlation, CorrelationIds};
//! let mut ids = CorrelationIds::starting_at(0xffff_feff);
//! assert_eq!(ids.next::<LeOperationId>().0, 0xffff_feff);
//! assert_eq!(ids.next::<LeOperationId>().0, 0);
//! assert!(LeOperationId(0xffff_ff00).is_backend_reserved());
//! ```
//! Session states remain affine; an idle session cannot issue End:
//! ```compile_fail,E0308
//! use oer_bluetooth_le_port::{DtmCommand, Idle, LeCapabilities, Session};
//! use oer_radio_port::PoolPath;
//! fn end<'p, P: PoolPath, C: LeCapabilities>(session: Session<'p, P, Idle>) -> DtmCommand<'p, P, C> {
//!     DtmCommand::End(session)
//! }
//! ```

use core::{future::Future, marker::PhantomData};
use oer_bluetooth_le_radio::{Capability, Untimed};
pub use oer_bluetooth_le_radio::{
    EventResult, LeCapabilities, LeRadio, LeRadioCapabilities, Supported, TestChannel,
    TestPayloadType, TestPhy, TxPower,
};
use oer_radio_port::{
    BodyLoan, Child, Correlation, Permit, PoolPath, PortContext, PortPhase, Seat, Slot, SlotOwner,
};
pub use oer_radio_port::{EventEnd, EventsLost, PortResult};

/// LE correlation key; the upper 256 values are reserved to backends.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeOperationId(pub u32);
impl Correlation for LeOperationId {
    fn from_raw(raw: u32) -> Self {
        Self(raw)
    }
    fn raw(self) -> u32 {
        self.0
    }
}

/// One command seat, item lease and mandatory-terminal reservation together.
/// The backend sizes all three stores for its complete supply. Capacity waits
/// are FIFO and cannot refuse; cancelling a wait returns its waiter seat.
pub enum CommandCapacity {}
/// The unique DTM session of an installation.
pub enum DtmSessionKind {}
/// A DTM session ready to start.
pub enum Idle {}
/// A DTM session whose transmitter or receiver is running.
pub enum Running {}

/// Affine test session; its state, component path and invariant body loan
/// prevent release through another pool, context or installation.
#[must_use = "retain the session until its next command or normal disable"]
pub struct Session<'p, P: PoolPath, State> {
    slot: Slot<'p, DtmSessionKind, Child<P, 1>>,
    epoch: u64,
    state: PhantomData<State>,
}
impl<P: PoolPath, S> Session<'_, P, S> {
    /// Original resource index, preserved through every terminal.
    pub fn index(&self) -> u8 {
        self.slot.index()
    }
    /// Backend's installation generation, independent of correlation IDs.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

/// DTM transmitter settings. Payload bytes are generated from the standard
/// pattern by the backend; no borrowed P1 buffer crosses the port.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Transmit {
    /// RF test channel.
    pub channel: TestChannel,
    /// Test PHY.
    pub phy: TestPhy,
    /// Payload pattern.
    pub payload: TestPayloadType,
    /// Number of payload octets.
    pub length: u8,
    /// Regulatory-capped requested power.
    pub power: TxPower,
}
impl Capability for Transmit {
    fn supported(&self, caps: &LeRadioCapabilities) -> bool {
        caps.direct_test_mode
            && caps.test_phys.contains(self.phy.phy())
            && self.power.supported(caps)
    }
}
impl Untimed for Transmit {}

/// DTM receive settings. The receiver requires no TX-power authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Receive {
    /// RF test channel.
    pub channel: TestChannel,
    /// Test PHY.
    pub phy: TestPhy,
}
impl Capability for Receive {
    fn supported(&self, caps: &LeRadioCapabilities) -> bool {
        caps.direct_test_mode && caps.test_phys.contains(self.phy.phy())
    }
}
impl Untimed for Receive {}

/// Commands supported by this slice. Other LE operations can share the same
/// admission, terminal and context envelope without changing their base types.
pub enum DtmCommand<'p, P: PoolPath, C: LeCapabilities> {
    /// Start the transmitter and retain the test lease through End.
    Transmit(Session<'p, P, Idle>, Supported<C, Transmit>),
    /// Start the receiver and retain the test lease through End.
    Receive(Session<'p, P, Idle>, Supported<C, Receive>),
    /// Stop a test and return the receiver's packet count (zero for TX).
    End(Session<'p, P, Running>),
}

/// Protocol operation envelope. Future operation families add variants while
/// preserving the submission, capacity, context and lifecycle types.
pub enum Command<'p, P: PoolPath, C: LeCapabilities> {
    /// Direct Test Mode under an explicit test-air profile.
    DirectTest(DtmCommand<'p, P, C>),
}

/// Complete owned submission. A pre-admission refusal returns this exact value.
pub struct Request<'p, P: PoolPath, X, C: LeCapabilities> {
    /// Caller correlation, unique among outstanding commands.
    pub id: LeOperationId,
    /// Capacity taken before admission; no command waits for space after it.
    pub capacity: Permit<'p, CommandCapacity, Child<P, 0>>,
    /// The affine session command.
    pub command: Command<'p, P, C>,
    /// Opaque caller context, fixed by the backend before the body loan exists.
    pub context: PortContext<'p, P::Identity, X>,
}
/// Reason for a refusal before admission. No request owner was changed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Refusal {
    /// Caller chose one of the backend's reserved IDs.
    ReservedId,
    /// This identity already names an outstanding terminal or command.
    DuplicateId,
    /// Session belongs to an earlier installation.
    StaleEpoch,
    /// External test profile does not currently permit operation admission.
    Unavailable,
}
/// A refusal preserves every submitted resource and the complete command.
pub struct Refused<R> {
    /// Why nothing changed.
    pub reason: Refusal,
    /// Original, unmodified request.
    pub request: R,
}
/// Admission receipt. It grants no resource-release authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Admitted {
    /// Correlation of the admitted command.
    pub id: LeOperationId,
    /// Installation generation that accepted it.
    pub epoch: u64,
}
/// Fact accompanying a terminal's execution class.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Cause {
    /// Ordinary test execution.
    Complete,
    /// Disable ended admitted work before publication.
    Disabled,
    /// Command was stale when P2 reached its publication boundary.
    StaleEpoch,
    /// A test-profile or coexistence decision prevented execution.
    AirDenied,
    /// Backend has proved withdrawal after publication.
    Withdrawn,
    /// Execution and withdrawal are unknown; hardware owners remain quarantined.
    Fault,
}
/// Received packet count, saturated explicitly at the HCI DTM limit.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PacketCount {
    /// Good packets observed during the receiver session.
    pub packets: u16,
    /// More than `u16::MAX` packets arrived.
    pub saturated: bool,
}
/// Returned session authority. Indeterminate execution never returns a lease.
pub enum ReturnedSession<'p, P: PoolPath> {
    /// Test is stopped and this owner may start another one.
    Idle(Session<'p, P, Idle>),
    /// Start executed (or End did not execute); End still owns the next stop.
    Running(Session<'p, P, Running>),
    /// Owners remain in durable backend quarantine.
    Quarantined,
}
/// Operation-specific returned owners. Each operation family extends this
/// envelope without altering terminal correlation or its opaque context.
pub enum Completion<'p, P: PoolPath> {
    /// Direct Test Mode resources and End count.
    DirectTest {
        /// Reusable or still-running session; unknown reachability has none.
        session: ReturnedSession<'p, P>,
        /// Present on an executed End, zero for a transmitter.
        count: Option<PacketCount>,
    },
}
/// The installation's single owned stream. Reports and other operation
/// families extend this envelope while preserving its one consumer.
pub enum Event<'p, P: PoolPath, X> {
    /// Mandatory terminal of admitted work.
    Terminal(Terminal<'p, P, X>),
}
/// One mandatory owned event, including actual owners rather than copied IDs.
pub struct Terminal<'p, P: PoolPath, X> {
    /// Operation identity read from the admitted request.
    pub admitted: Admitted,
    /// Monotonic instant sampled before enqueue.
    pub admitted_at: oer_time::Instant,
    /// Execution versus withdrawal versus unknown completion.
    pub result: EventResult,
    /// Execution or lost-air fact.
    pub cause: Cause,
    /// Returned operation-specific owners and observations.
    pub completion: Completion<'p, P>,
    /// Original caller context.
    pub context: PortContext<'p, P::Identity, X>,
}

/// Synchronous admission result, preserving the complete request on refusal.
pub type Submission<'p, P, X, C, F> = PortResult<Admitted, Refused<Request<'p, P, X, C>>, F>;

/// Running LE queue interface, lent by `PortBackend::phase`. One implementation
/// serves hardware and the reference model. Lifecycle is exclusively the shared
/// `Owner`/`PortSlot`/`run_port` contract, never a second LE supervisor.
pub trait LePhase<'p, P: PoolPath, X>: PortPhase<Event = Event<'p, P, X>> {
    /// Statically checked backend capabilities.
    type Caps: LeCapabilities;
    /// Read capabilities without queueing a command or progressing hardware.
    fn capabilities(&self) -> LeRadioCapabilities {
        Self::Caps::CAPS
    }
    /// Reserve a complete command bundle in FIFO order. Drop before admission
    /// releases its queue position; the seat remains with its caller.
    fn take<'a>(
        &'a self,
        seat: &'a mut Seat<'p, CommandCapacity, Child<P, 0>>,
    ) -> impl Future<Output = Permit<'p, CommandCapacity, Child<P, 0>>> + 'a
    where
        'p: 'a;
    /// Return an unused reservation, once. An admitted reservation is retained
    /// by P2 through its terminal, even if the caller drops an event wait.
    fn release(&self, capacity: Permit<'p, CommandCapacity, Child<P, 0>>);
    /// Synchronously reserve and enqueue the complete request. Outer poison
    /// retains all owners in the permanent slot; inner refusal returns all of
    /// them unchanged. P1 records monotonic admission time before enqueue.
    fn submit(
        &self,
        request: Request<'p, P, X, Self::Caps>,
    ) -> Submission<'p, P, X, Self::Caps, Self::Fault>;
}

/// Durable backend representations. These require private `BodyLoan` authority
/// to cross the P1 boundary; portable bodies never receive that authority.
pub mod backend;
/// Allocator-free scenarios and observed-owner trace validation.
#[cfg(feature = "test-support")]
pub mod conformance;

#[cfg(test)]
#[allow(clippy::disallowed_macros)]
mod tests;
