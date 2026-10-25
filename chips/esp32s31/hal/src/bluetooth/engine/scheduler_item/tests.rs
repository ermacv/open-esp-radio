use core::num::NonZeroU32;

use crate::layouts::*;

use super::{SchedulerItemCompletionStatus, SchedulerItemHeader};
use crate::bluetooth::engine::sram_link::model_link;

fn item() -> BleSchedulerItem {
    BleSchedulerItem::zeroed()
}

#[test]
fn linking_preserves_flags_and_the_unassigned_tail() {
    let words = item();
    let header = SchedulerItemHeader::new(&words);
    words
        .hardware_next()
        .write(BleSchedulerItemHardwareNext::from_bits(0xfff0_0000));
    let next = model_link(0x2f00_0100);

    header.link_hardware_next(Some(next));
    assert_eq!(
        (
            words.hardware_next().read().bits_20_21_opaque(),
            header.hardware_next_address()
        ),
        (3, Some(next.controller_address()))
    );
    assert_eq!(
        header.hardware_next_address(),
        Some(next.controller_address())
    );
    assert_eq!(words.hardware_next().read().bits() >> 20, 0xfff);

    header.link_hardware_next(None);
    assert_eq!(words.hardware_next().read().bits_20_21_opaque(), 3);
    assert_eq!(header.hardware_next_address(), None);
    assert_eq!(words.hardware_next().read().bits() >> 20, 0xfff);
}

#[test]
fn the_sequence_adds_the_lead_and_keeps_the_window_duration() {
    let words = item();
    let header = SchedulerItemHeader::new(&words);
    header.set_sequence(0xffff_fff0, 0x0000_0010, 4);
    assert_eq!(header.sequence_start(), 0xffff_fff4);
    assert_eq!(header.sequence_duration(), 0x20);
    assert_eq!(words.sequence_start().read().bits(), 0xffff_fff4);
    assert_eq!(words.sequence_duration().read().bits(), 0x20);
}

#[test]
fn control_bytes_are_cleared_independently() {
    let words = item();
    let header = SchedulerItemHeader::new(&words);
    let original = BleSchedulerItemControl::from_bits(0x1122_3344);
    words.control().write(original);

    header.clear_event_byte();
    assert_eq!(words.control().read(), original.with_event(0));
}

#[test]
fn status_decodes_the_unexecuted_sentinel_zero_and_nonzero() {
    let words = item();
    let header = SchedulerItemHeader::new(&words);
    header.mark_unexecuted();
    assert_eq!(words.status().read().bits(), u32::MAX);
    assert_eq!(header.completion_status(), None);

    header.set_status(0);
    assert_eq!(
        header.completion_status(),
        Some(SchedulerItemCompletionStatus::Zero)
    );
    header.set_status(5);
    assert_eq!(
        header.completion_status(),
        Some(SchedulerItemCompletionStatus::NonZero(
            NonZeroU32::new(5).expect("test status is nonzero")
        ))
    );
}

#[test]
fn window_and_links_use_their_own_words() {
    let words = item();
    let header = SchedulerItemHeader::new(&words);
    header.set_raw_start(10);
    header.set_raw_end(20);
    header.link_previous(Some(model_link(0x2f00_0200)));
    header.link_completion(Some(model_link(0x2f00_0300)));

    assert_eq!((header.raw_start(), header.raw_end()), (10, 20));
    assert_eq!(
        (
            words.raw_start().read().bits(),
            words.raw_end().read().bits()
        ),
        (10, 20)
    );
    assert_eq!(words.previous().read().bits(), 0x2f00_0200);
    assert_eq!(words.completion_next().read().bits(), 0x2f00_0300);
    assert_eq!(header.previous(), 0x2f00_0200);
    assert_eq!(header.completion_link(), 0x2f00_0300);
}

#[test]
fn list_preparation_resets_the_scheduler_state_and_keeps_role_bits() {
    let words = item();
    let header = SchedulerItemHeader::new(&words);
    words.hardware_next().write(
        BleSchedulerItemHardwareNext::ZERO
            .with_bits_20_21_opaque(3)
            .expect("allocation image fits")
            .with_skip(true),
    );
    header.set_control(0x0f5a_1122);
    header.set_status(0);
    let previous = model_link(0x2f00_0100);
    let next = model_link(0x2f00_0200);

    header.prepare_for_list(Some(previous), Some(next));
    assert_eq!(
        (
            words.hardware_next().read().bits_20_21_opaque(),
            header.hardware_next_address()
        ),
        (3, Some(next.controller_address()))
    );
    // The event and kind bytes stay; `+0x4e` and the `+0x4f` list state clear.
    let control = words.control().read();
    assert_eq!((control.event(), control.kind()), (0x22, 0x11));
    assert!(control.overlap_resolved());
    assert!(
        !control.deleted()
            && !control.preempted()
            && !control.bit_24_opaque()
            && !control.bit_16_opaque()
    );
    assert_eq!(control.bits_17_23_opaque(), 0);
    assert_eq!(header.completion_status(), None);
    assert_eq!(header.previous(), 0x2f00_0100);

    header.prepare_for_list(None, None);
    assert_eq!(header.hardware_next_address(), None);
    assert_eq!(header.previous(), 0);
}

#[test]
fn deletion_marks_the_item_and_keeps_its_next_link() {
    let words = item();
    let header = SchedulerItemHeader::new(&words);
    let next = model_link(0x2f00_0200);
    header.prepare_for_list(None, Some(next));
    header.set_control(0x0000_0011);

    header.mark_deleted();
    assert!(words.hardware_next().read().skip());
    assert_eq!(
        header.hardware_next_address(),
        Some(next.controller_address())
    );
    assert!(words.control().read().deleted());
    assert_eq!(words.control().read().event(), 0x11);
}
