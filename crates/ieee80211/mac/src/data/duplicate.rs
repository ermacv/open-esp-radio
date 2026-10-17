//! Association/peer-scoped receive retry history.

/// Per-traffic-class IEEE 802.11 receive history.
///
/// A retransmission is a duplicate only when Retry is set and the complete
/// Sequence Control value matches the last accepted MPDU in the same legacy
/// or QoS/TID sequence space. The owner is role-neutral and is reset at every
/// STA association or AP peer epoch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxDuplicateFilter {
    last_sequence_control: [u16; 17],
    valid: u32,
}

impl RxDuplicateFilter {
    pub const fn new() -> Self {
        Self {
            last_sequence_control: [0; 17],
            valid: 0,
        }
    }

    /// Query whether an MPDU is already present without accepting it.
    ///
    /// Fragment reassembly uses this read-only edge for fragment zero: an
    /// ordinary MPDU already accepted with the same Sequence Control must
    /// fence a Retry that toggles More Fragments, while fragmented MPDUs keep
    /// their acceptance and retry history entirely inside the reassembler.
    #[inline(always)]
    pub fn is_known_duplicate(&self, retry: bool, sequence_control: u16, tid: Option<u8>) -> bool {
        let index = match tid {
            Some(tid @ 0..=15) => usize::from(tid) + 1,
            _ => 0,
        };
        let mask = 1_u32 << index;
        retry && self.valid & mask != 0 && self.last_sequence_control[index] == sequence_control
    }

    #[inline(always)]
    pub fn is_duplicate(&mut self, retry: bool, sequence_control: u16, tid: Option<u8>) -> bool {
        let index = match tid {
            Some(tid @ 0..=15) => usize::from(tid) + 1,
            _ => 0,
        };
        let mask = 1_u32 << index;
        if self.is_known_duplicate(retry, sequence_control, tid) {
            return true;
        }
        self.last_sequence_control[index] = sequence_control;
        self.valid |= mask;
        false
    }
}

impl Default for RxDuplicateFilter {
    fn default() -> Self {
        Self::new()
    }
}
