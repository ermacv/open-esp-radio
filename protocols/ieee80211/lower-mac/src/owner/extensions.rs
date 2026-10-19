//! The optional operations of a lower-MAC phase.
//!
//! A structural feature a backend may lack is an extension trait of the
//! phase or interface that performs it: an upper layer that needs the
//! feature requires the bound, and a backend without it does not implement
//! the trait. Every rule a caller controls is a type here, as in the base
//! contract: an aggregate's rate, traffic identifier and size; only the
//! world's outcome stays a result. No operation awaits: a setting takes
//! effect before the next submission, and what the hardware answers
//! arrives as an event.
//!
//! - [`AmpduPhase`]: one attempt sends one A-MPDU, built in an aggregate
//!   whose subframe count is the backend's limit, each subframe in a buffer
//!   of its own; the planner above the port resends the subframes the
//!   Block Ack report misses.
//! - [`BeaconTimingPhase`]: an interface's TSF and TBTT schedule; its TBTT
//!   events and TSF samples come in the phase's stream.
//! - [`AirReservationPhase`]: a gate's closing that reserves the access
//!   point's air for an absence of the station beside it.
//! - [`MonitorPhase`]: monitor reception, a radio-wide setting.

use oer_ieee80211_mac::tsf::{TsfDuration, TsfInstant};
use oer_radio_port::{Poisoned, PoolPath, Root, Slot};

use super::{Ieee80211Capabilities, PhaseRate, SupportedPower, Tid};
use crate::{
    AmpduCapabilities, PreparationError, Protection, TxBufferClass, TxBufferPool, TxProtection,
    phase::{GateOpen, InterfacePhase, LowerMacPhase, PhaseTypes, RoleKeys},
};

mod state {
    pub trait Sealed {}
}

/// Whether a builder has accepted at least one subframe. Only the two closed
/// states implement this trait; callers cannot forge the nonempty transition.
pub trait AggregateState: state::Sealed {
    /// Storage for the first subframe's fact: absent in Empty, owned in NonEmpty.
    type Head<T>;
    /// Preserve the first fact while appending, creating it on the first push.
    fn retain_head<T: Copy>(head: &Self::Head<T>, next: T) -> T;
}
/// A builder which has not accepted a subframe and cannot be submitted.
pub enum EmptyAggregate {}
/// A builder with at least one subframe; this is the only submission state.
pub enum NonEmptyAggregate {}
impl state::Sealed for EmptyAggregate {}
impl state::Sealed for NonEmptyAggregate {}
impl AggregateState for EmptyAggregate {
    type Head<T> = ();
    fn retain_head<T: Copy>((): &(), next: T) -> T {
        next
    }
}
impl AggregateState for NonEmptyAggregate {
    type Head<T> = T;
    fn retain_head<T: Copy>(head: &T, _: T) -> T {
        *head
    }
}

/// Closed proof that an aggregate has a first subframe. Only
/// `NonEmptyAggregate` implements this refinement of `AggregateState`.
pub trait NonEmptyState: AggregateState {
    /// Recover the first subframe's fact without a runtime empty case.
    fn into_head<T>(head: Self::Head<T>) -> T;
}
impl NonEmptyState for NonEmptyAggregate {
    fn into_head<T>(head: T) -> T {
        head
    }
}

/// A class-checked QoS data description. The builder supplies receiver and
/// TID when encoding it, together with its one protection. Other header fields
/// and the MPDU length remain this subframe's own.
///
/// ```compile_fail,E0308
/// use oer_ieee80211_lower_mac::{ClassSubframe, Ieee80211Capabilities, Tid, TxBufferClass};
/// use oer_ieee80211_mac::address::IndividualAddress;
/// fn flag<C: TxBufferClass, K: Ieee80211Capabilities>(
///     frame: &ClassSubframe<C, K>, receiver: IndividualAddress, tid: Tid,
/// ) { let _header = frame.header(receiver, tid, true); }
/// ```
///
/// ```
/// use oer_ieee80211_lower_mac::{
///     AckHeader, ClassSubframe, Ieee80211Capabilities, MpduProtection, Tid, TxBufferClass,
/// };
/// use oer_ieee80211_mac::address::IndividualAddress;
/// fn typed<C: TxBufferClass, K: Ieee80211Capabilities>(
///     frame: &ClassSubframe<C, K>, receiver: IndividualAddress, tid: Tid,
/// ) -> AckHeader { frame.header(receiver, tid, MpduProtection::Open) }
/// ```
pub struct ClassSubframe<C, K> {
    frame: super::SizedFrame<super::AckHeader, K>,
    qos: super::QosControl,
    class: core::marker::PhantomData<C>,
}
impl<C: TxBufferClass, K: Ieee80211Capabilities> ClassSubframe<C, K> {
    /// Check that a QoS MPDU fits this class. The input is returned on refusal.
    pub fn new(
        frame: super::SizedFrame<super::AckHeader, K>,
    ) -> Result<Self, super::SizedFrame<super::AckHeader, K>> {
        match frame.header().header().qos() {
            Some(qos) if frame.len().get() <= C::CAPACITY => Ok(Self {
                frame,
                qos,
                class: core::marker::PhantomData,
            }),
            _ => Err(frame),
        }
    }
    /// Checked description; receiver and TID will be supplied by the builder.
    pub const fn frame(&self) -> &super::SizedFrame<super::AckHeader, K> {
        &self.frame
    }
    /// Bind the checked QoS header to the builder's recipient and protection.
    pub fn header(
        &self,
        receiver: super::IndividualAddress,
        tid: Tid,
        protection: crate::MpduProtection,
    ) -> super::AckHeader {
        let mut qos = self.qos;
        qos.tid = tid;
        super::AckHeader(
            self.frame
                .header()
                .header()
                .in_aggregate(receiver, qos)
                .with_protection(protection),
        )
    }
}

/// Resources of a subframe. Fresh subframes deliberately have no key or
/// protection argument. Sent subframes own their installed-key identity.
pub enum SubframeSource<B, N, F, O> {
    /// Unpublished memory and its description/body, prepared by the builder.
    Fresh {
        /// Fresh memory of this subframe's buffer class.
        buffer: B,
        /// The class-checked QoS header and complete MPDU length.
        frame: F,
        /// The optional external body, retained until cancellation or its terminal.
        body: Option<O>,
    },
    /// The exact returned MPDU; no replacement header, key or body is accepted.
    Sent(N),
}

/// Class-specific input to the builder; a refusal retains this exact type.
pub type PoolSubframe<'id, T, C, K> = SubframeSource<
    <T as TxBufferPool<'id, C>>::Buffer,
    <T as TxBufferPool<'id, C>>::Sent,
    ClassSubframe<C, K>,
    <T as PhaseTypes<'id>>::TxBody,
>;

/// A refused push returns the builder and all of the rejected subframe.
pub struct AggregateRefused<A, M> {
    /// Preparation or capacity refusal, before admission and PN allocation.
    pub reason: PreparationError,
    /// Every subframe already held, with the original empty/nonempty state.
    pub aggregate: A,
    /// The unchanged rejected subframe, retaining its class.
    pub subframe: M,
}
/// Result of preparing and adding through a class pool. Success is nonempty.
pub type AggregatePushResult<'k, 'p, 'id, T, C, R, K, S> = Result<
    <T as PhaseTypes<'id>>::Aggregate<'k, 'p, R, K, NonEmptyAggregate>,
    AggregateRefused<
        <T as PhaseTypes<'id>>::Aggregate<'k, 'p, R, K, S>,
        PoolSubframe<'id, T, C, K>,
    >,
>;
/// A builder and subframe whose portable layout was checked together.
pub type CheckedPoolAggregate<'k, 'p, 'id, T, C, R, K, S> = crate::CheckedAggregate<
    <T as PhaseTypes<'id>>::Aggregate<'k, 'p, R, K, S>,
    <T as TxBufferPool<'id, C>>::Buffer,
    <T as TxBufferPool<'id, C>>::Sent,
    ClassSubframe<C, K>,
    <T as PhaseTypes<'id>>::TxBody,
>;

/// A rate the backend aggregates at, checked by [`AmpduPhase::ampdu_rate`].
#[derive(Debug)]
pub struct AmpduRate<'p, 'id, K>(PhaseRate<'p, 'id, K>);
impl<K> Copy for AmpduRate<'_, '_, K> {}
impl<K> Clone for AmpduRate<'_, '_, K> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<'p, 'id, K> AmpduRate<'p, 'id, K> {
    /// The rate.
    pub const fn rate(&self) -> &PhaseRate<'p, 'id, K> {
        &self.0
    }
}

/// A builder owns its receiver, TID, role and ONE protection. A class pool's
/// [`TxBufferPool::push_aggregate`] prepares fresh subframes through that
/// builder; it never accepts an independently protected prepared MPDU.
/// Sent subframes must retain the same installed key, even after slot reuse.
///
/// A fresh source with its own protection cannot enter the builder:
///
/// ```compile_fail,E0308
/// use oer_ieee80211_lower_mac::*;
/// fn independent<'k, 'p, 'id: 'k + 'p, T, C, R, K, S>(
///     builder: T::Aggregate<'k, 'p, R, K, S>,
///     source: PoolSource<'k, 'p, 'id, T, C, K, R>,
/// ) where T: TxBufferPool<'id, C>, C: TxBufferClass,
///     R: RoleKeys<'id, T>, K: Ieee80211Capabilities, S: AggregateState,
/// {
///     let _ = CheckedAggregate::new(builder, source);
/// }
/// ```
///
/// Its compiling twin has no independent protection in the source:
///
/// ```
/// use oer_ieee80211_lower_mac::*;
/// fn through_builder<'k, 'p, 'id: 'k + 'p, T, C, R, K, S>(
///     builder: T::Aggregate<'k, 'p, R, K, S>,
///     source: PoolSubframe<'id, T, C, K>,
/// ) -> AggregatePushResult<'k, 'p, 'id, T, C, R, K, S>
/// where T: TxBufferPool<'id, C>, C: TxBufferClass,
///     R: RoleKeys<'id, T>, K: Ieee80211Capabilities, S: AggregateState,
/// { T::push_aggregate(CheckedAggregate::new(builder, source).ok().unwrap()) }
/// ```
pub trait PhaseAggregate<'id> {
    /// The backend resource family.
    type Types: PhaseTypes<'id>;
    /// Whether this builder owns any subframes.
    type State: AggregateState;
    /// Backend execution metadata, separate from the complete aggregate.
    type Execution;
    /// Lower the nonempty builder before backend custody. Empty builders
    /// have cancellation only and cannot invoke this operation.
    fn into_admission(
        self,
    ) -> (
        <Self::Types as PhaseTypes<'id>>::TxAggregate,
        Self::Execution,
    )
    where
        Self::State: NonEmptyState;
    /// Cancellation releases each complete MPDU, including for an empty
    /// builder. It does not manufacture an admitted nonempty aggregate.
    type Cancelled: IntoIterator<Item = <Self::Types as PhaseTypes<'id>>::TxMpdu>;
    /// The receiver fixed when this builder was created.
    fn receiver(&self) -> super::IndividualAddress;
    /// The TID fixed when this builder was created.
    fn tid(&self) -> Tid;
    /// The encryption fixed for all fresh subframes by this builder's key.
    fn protection(&self) -> crate::MpduProtection;
    /// Number of subframes currently held.
    fn subframes(&self) -> usize;
    /// Cancel preparation, recovering all original MPDUs and fresh bodies.
    /// No ticket was taken and no TX terminal is created.
    fn into_parts(self) -> Self::Cancelled;
}

/// The minimum MPDU start spacing of an aggregate, as the 802.11 encoding
/// 0 to 7.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MpduStartSpacing(u8);

impl MpduStartSpacing {
    /// `encoding`, when it is 0 to 7.
    pub const fn new(encoding: u8) -> Option<Self> {
        if encoding <= 7 {
            Some(Self(encoding))
        } else {
            None
        }
    }

    /// The encoding.
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// One A-MPDU attempt: its ticket, its aggregate and how it goes.
pub struct AggregateAttempt<'id, T, A, K, X, Path: PoolPath = Root> {
    /// The caller's context for all subframes, returned with their terminal.
    pub context: X,
    /// The attempt's place in flight.
    pub ticket: Slot<'id, T, Path>,
    /// The aggregate, whose subframes' buffers come back with the terminal.
    pub aggregate: A,
    /// The control exchange that protects it.
    pub protection: Protection,
    /// Its transmit power.
    pub power: SupportedPower<K>,
    /// The recipient's minimum MPDU start spacing.
    pub min_mpdu_start_spacing: MpduStartSpacing,
}

/// A complete aggregate sealed before backend custody, retaining its original
/// preparation's phase, installation and key borrow until submission.
pub struct AdmittedAggregate<'id, T, A: PhaseAggregate<'id>, K, X, Path: PoolPath = Root> {
    /// Indivisible owner of every submitted MPDU and body.
    pub operation:
        crate::InFlight<X, Slot<'id, T, Path>, <A::Types as PhaseTypes<'id>>::TxAggregate>,
    /// Backend execution metadata, without owning subframes.
    pub execution: A::Execution,
    /// Protection selected by the caller.
    pub protection: Protection,
    /// Transmit power selected by the caller.
    pub power: SupportedPower<K>,
    /// Minimum MPDU start spacing of the recipient.
    pub min_mpdu_start_spacing: MpduStartSpacing,
    proof: core::marker::PhantomData<fn(A) -> A>,
}

impl<'id, T, A: PhaseAggregate<'id, State = NonEmptyAggregate>, K, X, Path: PoolPath>
    AggregateAttempt<'id, T, A, K, X, Path>
{
    /// Seal every subframe together before passing the attempt to a backend.
    pub fn admit(self) -> AdmittedAggregate<'id, T, A, K, X, Path> {
        let (payload, execution) = self.aggregate.into_admission();
        AdmittedAggregate {
            operation: crate::InFlight::admit(self.context, self.ticket, payload),
            execution,
            protection: self.protection,
            power: self.power,
            min_mpdu_start_spacing: self.min_mpdu_start_spacing,
            proof: core::marker::PhantomData,
        }
    }
}

/// The aggregate attempt of this interface, including its context and
/// every subframe. A preparation refusal returns this same value.
pub type InterfaceAggregateAttempt<'k, 'p, 'id, I> = AggregateAttempt<
    'id,
    <<I as InterfacePhase<'p, 'id>>::Types as PhaseTypes<'id>>::TicketKind,
    InterfaceAggregate<'k, 'p, 'id, I, NonEmptyAggregate>,
    <I as InterfacePhase<'p, 'id>>::Caps,
    <<I as InterfacePhase<'p, 'id>>::Types as PhaseTypes<'id>>::Context,
    <<I as InterfacePhase<'p, 'id>>::Types as PhaseTypes<'id>>::TicketPath,
>;

/// A sealed nonempty aggregate, as received by an interface backend.
pub type InterfaceAggregateSubmission<'k, 'p, 'id, I> = AdmittedAggregate<
    'id,
    <<I as InterfacePhase<'p, 'id>>::Types as PhaseTypes<'id>>::TicketKind,
    InterfaceAggregate<'k, 'p, 'id, I, NonEmptyAggregate>,
    <I as InterfacePhase<'p, 'id>>::Caps,
    <<I as InterfacePhase<'p, 'id>>::Types as PhaseTypes<'id>>::Context,
    <<I as InterfacePhase<'p, 'id>>::Types as PhaseTypes<'id>>::TicketPath,
>;

/// An interface's builder, with its role, phase, install and occupancy state.
pub type InterfaceAggregate<'k, 'p, 'id, I, S> =
    <<I as InterfacePhase<'p, 'id>>::Types as PhaseTypes<'id>>::Aggregate<
        'k,
        'p,
        <I as InterfacePhase<'p, 'id>>::Role,
        <I as InterfacePhase<'p, 'id>>::Caps,
        S,
    >;
/// The protection this interface's role may use.
pub type InterfaceProtection<'k, 'p, 'id, I> = TxProtection<
    <<I as InterfacePhase<'p, 'id>>::Role as RoleKeys<
        'id,
        <I as InterfacePhase<'p, 'id>>::Types,
    >>::TxKey<'k>,
>;

/// An interface that sends A-MPDUs. Empty builders cannot be submitted.
///
/// An empty builder cannot be submitted:
///
/// ```compile_fail,E0599
/// use oer_ieee80211_lower_mac::*;
/// fn empty<'k, 'p, 'id: 'k + 'p, I: AmpduPhase<'p, 'id>>(
///     port: &mut I,
///     attempt: AggregateAttempt<'id, <I::Types as PhaseTypes<'id>>::TicketKind,
///         InterfaceAggregate<'k, 'p, 'id, I, EmptyAggregate>, I::Caps,
///         <I::Types as PhaseTypes<'id>>::Context>,
/// ) { let _ = port.submit_ampdu(attempt.admit()); }
/// ```
pub trait AmpduPhase<'p, 'id: 'p>: InterfacePhase<'p, 'id> {
    /// What the backend aggregates.
    fn ampdu_capabilities(&self) -> AmpduCapabilities;

    /// Check that the backend aggregates at this phase's rate.
    fn ampdu_rate(
        &self,
        rate: PhaseRate<'p, 'id, Self::Caps>,
    ) -> Result<AmpduRate<'p, 'id, Self::Caps>, PhaseRate<'p, 'id, Self::Caps>> {
        if self.ampdu_capabilities().formats.contains_rate(rate.get()) {
            Ok(AmpduRate(rate))
        } else {
            Err(rate)
        }
    }

    /// An empty builder with one receiver, TID, role and protection.
    /// Every fresh subframe is prepared through these fixed values.
    fn aggregate<'k>(
        &mut self,
        receiver: super::IndividualAddress,
        tid: Tid,
        rate: AmpduRate<'p, 'id, Self::Caps>,
        protection: InterfaceProtection<'k, 'p, 'id, Self>,
    ) -> InterfaceAggregate<'k, 'p, 'id, Self, EmptyAggregate>
    where
        'id: 'k;

    /// Submit a nonempty aggregate. All preparation refusals happen at push;
    /// only poisoning can refuse this operation, with the usual fail-stop.
    fn submit_ampdu<'k>(
        &mut self,
        attempt: InterfaceAggregateSubmission<'k, 'p, 'id, Self>,
    ) -> Result<(), Poisoned<<Self::Types as PhaseTypes<'id>>::Fault>>
    where
        'id: 'k;
}

/// The TBTT schedule of an interface: the wake plan the backend carries
/// out at each TBTT.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TbttPlan {
    /// The TSF of the next TBTT.
    pub next: TsfInstant,
    /// The beacon interval.
    pub beacon_interval: TsfDuration,
    /// How far ahead of each TBTT its event fires.
    pub lead: TsfDuration,
    /// How far ahead of each TBTT the radio wakes to receive its beacon.
    pub wake_ahead: TsfDuration,
    /// The window the radio keeps open for the beacon, which it asks the air
    /// for as [`Operation::BeaconWindow`](crate::Operation::BeaconWindow);
    /// `None` keeps no window.
    pub beacon_window: Option<BeaconWindow>,
}

/// The window after a TBTT the radio keeps open for the beacon.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BeaconWindow {
    /// How long the window stays open after the TBTT.
    pub duration: TsfDuration,
}

/// An interface with a TSF and a TBTT schedule. Its settings take effect
/// before the next submission; [`Self::sample_tsf`]'s answer and each
/// TBTT arrive as events.
pub trait BeaconTimingPhase<'p, 'id: 'p>: InterfacePhase<'p, 'id> {
    /// Ask for a paired reading of the interface's TSF and the port's radio
    /// clock, which [`LowerMacTerminal::TsfSampled`](crate::LowerMacTerminal::TsfSampled)
    /// answers.
    fn sample_tsf(
        &mut self,
        permission: crate::TsfPermission<
            'id,
            Self::Role,
            <Self::Role as crate::RoleKeys<'id, Self::Types>>::CommandPath,
        >,
    );

    /// Set the interface's TSF to `at`.
    fn set_tsf(&mut self, at: TsfInstant);

    /// Follow `plan`, in place of the earlier one.
    fn set_tbtt(&mut self, plan: TbttPlan);

    /// Stop the TBTT schedule.
    fn stop_tbtt(&mut self);
}

/// A phase whose gate's closing can take one last frame that reserves the
/// air: a CTS-to-self of the access point, which its peers honour by
/// keeping quiet while the station beside it is away.
pub trait AirReservationPhase<'p, 'id: 'p>: LowerMacPhase<'p, 'id> {
    /// Close the gate as [`LowerMacPhase::close_gate`] does, and after the
    /// published attempts end, before
    /// [`LowerMacTerminal::GateClosed`](crate::LowerMacTerminal::GateClosed),
    /// send the access point's CTS-to-self reserving the air until `until`:
    /// its Duration is `min(until − now, MAX_RESERVATION)` when it goes,
    /// and no frame goes once no time is left.
    fn close_gate_reserving(
        &mut self,
        gate: GateOpen<'id, <Self::Types as PhaseTypes<'id>>::GatePath>,
        until: crate::Ieee80211Instant,
    );
}

/// The longest reservation a CTS-to-self's Duration field carries.
pub const MAX_RESERVATION_MICROS: u16 = 32_767;

/// A phase that receives as a monitor.
pub trait MonitorPhase<'p, 'id: 'p>: LowerMacPhase<'p, 'id> {
    /// Receive every frame the radio hears, or only the interfaces' own.
    fn set_monitor(&mut self, enabled: bool);
}
