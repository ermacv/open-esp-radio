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
