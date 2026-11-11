//! The echo replies the core owes.
//!
//! SOURCE: RFC 792 (Echo or Echo Reply Message); RFC 1122, 3.2.2.6 (Echo
//! Request/Reply): the reply comes from the address the request was sent to
//! and returns all its data; a request to a broadcast or multicast address
//! may be discarded, and is. Bounds are this core's: at most [`ECHOES`]
//! replies owed at once, each holding one block, never the pool's last free
//! one, and at most [`RATE`] replies a second.

use oer_portable_time::{Duration, Instant};

use crate::block::Block;

/// Replies owed at once.
pub const ECHOES: usize = 4;
/// Replies accepted in one [`WINDOW`].
pub const RATE: u8 = 16;
const WINDOW: Duration = Duration::from_secs(1);

/// Why an ICMP message got no reply. Counted, never answered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IcmpDrop {
    /// Shorter than an echo's header, or an echo with a code other than 0.
    Malformed,
    /// The ICMP checksum failed, RFC 792.
    Checksum,
    /// An echo to a broadcast address, RFC 1122, 3.2.2.6.
    Broadcast,
    /// A message that is not an echo: this core asks nothing that a reply
    /// could answer, and RFC 1122, 3.2.2 has the rest discarded.
    Other,
    /// The data does not fit a block.
    TooLong,
    /// [`ECHOES`] replies are owed already.
    Busy,
    /// [`RATE`] replies were accepted in this second.
    Limited,
    /// No block, or only the pool's last one, is free.
    NoBlock,
    /// No route leads to the asker.
    NoRoute,
    /// Every neighbor entry this traffic may use is held.
    NeighborsFull,
    /// The asker's next hop did not answer ARP, or its request was refused.
    Unreachable,
    /// The interface's address changed before the reply went out.
    Readdressed,
}

/// One reply owed: the request's ICMP message, whose type it changes.
#[derive(Debug)]
pub(crate) struct Owed<'m, B, const SIZE: usize> {
    pub(crate) serial: u64,
    pub(crate) block: Block<'m, B, SIZE>,
    /// The asker.
    pub(crate) to: [u8; 4],
    /// The neighbor the reply goes through.
    pub(crate) hop: [u8; 4],
}

/// The replies owed and the rate's window.
#[derive(Debug)]
pub(crate) struct Echoes<'m, B, const SIZE: usize> {
    owed: [Option<Owed<'m, B, SIZE>>; ECHOES],
    window: Instant,
    accepted: u8,
}

impl<'m, B, const SIZE: usize> Echoes<'m, B, SIZE> {
    pub(crate) const fn new() -> Self {
        Self {
            owed: [const { None }; ECHOES],
            window: Instant::from_micros(0),
            accepted: 0,
        }
    }

    /// Whether a reply may be accepted at `now`: room, and the rate.
    pub(crate) fn admits(&mut self, now: Instant) -> Result<(), IcmpDrop> {
        if self.owed.iter().all(Option::is_some) {
            return Err(IcmpDrop::Busy);
        }
        if now >= self.window.saturating_add(WINDOW) {
            self.window = now;
            self.accepted = 0;
        }
        if self.accepted >= RATE {
            return Err(IcmpDrop::Limited);
        }
        Ok(())
    }

    /// Owe `owed`, or hand it back when no place is free.
    pub(crate) fn push(&mut self, owed: Owed<'m, B, SIZE>) -> Result<(), Owed<'m, B, SIZE>> {
        let Some(place) = self.owed.iter_mut().find(|place| place.is_none()) else {
            return Err(owed);
        };
        *place = Some(owed);
        self.accepted = self.accepted.saturating_add(1);
        Ok(())
    }

    /// Whether no reply is owed.
    pub(crate) fn none(&self) -> bool {
        self.owed.iter().all(Option::is_none)
    }

    /// The replies owed, oldest first.
    #[cfg(test)]
    pub(crate) fn iter(&self) -> impl Iterator<Item = &Owed<'m, B, SIZE>> + Clone + '_ {
        let mut oldest = [None; ECHOES];
        for (place, owed) in oldest.iter_mut().zip(self.owed.iter()) {
            *place = owed.as_ref().map(|owed| owed.serial);
        }
        oldest.sort_unstable_by_key(|serial| serial.unwrap_or(u64::MAX));
        oldest
            .into_iter()
            .flatten()
            .filter_map(move |serial| self.get(serial))
    }

    /// The oldest reply owed after `after`'s serial, as [`Self::iter`]
    /// orders them; with `None`, the oldest.
    pub(crate) fn next_after(&self, after: Option<u64>) -> Option<&Owed<'m, B, SIZE>> {
        self.owed
            .iter()
            .flatten()
            .filter(|owed| after.is_none_or(|after| owed.serial > after))
            .min_by_key(|owed| owed.serial)
    }

    pub(crate) fn get(&self, serial: u64) -> Option<&Owed<'m, B, SIZE>> {
        self.owed
            .iter()
            .flatten()
            .find(|owed| owed.serial == serial)
    }

    /// Whether a reply goes through `address`.
    pub(crate) fn needs(&self, address: [u8; 4]) -> bool {
        self.owed.iter().flatten().any(|owed| owed.hop == address)
    }

    /// Take the reply of `serial`.
    pub(crate) fn take(&mut self, serial: u64) -> Option<Owed<'m, B, SIZE>> {
        self.owed
            .iter_mut()
            .find(|place| place.as_ref().is_some_and(|owed| owed.serial == serial))
            .and_then(Option::take)
    }

    /// Take every reply that goes through `address`, each to `end`.
    pub(crate) fn take_through(
        &mut self,
        address: [u8; 4],
        mut end: impl FnMut(Owed<'m, B, SIZE>),
    ) {
        for place in &mut self.owed {
            if place.as_ref().is_some_and(|owed| owed.hop == address)
                && let Some(owed) = place.take()
            {
                end(owed);
            }
        }
    }

    /// Take every reply, each to `end`.
    pub(crate) fn take_all(&mut self, mut end: impl FnMut(Owed<'m, B, SIZE>)) {
        for owed in self.owed.iter_mut().filter_map(Option::take) {
            end(owed);
        }
    }
}
