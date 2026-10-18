//! Bounded standalone-STA RX producer/consumer owner.
//!
//! The physical DMA producer and connected protocol processor deliberately
//! live in one DATAPATH service. A turn first consumes already staged leases,
//! refills the hardware ring once, then consumes newly staged leases with the
//! remaining frame credit. This preserves the finite DMA frontier while
//! removing the executor task and `select` boundary between the two phases.

use core::future::Future;

use crate::{
    datapath::{
        DatapathRxProgress, DatapathRxServiceContext, DatapathRxWorkCounters,
        network::DatapathNetworkRxSet, rx::turn::FusedRxTurn, services::DatapathRxService,
    },
    roles::station::{
        rx_protocol::{
            ConnectedProtocolStopped, ConnectedReceiveProtocol, ConnectedRxProtocolSink,
        },
        teardown::ConnectedStaRxPark,
    },
};

use embassy_sync::blocking_mutex::raw::RawMutex;

const ACTIVE_TX_PROTOCOL_QUANTUM_FRAMES: usize = 4;

/// One standalone-STA RX owner spanning DMA staging and 802.11 processing.
pub struct ConnectedStaRxService<R, P> {
    dma: R,
    protocol: P,
    serviced_frames: u64,
}

/// Reusable owners returned only after both DMA and protocol state stop.
pub struct ConnectedStaRxParked<R, P> {
    dma: R,
    protocol: P,
}

impl<R, P> ConnectedStaRxParked<R, P> {
    pub fn into_parts(self) -> (R, P) {
        (self.dma, self.protocol)
    }
}

impl<R, P> ConnectedStaRxService<R, P> {
    /// Retain protocol and progress with either the new DMA owner or its fault.
    #[allow(clippy::type_complexity, clippy::result_large_err)]
    pub fn try_map_dma<T, E>(
        self,
        map: impl FnOnce(R) -> Result<T, E>,
    ) -> Result<ConnectedStaRxService<T, P>, ConnectedStaRxService<E, P>> {
        match map(self.dma) {
            Ok(dma) => Ok(ConnectedStaRxService {
                dma,
                protocol: self.protocol,
                serviced_frames: self.serviced_frames,
            }),
            Err(dma) => Err(ConnectedStaRxService {
                dma,
                protocol: self.protocol,
                serviced_frames: self.serviced_frames,
            }),
        }
    }

    /// Change DMA state while retaining protocol queues, security/reorder
    /// owners and the cumulative number of serviced frames.
    pub fn map_dma<T>(self, map: impl FnOnce(R) -> T) -> ConnectedStaRxService<T, P> {
        ConnectedStaRxService {
            dma: map(self.dma),
            protocol: self.protocol,
            serviced_frames: self.serviced_frames,
        }
    }

    pub const fn new(dma: R, protocol: P) -> Self {
        Self {
            dma,
            protocol,
            serviced_frames: 0,
        }
    }

    pub const fn dma(&self) -> &R {
        &self.dma
    }

    pub fn dma_mut(&mut self) -> &mut R {
        &mut self.dma
    }

    pub const fn protocol(&self) -> &P {
        &self.protocol
    }

    pub fn protocol_mut(&mut self) -> &mut P {
        &mut self.protocol
    }

    pub const fn serviced_frames(&self) -> u64 {
        self.serviced_frames
    }

    pub fn into_parts(self) -> (R, P) {
        (self.dma, self.protocol)
    }
}

impl<
    'queue,
    'pool,
    'scratch,
    'irq,
    M,
    H,
    R,
    S,
    K,
    const DEPTH: usize,
    const CAPACITY: usize,
    const SLOTS: usize,
    const REORDER_SLOTS: usize,
> ConnectedStaRxPark<H>
    for ConnectedStaRxService<
        R,
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
        >,
    >
where
    M: RawMutex,
    R: ConnectedStaRxPark<H>,
    S: ConnectedRxProtocolSink<CAPACITY, SLOTS>,
    K: oer_time::Timer,
{
    type Parked = ConnectedStaRxParked<
        R::Parked,
        ConnectedProtocolStopped<'scratch, 'pool, CAPACITY, SLOTS, REORDER_SLOTS>,
    >;
    type Error = R::Error;

    fn try_park(self, hardware: &mut H) -> Result<Self::Parked, (Self, Self::Error)> {
        let Self {
            dma,
            protocol,
            serviced_frames,
        } = self;
        match dma.try_park(hardware) {
            Ok(dma) => Ok(ConnectedStaRxParked {
                dma,
                protocol: protocol.into_stopped(),
            }),
            Err((dma, error)) => Err((
                Self {
                    dma,
                    protocol,
                    serviced_frames,
                },
                error,
            )),
        }
    }
}

impl<
    'queue,
    'pool,
    'scratch,
    'irq,
    M,
    H,
    R,
    S,
    K,
    const DEPTH: usize,
    const CAPACITY: usize,
    const SLOTS: usize,
    const REORDER_SLOTS: usize,
> DatapathRxService<H>
    for ConnectedStaRxService<
        R,
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
        >,
    >
where
    M: RawMutex,
    R: DatapathRxService<H>,
    S: ConnectedRxProtocolSink<CAPACITY, SLOTS>,
    K: oer_time::Timer,
{
    type Error = R::Error;

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a {
        self.dma.service(hardware)
    }

    fn work_counters(&self) -> DatapathRxWorkCounters {
        self.dma.work_counters()
    }

    async fn service_turn<'a>(
        &'a mut self,
        hardware: &'a mut H,
        _network_rx: &'a mut dyn DatapathNetworkRxSet,
        context: DatapathRxServiceContext,
    ) -> Result<DatapathRxProgress, Self::Error> {
        let mut fused_turn = FusedRxTurn::from_context(context, SLOTS);
        let before_dma = if self.protocol.has_ready_work() {
            let protocol_started =
                crate::diagnostics::core0_rx_performance::Core0PerformanceSample::read();
            crate::diagnostics::core0_rx_cycles::CORE0_RX_CYCLES
                .begin_protocol_poll(protocol_started.cycles());
            crate::diagnostics::core0_rx_performance::CORE0_PERFORMANCE
                .begin_protocol_poll(protocol_started);
            let turn = self
                .protocol
                .service_bounded(fused_turn.remaining_protocol_frames())
                .await;
            crate::diagnostics::core0_rx_performance::CORE0_PERFORMANCE.record_protocol_paths(
                turn.direct_frames.value(),
                turn.asynchronous_frames.value(),
            );
            {
                let protocol_ended =
                    crate::diagnostics::core0_rx_performance::Core0PerformanceSample::read();
                crate::diagnostics::core0_rx_cycles::CORE0_RX_CYCLES
                    .end_protocol_poll(protocol_ended.cycles());
                crate::diagnostics::core0_rx_performance::CORE0_PERFORMANCE
                    .end_protocol_poll(protocol_ended);
            }
            turn
        } else {
            Default::default()
        };
        self.serviced_frames = self
            .serviced_frames
            .saturating_add(before_dma.consumed_frames as u64);
        fused_turn.observe_protocol(before_dma.consumed_frames, before_dma.work_remaining);

        // Refill once even when the protocol budget was consumed by an
        // existing backlog. The leases just released above are the exact
        // credits needed to keep the hardware ring from becoming the only
        // remaining ingress reserve.
        let dma_progress = self.dma.service(hardware).await?;
        fused_turn.observe_dma(dma_progress);

        let remaining = fused_turn.remaining_protocol_frames();
        let after_dma = if remaining == 0 || !self.protocol.has_ready_work() {
            Default::default()
        } else {
            let protocol_started =
                crate::diagnostics::core0_rx_performance::Core0PerformanceSample::read();
            crate::diagnostics::core0_rx_cycles::CORE0_RX_CYCLES
                .begin_protocol_poll(protocol_started.cycles());
            crate::diagnostics::core0_rx_performance::CORE0_PERFORMANCE
                .begin_protocol_poll(protocol_started);
            let turn = self.protocol.service_bounded(remaining).await;
            crate::diagnostics::core0_rx_performance::CORE0_PERFORMANCE.record_protocol_paths(
                turn.direct_frames.value(),
                turn.asynchronous_frames.value(),
            );
            {
                let protocol_ended =
                    crate::diagnostics::core0_rx_performance::Core0PerformanceSample::read();
                crate::diagnostics::core0_rx_cycles::CORE0_RX_CYCLES
                    .end_protocol_poll(protocol_ended.cycles());
                crate::diagnostics::core0_rx_performance::CORE0_PERFORMANCE
                    .end_protocol_poll(protocol_ended);
            }
            turn
        };
        self.serviced_frames = self
            .serviced_frames
            .saturating_add(after_dma.consumed_frames as u64);
        fused_turn.observe_protocol(after_dma.consumed_frames, after_dma.work_remaining);

        Ok(fused_turn.finish(self.protocol.has_ready_work()))
    }

    fn service_turn_during_tx<'a>(
        &'a mut self,
        hardware: &'a mut H,
        network_rx: &'a mut dyn DatapathNetworkRxSet,
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a {
        self.service_turn(
            hardware,
            network_rx,
            DatapathRxServiceContext {
                maximum_protocol_frames: Some(ACTIVE_TX_PROTOCOL_QUANTUM_FRAMES),
            },
        )
    }

    fn has_work(&self) -> bool {
        self.protocol.has_ready_work()
    }

    fn serviced_frames(&self) -> u64 {
        self.serviced_frames
    }

    fn wait_ready(&mut self) -> impl Future<Output = ()> + '_ {
        self.protocol.wait_ready()
    }
}
