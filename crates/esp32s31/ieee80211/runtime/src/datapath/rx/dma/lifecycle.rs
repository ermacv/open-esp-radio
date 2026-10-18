use super::*;

impl<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
>
    StoppedReceive<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >
{
    pub const fn ring(&self) -> &RxRingHalted<'storage, COUNT> {
        &self.ring
    }

    pub const fn buffers(
        &self,
    ) -> &'storage [ReceiveDmaBuffer<DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>; COUNT] {
        self.storage.buffers()
    }

    pub const fn storage(
        &self,
    ) -> &'storage ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE> {
        self.storage
    }

    pub const fn pool(&self) -> &'pool RxStagePool<STAGE_SLOTS, STAGE_CAPACITY> {
        self.pool
    }

    pub const fn delay(&self) -> &D {
        &self.delay
    }

    #[cfg(any(feature = "diagnostics", test))]
    pub const fn pipeline_observer(&self) -> Option<&'pool dyn RxPipelineObserver> {
        self.pipeline_observer
    }

    pub fn queued_frames(&self) -> usize {
        self.frames.len()
    }

    /// Separate the peer-specific halted frontier from persistent connected
    /// RX resources for a finite pre-connected protocol epoch.
    pub fn into_epoch_parts(
        self,
    ) -> (
        RxRingHalted<'storage, COUNT>,
        RxEpochResources<
            'storage,
            'pool,
            'queue,
            D,
            M,
            QUEUE_DEPTH,
            COUNT,
            STAGE_CAPACITY,
            STAGE_SLOTS,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
        >,
    ) {
        let Self {
            ring,
            storage,
            pool,
            frames,
            delay,
            #[cfg(any(feature = "diagnostics", test))]
            pipeline_observer,
        } = self;
        (
            ring,
            RxEpochResources {
                ring_lifetime: PhantomData,
                storage,
                pool,
                frames,
                delay,
                #[cfg(any(feature = "diagnostics", test))]
                pipeline_observer,
            },
        )
    }

    /// Rebuild descriptor and buffer state for a fresh association epoch.
    ///
    /// Hardware is already confirmed stopped by this type. On every failure
    /// the complete halted owner is reconstructed, including its queue sender
    /// and delay implementation.
    #[allow(clippy::result_large_err)]
    pub fn prepare<H: RxDma>(
        self,
        hardware: &mut H,
    ) -> Result<
        PreparedRx<
            'storage,
            'pool,
            'queue,
            D,
            M,
            QUEUE_DEPTH,
            COUNT,
            STAGE_CAPACITY,
            STAGE_SLOTS,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
        >,
        (Self, RxRingError),
    > {
        if DMA_BUFFER_SIZE > u32::MAX as usize {
            return Err((self, RxRingError::Size));
        }
        let Self {
            ring,
            storage,
            pool,
            frames,
            delay,
            #[cfg(any(feature = "diagnostics", test))]
            pipeline_observer,
        } = self;
        match storage.prepare_halted(ring, hardware) {
            Ok(ring) => Ok(PreparedRx {
                ring,
                storage,
                pool,
                frames,
                delay,
                #[cfg(any(feature = "diagnostics", test))]
                pipeline_observer,
            }),
            Err((ring, error)) => Err((
                Self {
                    ring,
                    storage,
                    pool,
                    frames,
                    delay,
                    #[cfg(any(feature = "diagnostics", test))]
                    pipeline_observer,
                },
                error,
            )),
        }
    }
}

impl<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
>
    RxEpochResources<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >
{
    /// Bind board-allocated DMA/staging resources before the first connected
    /// epoch. Later epochs recover this same owner from [`StoppedReceive`].
    pub fn new(
        storage: &'static ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        pool: &'pool RxStagePool<STAGE_SLOTS, STAGE_CAPACITY>,
        frames: StagedRxSender<'queue, 'pool, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
        delay: D,
    ) -> Self {
        Self {
            ring_lifetime: PhantomData,
            storage,
            pool,
            frames: StagedRxPublisher::standalone(frames),
            delay,
            #[cfg(any(feature = "diagnostics", test))]
            pipeline_observer: None,
        }
    }

    /// Bind one ordered fact-routed queue for a same-channel STA+AP epoch.
    pub fn new_sta_ap(
        storage: &'static ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        pool: &'pool RxStagePool<STAGE_SLOTS, STAGE_CAPACITY>,
        frames: crate::datapath::rx::routed::StaApStagedRxSender<
            'pool,
            'queue,
            M,
            QUEUE_DEPTH,
            STAGE_CAPACITY,
            STAGE_SLOTS,
        >,
        ingress: oer_esp32s31_ieee80211_mac::rx::RxIngressConfig,
        addresses: oer_ieee80211_mac::vif::StaApRxAddresses,
        delay: D,
    ) -> Self {
        Self {
            ring_lifetime: PhantomData,
            storage,
            pool,
            frames: StagedRxPublisher::sta_ap(frames, ingress, addresses),
            delay,
            #[cfg(any(feature = "diagnostics", test))]
            pipeline_observer: None,
        }
    }

    #[cfg(any(feature = "diagnostics", test))]
    pub fn with_pipeline_observer(mut self, observer: &'pool dyn RxPipelineObserver) -> Self {
        self.pipeline_observer = Some(observer);
        self
    }

    pub const fn buffers(
        &self,
    ) -> &'storage [ReceiveDmaBuffer<DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>; COUNT] {
        self.storage.buffers()
    }

    pub const fn storage(
        &self,
    ) -> &'storage ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE> {
        self.storage
    }

    /// The timer the walker settles on.
    pub const fn delay(&self) -> &D {
        &self.delay
    }

    pub fn queued_frames(&self) -> usize {
        self.frames.len()
    }

    /// Reacquire the sole standalone protocol consumer while the physical
    /// producer is detached from a logical DMA role.
    pub fn try_resume_standalone_receiver(
        &self,
    ) -> Option<StagedRxReceiver<'queue, 'pool, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>> {
        self.frames.try_resume_standalone_receiver()
    }

    /// Reassemble the stopped production owner after a finite join attempt.
    pub fn with_halted_ring(
        self,
        ring: RxRingHalted<'storage, COUNT>,
    ) -> StoppedReceive<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    > {
        StoppedReceive {
            ring,
            storage: self.storage,
            pool: self.pool,
            frames: self.frames,
            delay: self.delay,
            #[cfg(any(feature = "diagnostics", test))]
            pipeline_observer: self.pipeline_observer,
        }
    }

    /// Promote the same persistent resources into a connected RX service
    /// after Association/WPA2 returns the live ring frontier.
    pub fn with_live_ring(
        self,
        ring: RxRingLive<'storage, COUNT>,
    ) -> StagedRxProducer<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    > {
        StagedRxProducer {
            ring_lifetime: PhantomData,
            ring,
            storage: self.storage,
            pool: self.pool,
            frames: self.frames,
            delay: self.delay,
            #[cfg(any(feature = "diagnostics", test))]
            pipeline_observer: self.pipeline_observer,
            admission: FullRxStageAdmission,
            serviced_descriptors: 0,
            serviced_units: 0,
            serviced_bytes: 0,
        }
    }
}

impl<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
>
    PreparedRx<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >
{
    /// The timer the walker settles on.
    pub(crate) const fn delay(&self) -> &D {
        &self.delay
    }

    pub const fn ring(&self) -> &RxRingStopped<'storage, COUNT> {
        &self.ring
    }

    pub fn queued_frames(&self) -> usize {
        self.frames.len()
    }

    /// Observe the required settle delay and open a fresh live RX epoch.
    ///
    /// A rejected walker-enable readback returns this prepared owner intact,
    /// so a higher-level reset/retry policy never loses static resources.
    #[allow(clippy::result_large_err)]
    pub async fn start<H: RxDma>(
        self,
        hardware: &mut H,
    ) -> Result<
        StagedRxProducer<
            'storage,
            'pool,
            'queue,
            D,
            M,
            QUEUE_DEPTH,
            COUNT,
            STAGE_CAPACITY,
            STAGE_SLOTS,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
        >,
        (Self, RxRingError),
    >
    where
        D: oer_time::Timer,
    {
        let Self {
            ring,
            storage,
            pool,
            frames,
            delay,
            #[cfg(any(feature = "diagnostics", test))]
            pipeline_observer,
        } = self;
        walker_enable_settle(&delay).await;
        match ring.try_start(hardware) {
            Ok(ring) => Ok(StagedRxProducer {
                ring_lifetime: PhantomData,
                ring,
                storage,
                pool,
                frames,
                delay,
                #[cfg(any(feature = "diagnostics", test))]
                pipeline_observer,
                admission: FullRxStageAdmission,
                serviced_descriptors: 0,
                serviced_units: 0,
                serviced_bytes: 0,
            }),
            Err((ring, error)) => Err((
                Self {
                    ring,
                    storage,
                    pool,
                    frames,
                    delay,
                    #[cfg(any(feature = "diagnostics", test))]
                    pipeline_observer,
                },
                error,
            )),
        }
    }
}

impl<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
>
    StagedRxProducer<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >
{
    /// The timer the walker settles on.
    pub(crate) const fn delay(&self) -> &D {
        &self.delay
    }

    pub fn new(
        ring: RxRingLive<'storage, COUNT>,
        storage: &'static ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        pool: &'pool RxStagePool<STAGE_SLOTS, STAGE_CAPACITY>,
        delay: D,
        frames: StagedRxSender<'queue, 'pool, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>,
    ) -> Self {
        Self {
            ring_lifetime: PhantomData,
            ring,
            storage,
            pool,
            frames: StagedRxPublisher::standalone(frames),
            delay,
            #[cfg(any(feature = "diagnostics", test))]
            pipeline_observer: None,
            admission: FullRxStageAdmission,
            serviced_descriptors: 0,
            serviced_units: 0,
            serviced_bytes: 0,
        }
    }

    /// Create the sole physical producer for a same-channel STA+AP stream.
    pub fn new_sta_ap(
        ring: RxRingLive<'storage, COUNT>,
        storage: &'static ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        pool: &'pool RxStagePool<STAGE_SLOTS, STAGE_CAPACITY>,
        delay: D,
        frames: crate::datapath::rx::routed::StaApStagedRxSender<
            'pool,
            'queue,
            M,
            QUEUE_DEPTH,
            STAGE_CAPACITY,
            STAGE_SLOTS,
        >,
        ingress: oer_esp32s31_ieee80211_mac::rx::RxIngressConfig,
        addresses: oer_ieee80211_mac::vif::StaApRxAddresses,
    ) -> Self {
        Self {
            ring_lifetime: PhantomData,
            ring,
            storage,
            pool,
            frames: StagedRxPublisher::sta_ap(frames, ingress, addresses),
            delay,
            #[cfg(any(feature = "diagnostics", test))]
            pipeline_observer: None,
            admission: FullRxStageAdmission,
            serviced_descriptors: 0,
            serviced_units: 0,
            serviced_bytes: 0,
        }
    }

    #[cfg(any(feature = "diagnostics", test))]
    pub fn with_pipeline_observer(mut self, observer: &'pool dyn RxPipelineObserver) -> Self {
        self.pipeline_observer = Some(observer);
        self
    }

    /// Install a statically dispatched ingress admission policy.
    ///
    /// The default policy is zero-sized and admits the complete physical
    /// staging slot. Changing it consumes the owner so a policy cannot be
    /// swapped while a DMA transaction is in progress.
    /// The selected credit policy is validated against both pool and queue at
    /// the next service entry, before any hardware or ownership changes. The
    /// default reserves one credit and rejects a one-credit domain; such a
    /// profile must explicitly install [`UnreservedRxStageAdmission`].
    pub fn with_stage_admission_policy<P>(
        self,
        admission: P,
    ) -> StagedRxProducer<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
        P,
    > {
        StagedRxProducer {
            ring_lifetime: PhantomData,
            ring: self.ring,
            storage: self.storage,
            pool: self.pool,
            frames: self.frames,
            delay: self.delay,
            #[cfg(any(feature = "diagnostics", test))]
            pipeline_observer: self.pipeline_observer,
            admission,
            serviced_descriptors: self.serviced_descriptors,
            serviced_units: self.serviced_units,
            serviced_bytes: self.serviced_bytes,
        }
    }
}

impl<
    'storage,
    'pool,
    'queue,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    P,
>
    StagedRxProducer<
        'storage,
        'pool,
        'queue,
        D,
        M,
        QUEUE_DEPTH,
        COUNT,
        STAGE_CAPACITY,
        STAGE_SLOTS,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
        P,
    >
{
    pub const fn ring(&self) -> &RxRingLive<'storage, COUNT> {
        &self.ring
    }

    pub const fn storage(
        &self,
    ) -> &'storage ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE> {
        self.storage
    }

    /// Preserve exact immutable layout evidence after a fail-closed RX exit.
    pub fn first_buffer_address_mismatch(
        &self,
    ) -> Option<oer_esp32s31_ieee80211_dma::rx_storage::RxBufferAddressMismatch> {
        self.storage
            .first_buffer_address_mismatch(self.ring.buffer_addresses())
    }

    pub fn queued_frames(&self) -> usize {
        self.frames.len()
    }

    /// Return whether this physical producer can cross a logical role edge
    /// without stopping the descriptor walker.
    ///
    /// Every staged lease must already have returned to the common pool and
    /// the producer queue must be empty. The live ring itself is deliberately
    /// not inspected or modified: its DMA frontier continues unchanged.
    pub fn can_park_for_role_handoff(&self) -> bool {
        self.pool.claimed_slots() == 0 && self.frames.len() == 0
    }

    /// Split a quiescent logical producer from the still-running physical RX
    /// ring.
    ///
    /// This is the inverse of [`RxEpochResources::with_live_ring`]. It
    /// changes only software ownership; WALKER_ENABLE, NEXT, LAST and every
    /// descriptor word remain owned by the same continuous DMA epoch.
    #[allow(clippy::type_complexity, clippy::result_large_err)]
    pub fn try_into_live_epoch_parts(
        self,
    ) -> Result<
        (
            RxRingLive<'storage, COUNT>,
            RxEpochResources<
                'storage,
                'pool,
                'queue,
                D,
                M,
                QUEUE_DEPTH,
                COUNT,
                STAGE_CAPACITY,
                STAGE_SLOTS,
                DMA_BUFFER_SIZE,
                DMA_STORAGE_SIZE,
            >,
        ),
        (Self, RxRingError),
    > {
        if !self.can_park_for_role_handoff() {
            return Err((self, RxRingError::Busy));
        }
        let Self {
            ring_lifetime: _,
            ring,
            storage,
            pool,
            frames,
            delay,
            #[cfg(any(feature = "diagnostics", test))]
            pipeline_observer,
            admission: _,
            serviced_descriptors: _,
            serviced_units: _,
            serviced_bytes: _,
        } = self;
        Ok((
            ring,
            RxEpochResources {
                ring_lifetime: PhantomData,
                storage,
                pool,
                frames,
                delay,
                #[cfg(any(feature = "diagnostics", test))]
                pipeline_observer,
            },
        ))
    }

    /// Reacquire the sole standalone protocol consumer retained by neither
    /// the stopped nor live DMA owner across a station reconnect boundary.
    pub fn try_resume_standalone_receiver(
        &self,
    ) -> Option<StagedRxReceiver<'queue, 'pool, M, QUEUE_DEPTH, STAGE_CAPACITY, STAGE_SLOTS>> {
        self.frames.try_resume_standalone_receiver()
    }

    /// Monotonic descriptor progress used only to prove that a bounded drain
    /// iteration advanced the hardware ownership frontier.
    pub const fn serviced_descriptors(&self) -> u64 {
        self.serviced_descriptors
    }

    /// Monotonic completed-unit and byte totals independent of the hardware
    /// type used to service this producer.
    pub const fn work_counters(&self) -> DatapathRxWorkCounters {
        DatapathRxWorkCounters {
            completed_units: self.serviced_units,
            staged_bytes: self.serviced_bytes,
        }
    }

    /// Confirm that DMA released the ring and return a stopped RX owner.
    ///
    /// On failure the complete live owner is returned together with the
    /// hardware error; no staging, queue or delay capability is lost.
    #[allow(clippy::result_large_err)]
    pub fn try_stop<H: RxDma>(
        self,
        hardware: &mut H,
    ) -> Result<
        StoppedReceive<
            'storage,
            'pool,
            'queue,
            D,
            M,
            QUEUE_DEPTH,
            COUNT,
            STAGE_CAPACITY,
            STAGE_SLOTS,
            DMA_BUFFER_SIZE,
            DMA_STORAGE_SIZE,
        >,
        (Self, RxRingError),
    > {
        if self.pool.claimed_slots() != 0 {
            return Err((self, RxRingError::Busy));
        }
        let Self {
            ring_lifetime: _,
            ring,
            storage,
            pool,
            frames,
            delay,
            #[cfg(any(feature = "diagnostics", test))]
            pipeline_observer,
            admission,
            serviced_descriptors,
            serviced_units,
            serviced_bytes,
        } = self;
        match ring.try_stop(hardware) {
            Ok(ring) => Ok(StoppedReceive {
                ring,
                storage,
                pool,
                frames,
                delay,
                #[cfg(any(feature = "diagnostics", test))]
                pipeline_observer,
            }),
            Err((ring, error)) => Err((
                Self {
                    ring_lifetime: PhantomData,
                    ring,
                    storage,
                    pool,
                    frames,
                    delay,
                    #[cfg(any(feature = "diagnostics", test))]
                    pipeline_observer,
                    admission,
                    serviced_descriptors,
                    serviced_units,
                    serviced_bytes,
                },
                error,
            )),
        }
    }
}
