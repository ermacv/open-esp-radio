//! The neighbor table: IPv4 addresses to Ethernet addresses.
//!
//! SOURCE: RFC 826 (Packet Reception); RFC 1122, 2.3.2.1 for the one request
//! per second per destination.

use oer_portable_time::{Duration, Instant};

/// Requests before a neighbor counts as unreachable.
pub(crate) const REQUESTS: u8 = 3;
/// RFC 1122, 2.3.2.1: at most one request per second per destination.
const RETRY: Duration = Duration::from_secs(1);
const LIFETIME: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Entry {
    Free,
    Asking {
        address: [u8; 4],
        /// This resolution's: a grant names it and no later one.
        serial: u64,
        requests: u8,
        next: Instant,
        /// A request is owed to the port.
        owed: bool,
    },
    Known {
        address: [u8; 4],
        hardware: [u8; 6],
        expires: Instant,
    },
}

/// What is known of an address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Lookup {
    Known,
    Asking,
    /// Every entry holds a neighbor a datagram needs, so this one cannot be.
    Full,
}

/// The count of known-entry changes past which a finding never holds again.
const EXHAUSTED: u32 = u32::MAX;

/// What a look at the table found of an address, and while it holds: until
/// its entry expires, as long as no known entry changed meanwhile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Found {
    pub(crate) hardware: [u8; 6],
    known: u32,
    expires: Instant,
}

/// While what a look found holds: its station address goes apart, with
/// the work it was found for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Holds {
    known: u32,
    expires: Instant,
}

impl Found {
    /// While it holds.
    pub(crate) const fn holds(&self) -> Holds {
        Holds {
            known: self.known,
            expires: self.expires,
        }
    }
}

/// The table.
#[derive(Debug)]
pub(crate) struct Neighbors<const N: usize> {
    entries: [Entry; N],
    /// Moves with every write of a known entry and every replacement of one:
    /// while it stands, the known entries are as they were. It never comes
    /// back to a value it had: at [`EXHAUSTED`] it stops, and from then on
    /// nothing found holds, so every frame looks again.
    known: u32,
}

impl<const N: usize> Neighbors<N> {
    pub(crate) const fn new() -> Self {
        Self {
            entries: [Entry::Free; N],
            known: 0,
        }
    }

    /// Forget every entry, as a new network's table: what was found in it
    /// holds no more.
    pub(crate) fn forget(&mut self) {
        self.entries = [Entry::Free; N];
        self.moved();
    }

    fn moved(&mut self) {
        // Saturating, never wrapping: a count that came back would make an
        // old finding hold again for an address now elsewhere.
        self.known = self.known.saturating_add(1);
    }

    /// Where `address` is, while that is known.
    pub(crate) fn hardware(&self, now: Instant, address: [u8; 4]) -> Option<[u8; 6]> {
        self.find(now, address).map(|found| found.hardware)
    }

    /// Where `address` is, while that is known, and while that holds.
    pub(crate) fn find(&self, now: Instant, address: [u8; 4]) -> Option<Found> {
        self.entries.iter().find_map(|entry| match *entry {
            Entry::Known {
                address: known,
                hardware,
                expires,
            } if known == address && now < expires => Some(Found {
                hardware,
                known: self.known,
                expires,
            }),
            _ => None,
        })
    }

    /// Whether a look that found `holds` still holds at `now`: no known
    /// entry changed since, and its entry has not expired. A look then would
    /// find the same. Once
    /// the count is exhausted nothing holds.
    pub(crate) fn holds_still(&self, holds: &Holds, now: Instant) -> bool {
        self.known != EXHAUSTED && holds.known == self.known && now < holds.expires
    }

    /// Find `address`, or begin asking for it: in its own expired entry, a
    /// free one, or the known one that expires first among those `needed`
    /// says no datagram holds. An accepted datagram's neighbor is never
    /// evicted, so one datagram never asks another's neighbor away.
    pub(crate) fn resolve(
        &mut self,
        now: Instant,
        address: [u8; 4],
        serial: u64,
        needed: impl Fn([u8; 4]) -> bool,
    ) -> Lookup {
        let mut oldest: Option<(usize, Instant)> = None;
        let mut free = None;
        let mut own = None;
        for (index, entry) in self.entries.iter().enumerate() {
            match *entry {
                Entry::Known {
                    address: known,
                    expires,
                    ..
                } => {
                    if known == address {
                        if now < expires {
                            return Lookup::Known;
                        }
                        own = Some(index);
                    }
                    if !needed(known) && oldest.is_none_or(|(_, at)| expires < at) {
                        oldest = Some((index, expires));
                    }
                }
                Entry::Asking { address: asked, .. } if asked == address => {
                    return Lookup::Asking;
                }
                Entry::Asking { .. } => {}
                Entry::Free => free = free.or(Some(index)),
            }
        }
        let Some(entry) = own
            .or(free)
            .or(oldest.map(|(index, _)| index))
            .and_then(|index| self.entries.get_mut(index))
        else {
            return Lookup::Full;
        };
        let replaced = matches!(entry, Entry::Known { .. });
        *entry = Self::asking(now, address, serial);
        if replaced {
            self.moved();
        }
        Lookup::Asking
    }

    const fn asking(now: Instant, address: [u8; 4], serial: u64) -> Entry {
        Entry::Asking {
            address,
            serial,
            requests: 0,
            next: now,
            owed: true,
        }
    }

    /// Whether `address` has an entry of its own: known, alive or expired
    /// (which [`Self::resolve`] asks for again in place), or asked for.
    pub(crate) fn holds(&self, address: [u8; 4]) -> bool {
        self.entries.iter().any(|entry| match *entry {
            Entry::Known { address: known, .. } => known == address,
            Entry::Asking { address: asked, .. } => asked == address,
            Entry::Free => false,
        })
    }

    /// The entries whose address `needed` names.
    pub(crate) fn count(&self, needed: impl Fn([u8; 4]) -> bool) -> usize {
        self.entries
            .iter()
            .filter(|entry| match **entry {
                Entry::Known { address, .. } | Entry::Asking { address, .. } => needed(address),
                Entry::Free => false,
            })
            .count()
    }

    /// Forget requests no datagram needs any more. An entry is asked for only
    /// while one does, so a refused port holds no entry for a closed socket.
    pub(crate) fn forget_unneeded(&mut self, needed: impl Fn([u8; 4]) -> bool) {
        for entry in &mut self.entries {
            if let Entry::Asking { address, .. } = *entry
                && !needed(address)
            {
                *entry = Entry::Free;
            }
        }
    }

    /// The addresses whose known entries expired by `now`, each once: what
    /// [`Self::reask`] would ask again for. Usually none, and never more
    /// than the table holds, so a caller looks at its own work only for
    /// these.
    pub(crate) fn expired(&self, now: Instant) -> [Option<[u8; 4]>; N] {
        let mut expired = [None; N];
        for (slot, entry) in expired.iter_mut().zip(&self.entries) {
            if let Entry::Known {
                address, expires, ..
            } = *entry
                && now >= expires
            {
                *slot = Some(address);
            }
        }
        expired
    }

    /// Ask again for `address` if its entry expired; whether it did.
    pub(crate) fn reask(&mut self, now: Instant, address: [u8; 4], serial: u64) -> bool {
        for entry in &mut self.entries {
            if let Entry::Known {
                address: known,
                expires,
                ..
            } = *entry
                && known == address
                && now >= expires
            {
                *entry = Self::asking(now, address, serial);
                self.moved();
                return true;
            }
        }
        false
    }

    /// Note `address` at `hardware`. An entry is made only when `make`: RFC
    /// 826 merges what it knows, and adds only when the packet was for us.
    /// Whether an entry took it.
    pub(crate) fn learn(
        &mut self,
        now: Instant,
        address: [u8; 4],
        hardware: [u8; 6],
        make: bool,
    ) -> bool {
        let known = Entry::Known {
            address,
            hardware,
            expires: now.saturating_add(LIFETIME),
        };
        let held = self.entries.iter_mut().find(|entry| match **entry {
            Entry::Asking { address: held, .. } | Entry::Known { address: held, .. } => {
                held == address
            }
            Entry::Free => false,
        });
        let entry = match held {
            Some(entry) => entry,
            None if make => match self.entries.iter_mut().find(|entry| **entry == Entry::Free) {
                Some(entry) => entry,
                None => return false,
            },
            None => return false,
        };
        *entry = known;
        self.moved();
        true
    }

    /// The requests owed: each entry with its address and resolution.
    /// The first entry from `from` on that owes a request, as
    /// [`Self::owed_requests`] gives it, with the index after it.
    pub(crate) fn owed_request_from(&self, from: usize) -> Option<((u8, [u8; 4], u64), usize)> {
        let rest = self.entries.get(from..)?;
        rest.iter()
            .zip(from..)
            .find_map(|(entry, index)| match *entry {
                Entry::Asking {
                    address,
                    serial,
                    owed: true,
                    ..
                } => u8::try_from(index)
                    .ok()
                    .map(|place| ((place, address, serial), index.saturating_add(1))),
                _ => None,
            })
    }

    pub(crate) fn owed_requests(&self) -> impl Iterator<Item = (u8, [u8; 4], u64)> + Clone + '_ {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| match *entry {
                Entry::Asking {
                    address,
                    serial,
                    owed: true,
                    ..
                } => u8::try_from(index)
                    .ok()
                    .map(|index| (index, address, serial)),
                _ => None,
            })
    }

    /// Whether any request is owed: a look at each entry, as
    /// [`Self::owed_requests`] makes.
    pub(crate) fn owes_request(&self) -> bool {
        self.entries
            .iter()
            .any(|entry| matches!(entry, Entry::Asking { owed: true, .. }))
    }

    /// The port took the request of entry `index`.
    pub(crate) fn request_accepted(&mut self, index: u8, now: Instant) {
        if let Some(Entry::Asking {
            requests,
            next,
            owed,
            ..
        }) = self.entries.get_mut(usize::from(index))
        {
            *requests = requests.saturating_add(1);
            *next = now.saturating_add(RETRY);
            *owed = false;
        }
    }

    /// The port refused the request of entry `index` for good: this resolution
    /// ends here, and its address comes back. Nothing asks in its name, so a
    /// later resolution of that address is a new one.
    pub(crate) fn request_refused(&mut self, index: u8, resolution: u64) -> Option<[u8; 4]> {
        let entry = self.entries.get_mut(usize::from(index))?;
        match *entry {
            Entry::Asking {
                address, serial, ..
            } if serial == resolution => {
                *entry = Entry::Free;
                Some(address)
            }
            _ => None,
        }
    }

    /// Ask again where the time came. Each address given up goes to `gave_up`.
    pub(crate) fn tick(&mut self, now: Instant, mut gave_up: impl FnMut([u8; 4])) {
        for entry in &mut self.entries {
            if let Entry::Asking {
                address,
                requests,
                next,
                owed,
                ..
            } = entry
                && !*owed
                && now >= *next
            {
                if *requests >= REQUESTS {
                    gave_up(*address);
                    *entry = Entry::Free;
                } else {
                    *owed = true;
                }
            }
        }
    }

    /// When the next request, end, or expiry of a neighbor `needed` falls
    /// due.
    pub(crate) fn deadline(&self, needed: impl Fn([u8; 4]) -> bool) -> Option<Instant> {
        self.entries
            .iter()
            .filter_map(|entry| match *entry {
                Entry::Asking {
                    next, owed: false, ..
                } => Some(next),
                Entry::Known {
                    address, expires, ..
                } if needed(address) => Some(expires),
                _ => None,
            })
            .min()
    }
}

#[cfg(test)]
#[allow(
    clippy::disallowed_macros,
    clippy::unwrap_used,
    reason = "tests assert on a table at its count's boundary"
)]
mod tests {
    use super::*;

    const PEER: [u8; 4] = [192, 0, 2, 2];
    const FIRST: [u8; 6] = [2, 0, 0, 0, 0, 2];
    const MOVED: [u8; 6] = [2, 0, 0, 0, 0, 3];

    fn at(seconds: u64) -> Instant {
        Instant::from_micros(seconds * 1_000_000)
    }

    /// A finding taken at a count of 0, then the count driven to its end
    /// and the peer moved: with a wrapping count the move would bring it
    /// back to 0 and the old finding would hold for the old station. It
    /// does not: the move is seen, and a look finds the new station.
    #[test]
    fn a_finding_never_holds_again_once_the_count_is_exhausted() {
        let mut table = Neighbors::<2>::new();
        assert!(table.learn(at(0), PEER, FIRST, true));
        table.known = 0;
        let found = table.find(at(1), PEER).unwrap();
        assert!(table.holds_still(&found.holds(), at(1)));
        table.known = EXHAUSTED - 1;
        assert!(table.learn(at(1), PEER, MOVED, true));
        assert!(
            table.learn(at(1), PEER, MOVED, true),
            "a write past the end"
        );
        assert!(
            !table.holds_still(&found.holds(), at(1)),
            "the old station's finding holds no more"
        );
        assert_eq!(table.hardware(at(1), PEER), Some(MOVED));
        assert_eq!(table.known, EXHAUSTED, "the count stays at its end");
    }

    /// At the end of the count a fresh finding holds neither: every frame
    /// looks again, and a look is always right.
    #[test]
    fn at_the_end_of_the_count_nothing_found_holds() {
        let mut table = Neighbors::<2>::new();
        table.known = EXHAUSTED;
        assert!(table.learn(at(0), PEER, FIRST, true));
        let found = table.find(at(1), PEER).unwrap();
        assert_eq!(found.hardware, FIRST);
        assert!(!table.holds_still(&found.holds(), at(1)));
    }
}
