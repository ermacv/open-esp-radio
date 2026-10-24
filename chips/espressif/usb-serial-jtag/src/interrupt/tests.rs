//! The interrupt's registers are installed once, and the handler reads
//! nothing before.

use std::boxed::Box;

use oer_interrupt_table::signal::host::Deferred;

use super::{AlreadyInstalled, Interrupt};
use crate::fake::{Irq, Shared};

fn interrupt() -> &'static Interrupt<Deferred, Irq> {
    Box::leak(Box::new(Interrupt::new()))
}

#[test]
fn the_handler_reads_nothing_before_its_registers_are_installed() {
    let interrupt = interrupt();
    assert!(!interrupt.service());
}

#[test]
fn the_registers_are_installed_once() {
    let interrupt = interrupt();
    let block = Shared::default();
    assert!(interrupt.install(Irq(block.clone())).is_ok());
    assert_eq!(
        interrupt.install(Irq(block.clone())).err(),
        Some(AlreadyInstalled)
    );
    block.lock().events.in_empty = true;
    assert!(interrupt.service());
    assert_eq!(
        block.lock().acknowledged.len(),
        1,
        "one handler acknowledges"
    );
}

#[test]
fn a_raised_event_waits_on_the_signal_for_the_task() {
    let interrupt = interrupt();
    let block = Shared::default();
    let signal = interrupt
        .install(Irq(block.clone()))
        .expect("a new interrupt takes its registers");
    block.lock().events.out_received = true;
    assert!(interrupt.service());
    assert_eq!(signal.has_pending(), Ok(true), "the task finds the event");
}
