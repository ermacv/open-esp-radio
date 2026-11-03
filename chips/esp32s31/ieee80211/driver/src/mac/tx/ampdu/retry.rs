//! Retention of a detached batch's kept frames, compacted in order.

use core::pin::Pin;

use super::{HtAmpduLength, HtAmpduTxError, HtAmpduTxStorage, TxCookie, TxDmaState};

impl<const SLOTS: usize> HtAmpduTxStorage<SLOTS> {
    /// Keep the frames at the positions of `keep`, at least one, compacted
    /// in their order to positions `0..k`: the detached batch is reserved
    /// again under its cookie, its length recalculated.
    pub(super) fn retain_frames(
        mut self: Pin<&mut Self>,
        cookie: TxCookie,
        keep: u64,
    ) -> Result<HtAmpduLength, HtAmpduTxError> {
        let storage = self.as_ref().get_ref();
        if storage.state != TxDmaState::Completed || storage.active != cookie || !storage.detached {
            return Err(HtAmpduTxError::Stale);
        }
        let count = storage.count;
        check_frame_mask(keep, count)?;
        let storage = self.as_mut().project();
        let mut destination = 0_usize;
        for source in 0..usize::from(count) {
            let selected = u32::try_from(source)
                .ok()
                .and_then(|source| keep.checked_shr(source))
                .is_some_and(|bits| bits & 1 != 0);
            if !selected {
                continue;
            }
            if destination != source {
                shift(storage.buffer_addresses, source, destination);
                shift(storage.frame_lengths, source, destination);
                shift(storage.hardware_mic_lengths, source, destination);
                shift(storage.msdu_lengths, source, destination);
                shift(storage.psdu_lengths, source, destination);
                shift(storage.hardware_he_control, source, destination);
                shift(storage.empty_delimiters, source, destination);
                shift(storage.descriptor_capacities, source, destination);
            }
            destination = destination.saturating_add(1);
        }
        *storage.count = u8::try_from(destination)
            .map_err(|_| HtAmpduTxError::SlotCountOverflow { count: destination })?;
        *storage.prepared_length = 0;
        *storage.aggregate_length = 0;
        *storage.trigger_reservation = None;
        *storage.trigger_publication_snapshot = None;
        *storage.detached = false;
        *storage.state = TxDmaState::Reserved;
        self.recalculate_prepared_length()
    }
}

/// Refuse a mask of no frame, or naming a position at or above `count`.
pub(super) fn check_frame_mask(keep: u64, count: u8) -> Result<(), HtAmpduTxError> {
    if keep == 0 {
        return Err(HtAmpduTxError::TooFewFrames);
    }
    if keep.checked_shr(u32::from(count)).unwrap_or(0) != 0 {
        let highest = (u64::BITS - 1).saturating_sub(keep.leading_zeros());
        return Err(HtAmpduTxError::FrameIndexOutOfRange {
            index: u8::try_from(highest).unwrap_or(u8::MAX),
            count,
        });
    }
    Ok(())
}

/// Copy the entry at `source` to `destination`; positions outside `values`
/// change nothing.
fn shift<T: Copy>(values: &mut [T], source: usize, destination: usize) {
    if let Some(value) = values.get(source).copied()
        && let Some(target) = values.get_mut(destination)
    {
        *target = value;
    }
}
