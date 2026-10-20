//! A guardian process that kills every owned child process group once the
//! owner is gone, however it ends.
//!
//! Each owned child leads a process group of its own, so a terminal's
//! signals reach the owner alone and [`crate::owned::Child`] can stop the
//! whole group. When the owner itself dies without that cleanup (SIGKILL,
//! an abort, a panic in another thread), its groups would keep running:
//! a `cargo test` started by a killed wrapper command held its build lock for
//! as long as the tests ran. The guardian closes that gap. It is forked on
//! the first spawn and reads the process groups the owner registers from a
//! pipe. Only the owner holds the pipe's write end (it is close-on-exec, so
//! no child inherits it), and the kernel closes it when the owner exits by
//! any means; the guardian then sees end of file and terminates every group
//! still registered, first with SIGTERM, a second later with SIGKILL.
//!
//! The guardian runs only async-signal-safe system calls after the fork:
//! no allocation, no locks, no Rust standard I/O. It therefore tracks a
//! fixed number of slots. The owner mirrors the guardian's slots, one per
//! registration, so a group the guardian could not track is refused before
//! its record is sent, and
//! a guardian that could not start or no longer reads refuses every group:
//! no group runs unguarded.

use std::{
    collections::HashMap,
    fs::File,
    io::Write,
    os::fd::FromRawFd,
    sync::{Mutex, OnceLock},
};

/// Process groups one guardian tracks at once.
pub(crate) const CAPACITY: usize = 256;

/// One registration record: an operation and a process group id.
const ADD: i32 = 1;
const REMOVE: i32 = 2;

/// The owner's end: the registration pipe and a mirror of the guardian's
/// slots.
///
/// A group id can be registered more than once: once its leader is reaped,
/// the kernel may hand the same PID to the next owned child before the
/// first child unregisters. Each registration holds one guardian slot, so
/// the mirror counts registrations per group and every unregistration
/// frees exactly one of them.
struct Registry<W = File> {
    pipe: W,
    groups: HashMap<i32, usize>,
    slots: usize,
}

impl<W: Write> Registry<W> {
    fn new(pipe: W) -> Self {
        Self {
            pipe,
            groups: HashMap::new(),
            slots: 0,
        }
    }

    fn register(&mut self, group: i32) -> crate::Result<()> {
        if self.slots >= CAPACITY {
            return Err(format!(
                "the process guardian already tracks {CAPACITY} process groups, so group \
                 {group} would outlive a lost owner"
            )
            .into());
        }
        send(&mut self.pipe, ADD, group)
            .map_err(|error| format!("the process guardian does not read: {error}"))?;
        *self.groups.entry(group).or_default() += 1;
        self.slots += 1;
        Ok(())
    }

    fn unregister(&mut self, group: i32) {
        let Some(count) = self.groups.get_mut(&group) else {
            return;
        };
        *count -= 1;
        if *count == 0 {
            self.groups.remove(&group);
        }
        self.slots -= 1;
        // A guardian that no longer reads has nothing to forget.
        let _ = send(&mut self.pipe, REMOVE, group);
    }
}

fn registry() -> crate::Result<&'static Mutex<Registry>> {
    static REGISTRY: OnceLock<Result<Mutex<Registry>, String>> = OnceLock::new();
    REGISTRY
        .get_or_init(|| {
            start()
                .map(|pipe| Mutex::new(Registry::new(pipe)))
                .map_err(|error| format!("the process guardian did not start: {error}"))
        })
        .as_ref()
        .map_err(|error| error.clone().into())
}

/// Registers `group` with the guardian; an error when the guardian cannot
/// take it, and the caller must not leave the group running.
pub(crate) fn register(group: i32) -> crate::Result<()> {
    registry()?
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .register(group)
}

/// Unregisters one successful registration of `group` once the owner itself
/// has terminated it.
pub(crate) fn unregister(group: i32) {
    let Ok(registry) = registry() else { return };
    registry
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .unregister(group);
}

fn send(pipe: &mut impl Write, operation: i32, group: i32) -> std::io::Result<()> {
    let mut record = [0_u8; 8];
    record[..4].copy_from_slice(&operation.to_ne_bytes());
    record[4..].copy_from_slice(&group.to_ne_bytes());
    // Eight bytes are below PIPE_BUF: the write is atomic.
    pipe.write_all(&record)
}

/// Forks the guardian and returns the write end of its registration pipe.
fn start() -> std::io::Result<File> {
    let mut fds = [0; 2];
    // SAFETY: `fds` is a valid two-element array for pipe2 to fill.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let [read, write] = fds;
    // SAFETY: the child runs only async-signal-safe calls in `guard`, which
    // never returns; the parent continues normally.
    match unsafe { libc::fork() } {
        -1 => {
            let error = std::io::Error::last_os_error();
            // SAFETY: both descriptors were just created by pipe2.
            unsafe {
                libc::close(read);
                libc::close(write);
            }
            Err(error)
        }
        0 => guard(read),
        _ => {
            // SAFETY: `read` belongs to this process and is not used again.
            unsafe { libc::close(read) };
            // SAFETY: `write` is an open descriptor this process owns alone.
            Ok(unsafe { File::from_raw_fd(write) })
        }
    }
}

/// The guardian: collect registrations until the owner is gone, then
/// terminate every registered group and exit.
fn guard(read: i32) -> ! {
    // SAFETY: every call below is async-signal-safe and touches only this
    // process's descriptors, signal dispositions and the registered groups.
    unsafe {
        // Detach from the owner's terminal signals: a Ctrl-C reaches the
        // owner, which cleans up itself; the guardian must outlive it.
        libc::setsid();
        crate::child_setup::ignore_terminal_signals();
        // Keep only the registration pipe: an inherited pipe write end would
        // keep the owner's readers waiting for end of file.
        let null = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR);
        for standard in 0..3 {
            libc::dup2(null, standard);
        }
        libc::dup2(read, 3);
        if libc::syscall(libc::SYS_close_range, 4_u32, u32::MAX, 0_u32) != 0 {
            for descriptor in 4..4096 {
                libc::close(descriptor);
            }
        }
        let mut groups = [0_i32; CAPACITY];
        let mut record = [0_u8; 8];
        let mut filled = 0;
        loop {
            let count = libc::read(3, record.as_mut_ptr().add(filled).cast(), 8 - filled);
            if count == 0 {
                break;
            }
            if count < 0 {
                if *libc::__errno_location() == libc::EINTR {
                    continue;
                }
                break;
            }
            filled += count as usize;
            if filled < 8 {
                continue;
            }
            filled = 0;
            let operation = i32::from_ne_bytes([record[0], record[1], record[2], record[3]]);
            let group = i32::from_ne_bytes([record[4], record[5], record[6], record[7]]);
            if group <= 1 {
                continue;
            }
            // The owner refuses a group beyond CAPACITY before sending it,
            // so an ADD always finds a free slot.
            if operation == ADD {
                if let Some(slot) = groups.iter_mut().find(|slot| **slot == 0) {
                    *slot = group;
                }
            } else if operation == REMOVE
                && let Some(slot) = groups.iter_mut().find(|slot| **slot == group)
            {
                *slot = 0;
            }
        }
        let mut any = false;
        for &group in groups.iter().filter(|group| **group > 1) {
            any |= libc::kill(-group, libc::SIGTERM) == 0;
        }
        if any {
            let grace = libc::timespec {
                tv_sec: 1,
                tv_nsec: 0,
            };
            libc::nanosleep(&grace, std::ptr::null_mut());
            for &group in groups.iter().filter(|group| **group > 1) {
                libc::kill(-group, libc::SIGKILL);
            }
        }
        libc::_exit(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn records(pipe: &[u8]) -> Vec<(i32, i32)> {
        pipe.chunks_exact(8)
            .map(|record| {
                let operation = i32::from_ne_bytes(record[..4].try_into().unwrap());
                let group = i32::from_ne_bytes(record[4..].try_into().unwrap());
                (operation, group)
            })
            .collect()
    }

    #[test]
    fn a_reused_group_id_holds_one_slot_per_registration() {
        let mut registry = Registry::new(Vec::new());
        registry.register(42).unwrap();
        // The first child's leader was reaped and its PID handed to the next
        // child before the first one unregistered.
        registry.register(42).unwrap();
        registry.unregister(42);
        registry.unregister(42);
        registry.unregister(42);
        assert_eq!(
            records(&registry.pipe),
            [(ADD, 42), (ADD, 42), (REMOVE, 42), (REMOVE, 42)]
        );
        assert_eq!(registry.slots, 0);
        assert!(registry.groups.is_empty());
    }

    #[test]
    fn capacity_counts_registrations_of_a_reused_group_id() {
        let mut registry = Registry::new(Vec::new());
        for _ in 0..CAPACITY {
            registry.register(42).unwrap();
        }
        assert!(registry.register(43).is_err());
        registry.unregister(42);
        registry.register(43).unwrap();
        assert_eq!(records(&registry.pipe).len(), CAPACITY + 2);
    }
}
