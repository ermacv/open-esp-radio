//! What a service tells applications of the interface's address.

use core::future::poll_fn;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use core::task::Poll;

use oer_portable_memory::{AtomicWaker, Claim, ClaimCell};
use oer_protocols_network_core::Configuration;

/// The interface's address as a service last published it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Watched {
    /// The address, its version and its lease; `None` once the service
    /// ended, as the lease leaves with its core.
    pub configuration: Option<Configuration>,
}

/// Why a read found nothing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WatchRefused {
    /// No service published here yet: [`crate::Service::watch`] does.
    Empty,
    /// A publication holds the value now, on another hart or below this
    /// caller: it is free again when that call ends.
    Publishing,
    /// Another service publishes here already: this one does not.
    Taken,
}

impl From<Claim> for WatchRefused {
    fn from(claim: Claim) -> Self {
        match claim {
            Claim::Empty => Self::Empty,
            Claim::Lent => Self::Publishing,
            Claim::Kept => Self::Taken,
        }
    }
}

/// Where a service publishes the interface's address at the end of every
/// turn that changed it ([`Core::configuration_changes`]). The service's end,
/// by retirement or drop, ends the watch for good: every read from then on
/// finds no configuration, and the waiter wakes, whatever the value cell
/// holds. One task waits for a change at a time.
///
/// [`Core::configuration_changes`]: oer_protocols_network_core::Core::configuration_changes
pub struct ConfigurationWatch {
    value: ClaimCell<Watched>,
    /// Publications so far.
    changes: AtomicU32,
    /// The service ended: the lease left with its core.
    ended: AtomicBool,
    waker: AtomicWaker,
}

impl ConfigurationWatch {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            value: ClaimCell::new(),
            changes: AtomicU32::new(0),
            ended: AtomicBool::new(false),
            waker: AtomicWaker::new(),
        }
    }

    /// The last publication, or none once the service ended.
    ///
    /// # Errors
    /// [`WatchRefused`].
    pub fn read(&self) -> Result<Watched, WatchRefused> {
        if self.ended.load(Ordering::Acquire) {
            return Ok(Watched {
                configuration: None,
            });
        }
        Ok(self.value.lend(|watched| *watched)?)
    }

    /// The service ended: a flag, a count and a wake, none of which borrows
    /// the value cell.
    pub(crate) fn end(&self) {
        self.ended.store(true, Ordering::Release);
        self.told();
    }

    /// A wrapping count of publications.
    pub fn changes(&self) -> u32 {
        self.changes.load(Ordering::Acquire)
    }

    /// Wait until the count of publications is no longer `seen`, and return
    /// it.
    pub async fn changed(&self, seen: u32) -> u32 {
        poll_fn(|cx| {
            self.waker.register(cx.waker());
            let changes = self.changes();
            if changes == seen {
                Poll::Pending
            } else {
                Poll::Ready(changes)
            }
        })
        .await
    }

    /// The first publication.
    pub(crate) fn begin(&self, watched: Watched) -> Result<(), WatchRefused> {
        self.value
            .install(watched)
            .map_err(|_| WatchRefused::Taken)?;
        self.told();
        Ok(())
    }

    /// A later one.
    pub(crate) fn publish(&self, watched: Watched) -> Result<(), WatchRefused> {
        self.value.lend(|value| *value = watched)?;
        self.told();
        Ok(())
    }

    fn told(&self) {
        let _previous = self.changes.fetch_add(1, Ordering::AcqRel);
        self.waker.wake();
    }
}

impl Default for ConfigurationWatch {
    fn default() -> Self {
        Self::new()
    }
}
