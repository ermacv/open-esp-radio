//! The network's transmit memory over the radio's transmit buffers, in the
//! IP stack's form: the buffers the port does not hold, parked as the
//! stack's packet slots, so a packet the stack writes is already in the
//! buffer it goes to the MAC in. The station's IP service and the access
//! point's take the same form; one radio runs either at a time, and the
//! slots serve the one that runs.

use super::*;
use oer_portable_embassy_net::ethernet::{ParkedTx, TxSlots, TxWriteRefused};
use oer_protocols_ieee80211_runtime::station::{TxRefused, TxSupply};

/// The IP stack's packet owner a transmission hands the MAC.
pub(super) type TxPacket = oer_portable_embassy_net::OwnedNetworkTxFrame;

/// The packet slots over the network's transmit buffers, but the spare the
/// network keeps for copies: the IP stack's general allocator.
pub(super) static TX_SLOTS: TxSlots<wifi::TxSlot, { wifi::NETWORK_BUFFERS - 1 }> = TxSlots::new();

/// The network end's transmit memory: the network's transmit buffers parked
/// in [`TX_SLOTS`], where the IP stack writes its packets, and the spare
/// that a packet from elsewhere is copied into. A packet goes to the MAC in
/// the buffer it was written in.
pub struct WifiTx {
    publisher: wifi::TxPublisher,
    parked: ParkedTx<wifi::TxSlot, { wifi::NETWORK_BUFFERS - 1 }>,
}

impl wifi::NetworkTxSupply for WifiTx {
    type Packet = TxPacket;
    fn new(tx: wifi::NetworkTx) -> Self {
        let (publisher, slots) = tx.into_parts();
        let mut parked = ParkedTx::new(&TX_SLOTS);
        for slot in slots {
            parked.give(slot);
        }
        Self { publisher, parked }
    }
}

impl TxSupply<TxPacket> for WifiTx {
    type Memory = wifi::TxMemory;
    const MEMORIES: usize = ParkedTx::<wifi::TxSlot, { wifi::NETWORK_BUFFERS - 1 }>::CAPACITY;
    fn available(&self) -> bool {
        self.parked.available()
    }
    fn write(&mut self, frame: TxPacket) -> Result<wifi::TxMemory, TxRefused<TxPacket>> {
        let written = match self.parked.write(frame) {
            Ok(written) => written,
            Err(TxWriteRefused::Kept(refused)) => return Err(TxRefused::Kept(refused)),
            Err(TxWriteRefused::Lost) => return Err(TxRefused::Lost),
        };
        self.publisher
            .publish(written.storage, written.start, written.len)
            .map_err(|refused| {
                // A packet stays inside its buffer: a frame past it ends
                // here, and the buffer is free again.
                if let wifi::TxPublishRefused::Range(slot) = refused {
                    self.parked.give(slot);
                }
                TxRefused::Lost
            })
    }
    /// The MAC returned the memory: its buffer is free again.
    fn give(&mut self, memory: wifi::TxMemory) {
        self.parked.give(self.publisher.slot(memory));
    }
}
