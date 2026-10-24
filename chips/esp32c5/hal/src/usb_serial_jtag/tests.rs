//! The HAL's console parts over host memory forward to the PAC's owners.

use std::{vec, vec::Vec};

use oer_espressif_usb_serial_jtag::{EndpointRegisters, Events, InterruptRegisters};

use super::{UsbSerialJtagRegisters, console_parts};

/// Enough host memory for either of the console's blocks.
const BLOCK_BYTES: usize = 0x40;

fn memory(fill: u32) -> Vec<u32> {
    vec![fill; BLOCK_BYTES / 4]
}

#[test]
fn the_console_parts_read_what_the_block_shows() {
    let (mut words, mut pads) = (memory(u32::MAX), memory(0));
    let registers =
        UsbSerialJtagRegisters::host(&mut words, &mut pads).expect("host memory holds the blocks");
    let (endpoint, mut interrupt, _pads) = console_parts(registers);
    assert!(endpoint.has_room());
    assert!(endpoint.has_data());
    assert_eq!(
        interrupt.take_events(),
        Events {
            in_empty: true,
            out_received: true
        }
    );
}

#[test]
fn a_quiet_block_shows_no_room_no_data_and_no_event() {
    let (mut words, mut pads) = (memory(0), memory(0));
    let registers =
        UsbSerialJtagRegisters::host(&mut words, &mut pads).expect("host memory holds the blocks");
    let (endpoint, mut interrupt, _pads) = console_parts(registers);
    assert!(!endpoint.has_room());
    assert!(!endpoint.has_data());
    assert_eq!(interrupt.take_events(), Events::NONE);
}

#[test]
fn the_interrupt_registers_can_move_into_the_handlers_static() {
    fn handler_static<T: Send + 'static>() {}
    handler_static::<super::UsbInterrupt>();
}
