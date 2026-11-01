//! Boot values every chip shares: the boot's identity, published once
//! before the executor starts, and the lowest channel a domain permits.

use core::sync::atomic::{AtomicU32, Ordering};

/// The boot's identity in two words: written once before any task runs,
/// read by every task after.
pub(crate) struct BootId {
    low: AtomicU32,
    high: AtomicU32,
}

impl BootId {
    pub(crate) const fn new() -> Self {
        Self {
            low: AtomicU32::new(0),
            high: AtomicU32::new(0),
        }
    }

    pub(crate) fn publish(&self, id: u64) {
        let [l0, l1, l2, l3, h0, h1, h2, h3] = id.to_le_bytes();
        self.low
            .store(u32::from_le_bytes([l0, l1, l2, l3]), Ordering::Release);
        self.high
            .store(u32::from_le_bytes([h0, h1, h2, h3]), Ordering::Release);
    }

    pub(crate) fn get(&self) -> u64 {
        (u64::from(self.high.load(Ordering::Acquire)) << 32)
            | u64::from(self.low.load(Ordering::Acquire))
    }
}

/// A nonzero identity from eight random bytes.
pub(crate) fn identity(random: [u8; 8]) -> u64 {
    u64::from_le_bytes(random).max(1)
}

#[cfg(any(test, feature = "wifi"))]
/// The first of the 2.4 GHz channels 1 to 13 that `attempt` accepts. An
/// error `refused` names as the channel's own is passed over; any other
/// ends the search with it. `None` when every channel was refused.
pub(crate) fn lowest_channel<T, E>(
    mut attempt: impl FnMut(u8) -> Result<T, E>,
    refused: impl Fn(&E) -> bool,
) -> Result<T, Option<E>> {
    for channel in 1..=13 {
        match attempt(channel) {
            Ok(value) => return Ok(value),
            Err(error) if refused(&error) => {}
            Err(error) => return Err(Some(error)),
        }
    }
    Err(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_published_identity_reads_back_whole() {
        let id = BootId::new();
        assert_eq!(id.get(), 0);
        id.publish(0x0123_4567_89ab_cdef);
        assert_eq!(id.get(), 0x0123_4567_89ab_cdef);
    }

    #[test]
    fn an_identity_is_never_zero() {
        assert_eq!(identity([0; 8]), 1);
        assert_eq!(identity([2, 0, 0, 0, 0, 0, 0, 1]), 0x0100_0000_0000_0002);
    }

    #[test]
    fn the_lowest_permitted_channel_wins_and_another_refusal_ends_the_search() {
        let mut tried = Vec::new();
        let found = lowest_channel(
            |channel| {
                tried.push(channel);
                if channel >= 3 {
                    Ok(channel)
                } else {
                    Err("channel")
                }
            },
            |error| *error == "channel",
        );
        assert_eq!(found, Ok(3));
        assert_eq!(tried, [1, 2, 3]);

        let country = lowest_channel(|_| Err::<u8, _>("country"), |error| *error == "channel");
        assert_eq!(country, Err(Some("country")));
        let none = lowest_channel(|_| Err::<u8, _>("channel"), |error| *error == "channel");
        assert_eq!(none, Err(None));
    }
}
