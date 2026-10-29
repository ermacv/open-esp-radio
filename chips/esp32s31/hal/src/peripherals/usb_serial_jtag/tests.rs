//! The console's parts over host memory from the block's base.
//!
//! Word offsets are ESP-IDF 4d80c1b2 `usb_serial_jtag_reg.h`'s (`EP1` 0x0,
//! `EP1_CONF` 0x4, `INT_ST` 0xc, `INT_ENA` 0x10, `INT_CLR` 0x14, `CONF0`
//! 0x18).

use std::{vec, vec::Vec};

use oer_esp32s31_pac::instances;
use oer_espressif_usb_serial_jtag::{EndpointRegisters, Events, InterruptRegisters};

use super::{ConsoleRegisters, parts};

fn memory(fill: u32) -> Vec<u32> {
    vec![fill; instances::usb_device::SIZE / 4]
}

#[test]
fn the_console_parts_read_what_the_block_shows() {
    let (mut words, mut pads) = (memory(u32::MAX), memory(0));
    let registers =
        ConsoleRegisters::host(&mut words, &mut pads).expect("host memory holds the blocks");
    let (endpoint, mut interrupt, _pads) = parts(registers);
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
fn the_handler_acknowledges_exactly_the_events_it_sampled() {
    let (mut words, mut pads) = (memory(0), memory(0));
    // SOURCE: ESP-IDF 4d80c1b2 `usb_serial_jtag_reg.h`: `SERIAL_IN_EMPTY`
    // bit 3, `SERIAL_OUT_RECV_PKT` bit 2; every other status bit set too.
    words[0xc / 4] = !(1 << 2);
    let registers =
        ConsoleRegisters::host(&mut words, &mut pads).expect("host memory holds the blocks");
    let (_endpoint, mut interrupt, _pads) = parts(registers);
    assert_eq!(
        interrupt.take_events(),
        Events {
            in_empty: true,
            out_received: false
        }
    );
    assert_eq!(words[0x14 / 4], 1 << 3, "only the sampled event is cleared");
}

#[test]
fn a_quiet_block_shows_no_room_no_data_and_no_event() {
    let (mut words, mut pads) = (memory(0), memory(0));
    let registers =
        ConsoleRegisters::host(&mut words, &mut pads).expect("host memory holds the blocks");
    let (endpoint, mut interrupt, _pads) = parts(registers);
    assert!(!endpoint.has_room());
    assert!(!endpoint.has_data());
    assert_eq!(interrupt.take_events(), Events::NONE);
    let _ = (endpoint, interrupt);
    assert!(
        words.iter().all(|word| *word == 0),
        "nothing was acknowledged"
    );
}

#[test]
fn disconnecting_drops_the_pull_ups_and_the_pads() {
    let (mut words, mut pads) = (memory(0), memory(u32::MAX));
    let registers =
        ConsoleRegisters::host(&mut words, &mut pads).expect("host memory holds the blocks");
    let (_endpoint, _interrupt, usb_pads) = parts(registers);
    usb_pads.disconnect();
    // SOURCE: ESP-IDF 4d80c1b2 `usb_serial_jtag_reg.h` (`CONF0`):
    // `PAD_PULL_OVERRIDE` bit 8, `DP_PULLUP` bit 9, `DM_PULLUP` bit 11,
    // `USB_PAD_ENABLE` bit 14 (lines 541-625).
    assert_eq!(pads[0x18 / 4], !((1 << 9) | (1 << 11) | (1 << 14)));
}

#[test]
fn the_interrupt_registers_and_the_installed_parts_can_move_between_contexts() {
    fn handler_static<T: Send + 'static>() {}
    handler_static::<super::ConsoleInterrupt>();
    handler_static::<super::ConsoleRegisters>();
}

#[test]
fn a_duplicate_console_is_a_typed_root_failure_and_retains_the_first_owner() {
    let (mut first, mut first_pads) = (memory(0), memory(0));
    let (mut second, mut second_pads) = (memory(0), memory(0));
    let ownership = super::Ownership::new();
    super::install_in(
        &ownership,
        ConsoleRegisters::host(&mut first, &mut first_pads).unwrap(),
    )
    .unwrap();
    let error = super::install_in(
        &ownership,
        ConsoleRegisters::host(&mut second, &mut second_pads).unwrap(),
    )
    .unwrap_err();
    let error = crate::root::InitError::from(error);
    assert_eq!(error, crate::root::InitError::ConsoleInstalled);
    assert_eq!(error.name().as_str(), "console-installed");
    let (mut endpoint, _, _) = parts(ownership.retire().unwrap());
    endpoint.push(0x42);
    drop(endpoint);
    drop(ownership);
    assert_eq!(first[0], 0x42, "the original owner remains installed");
    assert_eq!(second[0], 0, "the refused owner is never used");
}
