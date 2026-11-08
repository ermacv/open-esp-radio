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

/// The table.
#[derive(Debug)]
pub(crate) struct Neighbors<const N: usize> {
    entries: [Entry; N],
}

impl<const N: usize> Neighbors<N> {
    pub(crate) const fn new() -> Self {
        Self {
            entries: [Entry::Free; N],
        }
    }

    /// Where `address` is, while that is known.
    pub(crate) fn hardware(&self, now: Instant, address: [u8; 4]) -> Option<[u8; 6]> {
        self.entries.iter().find_map(|entry| match *entry {
            Entry::Known {
                address: known,
                hardware,
                expires,
            } if known == address && now < expires => Some(hardware),
            _ => None,
        })
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
        *entry = Self::asking(now, address, serial);
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
        for entry in &mut self.entries {
            match *entry {
                Entry::Asking { address: held, .. } | Entry::Known { address: held, .. }
                    if held == address =>
                {
                    *entry = known;
                    return true;
                }
                _ => {}
            }
        }
        if make && let Some(entry) = self.entries.iter_mut().find(|entry| **entry == Entry::Free) {
            *entry = known;
            return true;
        }
        false
    }

    /// The requests owed: each entry with its address and resolution.
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
