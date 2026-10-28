use oer_memory::ExecutionContext;

#[cfg(not(ui_reject))]
pub fn check<X: ExecutionContext>() -> bool { X::running() }

#[cfg(ui_reject)]
struct Forged;
#[cfg(ui_reject)]
impl ExecutionContext for Forged { // ui-error: E0200
    type Sharing = ();
    fn running() -> bool { true }
}
