//! The actor's receive path: units from the port through the receive
//! agreements' reordering and the link's admission to the network, the
//! station's frames to the lifecycle.
//!
//! A frame that delivered exactly one MSDU is lent to the network in place;
//! the others are copied. While the network holds all the units the port
//! may lend, the frames in delivery order wait and the actor pulls no more,
//! so the port keeps further units in its ring: a burst the network is on
//! its way to loses nothing, and a network that takes slowly paces the
//! ring, whose full slots the recipient does not acknowledge, so the peer
//! sends them again. A network that returns nothing for [`NETWORK_LATE`]
//! no longer holds the ring back: the actor walks it, a frame of one MSDU
//! is copied by pressure while the copy queue has room, and an MSDU no
//! queue takes is refused, counted ([`RxMsdusRefused`]), its buffer back to
//! the ring at once. So the station's frames, the link's admission and the
//! receive agreements go on whatever the network takes. The walk lasts
//! until the network has room again. The units the network holds stay lent
//! until it drops them; the port never lends past its limit.

use super::turn::{Failed, PortBlockAcks};
use super::*;
use crate::station::{CopiedMark, NetworkRx, RxDrop, RxMsdusCopied, RxMsdusRefused, ServiceEvent};
use oer_protocols_ieee80211_contracts_port::RxPull;
use oer_protocols_ieee80211_core_roles_sta::data::{
    MAX_ETHERNET_PAYLOAD_LEN, MsduDelivery, MsduPlan, PlannedCopyError,
};
use oer_protocols_ieee80211_core_roles_sta::link::LinkRx;
use oer_protocols_ieee80211_core_roles_sta::machine::{StaEapol, StaReceived};
use oer_protocols_ieee80211_core_upper_mac::block_ack::{Parked, RxDisposition};
use oer_protocols_network_port::{CopyReason, LentFrame};

/// Why the actor did not deliver one received MSDU.
#[derive(Clone, Copy)]
enum Refusal {
    TooLong,
    QueueFull,
    /// The port kept the buffer and the copy found no room.
    Pressure,
    NoNetwork,
}

fn count(refused: &mut RxMsdusRefused, refusal: Option<Refusal>) {
    match refusal {
        Some(Refusal::TooLong) => refused.too_long = refused.too_long.saturating_add(1),
        Some(Refusal::QueueFull) => refused.queue_full = refused.queue_full.saturating_add(1),
        Some(Refusal::Pressure) => refused.pressure = refused.pressure.saturating_add(1),
        Some(Refusal::NoNetwork) => refused.no_network = refused.no_network.saturating_add(1),
        None => {}
    }
}

/// Copy one received MSDU of `frame` to the network queue; a refusal
/// returns its reason.
fn copy_received<
    L,
    NS: NetworkStop,
    const TX_DEPTH: usize,
    const RX_DEPTH: usize,
    N: Notifies,
    M,
>(
    network: &NetworkEnds<'_, M, L, NS, TX_DEPTH, RX_DEPTH, N>,
    frame: &[u8],
    plan: &MsduPlan,
    reason: CopyReason,
) -> Option<Refusal> {
    // The mark follows its copy: room for both, or neither is sent.
    if network.receive.free_capacity() == 0 || network.receive_copied.free_capacity() == 0 {
        return Some(Refusal::QueueFull);
    }
    match EthernetFrame::copy_planned(frame, plan) {
        Ok(frame) => (network.receive_copied.try_send(frame).is_err()
            || network
                .receive
                .try_send_held(Received::Copied(CopiedMark::new(reason)))
                .is_err())
        .then_some(Refusal::QueueFull),
        // The link located the payload in this frame.
        Err(PlannedCopyError::TooLong(_) | PlannedCopyError::OutsideFrame) => {
            Some(Refusal::TooLong)
        }
    }
}

/// Hand one lent MSDU to the network queue; a refusal drops the lent
/// buffer, which returns it to the port, and returns its reason.
fn queue_lent<
    L: LentRxBuffer,
    NS: NetworkStop,
    const TX_DEPTH: usize,
    const RX_DEPTH: usize,
    N: Notifies,
    M,
>(
    network: &NetworkEnds<'_, M, L, NS, TX_DEPTH, RX_DEPTH, N>,
    lent: L,
    plan: MsduPlan,
) -> Option<Refusal> {
    let MsduPlan {
        destination,
        source,
        ether_type,
        payload,
    } = plan;
    if payload.len() > MAX_ETHERNET_PAYLOAD_LEN {
        return Some(Refusal::TooLong);
    }
    match LentFrame::new(NetworkRx(lent), destination, source, ether_type, payload) {
        Ok(frame) => network
            .receive
            .try_send_held(Received::Lent(frame))
            .is_err()
            .then_some(Refusal::QueueFull),
        // The payload lies in the MPDU, which starts the lent storage.
        Err(_storage) => Some(Refusal::TooLong),
    }
}

/// Where `part` starts and ends in `whole`, of which it is a part.
fn range_in(whole: &[u8], part: &[u8]) -> Option<core::ops::Range<u16>> {
    let start = part.as_ptr().addr().checked_sub(whole.as_ptr().addr())?;
    let end = start.checked_add(part.len())?;
    if end > whole.len() {
        return None;
    }
    Some(u16::try_from(start).ok()?..u16::try_from(end).ok()?)
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
    /// Whether the network takes frames and holds all it may: a lend would
    /// be refused for capacity.
    fn network_full<NS: NetworkStop, const TX_DEPTH: usize, const RX_DEPTH: usize, N: Notifies>(
        &self,
        network: &NetworkEnds<'_, D::TxMemory, D::LentRx, NS, TX_DEPTH, RX_DEPTH, N>,
    ) -> bool {
        network.network.takes_frames() && !self.port.lend_room()
    }

    /// Whether frames have waited for the network for [`NETWORK_LATE`].
    pub(super) fn network_late(&self) -> bool {
        self.held_back
            .is_some_and(|since| since.saturating_add(NETWORK_LATE) <= self.now)
    }

    /// Whether received frames wait for the network: it holds all it may
    /// and has not been late for [`NETWORK_LATE`] yet.
    fn holds_back<NS: NetworkStop, const TX_DEPTH: usize, const RX_DEPTH: usize, N: Notifies>(
        &self,
        network: &NetworkEnds<'_, D::TxMemory, D::LentRx, NS, TX_DEPTH, RX_DEPTH, N>,
    ) -> bool {
        self.network_full(network) && !self.network_late()
    }

    /// A network with room returned a unit since frames began to wait: it
    /// makes progress, however slowly, and the next back-pressure starts
    /// its own bound.
    fn progressed<NS: NetworkStop, const TX_DEPTH: usize, const RX_DEPTH: usize, N: Notifies>(
        &mut self,
        network: &NetworkEnds<'_, D::TxMemory, D::LentRx, NS, TX_DEPTH, RX_DEPTH, N>,
    ) {
        if !self.network_full(network) {
            self.held_back = None;
        }
    }

    /// The turn's receive work: the frames in delivery order the network
    /// takes, the deferred management frame, then at most [`RX_BUDGET`]
    /// units of the ring and the agreements' due deadlines.
    pub(super) fn rx<
        const TO: usize,
        const FROM: usize,
        const RETURNS: usize,
        NS: NetworkStop,
        const TX_DEPTH: usize,
        const RX_DEPTH: usize,
        N: Notifies,
    >(
        &mut self,
        signalled: bool,
        lifecycle: &mut LifecycleEnds<'_, 'id, D, TO, FROM, RETURNS>,
        network: &mut NetworkEnds<'_, D::TxMemory, D::LentRx, NS, TX_DEPTH, RX_DEPTH, N>,
    ) -> Result<(), Failed<'id, D>> {
        let now = self.now;
        self.progressed(network);
        self.pending(lifecycle, network)?;
        let continued = self.rx_later.is_some_and(|at| at <= now);
        if (signalled || continued) && self.radio_receives() {
            self.rx_later = None;
            let mut pulled = 0_u8;
            loop {
                if pulled >= RX_BUDGET {
                    // The budget ran out with the ring's work left: the
                    // next turn pulls on.
                    self.facts.rx = true;
                    self.again = true;
                    break;
                }
                if self.receive.has_ready() && self.holds_back(network) {
                    // Back-pressure: the network's next return, or its
                    // lateness bound, wakes us.
                    self.held_back.get_or_insert(now);
                    self.facts.rx = true;
                    break;
                }
                let room = D::RX_HELD.saturating_sub(self.frames_out);
                match self.port.receive(room, now).map_err(ActorFault::Port)? {
                    RxPull::Unit { frame, meta } => {
                        pulled = pulled.saturating_add(1);
                        self.progressed = true;
                        D::received(frame.bytes(), &meta);
                        if matches!(self.radio, Radio::Stopping { .. }) {
                            // A stopping radio's units go back unread.
                            self.port.recycle(frame);
                            report(&mut self.events, || {
                                ServiceEvent::RxFrameDropped(RxDrop::Draining)
                            });
                        } else {
                            self.received(frame, meta, lifecycle, network)?;
                        }
                    }
                    RxPull::Empty => break,
                    RxPull::Later(at) if at <= now => {
                        // The port's call ended at its own bound with the
                        // ring's work left: no alarm keeps an instant
                        // already reached, so the next turn pulls on after
                        // one repost.
                        self.facts.rx = true;
                        self.again = true;
                        break;
                    }
                    RxPull::Later(at) => {
                        self.rx_later = Some(at);
                        break;
                    }
                }
            }
        }
        if self.receive.deadline().is_some_and(|at| at <= now) {
            self.receive
                .tick(now, &mut PortBlockAcks(&mut self.port), &mut self.events);
            self.pending(lifecycle, network)?;
        }
        if self.receive.failed() {
            return Err(ActorFault::Contract(ActorViolation::ReorderOverflow));
        }
        Ok(())
    }

    /// Whether the ring gives units now: the radio runs or winds down.
    fn radio_receives(&self) -> bool {
        matches!(
            self.radio,
            Radio::Running | Radio::Stopping { .. } | Radio::Retuning { .. }
        )
    }

    /// Hand on the frames in delivery order the network takes, and the
    /// deferred management frames once their replies have a place.
    fn pending<
        const TO: usize,
        const FROM: usize,
        const RETURNS: usize,
        NS: NetworkStop,
        const TX_DEPTH: usize,
        const RX_DEPTH: usize,
        N: Notifies,
    >(
        &mut self,
        lifecycle: &mut LifecycleEnds<'_, 'id, D, TO, FROM, RETURNS>,
        network: &mut NetworkEnds<'_, D::TxMemory, D::LentRx, NS, TX_DEPTH, RX_DEPTH, N>,
    ) -> Result<(), Failed<'id, D>> {
        self.release_ready(lifecycle, network)?;
        while let Some((frame, meta)) = self.receive.pop_deferred() {
            self.received(frame, meta, lifecycle, network)?;
        }
        Ok(())
    }

    /// Hand on the frames the receive agreements released in delivery
    /// order, while the network does not hold them back.
    fn release_ready<
        const TO: usize,
        const FROM: usize,
        const RETURNS: usize,
        NS: NetworkStop,
        const TX_DEPTH: usize,
        const RX_DEPTH: usize,
        N: Notifies,
    >(
        &mut self,
        lifecycle: &mut LifecycleEnds<'_, 'id, D, TO, FROM, RETURNS>,
        network: &mut NetworkEnds<'_, D::TxMemory, D::LentRx, NS, TX_DEPTH, RX_DEPTH, N>,
    ) -> Result<(), Failed<'id, D>> {
        while !self.holds_back(network) {
            let Some((frame, meta)) = self.receive.pop_ready() else {
                break;
            };
            self.route(frame, meta, lifecycle, network)?;
        }
        Ok(())
    }

    /// Take one received unit: reorder it or hand it on, after the frames
    /// it released or found released in delivery order.
    pub(super) fn received<
        const TO: usize,
        const FROM: usize,
        const RETURNS: usize,
        NS: NetworkStop,
        const TX_DEPTH: usize,
        const RX_DEPTH: usize,
        N: Notifies,
    >(
        &mut self,
        frame: ActorRxBuffer<'id, D>,
        meta: ActorRxMeta<'id, D>,
        lifecycle: &mut LifecycleEnds<'_, 'id, D, TO, FROM, RETURNS>,
        network: &mut NetworkEnds<'_, D::TxMemory, D::LentRx, NS, TX_DEPTH, RX_DEPTH, N>,
    ) -> Result<(), Failed<'id, D>> {
        let local = self
            .link
            .as_ref()
            .filter(|link| link.carries())
            .map(|link| link.rx().address().get());
        let (frame, meta) =
            match self
                .receive
                .receive(self.now, local, frame, meta, &mut self.events)
            {
                RxDisposition::Deliver(received) => received,
                RxDisposition::Retained => return self.release_ready(lifecycle, network),
                RxDisposition::Recycle(frame) => {
                    // A BlockAckReq may have released frames.
                    self.port.recycle(frame);
                    return self.release_ready(lifecycle, network);
                }
                RxDisposition::Capacity(frame) => {
                    self.port.recycle(frame);
                    report(&mut self.events, || ActorEvent::ReorderFull);
                    return Ok(());
                }
            };
        self.release_ready(lifecycle, network)?;
        // A data frame waits behind the frames still waiting, or while the
        // network holds it back: neither copied nor dropped.
        if self.receive.has_ready() || self.holds_back(network) {
            return match self.receive.park(frame, meta) {
                Parked::Waiting => Ok(()),
                Parked::Deliver((frame, meta)) => self.route(frame, meta, lifecycle, network),
                Parked::Capacity(frame) => {
                    self.port.recycle(frame);
                    report(&mut self.events, || ActorEvent::ReorderFull);
                    Ok(())
                }
            };
        }
        self.route(frame, meta, lifecycle, network)
    }

    /// Give a frame to the lifecycle while its lane has room; otherwise it
    /// goes back to the ring, counted.
    fn hand_to_lifecycle<const TO: usize, const FROM: usize, const RETURNS: usize>(
        &mut self,
        message: FromActor<'id, D>,
        lifecycle: &LifecycleEnds<'_, 'id, D, TO, FROM, RETURNS>,
    ) {
        let lane = usize::from(self.frames_out) < LIFECYCLE_FRAMES;
        let refused = if lane {
            match lifecycle.from_actor.try_send(message) {
                Ok(()) => {
                    self.frames_out = self.frames_out.saturating_add(1);
                    return;
                }
                Err(refused) => refused.0,
            }
        } else {
            message
        };
        match refused {
            FromActor::Received { frame, .. } | FromActor::Eapol { frame, .. } => {
                self.port.recycle(frame);
            }
            // Only frames take the lane.
            other => {
                let _quarantined = core::mem::ManuallyDrop::new(other);
            }
        }
        report(&mut self.events, || ActorEvent::LaneFull);
    }

    /// Hand on one frame in delivery order: the link admits its data to the
    /// network and its EAPOL to the lifecycle; any other frame, and every
    /// frame before the link, is the station's. A frame that delivered
    /// exactly one MSDU and nothing for the station is lent in place.
    fn route<
        const TO: usize,
        const FROM: usize,
        const RETURNS: usize,
        NS: NetworkStop,
        const TX_DEPTH: usize,
        const RX_DEPTH: usize,
        N: Notifies,
    >(
        &mut self,
        frame: ActorRxBuffer<'id, D>,
        meta: ActorRxMeta<'id, D>,
        lifecycle: &mut LifecycleEnds<'_, 'id, D, TO, FROM, RETURNS>,
        network: &mut NetworkEnds<'_, D::TxMemory, D::LentRx, NS, TX_DEPTH, RX_DEPTH, N>,
    ) -> Result<(), Failed<'id, D>> {
        let Some(link) = self.link.as_mut() else {
            self.hand_to_lifecycle(FromActor::Received { frame, meta }, lifecycle);
            return Ok(());
        };
        // Delivery runs for every sink; only an enabled one counts.
        let counted = <O as EventSink<ServiceEvent>>::ENABLED;
        let mut refused = RxMsdusRefused::default();
        let mut copied = RxMsdusCopied::default();
        // The first MSDU waits: it is lent if it stays the frame's only one.
        let mut first: Option<MsduPlan> = None;
        let mut several = false;
        let mut eapol: Option<(core::ops::Range<u16>, EapolProtection)> = None;
        let mut eapols = 0_u8;
        let takes = network.network.takes_frames();
        let routed = {
            let bytes = frame.bytes();
            let deliver = |delivery: MsduDelivery<'_>| {
                if !takes {
                    if counted {
                        count(&mut refused, Some(Refusal::NoNetwork));
                    }
                    return;
                }
                if first.is_none() && !several {
                    first = Some(delivery.plan());
                } else {
                    several = true;
                    let earlier = first
                        .take()
                        .map(|plan| copy_received(network, bytes, &plan, CopyReason::Format));
                    let refusal =
                        copy_received(network, bytes, &delivery.plan(), CopyReason::Format);
                    if counted {
                        for refusal in earlier.into_iter().chain([refusal]) {
                            count(&mut refused, refusal);
                            copied.aggregated = copied.aggregated.saturating_add(1);
                        }
                    }
                }
            };
            link.receive(bytes, &meta, deliver, |payload, protection| {
                // The station takes the link's EAPOL in its own task: the
                // frame goes to it, its first EAPOL packet named.
                eapols = eapols.saturating_add(1);
                if eapol.is_none() {
                    eapol = range_in(bytes, payload).map(|range| (range, protection));
                }
                Ok(StaEapol::Taken)
            })
        };
        let result = match routed {
            LinkRx::Station => {
                self.hand_to_lifecycle(FromActor::Received { frame, meta }, lifecycle);
                return Ok(());
            }
            LinkRx::Taken(result) => result,
        };
        match eapol {
            Some((payload, protection)) => {
                // The frame goes to the lifecycle: its data MSDU is copied.
                if let Some(plan) = first.take() {
                    let refusal = copy_received(network, frame.bytes(), &plan, CopyReason::Format);
                    if counted {
                        count(&mut refused, refusal);
                        copied.shared = copied.shared.saturating_add(1);
                    }
                }
                self.hand_to_lifecycle(
                    FromActor::Eapol {
                        frame,
                        payload,
                        protection,
                    },
                    lifecycle,
                );
            }
            None => match first {
                None => self.port.recycle(frame),
                Some(plan) => {
                    let refusal = match self.port.lend(frame) {
                        Ok(lent) => queue_lent(network, lent, plan),
                        Err(refusal) => {
                            let buffer = refusal.buffer;
                            let refusal =
                                copy_received(network, buffer.bytes(), &plan, CopyReason::Pressure)
                                    .map(|refusal| match refusal {
                                        Refusal::QueueFull => Refusal::Pressure,
                                        other => other,
                                    });
                            if counted {
                                copied.kept = copied.kept.saturating_add(1);
                            }
                            self.port.recycle(buffer);
                            refusal
                        }
                    };
                    if counted {
                        count(&mut refused, refusal);
                    }
                }
            },
        }
        if refused != RxMsdusRefused::default() {
            report(&mut self.events, || ServiceEvent::RxMsdusRefused(refused));
        }
        if copied != RxMsdusCopied::default() {
            report(&mut self.events, || ServiceEvent::RxMsdusCopied(copied));
        }
        match result {
            Ok(StaReceived::Data | StaReceived::Input) => {}
            Err(reason) => report(&mut self.events, || {
                ServiceEvent::RxFrameDropped(RxDrop::Station(reason))
            }),
        }
        if eapols > 1 {
            // One EAPOL packet per frame reaches the station; an access
            // point sends no more.
            report(&mut self.events, || ActorEvent::LaneFull);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msdu_refusals_of_one_frame_aggregate_by_reason_whatever_their_order() {
        let mut refused = RxMsdusRefused::default();
        for refusal in [
            Some(Refusal::QueueFull),
            None,
            Some(Refusal::TooLong),
            Some(Refusal::QueueFull),
            Some(Refusal::NoNetwork),
            Some(Refusal::Pressure),
        ] {
            count(&mut refused, refusal);
        }
        assert_eq!(
            refused,
            RxMsdusRefused {
                too_long: 1,
                queue_full: 2,
                pressure: 1,
                no_network: 1,
            }
        );
        let mut saturated = RxMsdusRefused {
            too_long: u16::MAX,
            ..RxMsdusRefused::default()
        };
        count(&mut saturated, Some(Refusal::TooLong));
        assert_eq!(saturated.too_long, u16::MAX);
    }
}
