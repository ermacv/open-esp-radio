//! The radio doorbell's raising half and the static reply signal P2 awaits.

use core::task::{Context, Poll};
use oer_esp32s31_hal::bluetooth::engine::scheduler::interrupt::{
    SchedulerDoorbell as HalSchedulerDoorbell, SchedulerInterruptError,
};
use oer_espressif_interrupt_table::signal::{IrqSignal, SignalExecutor};

/// The composition's shared FROM_CPU line 3 radio doorbell, at P3/core 0.
pub trait RadioDoorbell {
    const HANDLER: oer_espressif_interrupt_table::Context;
    const EXECUTOR: oer_espressif_interrupt_table::Context;
    fn ring(&self);
}

/// P2 can pend the radio doorbell and wait for its durable reply notification.
/// The notification has its own signal; scheduler and modem-timer wake cells
/// retain their independent facts and consumers.
pub struct SchedulerDoorbell<X: SignalExecutor, D> {
    doorbell: D,
    reply: &'static IrqSignal<X>,
}

impl<X: SignalExecutor, D> SchedulerDoorbell<X, D> {
    pub const fn new(doorbell: D, reply: &'static IrqSignal<X>) -> Self {
        Self { doorbell, reply }
    }
}

impl<X: SignalExecutor, D: RadioDoorbell> HalSchedulerDoorbell for SchedulerDoorbell<X, D> {
    fn ring(&self) {
        self.doorbell.ring();
    }
    fn poll_reply(&self, cx: &mut Context<'_>) -> Poll<Result<(), SchedulerInterruptError>> {
        self.reply.poll_take(cx).map(|result| {
            result
                .map(|_| ())
                .map_err(|_| SchedulerInterruptError::ForeignContext)
        })
    }
}

#[cfg(test)]
mod tests;
