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

/// The child enters the control group whose `cgroup.procs` is open as
/// `group`, when given, then leads a new session and process group of its
/// own, so a signal to that group reaches it and its descendants only;
/// `terminal`, when given, becomes the session's controlling terminal. The
/// command must not also ask for a process group of its own: a group
/// leader cannot start a session. Entering the group before `exec` puts the
/// command and everything it starts in it.
pub fn new_session(command: &mut Command, group: Option<OwnedFd>, terminal: Option<OwnedFd>) {
    // SAFETY: after fork the closure only writes the fixed text "0" to the
    // control-group descriptor it owns, which moves the calling process,
    // calls setsid on the new child, which is not a group leader yet, and
    // makes the terminal descriptor it owns the session's controlling
    // terminal; it allocates nothing and takes no lock.
    unsafe {
        std::os::unix::process::CommandExt::pre_exec(command, move || {
            if let Some(group) = &group {
                rustix::io::write(group, b"0")?;
            }
            rustix::process::setsid().map_err(std::io::Error::from)?;
            if let Some(terminal) = &terminal {
                rustix::process::ioctl_tiocsctty(terminal)?;
            }
            Ok(())
        });
    }
}

/// Ignore terminal and broken-pipe signals in a freshly forked helper.
///
/// # Safety
/// Call only in the helper child: it must keep running after the owner dies.
/// No handler is installed; these raw system calls allocate and lock nothing.
pub(crate) unsafe fn ignore_terminal_signals() {
    use rustix::runtime::{
        KernelSigaction, KernelSigactionFlags, Signal, kernel_sig_ign, kernel_sigaction,
    };
    for signal in [Signal::INT, Signal::TERM, Signal::HUP, Signal::PIPE] {
        let action = KernelSigaction {
            sa_handler_kernel: kernel_sig_ign(),
            sa_flags: KernelSigactionFlags::RESTART,
            ..KernelSigaction::default()
        };
        // SAFETY: this is the freshly forked helper; SIG_IGN invokes no code,
        // and the action's empty mask and restart flag match libc::signal.
        let _ = unsafe { kernel_sigaction(signal, Some(action)) };
    }
}
