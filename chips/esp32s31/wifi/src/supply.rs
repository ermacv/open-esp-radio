//! The network's transmit memory: the pool buffers the port does not hold.
//! The network writes each frame into one where the station's data machine
//! reads it, and the station writes the MPDU header in front of the
//! payload, in place; the radio publishes that same buffer.
/// A pool buffer's octets.
const TX: usize = crate::FRAME_CAPACITY;
use oer_chips_esp32s31_ieee80211_driver::{
    mac::tx::TxBacking,
    ordinary_tx::{TX_CCMP_MIC_SIZE, TX_FCS_SIZE, TX_METADATA_SIZE},
    port::{TX_BUFFERS, TxPool},
};
use oer_portable_memory::{FrameLen, PinnedDmaTxNetworkLease};
use oer_protocols_ieee80211_core_roles_sta::data::{
    MAX_ETHERNET_PAYLOAD_LEN, MSDU_ETHERNET_OFFSET, MSDU_PAYLOAD_OFFSET,
};
use oer_protocols_ieee80211_runtime::station::TxSupply;
use oer_protocols_network_port::{EthernetFrameView, FrameLengthError, TransmitError};

/// The pool buffers the port holds itself, at the station's management
/// places; the network holds the rest.
pub(crate) const PORT_BUFFERS: u8 = 4;
/// The pool buffers the network holds.
const NETWORK_BUFFERS: usize = TX_BUFFERS - PORT_BUFFERS as usize;
/// Where the network writes a frame in a pool buffer: behind the radio's
/// metadata, where the data machine reads it.
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

/// The network's free pool buffers, on the network's executor.
pub struct NetworkSupply {
    pool: &'static TxPool<TX>,
    free: heapless::Vec<NetworkLease, NETWORK_BUFFERS>,
}

impl NetworkSupply {
    /// Every pool buffer the port does not hold, for the network to write.
    ///
    /// # Errors
    ///
    /// A buffer the pool does not give to a writer: the buffers taken so
    /// far are released.
    pub(crate) fn claim(pool: &'static TxPool<TX>) -> Result<Self, oer_portable_memory::PoolError> {
        let mut free = heapless::Vec::new();
        for index in PORT_BUFFERS..=u8::MAX {
            if usize::from(index) >= TX_BUFFERS {
                break;
            }
            let lease = pool.claim_network(index)?;
            // NETWORK_BUFFERS holds every index from PORT_BUFFERS on.
            if let Err(lease) = free.push(lease) {
                drop(lease);
                break;
            }
        }
        Ok(Self { pool, free })
    }
}

impl TxSupply for NetworkSupply {
    type Memory = TxMemory;
    fn available(&self) -> bool {
        !self.free.is_empty()
    }
    fn write(&mut self, frame: EthernetFrameView<'_>) -> Result<TxMemory, TransmitError> {
        let too_long = TransmitError::InvalidLength(FrameLengthError::TooLong);
        let end = ETHERNET_AT
            .checked_add(14)
            .and_then(|header| header.checked_add(frame.payload.len()))
            .ok_or(too_long)?;
        let length = FrameLen::<TX>::new(end).map_err(|_| too_long)?;
        let lease = self.free.pop().ok_or(TransmitError::QueueFull)?;
        let written = lease.try_publish(length, |bytes| {
            let ethernet = bytes.get_mut(ETHERNET_AT..).ok_or(())?;
            let (destination, rest) = ethernet.split_first_chunk_mut::<6>().ok_or(())?;
            *destination = frame.destination;
            let (source, rest) = rest.split_first_chunk_mut::<6>().ok_or(())?;
            *source = frame.source;
            let (ether_type, payload) = rest.split_first_chunk_mut::<2>().ok_or(())?;
            *ether_type = frame.ether_type.to_be_bytes();
            payload
                .get_mut(..frame.payload.len())
                .ok_or(())?
                .copy_from_slice(frame.payload);
            Ok::<(), ()>(())
        });
        let index = match written {
            Ok((index, ())) => index,
            Err((lease, ())) => {
                // The length was checked against the buffer: the writer
                // keeps its free buffer.
                if self.free.push(lease).is_err() {
                    return Err(TransmitError::QueueFull);
                }
                return Err(too_long);
            }
        };
        // The published buffer is the radio's from here: claimed at once,
        // it never waits unclaimed. A refused claim leaves it published,
        // out of the network's reach.
        self.pool
            .claim_radio(index)
            .map_err(|_| TransmitError::QueueFull)
    }
    fn give(&mut self, memory: TxMemory) {
        // The free buffers hold every buffer the network has.
        let _held = self.free.push(memory.into_inner().into_network());
    }
}
