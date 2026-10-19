//! Scheduler-owned header of every Controller scheduler item.
//!
//! Each role allocation places the same scheduler fields at the same offsets;
//! the role codecs own every other word. This module is the only definition of
//! those offsets and of the field transforms the scheduler performs. The
//! contracts come from the vendor scheduler list bodies described in the
//! scheduler list reference of the vendor investigation.
//!
//! | Byte offset | Field |
//! | --- | --- |
//! | `+0x00` bits 19:0 | Compressed hardware next link; bit 25 is the skip marker; the other upper bits belong to the role allocation |
//! | `+0x0c`, `+0x10` | Sequence start (raw start plus the scheduler lead) and window duration |
//! | `+0x38` | Execution status; [`SCHEDULER_ITEM_UNEXECUTED`] until hardware records a result |
//! | `+0x44`, `+0x48` | Raw start and end of the scheduled window |
//! | `+0x4c` | Role event byte, role kind byte `+0x4d`, scheduler bytes `+0x4e` and `+0x4f`; `+0x4f` bit 1 marks a deleted item |
//! | `+0x50` | Full address of the previous item in the list |
//! | `+0x54` | Full address link of the software completion queue |
//!
//! The header performs plain volatile word accesses. It grants no ownership,
//! ordering or publication authority; callers keep those contracts.

#![forbid(unsafe_code)]

use core::num::NonZeroU32;

use vcell::VolatileCell;

use crate::sram_link::ControllerSramLinkAddress;

const HARDWARE_NEXT_WORD: usize = 0;
const SEQUENCE_START_WORD: usize = 0x0c / 4;
const SEQUENCE_DURATION_WORD: usize = 0x10 / 4;
const STATUS_WORD: usize = 0x38 / 4;
const RAW_START_WORD: usize = 0x44 / 4;
const RAW_END_WORD: usize = 0x48 / 4;
const CONTROL_WORD: usize = 0x4c / 4;
const PREVIOUS_WORD: usize = 0x50 / 4;
const COMPLETION_LINK_WORD: usize = 0x54 / 4;

const COMPRESSED_LINK_MASK: u32 = 0x000f_ffff;
const SKIP_MARKER: u32 = 1 << 25;
const EVENT_BYTE_MASK: u32 = 0x0000_00ff;
const SCHEDULER_BYTE_4E_MASK: u32 = 0x00ff_0000;
/// `+0x4f` bits 2:0: the deleted and preempted state of a list entry.
const LIST_STATE_4F_MASK: u32 = 0x0700_0000;
/// `+0x4f` bit 1.
const DELETED_4F: u32 = 0x0200_0000;

/// Status of an item that hardware has not executed.
pub const SCHEDULER_ITEM_UNEXECUTED: u32 = u32::MAX;

/// Status that hardware recorded for an executed item.
///
/// The value is positional; its meaning belongs to the role that owns the
/// item.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerItemCompletionStatus {
    /// Status zero.
    Zero,
    /// A nonzero status, retained exactly.
    NonZero(NonZeroU32),
}

/// Volatile word access to one scheduler-item allocation.
pub(crate) trait SchedulerItemWords {
    fn word(&self, index: usize) -> u32;
    fn set_word(&self, index: usize, value: u32);
}

impl SchedulerItemWords for [VolatileCell<u32>] {
    fn word(&self, index: usize) -> u32 {
        self[index].get()
    }

    fn set_word(&self, index: usize, value: u32) {
        self[index].set(value);
    }
}

impl<const WORDS: usize> SchedulerItemWords for [VolatileCell<u32>; WORDS] {
    fn word(&self, index: usize) -> u32 {
        self[index].get()
    }

    fn set_word(&self, index: usize, value: u32) {
        self[index].set(value);
    }
}

/// Scheduler header view of one item allocation.
pub(crate) struct SchedulerItemHeader<'item, W: ?Sized> {
    words: &'item W,
}

impl<'item, W: SchedulerItemWords + ?Sized> SchedulerItemHeader<'item, W> {
    pub(crate) const fn new(words: &'item W) -> Self {
        Self { words }
    }

    /// Complete word `+0x00`, including the role allocation bits.
    pub(crate) fn hardware_next_word(&self) -> u32 {
        self.words.word(HARDWARE_NEXT_WORD)
    }

    pub(crate) fn set_hardware_next_word(&self, value: u32) {
        self.words.set_word(HARDWARE_NEXT_WORD, value);
    }

    /// Compressed hardware next link; zero terminates the list.
    pub(crate) fn hardware_next_image(&self) -> u32 {
        self.hardware_next_word() & COMPRESSED_LINK_MASK
    }

    /// Link `next` while preserving bits 31:20.
    pub(crate) fn link_hardware_next(&self, next: Option<ControllerSramLinkAddress>) {
        let image = next.map_or(0, ControllerSramLinkAddress::compressed_image);
        let word = self.hardware_next_word();
        self.set_hardware_next_word((word & !COMPRESSED_LINK_MASK) | image);
    }

    #[cfg(test)]
    pub(crate) fn sequence_start(&self) -> u32 {
        self.words.word(SEQUENCE_START_WORD)
    }

    #[cfg(test)]
    pub(crate) fn sequence_duration(&self) -> u32 {
        self.words.word(SEQUENCE_DURATION_WORD)
    }

    /// Store the sequence projection of `[start, end)` with the scheduler
    /// `lead`, as the vendor sequence-time function does.
    pub(crate) fn set_sequence(&self, start: u32, end: u32, lead: u32) {
        self.words
            .set_word(SEQUENCE_START_WORD, start.wrapping_add(lead));
        self.words
            .set_word(SEQUENCE_DURATION_WORD, end.wrapping_sub(start));
    }

    pub(crate) fn status(&self) -> u32 {
        self.words.word(STATUS_WORD)
    }

    pub(crate) fn set_status(&self, value: u32) {
        self.words.set_word(STATUS_WORD, value);
    }

    /// Mark the item unexecuted before it is linked into a list.
    pub(crate) fn mark_unexecuted(&self) {
        self.set_status(SCHEDULER_ITEM_UNEXECUTED);
    }

    /// The recorded status, or `None` while the item is unexecuted.
    pub(crate) fn completion_status(&self) -> Option<SchedulerItemCompletionStatus> {
        match self.status() {
            SCHEDULER_ITEM_UNEXECUTED => None,
            status => Some(NonZeroU32::new(status).map_or(
                SchedulerItemCompletionStatus::Zero,
                SchedulerItemCompletionStatus::NonZero,
            )),
        }
    }

    pub(crate) fn raw_start(&self) -> u32 {
        self.words.word(RAW_START_WORD)
    }

    pub(crate) fn set_raw_start(&self, value: u32) {
        self.words.set_word(RAW_START_WORD, value);
    }

    pub(crate) fn raw_end(&self) -> u32 {
        self.words.word(RAW_END_WORD)
    }

    pub(crate) fn set_raw_end(&self, value: u32) {
        self.words.set_word(RAW_END_WORD, value);
    }

    /// Complete control word `+0x4c`.
    pub(crate) fn control(&self) -> u32 {
        self.words.word(CONTROL_WORD)
    }

    pub(crate) fn set_control(&self, value: u32) {
        self.words.set_word(CONTROL_WORD, value);
    }

    /// Clear the role event byte `+0x4c`, as a new role event does.
    pub(crate) fn clear_event_byte(&self) {
        self.set_control(self.control() & !EVENT_BYTE_MASK);
    }

    /// Prepare the item for list insertion as the vendor reset and merge do
    /// together: clear the skip marker, scheduler byte `+0x4e` and the list
    /// state of `+0x4f`, mark the item unexecuted and install both links.
    pub(crate) fn prepare_for_list(
        &self,
        previous: Option<ControllerSramLinkAddress>,
        next: Option<ControllerSramLinkAddress>,
    ) {
        let word = self.hardware_next_word() & !(COMPRESSED_LINK_MASK | SKIP_MARKER);
        self.set_hardware_next_word(
            word | next.map_or(0, ControllerSramLinkAddress::compressed_image),
        );
        self.set_control(self.control() & !(SCHEDULER_BYTE_4E_MASK | LIST_STATE_4F_MASK));
        self.mark_unexecuted();
        self.link_previous(previous);
    }

    /// Mark the item deleted as vendor cancellation does: set the skip
    /// marker and the deleted flag, keeping the hardware next link.
    pub(crate) fn mark_deleted(&self) {
        self.set_hardware_next_word(self.hardware_next_word() | SKIP_MARKER);
        self.set_control(self.control() | DELETED_4F);
    }

    /// Store the full address of the previous item, or zero for the head.
    pub(crate) fn link_previous(&self, previous: Option<ControllerSramLinkAddress>) {
        self.set_previous(previous.map_or(0, |previous| previous.controller_address().address()));
    }

    /// Full address of the previous item; zero when the item is the head.
    #[cfg(test)]
    pub(crate) fn previous(&self) -> u32 {
        self.words.word(PREVIOUS_WORD)
    }

    pub(crate) fn set_previous(&self, address: u32) {
        self.words.set_word(PREVIOUS_WORD, address);
    }

    #[cfg(test)]
    pub(crate) fn completion_link(&self) -> u32 {
        self.words.word(COMPLETION_LINK_WORD)
    }

    pub(crate) fn set_completion_link(&self, address: u32) {
        self.words.set_word(COMPLETION_LINK_WORD, address);
    }
}

#[cfg(test)]
mod tests;
