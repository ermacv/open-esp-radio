//! The facade's permanent DMA backing. Claim transfers each placed owner once.
use crate::chip::{B, N, Q, RadioExecutor, S, TX};
use oer_esp32s31_hal::{
    ieee80211::dma::{tx_ampdu_storage::AmpduDmaStorage, tx_storage::TxDmaStorage},
    memory::dma_static::Placed,
};
use oer_esp32s31_ieee80211_driver::port::{TX_AMPDU_SUBFRAMES, TX_BUFFERS};
use oer_esp32s31_ieee80211_driver::rx::arena::WifiRxStatic;
use oer_memory::PinnedDmaTxPool;
type Rx = WifiRxStatic<N, B, S, Q, RadioExecutor>;
/// Every buffer an MPDU or a subframe is written into.
pub(crate) type TxPool = PinnedDmaTxPool<TX, 0, 0, TX_BUFFERS>;
/// One ordinary descriptor; its buffer comes from the pool.
pub(crate) type TxDescriptor = TxDmaStorage;
/// One aggregate's descriptor chain over pool buffers.
pub(crate) type TxChain = AmpduDmaStorage<TX_AMPDU_SUBFRAMES, 0>;
oer_esp32s31_hal::dma_static! {
    static RX: Rx = zeroed in ".dma.bss.wifi_rx";
    static TX_POOL: TxPool = zeroed in ".dma.bss.wifi_tx_pool";
    static TX0: TxDescriptor = zeroed in ".dma.bss.wifi_tx0";
    static TX1: TxDescriptor = zeroed in ".dma.bss.wifi_tx1";
    static TX2: TxDescriptor = zeroed in ".dma.bss.wifi_tx2";
    static TX3: TxDescriptor = zeroed in ".dma.bss.wifi_tx3";
    static TX4: TxDescriptor = zeroed in ".dma.bss.wifi_tx4";
    static TX_CHAIN0: TxChain = zeroed in ".dma.bss.wifi_tx_chain0";
    static TX_CHAIN1: TxChain = zeroed in ".dma.bss.wifi_tx_chain1";
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DmaError {
    RxClaimed,
    TxClaimed,
}
/// The transmit backing: the pool, the transmitter's descriptor and one per
/// EDCA queue, and the two aggregate chains.
pub(crate) struct TxDma {
    pub(crate) pool: Placed<TxPool>,
    pub(crate) descriptors: [Placed<TxDescriptor>; 5],
    pub(crate) chains: [Placed<TxChain>; 2],
}
/// An affine claim of the same RX ring and TX backing used by station
/// images. Dropping a claim never makes permanent storage reusable.
pub struct DmaResources {
    pub(crate) rx: Placed<Rx>,
    pub(crate) tx: TxDma,
}
impl DmaResources {
    /// Claim before installation; no hardware can reach this backing yet.
    /// A partial refusal retires its unpublished claims until reset.
    pub fn claim() -> Result<Self, DmaError> {
        Ok(Self {
            rx: RX.take().ok_or(DmaError::RxClaimed)?,
            tx: TxDma {
                pool: TX_POOL.take().ok_or(DmaError::TxClaimed)?,
                descriptors: [
                    TX0.take().ok_or(DmaError::TxClaimed)?,
                    TX1.take().ok_or(DmaError::TxClaimed)?,
                    TX2.take().ok_or(DmaError::TxClaimed)?,
                    TX3.take().ok_or(DmaError::TxClaimed)?,
                    TX4.take().ok_or(DmaError::TxClaimed)?,
                ],
                chains: [
                    TX_CHAIN0.take().ok_or(DmaError::TxClaimed)?,
                    TX_CHAIN1.take().ok_or(DmaError::TxClaimed)?,
                ],
            },
        })
    }
}
