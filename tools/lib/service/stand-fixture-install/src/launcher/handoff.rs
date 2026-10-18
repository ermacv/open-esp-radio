//! Hand the provider lease from a fixed launcher to its helper across `exec`.
//!
//! The launcher places a non-close-on-exec duplicate at [`LEASE_FD`]; the
//! helper adopts that descriptor exactly once. This module is the only owner
//! of the raw descriptor number and of the helper's launch environment.

use std::{
    fs::File,
    os::fd::{AsFd, AsRawFd as _, FromRawFd as _, IntoRawFd as _, OwnedFd, RawFd},
    sync::atomic::{AtomicBool, Ordering},
};

use rustix::io::{FdFlags, fcntl_dupfd_cloexec, fcntl_setfd};

use crate::Result;

/// Descriptor number agreed between the fixed launchers and their helpers.
const LEASE_FD: RawFd = 9;
pub(super) const BOUND: &str = "OPEN_RADIO_GENERATION_BOUND";
pub(super) const DIRECTORY: &str = "OPEN_RADIO_GENERATION_DIR";

static ADOPTED: AtomicBool = AtomicBool::new(false);

/// Leave a duplicate of `lease` open at [`LEASE_FD`] for the next `exec`.
pub(super) fn pass(lease: impl AsFd) -> Result<()> {
    pass_at(lease, LEASE_FD)
}

fn pass_at(lease: impl AsFd, number: RawFd) -> Result<()> {
    let slot = fcntl_dupfd_cloexec(lease, number)?;
    if slot.as_raw_fd() != number {
        return Err("provider lease descriptor slot is already in use".into());
    }
    fcntl_setfd(&slot, FdFlags::empty())?;
    // The descriptor intentionally outlives this process image.
    let _ = slot.into_raw_fd();
    Ok(())
}

/// Take the launcher marker and the lease descriptor passed by [`pass`].
///
/// Call this first in the helper's `main`, before it starts any thread.
pub(super) fn adopt(expected_marker: &str) -> Result<File> {
    let marker = std::env::var(BOUND).unwrap_or_default();
    if marker != expected_marker {
        return Err("generation helper was not entered through its fixed launcher".into());
    }
    if ADOPTED.swap(true, Ordering::AcqRel) {
        return Err("provider lease was already adopted".into());
    }
    #[allow(
        unsafe_code,
        reason = "the launch environment is private to this handoff"
    )]
    // SAFETY: the helper calls `adopt` before starting threads, so no other
    // thread reads or writes the environment concurrently.
    unsafe {
        std::env::remove_var(BOUND);
        std::env::remove_var(DIRECTORY);
    }
    adopt_at(LEASE_FD)
}

/// Own the inherited descriptor `number` and close it on any later `exec`.
fn adopt_at(number: RawFd) -> Result<File> {
    if std::fs::symlink_metadata(format!("/proc/self/fd/{number}")).is_err() {
        return Err("generation helper did not inherit its provider lease descriptor".into());
    }
    #[allow(
        unsafe_code,
        reason = "adopting the inherited lease asserts descriptor ownership"
    )]
    // SAFETY: the descriptor is open (checked above) and was passed to this
    // process for this owner; `adopt` admits one caller through `ADOPTED`.
    let lease = unsafe { OwnedFd::from_raw_fd(number) };
    fcntl_setfd(&lease, FdFlags::CLOEXEC)?;
    Ok(File::from(lease))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt as _;

    fn close_on_exec(number: RawFd) -> bool {
        let info = std::fs::read_to_string(format!("/proc/self/fdinfo/{number}")).unwrap();
        let flags = info
            .lines()
            .find_map(|line| line.strip_prefix("flags:"))
            .map(|flags| u32::from_str_radix(flags.trim(), 8).unwrap())
            .unwrap();
        flags & rustix::fs::OFlags::CLOEXEC.bits() != 0
    }

    #[test]
    fn lease_survives_exec_until_the_helper_adopts_it() {
        // A high descriptor number avoids the lowest-free allocation of
        // concurrently running tests.
        const NUMBER: RawFd = 900;
        let file = tempfile::tempfile().unwrap();
        pass_at(&file, NUMBER).unwrap();
        assert!(!close_on_exec(NUMBER));
        assert!(pass_at(&file, NUMBER).is_err(), "occupied slot is reused");

        let adopted = adopt_at(NUMBER).unwrap();
        assert!(close_on_exec(NUMBER));
        assert_eq!(
            adopted.metadata().unwrap().ino(),
            file.metadata().unwrap().ino()
        );
        drop(adopted);
        assert!(adopt_at(NUMBER).is_err(), "closed descriptor is adopted");
    }
}
