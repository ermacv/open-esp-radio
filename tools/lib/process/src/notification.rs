//! Wake blocking clients from a normal thread after signal cancellation.

use std::sync::{Arc, Mutex, OnceLock, Weak};

type Wake = dyn Fn() + Send + Sync;
fn listeners() -> &'static Mutex<Vec<Weak<Wake>>> {
    static LISTENERS: OnceLock<Mutex<Vec<Weak<Wake>>>> = OnceLock::new();
    LISTENERS.get_or_init(Mutex::default)
}

/// Keep a cancellation notification registered for this guard's lifetime.
/// Callbacks run outside signal context and must return promptly.
pub struct CancellationNotification {
    _wake: Arc<Wake>,
}

pub fn notify_on_cancel(wake: impl Fn() + Send + Sync + 'static) -> CancellationNotification {
    let wake: Arc<Wake> = Arc::new(wake);
    {
        let mut listeners = listeners()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        listeners.retain(|listener| listener.strong_count() != 0);
        listeners.push(Arc::downgrade(&wake));
    }
    if crate::cancellation_requested() {
        wake();
    }
    CancellationNotification { _wake: wake }
}

pub(crate) fn notify() {
    let callbacks: Vec<_> = listeners()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .filter_map(Weak::upgrade)
        .collect();
    for callback in callbacks {
        callback();
    }
}
