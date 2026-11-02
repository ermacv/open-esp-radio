//! What the library observes of its own work for an image that measures it
//! (`observe` feature): the executors' CPU, the polls of named parts, the
//! station's and the network bridge's counts, the shared RF actor's PHY
//! tracking ticks and, on the boot hart, its cycle, instruction and cache
//! counters and a PC sampler.
//!
//! The executors, the station and the RF actor report through
//! `oer_portable_trace::observe!`, which only an `oer_trace` build compiles: an image
//! with this feature declares `instrumentation = ["trace"]`.
//!
//! Each counter has one writer, the context that observes it; a counter a
//! reader on another task or hart takes whole is published under a sequence
//! number, odd while its writer writes, in atomic words only. A read that
//! sees the number change was concurrent with a write and is refused
//! ([`Concurrent`]); the async readers sample again after yielding and
//! never wait for the writer. Counts and microseconds wrap at 2^32 unless a
//! type says otherwise: a reader takes the differences of two readings.
//!
//! The types here are the library's own: an image translates them into the
//! vocabulary of whatever reports them.

pub mod cpu;
pub mod link;
pub mod parts;
pub mod tracking;

/// A reading ran while the counter's writer wrote: it holds no state the
/// writer published.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Concurrent;

/// What the network bridge did with the station's frames: the frames it
/// took, those the stack refused for want of a pool packet, of queue room
/// or for their length or a down link, and those the station lent in place
/// and the bridge copied after all.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Bridge {
    pub received: u32,
    pub refused_pool: u32,
    pub refused_queue: u32,
    pub refused_other: u32,
    pub copied: u32,
    pub tx: BridgeTx,
}

/// What the network bridge's turns did with the stack's transmissions.
///
/// Each turn moves at most one packet from the stack's queue to the
/// station. The sums are taken at each turn's start: divided by `turns`,
/// they give the mean packets the stack's queue held and the mean network
/// buffers the station held.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BridgeTx {
    /// The bridge's turns, idle ones included.
    pub turns: u32,
    /// Turns that handed a packet to the station.
    pub moved: u32,
    /// Turns whose packet waits for the station's room.
    pub held: u32,
    /// Turns whose packet the station refused for its length.
    pub dropped: u32,
    /// Turns that began with the stack's queue full: a send meanwhile is
    /// refused for the device's room.
    pub queue_full: u32,
    /// Turns that began with no buffer free for the stack: every network
    /// buffer but the spare with the station, queued or waiting. A send
    /// meanwhile is refused for a packet buffer.
    pub starved: u32,
    pub queued_sum: u32,
    pub station_sum: u32,
    /// Of the station's buffers, summed likewise by where they waited at
    /// the station's latest pass: in the queue to the station, owed by its
    /// data machine, members of aggregates out at the port, freed and not
    /// yet given back, and in the queue back to the network.
    pub incoming_sum: u32,
    pub owed_sum: u32,
    pub members_sum: u32,
    pub freed_sum: u32,
    pub returning_sum: u32,
}

/// Where the network's transmit buffers the station holds waited at its
/// latest pass ([`BridgeTx`]'s parts).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StationHeld {
    pub incoming: u8,
    pub owed: u8,
    pub members: u8,
    pub freed: u8,
    pub returning: u8,
}

/// The network bridge's counts; the IP service is their one writer, and a
/// reader takes each word on its own.
pub struct BridgeCounter {
    /// Received, refused for the pool, the queue and otherwise, copied,
    /// then [`BridgeTx`]'s in its order.
    counts: [core::sync::atomic::AtomicU32; 18],
}

impl BridgeCounter {
    pub const fn new() -> Self {
        Self {
            counts: [const { core::sync::atomic::AtomicU32::new(0) }; 18],
        }
    }

    /// One bridge turn, while the station held `station` of the network's
    /// `buffers` transmit buffers, `held` where its latest pass saw them.
    pub fn turned(
        &self,
        turn: oer_portable_embassy_net::ethernet::TxTurn,
        station: u32,
        buffers: u32,
        held: StationHeld,
    ) {
        use oer_portable_embassy_net::ethernet::TxOutcome;
        let queued = u32::try_from(turn.queued).unwrap_or(u32::MAX);
        let outcome = match turn.outcome {
            TxOutcome::Idle => None,
            TxOutcome::Moved => Some(6),
            TxOutcome::Held => Some(7),
            TxOutcome::Dropped => Some(8),
        };
        self.bump(5);
        if let Some(index) = outcome {
            self.bump(index);
        }
        if turn.queued >= turn.capacity {
            self.bump(9);
        }
        let taken = station
            .saturating_add(queued)
            .saturating_add(u32::from(turn.waited));
        // One buffer is the spare that frames from elsewhere are copied to.
        if taken >= buffers.saturating_sub(1) {
            self.bump(10);
        }
        self.add(11, queued);
        self.add(12, station);
        self.add(13, held.incoming.into());
        self.add(14, held.owed.into());
        self.add(15, held.members.into());
        self.add(16, held.freed.into());
        self.add(17, held.returning.into());
    }

    /// The bridge took a frame from the station.
    pub fn received(&self) {
        self.bump(0);
    }

    /// The stack refused a frame for `reason`.
    pub fn refused(&self, reason: oer_portable_embassy_net::RxEnqueueError) {
        use oer_portable_embassy_net::RxEnqueueError;
        self.bump(match reason {
            RxEnqueueError::PoolExhausted => 1,
            RxEnqueueError::QueueFull => 2,
            RxEnqueueError::InvalidLength(_) | RxEnqueueError::LinkDown => 3,
        });
    }

    /// The bridge copied a frame the station lent in place.
    pub fn copied(&self) {
        self.bump(4);
    }

    fn bump(&self, index: usize) {
        self.add(index, 1);
    }

    fn add(&self, index: usize, by: u32) {
        if let Some(count) = self.counts.get(index) {
            bump(count, by);
        }
    }

    /// The counts now.
    pub fn read(&self) -> Bridge {
        use core::sync::atomic::Ordering;
        let [
            received,
            refused_pool,
            refused_queue,
            refused_other,
            copied,
            turns,
            moved,
            held,
            dropped,
            queue_full,
            starved,
            queued_sum,
            station_sum,
            incoming_sum,
            owed_sum,
            members_sum,
            freed_sum,
            returning_sum,
        ] = self
            .counts
            .each_ref()
            .map(|count| count.load(Ordering::Relaxed));
        Bridge {
            received,
            refused_pool,
            refused_queue,
            refused_other,
            copied,
            tx: BridgeTx {
                turns,
                moved,
                held,
                dropped,
                queue_full,
                starved,
                queued_sum,
                station_sum,
                incoming_sum,
                owed_sum,
                members_sum,
                freed_sum,
                returning_sum,
            },
        }
    }
}

impl Default for BridgeCounter {
    fn default() -> Self {
        Self::new()
    }
}

/// Add `by` to `count`, wrapping; only its one writer calls it.
fn bump(count: &core::sync::atomic::AtomicU32, by: u32) {
    use core::sync::atomic::Ordering;
    count.store(
        count.load(Ordering::Relaxed).wrapping_add(by),
        Ordering::Relaxed,
    );
}

#[cfg(test)]
mod tests;
