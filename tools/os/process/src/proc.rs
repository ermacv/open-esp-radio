//! Linux process identities and exit fences. `/proc` start time distinguishes
//! recycled PIDs; kernel pidfds observe complete thread-group exit.

/// Open a kernel process identity before handing it to a forked broker.
pub(crate) fn pidfd_open(pid: u32) -> std::io::Result<std::os::fd::OwnedFd> {
    use std::os::fd::FromRawFd as _;
    // SAFETY: pidfd_open takes only a PID and flags, with no memory pointers.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: fd is a new descriptor owned by this call.
    Ok(unsafe { std::os::fd::OwnedFd::from_raw_fd(fd as i32) })
}

/// Observe complete thread-group exit without allocation or locks. A lifetime
/// socket can close earlier during exit_files, so killed holders retain their
/// pidfd until this succeeds. Errors retain exclusion and retry later.
pub(crate) fn process_exited(pidfd: i32) -> bool {
    let mut descriptor = libc::pollfd {
        fd: pidfd,
        events: libc::POLLIN,
        revents: 0,
    };
    let timeout = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: all pointers refer to valid stack storage. A zero timeout makes
    // this async-signal-safe Linux syscall nonblocking. No signal mask is changed.
    let ready = unsafe {
        libc::syscall(
            libc::SYS_ppoll,
            &raw mut descriptor,
            1 as libc::nfds_t,
            &raw const timeout,
            std::ptr::null::<libc::sigset_t>(),
            0,
        )
    };
    ready > 0 && descriptor.revents & (libc::POLLIN | libc::POLLHUP) != 0
}

/// The fields of `/proc/<pid>/stat` after the command name (field 2), which
/// is enclosed in parentheses and may itself contain spaces or parentheses.
/// The first returned field is field 3 (state).
fn stat_fields(pid: u32) -> Option<Vec<String>> {
    fields_after_command(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
}

fn fields_after_command(stat: &str) -> Option<Vec<String>> {
    let (_, fields) = stat.rsplit_once(')')?;
    Some(fields.split_whitespace().map(str::to_owned).collect())
}

/// Field `number` (1-based, as `proc(5)` numbers them, from 3) of a stat line.
fn field(fields: &[String], number: usize) -> Option<u64> {
    fields.get(number.checked_sub(3)?)?.parse().ok()
}

/// When the live process `pid` started, in clock ticks since boot (field 22).
pub fn start_ticks(pid: u32) -> Option<u64> {
    field(&stat_fields(pid)?, 22)
}

/// When the live process `pid` started, in Unix milliseconds, to the
/// resolution of the kernel's boot time (one second).
pub fn started_unix_millis(pid: u32) -> Option<u64> {
    let ticks = start_ticks(pid)?;
    let boot_seconds: u64 = std::fs::read_to_string("/proc/stat")
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("btime "))?
        .trim()
        .parse()
        .ok()?;
    let ticks_per_second = rustix::param::clock_ticks_per_second();
    Some(boot_seconds * 1000 + ticks * 1000 / ticks_per_second)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_skip_command_names_with_spaces_and_parentheses() {
        let stat = "42 (a (b) c) S 7 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 777 19";
        let fields = fields_after_command(stat).unwrap();
        assert_eq!(field(&fields, 4), Some(7));
        assert_eq!(field(&fields, 22), Some(777));
        assert_eq!(
            field(&fields_after_command("42 (x) S 1").unwrap(), 22),
            None
        );
    }

    #[test]
    fn this_process_started_before_now() {
        let started = started_unix_millis(std::process::id()).unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        assert!(started <= now + 1000 && now - started < 24 * 3600 * 1000);
    }

    #[test]
    fn an_exited_process_has_no_facts() {
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert!(start_ticks(pid).is_none());
    }

    #[test]
    fn a_pidfd_distinguishes_a_running_process_from_completed_exit() {
        use std::os::fd::AsRawFd as _;
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let identity = pidfd_open(child.id()).unwrap();
        assert!(!process_exited(identity.as_raw_fd()));
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(process_exited(identity.as_raw_fd()));
    }
}
