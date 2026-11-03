//! The data transmit path on two arenas ([`TxArena`]) over the core's two
//! retained A-MPDU descriptor chains.
//!
//! [`Esp32s31TxArena`] owns the chains, one admission ticket per arena and
//! each arena's phase; [`ArenaPort`] lends it the core, the hardware, the
//! free EDCA queues, the interface's pairwise key, the core's sink and the
//! contention observer for one synchronous step, and is the port's
//! [`TxArena`].
//!
//! Intake moves the caller's memory into the chain's next retention slot and
//! binds its descriptor word there once
//! ([`RetainedDmaAmpduTx::intake`](crate::mac::tx::ampdu::RetainedDmaAmpduTx::intake)):
//! the MPDU is written where it is published, after the eight-byte private
//! prefix, and every publication of the arena, a retry included, relinks
//! the bound words. Publication checks what the hardware cannot take (a
//! Protected bit the encryption contradicts, an A-MPDU past its rate's octet
//! limit), admits the interface's pairwise key for an encrypted arena, draws
//! its backoff and programs an HT or HE A-MPDU of every frame, or position 0
//! alone as a single MPDU at a legacy, HT or HE rate, sent again by its
//! retry plan (`SingleRetry`) until an ACK or the plan's end. The key right stays
//! with the arena while any frame it encrypted is on the air or retained;
//! only a release or withdrawal of every frame returns it.
//!
//! A completion acknowledges the hardware edge, detaches the chain and maps
//! the end: a valid BlockAck
//! ([`HtAmpduTxCompletion::valid_block_ack`]) answers an A-MPDU, a
//! completion without one is a missing response, a CTS timeout a protection
//! failure, a collision detach a collision and a hardware timeout an
//! aborted publication. Each publication's watchdog is the only deadline.

use super::{ArenaValues, ContentionDraw};
use super::{
    CcmpKeyHardware, ESP32S31_AMPDU_MAX_LENGTH, Esp32s31AmpduOwner, FreeQueue, KeyHandle,
    LOWER_MAC_TX_QUEUES, LowerMacCore, LowerMacFault, LowerMacSink, RadioCoexPriorities,
    TxAdmission, TxTicket,
};
use crate::mac::irq::EVENT_TX_TIMEOUT;
use crate::mac::tx::{
    CompletionObservation, LegacyTxQueue, TxCompletion, TxCookie, TxDmaState, TxPhyRate,
    ampdu::{AmpduIntakeRefused, HtAmpduHardware, HtAmpduTxCompletion, HtAmpduTxError},
};
use crate::ordinary_tx::{TX_ABORT_SETTLE, WifiTxEntropy, WifiTxPowerProfile};
use crate::tx::WifiTxWake;
use core::{convert::Infallible, marker::PhantomData, num::NonZeroU16};
use oer_chips_esp32s31_hal::memory::dma_static::PlacedBacking;
use oer_portable_events::EventSink;
use oer_portable_time::Instant;
use oer_protocols_ieee80211_contracts_port::arena::{
    ARENA_FRAMES, Arena, ArenaFormat, ArenaId, ArenaPublication, AttemptEnd, Completion, FrameMask,
    IntakeRefused, PublishError, PublishRefused, Settlement, TxArena,
};
use oer_protocols_ieee80211_contracts_port::{
    Air, BlockAckSnr, Ieee80211Capabilities, MpduProtection, PhyRate, Protection, RetryPlan,
    SnrSample, TxPower, TxStatus, VifRole,
};
use oer_protocols_radio_port::PoolPath;

/// What the S31 observed of an arena's ACK or BlockAck: its SNR, when the
/// completion carried a sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockAckAnswer {
    pub snr_db: Option<i8>,
}

impl BlockAckSnr for BlockAckAnswer {
    fn snr_sample(&self) -> SnrSample {
        match self.snr_db {
            Some(snr_db) => SnrSample::Measured(snr_db),
            None => SnrSample::Unmeasured,
        }
    }
}

/// Why the arenas quarantined themselves; every chain, ticket, key right
/// and queue they hold stays with them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArenaFault {
    /// An aggregate owner refused a lifecycle or range edge.
    Chain(HtAmpduTxError),
    /// The core is poisoned.
    Core(LowerMacFault),
    /// An operation disagreed with the arena's phase: a settlement or
    /// withdrawal of an arena not in it, a rate outside the S31's formats,
    /// a missing admission or a queue place already taken.
    State,
    /// The publication watchdog expired and no timeout abort could begin.
    Watchdog,
}

impl From<HtAmpduTxError> for ArenaFault {
    fn from(error: HtAmpduTxError) -> Self {
        Self::Chain(error)
    }
}

/// A published arena on its EDCA queue.
struct OnAir<'core, Path: PoolPath> {
    queue: FreeQueue<'core, Path>,
    /// The caller's right, held until the completion.
    arena: Arena,
    /// Frames the PPDU carries: every frame, or 1 for a single MPDU.
    published: u8,
    /// Whether the PPDU is a single MPDU.
    single: bool,
    /// A single MPDU's retry plan and its progress.
    retry: Option<SingleRetry>,
    rate: PhyRate,
    role: VifRole,
    /// The publication watchdog, or the end of the abort settle.
    deadline: Instant,
    settling: bool,
}

/// A published single MPDU's retry plan, run as the core runs an MPDU's
/// (`port::driver` `collect`): each completion without an ACK, a
/// collision or a protection failure takes one try of the current stage,
/// and the frame goes again at the stage's rate, with its Retry bit, until
/// an ACK or the plan's end.
#[derive(Clone, Copy)]
struct SingleRetry {
    plan: RetryPlan<PhyRate>,
    /// The publication's values, for each retransmission.
    values: ArenaValues,
    stage: usize,
    stage_tries: u8,
    /// Transmissions that reached the air, and the rate of the last.
    transmissions: u16,
    air_rate: Option<PhyRate>,
}

/// What a single MPDU's completion leaves of its plan.
enum Retried {
    /// Send it again at this stage's rate and protection.
    Again {
        rate: PhyRate,
        protection: Protection,
    },
    /// The plan ended: the arena's end.
    End(AttemptEnd<BlockAckAnswer, Infallible>),
}

impl SingleRetry {
    /// The air of every transmission that went out.
    fn air(&self) -> Option<Air> {
        Some(Air {
            transmissions: NonZeroU16::new(self.transmissions)?,
            rate: self.air_rate?,
        })
    }

    /// Account `end`, a completion of a publication at `rate`.
    fn observe(
        &mut self,
        end: AttemptEnd<BlockAckAnswer, Infallible>,
        rate: PhyRate,
    ) -> Result<Retried, ArenaFault> {
        let sent = matches!(
            end,
            AttemptEnd::Acked { .. } | AttemptEnd::Answered { .. } | AttemptEnd::NoResponse(_)
        );
        if sent {
            self.transmissions = self.transmissions.checked_add(1).ok_or(ArenaFault::State)?;
            self.air_rate = Some(rate);
        }
        match end {
            AttemptEnd::Acked { response, .. } => {
                let air = self.air().ok_or(ArenaFault::State)?;
                return Ok(Retried::End(AttemptEnd::Acked { air, response }));
            }
            // A hardware timeout is final, as an MPDU's is.
            AttemptEnd::Aborted | AttemptEnd::Answered { .. } | AttemptEnd::AirDenied { .. } => {
                return Ok(Retried::End(end));
            }
            AttemptEnd::NoResponse(_)
            | AttemptEnd::ProtectionFailure(_)
            | AttemptEnd::Collision(_) => {}
        }
        self.stage_tries = self.stage_tries.checked_add(1).ok_or(ArenaFault::State)?;
        let current = self
            .plan
            .stages()
            .nth(self.stage)
            .ok_or(ArenaFault::State)?;
        if self.stage_tries >= current.transmissions.get() {
            self.stage = self.stage.saturating_add(1);
            self.stage_tries = 0;
        }
        if let Some(next) = self.plan.stages().nth(self.stage) {
            return Ok(Retried::Again {
                rate: next.rate,
                protection: next.protection,
            });
        }
        let air = self.air();
        Ok(Retried::End(match end {
            AttemptEnd::NoResponse(_) => AttemptEnd::NoResponse(air.ok_or(ArenaFault::State)?),
            AttemptEnd::ProtectionFailure(_) => AttemptEnd::ProtectionFailure(air),
            _ => AttemptEnd::Collision(air),
        }))
    }
}

enum Phase<'core, Path: PoolPath> {
    /// Filled, retained or empty: the caller holds the right.
    Idle,
    OnAir(OnAir<'core, Path>),
    /// Ended and detached; awaits its settlement. Positions
    /// `0..published` went into its PPDU.
    Completed {
        published: u8,
    },
}

struct Chain<'core, 'slot, L, Path: PoolPath, const SLOTS: usize> {
    owner: Esp32s31AmpduOwner<'slot, L, SLOTS>,
    /// The batch the owner holds while it is not free.
    cookie: TxCookie,
    /// The arena's ticket, or the key right it became.
    admission: Option<TxAdmission<'core, Path>>,
    phase: Phase<'core, Path>,
    len: u8,
}

impl<'core, 'slot, L: PlacedBacking, Path: PoolPath, const SLOTS: usize>
    Chain<'core, 'slot, L, Path, SLOTS>
{
    /// Whether every frame position to publish carries a Protected bit
    /// matching `encrypted`.
    fn protection_matches(&self, published: u8, encrypted: bool) -> bool {
        (0..usize::from(published)).all(|position| {
            self.owner
                .frame_size(position)
                .is_some_and(|(_, mic)| (mic != 0) == encrypted)
        })
    }
}

/// The S31's two TX arenas over two retained A-MPDU chains.
pub struct Esp32s31TxArena<'core, 'slot, L, Path: PoolPath, const SLOTS: usize> {
    chains: [Chain<'core, 'slot, L, Path, SLOTS>; 2],
    claimed: bool,
    fault: Option<ArenaFault>,
}

/// The chains and tickets an [`Esp32s31TxArena`] is made of.
pub type ArenaParts<'core, 'slot, L, Path, const SLOTS: usize> = (
    [Esp32s31AmpduOwner<'slot, L, SLOTS>; 2],
    [TxTicket<'core, Path>; 2],
);

impl<'core, 'slot, L: PlacedBacking, Path: PoolPath, const SLOTS: usize>
    Esp32s31TxArena<'core, 'slot, L, Path, SLOTS>
{
    /// The slots of each arena.
    pub const CAPACITY: u8 = {
        if SLOTS < 2 || SLOTS > ARENA_FRAMES as usize {
            panic!("an arena chain holds 2 to ARENA_FRAMES frames");
        }
        SLOTS as u8
    };

    /// Two idle arenas over two idle chains, each with its admission
    /// ticket; the chains take the widest aggregate, each publication's
    /// rate bounding it ([`super::esp32s31_ampdu_length`]).
    ///
    /// # Errors
    ///
    /// Every part back, when a chain is not idle.
    #[allow(clippy::result_large_err, reason = "the parts go back by value")]
    pub fn new(
        chains: [Esp32s31AmpduOwner<'slot, L, SLOTS>; 2],
        tickets: [TxTicket<'core, Path>; 2],
    ) -> Result<Self, ArenaParts<'core, 'slot, L, Path, SLOTS>> {
        let _capacity = Self::CAPACITY;
        let [mut first, mut second] = chains;
        let idle = [&mut first, &mut second].into_iter().all(|owner| {
            owner.is_fully_free()
                && owner
                    .configure_max_aggregate_bytes(ESP32S31_AMPDU_MAX_LENGTH)
                    .is_ok()
        });
        if !idle {
            return Err(([first, second], tickets));
        }
        let [one, two] = tickets;
        let chain = |owner, ticket| Chain {
            owner,
            cookie: TxCookie(0),
            admission: Some(TxAdmission::Open(ticket)),
            phase: Phase::Idle,
            len: 0,
        };
        Ok(Self {
            chains: [chain(first, one), chain(second, two)],
            claimed: false,
            fault: None,
        })
    }

    /// The chains and tickets back, when both arenas are idle and empty,
    /// hold no key right and never faulted.
    ///
    /// # Errors
    ///
    /// The arenas, unchanged.
    #[allow(clippy::result_large_err, reason = "the arenas go back by value")]
    pub fn into_parts(self) -> Result<ArenaParts<'core, 'slot, L, Path, SLOTS>, Self> {
        let returnable = self.fault.is_none()
            && self.chains.iter().all(|chain| {
                matches!(chain.phase, Phase::Idle)
                    && chain.owner.is_fully_free()
                    && matches!(chain.admission, Some(TxAdmission::Open(_)))
            });
        if !returnable {
            return Err(self);
        }
        let [first, second] = self.chains;
        match (first.admission, second.admission) {
            (Some(TxAdmission::Open(one)), Some(TxAdmission::Open(two))) => {
                Ok(([first.owner, second.owner], [one, two]))
            }
            // Checked above: both arenas hold their ticket.
            (one, two) => Err(Self {
                chains: [
                    Chain {
                        admission: one,
                        ..first
                    },
                    Chain {
                        admission: two,
                        ..second
                    },
                ],
                claimed: self.claimed,
                fault: Some(ArenaFault::State),
            }),
        }
    }

    /// The fault the arenas quarantined themselves for.
    pub const fn fault(&self) -> Option<ArenaFault> {
        self.fault
    }

    /// The chain of `arena`, for host observation.
    #[cfg(test)]
    pub(crate) fn chain_mut(
        &mut self,
        arena: ArenaId,
    ) -> Option<&mut Esp32s31AmpduOwner<'slot, L, SLOTS>> {
        self.chains
            .get_mut(arena.index())
            .map(|chain| &mut chain.owner)
    }

    /// The arena on the air, the frames its PPDU carries and whether it is
    /// a single MPDU, for host observation.
    #[cfg(test)]
    pub(crate) fn on_air_shape(&self) -> Option<(ArenaId, u8, bool)> {
        let id = self.on_air()?;
        match &self.chains.get(id.index())?.phase {
            Phase::OnAir(on_air) => Some((id, on_air.published, on_air.single)),
            Phase::Idle | Phase::Completed { .. } => None,
        }
    }

    /// Whether `arena` holds a key right, for host observation.
    #[cfg(test)]
    pub(crate) fn holds_key(&self, arena: ArenaId) -> bool {
        self.chains
            .get(arena.index())
            .is_some_and(|chain| matches!(chain.admission, Some(TxAdmission::Protected(_))))
    }

    /// Whether an arena holds a key right: a frame it encrypted is on the
    /// air, awaits settlement or is retained. A key's removal waits for it.
    pub fn holds_key_right(&self) -> bool {
        self.chains
            .iter()
            .any(|chain| matches!(chain.admission, Some(TxAdmission::Protected(_))))
    }

    fn quarantine(&mut self, fault: ArenaFault) -> ArenaFault {
        *self.fault.get_or_insert(fault)
    }

    /// The arena on the air: published and not yet completed.
    pub fn on_air(&self) -> Option<ArenaId> {
        [ArenaId::First, ArenaId::Second].into_iter().find(|id| {
            self.chains
                .get(id.index())
                .is_some_and(|chain| matches!(chain.phase, Phase::OnAir(_)))
        })
    }

    /// Whether the arena on the air waits for its timeout abort to settle.
    pub fn settling(&self) -> bool {
        self.chains
            .iter()
            .any(|chain| matches!(&chain.phase, Phase::OnAir(on_air) if on_air.settling))
    }

    /// The published arena's hardware deadline: its watchdog, or the end of
    /// its abort settle.
    pub fn deadline(&self) -> Option<Instant> {
        self.chains.iter().find_map(|chain| match &chain.phase {
            Phase::OnAir(on_air) => Some(on_air.deadline),
            Phase::Idle | Phase::Completed { .. } => None,
        })
    }

    /// The frames `arena` holds.
    pub fn len(&self, arena: ArenaId) -> u8 {
        self.chains.get(arena.index()).map_or(0, |chain| chain.len)
    }

    /// Claim the two idle arenas: `true` exactly once.
    pub fn claim(&mut self) -> bool {
        !core::mem::replace(&mut self.claimed, true)
    }

    /// Add a frame at the next position of `arena`, as [`TxArena::push`]
    /// does: the arenas alone take it.
    ///
    /// # Errors
    ///
    /// The memory and why the frame was refused.
    pub fn push(
        &mut self,
        arena: ArenaId,
        memory: L,
        write: impl FnOnce(&mut [u8]) -> Option<u16>,
    ) -> Result<(), (L, IntakeRefused)> {
        if self.fault.is_some() {
            return Err((memory, IntakeRefused::Quarantined));
        }
        let Some(chain) = self.chains.get_mut(arena.index()) else {
            return Err((memory, IntakeRefused::Busy));
        };
        if !matches!(chain.phase, Phase::Idle) {
            return Err((memory, IntakeRefused::Busy));
        }
        if chain.len >= Self::CAPACITY {
            return Err((memory, IntakeRefused::Full));
        }
        if chain.owner.state() == TxDmaState::Free {
            match chain.owner.begin() {
                Ok(cookie) => chain.cookie = cookie,
                Err(error) => {
                    self.quarantine(ArenaFault::Chain(error));
                    return Err((memory, IntakeRefused::Quarantined));
                }
            }
        }
        match chain.owner.intake(chain.cookie, memory, write) {
            Ok(()) => {
                chain.len = chain.len.saturating_add(1);
                Ok(())
            }
            Err((memory, refused)) => {
                let refused = match refused {
                    AmpduIntakeRefused::Declined => IntakeRefused::Declined,
                    AmpduIntakeRefused::Length => IntakeRefused::Length,
                    AmpduIntakeRefused::Full => IntakeRefused::Full,
                    AmpduIntakeRefused::Fault(error) => {
                        self.quarantine(ArenaFault::Chain(error));
                        IntakeRefused::Quarantined
                    }
                };
                Err((memory, refused))
            }
        }
    }
}

/// One synchronous step's view of the arenas: what the S31 lends them to
/// publish, serve and settle. It is the port's [`TxArena`] over the
/// capabilities `K`.
pub struct ArenaPort<
    'a,
    'core,
    'slot,
    P,
    E,
    T,
    C: RadioCoexPriorities,
    H,
    S,
    O,
    L,
    K,
    const BUFFER_SIZE: usize,
    Path: PoolPath,
    const TICKETS: usize,
    const SLOTS: usize,
> {
    pub arenas: &'a mut Esp32s31TxArena<'core, 'slot, L, Path, SLOTS>,
    pub(in crate::lower_mac) core:
        &'a mut LowerMacCore<'core, 'slot, P, E, T, C, BUFFER_SIZE, Path, TICKETS>,
    pub hardware: &'a mut H,
    /// The free EDCA queues by hardware index; a publication holds its
    /// category's until its completion.
    pub queues: &'a mut [Option<FreeQueue<'core, Path>>; LOWER_MAC_TX_QUEUES],
    /// The interface the arenas transmit for.
    pub role: VifRole,
    /// The interface's installed pairwise key.
    pub pairwise: Option<&'a KeyHandle<'core, Path>>,
    /// Where a key removal the last release finishes is reported.
    pub sink: &'a mut S,
    pub observations: &'a mut O,
    pub caps: PhantomData<fn() -> K>,
}

impl<
    'a,
    'core,
    'slot,
    P,
    E,
    T,
    C: RadioCoexPriorities,
    H,
    S,
    O,
    L,
    K,
    const BUFFER_SIZE: usize,
    Path: PoolPath,
    const TICKETS: usize,
    const SLOTS: usize,
> ArenaPort<'a, 'core, 'slot, P, E, T, C, H, S, O, L, K, BUFFER_SIZE, Path, TICKETS, SLOTS>
{
    /// One step's view of `arenas` over the core a phase lends them
    /// ([`CoreView::arena_core`](super::CoreView::arena_core)), for the
    /// interface `role` and its installed `pairwise` key.
    #[allow(clippy::too_many_arguments, reason = "every part the arenas borrow")]
    pub fn new(
        arenas: &'a mut Esp32s31TxArena<'core, 'slot, L, Path, SLOTS>,
        core: super::ArenaCore<
            'a,
            LowerMacCore<'core, 'slot, P, E, T, C, BUFFER_SIZE, Path, TICKETS>,
        >,
        hardware: &'a mut H,
        queues: &'a mut [Option<FreeQueue<'core, Path>>; LOWER_MAC_TX_QUEUES],
        role: VifRole,
        pairwise: Option<&'a KeyHandle<'core, Path>>,
        sink: &'a mut S,
        observations: &'a mut O,
    ) -> Self {
        Self {
            arenas,
            core: core.core,
            hardware,
            queues,
            role,
            pairwise,
            sink,
            observations,
            caps: PhantomData,
        }
    }
}

/// The end of a published chain's one hardware edge.
type Step = Result<Option<AttemptEnd<BlockAckAnswer, Infallible>>, ArenaFault>;

/// An A-MPDU completion's end at `rate`.
fn aggregate_end(
    completion: HtAmpduTxCompletion,
    air: Air,
) -> AttemptEnd<BlockAckAnswer, Infallible> {
    let response = BlockAckAnswer {
        snr_db: completion.tx.ack_snr_sample(),
    };
    if let Some(block_ack) = completion.valid_block_ack() {
        return AttemptEnd::Answered {
            air,
            report: block_ack.report(),
            response,
        };
    }
    lost_end(completion.tx.tx_status(), air)
}

/// A single MPDU's completion end.
fn single_end(completion: TxCompletion, air: Air) -> AttemptEnd<BlockAckAnswer, Infallible> {
    match completion.tx_status() {
        TxStatus::Success => AttemptEnd::Acked {
            air,
            response: BlockAckAnswer {
                snr_db: completion.ack_snr_sample(),
            },
        },
        status => lost_end(status, air),
    }
}

/// The end of a completion without its solicited response.
fn lost_end(status: TxStatus, air: Air) -> AttemptEnd<BlockAckAnswer, Infallible> {
    match status {
        TxStatus::Success | TxStatus::AckTimeout => AttemptEnd::NoResponse(air),
        // The protection exchange drew no CTS: no frame went out.
        TxStatus::CtsTimeout => AttemptEnd::ProtectionFailure(None),
        TxStatus::Collision => AttemptEnd::Collision(None),
        TxStatus::TimedOut => AttemptEnd::Aborted,
    }
}

/// Serve one hardware edge of the published `chain`, as the aggregate
/// service does ([`super::ampdu::service`]), keeping every frame.
fn serve<L: PlacedBacking, H: HtAmpduHardware, Path: PoolPath, const SLOTS: usize>(
    hardware: &mut H,
    owner: &mut Esp32s31AmpduOwner<'_, L, SLOTS>,
    cookie: TxCookie,
    on_air: &mut OnAir<'_, Path>,
    wake: WifiTxWake,
    may_begin_timeout_abort: bool,
    now: Instant,
) -> Step {
    if on_air.settling {
        if now < on_air.deadline {
            return Ok(None);
        }
        owner.finish_timeout_abort_retained(hardware, cookie)?;
        return Ok(Some(AttemptEnd::Aborted));
    }
    let air = Air {
        transmissions: NonZeroU16::MIN,
        rate: on_air.rate,
    };
    let end = if on_air.single {
        match owner.acknowledge_single_completion(hardware)? {
            Some(CompletionObservation::Observed(completion)) => Some(single_end(completion, air)),
            Some(CompletionObservation::NoEffect) | None => None,
        }
    } else {
        match owner.acknowledge_completion(hardware)? {
            Some(CompletionObservation::Observed(completion)) => {
                Some(aggregate_end(completion, air))
            }
            Some(CompletionObservation::NoEffect) | None => None,
        }
    };
    if let Some(end) = end {
        owner.detach_completed(hardware, cookie)?;
        return Ok(Some(end));
    }
    let events = match wake {
        WifiTxWake::Interrupt { events } => events,
        WifiTxWake::Deadline => 0,
    };
    if owner.abort_collision_retained(hardware, cookie)? {
        return Ok(Some(AttemptEnd::Collision(None)));
    }
    let expired = now >= on_air.deadline;
    if (events & EVENT_TX_TIMEOUT != 0 || expired) && may_begin_timeout_abort {
        if owner.begin_timeout_abort(hardware, cookie)? {
            on_air.settling = true;
            on_air.deadline = now.saturating_add(TX_ABORT_SETTLE);
            return Ok(None);
        }
        if expired {
            owner.require_reset(cookie)?;
            return Err(ArenaFault::Watchdog);
        }
    }
    Ok(None)
}

impl<
    'p,
    'id: 'p,
    'core,
    'slot,
    P,
    E,
    T,
    C,
    H,
    S,
    O,
    L,
    K,
    const BUFFER_SIZE: usize,
    Path,
    const TICKETS: usize,
    const SLOTS: usize,
> TxArena<'p, 'id>
    for ArenaPort<'_, 'core, 'slot, P, E, T, C, H, S, O, L, K, BUFFER_SIZE, Path, TICKETS, SLOTS>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_portable_time::Timer,
    C: RadioCoexPriorities,
    H: HtAmpduHardware + CcmpKeyHardware,
    S: LowerMacSink<'core, 'slot, BUFFER_SIZE, Path>,
    O: EventSink<ContentionDraw>,
    L: PlacedBacking,
    K: Ieee80211Capabilities,
    Path: PoolPath,
{
    type Caps = K;
    type Memory = L;
    type Events = WifiTxWake;
    type Response = BlockAckAnswer;
    type Denial = Infallible;
    type Fault = ArenaFault;

    fn claim_arenas(&mut self) -> bool {
        self.arenas.claim()
    }

    fn capacity(&self) -> u8 {
        Esp32s31TxArena::<'core, 'slot, L, Path, SLOTS>::CAPACITY
    }

    fn push(
        &mut self,
        arena: &Arena,
        memory: L,
        write: impl FnOnce(&mut [u8]) -> Option<u16>,
    ) -> Result<(), (L, IntakeRefused)> {
        self.arenas.push(arena.id(), memory, write)
    }

    fn len(&self, arena: ArenaId) -> u8 {
        self.arenas.len(arena)
    }

    fn on_air(&self) -> Option<ArenaId> {
        self.arenas.on_air()
    }

    fn publish(
        &mut self,
        publication: ArenaPublication<'p, 'id, K>,
        now: Instant,
    ) -> Result<(), PublishError<'p, 'id, K, ArenaFault>> {
        if let Some(fault) = self.arenas.fault {
            return Err(PublishError::Fault(fault));
        }
        if let Some(fault) = self.core.fault() {
            return Err(PublishError::Fault(
                self.arenas.quarantine(ArenaFault::Core(fault)),
            ));
        }
        let id = publication.arena.id();
        let category = publication.access_category;
        let index = usize::from(LegacyTxQueue::from_access_category(category).hardware_index());
        let busy =
            self.arenas.on_air().is_some() || self.queues.get(index).is_none_or(Option::is_none);
        let encrypted = publication.encryption == MpduProtection::Ccmp;
        let Ok(rate) = TxPhyRate::try_from(publication.format.rate()) else {
            let fault = self.arenas.quarantine(ArenaFault::State);
            return Err(PublishError::Fault(fault));
        };
        let values = ArenaValues {
            role: self.role,
            access_category: category,
            rate,
            protection: publication.protection,
            power_ceiling_dbm: match publication.power.get() {
                TxPower::Calibrated => None,
                TxPower::MaxDbm(dbm) => Some(dbm),
            },
            backoff_slots: 0,
        };
        let Some(chain) = self.arenas.chains.get(id.index()) else {
            return Err(PublishError::Refused {
                publication,
                reason: PublishRefused::Busy,
            });
        };
        let (published, spacing, plan) = match publication.format {
            ArenaFormat::Single(plan) => {
                (chain.len.min(1), None, Some(plan.map(|rate| rate.get())))
            }
            ArenaFormat::Aggregate { spacing, .. } => (chain.len, Some(spacing.get()), None),
        };
        let key_held = matches!(chain.admission, Some(TxAdmission::Protected(_)));
        let budget = match spacing {
            Some(spacing) => {
                self.core
                    .arena_budget(values, spacing, SLOTS)
                    .and_then(|mut budget| {
                        for position in 0..usize::from(chain.len) {
                            let (length, mic) = chain
                                .owner
                                .frame_size(position)
                                .ok_or(super::AmpduRefusal::Length)?;
                            budget.push(
                                usize::from(length)
                                    .saturating_add(usize::from(mic))
                                    .saturating_add(crate::ordinary_tx::TX_FCS_SIZE),
                            )?;
                        }
                        Ok(Some(budget))
                    })
            }
            None => Ok(None),
        };
        let reason = if busy || !matches!(chain.phase, Phase::Idle) {
            Some(PublishRefused::Busy)
        } else if chain.len == 0 {
            Some(PublishRefused::Empty)
        } else if encrypted && !key_held && self.pairwise.is_none() {
            Some(PublishRefused::NoKey)
        } else if !chain.protection_matches(published, encrypted) {
            Some(PublishRefused::Protection)
        } else if budget.is_err() {
            Some(PublishRefused::Length)
        } else {
            None
        };
        if let Some(reason) = reason {
            return Err(PublishError::Refused {
                publication,
                reason,
            });
        }
        let he = budget.ok().flatten().and_then(|budget| budget.he_policy());
        let role = self.role;
        let backoff = self.core.draw_backoff(role, category, self.observations);
        let values = ArenaValues {
            backoff_slots: backoff,
            ..values
        };
        let timeout = self.core.publication_timeout();
        let Some(chain) = self.arenas.chains.get_mut(id.index()) else {
            return Err(PublishError::Fault(ArenaFault::State));
        };
        // Admit the pairwise key once; a retained arena keeps its right.
        if encrypted && !key_held {
            let admitted = match (chain.admission.take(), self.pairwise) {
                (Some(TxAdmission::Open(ticket)), Some(key)) => {
                    match self.core.admit_key(key, ticket) {
                        Ok(key) => Ok(TxAdmission::Protected(key)),
                        Err(refused) => Err(Some(TxAdmission::Open(refused.ticket))),
                    }
                }
                (held, _) => Err(held),
            };
            match admitted {
                Ok(admission) => chain.admission = Some(admission),
                Err(held) => {
                    chain.admission = held;
                    let fault = self
                        .core
                        .fault()
                        .map_or(ArenaFault::State, ArenaFault::Core);
                    return Err(PublishError::Fault(self.arenas.quarantine(fault)));
                }
            }
        }
        let Some(queue) = self.queues.get_mut(index).and_then(Option::take) else {
            return Err(PublishError::Fault(
                self.arenas.quarantine(ArenaFault::State),
            ));
        };
        let cookie = chain.cookie;
        let result = (|| {
            let admission = chain.admission.as_ref().ok_or(ArenaFault::State)?;
            match spacing {
                Some(spacing) => {
                    let settings = self
                        .core
                        .arena_aggregate(values, admission, spacing, he)
                        .map_err(|_| ArenaFault::State)?;
                    let delimiters = |psdu: u16| match he {
                        Some(policy) => {
                            policy.rate().ampdu_empty_delimiters(psdu, policy.density())
                        }
                        None => Some(0),
                    };
                    let length = chain.owner.prepare_format(cookie, delimiters, false)?;
                    super::ampdu::submit_planned(
                        self.hardware,
                        &mut chain.owner,
                        cookie,
                        &settings,
                        length,
                    )?;
                }
                None => {
                    let (length, mic) = chain.owner.frame_size(0).ok_or(ArenaFault::State)?;
                    let individual = chain.owner.first_receiver_individual();
                    let config = self
                        .core
                        .arena_single(values, admission, length, mic, individual)
                        .map_err(|_| ArenaFault::State)?;
                    chain.owner.prepare_format(
                        cookie,
                        |_| Some(0),
                        matches!(rate, TxPhyRate::He(_)),
                    )?;
                    chain.owner.submit_single(
                        self.hardware,
                        cookie,
                        LegacyTxQueue::from_access_category(category),
                        config,
                    )?;
                }
            }
            Ok::<(), ArenaFault>(())
        })();
        chain.phase = Phase::OnAir(OnAir {
            queue,
            arena: publication.arena,
            published,
            rate: publication.format.rate(),
            role,
            deadline: now.saturating_add(timeout),
            settling: false,
            single: spacing.is_none(),
            retry: plan.map(|plan| SingleRetry {
                plan,
                values,
                stage: 0,
                stage_tries: 0,
                transmissions: 0,
                air_rate: None,
            }),
        });
        match result {
            Ok(()) => Ok(()),
            // The arena, its queue and its admission stay quarantined.
            Err(fault) => Err(PublishError::Fault(self.arenas.quarantine(fault))),
        }
    }

    fn complete(
        &mut self,
        wake: WifiTxWake,
        now: Instant,
    ) -> Result<Option<Completion<BlockAckAnswer, Infallible>>, ArenaFault> {
        if let Some(fault) = self.arenas.fault {
            return Err(fault);
        }
        let Some(id) = self.arenas.on_air() else {
            return Ok(None);
        };
        let Some(chain) = self.arenas.chains.get_mut(id.index()) else {
            return Err(self.arenas.quarantine(ArenaFault::State));
        };
        let Phase::OnAir(on_air) = &mut chain.phase else {
            return Err(self.arenas.quarantine(ArenaFault::State));
        };
        let index = usize::from(
            LegacyTxQueue::from_access_category(on_air.queue.category()).hardware_index(),
        );
        let may_begin = self.core.begin_arena_service(index);
        let step = serve(
            self.hardware,
            &mut chain.owner,
            chain.cookie,
            on_air,
            wake,
            may_begin,
            now,
        );
        let settling = on_air.settling;
        self.core
            .end_arena_service(index, matches!(step, Ok(None)) && settling);
        let end = match step {
            Ok(Some(end)) => end,
            Ok(None) => return Ok(None),
            Err(fault) => return Err(self.arenas.quarantine(fault)),
        };
        let end = match on_air
            .retry
            .as_mut()
            .map(|retry| retry.observe(end, on_air.rate))
        {
            None => end,
            Some(Ok(Retried::End(end))) => end,
            Some(Ok(Retried::Again { rate, protection })) => {
                let sent = republish_single(
                    chain,
                    &mut *self.core,
                    &mut *self.hardware,
                    &mut *self.observations,
                    rate,
                    protection,
                    now,
                );
                return match sent {
                    Ok(()) => Ok(None),
                    Err(fault) => Err(self.arenas.quarantine(fault)),
                };
            }
            Some(Err(fault)) => return Err(self.arenas.quarantine(fault)),
        };
        let Phase::OnAir(on_air) = &mut chain.phase else {
            return Err(self.arenas.quarantine(ArenaFault::State));
        };
        let published = on_air.published;
        let Phase::OnAir(on_air) =
            core::mem::replace(&mut chain.phase, Phase::Completed { published })
        else {
            return Err(self.arenas.quarantine(ArenaFault::State));
        };
        let OnAir {
            queue, arena, role, ..
        } = on_air;
        let category = queue.category();
        let Some(place @ None) = self.queues.get_mut(index) else {
            // No place takes the queue back: it stays quarantined with the
            // completed arena's right.
            let _quarantined = core::mem::ManuallyDrop::new((queue, arena));
            return Err(self.arenas.quarantine(ArenaFault::State));
        };
        *place = Some(queue);
        self.core.reset_contention(role, category);
        Ok(Some(Completion::new(arena, chain.len, end)))
    }

    fn settle(
        &mut self,
        settlement: Settlement,
        returned: impl FnMut(L),
    ) -> Result<(), ArenaFault> {
        if let Some(fault) = self.arenas.fault {
            return Err(fault);
        }
        let Some(chain) = self
            .arenas
            .chains
            .get_mut(settlement.arena().index())
            .filter(|chain| chain.len == settlement.frames())
        else {
            return Err(self.arenas.quarantine(ArenaFault::State));
        };
        let Phase::Completed { published } = chain.phase else {
            return Err(self.arenas.quarantine(ArenaFault::State));
        };
        let keep = settlement.keep();
        // Only the frames the publication sent may have reached a receiver.
        let sent = FrameMask::first(published).unwrap_or(FrameMask::EMPTY);
        let retry = if settlement.retransmission() {
            keep.intersection(sent)
        } else {
            FrameMask::EMPTY
        };
        let result = if keep.is_empty() {
            chain.owner.release_completed(chain.cookie, returned)
        } else {
            chain
                .owner
                .retain_frames(chain.cookie, keep.bits(), retry.bits(), returned)
        };
        if let Err(error) = result {
            return Err(self.arenas.quarantine(ArenaFault::Chain(error)));
        }
        chain.len = keep.count();
        chain.phase = Phase::Idle;
        if keep.is_empty() {
            release_key(chain, self.core, self.hardware, self.sink);
        }
        Ok(())
    }

    fn withdraw(&mut self, arena: &Arena, returned: impl FnMut(L)) -> Result<(), ArenaFault> {
        if let Some(fault) = self.arenas.fault {
            return Err(fault);
        }
        let Some(chain) = self
            .arenas
            .chains
            .get_mut(arena.id().index())
            .filter(|chain| matches!(chain.phase, Phase::Idle))
        else {
            return Err(self.arenas.quarantine(ArenaFault::State));
        };
        if chain.owner.state() == TxDmaState::Reserved
            && let Err(error) = chain.owner.cancel(chain.cookie, returned)
        {
            return Err(self.arenas.quarantine(ArenaFault::Chain(error)));
        }
        chain.len = 0;
        release_key(chain, self.core, self.hardware, self.sink);
        Ok(())
    }

    fn deadline(&self) -> Option<Instant> {
        self.arenas.deadline()
    }
}

/// Send the published single MPDU of `chain` again at `rate` under
/// `protection`, as the core sends an MPDU again: every frame stays, position
/// 0 gains its Retry bit, the queue's contention window grows and a new
/// backoff is drawn; the arena stays on the air with a new watchdog.
#[allow(
    clippy::too_many_arguments,
    reason = "the parts one retransmission borrows"
)]
fn republish_single<
    'core,
    'slot,
    P,
    E,
    T,
    C,
    H,
    O,
    L,
    Path,
    const N: usize,
    const TICKETS: usize,
    const SLOTS: usize,
>(
    chain: &mut Chain<'core, 'slot, L, Path, SLOTS>,
    core: &mut LowerMacCore<'core, 'slot, P, E, T, C, N, Path, TICKETS>,
    hardware: &mut H,
    observations: &mut O,
    rate: PhyRate,
    protection: Protection,
    now: Instant,
) -> Result<(), ArenaFault>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_portable_time::Timer,
    C: RadioCoexPriorities,
    H: HtAmpduHardware,
    O: EventSink<ContentionDraw>,
    L: PlacedBacking,
    Path: PoolPath,
{
    let Phase::OnAir(on_air) = &mut chain.phase else {
        return Err(ArenaFault::State);
    };
    let first = on_air.retry.as_ref().ok_or(ArenaFault::State)?.values;
    let (role, category) = (on_air.role, on_air.queue.category());
    let cookie = chain.cookie;
    let keep = FrameMask::first(chain.len).ok_or(ArenaFault::State)?;
    // Every frame stays: no memory leaves the arena here.
    let mut left = false;
    chain
        .owner
        .retain_frames(cookie, keep.bits(), 1, |memory| {
            left = true;
            let _quarantined = core::mem::ManuallyDrop::new(memory);
        })?;
    if left {
        return Err(ArenaFault::State);
    }
    core.record_retry_failure(role, category);
    let tx_rate = TxPhyRate::try_from(rate).map_err(|_| ArenaFault::State)?;
    let values = ArenaValues {
        rate: tx_rate,
        protection,
        backoff_slots: core.draw_backoff(role, category, observations),
        ..first
    };
    let (length, mic) = chain.owner.frame_size(0).ok_or(ArenaFault::State)?;
    let individual = chain.owner.first_receiver_individual();
    let admission = chain.admission.as_ref().ok_or(ArenaFault::State)?;
    let config = core
        .arena_single(values, admission, length, mic, individual)
        .map_err(|_| ArenaFault::State)?;
    chain
        .owner
        .prepare_format(cookie, |_| Some(0), matches!(tx_rate, TxPhyRate::He(_)))?;
    chain.owner.submit_single(
        hardware,
        cookie,
        LegacyTxQueue::from_access_category(category),
        config,
    )?;
    on_air.rate = rate;
    on_air.deadline = now.saturating_add(core.publication_timeout());
    on_air.settling = false;
    Ok(())
}

/// Return the key right of `chain`, which holds no frame any more: its
/// ticket comes back, and a removal waiting for it finishes.
fn release_key<
    'core,
    'slot,
    P,
    E,
    T,
    C,
    H,
    S,
    L,
    Path,
    const N: usize,
    const TICKETS: usize,
    const SLOTS: usize,
>(
    chain: &mut Chain<'core, 'slot, L, Path, SLOTS>,
    core: &mut LowerMacCore<'core, 'slot, P, E, T, C, N, Path, TICKETS>,
    hardware: &mut H,
    sink: &mut S,
) where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_portable_time::Timer,
    C: RadioCoexPriorities,
    H: CcmpKeyHardware,
    S: LowerMacSink<'core, 'slot, N, Path>,
    Path: PoolPath,
{
    chain.admission = match chain.admission.take() {
        Some(TxAdmission::Protected(key)) => Some(TxAdmission::Open(
            core.release_arena_key(hardware, key, sink),
        )),
        held => held,
    };
}
