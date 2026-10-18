//! Cooperative cancellation with an explicit, scoped cleanup exception.

use crate::Result;
use std::{
    cell::Cell,
    error::Error,
    fmt,
    time::{Duration, Instant},
};

#[derive(Debug)]
pub struct Cancelled;
impl fmt::Display for Cancelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("command cancelled by signal")
    }
}
impl Error for Cancelled {}

pub fn is_cancelled(error: &(dyn Error + 'static)) -> bool {
    let mut cause = Some(error);
    while let Some(error) = cause {
        if error.is::<Cancelled>() {
            return true;
        }
        cause = error.source();
    }
    false
}

pub fn check_cancelled() -> Result<()> {
    if cleanup_deadline().is_some_and(|deadline| Instant::now() >= deadline) {
        return Err(crate::owned::DeadlineExceeded.into());
    }
    if crate::cancellation_requested() {
        Err(Cancelled.into())
    } else {
        Ok(())
    }
}

pub fn sleep(duration: Duration) -> Result<()> {
    let deadline = Instant::now() + duration;
    loop {
        check_cancelled()?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        std::thread::sleep(remaining.min(Duration::from_millis(20)));
    }
}

thread_local! { static CLEANUP: Cell<Option<Instant>> = const { Cell::new(None) }; }
pub(crate) fn in_cleanup() -> bool {
    CLEANUP.get().is_some()
}
pub fn cleanup_deadline() -> Option<Instant> {
    CLEANUP.get()
}

/// Cleanup may run despite cancellation. Its commands still need deadlines.
/// The previous policy is restored even if the closure unwinds.
pub fn cleanup<T>(action: impl FnOnce() -> T) -> T {
    struct Restore(Option<Instant>);
    impl Drop for Restore {
        fn drop(&mut self) {
            CLEANUP.set(self.0);
        }
    }
    let deadline = cleanup_deadline().unwrap_or_else(|| Instant::now() + Duration::from_secs(30));
    let _restore = Restore(CLEANUP.replace(Some(deadline)));
    action()
}
