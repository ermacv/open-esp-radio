#![no_std]
//! A start keeps the poll record of the task it starts: the record is typed
//! by the task's start, whose poll instance it names, so the record of a
//! task with another start does not fit.
use core::future::{Pending, Ready};
use core::task::{Context, RawWakerVTable};
use oer_espressif_executor::task::{
    Interrupt, PollBoundary, PollToken, Task, Wakes, waker_vtable,
};
use oer_espressif_executor::{SoftwareInterrupt, Spawner, Tasks};

pub struct Binding;
static VTABLE: RawWakerVTable = waker_vtable::<Binding>();
impl Wakes for Binding {
    fn vtable() -> &'static RawWakerVTable {
        &VTABLE
    }
    fn pend_task(_: SoftwareInterrupt) {}
}

pub struct Set;
static FIRST: Task<fn() -> Ready<()>, Set> = Task::new(0);
static FIRST_RECORD: PollBoundary<fn() -> Ready<()>> = Task::<fn() -> Ready<()>, Set>::BOUNDARY;
static SECOND: Task<fn() -> Pending<()>, Set> = Task::new(1);
static SECOND_RECORD: PollBoundary<fn() -> Pending<()>> =
    Task::<fn() -> Pending<()>, Set>::BOUNDARY;
impl Tasks for Set {
    type Binding = Binding;
    type Mode = Interrupt;
    const EXECUTOR: u8 = 2;
    const COUNT: u8 = 2;
    fn poll(index: u8, token: &PollToken<Self>, cx: &mut Context<'_>) {
        match index {
            0 => FIRST.poll(token, cx),
            _ => SECOND.poll(token, cx),
        }
    }
}

pub fn start_both(spawner: &Spawner<Set>) {
    let _ = spawner.start(&FIRST, &FIRST_RECORD, || core::future::ready(()));
    #[cfg(not(ui_reject))]
    let _ = spawner.start(&SECOND, &SECOND_RECORD, core::future::pending::<()>);
    #[cfg(ui_reject)]
    let _ = spawner.start(&SECOND, &FIRST_RECORD, core::future::pending::<()>); // ui-error: E0308
}
