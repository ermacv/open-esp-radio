//! The actor's data transmission over the port's two data arenas.
//!
//! The station's data plane ([`DataPlane`]) fills and publishes the arenas
//! the port lends for one turn ([`DataPort::arena`]). It takes the link's
//! data state once the link first carries data; each intake draws the
//! MPDU's numbers from the link. It keeps one network MSDU the standby
//! arena cannot take yet and the network's memory every ended frame
//! returns until the network's queue takes it back. A publication takes
//! one rate plan; an arena's end comes back from the transmit service and
//! settles in the same turn, which owes the access point a BlockAckReq when
//! the plane dropped a frame under an agreement, and teaches the rate
//! selection. Before a new pairwise key or a key removal, the frames written
//! under the old key are withdrawn from the arenas that have not published
//! them.

use super::turn::Failed;
use super::*;
use oer_protocols_ieee80211_contracts_port::arena::{ArenaFormat, PublishError, TxArena};
use oer_protocols_ieee80211_contracts_port::{PortArena, PortCompleted};
use oer_protocols_ieee80211_core_roles_sta::data::plane::{
    AggregateRate, Intake, PpduKind, SettleError,
};

/// Keep `memory` for the network; the bound holds every memory the plane can
/// hold, so a refusal keeps it quarantined rather than dropping it.
fn free<M>(freed: &mut heapless::Deque<M, HELD>, memory: M) {
    if let Err(memory) = freed.push_back(memory) {
        let _quarantined = core::mem::ManuallyDrop::new(memory);
    }
}

impl<'id, D: DatapathPort<'id>, S, O> Actor<'id, D, S, O>
where
    S: RateSelection<
            ActorRxMeta<'id, D>,
            <Ty<'id, D> as PortTypes<'id>>::AckEvidence,
            <Ty<'id, D> as PortTypes<'id>>::BlockAckEvidence,
        >,
    O: ActorEvents,
{
    /// Whether the actor takes the network's next MSDU: the link carries
    /// data and no MSDU waits.
    pub(super) fn accepts(&self) -> bool {
        self.carries() && self.data.deferred.is_none()
    }

    /// Whether the link carries data: the radio runs, the link carries
    /// data and the plane holds it.
    fn carries(&self) -> bool {
        self.radio.running()
            && self.link.as_ref().is_some_and(LinkHandover::carries)
            && self
                .data
                .plane
                .as_ref()
                .is_some_and(DataPlane::is_connected)
    }

    /// Hand the link's data state to the plane once the link carries data.
    pub(super) fn connect_plane(&mut self) -> Result<(), Failed<'id, D>> {
        let Some(link) = self.link.as_mut().filter(|link| link.carries()) else {
            return Ok(());
        };
        let Some(plane) = self.data.plane.as_mut() else {
            return Ok(());
        };
        let Some(data) = link.take_data() else {
            return Ok(());
        };
        if plane.handover(data).is_err() {
            return Err(ActorFault::Contract(ActorViolation::ArenaClaim));
        }
        self.aggregate_rate();
        Ok(())
    }

    /// Hand at most `TX_DEPTH` of the network's MSDUs (the waiting one
    /// first) to the plane until the standby takes no more, admitted at the
    /// turn's instant, then publish when nothing is on the air. A TID that
    /// took one owes the lifecycle its traffic, at most once per
    /// [`TX_TRAFFIC_REFRESH`].
    pub(super) fn intake<
        const TO: usize,
        const FROM: usize,
        const RETURNS: usize,
        NS: NetworkStop,
        const TX_DEPTH: usize,
        const RX_DEPTH: usize,
        N: Notifies,
    >(
        &mut self,
        _lifecycle: &mut LifecycleEnds<'_, 'id, D, TO, FROM, RETURNS>,
        network: &mut NetworkEnds<'_, D::TxMemory, D::LentRx, NS, TX_DEPTH, RX_DEPTH, N>,
    ) -> Result<(), Failed<'id, D>> {
        self.connect_plane()?;
        // The waiting MSDU is offered again at every turn the link carries
        // data.
        if !self.carries() {
            return Ok(());
        }
        let now = self.now;
        let mut tids = [false; 8];
        let mut taken = 0_usize;
        {
            let Self {
                port,
                link,
                data:
                    DataTx {
                        plane,
                        deferred,
                        freed,
                    },
                owed,
                sequence_hold,
                ..
            } = self;
            let (Some(plane), Some(link)) = (plane.as_mut(), link.as_mut()) else {
                return Ok(());
            };
            if !plane.is_connected() {
                return Ok(());
            }
            let mut arena = port.arena();
            while taken < TX_DEPTH {
                let msdu = match deferred.take() {
                    Some(msdu) => msdu,
                    None => match network.transmit.try_receive_with(Option::take) {
                        Ok(Some(Outgoing {
                            memory,
                            length,
                            priority,
                        })) => OutboundMsdu {
                            memory,
                            length: usize::from(length),
                            priority,
                        },
                        _ => break,
                    },
                };
                // The held TID's data waits, in the network's order, for the
                // request that names its next number.
                if sequence_hold.is_some() && plane.tid(msdu.priority) == *sequence_hold {
                    *deferred = Some(msdu);
                    break;
                }
                taken = taken.saturating_add(1);
                match plane.intake(&mut arena, msdu, link.numbers_mut(), now) {
                    Intake::Taken(tid) => *tid.select_mut(&mut tids) = true,
                    Intake::Wait(msdu) => {
                        *deferred = Some(msdu);
                        break;
                    }
                    Intake::Refused { memory, reason } => {
                        owed.refused(reason.into());
                        free(freed, memory);
                    }
                }
            }
            for tid in WmmUserPriority::ALL {
                if !*tid.select(&tids) {
                    continue;
                }
                let (told, next) = tid.select_mut(&mut owed.traffic);
                let quiet = told
                    .is_none_or(|told| now.saturating_duration_since(told) >= TX_TRAFFIC_REFRESH);
                if quiet {
                    *next = Some(link.numbers().next(tid));
                }
            }
        }
        // The budget ran out while the network still has MSDUs the standby
        // may take.
        if taken >= TX_DEPTH && self.accepts() && !network.transmit.is_empty() {
            self.again = true;
        }
        self.publish()
    }

    /// Publish the next arena while the radio runs and the link carries
    /// data: a single MPDU by its rate plan, an A-MPDU at the plan's first
    /// rate; a refusal returns the arena to the plane, which publishes it
    /// again at the next turn.
    pub(super) fn publish(&mut self) -> Result<(), Failed<'id, D>> {
        if !self.radio.running() || !self.link.as_ref().is_some_and(LinkHandover::carries) {
            return Ok(());
        }
        let Self {
            port,
            rates,
            data: DataTx { plane, .. },
            now,
            ..
        } = self;
        let Some(plane) = plane.as_mut() else {
            return Ok(());
        };
        if plane.publishable().is_none() {
            return Ok(());
        }
        let mut arena = port.arena();
        let publication = {
            let arena = &arena;
            plane.publication(|plan| {
                let rates = rates.plan(plan.frame_rates(), |rate| arena.rate(rate))?;
                Some(match plan.kind {
                    // A single MPDU goes by the whole plan, as any data MPDU.
                    PpduKind::Single => ArenaFormat::Single(rates),
                    PpduKind::Aggregate { spacing } => ArenaFormat::Aggregate {
                        rate: arena.ampdu_rate(rates.into_first().rate).ok()?,
                        spacing,
                    },
                })
            })
        };
        let Some(publication) = publication else {
            return Ok(());
        };
        match arena.publish(publication, *now) {
            Ok(()) => Ok(()),
            Err(PublishError::Refused { publication, .. }) => {
                plane.refused(publication);
                Ok(())
            }
            Err(PublishError::Fault(fault)) => Err(ActorFault::Port(fault)),
        }
    }

    /// Settle the arena's end `done`: its acknowledged and dropped frames'
    /// memory back to the network, a BlockAckReq owed for a frame dropped
    /// under an agreement, the lesson to the rate selection.
    pub(super) fn settle(
        &mut self,
        done: PortCompleted<'id, D::Port>,
    ) -> Result<(), Failed<'id, D>> {
        let Self {
            port,
            rates,
            events,
            data: DataTx { plane, freed, .. },
            owed,
            now,
            retained,
            ..
        } = self;
        let Some(plane) = plane.as_mut() else {
            retained.completed = Some(done);
            return Err(ActorFault::Contract(ActorViolation::ArenaCompletion));
        };
        let settled = {
            let mut arena = port.arena();
            plane.completed(&mut arena, done, *now, |memory| free(freed, memory), events)
        };
        let settled = match settled {
            Ok(settled) => settled,
            Err(SettleError::Unexpected(done)) => {
                retained.completed = Some(done);
                return Err(ActorFault::Contract(ActorViolation::ArenaCompletion));
            }
            Err(SettleError::Fault(fault)) => return Err(ActorFault::Port(fault)),
        };
        if let Some(lesson) = settled.lesson {
            lesson.teach(*now, rates, D::arena_ack);
        }
        if let Some((tid, head)) = settled.bar {
            *tid.select_mut(&mut owed.bar) = Some(head);
        }
        if settled.lesson.is_some() {
            self.aggregate_rate();
        }
        Ok(())
    }

    /// Give the plane the port's A-MPDU limits at the rate the aggregate
    /// rate plan names now; it computes its bounds again only when they
    /// changed.
    pub(super) fn aggregate_rate(&mut self) {
        let Self {
            port,
            rates,
            data: DataTx { plane, .. },
            ..
        } = self;
        let Some(plane) = plane.as_mut() else {
            return;
        };
        let arena = port.arena();
        let rate = rates
            .plan(FrameRates::Aggregate, |rate| arena.rate(rate))
            .map(|plan| plan.into_first().rate)
            .and_then(|rate| {
                let rate = arena.ampdu_rate(rate).ok()?.rate().get();
                let capabilities = arena.ampdu_capabilities();
                Some(AggregateRate {
                    rate,
                    max_subframes: capabilities.max_subframes,
                    max_length: capabilities.max_length.at(rate).ok()?.get(),
                })
            });
        plane.aggregate_rate(rate);
    }

    /// Take the link's data back from the plane: every frame its arenas hold
    /// unpublished and the waiting MSDU go back to the network; the arena on
    /// the air settles at its end.
    pub(super) fn stop_data(&mut self) -> Result<(), Failed<'id, D>> {
        let Self {
            port,
            events,
            data:
                DataTx {
                    plane,
                    deferred,
                    freed,
                },
            ..
        } = self;
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
        let mut arena = port.arena();
        plane
            .stop(&mut arena, |memory| free(freed, memory), events)
            .map(drop)
            .map_err(ActorFault::Port)
    }

    /// Withdraw the frames the arenas hold unpublished: they carry the
    /// packet numbers of the key a new installation or a removal replaces.
    pub(super) fn withdraw_unpublished(&mut self) -> Result<(), Failed<'id, D>> {
        let Self {
            port,
            events,
            data: DataTx { plane, freed, .. },
            ..
        } = self;
        let Some(plane) = plane.as_mut() else {
            return Ok(());
        };
        let mut arena = port.arena();
        plane
            .withdraw_unpublished(&mut arena, |memory| free(freed, memory), events)
            .map_err(ActorFault::Port)
    }

    /// Give the network the memory of ended frames while its queue has
    /// room.
    pub(super) fn return_freed<
        NS: NetworkStop,
        const TX_DEPTH: usize,
        const RX_DEPTH: usize,
        N: Notifies,
    >(
        &mut self,
        network: &mut NetworkEnds<'_, D::TxMemory, D::LentRx, NS, TX_DEPTH, RX_DEPTH, N>,
    ) {
        while network.returned.free_capacity() != 0 {
            let Some(memory) = self.data.freed.pop_front() else {
                return;
            };
            let mut memory = Some(memory);
            let _sent = network.returned.try_send_with(|slot| {
                *slot = memory.take();
                Ok::<_, core::convert::Infallible>(())
            });
            if let Some(memory) = memory {
                // The room was checked: the queue only keeps it when broken.
                let _quarantined = core::mem::ManuallyDrop::new(memory);
            }
        }
    }
}
