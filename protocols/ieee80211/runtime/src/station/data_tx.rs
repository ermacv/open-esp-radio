//! The service's data transmission over the port's two data arenas, and the
//! station's link it carries.
//!
//! The station hands its link over at connect ([`LinkAction::HandOver`]):
//! the service keeps it ([`PortLink`]), numbers the link's data and the
//! station's own frames from it and admits the link's received data
//! without the station. The station holds and releases the link's data
//! ([`LinkAction::Hold`], [`LinkAction::Release`]), tells each change of a
//! transmit agreement ([`LinkAction::Agreement`]) and takes the link back
//! at disconnect ([`LinkAction::Return`]): the frames the arenas hold
//! unpublished and the waiting MSDU go back to the network then.
//!
//! The station's data plane ([`DataPlane`]) fills and publishes the arenas
//! the station phase lends ([`StationPhase::data_arena`]). It takes the
//! link's data state at the first pass after the link first carries data;
//! each intake draws the MPDU's numbers from the link. It keeps one network
//! MSDU the standby arena cannot take yet ([`Intake::Wait`]) and the
//! network's memory every ended frame returns until the network's queue
//! takes it back. A publication takes one rate plan; an arena's completion
//! comes back as the port's terminal ([`TxCompleted::Arena`]) and is settled
//! at the next step, which owes the access point a BlockAckReq when the
//! plane dropped a frame under an agreement and teaches the rate selection.
//! Before a new pairwise key or a key removal, the frames written under the
//! old key are withdrawn from the arenas that have not published them.
//!
//! [`StationPhase::data_arena`]: oer_protocols_ieee80211_contracts_port::StationPhase::data_arena
//! [`TxCompleted::Arena`]: oer_protocols_ieee80211_contracts_port::TxCompleted::Arena

use super::*;
use oer_protocols_ieee80211_contracts_port::arena::{
    ARENA_FRAMES, ArenaFormat, PublishError, TxArena,
};
use oer_protocols_ieee80211_contracts_port::{AmpduPhase, StationPhase};
use oer_protocols_ieee80211_core_roles_sta::data::plane::{
    AggregateRate, DataPlane, Intake, OutboundMsdu, PpduKind, SettleError,
};
use oer_protocols_ieee80211_core_roles_sta::link::{LinkAction, LinkHandover};

/// The network memory the station holds at most: both arenas' frames and
/// the one MSDU that waits for the standby.
const HELD: usize = 2 * ARENA_FRAMES as usize + 1;

/// The transmit half of the station's pairwise key on the port `B`, which
/// the link numbers its protected MPDUs with.
pub(super) type PortLinkKey<'id, B> = oer_protocols_ieee80211_core_upper_mac::ccmp::InstalledCcmpTxKey<
    <<<B as StationPort>::Types<'id> as PhaseTypes<'id>>::StationKeys as KeyTypes<'id>>::PairwiseKey,
>;

/// The station's link on the port `B`, while the service holds it.
pub(super) type PortLink<'id, B> = LinkHandover<
    'id,
    <<B as StationPort>::Types<'id> as PhaseTypes<'id>>::EpochPath,
    PortLinkKey<'id, B>,
>;

/// What the station tells the service of its link on the port `B`.
pub(super) type PortLinkAction<'id, B> = LinkAction<
    'id,
    <<B as StationPort>::Types<'id> as PhaseTypes<'id>>::EpochPath,
    PortLinkKey<'id, B>,
>;

/// The data transmission's state beside the station.
pub(super) struct DataTx<M> {
    /// The plane over the arenas, once the first phase lent their rights.
    plane: Option<DataPlane>,
    /// The MSDU the standby arena could not take yet.
    deferred: Option<OutboundMsdu<M>>,
    /// The network's memory of ended frames, until the network's queue
    /// takes it back.
    freed: heapless::Deque<M, HELD>,
}

impl<M> DataTx<M> {
    pub(super) const fn new() -> Self {
        Self {
            plane: None,
            deferred: None,
            freed: heapless::Deque::new(),
        }
    }

    /// Whether the plane takes the network's next MSDU: it holds a link and
    /// no MSDU waits.
    pub(super) fn accepts(&self) -> bool {
        self.deferred.is_none() && self.plane.as_ref().is_some_and(DataPlane::is_connected)
    }

    /// Whether an arena is on the air.
    pub(super) fn on_air(&self) -> bool {
        self.plane.as_ref().is_some_and(DataPlane::on_air)
    }

    /// The memory of the next ended frame, for the network.
    pub(super) fn take_freed(&mut self) -> Option<M> {
        self.freed.pop_front()
    }

    /// Whether memory waits for the network's queue.
    pub(super) fn has_freed(&self) -> bool {
        !self.freed.is_empty()
    }

    /// The MSDUs held before an arena took them, the frames the arenas
    /// hold, and the memory freed for the network.
    pub(super) fn held(&self) -> (usize, usize, usize) {
        (
            usize::from(self.deferred.is_some()),
            self.plane
                .as_ref()
                .map_or(0, |plane| usize::from(plane.frames())),
            self.freed.len(),
        )
    }
}

/// Whether the service takes the network's next MSDU: the station's link
/// carries data and the plane takes one.
pub(super) fn accepts<'id, B: StationPort<SlotId: 'id, Backend: 'id> + 'id>(
    link: Option<&PortLink<'id, B>>,
    data: &DataTx<B::TxMemory>,
) -> bool {
    link.is_some_and(LinkHandover::carries) && data.accepts()
}

/// Keep `memory` for the network; the bound holds every memory the station
/// can hold, so a refusal keeps it quarantined rather than dropping it.
fn free<M>(freed: &mut heapless::Deque<M, HELD>, memory: M) {
    if let Err(memory) = freed.push_back(memory) {
        let _quarantined = core::mem::ManuallyDrop::new(memory);
    }
}

/// What the caller owes the station after one change of its link.
pub(super) enum LinkFollow {
    /// Nothing.
    Nothing,
    /// Hand the held link back ([`return_link`]).
    Return,
    /// Tell the station the TID's next Sequence Number.
    Sequence(
        oer_protocols_ieee80211_core_roles_sta::data::WmmUserPriority,
        oer_protocols_ieee80211_core_roles_sta::link::SequenceNumber,
    ),
}

/// Take one change the station made to its link: keep the link it hands
/// over, hold or release its data, tell the plane (or the link's data state
/// it has not taken yet) a transmit agreement, stop the link's data, or
/// read a TID's next Sequence Number. What the caller owes the station:
/// the caller hands the held link to the station ([`return_link`]) once the
/// action no longer borrows it. A second handover while the service holds a
/// link is a contract violation; the service keeps the link it holds and
/// quarantines the new one. The service gives the pairwise key back with
/// its link, so a request for the key finds none: a contract violation.
pub(super) fn link_action<'p, 'id: 'p, B, P>(
    link: &mut Option<PortLink<'id, B>>,
    data: &mut DataTx<B::TxMemory>,
    phase: &mut P,
    events: &mut impl EventSink<oer_protocols_ieee80211_core_roles_sta::events::StaEvent>,
    action: PortLinkAction<'id, B>,
) -> Result<LinkFollow, ServiceFault<B>>
where
    B: StationPort<SlotId: 'id, Backend: 'id> + 'id,
    P: LowerMacPhase<'p, 'id, Caps = B::Caps, Types = B::Types<'id>>,
{
    let mismatch = || {
        ServiceFault::Contract(BackendContractViolation::StationKeyBank(
            contract::KeyContractViolation::LinkKeyMismatch,
        ))
    };
    match action {
        LinkAction::HandOver(handover) => {
            if link.is_some() {
                let _quarantined = core::mem::ManuallyDrop::new(handover);
                return Err(mismatch());
            }
            *link = Some(handover);
            Ok(LinkFollow::Nothing)
        }
        LinkAction::Hold | LinkAction::Release => {
            if let Some(link) = link.as_mut() {
                link.hold(matches!(action, LinkAction::Hold));
            }
            Ok(LinkFollow::Nothing)
        }
        LinkAction::Agreement { tid, window } => {
            if !link
                .as_mut()
                .is_some_and(|link| link.agreement(tid, window))
                && let Some(plane) = data.plane.as_mut()
            {
                plane.agreement(tid, window);
            }
            Ok(LinkFollow::Nothing)
        }
        LinkAction::Return => {
            stop::<B, P>(data, phase, events)?;
            Ok(LinkFollow::Return)
        }
        LinkAction::Sequence(tid) => match link.as_ref() {
            Some(link) => Ok(LinkFollow::Sequence(tid, link.numbers().next(tid))),
            None => Ok(LinkFollow::Nothing),
        },
        LinkAction::ReturnKey => Err(mismatch()),
    }
}

/// Give the station the link the service holds back, its pairwise key
/// with it.
pub(super) fn return_link<
    'id,
    B: StationPort<SlotId: 'id, Backend: 'id> + 'id,
    C: StaCallbacks,
    const GROUPS: usize,
>(
    link: &mut Option<PortLink<'id, B>>,
    station: &mut PortStation<'id, B, C, GROUPS>,
) {
    if let Some(link) = link.take() {
        let (link, key) = link.into_return();
        station.link_returned(link);
        if let Some(key) = key {
            station.key_returned(key);
        }
    }
}

impl<
    'id,
    B: StationPort<SlotId: 'id, Backend: 'id> + 'id,
    C: StaCallbacks,
    S: RateSelection<PortRxMeta<'id, B>, PortAck<'id, B>, PortBlockAck<'id, B>>,
    const GROUPS: usize,
    O: StationEvents,
> ServiceState<'id, B, C, S, GROUPS, O>
{
    /// The plane over the phase's arenas, which the first phase lends.
    fn plane<'p, P>(&mut self, phase: &mut P) -> Result<&mut DataPlane, ServiceFault<B>>
    where
        'id: 'p,
        P: LowerMacPhase<'p, 'id, Caps = B::Caps, Types = B::Types<'id>>,
    {
        // The plane stays in place: moving it out and back would copy its
        // books on every call.
        if self.data.plane.is_none() {
            let mut station = phase.station();
            let mut arena = station.data_arena();
            let rights = TxArena::arenas(&mut arena)
                .ok_or(ServiceFault::Contract(BackendContractViolation::ArenaClaim))?;
            let plane = DataPlane::new(rights)
                .map_err(|_| ServiceFault::Contract(BackendContractViolation::ArenaClaim))?;
            return Ok(self.data.plane.insert(plane));
        }
        self.data
            .plane
            .as_mut()
            .ok_or(ServiceFault::Contract(BackendContractViolation::ArenaClaim))
    }

    /// Take every change the station made to its link since its last
    /// input, before the link carries data again: its handover, a hold or
    /// release, its return. Transmit agreements wait for the next step.
    pub(super) fn link_changes<'p, P>(&mut self, phase: &mut P) -> Result<(), ServiceFault<B>>
    where
        'id: 'p,
        P: LowerMacPhase<'p, 'id, Caps = B::Caps, Types = B::Types<'id>>,
    {
        while let Some(action) = self.station.poll_link() {
            match link_action::<B, P>(
                &mut self.link,
                &mut self.data,
                phase,
                &mut self.events,
                action,
            )? {
                LinkFollow::Nothing => {}
                LinkFollow::Return => {
                    return_link::<B, C, GROUPS>(&mut self.link, &mut self.station);
                }
                LinkFollow::Sequence(tid, next) => self.station.tx_sequence(tid, next),
            }
        }
        Ok(())
    }

    /// One pass of the data at `now`: the completed arena settled, the
    /// link's data state handed to the plane once the link first carries
    /// data, the waiting MSDU offered and the next arena published. Whether
    /// the station owes an action it did not owe before.
    pub(super) fn data_turn<'p, P>(
        &mut self,
        now: Instant,
        phase: &mut P,
        output: &mut impl ServiceOutputSink<B::Caps>,
    ) -> Result<bool, StepFailure<'id, B>>
    where
        'id: 'p,
        P: LowerMacPhase<'p, 'id, Caps = B::Caps, Types = B::Types<'id>>,
    {
        self.plane(phase)?;
        let owed = self.settle(now, phase)?;
        self.connect_plane(phase)?;
        let owed = owed | self.take(now, phase, None::<&mut core::iter::Empty<_>>, output)?;
        self.publish(now, phase)?;
        Ok(owed)
    }

    /// Hand the link's data state to the plane once the link carries data.
    fn connect_plane<'p, P>(&mut self, phase: &mut P) -> Result<(), StepFailure<'id, B>>
    where
        'id: 'p,
        P: LowerMacPhase<'p, 'id, Caps = B::Caps, Types = B::Types<'id>>,
    {
        let Some(link) = self.link.as_mut().filter(|link| link.carries()) else {
            return Ok(());
        };
        let Some(data) = link.take_data() else {
            return Ok(());
        };
        if self.plane(phase)?.handover(data).is_err() {
            return Err(ServiceFault::Contract(BackendContractViolation::ArenaClaim).into());
        }
        self.aggregate_rate(phase);
        Ok(())
    }

    /// Hand every MSDU of `incoming` to the plane until the standby cannot
    /// take one, then publish: whether a TID's transmit agreement changed
    /// what the station owes.
    pub(super) fn send<'p, P>(
        &mut self,
        now: Instant,
        phase: &mut P,
        incoming: &mut impl Iterator<Item = Outgoing<B::TxMemory>>,
        output: &mut impl ServiceOutputSink<B::Caps>,
    ) -> Result<Taken, StepFailure<'id, B>>
    where
        'id: 'p,
        P: LowerMacPhase<'p, 'id, Caps = B::Caps, Types = B::Types<'id>>,
    {
        let owed = self.take(now, phase, Some(incoming), output)?;
        self.publish(now, phase)?;
        Ok(if owed { Taken::Input } else { Taken::Direct })
    }

    /// Offer the waiting MSDU, then those of `incoming`, to the plane on the
    /// station's link, admitted at `now`; each TID that took one tells the
    /// station once, with its next sequence number. Whether that changed
    /// what the station owes.
    fn take<'p, P>(
        &mut self,
        now: Instant,
        phase: &mut P,
        mut incoming: Option<&mut impl Iterator<Item = Outgoing<B::TxMemory>>>,
        output: &mut impl ServiceOutputSink<B::Caps>,
    ) -> Result<bool, StepFailure<'id, B>>
    where
        'id: 'p,
        P: LowerMacPhase<'p, 'id, Caps = B::Caps, Types = B::Types<'id>>,
    {
        let mut tids = [None; 8];
        let Some(link) = self.link.as_mut().filter(|link| link.carries()) else {
            return Ok(false);
        };
        {
            let DataTx {
                plane: Some(plane),
                deferred,
                freed,
            } = &mut self.data
            else {
                return Ok(false);
            };
            if !plane.is_connected() {
                return Ok(false);
            }
            let mut station = phase.station();
            let mut arena = station.data_arena();
            loop {
                let msdu = match deferred.take() {
                    Some(msdu) => msdu,
                    None => match incoming.as_mut().and_then(|incoming| incoming.next()) {
                        Some(Outgoing {
                            memory,
                            length,
                            priority,
                        }) => OutboundMsdu {
                            memory,
                            length: usize::from(length),
                            priority,
                        },
                        None => break,
                    },
                };
                match plane.intake(&mut arena, msdu, link.numbers_mut(), now) {
                    Intake::Taken(tid) => {
                        *tid.select_mut(&mut tids) = Some(tid);
                    }
                    Intake::Wait(msdu) => {
                        *deferred = Some(msdu);
                        break;
                    }
                    Intake::Refused { memory, reason } => {
                        output.emit(ServiceOutput::MsduRefused {
                            reason: reason.into(),
                        });
                        free(freed, memory);
                    }
                }
            }
        }
        let mut owed = false;
        for tid in tids.into_iter().flatten() {
            owed |= matches!(
                self.station.tx_traffic(tid, link.numbers().next(tid), now),
                StaTraffic::Input
            );
        }
        Ok(owed)
    }

    /// Publish the next arena while the station's link carries data: a
    /// single MPDU by its rate plan, an A-MPDU at the plan's first rate; a
    /// refusal returns the arena to the plane, which publishes it again at
    /// the next pass.
    fn publish<'p, P>(&mut self, now: Instant, phase: &mut P) -> Result<(), StepFailure<'id, B>>
    where
        'id: 'p,
        P: LowerMacPhase<'p, 'id, Caps = B::Caps, Types = B::Types<'id>>,
    {
        if !self.link.as_ref().is_some_and(LinkHandover::carries) {
            return Ok(());
        }
        let Some(plane) = self.data.plane.as_mut() else {
            return Ok(());
        };
        if plane.publishable().is_none() {
            return Ok(());
        }
        let rates = &self.rates;
        let publication = plane.publication(|plan| {
            let rates = rates.plan(plan.frame_rates(), |rate| phase.rate(rate))?;
            Some(match plan.kind {
                // A single MPDU goes by the whole plan, as any data MPDU.
                PpduKind::Single => ArenaFormat::Single(rates),
                PpduKind::Aggregate { spacing } => ArenaFormat::Aggregate {
                    rate: phase.station().ampdu_rate(rates.into_first().rate).ok()?,
                    spacing,
                },
            })
        });
        let Some(publication) = publication else {
            return Ok(());
        };
        let published = {
            let mut station = phase.station();
            let mut arena = station.data_arena();
            arena.publish(publication, now)
        };
        match published {
            Ok(()) => {
                self.pending.admissions.insert(PendingTerminal::Arena);
                Ok(())
            }
            Err(PublishError::Refused { publication, .. }) => {
                plane.refused(publication);
                Ok(())
            }
            Err(PublishError::Fault(fault)) => Err(ServiceFault::Backend(fault).into()),
        }
    }

    /// Settle the completed arena the port returned: its acknowledged and
    /// dropped frames' memory back to the network, a BlockAckReq owed for a
    /// frame dropped under an agreement, the lesson to the rate selection.
    /// Whether the station owes a BlockAckReq.
    pub(super) fn settle<'p, P>(
        &mut self,
        now: Instant,
        phase: &mut P,
    ) -> Result<bool, StepFailure<'id, B>>
    where
        'id: 'p,
        P: LowerMacPhase<'p, 'id, Caps = B::Caps, Types = B::Types<'id>>,
    {
        let Some(done) = self.pending.arena.take() else {
            return Ok(false);
        };
        let DataTx {
            plane: Some(plane),
            freed,
            ..
        } = &mut self.data
        else {
            return Err(ContractFault::sealed(
                BackendContractViolation::ArenaCompletion,
                LowerMacTerminal::TxCompleted(TxCompleted::Arena(done)),
            )
            .into());
        };
        let settled = {
            let mut station = phase.station();
            let mut arena = station.data_arena();
            plane.completed(
                &mut arena,
                done,
                now,
                |memory| free(freed, memory),
                &mut self.events,
            )
        };
        let settled = match settled {
            Ok(settled) => settled,
            Err(SettleError::Unexpected(done)) => {
                return Err(ContractFault::sealed(
                    BackendContractViolation::ArenaCompletion,
                    LowerMacTerminal::TxCompleted(TxCompleted::Arena(done)),
                )
                .into());
            }
            Err(SettleError::Fault(fault)) => return Err(ServiceFault::Backend(fault).into()),
        };
        if let Some(lesson) = settled.lesson {
            lesson.teach(now, &mut self.rates, B::arena_ack);
            self.aggregate_rate(phase);
        }
        Ok(match settled.bar {
            Some((tid, head)) => {
                self.station.tx_block_ack_request(tid, head);
                true
            }
            None => false,
        })
    }

    /// Give the plane the port's A-MPDU limits at the rate the aggregate
    /// rate plan names now; it computes its bounds again only when they
    /// changed.
    fn aggregate_rate<'p, P>(&mut self, phase: &mut P)
    where
        'id: 'p,
        P: LowerMacPhase<'p, 'id, Caps = B::Caps, Types = B::Types<'id>>,
    {
        let Some(plane) = self.data.plane.as_mut() else {
            return;
        };
        let rate = self
            .rates
            .plan(FrameRates::Aggregate, |rate| phase.rate(rate))
            .map(|plan| plan.into_first().rate)
            .and_then(|rate| {
                let station = phase.station();
                let rate = station.ampdu_rate(rate).ok()?.rate().get();
                let capabilities = station.ampdu_capabilities();
                Some(AggregateRate {
                    rate,
                    max_subframes: capabilities.max_subframes,
                    max_length: capabilities.max_length.at(rate).ok()?.get(),
                })
            });
        plane.aggregate_rate(rate);
    }

    /// Stop the data for the service's stop: every frame the arenas hold
    /// unpublished and the waiting MSDU go back to the network; the arena
    /// on the air settles at its completion.
    pub(super) fn stop_data<'p, P>(&mut self, phase: &mut P) -> Result<(), StepFailure<'id, B>>
    where
        'id: 'p,
        P: LowerMacPhase<'p, 'id, Caps = B::Caps, Types = B::Types<'id>>,
    {
        stop::<B, P>(&mut self.data, phase, &mut self.events).map_err(StepFailure::from)
    }
}

/// Take the link's data back from the plane: every frame its arenas hold
/// unpublished and the waiting MSDU go back to the network; the arena on
/// the air settles at its completion.
fn stop<'p, 'id: 'p, B, P>(
    data: &mut DataTx<B::TxMemory>,
    phase: &mut P,
    events: &mut impl EventSink<oer_protocols_ieee80211_core_roles_sta::events::StaEvent>,
) -> Result<(), ServiceFault<B>>
where
    B: StationPort<SlotId: 'id, Backend: 'id> + 'id,
    P: LowerMacPhase<'p, 'id, Caps = B::Caps, Types = B::Types<'id>>,
{
    let DataTx {
        plane,
        deferred,
        freed,
    } = data;
    if let Some(msdu) = deferred.take() {
        report(events, || {
            oer_protocols_ieee80211_core_roles_sta::events::StaEvent::DataLost(
                oer_protocols_ieee80211_core_roles_sta::events::DataLoss::LeftBehind,
            )
        });
        free(freed, msdu.memory);
    }
    let Some(plane) = plane.as_mut() else {
        return Ok(());
    };
    let mut station = phase.station();
    let mut arena = station.data_arena();
    plane
        .stop(&mut arena, |memory| free(freed, memory), events)
        .map_err(ServiceFault::Backend)?;
    Ok(())
}

/// Withdraw the frames the arenas hold unpublished: they carry the packet
/// numbers of the key a new installation or a removal replaces.
pub(super) fn withdraw_unpublished<'p, 'id: 'p, B, P>(
    data: &mut DataTx<B::TxMemory>,
    phase: &mut P,
    events: &mut impl EventSink<oer_protocols_ieee80211_core_roles_sta::events::StaEvent>,
) -> Result<(), ServiceFault<B>>
where
    B: StationPort<SlotId: 'id, Backend: 'id> + 'id,
    P: LowerMacPhase<'p, 'id, Caps = B::Caps, Types = B::Types<'id>>,
{
    let DataTx {
        plane: Some(plane),
        freed,
        ..
    } = data
    else {
        return Ok(());
    };
    let mut station = phase.station();
    let mut arena = station.data_arena();
    plane
        .withdraw_unpublished(&mut arena, |memory| free(freed, memory), events)
        .map_err(ServiceFault::Backend)
}
