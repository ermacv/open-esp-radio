//! Small runtime attachments to the permanently placed queue and driver state.

use core::ops::{Deref, DerefMut};

pub(super) struct Prepared<T>(Option<T>);
impl<T> Prepared<T> {
    pub(super) const fn vacant() -> Self {
        Self(None)
    }
    pub(super) fn prepare(&mut self, value: T) {
        self.0 = Some(value);
    }
}
impl<T> Deref for Prepared<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.0
            .as_ref()
            .expect("registered slot retains its attachment")
    }
}
impl<T> DerefMut for Prepared<T> {
    fn deref_mut(&mut self) -> &mut T {
        self.0
            .as_mut()
            .expect("registered slot retains its attachment")
    }
}
