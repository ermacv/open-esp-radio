//! The receive reorder buffer of one Block Ack agreement.
//!
//! IEEE Std 802.11-2020 10.25.6 makes the recipient of a Block Ack
//! agreement release MSDUs to the upper layer in sequence order. Reordering
//! is software above the lower-MAC port unless a backend reports
//! `HardwareServices::RX_REORDER`. [`RxReorderBuffer`] is that software: it
//! holds the window of one agreement and decides which buffered MPDUs a
//! received MPDU, a BlockAckReq, a gap timeout or a teardown releases.
//!
//! The buffer never holds frame bytes. Every retained MPDU is a checked
//! caller-chosen slot index ([`RxReorderMpdu::slot`]) into the caller's own
//! frame storage; the caller recycles every slot a [`RxReorderRelease`]
//! returns. The buffer never reads time: the caller decides when a gap has
//! waited long enough and calls [`RxReorderBuffer::expire_gap`].
//!
//! `WINDOW_CAPACITY` bounds the negotiated window (at most
//! [`MAX_RX_REORDER_WINDOW`], the width of the occupancy bitmap) and sizes
//! the retained-MPDU table; `SLOT_CAPACITY` is the size of the caller's slot
//! domain. A backend whose memory profile needs a smaller window chooses a
//! smaller `WINDOW_CAPACITY`.
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

use core::num::NonZeroUsize;

use oer_ieee80211_wire::sequence::SequenceNumber;

/// The widest window a [`RxReorderBuffer`] holds: its occupancy bitmap has
/// one bit per window position.
pub const MAX_RX_REORDER_WINDOW: u16 = 64;

/// The slots a one-octet [`RxReorderMpdu::slot`] can name: `u8::MAX + 1`.
const SLOT_DOMAIN: usize = 256;

/// One received MPDU of the agreement: its sequence number and the slot of
/// the caller's storage that holds it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxReorderMpdu {
    pub sequence: SequenceNumber,
    pub slot: u8,
}

/// Why the buffer refused an operation; the buffer did not change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxReorderError {
    /// A window of zero, or wider than the buffer's `WINDOW_CAPACITY`.
    InvalidWindow(u16),
    /// A slot outside the caller's `SLOT_CAPACITY` domain.
    InvalidSlot(u8),
    /// An agreement names a reorder bank outside its owner's fixed set. The
    /// buffer itself never returns it; an owner of several buffers keyed by
    /// bank reports it through the same type.
    InvalidBank(u8),
    /// The sequence is already buffered.
    DuplicateSequence(SequenceNumber),
    /// The slot already holds a buffered MPDU.
    SlotAlreadyOwned(u8),
    /// An internal storage position is outside its array; no state changed.
    InvalidPosition { position: usize, capacity: usize },
}

/// The MPDUs one operation hands back to the caller, in sequence order.
/// Every operation creates a fresh list. At most N-1 frames can wait behind
/// the current gap, leaving room for the one received frame in a release
/// of N positions. Transferring each retained position into its matching
/// result position and reserving the gap's position for that input prevents
/// an overflow; there is no append operation or dropped-frame counter.
///
/// The list cannot be overwritten or extended by another operation:
///
/// ```compile_fail,E0616
/// use oer_ieee80211_upper_mac::block_ack::RxReorderRelease;
/// fn overwrite(release: &mut RxReorderRelease<8>) {
///     release.frames = [None; 8];
/// }
/// ```
///
/// ```
/// use oer_ieee80211_upper_mac::block_ack::RxReorderRelease;
/// fn returned_slots(release: &RxReorderRelease<8>) -> usize {
///     release.iter().count()
/// }
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxReorderRelease<const WINDOW_CAPACITY: usize> {
    frames: [Option<RxReorderMpdu>; WINDOW_CAPACITY],
    /// Sequence numbers the window moved past without an MPDU.
    pub missing: u16,
    /// A received MPDU behind the window: the caller discards it.
    pub rejected: Option<RxReorderMpdu>,
    /// Whether the received MPDU stays buffered after the operation.
    pub buffered: bool,
}

impl<const WINDOW_CAPACITY: usize> RxReorderRelease<WINDOW_CAPACITY> {
    const fn empty() -> Self {
        Self {
            frames: [None; WINDOW_CAPACITY],
            missing: 0,
            rejected: None,
            buffered: false,
        }
    }

    /// The released MPDUs in sequence order.
    pub fn iter(&self) -> impl Iterator<Item = RxReorderMpdu> + '_ {
        self.frames.iter().flatten().copied()
    }

    fn order(&mut self, start: SequenceNumber) {
        self.frames.sort_unstable_by_key(|frame| {
            frame.map_or(u16::MAX, |frame| start.forward_distance(frame.sequence))
        });
    }
}

/// The bounded sequence range transferred by one window advance.
struct ReleaseRange {
    start: SequenceNumber,
    advance: u16,
    span: u16,
}

impl ReleaseRange {
    fn transfer(
        &self,
        index: usize,
        released: &mut Option<RxReorderMpdu>,
        retained: &mut Option<RxReorderMpdu>,
        missing: &mut u16,
        occupied: &mut u64,
    ) {
        if let Some(frame) = retained {
            let distance = self.start.forward_distance(frame.sequence);
            if distance < self.advance {
                // At most one MPDU per skipped sequence.
                *missing = missing.wrapping_sub(1);
            }
            if distance < self.span {
                *occupied &= !1_u64.wrapping_shl(index as u32);
                *released = retained.take();
            }
        }
    }
}

/// The reorder window of one receive Block Ack agreement.
pub struct RxReorderBuffer<const WINDOW_CAPACITY: usize, const SLOT_CAPACITY: usize> {
    starting_sequence: SequenceNumber,
    next_sequence: SequenceNumber,
    window: u16,
    occupied: u64,
    /// Physical position of `next_sequence`, always vacant between inputs.
    /// Mapping relative to this head also works when WINDOW_CAPACITY does
    /// not divide the 4096-value sequence space.
    head: usize,
    frames: [Option<RxReorderMpdu>; WINDOW_CAPACITY],
}

impl<const WINDOW_CAPACITY: usize, const SLOT_CAPACITY: usize>
    RxReorderBuffer<WINDOW_CAPACITY, SLOT_CAPACITY>
{
    /// An empty window of `window` positions starting at the agreement's
    /// starting sequence.
    ///
    /// A buffer without window positions does not compile, so the window
    /// arithmetic never reduces modulo zero:
    ///
    /// ```compile_fail,E0080
    /// use oer_ieee80211_upper_mac::block_ack::RxReorderBuffer;
    /// use oer_ieee80211_wire::sequence::SequenceNumber;
    /// let _ = RxReorderBuffer::<0, 8>::new(SequenceNumber::ZERO, 1);
    /// ```
    pub const fn new(
        starting_sequence: SequenceNumber,
        window: u16,
    ) -> Result<Self, RxReorderError> {
        const {
            if WINDOW_CAPACITY == 0 || WINDOW_CAPACITY > MAX_RX_REORDER_WINDOW as usize {
                panic!("the occupancy bitmap holds at most 64 window positions");
            }
        };
        if window == 0 || window as usize > WINDOW_CAPACITY {
            return Err(RxReorderError::InvalidWindow(window));
        }
        Ok(Self {
            starting_sequence,
            next_sequence: starting_sequence,
            window,
            occupied: 0,
            head: 0,
            frames: [None; WINDOW_CAPACITY],
        })
    }

    /// The sequence the window waits for (WinStartB).
    pub const fn next_sequence(&self) -> SequenceNumber {
        self.next_sequence
    }

    pub const fn window(&self) -> u16 {
        self.window
    }

    /// Buffered MPDUs.
    pub const fn occupied(&self) -> u32 {
        self.occupied.count_ones()
    }

    /// Rebase the window on the first aggregate after a new agreement when
    /// that aggregate lies behind it, releasing every buffered MPDU.
    ///
    /// A newly negotiated agreement normally accepts the first aggregate in
    /// the forward half of its sequence space. If that aggregate is instead
    /// behind the current window start, the window moves: with
    /// `use_received_sequence` it starts at the received sequence; otherwise
    /// it falls back to the negotiated starting sequence, and only an
    /// aggregate also behind that moves the window to end at the received
    /// sequence. `None` when the sequence is not behind the window.
    ///
    /// This is a recovery edge the caller chooses to apply; the Espressif
    /// reorder path applies it with `use_received_sequence` for formats newer
    /// than HT, as selected by the receive backend.
    pub fn resynchronize_stale_initial_ampdu(
        &mut self,
        sequence: SequenceNumber,
        use_received_sequence: bool,
    ) -> Option<(RxReorderRelease<WINDOW_CAPACITY>, SequenceNumber)> {
        if self.next_sequence.forward_distance(sequence) < SequenceNumber::HALF_SPACE {
            return None;
        }

        let mut next_sequence = self.starting_sequence;
        if use_received_sequence
            || self.starting_sequence.forward_distance(sequence) >= SequenceNumber::HALF_SPACE
        {
            // `new` refuses a zero window, so `window - 1` does not wrap.
            next_sequence = sequence.wrapping_sub(self.window.wrapping_sub(1));
            if use_received_sequence {
                next_sequence = sequence;
            }
        }

        let released = self.stop();
        self.next_sequence = next_sequence;
        Some((released, next_sequence))
    }

    /// Decide whether the caller must reserve backing storage before
    /// [`Self::ingest`].
    ///
    /// The next expected sequence is released immediately. A forward
    /// sequence requires backing for its gap; a window advance may then
    /// release it with a complete run in the same input. The release names
    /// that same backing slot even when `buffered` is false. A stale MPDU is
    /// rejected by `ingest` and never needs backing.
    pub fn retains_on_ingest(&self, sequence: SequenceNumber) -> Result<bool, RxReorderError> {
        let distance = self.next_sequence.forward_distance(sequence);
        if distance >= SequenceNumber::HALF_SPACE {
            return Ok(false);
        }
        if distance < self.window && self.has_sequence(sequence) {
            return Err(RxReorderError::DuplicateSequence(sequence));
        }
        let distance_after_advance = if distance >= self.window {
            // `new` refuses a zero window, so `window - 1` does not wrap.
            self.window.wrapping_sub(1)
        } else {
            distance
        };
        Ok(distance_after_advance != 0)
    }

    /// Consume the common in-order MPDU without building a release list.
    ///
    /// A buffered successor still requires [`Self::ingest`], because the
    /// missing MPDU at the window start must release the complete run in
    /// sequence order. A gap, a stale MPDU or a window advance likewise
    /// returns `None` without changing the buffer.
    pub fn try_ingest_immediate(
        &mut self,
        frame: RxReorderMpdu,
    ) -> Result<Option<RxReorderMpdu>, RxReorderError> {
        self.check_received_slot(frame.slot)?;
        if frame.sequence != self.next_sequence {
            return Ok(None);
        }
        let successor = self.next_sequence.wrapping_add(1);
        if self.has_sequence(successor) {
            return Ok(None);
        }
        self.head = self.relative_index(1);
        self.next_sequence = successor;
        Ok(Some(frame))
    }

    /// Accept one received MPDU (IEEE Std 802.11-2020 10.25.6.4): release
    /// what the window passes and the run from its start.
    #[inline(always)]
    pub fn ingest(
        &mut self,
        frame: RxReorderMpdu,
    ) -> Result<RxReorderRelease<WINDOW_CAPACITY>, RxReorderError> {
        self.check_received_slot(frame.slot)?;
        let distance = self.next_sequence.forward_distance(frame.sequence);
        if distance >= SequenceNumber::HALF_SPACE {
            let mut release = RxReorderRelease::empty();
            release.rejected = Some(frame);
            return Ok(release);
        }
        if self.has_sequence(frame.sequence) {
            return Err(RxReorderError::DuplicateSequence(frame.sequence));
        }
        let advance = if distance >= self.window {
            // 1 <= window <= distance < 2048, so this is 1..=2047 and does not wrap.
            distance.wrapping_sub(self.window).wrapping_add(1)
        } else {
            0
        };
        let start = self.next_sequence.wrapping_add(advance);
        let contiguous = self.contiguous_from(start, Some(frame.sequence));
        let span = advance.wrapping_add(contiguous);
        let released = distance < span;
        let position = if released {
            self.head
        } else {
            let next = self.next_sequence.wrapping_add(span);
            self.relative_index(span.wrapping_add(next.forward_distance(frame.sequence)))
        };
        let mut release = RxReorderRelease::empty();
        // Split both arrays around the chosen place before mutation. Keeping
        // that pair borrowed lets us transfer an old frame at the same ring
        // position before installing its successor, with no second lookup.
        let invalid = || RxReorderError::InvalidPosition {
            position,
            capacity: WINDOW_CAPACITY,
        };
        let (out_before, out_tail) = release
            .frames
            .split_at_mut_checked(position)
            .ok_or_else(invalid)?;
        let (out, out_after) = out_tail.split_first_mut().ok_or_else(invalid)?;
        let (owned_before, owned_tail) = self
            .frames
            .split_at_mut_checked(position)
            .ok_or_else(invalid)?;
        let (owned, owned_after) = owned_tail.split_first_mut().ok_or_else(invalid)?;
        let range = ReleaseRange {
            start: self.next_sequence,
            advance,
            span,
        };
        let mut missing = advance;
        for (index, (output, retained)) in
            out_before.iter_mut().zip(owned_before).enumerate().chain(
                out_after
                    .iter_mut()
                    .zip(owned_after)
                    .enumerate()
                    .map(|(index, pair)| (position.wrapping_add(1).wrapping_add(index), pair)),
            )
        {
            range.transfer(index, output, retained, &mut missing, &mut self.occupied);
        }
        range.transfer(position, out, owned, &mut missing, &mut self.occupied);
        if released {
            *out = Some(frame);
        } else {
            *owned = Some(frame);
            self.occupied |= Self::bit(position);
            release.buffered = true;
        }
        release.missing = missing;
        let original_start = self.next_sequence;
        self.head = self.relative_index(span);
        self.next_sequence = original_start.wrapping_add(span);
        release.order(original_start);
        Ok(release)
    }

    /// Release the first buffered run after the caller's gap timer expired.
    ///
    /// The scan is bounded by the negotiated window. It never reads time or
    /// waits; the caller decides when this edge is due.
    pub fn expire_gap(&mut self) -> RxReorderRelease<WINDOW_CAPACITY> {
        for distance in 0..self.window {
            let start = self.next_sequence.wrapping_add(distance);
            if self.has_sequence(start) {
                let contiguous = self.contiguous_from(start, None);
                return self.finish_into(distance, contiguous, RxReorderRelease::empty());
            }
        }
        RxReorderRelease::empty()
    }

    /// Move the window start to a received BlockAckReq's starting sequence.
    ///
    /// Every buffered MPDU before `starting_sequence` is released in order,
    /// then the contiguous run from the new start. A starting sequence equal
    /// to the current start or behind it (in the backward half of the
    /// sequence space) is ignored and returns `None`.
    ///
    /// SOURCE(esp32s31): `libnet80211.a::ieee80211_process_bar_info` ignores those two
    /// cases and otherwise calls `libnet80211[ieee80211_ht.o]::ampdu_dispatch_upto(ni, rx, ssn)`.
    /// The contiguous release after the move is the recipient behaviour of
    /// IEEE Std 802.11-2020 10.25.6.6.
    pub fn move_window_to(
        &mut self,
        starting_sequence: SequenceNumber,
    ) -> Option<RxReorderRelease<WINDOW_CAPACITY>> {
        let distance = self.next_sequence.forward_distance(starting_sequence);
        if distance == 0 || distance >= SequenceNumber::HALF_SPACE {
            return None;
        }
        let contiguous = self.contiguous_from(starting_sequence, None);
        Some(self.finish_into(distance, contiguous, RxReorderRelease::empty()))
    }

    /// End the agreement: release every buffered MPDU in sequence order.
    pub fn stop(&mut self) -> RxReorderRelease<WINDOW_CAPACITY> {
        let mut release = RxReorderRelease {
            frames: core::mem::replace(&mut self.frames, [None; WINDOW_CAPACITY]),
            ..RxReorderRelease::empty()
        };
        self.occupied = 0;
        release.order(self.next_sequence);
        release
    }

    fn check_received_slot(&self, slot: u8) -> Result<(), RxReorderError> {
        self.check_slot(slot)?;
        if self.frames.iter().flatten().any(|owned| owned.slot == slot) {
            return Err(RxReorderError::SlotAlreadyOwned(slot));
        }
        Ok(())
    }

    const fn check_slot(&self, slot: u8) -> Result<(), RxReorderError> {
        if SLOT_CAPACITY == 0 || SLOT_CAPACITY > SLOT_DOMAIN || slot as usize >= SLOT_CAPACITY {
            Err(RxReorderError::InvalidSlot(slot))
        } else {
            Ok(())
        }
    }

    /// Plan the complete run before changing any retained slot. Each
    /// examined sequence is distinct: window <= 64 < the sequence space.
    fn contiguous_from(&self, start: SequenceNumber, received: Option<SequenceNumber>) -> u16 {
        (0..self.window)
            .take_while(|offset| {
                let sequence = start.wrapping_add(*offset);
                received == Some(sequence) || self.has_sequence(sequence)
            })
            .count() as u16
    }

    /// Commit a timer/BAR transition without an incoming MPDU. Old frames
    /// transfer one-to-one into the same positions of the result array.
    fn finish_into(
        &mut self,
        advance: u16,
        contiguous: u16,
        mut release: RxReorderRelease<WINDOW_CAPACITY>,
    ) -> RxReorderRelease<WINDOW_CAPACITY> {
        let start = self.next_sequence;
        // advance < 2048 and contiguous <= 64: this span fits u16 and
        // never wraps the 4096-value sequence space while selecting frames.
        let span = advance.wrapping_add(contiguous);
        let range = ReleaseRange {
            start: self.next_sequence,
            advance,
            span,
        };
        let mut missing = advance;
        for (index, (released, retained)) in release
            .frames
            .iter_mut()
            .zip(self.frames.iter_mut())
            .enumerate()
        {
            range.transfer(index, released, retained, &mut missing, &mut self.occupied);
        }
        release.missing = missing;
        self.head = self.relative_index(span);
        self.next_sequence = start.wrapping_add(span);
        release.order(start);
        release
    }

    fn has_sequence(&self, sequence: SequenceNumber) -> bool {
        let distance = self.next_sequence.forward_distance(sequence);
        if distance >= self.window {
            return false;
        }
        let index = self.relative_index(distance);
        self.occupied & Self::bit(index) != 0
            && self
                .frames
                .get(index)
                .and_then(Option::as_ref)
                .is_some_and(|frame| frame.sequence == sequence)
    }

    /// The occupancy bit of a physical position below WINDOW_CAPACITY.
    const fn bit(index: usize) -> u64 {
        // index < WINDOW_CAPACITY <= 64, the width of the bitmap, so the
        // shift is in range.
        1_u64.wrapping_shl(index as u32)
    }

    fn relative_index(&self, distance: u16) -> usize {
        // The const constructor enforces nonempty geometry; the divisor's
        // type also makes remainder non-panicking at runtime.
        let modulus = const {
            match NonZeroUsize::new(WINDOW_CAPACITY) {
                Some(modulus) => modulus,
                None => panic!("a reorder window is nonempty"),
            }
        };
        self.head.wrapping_add(usize::from(distance)) % modulus
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "reorder/slot_tests.rs"]
mod slot_tests;
