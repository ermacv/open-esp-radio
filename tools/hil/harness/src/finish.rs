//! Ending a session under its lease: the device under test is left running
//! the built image, brought back through its board's reset ladder, or, when
//! not even its ROM answers after the ladder, reported unreachable: the LAB
//! service, which parks the board when the lease ends, then keeps it out of
//! service for a person.
//!
//! After a rung only another boot counts: the new console answers also a
//! device the rung did not restart.
//!
//! A ROM that answers, booting from flash or waiting for a download, means
//! the stand can write the board again, however bad its image. The cleanup is kept apart from the cases' verdicts:
//! a recovery does not turn a failed case into a pass.

use std::time::Duration;

use oer_hil_link::{CallError, Link};
use oer_hil_protocol::base::GetHello;
use oer_lab_model::rom::{Ladder, LadderEnd, RecoveryStep};

/// What finishing needs from the stand: the board's reset ladder and its
/// console.
pub(crate) trait Board {
    /// Climb the board's reset ladder from its stand file, stopping at the
    /// first rung after which `answers` sees the image.
    fn climb(&self, answers: &mut dyn FnMut() -> bool) -> Ladder;
    /// Reset the board through the first rung of its ladder.
    fn reset(&self) -> Result<(), String>;
    /// A link on the board's console, opened without a reset.
    fn connect(&self) -> Result<Link, String>;
    /// Journal that `step` brought the board back; best effort.
    fn recovered(&self, step: RecoveryStep, reset_line: Option<String>);
}

/// How long finishing waits for the device.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Waits {
    /// One `GetHello` of the running boot.
    pub probe: Duration,
    /// The Hello of a boot after a rung.
    pub hello: Duration,
}

/// How the cases left the device.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Ending {
    /// The last boot has a request whose outcome is unknown.
    pub unconfirmed: bool,
    /// The last boot the cases saw, also when no link shows it any more: a
    /// rung counts only when another boot answers.
    pub last_boot: Option<u64>,
}

/// What finishing did to the device under test.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Cleanup {
    /// It runs the built image and left no request unconfirmed.
    Clean,
    /// The built image answered again after `step` of the ladder.
    Recovered {
        step: RecoveryStep,
        reset_line: Option<String>,
    },
    /// After the ladder the ROM answered and the image did not: a fault of
    /// the image or the host. The stand can write the board again, so it
    /// stays in service.
    ImageFailed { reset_line: String },
    /// Neither the image nor the ROM answered: the board needs a person.
    Unreachable { reason: String },
    /// The program was cancelled before the device answered: nothing about
    /// the board is judged, and the next session resets it.
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
            Self::Recovered { step, reset_line } => Some(format!(
                "the device under test needed {step:?} ({})",
                reset_line.as_deref().unwrap_or("no reset line")
            )),
            Self::ImageFailed { reset_line } => Some(format!(
                "after the reset ladder the ROM answered ({reset_line}) but the image did not"
            )),
            Self::Unreachable { reason } => Some(format!("the board needs a person: {reason}")),
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

/// Finish with the device behind `link` that should run image
/// `image_crc32`: probe it, and climb `board`'s ladder when a request is
/// unconfirmed or the image does not answer. The link that ends the
/// session, when the device answers on one.
pub(crate) fn finish(
    board: &dyn Board,
    link: Option<Link>,
    image_crc32: u32,
    ending: Ending,
    waits: Waits,
) -> (Option<Link>, Cleanup) {
    let mut last = ending.last_boot;
    if let Some(link) = link {
        if !ending.unconfirmed && answers(&link, image_crc32, waits) {
            return (Some(link), Cleanup::Clean);
        }
        // The probe may have seen a later boot.
        last = link.running_boot_id().or(last);
        // The rungs need the console the link holds.
        drop(link);
    }
    if oer_os_process::cancellation_requested() {
        return (None, Cleanup::Cancelled);
    }
    let mut found = None;
    let ladder = board.climb(&mut || {
        let Ok(link) = board.connect() else {
            return false;
        };
        let answered = link.boot(waits.hello);
        let before = last;
        // A frame names its boot also when the boot's Hello does not come.
        last = link.running_boot_id().or(last);
        let Ok(boot) = answered else {
            return false;
        };
        let restarted = before != Some(boot.boot_id);
        let answered = restarted && boot.hello.image_crc32 == image_crc32;
        if answered {
            found = Some(link);
        }
        answered
    });
    let cleanup = judge(board, &ladder);
    (found, cleanup)
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

/// What `ladder` means for the board.
fn judge(board: &dyn Board, ladder: &Ladder) -> Cleanup {
    let last = ladder.steps.last();
    match &ladder.end {
        LadderEnd::Cleared => {
            let Some(last) = last else {
                return Cleanup::Clean;
            };
            let reset_line = last.outcome.clone().ok().flatten();
            board.recovered(last.step, reset_line.clone());
            Cleanup::Recovered {
                step: last.step,
                reset_line,
            }
        }
        _ if oer_os_process::cancellation_requested() => Cleanup::Cancelled,
        LadderEnd::Loadable { reset_line } => Cleanup::ImageFailed {
            reset_line: reset_line.clone(),
        },
        LadderEnd::Silent => {
            let steps = ladder
                .steps
                .iter()
                .map(|step| format!("{:?}", step.step))
                .collect::<Vec<_>>();
            let reason = format!(
                "neither its image nor its ROM answered after {}",
                if steps.is_empty() {
                    "an empty reset ladder".to_owned()
                } else {
                    steps.join(", ")
                }
            );
            Cleanup::Unreachable { reason }
        }
    }
}
#[cfg(test)]
mod tests;
