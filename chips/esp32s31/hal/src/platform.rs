//! The platform's chip bindings, split from the reset root's partitions:
//! the PAC's hart geometry, core control ([`core_control`]), each hart's
//! interrupt grant and controller ([`interrupt_controller`]), which the
//! interrupt table's router drives, and the family platform contracts
//! ([`Port`]).

pub use core_control::{CoreControlRegisters, Uart0ClockRegisters};
pub use interrupt_controller::{
    ForeignHart, Hart0Grant, Hart1Controller, Hart1Grant, InterruptBlocks, InterruptController,
    InterruptGrant, RouteBank,
};
pub use port::Port;

pub mod core_control;
pub mod interrupt_controller;
pub(crate) mod port;

pub use oer_esp32s31_pac::{BOOT_HART, HARTS, Hart, SecondaryHart};
