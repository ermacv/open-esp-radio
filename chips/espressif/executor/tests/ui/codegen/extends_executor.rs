// The executor's task module with a synthetic binding: a set implements
// `Extends` by hand for a set of another executor (`ui_reject`) or of its
// own, and converts its start right to the base's.
#[path = "__SRC__/software_line.rs"]
mod software_line;
pub use software_line::SoftwareInterrupt;
#[path = "__SRC__/task.rs"]
pub mod task;

use task::{Extends, PollToken, Spawner, Tasks, Thread};

/// What the module's `chip` items name, synthetic.
pub trait ChipBinding: 'static {
    fn waker_vtable() -> &'static core::task::RawWakerVTable;
}
pub use ChipBinding as Binding;
mod executor {
    pub(crate) fn ready(_: u8) -> Option<&'static crate::task::Ready> {
        None
    }
    pub(crate) fn pend_task<B: crate::Binding>(_: crate::SoftwareInterrupt) {}
}

pub struct Chip;
impl ChipBinding for Chip {
    fn waker_vtable() -> &'static core::task::RawWakerVTable {
        const VTABLE: core::task::RawWakerVTable = core::task::RawWakerVTable::new(
            |data| core::task::RawWaker::new(data, &VTABLE),
            |_| {},
            |_| {},
            |_| {},
        );
        &VTABLE
    }
}

pub struct Base;
impl Tasks for Base {
    type Binding = Chip;
    type Mode = Thread;
    const EXECUTOR: u8 = 0;
    const COUNT: u8 = 1;
    fn poll(_: u8, _: &PollToken<Self>, _: &mut core::task::Context<'_>) {}
}

pub struct Extending;
impl Tasks for Extending {
    type Binding = Chip;
    type Mode = Thread;
    #[cfg(ui_reject)]
    const EXECUTOR: u8 = 1;
    #[cfg(not(ui_reject))]
    const EXECUTOR: u8 = 0;
    const COUNT: u8 = 2;
    fn poll(_: u8, _: &PollToken<Self>, _: &mut core::task::Context<'_>) {}
}
impl Extends<Base> for Extending {}

pub fn base(spawner: &Spawner<Extending>) -> Spawner<Base> {
    spawner.base()
}
