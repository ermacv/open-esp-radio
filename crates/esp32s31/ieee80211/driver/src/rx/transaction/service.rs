use super::*;
use crate::datapath::DatapathRxProgress;
use oer_esp32s31_ieee80211_dma::rx_storage::RxDmaStorage;
use oer_esp32s31_ieee80211_mac::{
    rx::pool::{RxDmaDeferredStageUnitOutcome, RxStageError, RxStagePool, RxStageTransactionError},
    rx::{PUBLIC_HEADER_SIZE, RxDma, RxRingError, RxRingLive},
};

/// Maximum units in the existing masked RX poll epoch.
const BUDGET_UNITS: usize = 32;

/// Execute one finite physical completion/staging transaction immediately.
///
/// No owner is moved out of the caller. The helper is inlined into the
/// adapter's existing hot-text wrapper and introduces no scheduling edge.
/// An invalid credit reserve returns [`RxStageTransactionError::InvalidCreditReserve`]
/// before any hardware access or ownership change. A valid policy is sampled
/// once and remains fixed for this transaction, including capacity refreshes.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
pub fn service<
    'storage,
    'pool,
    H,
    E,
    P,
    O,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
>(
    ring: &mut RxRingLive<'storage, COUNT>,
    storage: &'static RxDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    pool: &'pool RxStagePool<STAGE_SLOTS, STAGE_CAPACITY>,
    publisher: &E,
    admission: &P,
    counters: Counters<'_>,
    hardware: &mut H,
    mut hooks: O,
) -> Result<DatapathRxProgress, RxStageTransactionError>
where
    H: RxDma,
    E: Publisher<'pool, STAGE_CAPACITY, STAGE_SLOTS>,
    P: Admission,
    O: Hooks,
{
    // Reject an impossible policy before sampling hardware or changing any
    // descriptor, lease, publisher or counter. Capacity never chooses policy.
    let credits = admission.credit_policy().validate(STAGE_SLOTS, E::DEPTH)?;
    let reserved = credits.critical_reserved_credits();
    let hardware_buffer_full_before = if hooks.observing() {
        hardware.buffer_full_count()
    } else {
        None
    };
    if O::SAMPLE_ENTRY_REMAINING {
        hooks.entry_remaining(ring.accepted_list_remaining_from_next(hardware.next_descriptor()));
    }
    // A direct BASE publication has no reload doorbell and may consume
    // its only completion edge while the previous IRQ is still being
    // serviced. Advance its durable two-turn probe before freezing the
    // next LAST-bounded completion frontier.
    ring.observe_exhausted_republication(hardware);
    let mut recycled_descriptors = 0_usize;
    // Upper ownership returns the exact original DMA allocations.
    // Rearm only the longest ordered released prefix before freezing
    // this service's completion frontier; at most 32 such leases can
    // exist, so ring96 retains 64 descriptors in the radio ownership
    // domain. They are not necessarily armed while a delayed masked
    // epoch accumulates completions; accepted-list pressure records
    // that separate hardware condition.
    let mut released_append = None;
    for _ in 0..2 {
        released_append = storage
            .recycle_released_prefix::<BUDGET_UNITS, _>(ring, hardware)
            .map_err(RxStageTransactionError::Ring)?;
        if released_append.is_some() || !ring.completion_release_probe_pending() {
            break;
        }
        // `RX_DONE` may precede the cursor edge which releases its
        // link word. The vendor worker refreshes that cursor without
        // returning to its scheduler; retain the same two bounded
        // observations here instead of manufacturing an executor
        // round trip.
    }
    if let Some(append) = released_append {
        recycled_descriptors = append.descriptor_count;
        if let Err(error) = ring.complete_pending_reload(hardware) {
            if O::LOG_ERRORS {
                hooks.log_busy("entry-reload", error, 0, 0, 0, 0);
            }
            return Err(RxStageTransactionError::Ring(error));
        }
        ring.observe_exhausted_republication(hardware);
    }
    // Freeze the completion frontier before any descriptor is rearmed.
    // A saturated producer can therefore only create a later epoch; it
    // cannot make this service call unbounded by refilling the ring.
    let mut frozen_cursor = ring.freeze_cursor(hardware);
    let mut deferred_recycle_pending = false;
    // A discard which followed a detached valid frame could not be
    // appended in the earlier service transaction without recycling
    // that still-owned prefix. Once returned frames above have moved
    // the recycle frontier, bind each ring-owned discard to this fresh
    // LAST generation before publishing it. This mirrors the vendor's
    // one-unit discard/append transaction and remains bounded by the
    // same per-service descriptor budget.
    let mut deferred_recycled = 0_usize;
    while deferred_recycled < BUDGET_UNITS {
        let Some(descriptor_count) = storage
            .first_observed_ring_unit_descriptor_count(ring)
            .map_err(RxStageTransactionError::Ring)?
        else {
            break;
        };
        if deferred_recycled.saturating_add(descriptor_count) > BUDGET_UNITS {
            deferred_recycle_pending = true;
            break;
        }
        let append = storage
            .recycle_completed_unit_through_frozen_last(
                ring,
                hardware,
                frozen_cursor,
                descriptor_count,
            )
            .map_err(RxStageTransactionError::Ring)?;
        let Some(append) = append else {
            deferred_recycle_pending = true;
            break;
        };
        recycled_descriptors = recycled_descriptors.saturating_add(append.descriptor_count);
        deferred_recycled = deferred_recycled.saturating_add(append.descriptor_count);
        if let Err(error) = ring.complete_pending_reload(hardware) {
            if O::LOG_ERRORS {
                hooks.log_busy("deferred-discard-reload", error, 0, 0, 0, 0);
            }
            return Err(RxStageTransactionError::Ring(error));
        }
        ring.observe_exhausted_republication(hardware);
        frozen_cursor = ring.freeze_cursor(hardware);
    }
    let frontier_snapshot = storage
        .completed_unit_frontier_through_cursor(
            ring,
            frozen_cursor.last_descriptor_low(),
            frozen_cursor.next_descriptor_low(),
        )
        .map_err(RxStageTransactionError::Ring)?;
    let frontier = frontier_snapshot.unit_count;
    let pool_credits = pool.available_slots();
    let queue_credits = publisher.free_capacity();
    let mut current_pool_credits = pool_credits;
    let mut current_queue_credits = queue_credits;
    let mut minimum_pool_credits = pool_credits;
    let mut minimum_queue_credits = queue_credits;
    // The radio producer is the only owner which can consume either
    // credit domain. Upper tasks can only return credits while this
    // finite service transaction runs, so the local values are safe
    // lower bounds. Refresh only at the reserve boundary instead of
    // rescanning all 32 stage slots for every completed frame.
    let service_budget = frontier.min(BUDGET_UNITS);
    let mut admitted = 0_usize;
    let mut admitted_descriptors = 0_usize;
    let mut staged_bytes = 0_usize;
    let mut staged_units = 0_usize;
    let mut discarded_units = 0_usize;
    let mut overload_discarded = 0_usize;
    let mut overload_recycled_descriptors = 0_usize;
    let mut critical_reserve_admitted = 0_usize;
    let mut critical_admission_blocked = false;
    let mut stage_capacity_blocked = false;
    let mut remaining_descriptors = frontier_snapshot.descriptor_count;

    hooks.phase(Phase::Frontier);
    for _ in 0..service_budget {
        hooks.phase(Phase::Admission);
        if current_pool_credits <= reserved || current_queue_credits <= reserved {
            current_pool_credits = pool.available_slots();
            current_queue_credits = publisher.free_capacity();
        }
        minimum_pool_credits = minimum_pool_credits.min(current_pool_credits);
        minimum_queue_credits = minimum_queue_credits.min(current_queue_credits);
        let current_credits = current_pool_credits.min(current_queue_credits);
        let ordinary_credit_available = current_credits > reserved;
        // A free ordinary credit makes ownership transfer
        // unconditional: policy can still reject the detached unit by
        // length, but no classification result can leave it at the
        // hardware frontier. In that common case the descriptor bound
        // captured by `frontier_snapshot` is already the complete
        // frozen-cursor proof, so do not rediscover the first unit and
        // then make `take_completed_unit` validate it a second time.
        //
        // At the reserve boundary we must classify before transfer
        // because PreserveForCapacity deliberately leaves ownership in
        // DMA. Keep the explicit first-unit proof only on that slow
        // path.
        let preflight = if ordinary_credit_available {
            None
        } else {
            let unit_frontier = storage
                .first_completed_unit_frontier_through_cursor(
                    ring,
                    frozen_cursor.last_descriptor_low(),
                    frozen_cursor.next_descriptor_low(),
                )
                .map_err(RxStageTransactionError::Ring)?;
            if unit_frontier.unit_count != 1
                || unit_frontier.descriptor_count == 0
                || unit_frontier.descriptor_count > remaining_descriptors
            {
                return Err(RxStageTransactionError::Ring(RxRingError::Corrupt));
            }
            let unit_head_index = storage
                .completion_frontier_head(ring)
                .map_err(RxStageTransactionError::Ring)?
                .ok_or(RxStageTransactionError::Ring(RxRingError::Corrupt))?;
            let unit_observation = CompletedUnit {
                head_index: unit_head_index,
                descriptor_count: unit_frontier.descriptor_count,
                payload_length: storage
                    .descriptors()
                    .get(unit_head_index)
                    .map(|descriptor| {
                        oer_esp32s31_ieee80211_dma::descriptor::length(descriptor.word0()) as usize
                    })
                    .unwrap_or(0),
            };
            let maximum_payload_length = admission
                .maximum_payload_length(unit_observation, STAGE_CAPACITY)
                .min(STAGE_CAPACITY);
            Some((
                unit_frontier.descriptor_count,
                unit_observation,
                maximum_payload_length,
            ))
        };
        let length_discard = preflight.is_some_and(|(_, unit, maximum)| {
            unit.payload_length == 0 || unit.payload_length > maximum
        });
        let (unavailable, unavailable_preview) =
            if !length_discard && let Some((_, unit_observation, _)) = preflight {
                // Classification only affects overload admission. An
                // ordinary unit with a free bulk credit is staged
                // regardless of its frame-control or VIF route, so do
                // not repeat a frozen-cursor proof and header copy on
                // every successful hot-path transaction.
                let mut header = [0_u8; 24];
                let preview = if storage
                    .copy_first_completed_unit_bytes_through_cursor(
                        ring,
                        frozen_cursor.last_descriptor_low(),
                        frozen_cursor.next_descriptor_low(),
                        PUBLIC_HEADER_SIZE,
                        &mut header,
                    )
                    .map_err(RxStageTransactionError::Ring)?
                {
                    publisher.preview(unit_observation, header)
                } else {
                    publisher.unclassified_preview(unit_observation)
                };
                (
                    Some(admission.unavailable_disposition(preview)),
                    Some(preview),
                )
            } else {
                (None, None)
            };
        let stage_with_reserved_credit =
            matches!(unavailable, Some(Unavailable::PreserveForCriticalAdmission))
                && current_credits != 0;
        if matches!(unavailable, Some(Unavailable::PreserveForCapacity)) {
            stage_capacity_blocked = true;
            admission.observe(Observation::BulkAdmissionBlocked(
                unavailable_preview.expect("unavailable bulk unit has a preview"),
            ));
            break;
        }
        if matches!(unavailable, Some(Unavailable::PreserveForCriticalAdmission))
            && !stage_with_reserved_credit
        {
            critical_admission_blocked = true;
            admission.observe(Observation::CriticalAdmissionBlocked(
                unavailable_preview.expect("unavailable critical unit has a preview"),
            ));
            break;
        }
        let overload_drop = matches!(unavailable, Some(Unavailable::DiscardAndRecycle));
        if overload_drop && staged_units != 0 {
            // Give the fused turn's post-DMA consumer a chance to return
            // the owners just published by this call. Filling our own
            // staging window does not establish sustained backpressure.
            // The untouched tail remains DMA-owned. If the next turn
            // starts blocked, normal discard/recycle still reaches critical
            // traffic instead of creating a receive/transmit progress cycle.
            stage_capacity_blocked = true;
            admission.observe(Observation::BulkAdmissionBlocked(
                unavailable_preview.expect("overload admission has a preview"),
            ));
            break;
        }

        hooks.phase(Phase::StageTake);
        let descriptor_limit = preflight
            .map(|(descriptor_count, _, _)| descriptor_count)
            .unwrap_or(remaining_descriptors);
        let recycle_start_before_take = ring.recycle_start();
        let unit = storage
            .take_completed_unit(ring, descriptor_limit)
            .map_err(RxStageTransactionError::Ring)?
            .ok_or(RxStageTransactionError::Ring(RxRingError::Corrupt))?;
        let unit_descriptor_count = unit.descriptor_count();
        let unit_observation = CompletedUnit {
            head_index: unit.head_index(),
            descriptor_count: unit_descriptor_count,
            payload_length: unit.total_length(),
        };
        let maximum_payload_length = preflight
            .map(|(_, _, maximum_payload_length)| maximum_payload_length)
            .unwrap_or_else(|| {
                admission
                    .maximum_payload_length(unit_observation, STAGE_CAPACITY)
                    .min(STAGE_CAPACITY)
            });
        let recoverable_discard = if unit_descriptor_count != 1 {
            Some(RxStageError::Chained)
        } else if unit_observation.payload_length == 0 {
            Some(RxStageError::Empty)
        } else if unit_observation.payload_length > maximum_payload_length {
            Some(RxStageError::TooLong)
        } else {
            None
        };
        remaining_descriptors = remaining_descriptors
            .checked_sub(unit_descriptor_count)
            .ok_or(RxStageTransactionError::Ring(RxRingError::Corrupt))?;
        admitted = admitted.saturating_add(1);
        admitted_descriptors = admitted_descriptors.saturating_add(unit_descriptor_count);

        let (frame, descriptor_replenished) = if overload_drop {
            unit.retain_for_deferred_recycle();
            let descriptor_replenished =
                unit_observation.head_index == recycle_start_before_take.get();
            deferred_recycle_pending |= !descriptor_replenished;
            discarded_units = discarded_units.saturating_add(1);
            overload_discarded = overload_discarded.saturating_add(1);
            if descriptor_replenished {
                overload_recycled_descriptors =
                    overload_recycled_descriptors.saturating_add(unit_descriptor_count);
            }
            if hooks.observing() {
                hooks.stage_discarded(Discard::OverloadBulk);
            }
            admission.observe(Observation::OverloadDiscardedAndRecycled(
                unavailable_preview.expect("overload discard has a preview"),
            ));
            (None, descriptor_replenished)
        } else if let Some(error) = recoverable_discard {
            unit.retain_for_deferred_recycle();
            let descriptor_replenished =
                unit_observation.head_index == recycle_start_before_take.get();
            deferred_recycle_pending |= !descriptor_replenished;
            discarded_units = discarded_units.saturating_add(1);
            if hooks.observing() {
                let discard = match error {
                    RxStageError::Empty => Discard::Empty,
                    RxStageError::TooLong => Discard::TooLong,
                    RxStageError::Chained => Discard::Chained,
                    RxStageError::Exhausted => unreachable!("capacity is not malformed"),
                };
                hooks.stage_discarded(discard);
            }
            admission.observe(Observation::DiscardRetained {
                unit: unit_observation,
                reason: match error {
                    RxStageError::Empty => Discard::Empty,
                    RxStageError::TooLong => Discard::TooLong,
                    RxStageError::Chained => Discard::Chained,
                    RxStageError::Exhausted => {
                        unreachable!("capacity is not malformed")
                    }
                },
            });
            (None, descriptor_replenished)
        } else {
            if stage_with_reserved_credit {
                critical_reserve_admitted = critical_reserve_admitted.saturating_add(1);
                admission.observe(Observation::CriticalReserveAdmitted(
                    unavailable_preview.expect("critical reserve admission has a preview"),
                ));
            }
            hooks.phase(Phase::StagePool);
            let stage = pool.stage_dma_unit_deferred_bounded(unit, maximum_payload_length);
            if O::LOG_ERRORS
                && let Err(error) = &stage
            {
                hooks.log_busy(
                    "detach",
                    match error {
                        RxStageTransactionError::Ring(error) => *error,
                        RxStageTransactionError::Stage(_)
                        | RxStageTransactionError::InvalidCreditReserve { .. } => {
                            RxRingError::Corrupt
                        }
                    },
                    unit_observation.head_index,
                    unit_descriptor_count,
                    storage.detached_buffer_count(),
                    storage.released_buffer_count(),
                );
            }
            match stage? {
                RxDmaDeferredStageUnitOutcome::Staged(frame) => (Some(frame), false),
                RxDmaDeferredStageUnitOutcome::Discarded(_) => {
                    return Err(RxStageTransactionError::Ring(RxRingError::Corrupt));
                }
            }
        };

        if descriptor_replenished {
            hooks.phase(Phase::Recycle);
            let append = storage
                .recycle_completed_unit_through_frozen_last(
                    ring,
                    hardware,
                    frozen_cursor,
                    unit_descriptor_count,
                )
                .map_err(RxStageTransactionError::Ring)?;
            let Some(append) = append else {
                if O::LOG_ERRORS {
                    hooks.log_recycle_refused(
                        unit_descriptor_count,
                        ring.recycle_start().get(),
                        ring.accepted_tail().get(),
                        ring.observed_mask().count_ones(),
                        frozen_cursor.last_descriptor_low(),
                        frozen_cursor.next_descriptor_low(),
                        ring.reload_pending(),
                        hardware.reload_pending(),
                    );
                }
                return Err(RxStageTransactionError::Ring(RxRingError::Busy));
            };
            if append.descriptor_count != unit_descriptor_count {
                return Err(RxStageTransactionError::Ring(RxRingError::Corrupt));
            }
            recycled_descriptors = recycled_descriptors.saturating_add(unit_descriptor_count);
            // The vendor append path does not yield between its reload
            // doorbell and conditional BASE-repair suffix. Preserve
            // that transaction boundary so a later completion cannot
            // replace the cursor observation used to settle this
            // append.
            let reload_started = hooks.now_micros();
            hooks.phase(Phase::Reload);
            if let Err(error) = ring.complete_pending_reload(hardware) {
                if O::LOG_ERRORS {
                    hooks.log_busy("discard-reload", error, 0, 0, 0, 0);
                }
                return Err(RxStageTransactionError::Ring(error));
            }
            hooks.reload_completed(reload_started);
        }
        hooks.phase(Phase::Publish);
        if let Some(frame) = frame {
            staged_units = staged_units.saturating_add(1);
            staged_bytes = staged_bytes.saturating_add(frame.length());
            // Rejection returns the same lease; preserve its immediate drop
            // at this error mapping before returning the existing Corrupt fault.
            publisher
                .try_send(frame)
                .map_err(|_| RxStageTransactionError::Ring(RxRingError::Corrupt))?;
            admission.observe(Observation::Staged(unit_observation));
            current_pool_credits = current_pool_credits.saturating_sub(1);
            current_queue_credits = current_queue_credits.saturating_sub(1);
        }
        minimum_pool_credits = minimum_pool_credits.min(current_pool_credits);
        minimum_queue_credits = minimum_queue_credits.min(current_queue_credits);

        // Immediate discard append changes the descriptor generation.
        // Valid packets remain in the original frozen generation
        // until the complete detached prefix is refilled below.
        if descriptor_replenished {
            frozen_cursor = ring.freeze_cursor(hardware);
        }
        hooks.phase(Phase::Frontier);
    }

    hooks.phase(Phase::Tail);
    *counters.descriptors = counters
        .descriptors
        .saturating_add(u64::try_from(admitted_descriptors).unwrap_or(u64::MAX));
    *counters.units = counters
        .units
        .saturating_add(u64::try_from(admitted).unwrap_or(u64::MAX));
    *counters.bytes = counters
        .bytes
        .saturating_add(u64::try_from(staged_bytes).unwrap_or(u64::MAX));

    // Every append changes descriptor generation. Retain one
    // cooperative post-append confirmation before the moderated IRQ
    // source is unmasked; a completion can consume the republished
    // unit while the current RX-success edge is still owned here.
    let completed_frontier_remaining = storage
        .completed_unit_frontier_through_cursor(
            ring,
            frozen_cursor.last_descriptor_low(),
            frozen_cursor.next_descriptor_low(),
        )
        .map_err(RxStageTransactionError::Ring)?
        .unit_count
        != 0;
    let recycled_probe_pending = hooks.recycled_probe_pending(recycled_descriptors);
    // The terminal cursor can precede the final descriptor writeback,
    // and that writeback has no guaranteed fresh RX-success edge.
    // Retain a cooperative pass until the exhausted finite list has
    // been staged and republished.
    let exhausted_writeback_pending = ring.exhausted_terminal_writeback_pending(hardware);
    let exhausted_republication_probe_pending = ring.exhausted_republication_probe_pending();

    hooks.probe_reasons(
        recycled_descriptors != 0,
        completed_frontier_remaining,
        exhausted_writeback_pending,
        exhausted_republication_probe_pending,
    );

    let budget_exhausted = frontier > BUDGET_UNITS;

    // Keep the two samples on opposite sides of the complete bounded
    // transaction. Diagnostics can then distinguish saturation while
    // the owner was elsewhere from saturation caused during staging
    // and descriptor republication. With only the entry sample the
    // counter increment was incorrectly paired with the following
    // service context.
    let hardware_buffer_full_after = if hooks.observing() {
        hardware.buffer_full_count()
    } else {
        None
    };

    if hooks.observing() {
        hooks.service_completed(ServiceObservation {
            frontier,
            pool_credits,
            queue_credits,
            completed_units: admitted,
            completed_descriptors: admitted_descriptors,
            admitted,
            staged_units,
            staged_bytes,
            discarded_units,
            recycled_descriptors,
            overload_discarded,
            overload_recycled_descriptors,
            critical_reserve_admitted,
            stage_capacity_blocked,
            critical_admission_blocked,
            minimum_pool_credits,
            minimum_queue_credits,
            micros: hooks.elapsed_service_micros(),
            hardware_buffer_full_before,
            hardware_buffer_full_after,
        });
    }

    hooks.finish(admitted);

    Ok(if stage_capacity_blocked || critical_admission_blocked {
        DatapathRxProgress::StageCapacityBlocked
    } else if budget_exhausted {
        DatapathRxProgress::BudgetExhausted
    } else if completed_frontier_remaining
        || exhausted_writeback_pending
        || exhausted_republication_probe_pending
    {
        DatapathRxProgress::ProbePending
    } else if recycled_probe_pending || deferred_recycle_pending {
        DatapathRxProgress::RecycledAppendPending
    } else if overload_discarded != 0 {
        DatapathRxProgress::UpperLayerBlockedButDroppable
    } else {
        DatapathRxProgress::Drained
    })
}
