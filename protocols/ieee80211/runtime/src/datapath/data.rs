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

use super::queues::QueueId;
use super::turn::Failed;
use super::*;
use oer_protocols_ieee80211_contracts_port::arena::{ArenaFormat, PublishError, TxArena};
use oer_protocols_ieee80211_contracts_port::{PortArena, PortCompleted};
use oer_protocols_ieee80211_core_roles_sta::data::plane::{
    AggregateRate, Intake, PpduKind, SettleError, Waits,
};

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
    /// data and the queues have room.
    pub(super) fn accepts(&self) -> bool {
        self.carries() && self.data.queues.has_room()
    }

    /// How many queued MSDUs wake the actor (of the network's `depth`): one
    /// while no arena is on the air, so the next goes out at once; while
    /// one is, the standby's room up to half the queue, since the
    /// completion takes whatever is queued and the network should not fill
    /// its queue while the actor sleeps; none while the standby is full.
    pub(super) fn intake_want(&mut self, depth: usize) -> usize {
        let Some(plane) = self.data.plane.as_ref() else {
            return 1;
        };
        let capacity =
            oer_protocols_ieee80211_contracts_port::arena::TxArena::capacity(&self.port.arena());
        plane
            .standby_room(capacity)
            .map_or(1, |room| usize::from(room).min((depth / 2).max(1)))
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
        let mut yielded = false;
        {
            let Self {
                port,
                link,
                data:
                    DataTx {
                        plane,
                        queues,
                        returns,
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
            // The network's queue is the transport: take what it published
            // into the TIDs' queues, so none of them waits behind another.
            while queues.has_room() {
                let Ok(Some(Outgoing {
                    memory,
                    length,
                    priority,
                })) = network.transmit.try_receive_held_with(Option::take)
                else {
                    break;
                };
                let msdu = OutboundMsdu {
                    memory,
                    length: usize::from(length),
                    priority,
                };
                let tid = plane.tid(priority).unwrap_or(priority);
                if let Err(msdu) = queues.push(QueueId::of_tid(tid), msdu) {
                    // The room was checked: a full pool keeps it quarantined.
                    let _quarantined = core::mem::ManuallyDrop::new(msdu);
                }
            }
            let mut arena = port.arena();
            while taken < TX_DEPTH {
                // The standby's TID goes on while its frames follow; else
                // the next arena starts from the queue the MAC chooses. The
                // held TID's data waits for the request that names its next
                // number.
                let hold = sequence_hold.map(QueueId::of_tid);
                let queue = match plane.standby_tid().map(QueueId::of_tid) {
                    Some(queue) if queues.ready(queue) && Some(queue) != hold => queue,
                    Some(_) => break,
                    None => match queues.select(hold) {
                        Some(queue) => queue,
                        None => break,
                    },
                };
                let Some(msdu) = queues.pop(queue) else {
                    break;
                };
                taken = taken.saturating_add(1);
                match plane.intake(&mut arena, msdu, link.numbers_mut(), now) {
                    Intake::Taken(tid) => *tid.select_mut(&mut tids) = true,
                    Intake::Wait(msdu, waits) => {
                        let queue =
                            QueueId::of_tid(plane.tid(msdu.priority).unwrap_or(msdu.priority));
                        if let Err(msdu) = queues.push_front(queue, msdu) {
                            // The place it left takes it back.
                            let _quarantined = core::mem::ManuallyDrop::new(msdu);
                        }
                        match waits {
                            // Only this TID waits for its window: another
                            // may start the arena.
                            Waits::Window => queues.block(queue),
                            Waits::Arena | Waits::Standby => break,
                        }
                    }
                    Intake::Refused { memory, reason } => {
                        owed.refused(reason.into());
                        // Refused before any arena: it never went.
                        returns.free(memory);
                    }
                }
                // The air waits for a completion, not for the next frame.
                if arena.event_waiting() {
                    yielded = true;
                    break;
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
        // The burst's slots go back to the network at once.
        network.transmit.commit();
        // The budget ran out while a queue is still ready for the standby,
        // or the network has MSDUs the queues may take.
        let more = self.data.queues.has_ready() || !network.transmit.is_empty();
        if (yielded || taken >= TX_DEPTH) && self.carries() && more {
            self.again = true;
        }
        self.publish()
    }

    /// Publish the next arena while the radio runs and the link carries
    /// data: a single MPDU by its rate plan, an A-MPDU at the plan's first
    /// rate; a refusal returns the arena to the plane, which publishes it
    /// again at the next turn.
    pub(super) fn publish(&mut self) -> Result<(), Failed<'id, D>> {
        self.publish_next(None)
    }

    /// Publish the other arena ahead of the settlement of `done` when the
    /// plane finds it the next one whatever the settlement does: the air
    /// waits for none of the settlement's work.
    pub(super) fn publish_ahead(
        &mut self,
        done: &PortCompleted<'id, D::Port>,
    ) -> Result<(), Failed<'id, D>> {
        self.publish_next(Some(done))
    }

    fn publish_next(
        &mut self,
        ended: Option<&PortCompleted<'id, D::Port>>,
    ) -> Result<(), Failed<'id, D>> {
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
        if ended.is_none() && plane.publishable().is_none() {
            return Ok(());
        }
        let mut arena = port.arena();
        let publication = {
            let arena = &arena;
            let format = |plan: oer_protocols_ieee80211_core_roles_sta::data::plane::PpduPlan| {
                let rates = rates.plan(plan.frame_rates(), |rate| arena.rate(rate))?;
                Some(match plan.kind {
                    // A single MPDU goes by the whole plan, as any data MPDU.
                    PpduKind::Single => ArenaFormat::Single(rates),
                    PpduKind::Aggregate { spacing } => ArenaFormat::Aggregate {
                        rate: arena.ampdu_rate(rates.into_first().rate).ok()?,
                        spacing,
                    },
                })
            };
            match ended {
                Some(done) => plane.publication_ahead(done, format),
                None => plane.publication(format),
            }
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
            Err(PublishError::FaultBeforeTaking { publication, fault }) => {
                plane.refused(publication);
                Err(ActorFault::Port(fault))
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
            data:
                DataTx {
                    plane,
                    queues,
                    returns,
                },
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
            plane.completed(
                &mut arena,
                done,
                *now,
                |memory| returns.free(memory),
                events,
            )
        };
        let settled = match settled {
            Ok(settled) => settled,
            Err(SettleError::Unexpected(done)) => {
                retained.completed = Some(done);
                return Err(ActorFault::Contract(ActorViolation::ArenaCompletion));
            }
            Err(SettleError::Fault(fault)) => return Err(ActorFault::Port(fault)),
        };
        // One charge for the attempt; its end may move the windows.
        queues.charge(QueueId::of_tid(settled.tid), settled.cost);
        queues.unblock();
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
                    queues,
                    returns,
                },
            ..
        } = self;
        // What waited in the queues never went.
        queues.drain(|msdu| {
            report(events, || {
                oer_protocols_ieee80211_core_roles_sta::events::StaEvent::DataLost(
                    oer_protocols_ieee80211_core_roles_sta::events::DataLoss::LeftBehind,
                )
            });
            returns.free(msdu.memory);
        });
        let Some(plane) = plane.as_mut() else {
            return Ok(());
        };
        let mut arena = port.arena();
        plane
            .stop(&mut arena, |memory| returns.free(memory), events)
            .map(drop)
            .map_err(ActorFault::Port)
    }

    /// Withdraw the frames the arenas hold unpublished: they carry the
    /// packet numbers of the key a new installation or a removal replaces.
    pub(super) fn withdraw_unpublished(&mut self) -> Result<(), Failed<'id, D>> {
        let Self {
            port,
            events,
            data: DataTx { plane, returns, .. },
            ..
        } = self;
        let Some(plane) = plane.as_mut() else {
            return Ok(());
        };
        let mut arena = port.arena();
        plane
            .withdraw_unpublished(&mut arena, |memory| returns.free(memory), events)
            .map_err(ActorFault::Port)
    }

    /// While a stop owes the network its stop
    /// ([`NetworkStopping::Owed`]): hand back the MSDUs the station's queues
    /// and the network's transmit queue still hold, each as data left
    /// behind; once the stopped radio holds none of the network's memory,
    /// ask the network to stop ([`NetworkReturn::return_left`]). The
    /// network's last turn then takes back all its memory.
    ///
    /// [`NetworkStopping::Owed`]: crate::network_return::NetworkStopping::Owed
    /// [`NetworkReturn::return_left`]: crate::network_return::NetworkReturn::return_left
    pub(super) fn return_network_memory<
        NS: NetworkStop,
        const TX_DEPTH: usize,
        const RX_DEPTH: usize,
        N: Notifies,
    >(
        &mut self,
        network: &mut NetworkEnds<'_, D::TxMemory, D::LentRx, NS, TX_DEPTH, RX_DEPTH, N>,
    ) {
        let may_ask =
            matches!(self.radio, Radio::Draining { .. } | Radio::Returning) && self.data_settled();
        let Self { data, events, .. } = self;
        let DataTx {
            queues, returns, ..
        } = data;
        returns.return_left::<PortReturns<D>, _, _, TX_DEPTH, RX_DEPTH, N>(
            network,
            || queues.pop_any().map(|msdu| msdu.memory),
            || {
                report(events, || {
                    oer_protocols_ieee80211_core_roles_sta::events::StaEvent::DataLost(
                        oer_protocols_ieee80211_core_roles_sta::events::DataLoss::LeftBehind,
                    )
                });
            },
            may_ask,
        );
    }

    /// Give the network the memory of ended frames while its queue has
    /// room, as one burst
    /// ([`crate::network_return::NetworkReturn::return_freed`]).
    pub(super) fn return_freed<
        NS: NetworkStop,
        const TX_DEPTH: usize,
        const RX_DEPTH: usize,
        N: Notifies,
    >(
        &mut self,
        network: &mut NetworkEnds<'_, D::TxMemory, D::LentRx, NS, TX_DEPTH, RX_DEPTH, N>,
    ) {
        self.data
            .returns
            .return_freed::<PortReturns<D>, _, _, TX_DEPTH, RX_DEPTH, N>(network);
    }
}

/// The port's observation of the actor's return bursts.
struct PortReturns<D>(core::marker::PhantomData<D>);

impl<'id, D: DatapathPort<'id>> crate::network_return::ReturnObserver for PortReturns<D> {
    const OBSERVES: bool = D::OBSERVES_RETURNS;
    fn returned(burst: ReturnBurst) {
        D::returned(burst);
    }
}
