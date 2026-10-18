//! AP-owned network TX transaction.
//!
//! DATAPATH schedules this owner but does not know peer admission, encoding,
//! aggregate publication, retry, or completion policy.

use core::marker::PhantomData;
use oer_time::Clock as _;

use super::*;

use crate::diagnostics::core0_rx_performance::{Core0Tally, TX_PHASE_TELEMETRY};

mod aggregate;
mod airtime;

pub use airtime::{
    AccessPointAirtimeConfiguration, AccessPointAirtimeCost, AccessPointAirtimeError,
    AccessPointAirtimePeer, AccessPointAirtimeSelection, AccessPointAirtimeStorage,
    AccessPointBlockAckTiming, AirtimeAction, AirtimeObservation,
};
fn radio_stamp(
    timer: &impl crate::mac_clock::MacClockReader,
) -> Result<oer_ieee80211_lower_mac::Ieee80211Stamp, AccessPointDatapathError> {
    timer
        .sample()
        .map(|sample| sample.stamp())
        .ok_or(AccessPointDatapathError::Aggregate(
            ApAmpduError::RadioClockUnavailable,
        ))
}

mod completion;
mod power_save;
mod queue;
mod service_phase;
mod storage;

pub use storage::AccessPointTxStorage;

#[cfg(test)]
use queue::{AP_ACTIVE_FRAME_CAPACITY, ApFrameLeaseArena};

use aggregate::aggregate_adapter_available;

use queue::{
    ApActiveFrameQueues, ApGroupFrameQueue, ApPowerSaveFrameQueue, ApTxFlowKey, BufferedGroup,
    BufferedUnicast,
};

use service_phase::{AggregateServiceAction, AggregateServicePhase};

use storage::FrameArenaLease;

/// Software-owner budget supported by AP retention, including active frames,
/// sleeping peers, DTIM groups and release tickets awaiting completion.
///
/// The owned composition uses this same limit for endpoint admission. Since
/// dequeue and power-save rollback retain admission, every admitted owner can
/// move into radio storage without acquiring a second capacity credit. This
/// is CPU-only storage policy, independent of the physical DMA working set.
pub const AP_SOFTWARE_TX_CAPACITY: usize = 128;

struct BufferedUnicastRelease {
    buffered: BufferedUnicast,
    release: ApBufferedUnicastRelease,
    cause: BufferedReleaseCause,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum BufferedReleaseCause {
    PsPoll,
    Awake,
}

impl BufferedUnicastRelease {
    fn can_publish(&self, engine: &ApEngine<'_>) -> bool {
        engine
            .association_status(self.release.identity())
            .is_some_and(|status| {
                self.cause == BufferedReleaseCause::PsPoll
                    || status.power_state == ApPeerPowerState::Active
            })
    }
}

enum ApTxSelection<N> {
    Frame(ApTxFlowKey, N),
    Buffered(BufferedUnicastRelease),
}

struct BufferedGroupRelease {
    buffered: BufferedGroup,
    release: ApBufferedGroupRelease,
}

struct PreparedStandby {
    admission: ApAggregateAdmission,
    policy: HtAmpduTxRolePolicy,
    admitted: usize,
    mismatch_claims: Core0Tally,
    #[cfg(any(feature = "diagnostics", test))]
    preparation_micros: u64,
}

pub struct AccessPointNetworkTx<'observer, B, N = B> {
    dma_backing: PhantomData<B>,
    #[cfg(any(feature = "diagnostics", test))]
    observer: Option<&'observer dyn AggregateTxObserver>,
    #[cfg(not(any(feature = "diagnostics", test)))]
    observer_lifetime: PhantomData<&'observer ()>,
    airtime: Option<airtime::Accounting<'observer>>,
    aggregate_phase: Option<AggregateServicePhase>,
    /// Ordinary transmissions made on behalf of the current aggregate
    /// exchange (individual retries, BlockAckReqs); the exchange is charged
    /// for them together with the aggregate's own work.
    exchange_ordinary_work: oer_ieee80211_softmac::MacTxWork,
    #[cfg(any(feature = "diagnostics", test))]
    exchange_started_micros: Option<u64>,
    #[cfg(any(feature = "diagnostics", test))]
    terminal_acknowledged: Option<u8>,
    frame_arena: FrameArenaLease<'observer, N>,
    active_frames: ApActiveFrameQueues,
    last_destination: Option<[u8; 6]>,
    prepared_first: Option<N>,
    prepared_first_key: Option<ApTxFlowKey>,
    prepared_second: Option<N>,
    prepared_second_key: Option<ApTxFlowKey>,
    prepared_standby: Option<PreparedStandby>,
    buffered_unicast: ApPowerSaveFrameQueue,
    buffered_group: ApGroupFrameQueue,
    prepared_buffered_release: Option<BufferedUnicastRelease>,
    active_buffered_release: Option<BufferedUnicastRelease>,
    /// Event-refreshed readiness hint, never a release reservation. Selection
    /// revalidates it against current association/PM state and the cursor.
    awake_buffered_peer: Option<ApAssociationIdentity>,
    prepared_group_release: Option<BufferedGroupRelease>,
    active_group_release: Option<BufferedGroupRelease>,
    /// Remaining prefix authorized by one successful DTIM beacon. Frames
    /// retained after that beacon can never join this release window.
    dtim_group_release_remaining: u16,
    last_started_frames: usize,
}

impl<'observer, B, N> AccessPointNetworkTx<'observer, B, N>
where
    B: StableDmaBacking,
{
    pub fn new(
        storage: &'observer mut AccessPointTxStorage<N>,
        #[cfg(any(feature = "diagnostics", test))] observer: Option<
            &'observer dyn AggregateTxObserver,
        >,
    ) -> Self {
        Self {
            dma_backing: PhantomData,
            #[cfg(any(feature = "diagnostics", test))]
            observer,
            #[cfg(not(any(feature = "diagnostics", test)))]
            observer_lifetime: PhantomData,
            airtime: None,
            aggregate_phase: None,
            exchange_ordinary_work: oer_ieee80211_softmac::MacTxWork::default(),
            #[cfg(any(feature = "diagnostics", test))]
            exchange_started_micros: None,
            #[cfg(any(feature = "diagnostics", test))]
            terminal_acknowledged: None,
            frame_arena: storage.borrow(),
            active_frames: ApActiveFrameQueues::new(),
            last_destination: None,
            prepared_first: None,
            prepared_first_key: None,
            prepared_second: None,
            prepared_second_key: None,
            prepared_standby: None,
            buffered_unicast: ApPowerSaveFrameQueue::new(),
            buffered_group: ApGroupFrameQueue::new(),
            prepared_buffered_release: None,
            active_buffered_release: None,
            awake_buffered_peer: None,
            prepared_group_release: None,
            active_group_release: None,
            dtim_group_release_remaining: 0,
            last_started_frames: 1,
        }
    }

    /// Attach accounting to this AP epoch without changing destination RR or
    /// limiting aggregate geometry. The caller explicitly supplies the cost
    /// model. Unknown terminal work returns an error and keeps its reservation.
    /// Storage outlives all active/standby tokens; use fresh storage for a fresh
    /// accounting domain. No hardware overhead model is selected implicitly.
    pub fn new_with_airtime_accounting(
        tx_storage: &'observer mut AccessPointTxStorage<N>,
        #[cfg(any(feature = "diagnostics", test))] observer: Option<
            &'observer dyn AggregateTxObserver,
        >,
        airtime_storage: &'observer mut AccessPointAirtimeStorage,
        minimum_exchange_micros: core::num::NonZeroU32,
        cost: AccessPointAirtimeCost,
    ) -> Self {
        let mut tx = Self::new(
            tx_storage,
            #[cfg(any(feature = "diagnostics", test))]
            observer,
        );
        tx.airtime = Some(airtime::Accounting::new(
            airtime_storage,
            minimum_exchange_micros,
            cost,
        ));
        tx
    }

    pub fn airtime_balance_micros(&self, peer: AccessPointAirtimePeer) -> Option<i64> {
        self.airtime
            .as_ref()
            .and_then(|accounting| accounting.balance(peer))
    }

    /// Add prospective HT A-MPDU admission to the explicit accounting model.
    /// Limits each initial publication, including standby refill, using its
    /// own reservation and the supplied compressed-BlockAck PHY assumption.
    /// Selection stays RR; ordinary/A-MSDU frames and retries are charged at
    /// completion but are not capped by this HT-only limit.
    pub fn new_with_airtime_admission(
        tx_storage: &'observer mut AccessPointTxStorage<N>,
        #[cfg(any(feature = "diagnostics", test))] observer: Option<
            &'observer dyn AggregateTxObserver,
        >,
        airtime_storage: &'observer mut AccessPointAirtimeStorage,
        minimum_exchange_micros: core::num::NonZeroU32,
        cost: AccessPointAirtimeCost,
        block_ack_timing: AccessPointBlockAckTiming,
    ) -> Self {
        let mut tx = Self::new(
            tx_storage,
            #[cfg(any(feature = "diagnostics", test))]
            observer,
        );
        tx.airtime = Some(
            airtime::Accounting::new(airtime_storage, minimum_exchange_micros, cost)
                .with_aggregate_admission(block_ack_timing),
        );
        tx
    }

    pub fn unresolved_airtime_work(
        &self,
    ) -> Option<(AccessPointAirtimePeer, oer_ieee80211_softmac::MacTxWork)> {
        self.airtime
            .as_ref()
            .and_then(airtime::Accounting::unresolved_work)
    }

    /// Select eligible destinations by modelled airtime deficit and cap initial
    /// HT aggregates using the same reservation. Requires destination-addressable
    /// queues; FIFO sources return `DestinationQueuesRequired` before dequeue.
    /// The caller supplies the quantum, minimum grant and terminal cost model.
    /// This does not bound ordinary/A-MSDU/retry work or establish measured fairness.
    pub fn new_with_airtime_scheduling(
        tx_storage: &'observer mut AccessPointTxStorage<N>,
        #[cfg(any(feature = "diagnostics", test))] observer: Option<
            &'observer dyn AggregateTxObserver,
        >,
        airtime_storage: &'observer mut AccessPointAirtimeStorage,
        minimum_exchange_micros: core::num::NonZeroU32,
        cost: AccessPointAirtimeCost,
        block_ack_timing: AccessPointBlockAckTiming,
    ) -> Self {
        let mut tx = Self::new(
            tx_storage,
            #[cfg(any(feature = "diagnostics", test))]
            observer,
        );
        tx.airtime = Some(
            airtime::Accounting::new(airtime_storage, minimum_exchange_micros, cost)
                .with_aggregate_admission(block_ack_timing)
                .with_peer_selection(),
        );
        tx
    }

    pub(super) const fn aggregate_pending(&self) -> bool {
        self.aggregate_phase.is_some()
    }

    /// Return the reusable software storage after the role has detached all
    /// physical TX. Remaining queued software owners are released here.
    pub fn into_storage(self) -> &'observer mut AccessPointTxStorage<N> {
        assert!(
            !self.aggregate_pending(),
            "AP aggregate must finish before storage return"
        );
        self.frame_arena.release()
    }

    pub(super) fn has_prepared(&self) -> bool {
        self.active_frames.len() != 0
            || self.prepared_first.is_some()
            || self.prepared_second.is_some()
            || self.prepared_standby.is_some()
            || self.prepared_buffered_release.is_some()
            || self.prepared_group_release.is_some()
            || self.awake_buffered_peer.is_some()
    }

    pub(super) fn prepared_start_ready(&self) -> bool {
        self.has_prepared()
    }

    pub(super) fn prepared_destination(&self) -> Option<[u8; 6]> {
        self.prepared_standby
            .as_ref()
            .map(|batch| batch.admission.peer())
            .or_else(|| self.prepared_first_key.map(|key| key.destination))
    }

    pub(super) fn prepared_frame_count(&self) -> usize {
        if self.prepared_group_release.is_some() || self.prepared_buffered_release.is_some() {
            return 1;
        }
        if let Some(batch) = self.prepared_standby.as_ref() {
            return batch.admitted;
        }
        if let Some(first) = self.prepared_first.as_ref() {
            let _ = first;
            return 1
                + self.active_frames.len_for(
                    self.prepared_first_key
                        .expect("prepared AP frame retains its flow key"),
                )
                + usize::from(self.prepared_second.is_some());
        }
        self.active_frames
            .scheduled_len()
            .max(usize::from(self.awake_buffered_peer.is_some()))
    }

    pub(super) const fn last_started_frame_count(&self) -> usize {
        self.last_started_frames
    }

    /// Publish the terminal aggregate observation at the outer role-service
    /// boundary, after role diagnostics have consumed the completed state.
    /// This keeps completion-to-publication focused on DATAPATH scheduling
    /// instead of charging unrelated observer bookkeeping to the scheduler.
    #[cfg(any(feature = "diagnostics", test))]
    pub(super) fn observe_service_boundary(&mut self) {
        let Some(acknowledged) = self.terminal_acknowledged.take() else {
            return;
        };
        if let Some(observer) = self.observer {
            observer.observe(AggregateTxObservation::Completed {
                acknowledged,
                individual_retry: false,
            });
        }
    }

    #[cfg(any(feature = "diagnostics", test))]
    pub(super) fn mark_prepared_scheduler_phase(
        &mut self,
        phase: PreparedTxSchedulerPhase,
        at_micros: u64,
    ) {
        // Every boundary is observed, including those without a prepared
        // successor: the observer classifies publications that did not enter
        // through the prepared path instead of silently losing them.
        if let Some(observer) = self.observer {
            observer.observe(AggregateTxObservation::PreparedSchedulerPhase { phase, at_micros });
        }
    }

    pub(super) fn can_prepare<const SLOTS: usize, const BUFFER_SIZE: usize>(
        &self,
        aggregate: &AccessPointAmpdu<'_, B, SLOTS, BUFFER_SIZE>,
        ordinary_publication_pending: bool,
    ) -> bool {
        // A retained peer-boundary frame may coexist with an ordinary MPDU
        // publication, but it cannot authorize claiming the next frame yet.
        // Encoding that pair borrows the ordinary descriptor policy adapter;
        // the adapter remains owned by the in-flight MPDU until its terminal
        // service edge. Aggregate-active preparation is still allowed because
        // an A-MPDU does not set the ordinary MAC publication owner.
        if !aggregate_adapter_available(ordinary_publication_pending) {
            return false;
        }
        if !aggregate.has_standby() {
            return false;
        }
        if self.prepared_buffered_release.is_some() {
            return false;
        }
        if self.prepared_group_release.is_some()
            || self.active_group_release.is_some()
            || self.dtim_group_release_remaining != 0
        {
            return false;
        }
        match self.prepared_standby.as_ref() {
            Some(batch) => {
                self.prepared_first.is_none()
                    && batch.admitted < usize::from(batch.policy.frame_limit())
            }
            None => {
                (self.aggregate_phase.is_some()
                    || self.active_frames.len() != 0
                    || self.prepared_first.is_some())
                    && self.prepared_second.is_none()
            }
        }
    }
}

impl<'observer, B, N> AccessPointNetworkTx<'observer, B, N>
where
    B: MaterializedTxFrame,
    N: SoftwareTxFrame,
{
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn start<
        P,
        E,
        T,
        H,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
        const SLOTS: usize,
        const BUFFER_SIZE: usize,
    >(
        &mut self,
        aggregate: &mut AccessPointAmpdu<'_, B, SLOTS, BUFFER_SIZE>,
        control: &mut AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        hardware: &mut H,
        frame: N,
        network: &impl SelectedBurstMaterializer<SoftwareFrame = N, PhysicalFrame = B>,
    ) -> Result<WifiTxProgress, AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: oer_time::Timer + crate::mac_clock::MacClockReader,
        H: TxHardware
            + ApRuntimeHardware
            + RxBlockAckHardware
            + oer_esp32s31_ieee80211_mac::tx::ampdu::HtAmpduHardware,
    {
        if let Some(accounting) = self.airtime.as_ref() {
            accounting.require_active_idle()?;
        }
        let result = (|| {
            self.last_started_frames = 1;
            let _ = self.stage_dtim_group_release(control)?;
            self.retain_active_frame(control.mac.engine_mut(), frame)?;
            if self.prepared_group_release.is_some() {
                return self.start_prepared_group_release(control, hardware);
            }
            let _ = self.refresh_power_save_demand(control)?;
            if self.prepared_buffered_release.is_some() {
                return self.start_prepared_buffered_release(control, hardware);
            }
            let Some(selected) =
                self.take_scheduled_active_or_network(control.mac.engine_mut(), network)?
            else {
                return Ok(WifiTxProgress::Complete);
            };
            let (flow_key, frame) = match selected {
                ApTxSelection::Frame(key, frame) => (key, frame),
                ApTxSelection::Buffered(release) => {
                    self.prepared_buffered_release = Some(release);
                    return self.start_prepared_buffered_release(control, hardware);
                }
            };
            if let Some(accounting) = self.airtime.as_mut() {
                accounting.reserve_active(control.mac.engine(), flow_key)?;
            }
            let admission = control.mac.aggregate_admission(frame.as_slice());
            let mut retained_aggregate_second = None;

            // Open APs have no BlockAck owner, so they use bounded ordinary
            // A-MSDUs whenever an ordered partner is available. For WPA2+BA keep
            // saturated bursts on A-MPDU; coalesce the exact two-frame tail only
            // when the negotiated agreement echoed A-MSDU support.
            let same_flow_ready = self.active_frames.len_for(flow_key);
            let producer_ready = network.destination_queues().map_or_else(
                || network.queue_len(),
                |queues| queues.pending_for(flow_key.destination),
            );
            if same_flow_ready.saturating_add(producer_ready) != 0
                && (admission.is_none()
                    || (same_flow_ready.saturating_add(producer_ready) == 1
                        && admission.is_some_and(ApAggregateAdmission::amsdu)))
                && let Some(second) = self.take_matching_active_or_network(
                    control.mac.engine_mut(),
                    flow_key,
                    network,
                )?
            {
                match control.start_network_amsdu_pair(
                    hardware,
                    frame.as_slice(),
                    second.as_slice(),
                ) {
                    Ok(Some(progress)) => {
                        self.last_started_frames = 2;
                        return self.ordinary_airtime_result(Ok(progress));
                    }
                    Ok(None) => {
                        if admission.is_some() {
                            // The pair may be too large for the ordinary AP
                            // scratch while both individual MPDUs still fit the
                            // retained A-MPDU arena. Preserve the already claimed
                            // second lease for that exact fallback.
                            retained_aggregate_second = Some(second);
                        } else {
                            self.restore_active_frame_front(flow_key, second);
                        }
                    }
                    Err(error) => {
                        self.restore_active_pair_front(flow_key, frame, second);
                        return Err(AccessPointDatapathError::Control(error));
                    }
                }
            }

            if let Some(admission) = admission
                && (retained_aggregate_second.is_some()
                    || self.active_frames.len_for(flow_key) != 0
                    || network.queue_len() != 0)
            {
                let second = if let Some(second) = retained_aggregate_second.take() {
                    second
                } else {
                    let Some(second) = self.take_matching_active_or_network(
                        control.mac.engine_mut(),
                        flow_key,
                        network,
                    )?
                    else {
                        return self.ordinary_airtime_result(
                            control.start_network_tx(hardware, frame.as_slice()),
                        );
                    };
                    second
                };
                #[cfg(any(feature = "diagnostics", test))]
                let preparation_started = self.observer.map(AggregateTxObserver::now_micros);
                debug_assert!(admission.accepts_ethernet(second.as_slice()));

                let mut length_budget = aggregate
                    .active_mut()
                    .length_budget(admission.rate())
                    .map_err(AccessPointDatapathError::Aggregate)?;
                if let Some(accounting) = self.airtime.as_ref() {
                    accounting.cap_active_aggregate(admission.rate(), &mut length_budget)?;
                }
                if !length_budget
                    .admit_ethernet(frame.as_slice())
                    .map_err(AccessPointDatapathError::Aggregate)?
                    || !length_budget
                        .admit_ethernet(second.as_slice())
                        .map_err(AccessPointDatapathError::Aggregate)?
                {
                    self.restore_active_frame_front(flow_key, second);
                    return self.ordinary_airtime_result(
                        control.start_network_tx(hardware, frame.as_slice()),
                    );
                }
                let peer = admission.peer();
                let (engine, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
                    AccessPointDatapathError::Control(AccessPointControlError::Mac(error))
                })?;
                let (mut frame, mut second) = match network.try_materialize_pair(frame, second) {
                    Ok(frames) => frames,
                    Err((frame, second)) => {
                        self.restore_active_pair_front(flow_key, frame, second);
                        return Ok(WifiTxProgress::Complete);
                    }
                };
                let first_offset = frame.ethernet_offset();
                let first_length = frame.ethernet_length();
                let first_encoded = engine
                    .encode_aggregate_ethernet_in_place(
                        admission.binding(),
                        frame.storage_mut(),
                        first_offset,
                        first_length,
                    )
                    .map_err(|error| {
                        AccessPointDatapathError::Control(AccessPointControlError::from(error))
                    })?;
                let policy = admission
                    .bind_policy(first_encoded.hardware_key_selector, SLOTS)
                    .map_err(ApAmpduError::from)
                    .map_err(AccessPointDatapathError::Aggregate)?;
                let active = aggregate.active_mut();
                active
                    .begin(
                        peer,
                        policy.rate(),
                        first_encoded.sequence_number,
                        policy.role().hardware_key_selector,
                        admission.agreement(),
                    )
                    .map_err(AccessPointDatapathError::Aggregate)?;
                active
                    .push(peer, frame, first_encoded)
                    .map_err(AccessPointDatapathError::Aggregate)?;

                let second_offset = second.ethernet_offset();
                let second_length = second.ethernet_length();
                let second_encoded = engine
                    .encode_aggregate_ethernet_in_place(
                        admission.binding(),
                        second.storage_mut(),
                        second_offset,
                        second_length,
                    )
                    .map_err(|error| {
                        AccessPointDatapathError::Control(AccessPointControlError::from(error))
                    })?;
                active
                    .push(peer, second, second_encoded)
                    .map_err(AccessPointDatapathError::Aggregate)?;

                let target = usize::from(policy.frame_limit());
                let mut admitted = 2_usize;
                let mut capacity_limited = false;
                while admitted < target {
                    let Some(next) =
                        self.take_matching_active_or_network(engine, flow_key, network)?
                    else {
                        break;
                    };
                    debug_assert!(admission.accepts_ethernet(next.as_slice()));
                    if !length_budget
                        .admit_ethernet(next.as_slice())
                        .map_err(AccessPointDatapathError::Aggregate)?
                    {
                        self.restore_active_frame_front(flow_key, next);
                        capacity_limited = true;
                        break;
                    }
                    let mut next = match network.try_materialize(next) {
                        Ok(next) => next,
                        Err(next) => {
                            self.restore_active_frame_front(flow_key, next);
                            break;
                        }
                    };
                    let offset = next.ethernet_offset();
                    let length = next.ethernet_length();
                    let encoded = engine
                        .encode_aggregate_ethernet_in_place(
                            admission.binding(),
                            next.storage_mut(),
                            offset,
                            length,
                        )
                        .map_err(|error| {
                            AccessPointDatapathError::Control(AccessPointControlError::from(error))
                        })?;
                    active
                        .push(peer, next, encoded)
                        .map_err(AccessPointDatapathError::Aggregate)?;
                    admitted += 1;
                }
                let _ = capacity_limited;
                #[cfg(any(feature = "diagnostics", test))]
                let publication_started = self.observer.map(AggregateTxObserver::now_micros);
                let stamp = radio_stamp(ordinary.timer())?;
                active
                    .publish(ordinary, hardware, stamp)
                    .map_err(AccessPointDatapathError::Aggregate)?;
                if let Some(accounting) = self.airtime.as_mut() {
                    accounting.publish_active();
                }
                #[cfg(any(feature = "diagnostics", test))]
                if let Some(observer) = self.observer {
                    let finished = observer.now_micros();
                    let started = publication_started.unwrap_or(finished);
                    observe_aggregate_rate(observer, policy.rate());
                    observer.observe(AggregateTxObservation::Prepared {
                        subframes: u8::try_from(admitted).unwrap_or(u8::MAX),
                        stop: if admitted == target {
                            AggregateBuildStop::FrameLimit
                        } else if capacity_limited {
                            AggregateBuildStop::CapacityLimit
                        } else {
                            AggregateBuildStop::QueueEmpty
                        },
                    });
                    observer.observe(AggregateTxObservation::PreparationCompleted {
                        micros: started.saturating_sub(preparation_started.unwrap_or(started)),
                    });
                    observer.observe(AggregateTxObservation::Published {
                        at_micros: started,
                        program_micros: finished.saturating_sub(started),
                    });
                    self.exchange_started_micros = Some(started);
                }
                let deadline = ordinary
                    .now()
                    .saturating_add(ordinary.publication_timeout());
                self.aggregate_phase = Some(AggregateServicePhase::Published(deadline));
                #[cfg(any(feature = "diagnostics", test))]
                control.observe_ht_aggregate(
                    policy.rate(),
                    admission.association().association_id(),
                    admitted,
                );
                self.last_started_frames = admitted;
                self.prepare_ready_standby(aggregate, control, network)?;
                return Ok(WifiTxProgress::Pending);
            }

            self.ordinary_airtime_result(control.start_network_tx(hardware, frame.as_slice()))
        })();
        if !matches!(result, Ok(WifiTxProgress::Pending))
            && let Some(accounting) = self.airtime.as_mut()
        {
            accounting.cancel_active_preparation()?;
            accounting.cancel_selection()?;
        }
        result
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn start_prepared<
        P,
        E,
        T,
        H,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
        const SLOTS: usize,
        const BUFFER_SIZE: usize,
    >(
        &mut self,
        aggregate: &mut AccessPointAmpdu<'_, B, SLOTS, BUFFER_SIZE>,
        control: &mut AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        hardware: &mut H,
        network: &impl SelectedBurstMaterializer<SoftwareFrame = N, PhysicalFrame = B>,
    ) -> Result<WifiTxProgress, AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: oer_time::Timer + crate::mac_clock::MacClockReader,
        H: TxHardware
            + ApRuntimeHardware
            + RxBlockAckHardware
            + oer_esp32s31_ieee80211_mac::tx::ampdu::HtAmpduHardware,
    {
        if let Some(accounting) = self.airtime.as_ref() {
            accounting.require_active_idle()?;
        }
        let _ = self.stage_dtim_group_release(control)?;
        if self.prepared_group_release.is_some() {
            return self.start_prepared_group_release(control, hardware);
        }
        if self.prepared_buffered_release.is_some() {
            return self.start_prepared_buffered_release(control, hardware);
        }
        self.prepare_ready_standby(aggregate, control, network)?;
        if self.prepared_buffered_release.is_some() {
            return self.start_prepared_buffered_release(control, hardware);
        }
        if TX_PHASE_TELEMETRY {
            self.record_partial_frontier(network);
        }
        let Some(_batch) = self.prepared_standby.as_ref() else {
            loop {
                let Some(frame) = self.prepared_first.take() else {
                    return Ok(WifiTxProgress::Complete);
                };
                let key = self
                    .prepared_first_key
                    .take()
                    .expect("prepared AP frame retains its flow key");
                self.prepared_first = self.prepared_second.take();
                self.prepared_first_key = self.prepared_second_key.take();
                if !key.is_current(control.mac.engine()) {
                    if let Some(accounting) = self.airtime.as_mut() {
                        accounting.cancel_selection_for(key)?;
                    }
                    drop(frame);
                    continue;
                }
                let Some((readmitted_key, frame)) =
                    self.retain_power_save(control.mac.engine_mut(), frame)?
                else {
                    if let Some(accounting) = self.airtime.as_mut() {
                        accounting.cancel_selection_for(key)?;
                    }
                    continue;
                };
                if readmitted_key != key {
                    if let Some(accounting) = self.airtime.as_mut() {
                        accounting.cancel_selection_for(key)?;
                    }
                    drop(frame);
                    continue;
                }
                if let Some(accounting) = self.airtime.as_mut() {
                    accounting.reserve_active(control.mac.engine(), key)?;
                }
                return self
                    .ordinary_airtime_result(control.start_network_tx(hardware, frame.as_slice()));
            }
        };
        #[cfg(any(feature = "diagnostics", test))]
        let publication_started = self.observer.map(AggregateTxObserver::now_micros);
        let (_, ordinary) = control.mac.try_aggregate_adapter().map_err(|error| {
            AccessPointDatapathError::Control(AccessPointControlError::Mac(error))
        })?;
        let stamp = radio_stamp(ordinary.timer())?;
        if let Err(error) = aggregate.publish_standby(ordinary, hardware, stamp) {
            return Err(AccessPointDatapathError::Aggregate(error));
        }
        let _batch = self
            .prepared_standby
            .take()
            .expect("published standby owns metadata");
        if let Some(accounting) = self.airtime.as_mut() {
            accounting.publish_standby();
        }
        self.aggregate_phase = Some(AggregateServicePhase::Published(
            ordinary
                .now()
                .saturating_add(ordinary.publication_timeout()),
        ));
        #[cfg(any(feature = "diagnostics", test))]
        {
            self.exchange_started_micros = publication_started;
        }
        #[cfg(any(feature = "diagnostics", test))]
        if let Some(observer) = self.observer {
            let finished = observer.now_micros();
            let started = publication_started.unwrap_or(finished);
            observe_aggregate_rate(observer, _batch.policy.rate());
            observer.observe(AggregateTxObservation::Prepared {
                subframes: u8::try_from(_batch.admitted).unwrap_or(u8::MAX),
                stop: if _batch.admitted == usize::from(_batch.policy.frame_limit()) {
                    AggregateBuildStop::FrameLimit
                } else if self.prepared_first_key
                    == Some(ApTxFlowKey::associated(_batch.admission.association()))
                {
                    AggregateBuildStop::CapacityLimit
                } else {
                    AggregateBuildStop::QueueEmpty
                },
            });
            observer.observe(AggregateTxObservation::PreparationCompleted {
                micros: _batch.preparation_micros,
            });
            observer.observe(AggregateTxObservation::Published {
                at_micros: started,
                program_micros: finished.saturating_sub(started),
            });
            observer.observe(AggregateTxObservation::StandbyPublished);
        }
        #[cfg(any(feature = "diagnostics", test))]
        control.observe_ht_aggregate(
            _batch.policy.rate(),
            _batch.admission.association().association_id(),
            _batch.admitted,
        );
        self.prepare_ready_standby(aggregate, control, network)?;
        Ok(WifiTxProgress::Pending)
    }

    pub(super) fn cancel_prepared<
        P,
        E,
        T,
        const DMA_BUFFER_SIZE: usize,
        const TX_BUFFER_SIZE: usize,
        const SLOTS: usize,
        const BUFFER_SIZE: usize,
    >(
        &mut self,
        aggregate: &mut AccessPointAmpdu<'_, B, SLOTS, BUFFER_SIZE>,
        control: &mut AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
        network: &impl SelectedBurstMaterializer<SoftwareFrame = N, PhysicalFrame = B>,
    ) -> Result<(), AccessPointDatapathError>
    where
        P: WifiTxPowerProfile,
        E: WifiTxEntropy,
        T: oer_time::Timer,
    {
        let _ = network;
        self.rollback_prepared_buffered_release(control)?;
        self.awake_buffered_peer = None;
        self.discard_group_buffer(control)?;
        control
            .rollback_pending_buffered_releases()
            .map_err(AccessPointDatapathError::Control)?;
        self.prepared_first = None;
        self.prepared_first_key = None;
        self.prepared_second = None;
        self.prepared_second_key = None;
        while let Some((_, index)) = self.active_frames.pop_scheduled() {
            drop(self.frame_arena.take(index));
        }
        if self.prepared_standby.is_some() {
            aggregate
                .standby_mut()
                .expect("prepared batch owns standby arena")
                .cancel_build()
                .map_err(AccessPointDatapathError::Aggregate)?;
            self.prepared_standby = None;
            #[cfg(any(feature = "diagnostics", test))]
            if let Some(observer) = self.observer {
                observer.observe(AggregateTxObservation::StandbyCancelled);
            }
        }
        if let Some(accounting) = self.airtime.as_mut() {
            accounting.cancel_standby()?;
            accounting.cancel_selection()?;
            accounting.cancel_active_preparation()?;
        }
        Ok(())
    }
}

/// Narrow bridge for PS-Poll ownership and awake demand. RX can refresh
/// readiness while ordinary TX is active or parked, without acquiring a
/// buffered release or exposing packet storage to the protocol processor.
pub(super) trait AccessPointPowerSaveNetworkTx<
    P,
    E,
    T,
    const DMA_BUFFER_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
>
{
    fn refresh_power_save_demand(
        &mut self,
        control: &mut AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
    ) -> Result<bool, AccessPointDatapathError>;

    fn refresh_awake_demand(&mut self, engine: &ApEngine<'_>);

    fn has_power_save_release(&self) -> bool;

    fn discard_group_power_save(
        &mut self,
        control: &mut AccessPointProtocolProcessor<
            '_,
            '_,
            '_,
            P,
            E,
            T,
            DMA_BUFFER_SIZE,
            TX_BUFFER_SIZE,
        >,
    ) -> Result<(), AccessPointDatapathError>;
}

#[cfg(test)]
mod tests;
