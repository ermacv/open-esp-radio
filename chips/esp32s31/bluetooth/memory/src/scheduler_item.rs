//! Scheduler operations on the generated header shared by every BLE role.
//!
//! `BleSchedulerItem` defines the word positions and fields. This view updates
//! the execution successor, software previous/completion links, window and
//! status through those named cells. The scheduler list reference records the
//! handover protocol and the pinned source bodies.
//!
//! These volatile accesses grant no ownership, ordering or publication
//! authority; callers retain those contracts.

#![forbid(unsafe_code)]

use core::num::NonZeroU32;

use oer_esp32s31_hal::layouts::*;

use crate::sram_link::ControllerSramLinkAddress;

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

/// Scheduler header view of one generated item allocation.
pub(crate) struct SchedulerItemHeader<'item> {
    layout: &'item BleSchedulerItem,
}

impl<'item> SchedulerItemHeader<'item> {
    pub(crate) const fn new(layout: &'item BleSchedulerItem) -> Self {
        Self { layout }
    }

    /// Complete word `+0x00`, including the role allocation bits.
    pub(crate) fn hardware_next_word(&self) -> u32 {
        self.layout.hardware_next().read().bits()
    }

    pub(crate) fn set_hardware_next_word(&self, value: u32) {
        self.layout
            .hardware_next()
            .write(BleSchedulerItemHardwareNext::from_bits(value));
    }

    /// Compressed hardware next link; zero terminates the list.
    pub(crate) fn hardware_next_image(&self) -> u32 {
        self.layout.hardware_next().read().address()
    }

    /// Link `next` while preserving bits 31:20.
    pub(crate) fn link_hardware_next(&self, next: Option<ControllerSramLinkAddress>) {
        let original = self.layout.hardware_next().read();
        let pointer = BleSchedulerItemHardwareNext::from_bits(
            next.map_or(0, ControllerSramLinkAddress::compressed_image),
        );
        self.layout
            .hardware_next()
            .write(original.with_address_from(pointer));
    }

    #[cfg(test)]
    pub(crate) fn sequence_start(&self) -> u32 {
        self.layout.sequence_start().read().bits()
    }

    #[cfg(test)]
    pub(crate) fn sequence_duration(&self) -> u32 {
        self.layout.sequence_duration().read().bits()
    }

    /// Store the sequence projection of `[start, end)` with the scheduler
    /// `lead`, as the vendor sequence-time function does.
    pub(crate) fn set_sequence(&self, start: u32, end: u32, lead: u32) {
        self.layout
            .sequence_start()
            .write(BleSchedulerItemSequenceStart::from_bits(
                start.wrapping_add(lead),
            ));
        self.layout
            .sequence_duration()
            .write(BleSchedulerItemSequenceDuration::from_bits(
                end.wrapping_sub(start),
            ));
    }

    pub(crate) fn status(&self) -> u32 {
        self.layout.status().read().bits()
    }

    pub(crate) fn set_status(&self, value: u32) {
        self.layout
            .status()
            .write(BleSchedulerItemStatus::from_bits(value));
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

    #[cfg(test)]
    pub(crate) fn raw_start(&self) -> u32 {
        self.layout.raw_start().read().bits()
    }

    pub(crate) fn set_raw_start(&self, value: u32) {
        self.layout
            .raw_start()
            .write(BleSchedulerItemRawStart::from_bits(value));
    }

    #[cfg(test)]
    pub(crate) fn raw_end(&self) -> u32 {
        self.layout.raw_end().read().bits()
    }

    pub(crate) fn set_raw_end(&self, value: u32) {
        self.layout
            .raw_end()
            .write(BleSchedulerItemRawEnd::from_bits(value));
    }

    pub(crate) fn set_control(&self, value: u32) {
        self.layout
            .control()
            .write(BleSchedulerItemControl::from_bits(value));
    }

    /// Clear the role event byte `+0x4c`, as a new role event does.
    pub(crate) fn clear_event_byte(&self) {
        self.layout
            .control()
            .write(self.layout.control().read().with_event(0));
    }

    /// Prepare the item for list insertion as the vendor reset and merge do
    /// together: clear the skip marker, scheduler byte `+0x4e` and the list
    /// state of `+0x4f`, mark the item unexecuted and install both links.
    pub(crate) fn prepare_for_list(
        &self,
        previous: Option<ControllerSramLinkAddress>,
        next: Option<ControllerSramLinkAddress>,
    ) {
        self.link_hardware_next(next);
        self.layout
            .hardware_next()
            .write(self.layout.hardware_next().read().with_skip(false));
        self.layout.control().write(
            self.layout
                .control()
                .read()
                .with_bit_16_opaque(false)
                .with_bits_17_23_opaque(0)
                .expect("zero fits")
                .with_bit_24_opaque(false)
                .with_deleted(false)
                .with_preempted(false),
        );
        self.mark_unexecuted();
        self.link_previous(previous);
    }

    /// Mark the item deleted as vendor cancellation does: set the skip
    /// marker and the deleted flag, keeping the hardware next link.
    pub(crate) fn mark_deleted(&self) {
        self.layout
            .hardware_next()
            .write(self.layout.hardware_next().read().with_skip(true));
        self.layout
            .control()
            .write(self.layout.control().read().with_deleted(true));
    }

    /// Store the full address of the previous item, or zero for the head.
    pub(crate) fn link_previous(&self, previous: Option<ControllerSramLinkAddress>) {
        self.set_previous(previous.map_or(0, |previous| previous.controller_address().address()));
    }

    /// Full address of the previous item; zero when the item is the head.
    #[cfg(test)]
    pub(crate) fn previous(&self) -> u32 {
        self.layout.previous().read().bits()
    }

    pub(crate) fn set_previous(&self, address: u32) {
        self.layout
            .previous()
            .write(BleSchedulerItemPrevious::from_bits(address));
    }

    #[cfg(test)]
    pub(crate) fn completion_link(&self) -> u32 {
        self.layout.completion_next().read().bits()
    }

    pub(crate) fn set_completion_link(&self, address: u32) {
        self.layout
            .completion_next()
            .write(BleSchedulerItemCompletionNext::from_bits(address));
    }
}

#[cfg(test)]
mod tests;
