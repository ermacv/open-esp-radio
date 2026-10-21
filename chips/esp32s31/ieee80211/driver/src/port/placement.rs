//! Small runtime attachments to the permanently placed queue and driver state.

use core::{
    cell::Cell,
    ops::{Deref, DerefMut},
};

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

pub(super) struct PreparedCell<T: Copy>(Cell<Option<T>>);
impl<T: Copy> PreparedCell<T> {
    pub(super) const fn vacant() -> Self {
        Self(Cell::new(None))
    }
    pub(super) fn get(&self) -> T {
        self.0.get().expect("registered slot retains its settings")
    }
    pub(super) fn set(&self, value: T) {
        self.0.set(Some(value));
    }
}
