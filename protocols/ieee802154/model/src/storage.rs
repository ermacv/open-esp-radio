//! Private ownership transfers. Callers run no user code while a value is out.
use core::cell::Cell;

#[derive(Default)]
pub(crate) struct Local<T>(Cell<T>);

impl<T: Default> Local<T> {
    // Only inspect plain values or move owners here. In particular, clone,
    // drop and wake user Wakers after returning removed owners from this call.
    pub fn with<R>(&self, operation: impl FnOnce(&mut T) -> R) -> R {
        let mut value = self.0.take();
        let result = operation(&mut value);
        self.0.set(value);
        result
    }
}
