#![no_std]
use oer_espressif_executor::{Binding, InterruptExecutor, Round, Tasks, task::Interrupt};
use oer_espressif_interrupt_table::{Controller, ExecutorEntry, InterruptSource, Router};
use oer_espressif_staged_runtime::Hart;

/// An interrupt-mode executor starts only the task set whose round its
/// entry's handler is: the entry of `Polled`'s round starts `Polled`, never
/// another set of the same executor.
pub fn start<B, Polled, Other, W, C>(
    executor: InterruptExecutor<B, 2>,
    hart: &Hart<0>,
    router: &mut Router<C>,
    wake: W,
    part: W::Part,
) where
    B: Binding,
    Polled: Tasks<Binding = B, Mode = Interrupt>,
    Other: Tasks<Binding = B, Mode = Interrupt>,
    W: ExecutorEntry<Handler = Round<Polled>>,
    W::Part: InterruptSource,
    C: Controller,
{
    #[cfg(not(ui_reject))]
    let _ = executor.start::<Polled, W, C, 0, ()>(hart, router, wake, part, ());
    #[cfg(ui_reject)]
    let _ = executor.start::<Other, W, C, 0, ()>(hart, router, wake, part, ()); // ui-error: E0271
}
