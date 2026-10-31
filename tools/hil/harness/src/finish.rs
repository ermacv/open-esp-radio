//! Ending a session: whether the device under test still runs the built
//! image and left no request unconfirmed. The board's hardware end is the
//! LAB service's alone: when the lease ends it brings the board to its end
//! state (its ROM's download mode) and reports what it could not, so the
//! harness climbs no reset ladder of its own. The cleanup is kept apart
//! from the cases' verdicts: it keeps a pass from passing and never turns a
//! failed case into a pass.

use oer_lab_client::protocol::RouterOperation;
use std::time::Duration;

use oer_hil_link::{CallError, Link};
use oer_hil_protocol::base::GetHello;

/// What a case needs from the stand's board: a reset into its image and its
/// console.
pub(crate) trait Board {
    /// Reset the board through the first reset its stand file lists.
    fn reset(&self) -> Result<(), String>;
    /// A link on the board's console, opened without a reset.
    fn connect(&self) -> Result<Link, String>;
    /// Run `request` on the lab router under the run's lease.
    fn router(&self, request: RouterOperation) -> Result<serde_json::Value, String>;
}

/// How long finishing waits for the device.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Waits {
    /// One `GetHello` of the running boot.
    pub probe: Duration,
}

/// How the cases left the device.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Ending {
    /// The last boot has a request whose outcome is unknown.
    pub unconfirmed: bool,
}

/// How the device under test ended the session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Cleanup {
    /// It runs the built image and left no request unconfirmed.
    Clean,
    /// A request of its last boot has no known outcome.
    Unconfirmed,
    /// The built image did not answer at the end.
    Unanswered,
    /// The program was cancelled before the device was checked.
    Cancelled,
    /// The LAB service ended the lease without confirming the board's end
    /// state, after `cleanup`.
    Unreleased {
        cleanup: Box<Cleanup>,
        reason: String,
    },
}

impl Cleanup {
    /// Why the cleanup keeps a passing scenario from passing; `None` when
    /// the device was left as the cases found it.
    pub fn problem(&self) -> Option<String> {
        match self {
            Self::Clean => None,
            Self::Unconfirmed => {
                Some("a request of the device's last boot has no known outcome".to_owned())
            }
            Self::Unanswered => Some("the built image did not answer at the end".to_owned()),
            Self::Cancelled => Some("cancelled before the device was checked".to_owned()),
            Self::Unreleased { cleanup, reason } => Some(format!(
                "{}; the LAB service ended the lease with {reason}",
                cleanup
                    .problem()
                    .unwrap_or_else(|| "the device was left clean".to_owned())
            )),
        }
    }

    /// This cleanup, then the end of the lease the LAB service reported.
    pub fn released(self, ended: Result<(), String>) -> Self {
        match ended {
            Ok(()) => self,
            Err(reason) => Self::Unreleased {
                cleanup: Box::new(self),
                reason,
            },
        }
    }
}

/// Finish with the device behind `link`, which should run image
/// `image_crc32`: probe it once. The link stays with the session until the
/// lease ends.
pub(crate) fn finish(
    link: Option<&Link>,
    image_crc32: u32,
    ending: Ending,
    waits: Waits,
) -> Cleanup {
    if oer_os_process::cancellation_requested() {
        return Cleanup::Cancelled;
    }
    let Some(link) = link else {
        return Cleanup::Unanswered;
    };
    if !answers(link, image_crc32, waits) {
        return Cleanup::Unanswered;
    }
    if ending.unconfirmed {
        return Cleanup::Unconfirmed;
    }
    Cleanup::Clean
}

/// Whether the running boot of `link` answers `GetHello` as the built
/// image; a reboot since the last case is followed once.
fn answers(link: &Link, image_crc32: u32, waits: Waits) -> bool {
    let mut boot = match link.boot(waits.probe) {
        Ok(boot) => boot,
        Err(_) => return false,
    };
    for _ in 0..2 {
        match link.call(&boot, GetHello, waits.probe) {
            Ok(hello) => return hello.image_crc32 == image_crc32,
            Err(CallError::Rebooted { boot_id }) => {
                boot = match link.boot(waits.probe) {
                    Ok(next) if next.boot_id == boot_id => next,
                    _ => return false,
                };
            }
            Err(_) => return false,
        }
    }
    false
}

#[cfg(test)]
mod tests;
