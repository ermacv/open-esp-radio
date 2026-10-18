//! Cooperative control plane for one long-lived monitor task.

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use embassy_sync::{blocking_mutex::raw::RawMutex, signal::Signal};

const MONITOR_IDLE: u8 = 0;
const MONITOR_RUNNING: u8 = 1;
const MONITOR_STOPPED: u8 = 2;
const MONITOR_FAULTED: u8 = 3;
const NO_ENDPOINTS: u8 = 0;
const MONITOR_ENDPOINTS: u8 = 2;

/// A faulted control domain is sticky and cannot manufacture another task
/// endpoint from storage whose hardware owner was not returned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorControlError {
    InUse,
    Faulted,
}

/// Terminal state acknowledged by the task which owns IRQ and DMA.
///
/// `Stopped` means both the interrupt epoch and RX walker were confirmed
/// inactive. `Faulted` means the task retained the hardware owner for an
/// board-level recovery policy; it never means that active resources were
/// released or that the adapter requested reset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorCompletion {
    Stopped,
    Faulted,
}

impl MonitorCompletion {
    const fn encode(self) -> u8 {
        match self {
            Self::Stopped => MONITOR_STOPPED,
            Self::Faulted => MONITOR_FAULTED,
        }
    }

    const fn decode(value: u8) -> Option<Self> {
        match value {
            MONITOR_STOPPED => Some(Self::Stopped),
            MONITOR_FAULTED => Some(Self::Faulted),
            _ => None,
        }
    }
}

/// Static mailbox shared by one application handle and one monitor task.
///
/// Hardware is deliberately absent from this value. Dropping the controller
/// cannot affect IRQ routing, DMA descriptors or their backing storage.
pub struct MonitorControlResources<M: RawMutex> {
    endpoints: AtomicU8,
    stop_requested: AtomicBool,
    completion: AtomicU8,
    command_wake: Signal<M, ()>,
    completion_wake: Signal<M, ()>,
}

impl<M: RawMutex> MonitorControlResources<M> {
    pub const fn new() -> Self {
        Self {
            endpoints: AtomicU8::new(NO_ENDPOINTS),
            stop_requested: AtomicBool::new(false),
            completion: AtomicU8::new(MONITOR_IDLE),
            command_wake: Signal::new(),
            completion_wake: Signal::new(),
        }
    }

    /// Begin one monitor task epoch.
    ///
    /// An endpoint lease proves that two command consumers cannot be created
    /// for the same mailbox. Both returned endpoints must be dropped before a
    /// later clean monitor epoch can reuse static control storage.
    pub fn split(
        &self,
    ) -> Result<(MonitorController<'_, M>, MonitorCommandReceiver<'_, M>), MonitorControlError>
    {
        self.ensure_reusable()?;
        if self
            .endpoints
            .compare_exchange(
                NO_ENDPOINTS,
                MONITOR_ENDPOINTS,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return Err(MonitorControlError::InUse);
        }
        Ok(self.split_checked())
    }

    pub(crate) fn ensure_reusable(&self) -> Result<(), MonitorControlError> {
        if self.completion.load(Ordering::Acquire) == MONITOR_FAULTED {
            Err(MonitorControlError::Faulted)
        } else {
            Ok(())
        }
    }

    fn split_checked(&self) -> (MonitorController<'_, M>, MonitorCommandReceiver<'_, M>) {
        debug_assert!(self.ensure_reusable().is_ok());
        self.stop_requested.store(false, Ordering::Release);
        self.completion.store(MONITOR_RUNNING, Ordering::Release);
        self.command_wake.reset();
        self.completion_wake.reset();
        let resources = self;
        (
            MonitorController { resources },
            MonitorCommandReceiver {
                resources,
                completion_published: false,
            },
        )
    }

    fn release_endpoint(&self) {
        let previous = self.endpoints.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous != NO_ENDPOINTS);
    }
}

impl<M: RawMutex> Default for MonitorControlResources<M> {
    fn default() -> Self {
        Self::new()
    }
}

/// Application-side handle for a monitor task.
///
/// A stop request is only publication. [`stop`](Self::stop) additionally waits
/// for the task to confirm that no ISR or DMA walker owns role resources.
pub struct MonitorController<'resources, M: RawMutex> {
    resources: &'resources MonitorControlResources<M>,
}

impl<M: RawMutex> Drop for MonitorController<'_, M> {
    fn drop(&mut self) {
        self.resources.release_endpoint();
    }
}

impl<M: RawMutex> MonitorController<'_, M> {
    pub fn request_stop(&self) -> bool {
        if self.resources.completion.load(Ordering::Acquire) != MONITOR_RUNNING {
            return false;
        }
        if self.resources.stop_requested.swap(true, Ordering::AcqRel) {
            return false;
        }
        self.resources.command_wake.signal(());
        true
    }

    pub async fn wait_completion(&mut self) -> MonitorCompletion {
        loop {
            if let Some(completion) =
                MonitorCompletion::decode(self.resources.completion.load(Ordering::Acquire))
            {
                return completion;
            }
            self.resources.completion_wake.wait().await;
        }
    }

    pub async fn stop(&mut self) -> MonitorCompletion {
        self.request_stop();
        self.wait_completion().await
    }
}

/// Single-consumer endpoint owned by the monitor task.
pub struct MonitorCommandReceiver<'resources, M: RawMutex> {
    resources: &'resources MonitorControlResources<M>,
    completion_published: bool,
}

impl<'resources, M: RawMutex> MonitorCommandReceiver<'resources, M> {
    pub(crate) const fn resources(&self) -> &'resources MonitorControlResources<M> {
        self.resources
    }

    pub async fn wait_stop(&mut self) {
        loop {
            if self.resources.stop_requested.load(Ordering::Acquire) {
                return;
            }
            self.resources.command_wake.wait().await;
        }
    }

    pub(crate) fn stop_requested(&self) -> bool {
        self.resources.stop_requested.load(Ordering::Acquire)
    }

    pub(crate) fn complete(&mut self, completion: MonitorCompletion) {
        if self.completion_published {
            return;
        }
        self.resources
            .completion
            .store(completion.encode(), Ordering::Release);
        self.completion_published = true;
        self.resources.completion_wake.signal(());
    }
}

impl<M: RawMutex> Drop for MonitorCommandReceiver<'_, M> {
    fn drop(&mut self) {
        if !self.completion_published {
            self.complete(MonitorCompletion::Faulted);
        }
        self.resources.release_endpoint();
    }
}

#[cfg(test)]
mod tests;
