//! Association/peer-scoped receive retry history.
//!
//! Received bytes reach this module; nothing here panics on them.
#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]

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
        let (bit, last) = space(&self.last_sequence_control, tid);
        retry && self.valid & bit != 0 && *last == sequence_control
    }

    #[inline(always)]
    pub fn is_duplicate(&mut self, retry: bool, sequence_control: u16, tid: Option<u8>) -> bool {
        if self.is_known_duplicate(retry, sequence_control, tid) {
            return true;
        }
        let (bit, last) = space_mut(&mut self.last_sequence_control, tid);
        *last = sequence_control;
        self.valid |= bit;
        false
    }
}

/// The validity bit and history entry of `tid`'s sequence space: the legacy
/// space (bit 0) for a non-QoS MPDU or a TID above 15, else that TID's
/// (bits 1 to 16).
fn space(history: &[u16; 17], tid: Option<u8>) -> (u32, &u16) {
    let [legacy, tids @ ..] = history;
    match tid.and_then(|tid| Some((tid, tids.get(usize::from(tid))?))) {
        Some((tid, entry)) => (tid_bit(tid), entry),
        None => (1, legacy),
    }
}

/// The mutable entry of [`space`].
fn space_mut(history: &mut [u16; 17], tid: Option<u8>) -> (u32, &mut u16) {
    let [legacy, tids @ ..] = history;
    match tid.and_then(|tid| Some((tid, tids.get_mut(usize::from(tid))?))) {
        Some((tid, entry)) => (tid_bit(tid), entry),
        None => (1, legacy),
    }
}

/// The validity bit of TID `tid`, at most 15: bit `tid + 1`.
const fn tid_bit(tid: u8) -> u32 {
    // `tid` indexes one of the 16 TID entries, so the shift is at most 16.
    2_u32 << tid
}

impl Default for RxDuplicateFilter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
