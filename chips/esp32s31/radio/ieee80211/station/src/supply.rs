//! The network's transmit memory: the pool buffers the port does not hold.
//! The network stack writes each frame into one where the station's data
//! machine, or the access point's runner, reads it, and the MAC writes the
//! MPDU header in front of the payload, in place; the radio publishes that
//! same buffer, which comes back to the network once its frame ended.
/// A pool buffer's octets.
const TX: usize = crate::FRAME_CAPACITY;
use oer_chips_esp32s31_radio_ieee80211_backend::{
    mac::tx::TxBacking,
    ordinary_tx::{TX_CCMP_MIC_SIZE, TX_FCS_SIZE, TX_METADATA_SIZE},
    port::{TX_BUFFERS, TxPool},
};
use oer_portable_memory::{FrameLen, PinnedDmaTxNetworkLease};
use oer_protocols_ieee80211_core_roles_sta::data::{
    MAX_ETHERNET_PAYLOAD_LEN, MSDU_ETHERNET_OFFSET, MSDU_PAYLOAD_OFFSET,
};
use oer_protocols_ieee80211_runtime::station::{self as runtime, TxSupply};
use oer_protocols_network_port::{EthernetFrame, TxStorage};

/// The pool buffers the port holds itself, at the station's management
/// places; the network holds the rest.
pub(crate) const PORT_BUFFERS: u8 = 4;
/// The pool buffers the network holds.
pub const NETWORK_BUFFERS: usize = TX_BUFFERS - PORT_BUFFERS as usize;
const _: () = {
    // Every network buffer the actor holds at a stop has its place home.
    if NETWORK_BUFFERS > oer_protocols_ieee80211_runtime::datapath::NETWORK_MEMORY_HELD {
        panic!("the actor's queues and returns hold every network buffer");
    }
};
/// Where a frame starts in a pool buffer: behind the radio's metadata,
/// where the data machine reads its Ethernet header.
const ETHERNET_AT: usize = TX_METADATA_SIZE + MSDU_ETHERNET_OFFSET;
/// Where a frame's first octet lies in a 32-bit word: a copy into a transmit
/// buffer from memory of the same offset moves whole words.
pub const FRAME_WORD_OFFSET: usize = ETHERNET_AT % 4;
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

impl TxSlot {
    /// The buffer's place among the network's, below [`NETWORK_BUFFERS`]:
    /// its pool index past the port's buffers. It names the frame the buffer
    /// holds until the station gives it back, so a policy keeps that frame
    /// at this place. A buffer of the port's, which the network never holds,
    /// would wrap past every place, which a policy refuses.
    pub const fn place(&self) -> u8 {
        self.0.index().wrapping_sub(PORT_BUFFERS)
    }
}

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
    /// Every pool buffer from the place `port` on, the ones a port's bank
    /// of `port` buffers does not hold, for the network to write: the
    /// station's from [`PORT_BUFFERS`], an access point's from its bank's
    /// size, which is at least that.
    ///
    /// # Errors
    ///
    /// A buffer the pool does not give to a writer: the buffers taken so
    /// far are released.
    pub(crate) fn claim(
        pool: &'static TxPool<TX>,
        port: u8,
    ) -> Result<Self, oer_portable_memory::PoolError> {
        let mut slots = heapless::Vec::new();
        for index in port..=u8::MAX {
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

    /// The publisher and every buffer, in the storage that holds them.
    pub(crate) fn into_owned(self) -> (TxPublisher, heapless::Vec<TxSlot, NETWORK_BUFFERS>) {
        (self.publisher, self.slots)
    }

    /// The memory again, from the parts [`Self::into_owned`] gave.
    pub(crate) fn from_owned(
        publisher: TxPublisher,
        slots: heapless::Vec<TxSlot, NETWORK_BUFFERS>,
    ) -> Self {
        Self { publisher, slots }
    }

    /// The publisher and the buffers, last built first.
    pub fn into_parts(self) -> (TxPublisher, impl Iterator<Item = TxSlot>) {
        let mut slots = self.slots;
        (self.publisher, core::iter::from_fn(move || slots.pop()))
    }
}

/// The pool as the network's own transmit end holds it
/// ([`runtime::LocalTx`]): a frame is published from the buffer it was
/// written in, from that buffer's start.
impl runtime::TxPool for TxPublisher {
    type Slot = TxSlot;
    type Memory = TxMemory;
    fn place(slot: &TxSlot) -> u8 {
        slot.place()
    }
    fn publish(
        &mut self,
        slot: TxSlot,
        length: usize,
    ) -> Result<TxMemory, runtime::PoolRefused<TxSlot>> {
        TxPublisher::publish(*self, slot, 0, length).map_err(|refused| match refused {
            TxPublishRefused::Range(slot) => runtime::PoolRefused::Range(slot),
            TxPublishRefused::Lost => runtime::PoolRefused::Lost,
        })
    }
    fn slot(&mut self, memory: TxMemory) -> TxSlot {
        TxPublisher::slot(*self, memory)
    }
}

/// The network end's transmit memory, built from [`NetworkTx`] at the
/// station's start, over the network stack's packets `Packet`.
pub trait NetworkTxSupply: TxSupply<Self::Packet, Memory = TxMemory> + 'static {
    /// The network stack's packet its transmissions hand the station.
    type Packet: EthernetFrame + 'static;
    fn new(tx: NetworkTx) -> Self;
}

/// The network's transmit end a composition takes, built at the station's
/// start over the network's transmit queues and [`NetworkTx`].
pub trait NetworkTxEnd<N: crate::Notifies>: 'static {
    /// The transmit end the station's start hands the network.
    type End;
    /// The end over `queues` and `memory`; an end that holds the network's
    /// buffers keeps them in `state`, the station's place for them.
    fn end(
        queues: crate::TxQueues<'static, N, TxMemory>,
        memory: NetworkTx,
        state: &'static mut LocalTxState,
    ) -> Self::End;
}

/// The place the network's own transmit end keeps its buffers in.
pub type LocalTxState = runtime::LocalTxState<TxSlot, NETWORK_BUFFERS>;

/// The network's transmit end over the memory `S`, which takes the network
/// stack's packets and puts each in a buffer.
pub struct SupplyEnd<S>(core::marker::PhantomData<S>);

impl<N: crate::Notifies, S: NetworkTxSupply> NetworkTxEnd<N> for SupplyEnd<S> {
    type End = crate::NetworkTransmit<'static, N, S::Packet, S>;
    /// The memory `S` holds the buffers: the place stays unused.
    fn end(
        queues: crate::TxQueues<'static, N, TxMemory>,
        memory: NetworkTx,
        _state: &'static mut LocalTxState,
    ) -> Self::End {
        crate::NetworkTransmit::over(queues, S::new(memory))
    }
}

/// The network's transmit end that owns the network's buffers: the network
/// stack writes each frame in the buffer it goes to the station in.
pub struct LocalEnd;

/// The transmit end [`LocalEnd`] builds.
pub type LocalTransmit<'a, N> = runtime::LocalTx<
    'a,
    TxPublisher,
    NETWORK_BUFFERS,
    { crate::TX_DEPTH },
    <N as crate::Notifies>::Channels,
>;

impl<N: crate::Notifies> NetworkTxEnd<N> for LocalEnd {
    type End = LocalTransmit<'static, N>;
    fn end(
        queues: crate::TxQueues<'static, N, TxMemory>,
        memory: NetworkTx,
        state: &'static mut LocalTxState,
    ) -> Self::End {
        let (publisher, slots) = memory.into_owned();
        runtime::LocalTx::new(
            queues.transmit,
            queues.returned,
            publisher,
            state,
            slots,
            queues.state.network_link(),
        )
    }
}

#[cfg(test)]
mod tests;
