use super::*;

/// Coherent mutable radio resources used by one finite attempt.
pub struct StaAttemptRadio<
    'hardware,
    'transmit,
    'storage,
    H,
    C,
    D,
    T,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
> {
    pub hardware: &'hardware mut H,
    pub channel: C,
    pub receive: ReceiveFrontier<'storage, COUNT, DMA_BUFFER_SIZE>,
    pub rx_storage: &'storage ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
    pub transmit: &'transmit mut T,
    /// The timer the RX walker settles on.
    pub timer: D,
}

impl<
    'hardware,
    'transmit,
    'storage,
    H,
    C,
    D,
    T,
    const COUNT: usize,
    const DMA_BUFFER_SIZE: usize,
    const DMA_STORAGE_SIZE: usize,
>
    StaAttemptRadio<
        'hardware,
        'transmit,
        'storage,
        H,
        C,
        D,
        T,
        COUNT,
        DMA_BUFFER_SIZE,
        DMA_STORAGE_SIZE,
    >
{
    pub const fn new(
        hardware: &'hardware mut H,
        channel: C,
        receive: ReceiveFrontier<'storage, COUNT, DMA_BUFFER_SIZE>,
        rx_storage: &'storage ReceiveDmaStorage<COUNT, DMA_BUFFER_SIZE, DMA_STORAGE_SIZE>,
        transmit: &'transmit mut T,
        timer: D,
    ) -> Self {
        Self {
            hardware,
            channel,
            receive,
            rx_storage,
            transmit,
            timer,
        }
    }
}

/// Allocation-free frame scratch used by management and EAPOL parsing.
pub struct StaAttemptStorage<'scratch> {
    pub frame: &'scratch mut [u8],
}

impl<'scratch> StaAttemptStorage<'scratch> {
    pub const fn new(frame: &'scratch mut [u8]) -> Self {
        Self { frame }
    }
}
