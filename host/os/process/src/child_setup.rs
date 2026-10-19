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

/// The child runs in a new user namespace that maps the caller's id to
/// `outside`, so a test run as root sees an unprivileged user. Call
/// [`new_session`] first if the child also needs its own session.
#[cfg(feature = "test-support")]
pub fn unprivileged_in_user_namespace(command: &mut Command, outside: u32) {
    let uid_map = format!("{outside} 0 1");
    // SAFETY: after fork the closure only calls unshare, single-threaded as
    // the child is, and open, write and close on its own /proc files with
    // buffers formatted before the fork; it allocates nothing and takes no
    // lock.
    unsafe {
        std::os::unix::process::CommandExt::pre_exec(command, move || {
            // The child runs one thread here, so no other thread shares the
            // descriptor table or credentials unshare changes.
            rustix::thread::unshare_unsafe(rustix::thread::UnshareFlags::NEWUSER)?;
            for (path, contents) in [
                (c"/proc/self/setgroups", &b"deny"[..]),
                (c"/proc/self/uid_map", uid_map.as_bytes()),
                (c"/proc/self/gid_map", uid_map.as_bytes()),
            ] {
                let file =
                    rustix::fs::open(path, rustix::fs::OFlags::WRONLY, rustix::fs::Mode::empty())?;
                let written = rustix::io::write(&file, contents)?;
                if written != contents.len() {
                    return Err(std::io::Error::other("a short write to the namespace map"));
                }
            }
            Ok(())
        });
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;
    use std::os::fd::AsRawFd as _;

    #[test]
    fn an_inherited_descriptor_survives_exec_and_closes_with_the_command() {
        let (mut read, write) = std::io::pipe().unwrap();
        let fd = write.as_raw_fd();
        let mut command = std::process::Command::new("sh");
        command.args(["-c", &format!("echo inherited >&{fd}")]);
        super::inherit(&mut command, vec![write.into()]);
        super::without_core_dumps(&mut command);
        let status = command.status().unwrap();
        drop(command);
        assert!(status.success());
        let mut text = String::new();
        read.read_to_string(&mut text).unwrap();
        assert_eq!(text, "inherited\n");
    }
}
