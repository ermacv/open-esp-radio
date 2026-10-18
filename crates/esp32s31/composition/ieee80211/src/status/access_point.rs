//! Application-visible latest state of the supervised AP epoch.

use core::cell::RefCell;

use embassy_sync::{
    blocking_mutex::{Mutex, raw::CriticalSectionRawMutex},
    signal::Signal,
};

use oer_ieee80211_ap::{AP_MAX_CLIENTS, AccessPointServiceStatus, ApPeerStatus};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccessPointStatusSnapshot {
    pub revision: u64,
    pub generation: Option<u32>,
    pub client_limit: u8,
    pub associated: u8,
    pub authorized: u8,
    pub peers: [Option<ApPeerStatus>; AP_MAX_CLIENTS],
}

impl AccessPointStatusSnapshot {
    const INACTIVE: Self = Self {
        revision: 0,
        generation: None,
        client_limit: 0,
        associated: 0,
        authorized: 0,
        peers: [None; AP_MAX_CLIENTS],
    };

    pub const fn active(self) -> bool {
        self.generation.is_some()
    }
}

struct AccessPointStatusChannel {
    snapshot: Mutex<CriticalSectionRawMutex, RefCell<AccessPointStatusSnapshot>>,
    changed: Signal<CriticalSectionRawMutex, ()>,
}

impl AccessPointStatusChannel {
    const fn new() -> Self {
        Self {
            snapshot: Mutex::new(RefCell::new(AccessPointStatusSnapshot::INACTIVE)),
            changed: Signal::new(),
        }
    }

    fn snapshot(&self) -> AccessPointStatusSnapshot {
        self.snapshot.lock(|snapshot| *snapshot.borrow())
    }

    fn publish(&self, generation: Option<u32>, status: Option<AccessPointServiceStatus>) {
        self.snapshot.lock(|snapshot| {
            let previous = *snapshot.borrow();
            let next = match status {
                Some(status) => AccessPointStatusSnapshot {
                    revision: previous.revision.wrapping_add(1),
                    generation,
                    client_limit: status.client_limit.get(),
                    associated: status.associated,
                    authorized: status.authorized,
                    peers: status.peers,
                },
                None => AccessPointStatusSnapshot {
                    revision: previous.revision.wrapping_add(1),
                    ..AccessPointStatusSnapshot::INACTIVE
                },
            };
            *snapshot.borrow_mut() = next;
        });
        self.changed.signal(());
    }
}

static AP_STATUS: AccessPointStatusChannel = AccessPointStatusChannel::new();

/// Single application receiver for AP association and authorization state.
pub struct AccessPointStatus {
    seen_revision: u64,
}

impl AccessPointStatus {
    pub(crate) fn new() -> Self {
        Self {
            seen_revision: AP_STATUS.snapshot().revision,
        }
    }

    pub fn snapshot(&self) -> AccessPointStatusSnapshot {
        AP_STATUS.snapshot()
    }

    pub async fn changed(&mut self) -> AccessPointStatusSnapshot {
        loop {
            let snapshot = AP_STATUS.snapshot();
            if snapshot.revision != self.seen_revision {
                self.seen_revision = snapshot.revision;
                return snapshot;
            }
            AP_STATUS.changed.wait().await;
        }
    }
}

pub(crate) fn publish_access_point_status(
    generation: oer_radio::wifi::RadioSubsystemGeneration,
    status: AccessPointServiceStatus,
) {
    AP_STATUS.publish(Some(generation.value()), Some(status));
}

pub(crate) fn publish_access_point_stopped() {
    AP_STATUS.publish(None, None);
}
