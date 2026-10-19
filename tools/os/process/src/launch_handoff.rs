//! Hand a descriptor from a fixed launcher to its helper across `exec`.
//!
//! The launcher places a non-close-on-exec duplicate at the handoff's
//! descriptor number; the helper adopts that descriptor exactly once, before
//! it starts a thread, and clears the handoff's launch environment. This
//! module is the only owner of the raw descriptor number and of that
//! environment, which is what makes adopting the number sound.

use std::{
    fs::File,
    os::fd::{AsFd, AsRawFd as _, FromRawFd as _, IntoRawFd as _, OwnedFd, RawFd},
    sync::atomic::{AtomicBool, Ordering},
};

use rustix::io::{FdFlags, fcntl_dupfd_cloexec, fcntl_setfd};

use crate::Result;

/// One launcher-to-helper handoff: the descriptor number both sides agree
/// on, the variable whose value marks a helper entered through its launcher,
/// and every variable of the launch environment the helper clears.
#[derive(Clone, Copy, Debug)]
pub struct LaunchHandoff {
    pub descriptor: RawFd,
    pub marker: &'static str,
    pub environment: &'static [&'static str],
}

static ADOPTED: AtomicBool = AtomicBool::new(false);

impl LaunchHandoff {
    /// Leave a duplicate of `lease` open at the handoff's number for the next
    /// `exec`.
    pub fn pass(&self, lease: impl AsFd) -> Result<()> {
        pass_at(lease, self.descriptor)
    }

    /// Take the launcher marker and the descriptor passed by [`Self::pass`].
    ///
    /// Call this first in the helper's `main`, before it starts any thread.
    pub fn adopt(&self, expected_marker: &str) -> Result<File> {
        let marker = std::env::var(self.marker).unwrap_or_default();
        if marker != expected_marker {
            return Err("generation helper was not entered through its fixed launcher".into());
        }
        if ADOPTED.swap(true, Ordering::AcqRel) {
            return Err("provider lease was already adopted".into());
        }
        for variable in self.environment {
            // SAFETY: the helper calls `adopt` before starting threads, so no
            // other thread reads or writes the environment concurrently.
            unsafe { std::env::remove_var(variable) };
        }
        adopt_at(self.descriptor)
    }
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

/// Own the inherited descriptor `number` and close it on any later `exec`.
fn adopt_at(number: RawFd) -> Result<File> {
    if std::fs::symlink_metadata(format!("/proc/self/fd/{number}")).is_err() {
        return Err("generation helper did not inherit its provider lease descriptor".into());
    }
    // SAFETY: the descriptor is open (checked above) and was passed to this
    // process for this owner by `pass`; `adopt` admits one caller through
    // `ADOPTED`, and nothing else in the process names the agreed number.
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
