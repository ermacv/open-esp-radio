//! The console's owners over host memory: each test fills the blocks with
//! one pattern and observes the words through the generated readers, never
//! where a word lies or which bit a field holds.

use std::{vec, vec::Vec};

use super::{UsbEvents, UsbSerialJtagRegisters};
use crate::generated::{usb_serial_jtag, usb_serial_jtag_pads};

fn memory(size: usize, fill: u32) -> Vec<u32> {
    vec![fill; size / 4]
}

/// Both blocks over host memory, each filled with its own pattern.
fn owners<'a>(
    endpoint: &'a mut [u32],
    pads: &'a mut [u32],
) -> UsbSerialJtagRegisters<oer_mmio::Host<'a>> {
    UsbSerialJtagRegisters::host(endpoint, pads).expect("host memory holds the blocks")
}

#[test]
fn a_push_writes_the_byte_alone_and_a_pop_takes_one() {
    let mut words = memory(usb_serial_jtag::SIZE, u32::MAX);
    let mut reference = words.clone();
    let mut pads = memory(usb_serial_jtag_pads::SIZE, 0);
    let (mut endpoint, _, _) = owners(&mut words, &mut pads).into_parts();
    endpoint.push(0x5a);
    assert_eq!(endpoint.pop(), 0x5a, "the FIFO word holds the byte");
    drop(endpoint);
    usb_serial_jtag::UsbSerialJtag::host(&mut reference)
        .expect("host block")
        .push_ep1(0x5a);
    assert_eq!(words, reference, "nothing beside the byte is written");
}

#[test]
fn sending_triggers_the_packet_and_writes_no_other_field() {
    let mut words = memory(usb_serial_jtag::SIZE, 0);
    let mut pads = memory(usb_serial_jtag_pads::SIZE, 0);
    let (mut endpoint, _, _) = owners(&mut words, &mut pads).into_parts();
    endpoint.send();
    drop(endpoint);
    let mut reference = memory(usb_serial_jtag::SIZE, 0);
    usb_serial_jtag::UsbSerialJtag::host(&mut reference)
        .expect("host block")
        .trigger_ep1_conf_wr_done();
    assert_eq!(words, reference, "only the trigger is written");
    assert_ne!(
        words,
        memory(usb_serial_jtag::SIZE, 0),
        "a trigger was written"
    );
}

#[test]
fn the_fifo_flags_read_as_the_hardware_shows_them() {
    let mut words = memory(usb_serial_jtag::SIZE, u32::MAX);
    let mut pads = memory(usb_serial_jtag_pads::SIZE, 0);
    let (endpoint, _, _) = owners(&mut words, &mut pads).into_parts();
    assert!(endpoint.has_room());
    assert!(endpoint.has_data());
    let mut words = memory(usb_serial_jtag::SIZE, 0);
    let mut pads = memory(usb_serial_jtag_pads::SIZE, 0);
    let (endpoint, _, _) = owners(&mut words, &mut pads).into_parts();
    assert!(!endpoint.has_room());
    assert!(!endpoint.has_data());
}

#[test]
fn enabling_sets_both_console_interrupts_and_keeps_the_others() {
    let mut words = memory(usb_serial_jtag::SIZE, 0);
    let mut pads = memory(usb_serial_jtag_pads::SIZE, 0);
    let (mut endpoint, _, _) = owners(&mut words, &mut pads).into_parts();
    endpoint.enable_events();
    drop(endpoint);
    let block = usb_serial_jtag::UsbSerialJtag::host(&mut words).expect("host block");
    let enables = block.read_int_ena();
    assert!(enables.serial_in_empty_int_ena());
    assert!(enables.serial_out_recv_pkt_int_ena());

    let mut words = memory(usb_serial_jtag::SIZE, u32::MAX);
    let before = words.clone();
    let mut pads = memory(usb_serial_jtag_pads::SIZE, 0);
    let (mut endpoint, _, _) = owners(&mut words, &mut pads).into_parts();
    endpoint.enable_events();
    drop(endpoint);
    assert_eq!(words, before, "every other enable keeps its value");
}

#[test]
fn disabling_clears_both_console_interrupts_and_keeps_the_others() {
    let mut words = memory(usb_serial_jtag::SIZE, u32::MAX);
    let mut pads = memory(usb_serial_jtag_pads::SIZE, 0);
    let (mut endpoint, _, _) = owners(&mut words, &mut pads).into_parts();
    endpoint.disable_events();
    drop(endpoint);
    let mut disabled = words.clone();
    let block = usb_serial_jtag::UsbSerialJtag::host(&mut disabled).expect("host block");
    let enables = block.read_int_ena();
    assert!(!enables.serial_in_empty_int_ena());
    assert!(!enables.serial_out_recv_pkt_int_ena());
    drop(block);
    let mut pads = memory(usb_serial_jtag_pads::SIZE, 0);
    let (mut endpoint, _, _) = owners(&mut words, &mut pads).into_parts();
    endpoint.enable_events();
    drop(endpoint);
    assert_eq!(
        words,
        memory(usb_serial_jtag::SIZE, u32::MAX),
        "every other enable kept its value"
    );
}

#[test]
fn the_handler_acknowledges_exactly_the_events_it_read() {
    for events in [
        UsbEvents {
            in_empty: true,
            out_received: false,
        },
        UsbEvents {
            in_empty: false,
            out_received: true,
        },
        UsbEvents {
            in_empty: true,
            out_received: true,
        },
    ] {
        let mut words = memory(usb_serial_jtag::SIZE, 0);
        set_status(&mut words, status_word_for(events));
        let mut reference = words.clone();
        usb_serial_jtag::UsbSerialJtag::host(&mut reference)
            .expect("host block")
            .write_int_clr(|clear| {
                clear
                    .set_serial_in_empty_int_clr(events.in_empty)
                    .set_serial_out_recv_pkt_int_clr(events.out_received);
            });
        let mut pads = memory(usb_serial_jtag_pads::SIZE, 0);
        let (_, mut interrupt, _) = owners(&mut words, &mut pads).into_parts();
        assert_eq!(interrupt.take_events(), events);
        drop(interrupt);
        assert_eq!(
            words, reference,
            "the clear word holds exactly the events read"
        );
    }
}

#[test]
fn no_event_writes_no_acknowledgement() {
    let mut words = memory(usb_serial_jtag::SIZE, 0);
    let mut pads = memory(usb_serial_jtag_pads::SIZE, 0);
    let (_, mut interrupt, _) = owners(&mut words, &mut pads).into_parts();
    assert_eq!(interrupt.take_events(), UsbEvents::default());
    drop(interrupt);
    assert_eq!(words, memory(usb_serial_jtag::SIZE, 0));
}

#[test]
fn disconnecting_drops_the_pull_ups_and_the_pads_and_keeps_every_other_bit() {
    let mut words = memory(usb_serial_jtag::SIZE, 0);
    let mut pads = memory(usb_serial_jtag_pads::SIZE, u32::MAX);
    let (_, _, owner) = owners(&mut words, &mut pads).into_parts();
    owner.disconnect();
    let mut expected = memory(usb_serial_jtag_pads::SIZE, u32::MAX);
    usb_serial_jtag_pads::UsbSerialJtagPads::host(&mut expected)
        .expect("host block")
        .modify_conf0(|conf| {
            conf.set_pad_pull_override(true)
                .set_dp_pullup(false)
                .set_dm_pullup(false)
                .set_usb_pad_enable(false);
        });
    assert_eq!(pads, expected);
    let block = usb_serial_jtag_pads::UsbSerialJtagPads::host(&mut pads).expect("host block");
    let conf = block.read_conf0();
    assert!(conf.pad_pull_override(), "software holds the pull-ups");
    assert!(!conf.dp_pullup() && !conf.dm_pullup(), "no pull-up remains");
    assert!(!conf.usb_pad_enable(), "the pads are off");
}

/// The interrupt status word that shows `events`, found by reading back
/// every single-bit word through the generated status reader.
fn status_word_for(events: UsbEvents) -> u32 {
    let mut word = 0;
    for bit in 0..32 {
        let mut words = memory(usb_serial_jtag::SIZE, 0);
        set_status(&mut words, 1 << bit);
        let block = usb_serial_jtag::UsbSerialJtag::host(&mut words).expect("host block");
        let status = block.read_int_st();
        if (events.in_empty && status.serial_in_empty_int_st())
            || (events.out_received && status.serial_out_recv_pkt_int_st())
        {
            word |= 1 << bit;
        }
    }
    word
}

/// Write `value` into the status word, located as the one word a status
/// read observes.
fn set_status(words: &mut [u32], value: u32) {
    let index = status_index();
    words[index] = value;
}

fn status_index() -> usize {
    (0..usb_serial_jtag::SIZE / 4)
        .find(|&index| {
            let mut words = memory(usb_serial_jtag::SIZE, 0);
            words[index] = u32::MAX;
            let block = usb_serial_jtag::UsbSerialJtag::host(&mut words).expect("host block");
            block.read_int_st().serial_in_empty_int_st()
        })
        .expect("one word holds the status")
}
