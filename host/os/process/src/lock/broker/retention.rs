//! Stop processes that retain admitted lifetime sockets after owner loss.
//!
//! This runs after fork: fixed buffers and syscalls only. Open /proc/PID
//! directories verify identity before pidfd_send_signal, avoiding PID reuse.
//! Unreadable holders remain protected by their still-open broker connection.

use std::ffi::CStr;

use super::{CAPACITY, Client};

#[cfg(test)]
mod tests;

struct Directory {
    fd: i32,
    bytes: [u8; 4096],
    cursor: usize,
    filled: usize,
}

impl Directory {
    fn open(parent: i32, name: &CStr) -> Option<Self> {
        // SAFETY: name is NUL terminated and parent is AT_FDCWD or a live
        // directory. The returned descriptor is owned by this Directory.
        let fd = unsafe {
            libc::openat(
                parent,
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return None;
        }
        Some(Self {
            fd,
            bytes: [0; 4096],
            cursor: 0,
            filled: 0,
        })
    }

    fn next(&mut self) -> Option<&CStr> {
        if self.cursor == self.filled {
            // SAFETY: getdents64 writes at most the size of this valid buffer.
            let count = unsafe {
                libc::syscall(
                    libc::SYS_getdents64,
                    self.fd,
                    self.bytes.as_mut_ptr(),
                    self.bytes.len(),
                )
            };
            if count <= 0 {
                return None;
            }
            self.filled = count as usize;
            self.cursor = 0;
        }
        // linux_dirent64: d_reclen at byte 16, d_name at byte 19. Validate
        // record boundaries and its terminating NUL before passing it to libc.
        let entry = &self.bytes[self.cursor..self.filled];
        let length = u16::from_ne_bytes(entry.get(16..18)?.try_into().ok()?) as usize;
        let names = entry.get(19..length)?;
        let end = names.iter().position(|byte| *byte == 0)?;
        self.cursor += length;
        CStr::from_bytes_with_nul(&names[..=end]).ok()
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        // SAFETY: this Directory owns fd and never uses it after close.
        unsafe { libc::close(self.fd) };
    }
}

fn number(bytes: &[u8]) -> Option<u64> {
    if bytes.is_empty() {
        return None;
    }
    bytes.iter().try_fold(0_u64, |value, byte| {
        byte.is_ascii_digit().then_some(())?;
        value.checked_mul(10)?.checked_add(u64::from(byte - b'0'))
    })
}

fn retains(process: i32, clients: &[Client]) -> bool {
    let Some(mut descriptors) = Directory::open(process, c"fd") else {
        return false;
    };
    let directory = descriptors.fd;
    while let Some(name) = descriptors.next() {
        if number(name.to_bytes()).is_none() {
            continue;
        }
        let mut link = [0_u8; 64];
        // SAFETY: directory is live, name is a validated NUL-terminated entry,
        // and readlinkat writes only inside link, without needing a final NUL.
        let count = unsafe {
            libc::readlinkat(
                directory,
                name.as_ptr(),
                link.as_mut_ptr().cast(),
                link.len(),
            )
        };
        if count <= 0 {
            continue;
        }
        let Some(inode) = link[..count as usize]
            .strip_prefix(b"socket:[")
            .and_then(|rest| rest.strip_suffix(b"]"))
            .and_then(number)
        else {
            continue;
        };
        if clients
            .iter()
            .any(|client| client.fd >= 0 && client.admitted && client.inode == inode)
        {
            return true;
        }
    }
    false
}

pub(super) struct Cleanup {
    // SIGKILL can close the lifetime socket before the rest of exit_files.
    // Retain each targeted process identity until its completed exit instead
    // of treating socket EOF alone as proof that its I/O has stopped.
    processes: [Tracked; CAPACITY],
}

impl Cleanup {
    pub(super) const fn new() -> Self {
        Self {
            processes: [NO_PROCESS; CAPACITY],
        }
    }

    pub(super) fn drained(&mut self) -> bool {
        for process in &mut self.processes {
            if process.fd >= 0 && crate::proc::process_exited(process.fd) {
                // SAFETY: this slot owns its live kernel pidfd.
                unsafe { libc::close(process.fd) };
                *process = NO_PROCESS;
            }
        }
        self.processes.iter().all(|process| process.fd < 0)
    }

    pub(super) fn receive(&mut self, fd: i32, owner_alive: bool, caller_pid: i32) -> (isize, bool) {
        let mut byte = [0_u8];
        let mut message = crate::socket::receive(fd, &mut byte);
        if message.count <= 0 {
            return (message.count, false);
        }
        let completed = byte == *b"D" && message.pid == caller_pid;
        if byte == *b"R" && !message.truncated && message.descriptors.iter().all(|fd| *fd >= 0) {
            // SAFETY: received descriptors are live; identity is valid stack
            // storage. Framework pins send their own pidfd and /proc directory.
            let accepted = unsafe {
                let mut identity: libc::stat = std::mem::zeroed();
                let valid = owner_alive
                    && libc::fstat(message.descriptors[1], &mut identity) == 0
                    && identity.st_uid == libc::geteuid()
                    && identity.st_mode & libc::S_IFMT == libc::S_IFDIR;
                valid
                    && self
                        .processes
                        .iter_mut()
                        .find(|entry| entry.fd < 0)
                        .is_some_and(|slot| {
                            *slot = Tracked {
                                fd: message.take(0),
                                device: identity.st_dev,
                                inode: identity.st_ino,
                            };
                            true
                        })
            };
            let acknowledgement = [u8::from(accepted)];
            // SAFETY: the received acknowledgement socket is live until message drops.
            unsafe {
                libc::send(
                    message.descriptors[2],
                    acknowledgement.as_ptr().cast(),
                    1,
                    libc::MSG_NOSIGNAL,
                );
            }
        }
        (message.count, completed)
    }

    pub(super) fn terminate(&mut self, clients: &[Client], owner_identity: i32, signal: i32) {
        // Direct commands register before exec; retain them through complete
        // thread-group exit even if a guardian closes their sockets first.
        // A holder claimed by cleanup remains owned through its exit too.
        for entry in self.processes.iter().filter(|entry| entry.fd >= 0) {
            // SAFETY: each slot owns its stable process identity.
            unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    entry.fd,
                    signal,
                    std::ptr::null::<libc::siginfo_t>(),
                    0,
                );
            }
        }
        // SAFETY: stat's fields accept zero and owner_identity is the retained
        // /proc directory of the original owner, even after its PID is recycled.
        let mut owner: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: fstat writes only owner, for this live descriptor.
        if unsafe { libc::fstat(owner_identity, &mut owner) } != 0 {
            return;
        }
        let Some(mut processes) = Directory::open(libc::AT_FDCWD, c"/proc") else {
            return;
        };
        let directory = processes.fd;
        // SAFETY: getpid and geteuid have no memory preconditions.
        let (self_pid, uid) = unsafe { (libc::getpid(), libc::geteuid()) };
        while let Some(name) = processes.next() {
            let Some(pid) = number(name.to_bytes()).and_then(|pid| i32::try_from(pid).ok()) else {
                continue;
            };
            if pid <= 1 || pid == self_pid {
                continue;
            }
            let Some(process) = Directory::open(directory, name) else {
                continue;
            };
            // SAFETY: stat's integer fields accept zero; fstat writes only
            // valid local storage for the live process directory descriptor.
            let mut identity: libc::stat = unsafe { std::mem::zeroed() };
            // SAFETY: process owns fd and identity is valid output storage.
            let eligible = unsafe {
                libc::fstat(process.fd, &mut identity) == 0
                    && identity.st_uid == uid
                    && (identity.st_dev != owner.st_dev || identity.st_ino != owner.st_ino)
            };
            if !eligible || !retains(process.fd, clients) {
                continue;
            }
            if self.processes.iter().any(|entry| {
                entry.fd >= 0 && entry.device == identity.st_dev && entry.inode == identity.st_ino
            }) {
                continue;
            }
            let Some(slot) = self.processes.iter_mut().find(|entry| entry.fd < 0) else {
                // Do not kill an untracked holder. Its connection retains
                // exclusion until a completed exit frees a tracking slot.
                break;
            };
            let Some(pidfd) = holder_pidfd(directory, name, pid, &identity) else {
                continue;
            };
            // Claim the exit fence before signalling. Even ESRCH can race
            // with an exit still closing its I/O; a failed signal never drops
            // the process identity or weakens exclusion.
            *slot = Tracked {
                fd: pidfd,
                device: identity.st_dev,
                inode: identity.st_ino,
            };
            // SAFETY: holder_pidfd verified this stable kernel identity against
            // the process directory whose lifetime sockets were inspected.
            unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    pidfd,
                    signal,
                    std::ptr::null::<libc::siginfo_t>(),
                    0,
                );
            };
        }
    }
}

#[derive(Clone, Copy)]
struct Tracked {
    fd: i32,
    device: libc::dev_t,
    inode: libc::ino_t,
}
const NO_PROCESS: Tracked = Tracked {
    fd: -1,
    device: 0,
    inode: 0,
};

fn holder_pidfd(directory: i32, name: &CStr, pid: i32, identity: &libc::stat) -> Option<i32> {
    // SAFETY: pidfd_open takes only a PID and flags. Identity is verified below
    // because the numeric PID could have been reused since the first lookup.
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) } as i32;
    if fd < 0 {
        return None;
    }
    let matches = Directory::open(directory, name).is_some_and(|current| {
        // SAFETY: stat accepts zero and fstat writes only valid stack storage.
        unsafe {
            let mut stat: libc::stat = std::mem::zeroed();
            libc::fstat(current.fd, &mut stat) == 0
                && stat.st_dev == identity.st_dev
                && stat.st_ino == identity.st_ino
        }
    });
    if matches {
        Some(fd)
    } else {
        // SAFETY: fd is a new descriptor owned by this function.
        unsafe { libc::close(fd) };
        None
    }
}
