//! Filter accept list commands.
//!
//! The backend owns the list: each Clear, Add or Remove command becomes one
//! `RadioRequest::FilterAcceptList` and completes with the backend's answer.
//! A full list answers Memory Capacity Exceeded and removing an absent device
//! Invalid HCI Command Parameters, as the vendor Controller does. HCI Reset clears a list that
//! may hold devices the same way before it completes.

use bt_hci::param::{Error as HciError, Status};
use oer_bluetooth_radio::{AcceptListChange, RequestError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Idle,
    Changing {
        change: AcceptListChange,
        sent: bool,
    },
}

// CAPABILITY: bluetooth-filter-accept-list-operation
pub(crate) struct AcceptList {
    phase: Phase,
    completion: Option<Status>,
    /// A device may be listed: an addition succeeded since the last clear.
    populated: bool,
}

impl AcceptList {
    pub(crate) const fn new() -> Self {
        Self {
            phase: Phase::Idle,
            completion: None,
            populated: false,
        }
    }

    /// Whether a change waits for the backend.
    pub(crate) const fn is_active(&self) -> bool {
        matches!(self.phase, Phase::Changing { .. })
    }

    /// Start one change; its status arrives through [`Self::take_completion`].
    pub(crate) fn change(&mut self, change: AcceptListChange) {
        self.phase = Phase::Changing {
            change,
            sent: false,
        };
        self.completion = None;
    }

    /// Clear the list for HCI Reset when it may hold a device.
    pub(crate) fn reset(&mut self) {
        if self.populated {
            self.change(AcceptListChange::Clear);
        }
    }

    pub(crate) fn wants_radio(&self) -> bool {
        matches!(self.phase, Phase::Changing { sent: false, .. })
    }

    /// The change the backend makes next.
    pub(crate) fn request(&mut self) -> Option<AcceptListChange> {
        match &mut self.phase {
            Phase::Changing {
                change,
                sent: sent @ false,
            } => {
                *sent = true;
                Some(*change)
            }
            _ => None,
        }
    }

    pub(crate) fn request_done(&mut self, result: Result<(), RequestError>) {
        if let (Phase::Changing { change, .. }, Ok(())) = (self.phase, result) {
            match change {
                AcceptListChange::Add(_) => self.populated = true,
                AcceptListChange::Clear => self.populated = false,
                AcceptListChange::Remove(_) => {}
            }
        }
        self.phase = Phase::Idle;
        self.completion = Some(match result {
            Ok(()) => Status::SUCCESS,
            Err(RequestError::ListFull) => HciError::MEMORY_CAPACITY_EXCEEDED.to_status(),
            Err(RequestError::NotListed) => HciError::INVALID_HCI_PARAMETERS.to_status(),
            Err(_) => HciError::HARDWARE_FAILURE.to_status(),
        });
    }

    pub(crate) fn take_completion(&mut self) -> Option<Status> {
        self.completion.take()
    }
}
