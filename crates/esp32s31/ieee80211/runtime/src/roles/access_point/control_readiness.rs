impl<
    'storage,
    'beacon,
    'slot,
    R,
    C,
    P,
    E,
    T,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
    const TX_BUFFER_SIZE: usize,
>
    AccessPointControl<
        'storage,
        'beacon,
        'slot,
        R,
        C,
        P,
        E,
        T,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
        TX_BUFFER_SIZE,
    >
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
{
    pub(super) fn rx_work_due(&self, now: oer_time::Instant) -> bool
    where
        C: AccessPointRxProtocolConsumer,
    {
        self.protocol_rx.queued_frames() != 0
            || !self.protocol_actions.is_empty()
            || self.rx_batch_pending()
            || self.rx_reorder.has_pending_release()
            || self
                .rx_reorder
                .next_deadline()
                .is_some_and(|deadline| deadline <= now)
    }

    pub(super) fn queued_rx_frames(&self) -> usize
    where
        C: AccessPointRxProtocolConsumer,
    {
        self.protocol_rx.queued_frames()
    }
}
