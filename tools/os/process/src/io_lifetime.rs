//! Retain an already admitted I/O operation through an external command.

use std::{
    os::{
        fd::AsRawFd,
        unix::{net::UnixStream, process::CommandExt},
    },
    process::Command,
    sync::Arc,
};

/// A lifetime connection, not authority to admit another operation. An empty
/// lifetime is used for operations without a device, such as hub discovery.
#[derive(Clone, Debug, Default)]
pub struct IoLifetime {
    connection: Option<Arc<Connection>>,
}

impl IoLifetime {
    pub(crate) fn new(connection: UnixStream) -> Self {
        Self {
            connection: Some(Arc::new(Connection { socket: connection })),
        }
    }

    /// Keep exclusion through the command's complete exit and its descendants'
    /// inherited connections, even if the operation's caller dies first.
    /// After owner loss the broker stops remaining holders during cleanup,
    /// including daemons in another process group or session.
    /// The child receives neither the lock descriptor nor admission authority.
    pub fn pin(&self, command: &mut Command) -> crate::Result<()> {
        let Some(connection) = &self.connection else {
            return Ok(());
        };
        let connection = connection.socket.try_clone()?;
        let (reply, acknowledgement) = UnixStream::pair()?;
        reply.set_read_timeout(Some(std::time::Duration::from_secs(3)))?;
        // SAFETY: pre_exec registers live descriptors and adjusts them with fcntl.
        // It performs no allocation, locking or application cleanup after fork.
        unsafe {
            command.pre_exec(move || {
                register_command(
                    connection.as_raw_fd(),
                    reply.as_raw_fd(),
                    acknowledgement.as_raw_fd(),
                )?;
                let fd = connection.as_raw_fd();
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Ok(())
    }
}

fn register_command(connection: i32, reply: i32, acknowledgement: i32) -> std::io::Result<()> {
    // SAFETY: all three sockets are captured live by pre_exec. This code uses
    // only syscalls and fixed stack buffers after fork. The child's own pidfd
    // and /proc directory cannot name a recycled process while it waits for ACK.
    unsafe {
        let pidfd = libc::syscall(libc::SYS_pidfd_open, libc::getpid(), 0) as i32;
        if pidfd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let directory = libc::open(
            c"/proc/self".as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        );
        if directory < 0 {
            let error = std::io::Error::last_os_error();
            libc::close(pidfd);
            return Err(error);
        }
        let sent = crate::socket::send(connection, b"R", &[pidfd, directory, acknowledgement]);
        libc::close(pidfd);
        libc::close(directory);
        sent?;
        let mut byte = 0_u8;
        let count = loop {
            let count = libc::read(reply, (&raw mut byte).cast(), 1);
            if count < 0 && *libc::__errno_location() == libc::EINTR {
                continue;
            }
            break count;
        };
        if count < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if count != 1 || byte != 1 {
            return Err(std::io::Error::from_raw_os_error(libc::ECANCELED));
        }
        Ok(())
    }
}

#[derive(Debug)]
struct Connection {
    socket: UnixStream,
}

impl Drop for Connection {
    fn drop(&mut self) {
        // SAFETY: this live socket is owned here. Typed callers close ports
        // before their final lifetime drops; signal that native I/O completion.
        // Nonblocking delivery keeps drop bounded, and a lost marker fails closed.
        unsafe {
            libc::send(
                self.socket.as_raw_fd(),
                b"D".as_ptr().cast(),
                1,
                libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
            );
        }
    }
}
