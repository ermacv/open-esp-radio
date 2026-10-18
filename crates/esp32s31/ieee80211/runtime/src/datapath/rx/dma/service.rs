use core::future::ready;

use super::*;

mod observation;

impl<
    'storage,
    'pool,
    'queue,
    H,
    D,
    M: RawMutex,
    const QUEUE_DEPTH: usize,
    const COUNT: usize,
    const STAGE_CAPACITY: usize,
    const STAGE_SLOTS: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    P,
> DatapathRxService<H>
    for StagedRxProducer<
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
where
    H: RxDma,
    D: oer_time::Timer,
    P: Esp32s31RxStageAdmissionPolicy,
{
    type Error = RxStageTransactionError;

    /// Service one frozen RX frontier as a synchronous transaction.
    ///
    /// This is the measured per-MPDU DMA walker, staging and publication
    /// working set. It deliberately lives in the semantic hot-text class;
    /// executor scheduling, protocol fallback and role control remain in
    /// cached external code.
    #[cfg_attr(
        target_arch = "riscv32",
        unsafe(link_section = ".hot.text.open_radio_rx_dma_service")
    )]
    #[inline(never)]
    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
    ) -> impl Future<Output = Result<DatapathRxProgress, Self::Error>> + 'a {
        ready(oer_esp32s31_ieee80211::rx::transaction::service(
            &mut self.ring,
            self.storage,
            self.pool,
            &ClockedRxPublisher {
                frames: &self.frames,
                clock: &self.delay,
            },
            &self.admission,
            oer_esp32s31_ieee80211::rx::transaction::Counters {
                descriptors: &mut self.serviced_descriptors,
                units: &mut self.serviced_units,
                bytes: &mut self.serviced_bytes,
            },
            hardware,
            observation::Context::new(
                #[cfg(any(feature = "diagnostics", test))]
                self.pipeline_observer,
            ),
        ))
    }

    fn work_counters(&self) -> DatapathRxWorkCounters {
        DatapathRxWorkCounters {
            completed_units: self.serviced_units,
            staged_bytes: self.serviced_bytes,
        }
    }
}
