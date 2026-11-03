//! The service's data transmission over the port's two data arenas.
//!
//! The station's data plane ([`DataPlane`]) fills and publishes the arenas
//! the station phase lends ([`StationPhase::data_arena`]). The service hands
//! it the station's link at connect and takes it back at disconnect; each
//! intake draws the MPDU's numbers from the station's link, whose spaces
//! the station's own frames share. It keeps one network MSDU the standby
//! arena cannot take yet ([`Intake::Wait`]) and the network's memory every
//! ended frame returns until the network's queue takes it back. A
//! publication takes one rate plan; an arena's completion comes back as the
//! port's terminal ([`TxCompleted::Arena`]) and is settled at the next step,
//! which owes the access point a BlockAckReq when the plane dropped a frame
//! under an agreement and teaches the rate selection. Before a new pairwise
//! key or a key removal, the frames written under the old key are withdrawn
//! from the arenas that have not published them.
//!
//! [`StationPhase::data_arena`]: oer_protocols_ieee80211_contracts_port::StationPhase::data_arena
//! [`TxCompleted::Arena`]: oer_protocols_ieee80211_contracts_port::TxCompleted::Arena

use super::*;
use oer_protocols_ieee80211_contracts_port::arena::{
    ARENA_FRAMES, ArenaFormat, PublishError, TxArena,
};
use oer_protocols_ieee80211_contracts_port::{AmpduPhase, StationPhase};
use oer_protocols_ieee80211_core_roles_sta::data::plane::{
    AggregateRate, DataLink, DataPlane, HtLimits, Intake, LinkNumbers, OutboundMsdu, PpduKind,
    SettleError, TxWindow,
};
use oer_protocols_ieee80211_core_roles_sta::machine::{LinkMode, LinkProtection};
use oer_protocols_ieee80211_core_upper_mac::block_ack::OperationalTxBlockAck;

/// The network memory the station holds at most: both arenas' frames and
/// the one MSDU that waits for the standby.
const HELD: usize = 2 * ARENA_FRAMES as usize + 1;

/// The data transmission's state beside the station.
pub(super) struct DataTx<M> {
    /// The plane over the arenas, once the first phase lent their rights.
    plane: Option<DataPlane>,
    /// The connection the plane holds the link of.
    epoch: Option<u64>,
    /// Each TID's transmit agreement as the plane last heard it.
    agreements: [Option<OperationalTxBlockAck>; 8],
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
            epoch: None,
            agreements: [None; 8],
            deferred: None,
            freed: heapless::Deque::new(),
        }
    }

    /// Whether the service takes the network's next MSDU: the plane holds a
    /// link and no MSDU waits.
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

/// Keep `memory` for the network; the bound holds every memory the station
/// can hold, so a refusal keeps it quarantined rather than dropping it.
fn free<M>(freed: &mut heapless::Deque<M, HELD>, memory: M) {
    if let Err(memory) = freed.push_back(memory) {
        let _quarantined = core::mem::ManuallyDrop::new(memory);
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
        let plane = match self.data.plane.take() {
            Some(plane) => plane,
            None => {
                let mut station = phase.station();
                let mut arena = station.data_arena();
                let rights = TxArena::arenas(&mut arena)
                    .ok_or(ServiceFault::Contract(BackendContractViolation::ArenaClaim))?;
                DataPlane::new(rights)
                    .map_err(|_| ServiceFault::Contract(BackendContractViolation::ArenaClaim))?
            }
        };
        Ok(self.data.plane.insert(plane))
    }

    /// One pass of the data at `now`: the completed arena settled, the
    /// station's link handed over or taken back, the waiting MSDU offered
    /// and the next arena published. Whether the station owes an action it
    /// did not owe before.
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
        let mut owed = self.settle(now, phase)?;
        owed |= self.sync_link(phase)?;
        owed |= self.take(now, phase, None::<&mut core::iter::Empty<_>>, output)?;
        self.publish(now, phase)?;
        Ok(owed)
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
    /// station's link, admitted at `now`; each TID that took one tells its
    /// transmit agreement once. Whether that changed what the station owes.
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
        {
            let Some(mut link) = self.station.link() else {
                return Ok(false);
            };
            let DataTx {
                plane: Some(plane),
                deferred,
                freed,
                ..
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
                let numbers = LinkNumbers {
                    sequences: match &mut link.mode {
                        LinkMode::Qos(counters) => LinkMode::Qos(counters),
                        LinkMode::NonQos(counter) => LinkMode::NonQos(counter),
                    },
                    packet_numbers: match &link.protection {
                        LinkProtection::Open => None,
                        LinkProtection::Ccmp { key, .. } => Some(key.packet_numbers()),
                    },
                };
                match plane.intake(&mut arena, msdu, numbers, now) {
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
                self.station.tx_block_ack_traffic(tid, now),
                StaTraffic::Input
            );
        }
        Ok(owed)
    }

    /// Publish the next arena while the station's link is up: a single MPDU
    /// by its rate plan, an A-MPDU at the plan's first rate; a refusal returns the arena to the plane, which
    /// publishes it again at the next pass.
    fn publish<'p, P>(&mut self, now: Instant, phase: &mut P) -> Result<(), StepFailure<'id, B>>
    where
        'id: 'p,
        P: LowerMacPhase<'p, 'id, Caps = B::Caps, Types = B::Types<'id>>,
    {
        if self.station.link().is_none() {
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

    /// Hand the plane the station's link once it connects, and take it back
    /// once the connection ends; then tell it every transmit agreement that
    /// started or ended. Whether that changed what the station owes.
    fn sync_link<'p, P>(&mut self, phase: &mut P) -> Result<bool, StepFailure<'id, B>>
    where
        'id: 'p,
        P: LowerMacPhase<'p, 'id, Caps = B::Caps, Types = B::Types<'id>>,
    {
        let connected = self.station.is_connected();
        let epoch = self.station.link().map(|link| link.epoch.sequence());
        // The connection ended, or another one holds the link now.
        let ended = self.data.epoch.is_some()
            && (!connected || epoch.is_some_and(|epoch| Some(epoch) != self.data.epoch));
        if ended {
            self.stop_data(phase)?;
        }
        if self.data.epoch.is_none()
            && let Some(epoch) = epoch
        {
            let retry = self.station.tx_aggregate_retry();
            let ht = self
                .station
                .tx_aggregate_limits()
                .and_then(|(spacing, max_length)| {
                    Some(HtLimits {
                        spacing: oer_protocols_ieee80211_contracts_port::MpduStartSpacing::new(
                            spacing,
                        )?,
                        max_length,
                    })
                });
            let link = self
                .station
                .link()
                .and_then(|link| DataLink::for_link(&link, retry, ht));
            if let Some(link) = link {
                let plane = self.plane(phase)?;
                if plane.handover(link).is_err() {
                    return Err(ServiceFault::Contract(BackendContractViolation::ArenaClaim).into());
                }
                self.data.epoch = Some(epoch);
                self.data.agreements = [None; 8];
                self.aggregate_rate(phase);
            }
        }
        let Some(plane) = self
            .data
            .plane
            .as_mut()
            .filter(|plane| plane.is_connected())
        else {
            return Ok(false);
        };
        for tid in data::WmmUserPriority::ALL {
            let agreement = self.station.tx_block_ack(tid);
            let heard = tid.select_mut(&mut self.data.agreements);
            if *heard != agreement {
                *heard = agreement;
                plane.agreement(
                    tid,
                    agreement.map(|agreement| TxWindow {
                        head: agreement.starting_sequence,
                        size: agreement.window,
                    }),
                );
            }
        }
        Ok(ended)
    }

    /// Take the link back from the plane: every frame its arenas hold
    /// unpublished and the waiting MSDU go back to the network; the arena on
    /// the air settles at its completion.
    pub(super) fn stop_data<'p, P>(&mut self, phase: &mut P) -> Result<(), StepFailure<'id, B>>
    where
        'id: 'p,
        P: LowerMacPhase<'p, 'id, Caps = B::Caps, Types = B::Types<'id>>,
    {
        let DataTx {
            plane,
            epoch,
            agreements,
            deferred,
            freed,
        } = &mut self.data;
        *epoch = None;
        *agreements = [None; 8];
        if let Some(msdu) = deferred.take() {
            report(&mut self.events, || {
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
            .stop(&mut arena, |memory| free(freed, memory), &mut self.events)
            .map_err(ServiceFault::Backend)?;
        Ok(())
    }
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
