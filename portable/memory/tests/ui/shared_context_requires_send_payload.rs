use oer_memory::{ContextCell, Entered, ExecutionContext};

fn share<T: Sync>() {}

#[cfg(not(ui_reject))]
pub fn check<X: ExecutionContext<Sharing = ()>>() {
    share::<ContextCell<u32, Entered<X>>>();
}

#[cfg(ui_reject)]
pub fn check<X: ExecutionContext<Sharing = ()>>() {
    share::<ContextCell<std::rc::Rc<u32>, Entered<X>>>(); // ui-error: E0277
}
