//! Distinct echo pairs; a duplicate never replaces a missing sequence.

/// The W3 workload in either direction.
pub const ECHO_COUNT: u16 = 100;
/// Claimed request cadence, independently checked by the router capture.
pub const INTERVAL_MS: u32 = 1000;

/// One identifier and a consecutive window of sequence numbers. Requests
/// enter only when sent/received; a reply must follow its matching request.
#[derive(Default)]
pub struct EchoSeries {
    identity: Option<(u16, u16)>,
    requests: [u64; 2],
    replies: [u64; 2],
}

impl EchoSeries {
    pub const fn new() -> Self {
        Self {
            identity: None,
            requests: [0; 2],
            replies: [0; 2],
        }
    }

    /// Start with the first actual request, including a sequence near wrap.
    /// Later requests of another identifier or outside this run are refused.
    pub fn request(&mut self, identifier: u16, sequence: u16) -> bool {
        let (expected, first) = *self.identity.get_or_insert((identifier, sequence));
        let offset = sequence.wrapping_sub(first);
        if identifier != expected || offset >= ECHO_COUNT {
            return false;
        }
        mark(&mut self.requests, offset)
    }

    pub fn reply(&mut self, identifier: u16, sequence: u16) -> bool {
        let Some((expected, first)) = self.identity else {
            return false;
        };
        let offset = sequence.wrapping_sub(first);
        if identifier != expected || offset >= ECHO_COUNT || !contains(&self.requests, offset) {
            return false;
        }
        mark(&mut self.replies, offset)
    }

    pub fn requests(&self) -> u32 {
        self.requests.iter().map(|word| word.count_ones()).sum()
    }

    pub fn replies(&self) -> u32 {
        self.replies.iter().map(|word| word.count_ones()).sum()
    }

    pub fn complete(&self) -> bool {
        self.requests() == u32::from(ECHO_COUNT) && self.replies() == u32::from(ECHO_COUNT)
    }
}

fn contains(bits: &[u64; 2], offset: u16) -> bool {
    bits.get(usize::from(offset / 64))
        .is_some_and(|word| word & (1 << (offset % 64)) != 0)
}

fn mark(bits: &mut [u64; 2], offset: u16) -> bool {
    let Some(word) = bits.get_mut(usize::from(offset / 64)) else {
        return false;
    };
    let mask = 1 << (offset % 64);
    let fresh = *word & mask == 0;
    *word |= mask;
    fresh
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_distinct_ordered_pairs_complete_including_sequence_wrap() {
        let mut echoes = EchoSeries::new();
        assert!(!echoes.reply(7, u16::MAX));
        for offset in 0..ECHO_COUNT {
            let sequence = u16::MAX.wrapping_add(offset);
            assert!(echoes.request(7, sequence));
            assert!(!echoes.request(7, sequence));
            assert!(!echoes.reply(8, sequence));
            assert!(echoes.reply(7, sequence));
            assert!(!echoes.reply(7, sequence));
        }
        assert!(echoes.complete());
        assert!(!echoes.request(7, 99));
        assert_eq!((echoes.requests(), echoes.replies()), (100, 100));
    }

    #[test]
    fn duplicates_and_early_replies_cannot_hide_a_missing_echo() {
        let mut echoes = EchoSeries::new();
        for sequence in 1..=100 {
            assert!(echoes.request(9, sequence));
            if sequence != 50 {
                assert!(echoes.reply(9, sequence));
            }
            assert!(!echoes.reply(9, sequence + 1));
        }
        for _ in 0..100 {
            assert!(!echoes.reply(9, 1));
        }
        assert!(!echoes.complete());
        assert_eq!((echoes.requests(), echoes.replies()), (100, 99));
        assert!(!echoes.request(10, 50));
        assert!(!echoes.request(9, 101));
        assert!(echoes.reply(9, 50));
        assert!(echoes.complete());
    }
}
