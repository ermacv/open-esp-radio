//! Fixed-size Unix descriptor messages, also usable after fork.
use std::os::fd::RawFd;

pub(crate) struct Message {
    pub count: isize,
    pub descriptors: [RawFd; 3],
    pub pid: libc::pid_t,
    pub truncated: bool,
}

impl Message {
    pub fn take(&mut self, index: usize) -> RawFd {
        std::mem::replace(&mut self.descriptors[index], -1)
    }
}

impl Drop for Message {
    fn drop(&mut self) {
        for fd in self.descriptors.iter().filter(|fd| **fd >= 0) {
            // SAFETY: each untransferred descriptor was received and is owned here.
            unsafe { libc::close(*fd) };
        }
    }
}

pub(crate) fn receive(fd: RawFd, bytes: &mut [u8]) -> Message {
    let mut received = Message {
        count: -1,
        descriptors: [-1; 3],
        pid: -1,
        truncated: false,
    };
    // SAFETY: fd is a live socket; all buffers are aligned, bounded and live
    // throughout recvmsg. Kernel-validated control records own their received FDs.
    unsafe {
        let mut vector = libc::iovec {
            iov_base: bytes.as_mut_ptr().cast(),
            iov_len: bytes.len(),
        };
        let mut control = [0_usize; 16];
        let mut message: libc::msghdr = std::mem::zeroed();
        message.msg_iov = &mut vector;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = std::mem::size_of_val(&control);
        received.count = libc::recvmsg(fd, &mut message, libc::MSG_CMSG_CLOEXEC);
        if received.count <= 0 {
            return received;
        }
        received.truncated = message.msg_flags & libc::MSG_CTRUNC != 0;
        let mut index = 0;
        let mut header = libc::CMSG_FIRSTHDR(&message);
        while !header.is_null() {
            if (*header).cmsg_level == libc::SOL_SOCKET {
                if (*header).cmsg_type == libc::SCM_CREDENTIALS
                    && (*header).cmsg_len
                        >= libc::CMSG_LEN(std::mem::size_of::<libc::ucred>() as u32) as usize
                {
                    received.pid = (*libc::CMSG_DATA(header).cast::<libc::ucred>()).pid;
                } else if (*header).cmsg_type == libc::SCM_RIGHTS {
                    let count = ((*header).cmsg_len - libc::CMSG_LEN(0) as usize)
                        / std::mem::size_of::<RawFd>();
                    let descriptors = libc::CMSG_DATA(header).cast::<RawFd>();
                    for offset in 0..count {
                        if index < received.descriptors.len() {
                            received.descriptors[index] = *descriptors.add(offset);
                            index += 1;
                        } else {
                            libc::close(*descriptors.add(offset));
                            received.truncated = true;
                        }
                    }
                }
            }
            header = libc::CMSG_NXTHDR(&message, header);
        }
    }
    received
}

pub(crate) fn send(fd: RawFd, bytes: &[u8], descriptors: &[RawFd]) -> std::io::Result<()> {
    assert!(descriptors.len() <= 3);
    // SAFETY: pointers refer to live fixed buffers, descriptor slots and input
    // bytes. sendmsg copies them during this syscall; no allocation or locks occur.
    unsafe {
        let mut vector = libc::iovec {
            iov_base: bytes.as_ptr().cast_mut().cast(),
            iov_len: bytes.len(),
        };
        let mut control = [0_usize; 16];
        let mut message: libc::msghdr = std::mem::zeroed();
        message.msg_iov = &mut vector;
        message.msg_iovlen = 1;
        if !descriptors.is_empty() {
            message.msg_control = control.as_mut_ptr().cast();
            message.msg_controllen =
                libc::CMSG_SPACE(std::mem::size_of_val(descriptors) as u32) as usize;
            let header = libc::CMSG_FIRSTHDR(&message);
            (*header).cmsg_level = libc::SOL_SOCKET;
            (*header).cmsg_type = libc::SCM_RIGHTS;
            (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of_val(descriptors) as u32) as usize;
            std::ptr::copy_nonoverlapping(
                descriptors.as_ptr(),
                libc::CMSG_DATA(header).cast(),
                descriptors.len(),
            );
        }
        loop {
            let count = libc::sendmsg(fd, &message, libc::MSG_NOSIGNAL);
            if count < 0 && *libc::__errno_location() == libc::EINTR {
                continue;
            }
            if count < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if count as usize != bytes.len() {
                return Err(std::io::Error::from_raw_os_error(libc::EIO));
            }
            return Ok(());
        }
    }
}
