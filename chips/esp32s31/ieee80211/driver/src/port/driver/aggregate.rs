//! Aggregates in the driver: their place in an access category's FIFO
//! beside MPDUs, their publication through a descriptor chain, their
//! service and their one terminal. An aggregate is one attempt; the planner
//! above the port resends what its BlockAck report misses.
use super::*;
use crate::lower_mac::{
    AmpduProgress, AmpduServed, AmpduValues, Esp32s31AmpduBuffer, PublishedAmpdu,
};
use crate::mac::tx::{TxPhyRate, ampdu::HtAmpduTxError};
use crate::port::aggregate::{AggregateExecution, Subframes, TX_AMPDU_SUBFRAMES};
use oer_memory::StableDmaBacking as _;

/// One admitted aggregate on its way to the hardware.
pub(in crate::port) struct PendingAggregate<
    'id,
    Id: SlotIdentity,
    D: Device<B>,
    const B: usize,
    O: Observations,
> {
    pub operation:
        InFlight<D::OwnedContext<Id>, Slot<'id, Tickets, Part<Id, 2>>, Subframes<'id, Id, D, B, O>>,
    pub execution: AggregateExecution,
    pub category: WmmAccessCategory,
    pub protection: Protection,
    pub power: TxPower,
    /// The recipient's Minimum MPDU Start Spacing encoding, 0 to 7.
    pub min_mpdu_start_spacing: u8,
    /// The earliest admission of its subframes.
    pub oldest: Option<Instant>,
}

impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>
    PendingAggregate<'_, Id, D, B, O>
{
    fn role(&self) -> VifRole {
        self.operation.payload().role()
    }
    fn key(&self) -> Option<RawKeyIdentity> {
        self.operation.payload().key()
    }
}

/// An access category's FIFO entry: an MPDU or an aggregate.
pub(super) enum Queued<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    Single(Pending<'id, Id, D, B, O>),
    Aggregate(PendingAggregate<'id, Id, D, B, O>),
}

impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> Queued<'_, Id, D, B, O> {
    pub(super) fn role(&self) -> VifRole {
        match self {
            Self::Single(pending) => pending.role(),
            Self::Aggregate(pending) => pending.role(),
        }
    }
    pub(super) fn key(&self) -> Option<RawKeyIdentity> {
        match self {
            Self::Single(pending) => pending.key(),
            Self::Aggregate(pending) => pending.key(),
        }
    }
    pub(super) fn category(&self) -> WmmAccessCategory {
        match self {
            Self::Single(pending) => pending.category,
            Self::Aggregate(pending) => pending.category,
        }
    }
    /// The earliest admission it holds.
    pub(super) fn admission(&self) -> Option<Instant> {
        match self {
            Self::Single(pending) => pending.operation.payload().buffer.admission,
            Self::Aggregate(pending) => pending.oldest,
        }
    }
}

/// The published attempt of an EDCA queue.
pub(super) enum Running<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> {
    Single(Pending<'id, Id, D, B, O>),
    Aggregate {
        pending: PendingAggregate<'id, Id, D, B, O>,
        published: Published<'id, Id, B>,
    },
}

type Published<'id, Id, const B: usize> =
    PublishedAmpdu<'id, 'static, TxBacking<B>, TX_AMPDU_SUBFRAMES, CorePath<Id>>;

impl<Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations> Running<'_, Id, D, B, O> {
    pub(super) fn role(&self) -> VifRole {
        match self {
            Self::Single(pending) => pending.role(),
            Self::Aggregate { pending, .. } => pending.role(),
        }
    }
    pub(super) fn key(&self) -> Option<RawKeyIdentity> {
        match self {
            Self::Single(pending) => pending.key(),
            Self::Aggregate { pending, .. } => pending.key(),
        }
    }
    /// A published aggregate's deadline.
    pub(super) fn deadline(&self) -> Option<Instant> {
        match self {
            Self::Single(_) => None,
            Self::Aggregate { published, .. } => Some(published.deadline()),
        }
    }
}

/// An aggregate's portable outcome after its one publication at `rate`.
fn outcome(
    end: PublicationEnd,
    rate: PhyRate,
) -> Result<AggregateOutcome<BlockAckAnswer, Infallible>, Fault> {
    let air = Air {
        transmissions: core::num::NonZeroU16::MIN,
        rate,
    };
    Ok(match end {
        PublicationEnd::Expired { msdus } => TxOutcome::Expired { msdus },
        PublicationEnd::Withdrawn => TxOutcome::Withdrawn(WithdrawCause::Disabled),
        PublicationEnd::TimedOut
        | PublicationEnd::Completed {
            status: TxStatus::TimedOut,
            ..
        } => TxOutcome::Indeterminate(IndeterminateCause::HardwareTimeout),
        PublicationEnd::Collision
        | PublicationEnd::Completed {
            status: TxStatus::Collision,
            ..
        } => TxOutcome::Ended(AggregateEnd::ContentionLimit(None)),
        PublicationEnd::Completed {
            status: TxStatus::Success,
            ack_snr_db,
            block_ack,
        } => TxOutcome::Ended(AggregateEnd::Delivered {
            air,
            // The core reports success only with a valid BlockAck.
            report: block_ack.ok_or(Fault::Capacity)?,
            ack: BlockAckAnswer { snr_db: ack_snr_db },
        }),
        PublicationEnd::Completed {
            status: TxStatus::AckTimeout,
            ..
        } => TxOutcome::Ended(AggregateEnd::AckTimeout(air)),
        // The protection exchange failed: no subframe went out.
        PublicationEnd::Completed {
            status: TxStatus::CtsTimeout,
            ..
        } => TxOutcome::Ended(AggregateEnd::CtsTimeout(None)),
    })
}

impl<'id, Id: SlotIdentity, D: Device<B>, const B: usize, O: Observations>
    Driver<'id, Id, D, B, O>
{
    /// Whether an aggregate, of `role` when given, is published.
    pub(super) fn aggregate_running(&self, role: Option<VifRole>) -> bool {
        self.running.iter().flatten().any(|running| {
            matches!(running, Running::Aggregate { .. })
                && role.is_none_or(|role| running.role() == role)
        })
    }

    /// Whether a descriptor chain is free for the next aggregate.
    pub(super) fn chain_free(&self) -> bool {
        self.chains.iter().any(Option::is_some)
    }

    /// The aggregate's one terminal, returning every subframe.
    pub(super) fn finish_aggregate(
        &mut self,
        held: &Held<'_, 'id, Id, D, B, O>,
        pending: PendingAggregate<'id, Id, D, B, O>,
        outcome: AggregateOutcome<BlockAckAnswer, Infallible>,
        published: bool,
    ) {
        if published {
            core_views::reset_contention::<Id, D, B>(
                self.core.view(),
                pending.role(),
                pending.category,
            );
        }
        if let TxOutcome::Expired { msdus } = outcome {
            held.storage.expired.set(
                held.storage
                    .expired
                    .get()
                    .saturating_add(msdus.count_ones()),
            );
        }
        held.terminal(owned::OwnedTerminal::TxCompleted(
            owned::OwnedTxCompleted::Aggregate(pending.operation.complete(outcome)),
        ));
    }

    /// End a queued aggregate whose oldest subframe aged: the subframes that
    /// did, by bit, and every owner returns. `false` when none aged.
    pub(super) fn expire_aggregate(
        &mut self,
        held: &Held<'_, 'id, Id, D, B, O>,
        mut pending: PendingAggregate<'id, Id, D, B, O>,
        now: Instant,
    ) -> Result<(), PendingAggregate<'id, Id, D, B, O>> {
        let lifetime = held.edca.lifetime(vif(pending.role()), pending.category);
        let msdus = pending.operation.execute().expired(now, lifetime);
        if msdus == 0 {
            return Err(pending);
        }
        self.finish_aggregate(held, pending, TxOutcome::Expired { msdus }, false);
        Ok(())
    }

    /// Publish the aggregate at the head of queue `index` through a free
    /// descriptor chain; its every subframe's buffer goes to the chain.
    pub(super) fn publish_aggregate(
        &mut self,
        held: &Held<'_, 'id, Id, D, B, O>,
        device: &mut D,
        index: usize,
        mut pending: PendingAggregate<'id, Id, D, B, O>,
    ) -> Result<(), Fault> {
        let role = pending.role();
        let rate = TxPhyRate::try_from(pending.execution.rate).map_err(|_| Fault::Capacity)?;
        let CoreView::Open(mut core) = self.core.view() else {
            return Err(Fault::Capacity);
        };
        let observations = self.observations.as_mut().ok_or(Fault::Capacity)?;
        let backoff_slots = core.draw_backoff(role, pending.category, observations);
        let ticket = self
            .tickets
            .iter_mut()
            .find_map(Option::take)
            .ok_or(Fault::Capacity)?;
        let key = admitted_key(&self.keys, pending.key())?;
        let admission = match key {
            Some(Installed::Transmit(key)) => core
                .admit_key(key, ticket)
                .map(TxAdmission::Protected)
                .map_err(|_| Fault::Capacity)?,
            Some(Installed::Receive(_)) => return Err(Fault::Capacity),
            None => TxAdmission::Open(ticket),
        };
        let chain = self
            .chains
            .iter_mut()
            .find_map(Option::take)
            .ok_or(Fault::Capacity)?;
        let mut buffer = Esp32s31AmpduBuffer::new(chain)
            .map_err(|_| Fault::Aggregate(HtAmpduTxError::ResetRequired))?;
        let subframes = pending.operation.execute();
        for subframe in 0..subframes.len() {
            let mut leased = subframes.lease(subframe).ok_or(Fault::Capacity)?;
            let bytes = leased.buffer.stable_dma_region().into_mut_slice();
            if leased.published {
                // Sent again: the same MPDU, its header and PN kept.
                crate::ordinary_tx::mark_retry(bytes, leased.len)
                    .map_err(|error| Fault::Core(LowerMacFault::Tx(error)))?;
            } else if let (Some(Installed::Transmit(key)), Some(pn)) = (key, leased.pn) {
                core.write_ccmp_header(key, bytes, leased.header_len, pn)
                    .map_err(|_| Fault::Capacity)?;
            }
            if buffer
                .push_backing(leased.buffer, leased.len, leased.admitted_at)
                .is_err()
            {
                return Err(Fault::Capacity);
            }
        }
        let values = AmpduValues {
            role,
            access_category: pending.category,
            rate,
            protection: pending.protection,
            power_ceiling_dbm: match pending.power {
                TxPower::Calibrated => None,
                TxPower::MaxDbm(dbm) => Some(dbm),
            },
            backoff_slots,
            min_mpdu_start_spacing: pending.min_mpdu_start_spacing,
            txop_limit_units_32_us: pending.execution.txop_limit_units_32_us,
        };
        let queue = self
            .free
            .get_mut(index)
            .and_then(Option::take)
            .ok_or(Fault::Capacity)?;
        let plan = match core.plan_ampdu(queue, admission, values, &mut buffer) {
            Ok(plan) => plan,
            // The builder accepted every subframe under this same rule.
            Err(refused) => {
                let reason = refused.reason;
                core::mem::forget((refused, buffer));
                return Err(Fault::AggregatePlan(reason));
            }
        };
        match core.publish_ampdu(device.hardware()?, buffer, plan) {
            Ok(published) => self.keep_running(index, Running::Aggregate { pending, published }),
            Err(crate::lower_mac::ampdu::AmpduStartError::Expired(expired)) => {
                // Aged at the publication edge: nothing reached the hardware.
                let msdus = expired.expired_msdus();
                let mut sink = Sink::default();
                let Some(mut view) = self.core.view().aggregates() else {
                    core::mem::forget(expired);
                    return Err(Fault::Capacity);
                };
                let buffer = match view.complete_expired(device.hardware()?, expired, &mut sink) {
                    Ok(buffer) => buffer,
                    Err(expired) => {
                        core::mem::forget(expired);
                        return Err(Fault::Capacity);
                    }
                };
                let subframes = pending.operation.execute();
                let mut next = 0;
                let mut lost = false;
                let chain = buffer.into_owner(|lease| {
                    lost |= !subframes.restore(next, lease, false);
                    next += 1;
                });
                self.return_chain(chain)?;
                let _end = self.return_admission(index, &mut sink)?;
                if lost {
                    return Err(Fault::Capacity);
                }
                self.finish_aggregate(held, pending, TxOutcome::Expired { msdus }, false);
                Ok(())
            }
            Err(crate::lower_mac::ampdu::AmpduStartError::Fault(fault)) => {
                let error = fault.error;
                core::mem::forget(fault);
                Err(Fault::Aggregate(error))
            }
        }
    }

    /// Record `running` as queue `index`'s published attempt.
    fn keep_running(
        &mut self,
        index: usize,
        running: Running<'id, Id, D, B, O>,
    ) -> Result<(), Fault> {
        match self.running.get_mut(index) {
            Some(place) => {
                *place = Some(running);
                Ok(())
            }
            None => {
                // No queue: its publication stays quarantined.
                let _quarantined = core::mem::ManuallyDrop::new(running);
                Err(Fault::Capacity)
            }
        }
    }

    /// Give an idle chain back to the free ones.
    fn return_chain(&mut self, chain: AmpduChain<B>) -> Result<(), Fault> {
        match self.chains.iter_mut().find(|place| place.is_none()) {
            Some(place) => {
                *place = Some(chain);
                Ok(())
            }
            None => {
                core::mem::forget(chain);
                Err(Fault::Capacity)
            }
        }
    }

    /// The queue and ticket an aggregate's terminal returned, from `sink`.
    fn return_admission(
        &mut self,
        index: usize,
        sink: &mut Sink<'id, Id, B>,
    ) -> Result<PublicationEnd, Fault> {
        if let Some(fault) = sink.fault() {
            return Err(fault);
        }
        if !sink.removed.is_empty() || !sink.completed.is_empty() {
            return Err(Fault::Capacity);
        }
        let (free, ticket, end) = sink.aggregates.pop().ok_or(Fault::Capacity)?;
        let Some(place) = self
            .free
            .get_mut(index)
            .filter(|_| queue(free.category()) == index)
        else {
            let _quarantined = core::mem::ManuallyDrop::new((free, ticket));
            return Err(Fault::Capacity);
        };
        *place = Some(free);
        let ticket_index = ticket.index();
        let place = self.tickets.get_mut(ticket_index).ok_or(Fault::Capacity)?;
        *place = Some(ticket);
        Ok(end)
    }

    /// Offer one TX edge to every published aggregate; `true` when an
    /// aggregate's timeout abort settled, so the MPDUs' latched timeouts
    /// are offered again. A publication is served where it lies; only its
    /// end takes it from its queue.
    pub(super) fn service_aggregates(
        &mut self,
        held: &Held<'_, 'id, Id, D, B, O>,
        device: &mut D,
        wake: WifiTxWake,
    ) -> Result<bool, Fault> {
        let mut settled = false;
        for index in 0..self.running.len() {
            let Some(Some(Running::Aggregate { pending, published })) = self.running.get_mut(index)
            else {
                continue;
            };
            let settling = published.abort_settling();
            let Some(mut view) = self.core.view().aggregates() else {
                return Err(Fault::Capacity);
            };
            let subframes = pending.operation.execute();
            let mut next = 0;
            let mut lost = false;
            let served = view.service(device.hardware()?, published, wake, |lease| {
                lost |= !subframes.restore(next, lease, true);
                next += 1;
            });
            match served {
                Ok(AmpduServed::Pending) => continue,
                Ok(AmpduServed::Ended) => {}
                Err(error) => {
                    // The failed publication leaves its queue's place
                    // retained, never dropped.
                    let _quarantined =
                        core::mem::ManuallyDrop::new(self.running.get_mut(index).map(Option::take));
                    return Err(Fault::Aggregate(error));
                }
            }
            let Some(Some(Running::Aggregate { pending, published })) =
                self.running.get_mut(index).map(Option::take)
            else {
                return Err(Fault::Capacity);
            };
            let completed = match published.into_progress() {
                Ok(AmpduProgress::Complete(completed)) => completed,
                Ok(AmpduProgress::Pending(published)) => {
                    let _quarantined = core::mem::ManuallyDrop::new((pending, published));
                    return Err(Fault::Capacity);
                }
                Err(fault) => {
                    let error = fault.error;
                    core::mem::forget((fault, pending));
                    return Err(Fault::Aggregate(error));
                }
            };
            let Some(mut view) = self.core.view().aggregates() else {
                core::mem::forget((completed, pending));
                return Err(Fault::Capacity);
            };
            settled |= settling;
            let mut sink = Sink::default();
            let (chain, _) = match view.complete(device.hardware()?, completed, &mut sink) {
                Ok(owner) => owner,
                Err(fault) => {
                    let error = fault.error;
                    core::mem::forget((fault, pending));
                    return Err(Fault::Aggregate(error));
                }
            };
            self.return_chain(chain)?;
            let end = self.return_admission(index, &mut sink)?;
            if lost || next != pending.operation.payload().len() {
                return Err(Fault::Capacity);
            }
            let outcome = outcome(end, pending.execution.rate)?;
            self.finish_aggregate(held, pending, outcome, true);
        }
        Ok(settled)
    }
}

/// The installed key `identity` names, `None` for an open attempt.
pub(super) fn admitted_key<'a, 'id, Id: SlotIdentity>(
    keys: &'a [Option<(RawKeyIdentity, Installed<'id, Id>)>],
    identity: Option<RawKeyIdentity>,
) -> Result<Option<&'a Installed<'id, Id>>, Fault> {
    let Some(identity) = identity else {
        return Ok(None);
    };
    keys.iter()
        .flatten()
        .find(|(key, _)| *key == identity)
        .map(|(_, key)| Some(key))
        .ok_or(Fault::Capacity)
}
