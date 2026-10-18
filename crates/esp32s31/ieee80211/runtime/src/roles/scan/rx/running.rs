#![expect(
    clippy::result_large_err,
    reason = "no-alloc running-scan transitions retain the exact DMA owner on failure"
)]

use super::*;
use crate::datapath::rx::dma::StoppedReceive;
/// Running-scan RX owner which retains every connected-epoch resource.
///
/// A connected teardown returns more than a descriptor ring: the staging
/// pool, queue sender, DMA observation delay and telemetry binding must survive
/// candidate refresh too. This owner changes only logical protocol ownership;
/// the physical RX frontier remains live for the complete scan.
pub struct RunningScanRx<
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
> {
    scan: ScanRx<'storage, COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    resources: RxEpochResources<
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
    RunningScanRx<
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
    /// Rebind a physically halted connected producer after a whole-radio
    /// shutdown. The first scan start drives the retained ring through its
    /// normal prepared/live lifecycle.
    pub fn from_stopped(
        stopped: StoppedReceive<
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
    ) -> Self {
        let (ring, resources) = stopped.into_epoch_parts();
        Self {
            scan: ScanRx::from_halted(ring, resources.storage()),
            resources,
        }
    }

    pub fn from_parked<P>(
        parked: StagedRxProducer<
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
        >,
    ) -> Result<
        Self,
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
        >,
    > {
        match parked.try_into_live_epoch_parts() {
            Ok((ring, resources)) => {
                let scan = ScanRx::from_live(ring, resources.storage());
                Ok(Self { scan, resources })
            }
            Err((parked, _)) => Err(parked),
        }
    }

    pub const fn phase(&self) -> RxFrontierPhase {
        self.scan.phase()
    }

    /// Validate the continuously live ring before the first scan channel.
    pub fn prepare_initial<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), RxFrontierError> {
        self.scan.prepare_next(hardware)
    }

    pub async fn start<H: RxDma>(&mut self, hardware: &mut H) -> Result<(), RxFrontierError>
    where
        D: oer_time::Timer,
    {
        if self.scan.phase() == RxFrontierPhase::Live {
            return Ok(());
        }
        walker_enable_settle(self.resources.delay()).await;
        self.scan.start(hardware)
    }

    pub fn observe_management<H, O, const RECORDS: usize>(
        &mut self,
        hardware: &mut H,
        context: &mut ScanObservationContext<'_, O, RECORDS>,
    ) -> Result<ScanRxProgress, RxFrontierError>
    where
        H: RxDma,
        O: ScanFrameObserver,
    {
        self.scan.observe_management(hardware, context)
    }

    pub fn park(&mut self) -> Result<(), RxFrontierError> {
        self.scan.park()
    }

    pub fn prepare_next_channel<H: RxDma>(
        &mut self,
        hardware: &mut H,
    ) -> Result<(), RxFrontierError> {
        self.scan.prepare_next(hardware)
    }

    pub fn into_parked(
        self,
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
        Self,
    > {
        let Self { scan, resources } = self;
        match scan.into_live() {
            Ok(ring) => Ok(resources.with_live_ring(ring)),
            Err(scan) => Err(Self { scan, resources }),
        }
    }
}
