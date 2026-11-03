//! The model's data transmit path: two arenas of plain slot arrays behind
//! the port's [`TxArena`].
//!
//! [`ModelArena`] holds each arena's memories and MPDU lengths by position.
//! Publication records what the hardware would read
//! ([`ModelArena::published`]). A publication whose sent frames carry a
//! Protected bit its encryption contradicts, or an A-MPDU past an optional
//! octet limit, is refused. The air answers each publication with the
//! next scripted [`ModelArenaAnswer`], and a publication without an answer
//! ends `Aborted` at its [`MODEL_ARENA_TIMEOUT`] deadline. Settlement sets
//! the Retry bit of a kept frame that may have reached the air. A fault,
//! injected or found in a disagreeing operation, quarantines the model:
//! every later operation fails and every memory stays where it is.

use super::{MODEL_ACK_SNR_DB, ModelAirDenied, ModelBlockAckEvidence, ModelNetworkMemory};
use alloc::{collections::VecDeque, vec::Vec};
use core::{marker::PhantomData, num::NonZeroU16};
use oer_portable_time::{Duration, Instant};
use oer_protocols_ieee80211_contracts_port::arena::{
    ARENA_FRAMES, Arena, ArenaFormat, ArenaId, ArenaPublication, AttemptEnd, Completion, FrameMask,
    IntakeRefused, PublishError, PublishRefused, Settlement, TxArena,
};
use oer_protocols_ieee80211_contracts_port::{
    Air, BlockAckReport, Ieee80211Capabilities, MpduProtection, MpduStartSpacing, PhyRate,
    Protection, TxPower,
};
use oer_protocols_ieee80211_wire::frame::{FrameControlFields, sequence_control, set_retry_bit};
use oer_protocols_ieee80211_wire::qos::WmmAccessCategory;
use oer_protocols_ieee80211_wire::sequence::SequenceNumber;

/// Slots of each model arena: the model's A-MPDU subframe limit.
pub const MODEL_ARENA_FRAMES: usize = super::MODEL_AMPDU_SUBFRAMES;

/// How long a model publication waits for its answer before it is aborted.
pub const MODEL_ARENA_TIMEOUT: Duration = Duration::from_millis(10);

/// The air's answer to one model publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelArenaAnswer {
    /// This BlockAck answers the A-MPDU.
    BlockAck(BlockAckReport),
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
    /// No completion arrives; the publication is aborted at its deadline.
    Timeout,
    /// The hardware faults when the publication is served.
    Fault,
}

/// Why the model quarantined itself.
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
    },
    /// The publication ended; `aggregate` when it was an A-MPDU, which sent
    /// every frame, else only position 0 went.
    Completed {
        aggregate: bool,
    },
}

struct Slots<const N: usize> {
    memory: [Option<ModelNetworkMemory>; N],
    lengths: [u16; N],
    len: u8,
    state: State,
    /// The right the backend holds while the arena is published.
    held: Option<Arena>,
}

impl<const N: usize> Slots<N> {
    fn new() -> Self {
        Self {
            memory: core::array::from_fn(|_| None),
            lengths: [0; N],
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

/// A host [`TxArena`] of `N` slots per arena over the capabilities `C`.
pub struct ModelArena<C, const N: usize = MODEL_ARENA_FRAMES> {
    arenas: [Slots<N>; 2],
    claimed: bool,
    key: bool,
    answers: VecDeque<ModelArenaAnswer>,
    length_limit: Option<usize>,
    published: Vec<ModelArenaPublication>,
    fault: Option<ModelArenaFault>,
    caps: PhantomData<fn() -> C>,
}

impl<C, const N: usize> Default for ModelArena<C, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C, const N: usize> ModelArena<C, N> {
    const CAPACITY: u8 = {
        if N == 0 || N > ARENA_FRAMES as usize {
            panic!("a model arena has 1 to ARENA_FRAMES slots");
        }
        N as u8
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
            caps: PhantomData,
        }
    }

    /// Install or remove the interface's pairwise key, which encrypted
    /// publications need.
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

    /// The fault the model quarantined itself for.
    pub const fn fault(&self) -> Option<ModelArenaFault> {
        self.fault
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
}

impl<'p, 'id: 'p, C: Ieee80211Capabilities, const N: usize> TxArena<'p, 'id> for ModelArena<C, N> {
    type Caps = C;
    type Memory = ModelNetworkMemory;
    type Events = ();
    type Response = ModelBlockAckEvidence;
    type Denial = ModelAirDenied;
    type Fault = ModelArenaFault;

    fn claim_arenas(&mut self) -> bool {
        !core::mem::replace(&mut self.claimed, true)
    }

    fn capacity(&self) -> u8 {
        Self::CAPACITY
    }

    fn push(
        &mut self,
        arena: &Arena,
        mut memory: ModelNetworkMemory,
        write: impl FnOnce(&mut [u8]) -> Option<u16>,
    ) -> Result<(), (ModelNetworkMemory, IntakeRefused)> {
        if self.fault.is_some() {
            return Err((memory, IntakeRefused::Quarantined));
        }
        let Some(slots) = self.arenas.get_mut(arena.id().index()) else {
            return Err((memory, IntakeRefused::Busy));
        };
        if slots.state != State::Idle {
            return Err((memory, IntakeRefused::Busy));
        }
        let position = usize::from(slots.len);
        if position >= N {
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

    fn len(&self, arena: ArenaId) -> u8 {
        self.arenas.get(arena.index()).map_or(0, |slots| slots.len)
    }

    fn on_air(&self) -> Option<ArenaId> {
        [ArenaId::First, ArenaId::Second].into_iter().find(|id| {
            self.arenas
                .get(id.index())
                .is_some_and(|slots| matches!(slots.state, State::OnAir { .. }))
        })
    }

    fn publish(
        &mut self,
        publication: ArenaPublication<'p, 'id, C>,
        now: Instant,
    ) -> Result<(), PublishError<'p, 'id, C, ModelArenaFault>> {
        if let Some(fault) = self.fault {
            return Err(PublishError::Fault(fault));
        }
        let id = publication.arena.id();
        let other_on_air = self.on_air().is_some_and(|on_air| on_air != id);
        let Some(slots) = self.arenas.get_mut(id.index()) else {
            return Err(PublishError::Refused {
                publication,
                reason: PublishRefused::Busy,
            });
        };
        let single = match publication.format {
            ArenaFormat::Single(_) => Some(()),
            ArenaFormat::Aggregate { .. } => None,
        };
        let sent = if single.is_some() { 1 } else { usize::MAX };
        let encrypted = publication.encryption == MpduProtection::Ccmp;
        let contradicted = slots.frames().take(sent).any(|frame| {
            frame
                .first_chunk::<2>()
                .is_none_or(|field| FrameControlFields::from_bytes(*field).protected() != encrypted)
        });
        let octets = slots
            .frames()
            .map(<[u8]>::len)
            .fold(0_usize, usize::saturating_add);
        let reason = if other_on_air || slots.state != State::Idle {
            Some(PublishRefused::Busy)
        } else if slots.len == 0 {
            Some(PublishRefused::Empty)
        } else if encrypted && !self.key {
            Some(PublishRefused::NoKey)
        } else if contradicted {
            Some(PublishRefused::Protection)
        } else if single.is_none() && self.length_limit.is_some_and(|limit| octets > limit) {
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
        let aggregate = match publication.format {
            ArenaFormat::Single(_) => None,
            ArenaFormat::Aggregate { spacing, .. } => Some(spacing),
        };
        let rate = publication.format.rate();
        self.published.push(ModelArenaPublication {
            arena: id,
            aggregate,
            rate,
            access_category: publication.access_category,
            protection: publication.protection,
            encryption: publication.encryption,
            power: publication.power.get(),
            frames: slots
                .frames()
                .take(if aggregate.is_some() { usize::MAX } else { 1 })
                .map(<[u8]>::to_vec)
                .collect(),
        });
        slots.state = State::OnAir {
            deadline: now.saturating_add(MODEL_ARENA_TIMEOUT),
            aggregate: aggregate.is_some(),
            rate,
        };
        slots.held = Some(publication.arena);
        Ok(())
    }

    fn complete(
        &mut self,
        (): (),
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
        let response = ModelBlockAckEvidence::Measured(MODEL_ACK_SNR_DB);
        let end = match answer {
            ModelArenaAnswer::BlockAck(report) if aggregate => AttemptEnd::Answered {
                air,
                report,
                response,
            },
            ModelArenaAnswer::Ack if !aggregate => AttemptEnd::Acked { air, response },
            ModelArenaAnswer::BlockAck(_) | ModelArenaAnswer::Ack => {
                return Err(self.quarantine(ModelArenaFault::MismatchedAnswer));
            }
            ModelArenaAnswer::NoResponse => AttemptEnd::NoResponse(air),
            ModelArenaAnswer::ProtectionFailure => AttemptEnd::ProtectionFailure(None),
            ModelArenaAnswer::Collision => AttemptEnd::Collision(None),
            ModelArenaAnswer::AirDenied => AttemptEnd::AirDenied {
                denial: ModelAirDenied,
                air: None,
            },
            ModelArenaAnswer::Timeout => AttemptEnd::Aborted,
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
        arena: &Arena,
        mut returned: impl FnMut(ModelNetworkMemory),
    ) -> Result<(), ModelArenaFault> {
        self.check()?;
        let Some(slots) = self
            .arenas
            .get_mut(arena.id().index())
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
        self.arenas.iter().find_map(|slots| match slots.state {
            State::OnAir { deadline, .. } => Some(deadline),
            State::Idle | State::Completed { .. } => None,
        })
    }
}
