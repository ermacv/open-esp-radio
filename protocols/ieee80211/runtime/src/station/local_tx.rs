//! The network's end of the station's transmit path, as one owner: the
//! network's free buffers, the run each holds from its last frame, the queue
//! a frame goes to the station by, and the buffers the station gives back
//! with their frames' ends. All of it lives on the network's executor, so
//! nothing of it is lent or checked twice.

use core::task::{Context, Poll};

use oer_portable_memory::{SlotReceiver, SlotSender};
use oer_protocols_bridges_ieee80211_ethernet::qos::station_user_priority;
use oer_protocols_ieee80211_core_roles_sta::data::{MAX_ETHERNET_PAYLOAD_LEN, WmmUserPriority};
use oer_protocols_network_port::{
    FrameMeta, Held, LinkState, OwnedTransmit, PortRefusal, Slot, TxStorage, Unpublished,
};

use super::{Completions, Notifies, Outgoing, StationKey, TxEnded};
use crate::egress::RadioNotice;

/// An Ethernet header's octets: destination, source and EtherType
/// (IEEE 802.3, clause 3.1.1).
const ETHERNET_HEADER: usize = 14;

/// The network's transmit memory as the platform holds it: the buffers a
/// frame is written in, and the publication that hands one to the station.
pub trait TxPool {
    /// One buffer the network holds, its octets from where a frame starts.
    type Slot: TxStorage;
    /// The memory the station holds for one MSDU.
    type Memory;
    /// The buffer's place among the network's: it names the frame the
    /// buffer holds until the station gives it back.
    fn place(slot: &Self::Slot) -> u8;
    /// The frame of `length` octets `slot` holds, published for the station.
    ///
    /// # Errors
    /// [`PoolRefused`]: a frame past the buffer keeps it; a pool that
    /// refuses the published buffer loses it.
    fn publish(
        &mut self,
        slot: Self::Slot,
        length: usize,
    ) -> Result<Self::Memory, PoolRefused<Self::Slot>>;
    /// The memory the station gave back, the network's to write again.
    fn slot(&mut self, memory: Self::Memory) -> Self::Slot;
}

/// A buffer [`TxPool::publish`] did not publish.
pub enum PoolRefused<S> {
    /// The frame reaches past the buffer; the buffer stays the network's.
    Range(S),
    /// The pool refused the buffer it published to the station: the buffer
    /// left the network's reach, quarantined.
    Lost,
}

/// The right to one frame: a free buffer, taken with the room to hand it
/// on. Only this owner sends to the queue and the station only empties it,
/// so the room a right saw stays until the right is used.
struct Right<S> {
    slot: S,
    place: u8,
}

/// The network's end of the station's transmit path over the pool `P`, of
/// `SLOTS` buffers and a queue of `TX` frames.
pub struct LocalTx<'c, P: TxPool, const SLOTS: usize, const TX: usize, N: Notifies> {
    /// The frames for the station, each in the network's memory, where the
    /// station writes its MPDU header in place.
    transmit: SlotSender<'c, Option<Outgoing<P::Memory>>, TX, N::Station, N::Network>,
    /// The memory the station gave back.
    returned: SlotReceiver<'c, Option<TxEnded<P::Memory>>, TX, N::Network, N::Station>,
    pool: P,
    /// The buffers no frame holds.
    free: heapless::Vec<P::Slot, SLOTS>,
    /// What the station gave back, each with its frame's end, until its
    /// notice is handed on: only then is the buffer free again.
    ended: Completions<P::Slot, u8, SLOTS>,
    /// The run of one octet each buffer holds from the last frame written
    /// into it, by its place.
    held: [Option<Held>; SLOTS],
    link: &'c super::NetworkLink<N::NetworkWaker>,
}

impl<'c, P: TxPool, const SLOTS: usize, const TX: usize, N: Notifies> LocalTx<'c, P, SLOTS, TX, N> {
    /// The network's end over the station's transmit channels, `pool` and
    /// its free buffers `slots`, and `state`'s link. A buffer past `SLOTS`
    /// is not the network's to hold, and stays out.
    pub fn new(
        transmit: SlotSender<'c, Option<Outgoing<P::Memory>>, TX, N::Station, N::Network>,
        returned: SlotReceiver<'c, Option<TxEnded<P::Memory>>, TX, N::Network, N::Station>,
        pool: P,
        slots: impl IntoIterator<Item = P::Slot>,
        link: &'c super::NetworkLink<N::NetworkWaker>,
    ) -> Self {
        let mut free = heapless::Vec::new();
        for slot in slots {
            if free.push(slot).is_err() {
                break;
            }
        }
        Self {
            transmit,
            returned,
            pool,
            free,
            ended: Completions::new(),
            held: [None; SLOTS],
            link,
        }
    }

    /// The buffers the network holds: free ones, and those the station gave
    /// back whose end waits to be handed on.
    pub fn buffers(&self) -> usize {
        self.free.len().saturating_add(self.ended.buffers())
    }

    /// Take back every memory the station returned: each waits with its
    /// frame's end until that end is handed on.
    fn take_returned(&mut self) {
        while let Ok(Some(ended)) = self.returned.try_receive_with(Option::take) {
            let slot = self.pool.slot(ended.memory);
            let place = P::place(&slot);
            // One end a buffer, and this holds an end for every buffer: no
            // end is refused. Were one, its buffer is free again at once.
            if let Err(slot) = self.ended.returned(slot, place, ended.end) {
                self.give(slot);
            }
        }
    }

    /// `slot` is free again.
    fn give(&mut self, slot: P::Slot) {
        // The vector holds every buffer the network does.
        let _more_than_the_pool_holds = self.free.push(slot);
    }

    /// The right to one frame, or why there is none.
    fn right(&mut self) -> Result<Right<P::Slot>, PortRefusal> {
        if self.transmit.free_capacity() == 0 {
            return Err(PortRefusal::NoRoom);
        }
        let slot = self.free.pop().ok_or(PortRefusal::NoRoom)?;
        let place = P::place(&slot);
        Ok(Right { slot, place })
    }
}

impl<P: TxPool, const SLOTS: usize, const TX: usize, N: Notifies> OwnedTransmit
    for LocalTx<'_, P, SLOTS, TX, N>
{
    /// The buffer's place, which names its frame until its end.
    type Ticket = u8;
    type Notice = RadioNotice<u8, StationKey>;

    fn publish<E>(
        &mut self,
        meta: FrameMeta,
        fill: impl FnOnce(&mut Slot<'_>) -> Result<(), E>,
    ) -> Result<u8, Unpublished<PortRefusal, E>> {
        if self.link.snapshot().state == LinkState::Down {
            return Err(Unpublished::Port(PortRefusal::LinkDown));
        }
        // A frame the station cannot carry is refused before any buffer.
        let length = meta
            .length
            .checked_sub(ETHERNET_HEADER)
            .filter(|payload| *payload <= MAX_ETHERNET_PAYLOAD_LEN)
            .and_then(|_| u16::try_from(meta.length).ok())
            .ok_or(Unpublished::Port(PortRefusal::BadFrame))?;
        let Right { mut slot, place } = match self.right() {
            Ok(right) => right,
            Err(refusal) => {
                // What the station returned meanwhile becomes the next
                // notices, which free buffers.
                self.take_returned();
                return Err(Unpublished::Port(refusal));
            }
        };
        let held = self.held.get_mut(usize::from(place));
        // A frame longer than a buffer is the port's refusal, before any
        // fill; a fill's own failure stays the fill's.
        let written = match (slot.bytes_mut().get_mut(..meta.length), held) {
            (Some(bytes), Some(held)) => {
                let mut room = Slot::new(bytes, held.take());
                let filled = fill(&mut room).map_err(Unpublished::Fill);
                // Whatever the fill did, the buffer holds what the room says.
                *held = room.held();
                filled
            }
            (Some(bytes), None) => fill(&mut Slot::new(bytes, None)).map_err(Unpublished::Fill),
            (None, _) => Err(Unpublished::Port(PortRefusal::BadFrame)),
        };
        if let Err(unpublished) = written {
            self.give(slot);
            return Err(unpublished);
        }
        let memory = match self.pool.publish(slot, meta.length) {
            Ok(memory) => memory,
            Err(PoolRefused::Range(slot)) => {
                self.give(slot);
                return Err(Unpublished::Port(PortRefusal::BadFrame));
            }
            Err(PoolRefused::Lost) => {
                // The station took it and lost its buffer: its notice says
                // so, and the buffer stays out of reach for good. This
                // holds an end for every buffer, so none is refused.
                let _held = self.ended.lost(place);
                return Ok(place);
            }
        };
        // The frame's marking requests a priority; the station decides the
        // final one. A frame that is not IP requests best effort.
        let priority = meta
            .dscp
            .map_or(WmmUserPriority::UP0, station_user_priority);
        let mut outgoing = Some(Outgoing {
            memory,
            length,
            priority,
        });
        let _sent = self.transmit.try_send_held_with(|place| {
            *place = outgoing.take();
            Ok::<_, core::convert::Infallible>(())
        });
        if let Some(outgoing) = outgoing {
            // The right saw room: a broken queue keeps the memory
            // quarantined rather than reused under the station.
            let _quarantined = core::mem::ManuallyDrop::new(outgoing);
        }
        Ok(place)
    }

    fn take_notice(&mut self) -> Option<Self::Notice> {
        let link = self.link.snapshot();
        let taken = match self.ended.take_notice(link) {
            Some(taken) => taken,
            None => {
                self.take_returned();
                self.ended.take_notice(link)?
            }
        };
        let (notice, slot) = taken;
        if let Some(slot) = slot {
            self.give(slot);
        }
        Some(notice)
    }

    fn notice_waiting(&self) -> bool {
        self.ended.waiting(self.link.snapshot())
    }

    fn poll_room(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        loop {
            self.take_returned();
            if core::pin::pin!(self.returned.ready()).poll(cx).is_pending() {
                break;
            }
        }
        if self.free.is_empty() {
            // A buffer frees only when its end is handed on, which the
            // returned memory above wakes for.
            return Poll::Pending;
        }
        core::pin::pin!(self.transmit.ready()).poll(cx)
    }

    fn flush(&mut self) {
        self.transmit.flush();
    }

    fn reclaim(&mut self) {
        self.take_returned();
    }
}

#[cfg(test)]
mod tests;
