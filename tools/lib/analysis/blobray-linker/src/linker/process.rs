//! Bounded subprocess transport shared by ELF linker dialects.
use super::*;
use sha2::{Digest, Sha256};
use std::os::fd::{FromRawFd, OwnedFd};
pub(super) fn digest(path: &Path, control: &mut dyn RunControl) -> Result<ArtifactId> {
    let mut file = File::open(path).map_err(io)?;
    let mut hash = Sha256::new();
    let mut bytes = [0; WORK_BLOCK];
    loop {
        control.checkpoint(1)?;
        let count = file.read(&mut bytes).map_err(io)?;
        if count == 0 {
            break;
        }
        control.bytes(count)?;
        hash.update(&bytes[..count]);
    }
    format!("{:x}", hash.finalize()).parse()
}
pub(super) fn pipe() -> Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0; 2];
    // SAFETY: two writable descriptors, flags valid for Linux pipe2.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(io(std::io::Error::last_os_error()));
    }
    // SAFETY: successful pipe2 transfers ownership of two distinct descriptors.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}
pub(super) struct ChildOwner(pub Child);
impl Drop for ChildOwner {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
pub(super) fn drain(
    child: &mut ChildOwner,
    channels: &[(i32, LinkOutput)],
    sink: &mut dyn LinkOutputSink,
    control: &mut dyn RunControl,
) -> Result<()> {
    let mut descriptors: Vec<_> = channels
        .iter()
        .map(|(fd, _)| libc::pollfd {
            fd: *fd,
            events: libc::POLLIN,
            revents: 0,
        })
        .collect();
    for (fd, _) in channels {
        // SAFETY: descriptors are live pipe reads retained by this invocation.
        let flags = unsafe { libc::fcntl(*fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(*fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err(io(std::io::Error::last_os_error()));
        }
    }
    let mut status = None;
    let mut bytes = [0; WORK_BLOCK];
    while status.is_none() || descriptors.iter().any(|d| d.fd >= 0) {
        control.checkpoint(1)?;
        // SAFETY: valid array of pollfd, bounded 20ms wait.
        let result = unsafe {
            libc::poll(
                descriptors.as_mut_ptr(),
                descriptors.len() as libc::nfds_t,
                20,
            )
        };
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(io(error));
            }
        }
        for (index, descriptor) in descriptors.iter_mut().enumerate() {
            if descriptor.fd < 0 || descriptor.revents == 0 {
                continue;
            }
            // One bounded read per channel gives cancellation and other channels a turn.
            // SAFETY: live pipe and writable bounded buffer.
            let count =
                unsafe { libc::read(descriptor.fd, bytes.as_mut_ptr().cast(), bytes.len()) };
            if count == 0 {
                descriptor.fd = -1;
            } else if count > 0 {
                sink.write(channels[index].1, &bytes[..count as usize], control)?;
            } else {
                let error = std::io::Error::last_os_error();
                if !matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) {
                    return Err(io(error));
                }
            }
        }
        if status.is_none() {
            status = child.0.try_wait().map_err(io)?;
        }
    }
    sink.exited(status.unwrap().code(), status.unwrap().signal());
    sink.observe(
        LinkObservation::ToolExit {
            code: status.unwrap().code(),
            signal: status.unwrap().signal(),
        },
        control,
    )?;
    if !status.unwrap().success() {
        return Err(Error::new(
            ErrorCode::LinkFailed,
            format!("linker exited as {}", status.unwrap()),
        ));
    }
    Ok(())
}
pub(super) fn command(path: &Path) -> Command {
    let mut command = oer_process::command(path);
    command
        .env_clear()
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // SAFETY: only setrlimit runs after fork; it takes a valid immutable value.
    unsafe {
        command.pre_exec(|| {
            let limit = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            if libc::setrlimit(libc::RLIMIT_CORE, &limit) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command
}
