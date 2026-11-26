//! What a spawned child does between fork and exec. `Command::pre_exec`
//! runs only async-signal-safe system calls here, on values the closure owns.

use std::os::fd::{AsFd, OwnedFd};
use std::process::Command;

/// The child writes no core dump.
pub fn without_core_dumps(command: &mut Command) {
    // SAFETY: after fork the closure only calls setrlimit with a fixed value;
    // it allocates nothing and takes no lock.
    unsafe {
        std::os::unix::process::CommandExt::pre_exec(command, || {
            rustix::process::setrlimit(
                rustix::process::Resource::Core,
                rustix::process::Rlimit {
                    current: Some(0),
                    maximum: Some(0),
                },
            )
            .map_err(std::io::Error::from)
        });
    }
}

/// The child inherits `descriptors` across exec. The command owns them, so
/// they stay open until it is dropped: drop it after spawning to close the
/// parent's copies.
pub fn inherit(command: &mut Command, descriptors: Vec<OwnedFd>) {
    // SAFETY: after fork the closure only clears close-on-exec on descriptors
    // it owns; it allocates nothing and takes no lock.
    unsafe {
        std::os::unix::process::CommandExt::pre_exec(command, move || {
            for descriptor in &descriptors {
                rustix::io::fcntl_setfd(descriptor.as_fd(), rustix::io::FdFlags::empty())?;
            }
            Ok(())
        });
    }
}

/// The child leads a new session and process group of its own, so a signal
/// to that group reaches it and its descendants only.
pub fn new_session(command: &mut Command) {
    // SAFETY: after fork the closure only calls setsid on the new child,
    // which is not a group leader yet; it allocates nothing and takes no lock.
    unsafe {
        std::os::unix::process::CommandExt::pre_exec(command, || {
            rustix::process::setsid()
                .map(|_| ())
                .map_err(std::io::Error::from)
        });
    }
}

/// Ignore terminal and broken-pipe signals in a freshly forked helper.
///
/// # Safety
/// Call only in the helper child: it must keep running after the owner dies.
/// No handler is installed; `signal` is async-signal-safe, and allocates and
/// locks nothing.
pub(crate) unsafe fn ignore_terminal_signals() {
    for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGPIPE] {
        // SAFETY: this is the freshly forked helper, the one thread of its
        // process. `signal` is a call a forked child may make; `SIG_IGN` is
        // a disposition, not a handler, so no code of this process runs by
        // it. Each of the four is a signal that can be ignored, which is all
        // `signal` refuses, so its answer says nothing here.
        unsafe { libc::signal(signal, libc::SIG_IGN) };
    }
}
