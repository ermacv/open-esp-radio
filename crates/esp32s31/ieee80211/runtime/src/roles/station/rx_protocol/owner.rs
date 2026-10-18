use super::*;

impl<
    'queue,
    'pool,
    'scratch,
    'irq,
    M: RawMutex,
    S,
    K,
    const CAPACITY: usize,
    const SLOTS: usize,
    const REORDER_SLOTS: usize,
> ConnectedRxProcessor<'queue, 'pool, 'scratch, 'irq, M, S, K, CAPACITY, SLOTS, REORDER_SLOTS>
where
    S: ConnectedRxProtocolSink<CAPACITY, SLOTS>,
{
    pub fn new_with_reorder_slots(
        irq: &'irq EmbassyMacIrqRuntime<M>,
        sink: S,
        mpdu: &'scratch mut [u8],
        ethernet: &'scratch mut [u8],
        runtime: &'pool mut ConnectedReceiveStorage<'pool, CAPACITY, SLOTS, REORDER_SLOTS>,
        clock: K,
    ) -> Self {
        assert!(
            runtime.dispatcher_configured(),
            "connected RX protocol requires a configured dispatcher epoch"
        );
        assert!(
            CAPACITY <= usize::from(u16::MAX),
            "staged RX capacity must fit the deferred record length"
        );
        assert!(
            ethernet.len() >= CAPACITY,
            "A-MSDU output scratch must cover one complete staged RX unit"
        );
        assert!(SLOTS != 0, "staged RX pool must not be empty");
        assert!(
            REORDER_SLOTS != 0,
            "RX reorder slot domain must not be empty"
        );
        assert!(
            REORDER_SLOTS <= RX_REORDER_BACKING_SLOT_COUNT,
            "RX reorder slot domain exceeds the MAC maximum"
        );
        assert!(
            SLOTS <= usize::from(u8::MAX) + 1,
            "reorder slot identity must fit the MAC token"
        );
        Self {
            irq,
            clock,
            sink,
            mpdu,
            ethernet,
            #[cfg(any(feature = "diagnostics", test))]
            pipeline_observer: None,
            #[cfg(any(feature = "diagnostics", test))]
            reorder_observer: None,
            reorder_commands: None,
            reorder_storage: None,
            reorder_scratch: None,
            runtime,
        }
    }

    pub fn with_rx_reorder_commands(
        mut self,
        commands: RxReorderCommandReceiver<'queue, M>,
    ) -> Self {
        self.reorder_commands = Some(commands);
        self
    }

    /// Install cold backing for MPDUs that actually cross a sequence gap.
    pub fn with_rx_reorder_storage(
        mut self,
        storage: &'pool RxReorderFrameStorage<CAPACITY, REORDER_SLOTS>,
    ) -> Self {
        self.reorder_storage = Some(storage);
        self
    }

    /// Install one internal-SRAM readback scratch for a retained ordinary MPDU.
    pub fn with_rx_reorder_scratch(mut self, scratch: &'scratch mut [u8]) -> Self {
        assert!(
            scratch.len() >= CAPACITY,
            "reorder readback scratch must cover one complete staged RX unit"
        );
        self.reorder_scratch = Some(scratch);
        self
    }

    #[cfg(any(feature = "diagnostics", test))]
    pub fn with_pipeline_observer(mut self, observer: &'queue dyn RxPipelineObserver) -> Self {
        self.pipeline_observer = Some(observer);
        self
    }

    #[cfg(any(feature = "diagnostics", test))]
    pub fn with_reorder_observer(
        mut self,
        observer: &'queue dyn RxReorderAgreementObserver,
    ) -> Self {
        self.reorder_observer = Some(observer);
        self
    }

    pub const fn dispatcher(&self) -> &ConnectedRxDispatcher {
        self.runtime.dispatcher()
    }

    pub const fn sink(&self) -> &S {
        &self.sink
    }

    pub fn sink_mut(&mut self) -> &mut S {
        &mut self.sink
    }

    /// Discard reorder/mailbox ownership retained by the protocol processor.
    pub fn shutdown_discard(&mut self) -> ConnectedRxProtocolShutdown {
        // The task is synchronously quiescent at this boundary, so no replay
        // publication permit should still exist. If that invariant is broken,
        // explicitly drop the static-arena endpoint: its Drop path revokes RX
        // authority and quarantines group replay instead of letting a later
        // epoch reuse this dispatcher.
        if self.runtime.dispatcher.stop_ccmp_rx_replay().is_err() {
            self.runtime.dispatcher.quarantine_ccmp_rx_replay();
        }
        let mut shutdown = ConnectedRxProtocolShutdown {
            esp_now_duplicate_entries: self.runtime.dispatcher.stop_esp_now_rx_epoch(),
            incomplete_fragment_contexts: self.runtime.dispatcher.clear_fragmentation(),
            ..ConnectedRxProtocolShutdown::default()
        };
        self.runtime.dispatcher.clear_duplicate_history();
        self.runtime.mark_dispatcher_stopped();
        if let Some(commands) = &self.reorder_commands {
            while try_receive_rx_reorder_command(commands).is_some() {
                shutdown.reorder_commands = shutdown.reorder_commands.saturating_add(1);
            }
        }
        for bank in 0..RX_BLOCK_ACK_BANK_COUNT {
            if self.runtime.reorder_banks.stop_bank(bank).is_some() {
                shutdown.active_reorders = shutdown.active_reorders.saturating_add(1);
            }
        }
        self.runtime.reorder_first_starts.fill(None);
        self.runtime.gap_deadlines.fill(None);
        for retained in &mut self.runtime.retained {
            if retained.take().is_some() {
                shutdown.retained_frames = shutdown.retained_frames.saturating_add(1);
            }
        }
        if shutdown.retained_frames != 0 {
            self.irq.notify_rx_capacity();
        }
        shutdown
    }

    /// Finish a queue-independent processor after its outer paired RX owner
    /// has stopped and discarded the route-tagged queue.
    ///
    /// The returned value is identical to the standalone protocol terminal
    /// owner: scratch and the persistent reorder arena cannot be recovered
    /// before every retained protocol lease has been revoked.
    pub fn into_stopped(
        mut self,
    ) -> ConnectedProtocolStopped<'scratch, 'pool, CAPACITY, SLOTS, REORDER_SLOTS> {
        let shutdown = self.shutdown_discard();
        let (mpdu, ethernet, runtime) = self.into_stopped_parts();
        ConnectedRxProtocolStopped {
            shutdown,
            mpdu,
            ethernet,
            runtime,
        }
    }

    /// Finish a queue-independent processor and return its role-local sink.
    ///
    /// Paired STA+AP owns an additional Ethernet batch scratch inside the
    /// sink. The outer composition must recover that exact allocation during
    /// rollback; dropping it would make the next paired epoch impossible even
    /// though DMA and protocol reorder state were cleanly stopped.
    pub fn into_stopped_with_sink(
        mut self,
    ) -> (
        ConnectedProtocolStopped<'scratch, 'pool, CAPACITY, SLOTS, REORDER_SLOTS>,
        S,
    ) {
        let shutdown = self.shutdown_discard();
        let Self {
            sink,
            mpdu,
            ethernet,
            runtime,
            ..
        } = self;
        (
            ConnectedRxProtocolStopped {
                shutdown,
                mpdu,
                ethernet,
                runtime,
            },
            sink,
        )
    }

    pub(super) fn into_stopped_parts(
        self,
    ) -> (
        &'scratch mut [u8],
        &'scratch mut [u8],
        &'pool mut ConnectedReceiveStorage<'pool, CAPACITY, SLOTS, REORDER_SLOTS>,
    ) {
        (self.mpdu, self.ethernet, self.runtime)
    }
}

impl<'queue, 'pool, 'scratch, 'irq, M: RawMutex, S, K, const CAPACITY: usize, const SLOTS: usize>
    ConnectedRxProcessor<
        'queue,
        'pool,
        'scratch,
        'irq,
        M,
        S,
        K,
        CAPACITY,
        SLOTS,
        RX_REORDER_BACKING_SLOT_COUNT,
    >
where
    S: ConnectedRxProtocolSink<CAPACITY, SLOTS>,
{
    pub fn new(
        irq: &'irq EmbassyMacIrqRuntime<M>,
        sink: S,
        mpdu: &'scratch mut [u8],
        ethernet: &'scratch mut [u8],
        runtime: &'pool mut ConnectedReceiveStorage<
            'pool,
            CAPACITY,
            SLOTS,
            RX_REORDER_BACKING_SLOT_COUNT,
        >,
        clock: K,
    ) -> Self {
        Self::new_with_reorder_slots(irq, sink, mpdu, ethernet, runtime, clock)
    }
}

impl<
    'queue,
    'pool,
    'scratch,
    'irq,
    M: RawMutex,
    S,
    K,
    const DEPTH: usize,
    const CAPACITY: usize,
    const SLOTS: usize,
    const REORDER_SLOTS: usize,
>
    ConnectedReceiveProtocol<
        'queue,
        'pool,
        'scratch,
        'irq,
        M,
        S,
        K,
        DEPTH,
        CAPACITY,
        SLOTS,
        REORDER_SLOTS,
    >
where
    S: ConnectedRxProtocolSink<CAPACITY, SLOTS>,
{
    pub fn new_with_reorder_slots(
        frames: StagedRxReceiver<'queue, 'pool, M, DEPTH, CAPACITY, SLOTS>,
        irq: &'irq EmbassyMacIrqRuntime<M>,
        sink: S,
        mpdu: &'scratch mut [u8],
        ethernet: &'scratch mut [u8],
        runtime: &'pool mut ConnectedReceiveStorage<'pool, CAPACITY, SLOTS, REORDER_SLOTS>,
        clock: K,
    ) -> Self {
        Self {
            frames,
            processor: ConnectedRxProcessor::new_with_reorder_slots(
                irq, sink, mpdu, ethernet, runtime, clock,
            ),
        }
    }

    pub fn with_rx_reorder_commands(
        mut self,
        commands: RxReorderCommandReceiver<'queue, M>,
    ) -> Self {
        self.processor.reorder_commands = Some(commands);
        self
    }

    pub fn with_rx_reorder_storage(
        mut self,
        storage: &'pool RxReorderFrameStorage<CAPACITY, REORDER_SLOTS>,
    ) -> Self {
        self.processor.reorder_storage = Some(storage);
        self
    }

    pub fn with_rx_reorder_scratch(mut self, scratch: &'scratch mut [u8]) -> Self {
        assert!(
            scratch.len() >= CAPACITY,
            "reorder readback scratch must cover one complete staged RX unit"
        );
        self.processor.reorder_scratch = Some(scratch);
        self
    }

    #[cfg(any(feature = "diagnostics", test))]
    pub fn with_pipeline_observer(mut self, observer: &'queue dyn RxPipelineObserver) -> Self {
        self.processor.pipeline_observer = Some(observer);
        self
    }

    #[cfg(any(feature = "diagnostics", test))]
    pub fn with_reorder_observer(
        mut self,
        observer: &'queue dyn RxReorderAgreementObserver,
    ) -> Self {
        self.processor.reorder_observer = Some(observer);
        self
    }

    pub const fn dispatcher(&self) -> &ConnectedRxDispatcher {
        self.processor.dispatcher()
    }

    pub const fn sink(&self) -> &S {
        self.processor.sink()
    }

    pub fn sink_mut(&mut self) -> &mut S {
        self.processor.sink_mut()
    }

    pub fn queue_len(&self) -> usize {
        self.frames.len()
    }

    /// Remove the standalone queue at a proven empty scheduling boundary.
    ///
    /// Same-channel STA+AP owns one different, route-tagged queue. Moving the
    /// queue-independent processor into that composition is valid only after
    /// the standalone producer has stopped and its consumer has drained every
    /// staged lease. A non-empty queue returns the complete original owner.
    pub fn try_into_processor(
        self,
    ) -> Result<
        ConnectedRxProcessor<
            'queue,
            'pool,
            'scratch,
            'irq,
            M,
            S,
            K,
            CAPACITY,
            SLOTS,
            REORDER_SLOTS,
        >,
        Self,
    > {
        if !self.frames.is_empty() {
            return Err(self);
        }
        Ok(self.processor)
    }

    /// Reattach the standalone queue after the paired producer has stopped.
    ///
    /// The supplied queue must be empty: queued paired frames have a distinct
    /// tagged type and must be drained before the station can resume its
    /// standalone lifecycle.
    pub fn from_processor(
        frames: StagedRxReceiver<'queue, 'pool, M, DEPTH, CAPACITY, SLOTS>,
        processor: ConnectedRxProcessor<
            'queue,
            'pool,
            'scratch,
            'irq,
            M,
            S,
            K,
            CAPACITY,
            SLOTS,
            REORDER_SLOTS,
        >,
    ) -> Self {
        assert_eq!(
            frames.len(),
            0,
            "standalone STA resume requires an empty staged-RX queue"
        );
        Self { frames, processor }
    }

    pub fn shutdown_discard(&mut self) -> ConnectedRxProtocolShutdown {
        let mut shutdown = ConnectedRxProtocolShutdown::default();
        while let Ok(frame) = self.frames.try_receive() {
            drop(frame);
            shutdown.queued_frames = shutdown.queued_frames.saturating_add(1);
        }
        let retained = self.processor.shutdown_discard();
        shutdown.retained_frames = retained.retained_frames;
        shutdown.reorder_commands = retained.reorder_commands;
        shutdown.active_reorders = retained.active_reorders;
        shutdown.esp_now_duplicate_entries = retained.esp_now_duplicate_entries;
        shutdown.incomplete_fragment_contexts = retained.incomplete_fragment_contexts;
        if shutdown.queued_frames != 0 {
            self.processor.irq.notify_rx_capacity();
        }
        shutdown
    }

    /// Stop the queue and processor now that the common DATAPATH owner has
    /// reached its terminal edge.
    pub fn into_stopped(
        mut self,
    ) -> ConnectedProtocolStopped<'scratch, 'pool, CAPACITY, SLOTS, REORDER_SLOTS> {
        let shutdown = self.shutdown_discard();
        let (mpdu, ethernet, runtime) = self.into_stopped_parts();
        ConnectedRxProtocolStopped {
            shutdown,
            mpdu,
            ethernet,
            runtime,
        }
    }

    pub(super) fn into_stopped_parts(
        self,
    ) -> (
        &'scratch mut [u8],
        &'scratch mut [u8],
        &'pool mut ConnectedReceiveStorage<'pool, CAPACITY, SLOTS, REORDER_SLOTS>,
    ) {
        self.processor.into_stopped_parts()
    }
}

impl<
    'queue,
    'pool,
    'scratch,
    'irq,
    M: RawMutex,
    S,
    K,
    const DEPTH: usize,
    const CAPACITY: usize,
    const SLOTS: usize,
>
    ConnectedReceiveProtocol<
        'queue,
        'pool,
        'scratch,
        'irq,
        M,
        S,
        K,
        DEPTH,
        CAPACITY,
        SLOTS,
        RX_REORDER_BACKING_SLOT_COUNT,
    >
where
    S: ConnectedRxProtocolSink<CAPACITY, SLOTS>,
{
    pub fn new(
        frames: StagedRxReceiver<'queue, 'pool, M, DEPTH, CAPACITY, SLOTS>,
        irq: &'irq EmbassyMacIrqRuntime<M>,
        sink: S,
        mpdu: &'scratch mut [u8],
        ethernet: &'scratch mut [u8],
        runtime: &'pool mut ConnectedReceiveStorage<
            'pool,
            CAPACITY,
            SLOTS,
            RX_REORDER_BACKING_SLOT_COUNT,
        >,
        clock: K,
    ) -> Self {
        Self::new_with_reorder_slots(frames, irq, sink, mpdu, ethernet, runtime, clock)
    }
}
