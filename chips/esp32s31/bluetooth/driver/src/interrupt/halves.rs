//! Stable P3 register custody and the static reply signal used by P2.

use core::task::{Context, Poll};
use oer_esp32s31_hal::bluetooth::engine::scheduler::interrupt::{
    SchedulerDoorbell as HalSchedulerDoorbell, SchedulerInterruptError, SchedulerInterruptHalf,
};
use oer_esp32s31_hal::bluetooth::{InterruptOutputAfterRoutesOwner, InterruptRegistersOwner};
use oer_interrupt_table::signal::{IrqSignal, SignalExecutor};

/// The composition's shared FROM_CPU line 3 radio doorbell, at P3/core 0.
pub trait RadioDoorbell {
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

/// One P3 half for the primary, NRT and shared radio doorbell handlers.
/// The image keeps it in stable handler storage. It never lends registers
/// to the scheduler task and contains no arena or task transaction state.
#[must_use]
pub struct ControllerInterruptHalf<X: SignalExecutor> {
    registers: InterruptRegistersOwner,
    scheduler: SchedulerInterruptHalf,
    reply: &'static IrqSignal<X>,
}

impl<X: SignalExecutor> ControllerInterruptHalf<X> {
    pub fn new(
        registers: InterruptRegistersOwner,
        scheduler: SchedulerInterruptHalf,
        reply: &'static IrqSignal<X>,
    ) -> Self {
        Self {
            registers,
            scheduler,
            reply,
        }
    }

    pub fn service_primary(&mut self, clock: &dyn oer_time::Clock) -> super::PrimaryInterruptStep {
        super::primary::step_primary_interrupt(&mut self.registers, clock)
    }

    pub fn service_nrt(&mut self) -> super::NrtDefaultInterruptEpoch {
        super::nrt::step_nrt_default_interrupt(&mut self.registers)
    }

    /// The shared handler acknowledges FROM_CPU first, then visits each
    /// installed radio half once. A later ring remains pending for its next
    /// entry. Only readiness ORs; the channel owns the exact affine reply.
    pub fn service_doorbell(&mut self, clock: &dyn oer_time::Clock) -> bool {
        let serviced = self.scheduler.service(&mut self.registers, clock);
        if serviced {
            self.reply.raise(1);
        }
        serviced
    }

    /// After removing the routes, return both owners for stopped retirement.
    pub fn deactivate(self) -> (InterruptOutputAfterRoutesOwner, SchedulerInterruptHalf) {
        (self.registers.deactivate(), self.scheduler)
    }
}

#[cfg(test)]
mod tests;
