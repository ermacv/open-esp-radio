//! The facade's permanent DMA backing. Claim transfers each placed owner once.
use crate::chip::{B, N, Q, RadioExecutor, S, TX};
use oer_esp32s31_hal::{ieee80211::dma::tx_storage::TxDmaStorage, memory::dma_static::Placed};
use oer_esp32s31_ieee80211_driver::rx::arena::WifiRxStatic;
type Rx = WifiRxStatic<N, B, S, Q, RadioExecutor>;
type Tx = TxDmaStorage<TX>;
oer_esp32s31_hal::dma_static! {
    static RX: Rx = zeroed in ".dma.bss.wifi_rx";
    static TX0: Tx = zeroed in ".dma.bss.wifi_tx0";
    static TX1: Tx = zeroed in ".dma.bss.wifi_tx1";
    static TX2: Tx = zeroed in ".dma.bss.wifi_tx2";
    static TX3: Tx = zeroed in ".dma.bss.wifi_tx3";
    static TX4: Tx = zeroed in ".dma.bss.wifi_tx4";
    static TX5: Tx = zeroed in ".dma.bss.wifi_tx5";
    static TX6: Tx = zeroed in ".dma.bss.wifi_tx6";
    static TX7: Tx = zeroed in ".dma.bss.wifi_tx7";
    static TX8: Tx = zeroed in ".dma.bss.wifi_tx8";
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DmaError {
    RxClaimed,
    TxClaimed,
}
/// An affine claim of the same RX ring and nine TX allocations used by
/// station images. Dropping a claim never makes permanent storage reusable.
pub struct DmaResources {
    pub(crate) rx: Placed<Rx>,
    pub(crate) tx: [Placed<Tx>; 9],
}
impl DmaResources {
    /// Claim before installation; no hardware can reach this backing yet.
    /// A partial refusal retires its unpublished claims until reset.
    pub fn claim() -> Result<Self, DmaError> {
        Ok(Self {
            rx: RX.take().ok_or(DmaError::RxClaimed)?,
            tx: [
                TX0.take().ok_or(DmaError::TxClaimed)?,
                TX1.take().ok_or(DmaError::TxClaimed)?,
                TX2.take().ok_or(DmaError::TxClaimed)?,
                TX3.take().ok_or(DmaError::TxClaimed)?,
                TX4.take().ok_or(DmaError::TxClaimed)?,
                TX5.take().ok_or(DmaError::TxClaimed)?,
                TX6.take().ok_or(DmaError::TxClaimed)?,
                TX7.take().ok_or(DmaError::TxClaimed)?,
                TX8.take().ok_or(DmaError::TxClaimed)?,
            ],
        })
    }
}
