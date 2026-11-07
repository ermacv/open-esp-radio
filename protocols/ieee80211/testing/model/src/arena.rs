//! The model's data transmit path: two arenas of plain slot arrays behind
//! the port's [`TxArena`].
//!
//! [`ModelArenas`] holds each arena's memories and MPDU lengths by
//! position, apart from any capabilities. [`ModelArena`] is the port over
//! it with scripted answers: the standalone host arena of the arena tests
//! and the conformance runner, on [`MODEL_ARENA_CHANNEL`], and the driver's
//! view of the station's arenas. [`ModelDataArena`] is the data port's
//! view ([`ModelPort`](crate::ModelPort)'s arenas), whose answers come from
//! the air's FIFO and whose frames the air records.
//!
//! Publication records what the hardware would read
//! ([`ModelArenas::published`]). A publication whose sent frames carry a
//! Protected bit its encryption contradicts, or an A-MPDU past an optional
//! octet limit, is refused. The air answers each publication with the
//! next scripted [`ModelArenaAnswer`], and a publication without an answer
//! ends `Aborted` at its [`MODEL_ARENA_TIMEOUT`] deadline. Settlement sets
//! the Retry bit of a kept frame that may have reached the air. A fault,
//! injected or found in a disagreeing operation, quarantines the arenas:
//! every later operation fails and every memory stays where it is.

use super::{
    AirAnswer, AirFrame, AirResponse, AirTicket, AirTimeError, MODEL_ACK_SNR_DB, ModelAir,
    ModelAirDenied, ModelBlockAckEvidence, ModelComposition, ModelFault, ModelHeld,
    ModelNetworkMemory, PreparedKey, RawKeyIdentity, air_time::FCS_OCTETS, ampdu_psdu_octets,
    values::Record,
};
use alloc::{collections::VecDeque, vec::Vec};
use core::{borrow::BorrowMut, cell::RefMut, marker::PhantomData, num::NonZeroU16};
use oer_portable_time::{Duration, Instant};
use oer_protocols_ieee80211_contracts_port::arena::{
    ARENA_FRAMES, Arena, ArenaFormat, ArenaId, ArenaPublication, AttemptEnd, Completion, FrameMask,
    IntakeRefused, PublishError, PublishRefused, Settlement, TxArena,
};
use oer_protocols_ieee80211_contracts_port::{
    Air, AmpduCapabilities, BlockAckReport, Channel, ChannelWidth, Ieee80211Capabilities,
    MpduProtection, MpduStartSpacing, PhyRate, PortArena, Protection, TxPower, VifRole,
};
use oer_protocols_ieee80211_wire::ccmp::{CCMP_HEADER_LEN, CCMP_MIC_LEN, CcmpPacketNumber};
use oer_protocols_ieee80211_wire::frame::{
    FrameControlFields, MacHeader, sequence_control, set_retry_bit,
};
use oer_protocols_ieee80211_wire::qos::WmmAccessCategory;
use oer_protocols_ieee80211_wire::sequence::SequenceNumber;

/// The frames a model arena holds, unless a timed air asks for more: the
/// model's A-MPDU subframe limit.
pub const MODEL_ARENA_FRAMES: usize = super::MODEL_AMPDU_SUBFRAMES;

/// The slots of each model arena: the most any air asks for.
const SLOTS: usize = ARENA_FRAMES as usize;

/// How long a model publication waits for its answer before it is aborted.
pub const MODEL_ARENA_TIMEOUT: Duration = Duration::from_millis(10);

/// How long a publication on a timed air may wait for its channel: past any
/// exchange of the inputs a scenario uses, a medium that stopped.
pub const MODEL_TIMED_ARENA_TIMEOUT: Duration = Duration::from_secs(1);

/// The air's answer to one model publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelArenaAnswer {
    /// This BlockAck, which the model measures, answers the A-MPDU.
    BlockAck(BlockAckReport),
    /// This BlockAck answers the A-MPDU; its completion carries no SNR.
    BlockAckUnmeasured(BlockAckReport),
    /// An ACK answers the single MPDU.
    Ack,
    /// The PPDU goes out unanswered.
    NoResponse,
    /// The protection exchange draws no CTS; no frame reaches the air.
    ProtectionFailure,
    /// Internal contention ends the attempt before the air.
    Collision,
    /// Coexistence denies the air.
    AirDenied,
    /// The hardware times out at once and the publication is aborted.
    Aborted,
    /// No completion arrives; the publication is aborted at its deadline.
    Timeout,
    /// The hardware faults when the publication is served.
    Fault,
}

/// Why the model's arenas quarantined themselves.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelArenaFault {
    /// A scripted [`ModelArenaAnswer::Fault`].
    Injected,
    /// An ACK answered an A-MPDU, or a BlockAck a single MPDU.
    MismatchedAnswer,
    /// A settlement or withdrawal disagreed with the arena's state.
    State,
}

/// What the model's hardware read for one publication.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelArenaPublication {
    /// The published arena.
    pub arena: ArenaId,
    /// The A-MPDU's start spacing; `None` for a single MPDU.
    pub aggregate: Option<MpduStartSpacing>,
    /// The PPDU's rate.
    pub rate: PhyRate,
    /// The EDCA queue.
    pub access_category: WmmAccessCategory,
    /// The control exchange ahead of the PPDU.
    pub protection: Protection,
    /// Whether the hardware encrypts the frames.
    pub encryption: MpduProtection,
    /// The transmit power.
    pub power: TxPower,
    /// Each MPDU sent, positions in order, without FCS and MIC: every frame
    /// of an A-MPDU, position 0 of a single MPDU.
    pub frames: Vec<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum State {
    Idle,
    OnAir {
        deadline: Instant,
        aggregate: bool,
        rate: PhyRate,
        access_category: WmmAccessCategory,
        /// Its exchange on a timed air.
        air: Option<AirTicket>,
    },
    /// The publication ended; `aggregate` when it was an A-MPDU, which sent
    /// every frame, else only position 0 went.
    Completed {
        aggregate: bool,
    },
}

struct Slots {
    memory: [Option<ModelNetworkMemory>; SLOTS],
    lengths: [u16; SLOTS],
    len: u8,
    state: State,
    /// The right the backend holds while the arena is published.
    held: Option<Arena>,
}

impl Slots {
    fn new() -> Self {
        Self {
            memory: core::array::from_fn(|_| None),
            lengths: [0; SLOTS],
            len: 0,
            state: State::Idle,
            held: None,
        }
    }

    fn frames(&self) -> impl Iterator<Item = &[u8]> {
        self.memory
            .iter()
            .zip(&self.lengths)
            .take(usize::from(self.len))
            .filter_map(|(memory, &length)| memory.as_ref()?.get(..usize::from(length)))
    }
}

/// The two arenas' state, apart from any capabilities: what each holds,
/// the air's scripted answers and what the hardware read.
pub struct ModelArenas {
    arenas: [Slots; 2],
    claimed: bool,
    key: bool,
    answers: VecDeque<ModelArenaAnswer>,
    length_limit: Option<usize>,
    published: Vec<ModelArenaPublication>,
    fault: Option<ModelArenaFault>,
}

impl Default for ModelArenas {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelArenas {
    const CAPACITY: u8 = {
        if MODEL_ARENA_FRAMES == 0 || MODEL_ARENA_FRAMES > ARENA_FRAMES as usize {
            panic!("a model arena has 1 to ARENA_FRAMES slots");
        }
        MODEL_ARENA_FRAMES as u8
    };

    /// Two idle, unclaimed arenas, no pairwise key and no scripted answer.
    pub fn new() -> Self {
        let _capacity = Self::CAPACITY;
        Self {
            arenas: [Slots::new(), Slots::new()],
            claimed: false,
            key: false,
            answers: VecDeque::new(),
            length_limit: None,
            published: Vec::new(),
            fault: None,
        }
    }

    /// Install or remove the pairwise key a standalone [`ModelArena`]'s
    /// encrypted publications need.
    pub fn set_key(&mut self, installed: bool) {
        self.key = installed;
    }

    /// Refuse an A-MPDU whose MPDUs exceed `octets` in all, as hardware
    /// refuses one past its length limit; `None` sets no limit.
    pub fn set_length_limit(&mut self, octets: Option<usize>) {
        self.length_limit = octets;
    }

    /// Queue the air's answer to the next publication served.
    pub fn answer(&mut self, answer: ModelArenaAnswer) {
        self.answers.push_back(answer);
    }

    /// Every publication, in order.
    pub fn published(&self) -> &[ModelArenaPublication] {
        &self.published
    }

    /// The fault the arenas quarantined themselves for.
    pub const fn fault(&self) -> Option<ModelArenaFault> {
        self.fault
    }

    /// The number of frames `arena` holds.
    pub fn len(&self, arena: ArenaId) -> u8 {
        self.arenas.get(arena.index()).map_or(0, |slots| slots.len)
    }

    /// The arena on the air: published and not yet completed.
    pub fn on_air(&self) -> Option<ArenaId> {
        [ArenaId::First, ArenaId::Second].into_iter().find(|id| {
            self.arenas
                .get(id.index())
                .is_some_and(|slots| matches!(slots.state, State::OnAir { .. }))
        })
    }

    /// The BlockAck a recipient sends when it received the frames of
    /// `acked` of `arena`: its window starts at position 0's sequence number.
    /// `None` when a frame has no Sequence Control field or lies outside
    /// that window.
    pub fn block_ack(&self, arena: ArenaId, acked: FrameMask) -> Option<BlockAckReport> {
        let slots = self.arenas.get(arena.index())?;
        let mut sequences = slots
            .frames()
            .map(|frame| sequence_control(frame).map(SequenceNumber::from_sequence_control));
        let start = sequences.next()??;
        let mut bitmap = 0_u64;
        for index in acked {
            let sequence = slots
                .frames()
                .nth(usize::from(index.get()))
                .and_then(sequence_control)
                .map(SequenceNumber::from_sequence_control)?;
            let offset = sequence.get().wrapping_sub(start.get()) & 0x0fff;
            bitmap |= 1_u64.checked_shl(u32::from(offset))?;
        }
        Some(BlockAckReport::new(start, bitmap))
    }

    fn quarantine(&mut self, fault: ModelArenaFault) -> ModelArenaFault {
        self.fault = Some(fault);
        fault
    }

    fn check(&self) -> Result<(), ModelArenaFault> {
        self.fault.map_or(Ok(()), Err)
    }

    fn claim(&mut self) -> bool {
        !core::mem::replace(&mut self.claimed, true)
    }

    fn push(
        &mut self,
        arena: ArenaId,
        mut memory: ModelNetworkMemory,
        write: impl FnOnce(&mut [u8]) -> Option<u16>,
        capacity: u8,
    ) -> Result<(), (ModelNetworkMemory, IntakeRefused)> {
        if self.fault.is_some() {
            return Err((memory, IntakeRefused::Quarantined));
        }
        let Some(slots) = self.arenas.get_mut(arena.index()) else {
            return Err((memory, IntakeRefused::Busy));
        };
        if slots.state != State::Idle {
            return Err((memory, IntakeRefused::Busy));
        }
        let position = usize::from(slots.len);
        if slots.len >= capacity {
            return Err((memory, IntakeRefused::Full));
        }
        let Some(length) = write(memory.as_mut_slice()) else {
            return Err((memory, IntakeRefused::Declined));
        };
        if length == 0 || usize::from(length) > memory.len() {
            return Err((memory, IntakeRefused::Length));
        }
        let (Some(place), Some(stored)) = (
            slots.memory.get_mut(position),
            slots.lengths.get_mut(position),
        ) else {
            return Err((memory, IntakeRefused::Full));
        };
        *place = Some(memory);
        *stored = length;
        slots.len = slots.len.saturating_add(1);
        Ok(())
    }

    /// Why the hardware would refuse to publish `arena` as one MPDU
    /// (`single`) or an A-MPDU, encrypted or not, with a pairwise key or
    /// without one.
    fn refusal(
        &self,
        arena: ArenaId,
        single: bool,
        encrypted: bool,
        key: bool,
    ) -> Option<PublishRefused> {
        let other_on_air = self.on_air().is_some_and(|on_air| on_air != arena);
        let Some(slots) = self.arenas.get(arena.index()) else {
            return Some(PublishRefused::Busy);
        };
        let sent = if single { 1 } else { usize::MAX };
        let contradicted = slots.frames().take(sent).any(|frame| {
            frame
                .first_chunk::<2>()
                .is_none_or(|field| FrameControlFields::from_bytes(*field).protected() != encrypted)
        });
        let octets = slots
            .frames()
            .map(<[u8]>::len)
            .fold(0_usize, usize::saturating_add);
        if other_on_air || slots.state != State::Idle {
            Some(PublishRefused::Busy)
        } else if slots.len == 0 {
            Some(PublishRefused::Empty)
        } else if encrypted && !key {
            Some(PublishRefused::NoKey)
        } else if contradicted {
            Some(PublishRefused::Protection)
        } else if !single && self.length_limit.is_some_and(|limit| octets > limit) {
            Some(PublishRefused::Length)
        } else {
            None
        }
    }

    /// The frames a publication of `arena` sends: position 0 of a single
    /// MPDU, every frame of an A-MPDU.
    fn sent(&self, arena: ArenaId, single: bool) -> impl Iterator<Item = &[u8]> {
        self.arenas
            .get(arena.index())
            .into_iter()
            .flat_map(Slots::frames)
            .take(if single { 1 } else { usize::MAX })
    }

    /// Put `publication`, which [`Self::refusal`] accepted, on the air.
    fn start<K: Ieee80211Capabilities>(
        &mut self,
        publication: ArenaPublication<'_, K>,
        now: Instant,
        air: Option<AirTicket>,
    ) {
        let id = publication.arena.id();
        let aggregate = match publication.format {
            ArenaFormat::Single(_) => None,
            ArenaFormat::Aggregate { spacing, .. } => Some(spacing),
        };
        let rate = publication.format.rate();
        let frames = self
            .sent(id, aggregate.is_none())
            .map(<[u8]>::to_vec)
            .collect();
        self.published.push(ModelArenaPublication {
            arena: id,
            aggregate,
            rate,
            access_category: publication.access_category,
            protection: publication.protection,
            encryption: publication.encryption,
            power: publication.power.get(),
            frames,
        });
        if let Some(slots) = self.arenas.get_mut(id.index()) {
            slots.state = State::OnAir {
                deadline: now.saturating_add(if air.is_some() {
                    MODEL_TIMED_ARENA_TIMEOUT
                } else {
                    MODEL_ARENA_TIMEOUT
                }),
                aggregate: aggregate.is_some(),
                rate,
                access_category: publication.access_category,
                air,
            };
            slots.held = Some(publication.arena);
        }
    }

    /// The exchange of the arena on the air, on a timed air.
    pub(crate) fn on_air_ticket(&self) -> Option<AirTicket> {
        self.arenas.iter().find_map(|slots| match slots.state {
            State::OnAir { air, .. } => air,
            State::Idle | State::Completed { .. } => None,
        })
    }

    /// The access category and deadline of the arena on the air.
    fn on_air_queue(&self) -> Option<(WmmAccessCategory, Instant)> {
        self.arenas.iter().find_map(|slots| match slots.state {
            State::OnAir {
                access_category,
                deadline,
                ..
            } => Some((access_category, deadline)),
            State::Idle | State::Completed { .. } => None,
        })
    }

    /// Replace the next answer by an abort: the hardware timed out.
    fn abort(&mut self) {
        self.replace_answer(ModelArenaAnswer::Aborted);
    }

    fn replace_answer(&mut self, replaced: ModelArenaAnswer) {
        match self.answers.front_mut() {
            Some(answer) => *answer = replaced,
            None => self.answers.push_back(replaced),
        }
    }

    fn complete(
        &mut self,
        now: Instant,
    ) -> Result<Option<Completion<ModelBlockAckEvidence, ModelAirDenied>>, ModelArenaFault> {
        self.check()?;
        let Some(id) = self.on_air() else {
            return Ok(None);
        };
        let Some(State::OnAir {
            deadline,
            aggregate,
            rate,
            ..
        }) = self.arenas.get(id.index()).map(|slots| slots.state)
        else {
            return Ok(None);
        };
        let answer = match self.answers.front().copied() {
            Some(ModelArenaAnswer::Timeout) | None if now < deadline => return Ok(None),
            Some(ModelArenaAnswer::Timeout) | None => {
                self.answers.pop_front();
                ModelArenaAnswer::Timeout
            }
            Some(answer) => {
                self.answers.pop_front();
                answer
            }
        };
        let air = Air {
            transmissions: NonZeroU16::MIN,
            rate,
        };
        let measured = ModelBlockAckEvidence::Measured(MODEL_ACK_SNR_DB);
        let end = match answer {
            ModelArenaAnswer::BlockAck(report) if aggregate => AttemptEnd::Answered {
                air,
                report,
                response: measured,
            },
            ModelArenaAnswer::BlockAckUnmeasured(report) if aggregate => AttemptEnd::Answered {
                air,
                report,
                response: ModelBlockAckEvidence::Unmeasured,
            },
            ModelArenaAnswer::Ack if !aggregate => AttemptEnd::Acked {
                air,
                response: measured,
            },
            ModelArenaAnswer::BlockAck(_)
            | ModelArenaAnswer::BlockAckUnmeasured(_)
            | ModelArenaAnswer::Ack => {
                return Err(self.quarantine(ModelArenaFault::MismatchedAnswer));
            }
            ModelArenaAnswer::NoResponse => AttemptEnd::NoResponse(air),
            ModelArenaAnswer::ProtectionFailure => AttemptEnd::ProtectionFailure(None),
            ModelArenaAnswer::Collision => AttemptEnd::Collision(None),
            ModelArenaAnswer::AirDenied => AttemptEnd::AirDenied {
                denial: ModelAirDenied,
                air: None,
            },
            ModelArenaAnswer::Aborted | ModelArenaAnswer::Timeout => AttemptEnd::Aborted,
            ModelArenaAnswer::Fault => return Err(self.quarantine(ModelArenaFault::Injected)),
        };
        let Some(slots) = self.arenas.get_mut(id.index()) else {
            return Err(self.quarantine(ModelArenaFault::State));
        };
        let Some(arena) = slots.held.take() else {
            return Err(self.quarantine(ModelArenaFault::State));
        };
        slots.state = State::Completed { aggregate };
        Ok(Some(Completion::new(arena, slots.len, end)))
    }

    fn settle(
        &mut self,
        settlement: Settlement,
        mut returned: impl FnMut(ModelNetworkMemory),
    ) -> Result<(), ModelArenaFault> {
        self.check()?;
        let keep = settlement.keep();
        let Some(slots) = self
            .arenas
            .get_mut(settlement.arena().index())
            .filter(|slots| slots.len == settlement.frames())
        else {
            return Err(self.quarantine(ModelArenaFault::State));
        };
        let State::Completed { aggregate } = slots.state else {
            return Err(self.quarantine(ModelArenaFault::State));
        };
        let mut kept = 0_u8;
        for index in FrameMask::first(slots.len).unwrap_or(FrameMask::EMPTY) {
            let position = usize::from(index.get());
            let Some(mut memory) = slots.memory.get_mut(position).and_then(Option::take) else {
                return Err(self.quarantine(ModelArenaFault::State));
            };
            if !keep.contains(index) {
                returned(memory);
                continue;
            }
            if settlement.retransmission() && (aggregate || index.get() == 0) {
                set_retry_bit(memory.as_mut_slice());
            }
            let Some(length) = slots.lengths.get(position).copied() else {
                return Err(self.quarantine(ModelArenaFault::State));
            };
            let target = usize::from(kept);
            let (Some(place), Some(stored)) =
                (slots.memory.get_mut(target), slots.lengths.get_mut(target))
            else {
                return Err(self.quarantine(ModelArenaFault::State));
            };
            *place = Some(memory);
            *stored = length;
            kept = kept.saturating_add(1);
        }
        slots.len = kept;
        slots.state = State::Idle;
        Ok(())
    }

    fn withdraw(
        &mut self,
        arena: ArenaId,
        mut returned: impl FnMut(ModelNetworkMemory),
    ) -> Result<(), ModelArenaFault> {
        self.check()?;
        let Some(slots) = self
            .arenas
            .get_mut(arena.index())
            .filter(|slots| slots.state == State::Idle)
        else {
            return Err(self.quarantine(ModelArenaFault::State));
        };
        for memory in slots
            .memory
            .iter_mut()
            .take(usize::from(slots.len))
            .filter_map(Option::take)
        {
            returned(memory);
        }
        slots.len = 0;
        Ok(())
    }

    fn deadline(&self) -> Option<Instant> {
        self.on_air_queue().map(|(_, deadline)| deadline)
    }
}

/// A host [`TxArena`] over the capabilities `C` and the arenas `S`: owned
/// for a standalone arena, borrowed for the driver's view of the
/// station's arenas. It derefs to its [`ModelArenas`].
pub struct ModelArena<C, S = ModelArenas> {
    state: S,
    caps: PhantomData<fn() -> C>,
}

impl<C> Default for ModelArena<C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C> ModelArena<C> {
    /// Two idle, unclaimed arenas, no pairwise key and no scripted answer.
    pub fn new() -> Self {
        Self::over(ModelArenas::new())
    }
}

impl<C, S: BorrowMut<ModelArenas>> ModelArena<C, S> {
    /// The port over `state`.
    pub const fn over(state: S) -> Self {
        Self {
            state,
            caps: PhantomData,
        }
    }
}

impl<C, S: BorrowMut<ModelArenas>> core::ops::Deref for ModelArena<C, S> {
    type Target = ModelArenas;
    fn deref(&self) -> &ModelArenas {
        self.state.borrow()
    }
}

impl<C, S: BorrowMut<ModelArenas>> core::ops::DerefMut for ModelArena<C, S> {
    fn deref_mut(&mut self) -> &mut ModelArenas {
        self.state.borrow_mut()
    }
}

/// The channel a standalone [`ModelArena`] publishes on, whose rates its
/// [`PortArena`] checks.
pub const MODEL_ARENA_CHANNEL: Channel = match Channel::ghz2_4(6, ChannelWidth::Mhz20) {
    Ok(channel) => channel,
    Err(_) => panic!("channel 6 is a 2.4 GHz channel"),
};

impl<'p, 'id: 'p, C: Ieee80211Capabilities, S: BorrowMut<ModelArenas>> PortArena<'p, 'id>
    for ModelArena<C, S>
{
    fn channel(&self) -> Channel {
        MODEL_ARENA_CHANNEL
    }

    fn ampdu_capabilities(&self) -> AmpduCapabilities {
        crate::data_port::model_ampdu_capabilities(ModelArenas::CAPACITY)
    }
}

impl<'p, 'id: 'p, C: Ieee80211Capabilities, S: BorrowMut<ModelArenas>> TxArena<'p, 'id>
    for ModelArena<C, S>
{
    type Caps = C;
    type Memory = ModelNetworkMemory;
    type Events = ();
    type Response = ModelBlockAckEvidence;
    type Denial = ModelAirDenied;
    type Fault = ModelArenaFault;

    fn claim_arenas(&mut self) -> bool {
        self.state.borrow_mut().claim()
    }

    fn capacity(&self) -> u8 {
        ModelArenas::CAPACITY
    }

    fn push(
        &mut self,
        arena: &Arena,
        memory: ModelNetworkMemory,
        write: impl FnOnce(&mut [u8]) -> Option<u16>,
    ) -> Result<(), (ModelNetworkMemory, IntakeRefused)> {
        self.state
            .borrow_mut()
            .push(arena.id(), memory, write, ModelArenas::CAPACITY)
    }

    fn len(&self, arena: ArenaId) -> u8 {
        self.state.borrow().len(arena)
    }

    fn on_air(&self) -> Option<ArenaId> {
        self.state.borrow().on_air()
    }

    fn event_waiting(&self) -> bool {
        false
    }

    fn publish(
        &mut self,
        publication: ArenaPublication<'p, C>,
        now: Instant,
    ) -> Result<(), PublishError<'p, C, ModelArenaFault>> {
        let state = self.state.borrow_mut();
        if let Some(fault) = state.fault {
            return Err(PublishError::Fault(fault));
        }
        let single = matches!(publication.format, ArenaFormat::Single(_));
        let encrypted = publication.encryption == MpduProtection::Ccmp;
        let key = state.key;
        if let Some(reason) = state.refusal(publication.arena.id(), single, encrypted, key) {
            return Err(PublishError::Refused {
                publication,
                reason,
            });
        }
        state.start(publication, now, None);
        Ok(())
    }

    fn complete(
        &mut self,
        (): (),
        now: Instant,
    ) -> Result<Option<Completion<ModelBlockAckEvidence, ModelAirDenied>>, ModelArenaFault> {
        self.state.borrow_mut().complete(now)
    }

    fn settle(
        &mut self,
        settlement: Settlement,
        returned: impl FnMut(ModelNetworkMemory),
    ) -> Result<(), ModelArenaFault> {
        self.state.borrow_mut().settle(settlement, returned)
    }

    fn withdraw(
        &mut self,
        arena: &Arena,
        returned: impl FnMut(ModelNetworkMemory),
    ) -> Result<(), ModelArenaFault> {
        self.state.borrow_mut().withdraw(arena.id(), returned)
    }

    fn deadline(&self) -> Option<Instant> {
        self.state.borrow().deadline()
    }
}

/// The station's arenas in the model's installation, and the pairwise key
/// each arena holds while a frame it encrypted is published or retained.
pub(crate) struct StationArenas {
    arenas: ModelArenas,
    keys: [Option<PreparedKey>; 2],
}

impl StationArenas {
    pub(crate) fn new() -> Self {
        Self {
            arenas: ModelArenas::new(),
            keys: [None, None],
        }
    }

    /// The arena on the air.
    pub(crate) fn on_air(&self) -> Option<ArenaId> {
        self.arenas.on_air()
    }

    /// The exchange of the arena on the air, on a timed air.
    pub(crate) fn on_air_ticket(&self) -> Option<AirTicket> {
        self.arenas.on_air_ticket()
    }

    /// Serve the arena on the air at `now`: its completion once its answer
    /// is due, at once when `abort` (the hardware timed out or the model
    /// drains), unanswered when `collided` on a timed air. The access
    /// category of the publication comes back with it.
    pub(crate) fn serve<C: Ieee80211Capabilities>(
        &mut self,
        now: Instant,
        held_result: impl Fn(WmmAccessCategory) -> bool,
        abort: bool,
        collided: bool,
    ) -> Result<Option<(super::ModelArenaCompleted, WmmAccessCategory)>, ModelArenaFault> {
        let Some((access_category, deadline)) = self.arenas.on_air_queue() else {
            return Ok(None);
        };
        let overdue = now >= deadline;
        if held_result(access_category) && !abort && !overdue {
            return Ok(None);
        }
        if abort || (held_result(access_category) && overdue) {
            self.arenas.abort();
        } else if collided {
            self.arenas.replace_answer(ModelArenaAnswer::NoResponse);
        }
        let done = ModelArena::<C, &mut ModelArenas>::over(&mut self.arenas).service((), now)?;
        Ok(done.map(|done| (done, access_category)))
    }

    /// When the arena on the air needs service: its hardware deadline while
    /// its result is held, at once otherwise; `None` with none on the air.
    pub(crate) fn due(
        &self,
        now: Instant,
        held_result: impl Fn(WmmAccessCategory) -> bool,
        abort: bool,
    ) -> Option<Instant> {
        let (access_category, deadline) = self.arenas.on_air_queue()?;
        if held_result(access_category) && !abort {
            Some(deadline)
        } else {
            Some(now)
        }
    }

    /// Return a key whose arena holds no frame any more.
    fn release_keys(&mut self) {
        for id in [ArenaId::First, ArenaId::Second] {
            if self.arenas.len(id) == 0
                && let Some(key) = self.keys.get_mut(id.index())
            {
                drop(key.take());
            }
        }
    }
}

/// The station's view of its two data arenas, lent for one turn
/// ([`ModelPortArena`](crate::ModelPortArena)).
///
/// An encrypted publication takes the installed station pairwise key and
/// keeps it, with a use that a removal waits for, until its arena holds no
/// frame. Its answer is the air's next ([`ModelAir::answer`]): a BlockAck's
/// bit n acknowledges arena position n, a hardware timeout aborts the
/// publication, an answer of the other shape poisons the model, and no
/// queued answer acknowledges every sent frame. Each sent frame is recorded
/// in [`ModelAir::sent`] without a ticket. Publication is refused as busy
/// while the gate is closed or closing, a station configuration is pending
/// or the model drains.
pub struct ModelDataArena<'a, 'p, 'id, C, B: ModelComposition> {
    held: &'a ModelHeld<'static, B>,
    state: Option<RefMut<'a, StationArenas>>,
    caps: PhantomData<fn() -> C>,
    /// The port borrow and installation whose checked rates it publishes.
    brand: PhantomData<fn(&'p (), &'id ()) -> (&'p (), &'id ())>,
}

impl<'a, C, B: ModelComposition> ModelDataArena<'a, '_, '_, C, B> {
    pub(crate) fn new(held: &'a ModelHeld<'static, B>) -> Self {
        let state = held.arenas.try_borrow_mut().ok();
        if state.is_none() {
            held.poison();
        }
        Self {
            held,
            state,
            caps: PhantomData,
            brand: PhantomData,
        }
    }

    /// The arenas' own recorded fault.
    pub(crate) fn arena_fault(&self) -> Option<ModelArenaFault> {
        self.state.as_ref().and_then(|state| state.arenas.fault())
    }

    fn state(&mut self) -> Result<&mut StationArenas, ModelFault> {
        match self.state.as_deref_mut() {
            Some(state) if !self.held.air.failed() => Ok(state),
            _ => Err(ModelFault),
        }
    }

    fn fault(&self, _fault: ModelArenaFault) -> ModelFault {
        self.held.poison();
        ModelFault
    }

    /// The installed station pairwise key: the newest active one.
    fn station_key(air: &ModelAir) -> Option<RawKeyIdentity> {
        let installations = air.key_installations.take();
        let key = installations
            .iter()
            .rev()
            .filter_map(|entry| entry.identity.upgrade())
            .find(|key| {
                key.role == VifRole::Station && key.group_role.is_none() && key.active.get()
            })
            .map(RawKeyIdentity);
        air.key_installations.set(installations);
        key
    }
}

impl<C: Ieee80211Capabilities, B: ModelComposition> ModelDataArena<'_, '_, '_, C, B> {
    /// The exchange of `publication` on a timed air, the frames with their
    /// FCS and MIC. An answer that ends the attempt before the air takes
    /// no time.
    fn exchange(
        held: &ModelHeld<'static, B>,
        state: &StationArenas,
        publication: &ArenaPublication<'_, C>,
        answer: ModelArenaAnswer,
    ) -> Result<Option<AirTicket>, AirTimeError> {
        let response = match answer {
            ModelArenaAnswer::Ack => AirResponse::Ack,
            ModelArenaAnswer::BlockAck(_) | ModelArenaAnswer::BlockAckUnmeasured(_) => {
                AirResponse::BlockAck
            }
            ModelArenaAnswer::NoResponse => AirResponse::None,
            _ => return Ok(None),
        };
        let id = publication.arena.id();
        let rate = publication.format.rate();
        let expansion = FCS_OCTETS.saturating_add(match publication.encryption {
            MpduProtection::Open => 0,
            MpduProtection::Ccmp => CCMP_MIC_LEN as u32,
        });
        let octets = |frame: &[u8]| {
            u32::try_from(frame.len())
                .unwrap_or(u32::MAX)
                .saturating_add(expansion)
        };
        let (psdu_octets, subframes) = match publication.format {
            ArenaFormat::Single(_) => (state.arenas.sent(id, true).next().map_or(0, octets), 1),
            ArenaFormat::Aggregate { spacing, .. } => (
                ampdu_psdu_octets(
                    state.arenas.sent(id, false).map(octets),
                    rate,
                    spacing.get(),
                )?,
                state.arenas.len(id),
            ),
        };
        held.air.request_air(
            rate,
            psdu_octets,
            publication.protection,
            response,
            publication.access_category,
            subframes,
        )
    }
}

/// The CCMP packet number of a protected MPDU's CCMP header.
fn packet_number(frame: &[u8]) -> Option<u64> {
    let header = MacHeader::decode(frame)?.header_len();
    let ccmp = frame
        .get(header..)?
        .first_chunk::<CCMP_HEADER_LEN>()
        .copied()?;
    Some(CcmpPacketNumber::from_header_bytes(ccmp).value())
}

impl<'p, 'id: 'p, C: Ieee80211Capabilities, B: ModelComposition> TxArena<'p, 'id>
    for ModelDataArena<'_, 'p, 'id, C, B>
{
    type Caps = C;
    type Memory = ModelNetworkMemory;
    type Events = ();
    type Response = ModelBlockAckEvidence;
    type Denial = ModelAirDenied;
    type Fault = ModelFault;

    fn claim_arenas(&mut self) -> bool {
        self.state().is_ok_and(|state| state.arenas.claim())
    }

    fn capacity(&self) -> u8 {
        self.held.air.arena_frames.get()
    }

    fn push(
        &mut self,
        arena: &Arena,
        memory: ModelNetworkMemory,
        write: impl FnOnce(&mut [u8]) -> Option<u16>,
    ) -> Result<(), (ModelNetworkMemory, IntakeRefused)> {
        let capacity = self.capacity();
        match self.state() {
            Ok(state) => state.arenas.push(arena.id(), memory, write, capacity),
            Err(ModelFault) => Err((memory, IntakeRefused::Quarantined)),
        }
    }

    fn len(&self, arena: ArenaId) -> u8 {
        self.state
            .as_ref()
            .map_or(0, |state| state.arenas.len(arena))
    }

    fn on_air(&self) -> Option<ArenaId> {
        self.state.as_ref().and_then(|state| state.arenas.on_air())
    }

    fn event_waiting(&self) -> bool {
        false
    }

    fn publish(
        &mut self,
        publication: ArenaPublication<'p, C>,
        now: Instant,
    ) -> Result<(), PublishError<'p, C, ModelFault>> {
        let held = self.held;
        let state = self.state().map_err(PublishError::Fault)?;
        if state.arenas.fault().is_some() {
            return Err(PublishError::Fault(ModelFault));
        }
        let id = publication.arena.id();
        let single = matches!(publication.format, ArenaFormat::Single(_));
        let encrypted = publication.encryption == MpduProtection::Ccmp;
        let held_key = state
            .keys
            .get(id.index())
            .and_then(Option::as_ref)
            .map(|key| key.identity.clone());
        let key = if encrypted {
            held_key.clone().or_else(|| Self::station_key(&held.air))
        } else {
            None
        };
        let barrier =
            held.gate_closed.get() || held.configuring_station.get() || held.draining.get();
        let reason = if barrier {
            Some(PublishRefused::Busy)
        } else {
            state.arenas.refusal(id, single, encrypted, key.is_some())
        };
        if let Some(reason) = reason {
            return Err(PublishError::Refused {
                publication,
                reason,
            });
        }
        let frames = state.arenas.len(id);
        let report = |bits: u64| {
            FrameMask::first(frames)
                .and_then(|all| FrameMask::from_bits(all.bits() & bits))
                .and_then(|acked| state.arenas.block_ack(id, acked))
        };
        let answer = match (held.air.arena_result(single), single) {
            (AirAnswer::Ack, true) => Some(ModelArenaAnswer::Ack),
            (AirAnswer::BlockAck(bits), false) => report(bits).map(ModelArenaAnswer::BlockAck),
            (AirAnswer::BlockAckUnmeasured(bits), false) => {
                report(bits).map(ModelArenaAnswer::BlockAckUnmeasured)
            }
            (AirAnswer::NoResponse, _) => Some(ModelArenaAnswer::NoResponse),
            (AirAnswer::Collision, _) => Some(ModelArenaAnswer::Collision),
            (AirAnswer::AirDenied, _) => Some(ModelArenaAnswer::AirDenied),
            (AirAnswer::HardwareTimeout | AirAnswer::KeyRefused, _) => {
                Some(ModelArenaAnswer::Aborted)
            }
            (AirAnswer::CtsTimeout, _) => Some(ModelArenaAnswer::ProtectionFailure),
            (AirAnswer::Ack, false)
            | (AirAnswer::BlockAck(_) | AirAnswer::BlockAckUnmeasured(_), true) => None,
        };
        let Some(answer) = answer else {
            held.poison();
            return Err(PublishError::Fault(ModelFault));
        };
        let air = match Self::exchange(held, state, &publication, answer) {
            Ok(air) => air,
            Err(fault) => {
                held.air.air_fault.set(Some(fault));
                held.poison();
                return Err(PublishError::FaultBeforeTaking {
                    publication,
                    fault: ModelFault,
                });
            }
        };
        // A denied PPDU never reached the air the journal records.
        if answer != ModelArenaAnswer::AirDenied {
            let mut sent = Vec::new();
            for frame in state.arenas.sent(id, single) {
                let wire = match &key {
                    Some(key) => match super::seal(key, frame) {
                        Ok(wire) => wire,
                        Err(ModelFault) => {
                            held.poison();
                            return Err(PublishError::Fault(ModelFault));
                        }
                    },
                    None => frame.into(),
                };
                sent.push(AirFrame {
                    ticket: None,
                    role: VifRole::Station,
                    frame: wire,
                    plaintext: frame.into(),
                    rate: publication.format.rate(),
                    key: key.as_ref().map(|key| (key.0.index, key.0.active.get())),
                    packet_number: key.as_ref().and_then(|_| packet_number(frame)),
                });
            }
            held.air.sent.record_all(sent);
        }
        if held_key.is_none()
            && let (Some(key), Some(place)) = (key, state.keys.get_mut(id.index()))
        {
            *place = Some(PreparedKey::new(key));
        }
        state.arenas.answer(answer);
        state.arenas.start(publication, now, air);
        Ok(())
    }

    fn complete(
        &mut self,
        (): (),
        now: Instant,
    ) -> Result<Option<Completion<ModelBlockAckEvidence, ModelAirDenied>>, ModelFault> {
        let state = self.state()?;
        match state.arenas.complete(now) {
            Ok(done) => Ok(done),
            Err(fault) => Err(self.fault(fault)),
        }
    }

    fn settle(
        &mut self,
        settlement: Settlement,
        returned: impl FnMut(ModelNetworkMemory),
    ) -> Result<(), ModelFault> {
        let state = self.state()?;
        let settled = state.arenas.settle(settlement, returned);
        state.release_keys();
        settled.map_err(|fault| self.fault(fault))
    }

    fn withdraw(
        &mut self,
        arena: &Arena,
        returned: impl FnMut(ModelNetworkMemory),
    ) -> Result<(), ModelFault> {
        let state = self.state()?;
        let withdrawn = state.arenas.withdraw(arena.id(), returned);
        state.release_keys();
        withdrawn.map_err(|fault| self.fault(fault))
    }

    fn deadline(&self) -> Option<Instant> {
        self.state
            .as_ref()
            .and_then(|state| state.arenas.deadline())
    }
}

#[cfg(test)]
mod tests;
