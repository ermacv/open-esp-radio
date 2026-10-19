//! A lock's lifetime broker. It owns the file description, not its clients.
//!
//! Owner loss closes admission before draining admitted operation connections.
//! Native callers report I/O completion; unexpected EOF waits for caller exit.
//! Pinned commands and holders claimed by cleanup retain their kernel pidfds
//! through complete exit, even if their lifetime socket closes earlier.
//! The detached child uses only async-signal-safe syscalls after fork, like the
//! process guardian; it allocates nothing and never returns into Rust cleanup.

use std::{
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        linux::net::SocketAddrExt,
        unix::{
            fs::MetadataExt,
            net::{SocketAddr, UnixListener, UnixStream},
        },
    },
    path::Path,
    time::Duration,
};

use super::{FileLock, Mode};
use crate::Result;

const CAPACITY: usize = 256;
const TOKEN_BYTES: usize = 64;
const REQUEST_BYTES: usize = TOKEN_BYTES + 8;
const HANDSHAKE: Duration = Duration::from_secs(3);

mod retention;

#[cfg(test)]
mod tests;

#[derive(Debug)]
pub struct LockBroker {
    owner: UnixStream,
    pid: libc::pid_t,
}

#[derive(Debug)]
pub struct BrokerOperation {
    // A separate close-on-exec connection for every admitted operation.
    lifetime: crate::IoLifetime,
}

impl BrokerOperation {
    pub fn lifetime(&self) -> &crate::IoLifetime {
        &self.lifetime
    }
}

impl LockBroker {
    /// Transfer an exclusive lock to a broker that drains admitted I/O after
    /// this owner drops or dies. No client's drop can unlock another's I/O.
    ///
    /// The broker is forked without exec: until it closes every descriptor
    /// above its inputs, it holds each file this process had open at the fork,
    /// so another thread's close does not yet release it (a pty master's
    /// close does not hang up its slave).
    pub fn start(lock: FileLock, token: &str) -> Result<Self> {
        if lock.mode() != Mode::Exclusive {
            return Err("a lifetime broker needs an exclusive lock".into());
        }
        let token: [u8; TOKEN_BYTES] = token
            .as_bytes()
            .try_into()
            .map_err(|_| "a broker capability must be 64 bytes")?;
        let listener = UnixListener::bind_addr(&address(&lock.file().metadata()?)?)?;
        listener.set_nonblocking(true)?;
        let (mut owner, client) = UnixStream::pair()?;
        owner.set_read_timeout(Some(HANDSHAKE))?;
        let owner_identity = std::fs::File::open("/proc/self")?;
        let owner_pidfd = crate::proc::pidfd_open(std::process::id())?;
        // Suppress FileLock's explicit unlock: the child receives this open file
        // description, and the parent only closes its duplicate after startup.
        let file = lock.into_file();
        let descriptors = [
            file.as_raw_fd(),
            client.as_raw_fd(),
            listener.as_raw_fd(),
            owner_identity.as_raw_fd(),
            owner_pidfd.as_raw_fd(),
        ];
        let owner_pid = std::process::id() as i32;
        // SAFETY: the child performs only async-signal-safe syscalls in serve
        // and exits with _exit. All arguments were allocated before the fork.
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        if pid == 0 {
            // SAFETY: descriptors and token are live in the child's copy
            // of this stack; serve never returns or runs Rust destructors.
            unsafe { serve(descriptors, &token, owner_pid) }
        }
        drop(client);
        drop(listener);
        drop(file);
        drop(owner_identity);
        drop(owner_pidfd);
        let mut ready = [0];
        if let Err(error) = owner.read_exact(&mut ready) {
            drop(owner);
            reap(pid, false);
            return Err(format!("start lock broker: {error}").into());
        }
        if ready != [1] {
            drop(owner);
            reap(pid, false);
            return Err("lock broker did not acknowledge ownership".into());
        }
        Ok(Self { owner, pid })
    }

    /// The server PID its clients authenticate before sending a capability.
    pub fn pid(&self) -> u32 {
        self.pid as u32
    }

    /// Admission and retention are one broker operation. Returning a guard
    /// means the broker already counts it; owner loss cannot release its lock.
    /// `path` identifies the lock file, regardless of its pathname's length.
    /// `broker_pid` comes from that owner's record, not the endpoint name.
    pub fn operation(path: &Path, token: &str, broker_pid: u32) -> Result<BrokerOperation> {
        // SAFETY: geteuid has no arguments or memory preconditions.
        Self::operation_for_peer(path, token, unsafe { libc::geteuid() }, broker_pid)
    }

    fn operation_for_peer(
        path: &Path,
        token: &str,
        uid: libc::uid_t,
        pid: u32,
    ) -> Result<BrokerOperation> {
        if token.len() != TOKEN_BYTES {
            return Err("invalid broker capability".into());
        }
        let mut connection = UnixStream::connect_addr(&address(&std::fs::metadata(path)?)?)
            .map_err(|error| format!("lock owner no longer admits operations: {error}"))?;
        connection.set_read_timeout(Some(HANDSHAKE))?;
        connection.set_write_timeout(Some(HANDSHAKE))?;
        authenticate_server(&connection, uid, pid)?;
        let inode = rustix::fs::fstat(&connection)?.st_ino;
        let mut request = [0_u8; REQUEST_BYTES];
        request[..TOKEN_BYTES].copy_from_slice(token.as_bytes());
        request[TOKEN_BYTES..].copy_from_slice(&inode.to_ne_bytes());
        // Guard failed admissions too: if acknowledgement is lost after the
        // server admitted us, no native I/O began and drop can report completion.
        let lifetime = crate::IoLifetime::new(connection.try_clone()?);
        let caller = crate::proc::pidfd_open(std::process::id())?;
        crate::socket::send(connection.as_raw_fd(), &request, &[caller.as_raw_fd()])?;
        let mut admitted = [0];
        connection
            .read_exact(&mut admitted)
            .map_err(|error| format!("lock owner no longer admits operations: {error}"))?;
        if admitted != [1] {
            return Err("lock owner no longer admits this operation".into());
        }
        Ok(BrokerOperation { lifetime })
    }
}

fn authenticate_server(connection: &UnixStream, uid: libc::uid_t, pid: u32) -> std::io::Result<()> {
    let mut peer = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: connection is a live Unix socket, and both output pointers refer
    // to correctly sized stack storage throughout getsockopt.
    if unsafe {
        libc::getsockopt(
            connection.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut peer).cast(),
            &raw mut length,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error());
    }
    if length as usize != std::mem::size_of::<libc::ucred>()
        || peer.uid != uid
        || peer.pid as u32 != pid
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "lock broker server has unexpected peer credentials",
        ));
    }
    Ok(())
}

// Linux abstract addresses are bounded by numeric file identity, rather than
// the XDG path. The capability is sent only in the handshake, never exposed in
// /proc/net/unix. Admission also checks peer credentials, replacing filesystem
// socket permissions. Closing the listener removes the address automatically.
fn address(metadata: &std::fs::Metadata) -> std::io::Result<SocketAddr> {
    // SAFETY: geteuid has no arguments or memory preconditions.
    let uid = unsafe { libc::geteuid() };
    SocketAddr::from_abstract_name(format!(
        "oer-lock:{uid}:{}:{}",
        metadata.dev(),
        metadata.ino()
    ))
}

impl Drop for LockBroker {
    fn drop(&mut self) {
        // Normal release waits for the broker's admission closure. A draining
        // delegate never blocks the owner's drop; a reaper waits for its exit.
        // A graceful end proves the local I/O caller is still in control. EOF
        // without this marker requires waiting for the owner's complete exit.
        let _ = (&self.owner).write_all(&[1]);
        let _ = self.owner.shutdown(std::net::Shutdown::Write);
        let mut draining = [1];
        let immediate = self.owner.read_exact(&mut draining).is_ok() && draining == [0];
        reap(self.pid, immediate);
    }
}

fn reap(pid: libc::pid_t, immediate: bool) {
    let wait = move || {
        // SAFETY: pid is the broker this process forked. No other owner waits
        // for it; waitpid touches only a valid local status pointer.
        unsafe {
            let mut status = 0;
            while libc::waitpid(pid, &mut status, 0) < 0 && *libc::__errno_location() == libc::EINTR
            {
            }
        }
    };
    if immediate {
        wait();
    } else {
        std::thread::spawn(wait);
    }
}

#[derive(Clone, Copy)]
struct Client {
    fd: i32,
    request: [u8; REQUEST_BYTES],
    filled: usize,
    admitted: bool,
    started: i64,
    inode: u64,
    caller: i32,
    caller_pid: i32,
    caller_owner: bool,
}

const EMPTY: Client = Client {
    fd: -1,
    request: [0; REQUEST_BYTES],
    filled: 0,
    admitted: false,
    started: 0,
    inode: 0,
    caller: -1,
    caller_pid: -1,
    caller_owner: false,
};

/// SAFETY: runs only in the freshly forked child. Every descriptor, pointer and
/// fixed array stays valid until _exit; only async-signal-safe calls are used.
unsafe fn serve(descriptors: [i32; 5], token: &[u8; TOKEN_BYTES], owner_pid: i32) -> ! {
    // SAFETY: operations below use only descriptors inherited by this child and
    // preallocated stack storage. It never acquires a Rust lock or allocates.
    unsafe {
        libc::setsid();
        for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGPIPE] {
            libc::signal(signal, libc::SIG_IGN);
        }
        // Duplicate all inputs first so dup2 cannot overwrite another input.
        let mut copies = [-1; 5];
        for index in 0..5 {
            copies[index] = libc::fcntl(descriptors[index], libc::F_DUPFD_CLOEXEC, 8);
            if copies[index] < 0 {
                libc::_exit(1);
            }
        }
        for (index, copy) in copies.iter().enumerate() {
            if libc::dup2(*copy, index as i32 + 3) < 0 {
                libc::_exit(1);
            }
        }
        let null = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR);
        if null < 0 {
            libc::_exit(1);
        }
        for fd in 0..3 {
            libc::dup2(null, fd);
        }
        if libc::syscall(libc::SYS_close_range, 8_u32, u32::MAX, 0_u32) != 0 {
            libc::_exit(1);
        }
        // SO_PEERCRED captures listener credentials at listen, not bind.
        // Refresh them here so clients authenticate this broker's PID.
        if libc::listen(5, CAPACITY as i32) != 0 {
            libc::_exit(1);
        }
        // fd 3: lock; fd 4: owner liveness; fd 5: listener;
        // fd 6: original owner's /proc directory; fd 7: original owner's pidfd.
        let ready = [1_u8];
        if libc::write(4, ready.as_ptr().cast(), 1) != 1 {
            libc::_exit(1);
        }
        let mut clients = [EMPTY; CAPACITY];
        let mut owner_alive = true;
        let mut owner_lost_at = 0;
        let mut next_cleanup_at = 0;
        let mut owner_exit_required = false;
        let mut cleanup = retention::Cleanup::new();
        loop {
            let mut polls = [libc::pollfd {
                fd: -1,
                events: libc::POLLIN,
                revents: 0,
            }; CAPACITY + 2];
            polls[0].fd = if owner_alive { 4 } else { -1 };
            polls[1].fd = if owner_alive { 5 } else { -1 };
            for index in 0..CAPACITY {
                polls[index + 2].fd = clients[index].fd;
            }
            let poll_failed = libc::poll(polls.as_mut_ptr(), polls.len() as libc::nfds_t, 100) < 0;
            if poll_failed {
                if *libc::__errno_location() == libc::EINTR {
                    continue;
                }
                // Bound retries even when poll fails persistently. Nonblocking
                // socket probes below still observe owner loss and drain I/O;
                // a polling error alone never releases the lock.
                let mut delay = libc::timespec {
                    tv_sec: 0,
                    tv_nsec: 100_000_000,
                };
                let mut remaining: libc::timespec = std::mem::zeroed();
                while libc::nanosleep(&delay, &mut remaining) != 0
                    && *libc::__errno_location() == libc::EINTR
                {
                    delay = remaining;
                }
            }
            let owner_lost = if owner_alive && (poll_failed || polls[0].revents != 0) {
                let mut byte = 0_u8;
                let count = libc::recv(4, (&raw mut byte).cast(), 1, libc::MSG_DONTWAIT);
                let lost = count >= 0
                    || (*libc::__errno_location() != libc::EAGAIN
                        && *libc::__errno_location() != libc::EINTR);
                if lost {
                    owner_exit_required = count != 1 || byte != 1;
                }
                lost
            } else {
                false
            };
            if owner_lost {
                owner_alive = false;
                libc::close(5);
            }
            let mut clock: libc::timespec = std::mem::zeroed();
            libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut clock);
            let now = clock.tv_sec * 1000 + clock.tv_nsec / 1_000_000;
            if owner_lost {
                owner_lost_at = now;
            }
            for index in 0..CAPACITY {
                let client = &mut clients[index];
                if client.fd < 0 {
                    if client.caller >= 0 && crate::proc::process_exited(client.caller) {
                        libc::close(client.caller);
                        *client = EMPTY;
                    }
                    continue;
                }
                if !client.admitted && (!owner_alive || now - client.started > 3000) {
                    libc::close(client.fd);
                    if client.caller >= 0 {
                        libc::close(client.caller);
                    }
                    *client = EMPTY;
                    continue;
                }
                if !poll_failed && polls[index + 2].revents == 0 && !owner_lost {
                    continue;
                }
                if client.admitted {
                    let (read, completed) =
                        cleanup.receive(client.fd, owner_alive, client.caller_pid);
                    if completed && client.caller >= 0 {
                        libc::close(client.caller);
                        client.caller = -1;
                    }
                    if read == 0
                        || (read < 0
                            && *libc::__errno_location() != libc::EAGAIN
                            && *libc::__errno_location() != libc::EINTR)
                    {
                        libc::close(client.fd);
                        client.fd = -1;
                        if client.caller < 0 {
                            *client = EMPTY;
                        }
                    }
                } else {
                    let mut message =
                        crate::socket::receive(client.fd, &mut client.request[client.filled..]);
                    let count = message.count;
                    if client.caller < 0 && message.descriptors[0] >= 0 {
                        client.caller = message.take(0);
                    }
                    if count == 0
                        || (count < 0
                            && *libc::__errno_location() != libc::EAGAIN
                            && *libc::__errno_location() != libc::EINTR)
                    {
                        libc::close(client.fd);
                        if client.caller >= 0 {
                            libc::close(client.caller);
                        }
                        *client = EMPTY;
                        continue;
                    }
                    if count > 0 {
                        client.filled += count as usize;
                    }
                    if client.filled == REQUEST_BYTES {
                        let mut inode = [0_u8; 8];
                        inode.copy_from_slice(&client.request[TOKEN_BYTES..]);
                        client.inode = u64::from_ne_bytes(inode);
                        let admitted = [u8::from(
                            client.request[..TOKEN_BYTES] == *token
                                && client.inode != 0
                                && client.caller >= 0,
                        )];
                        if admitted[0] == 1
                            && libc::write(client.fd, admitted.as_ptr().cast(), 1) == 1
                        {
                            client.admitted = true;
                        } else {
                            libc::close(client.fd);
                            if client.caller >= 0 {
                                libc::close(client.caller);
                            }
                            *client = EMPTY;
                        }
                    }
                }
            }
            let active = clients
                .iter()
                .any(|client| (client.fd >= 0 || client.caller >= 0) && client.admitted);
            let drained = cleanup.drained();
            if !owner_alive
                && (active || !drained)
                && now - owner_lost_at >= 1000
                && now >= next_cleanup_at
            {
                // First allow the guardian's existing one-second cleanup.
                // Then terminate remaining lifetime holders, even after setsid.
                // Repeat scans to catch forks. EOF and completed process exits
                // fence unlock; an empty scan never releases inaccessible I/O.
                let signal = if now - owner_lost_at < 2000 {
                    libc::SIGTERM
                } else {
                    libc::SIGKILL
                };
                // An uncompleted native caller retains I/O even if its socket
                // closed early. Its pidfd lets cleanup stop that caller too.
                for client in &clients {
                    if client.admitted && client.caller >= 0 && !client.caller_owner {
                        libc::syscall(
                            libc::SYS_pidfd_send_signal,
                            client.caller,
                            signal,
                            std::ptr::null::<libc::siginfo_t>(),
                            0,
                        );
                    }
                }
                cleanup.terminate(&clients, 6, signal);
                next_cleanup_at = now + 250;
            }
            let release = !owner_alive
                && !active
                && drained
                && (!owner_exit_required || crate::proc::process_exited(7));
            if release {
                // A concurrent fork in the owner may have copied the file
                // description before handoff. Explicitly release its flock
                // after I/O drains, regardless of those unrelated copies.
                while libc::flock(3, libc::LOCK_UN) != 0 {
                    if *libc::__errno_location() != libc::EINTR {
                        libc::_exit(1);
                    }
                }
                libc::close(3);
            }
            if owner_lost {
                let draining = [u8::from(!release)];
                libc::write(4, draining.as_ptr().cast(), 1);
                libc::close(4);
            }
            if release {
                libc::_exit(0);
            }
            if owner_alive && (poll_failed || polls[1].revents != 0) {
                let fd = libc::accept4(
                    5,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
                );
                if fd >= 0 {
                    let mut peer: libc::ucred = std::mem::zeroed();
                    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
                    if libc::getsockopt(
                        fd,
                        libc::SOL_SOCKET,
                        libc::SO_PEERCRED,
                        (&raw mut peer).cast(),
                        &raw mut length,
                    ) != 0
                        || length as usize != std::mem::size_of::<libc::ucred>()
                        || peer.uid != libc::geteuid()
                    {
                        libc::close(fd);
                        continue;
                    }
                    if let Some(client) = clients
                        .iter_mut()
                        .find(|client| client.fd < 0 && client.caller < 0)
                    {
                        let pass_credentials = 1_i32;
                        if libc::setsockopt(
                            fd,
                            libc::SOL_SOCKET,
                            libc::SO_PASSCRED,
                            (&raw const pass_credentials).cast(),
                            std::mem::size_of::<i32>() as libc::socklen_t,
                        ) != 0
                        {
                            libc::close(fd);
                            continue;
                        }
                        *client = Client {
                            fd,
                            started: now,
                            caller_pid: peer.pid,
                            caller_owner: peer.pid == owner_pid && !crate::proc::process_exited(7),
                            ..EMPTY
                        };
                    } else {
                        libc::close(fd);
                    }
                }
            }
        }
    }
}
