//! The handler raises exactly the events it takes, through the part its
//! entry lends it.

use std::boxed::Box;

use oer_interrupt_table::signal::host::Deferred;

use super::Interrupt;
use crate::fake::{Irq, Shared};

fn interrupt() -> &'static Interrupt<Deferred, Irq> {
    Box::leak(Box::new(Interrupt::new()))
}

#[test]
fn a_handler_without_events_raises_nothing() {
    let interrupt = interrupt();
    let block = Shared::default();
    interrupt.service(&mut Irq(block.clone()));
    assert_eq!(interrupt.signal().has_pending(), Ok(false));
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
    block.lock().events.out_received = true;
    interrupt.service(&mut Irq(block.clone()));
    assert_eq!(
        interrupt.signal().has_pending(),
        Ok(true),
        "the task finds the event"
    );
}
