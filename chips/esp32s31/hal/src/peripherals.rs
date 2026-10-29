//! Ordinary (non-radio) peripheral blocks: each module owns one block's
//! words, split from the reset root's partitions, and exposes its atoms and
//! the family drivers' register ports on them.
//!
//! - [`efuse`]: the factory identity from eFuse;
//! - [`entropy`]: the true random number generator;
//! - [`reset_cause`]: why each HP core last left reset;
//! - [`software_interrupt`]: the FROM_CPU software interrupt lines;
//! - [`system_timer`]: the SYSTIMER counter and comparators;
//! - [`timg0_alarm`]: TIMG0 timer 0 as a one-shot alarm;
//! - [`usb_serial_jtag`]: the USB Serial/JTAG console;
//! - [`watchdog`]: the timer groups' and RTC watchdogs.

pub mod aes;
pub mod efuse;
pub mod entropy;
pub mod reset_cause;
pub mod software_interrupt;
pub mod system_timer;
pub mod timg0_alarm;
pub mod usb_serial_jtag;
pub mod watchdog;
