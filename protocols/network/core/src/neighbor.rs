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
    /// Every entry is being asked for, so this one cannot be.
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
    /// free one, or the one that expires first.
    pub(crate) fn resolve(&mut self, now: Instant, address: [u8; 4]) -> Lookup {
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
                    if oldest.is_none_or(|(_, at)| expires < at) {
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
        *entry = Entry::Asking {
            address,
            requests: 0,
            next: now,
            owed: true,
        };
        Lookup::Asking
    }

    /// Note `address` at `hardware`. An entry is made only when `make`: RFC
    /// 826 merges what it knows, and adds only when the packet was for us.
    pub(crate) fn learn(&mut self, now: Instant, address: [u8; 4], hardware: [u8; 6], make: bool) {
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
                    return;
                }
                _ => {}
            }
        }
        if make && let Some(entry) = self.entries.iter_mut().find(|entry| **entry == Entry::Free) {
            *entry = known;
        }
    }

    /// The requests owed, by their entries.
    pub(crate) fn owed_requests(&self) -> impl Iterator<Item = (u8, [u8; 4])> + '_ {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| match *entry {
                Entry::Asking {
                    address,
                    owed: true,
                    ..
                } => u8::try_from(index).ok().map(|index| (index, address)),
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

    /// Ask again where the time came. Each address given up goes to `gave_up`.
    pub(crate) fn tick(&mut self, now: Instant, mut gave_up: impl FnMut([u8; 4])) {
        for entry in &mut self.entries {
            if let Entry::Asking {
                address,
                requests,
                next,
                owed,
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

    /// When the next request or end falls due.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.entries
            .iter()
            .filter_map(|entry| match *entry {
                Entry::Asking {
                    next, owed: false, ..
                } => Some(next),
                _ => None,
            })
            .min()
    }
}
