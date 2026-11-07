//! The network's transmit memory: the pool buffers the port does not hold.
//! The network stack writes each frame into one where the station's data
//! machine reads it, and the station writes the MPDU header in front of the
//! payload, in place; the radio publishes that same buffer.
/// A pool buffer's octets.
const TX: usize = crate::FRAME_CAPACITY;
use oer_chips_esp32s31_radio_ieee80211_driver::{
    mac::tx::TxBacking,
    ordinary_tx::{TX_CCMP_MIC_SIZE, TX_FCS_SIZE, TX_METADATA_SIZE},
    port::{TX_BUFFERS, TxPool},
};
use oer_portable_memory::{FrameLen, PinnedDmaTxNetworkLease};
use oer_protocols_ieee80211_core_roles_sta::data::{
    MAX_ETHERNET_PAYLOAD_LEN, MSDU_ETHERNET_OFFSET, MSDU_PAYLOAD_OFFSET,
};
use oer_protocols_ieee80211_runtime::station::TxSupply;
use oer_protocols_network_port::{EthernetFrame, TxStorage};

/// The pool buffers the port holds itself, at the station's management
/// places; the network holds the rest.
pub(crate) const PORT_BUFFERS: u8 = 4;
/// The pool buffers the network holds.
pub const NETWORK_BUFFERS: usize = TX_BUFFERS - PORT_BUFFERS as usize;
/// Where a frame starts in a pool buffer: behind the radio's metadata,
/// where the data machine reads its Ethernet header.
const ETHERNET_AT: usize = TX_METADATA_SIZE + MSDU_ETHERNET_OFFSET;
const _: () = {
    // The longest MPDU, its CCMP MIC and FCS fit a pool buffer.
    if TX_METADATA_SIZE
        + MSDU_PAYLOAD_OFFSET
        + MAX_ETHERNET_PAYLOAD_LEN
        + TX_CCMP_MIC_SIZE
        + TX_FCS_SIZE
        > TX
    {
        panic!("a pool buffer holds the longest data MPDU written in place");
    }
    if PORT_BUFFERS as usize > TX_BUFFERS {
        panic!("the port holds at most the pool");
    }
};

type NetworkLease = PinnedDmaTxNetworkLease<'static, TX, 0, 0>;

/// The memory the station gives back for the network's next frames.
pub type TxMemory = TxBacking<TX>;

/// One pool buffer the network holds: its bytes from where a frame starts.
pub struct TxSlot(NetworkLease);

impl TxStorage for TxSlot {
    fn bytes_mut(&mut self) -> &mut [u8] {
        // ETHERNET_AT lies inside the buffer (the check above).
        self.0
            .bytes_mut()
            .get_mut(ETHERNET_AT..)
            .unwrap_or_default()
    }
}

/// A frame [`TxPublisher::publish`] did not publish.
pub enum TxPublishRefused {
    /// The frame reaches past the buffer; the buffer stays the network's.
    Range(TxSlot),
    /// The pool refused the buffer it published to the radio: the buffer
    /// left the network's reach, quarantined.
    Lost,
}

/// Publishes the network's buffers to the station, and takes them back.
#[derive(Clone, Copy)]
pub struct TxPublisher {
    pool: &'static TxPool<TX>,
}

impl TxPublisher {
    /// The frame `slot` holds, `len` octets from `start` in its
    /// [`TxStorage::bytes_mut`], moved to their start when it is not
    /// there, published for the station.
    ///
    /// # Errors
    ///
    /// A frame past the buffer keeps it; a pool that refuses the published
    /// buffer loses it ([`TxPublishRefused`]).
    pub fn publish(
        self,
        mut slot: TxSlot,
        start: usize,
        len: usize,
    ) -> Result<TxMemory, TxPublishRefused> {
        let end = start.checked_add(len);
        let length = ETHERNET_AT
            .checked_add(len)
            .and_then(|length| FrameLen::<TX>::new(length).ok());
        let (Some(end), Some(length)) = (end, length) else {
            return Err(TxPublishRefused::Range(slot));
        };
        let bytes = slot.bytes_mut();
        if end > bytes.len() {
            return Err(TxPublishRefused::Range(slot));
        }
        if start != 0 {
            // A frame the stack wrote further in: the data machine reads it
            // at the start.
            bytes.copy_within(start..end, 0);
        }
        let index = slot.0.publish_written(length);
        // The published buffer is the radio's from here: claimed at once,
        // it never waits unclaimed.
        self.pool
            .claim_radio(index)
            .map_err(|_| TxPublishRefused::Lost)
    }

    /// The memory the station gave back, the network's to write again.
    pub fn slot(self, memory: TxMemory) -> TxSlot {
        TxSlot(memory.into_inner().into_network())
    }
}

/// The network's transmit memory as the station's start hands it over:
/// every pool buffer the port does not hold, and their publisher.
pub struct NetworkTx {
    publisher: TxPublisher,
    slots: heapless::Vec<TxSlot, NETWORK_BUFFERS>,
}

impl NetworkTx {
    /// Every pool buffer the port does not hold, for the network to write.
    ///
    /// # Errors
    ///
    /// A buffer the pool does not give to a writer: the buffers taken so
    /// far are released.
    pub(crate) fn claim(pool: &'static TxPool<TX>) -> Result<Self, oer_portable_memory::PoolError> {
        let mut slots = heapless::Vec::new();
        for index in PORT_BUFFERS..=u8::MAX {
            if usize::from(index) >= TX_BUFFERS {
                break;
            }
            let lease = pool.claim_network(index)?;
            // NETWORK_BUFFERS holds every index from PORT_BUFFERS on.
            if let Err(slot) = slots.push(TxSlot(lease)) {
                drop(slot);
                break;
            }
        }
        Ok(Self {
            publisher: TxPublisher { pool },
            slots,
        })
    }

    /// The publisher and the buffers, last built first.
    pub fn into_parts(self) -> (TxPublisher, impl Iterator<Item = TxSlot>) {
        let mut slots = self.slots;
        (self.publisher, core::iter::from_fn(move || slots.pop()))
    }
}

/// The network end's transmit memory, built from [`NetworkTx`] at the
/// station's start, over the network stack's packets `Packet`.
pub trait NetworkTxSupply: TxSupply<Self::Packet, Memory = TxMemory> + 'static {
    /// The network stack's packet its transmissions hand the station.
    type Packet: EthernetFrame + 'static;
    fn new(tx: NetworkTx) -> Self;
}

#[cfg(test)]
mod tests;
