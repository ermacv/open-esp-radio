//! Bounded subprocess transport shared by ELF linker dialects.
use super::*;
use sha2::{Digest, Sha256};
use std::os::fd::{BorrowedFd, OwnedFd};
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
    rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC).map_err(|error| io(error.into()))
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
    channels: &[(BorrowedFd<'_>, LinkOutput)],
    sink: &mut dyn LinkOutputSink,
    control: &mut dyn RunControl,
) -> Result<()> {
    use rustix::event::{PollFd, PollFlags, Timespec};
    use rustix::io::Errno;
    for (fd, _) in channels {
        let flags = rustix::fs::fcntl_getfl(fd).map_err(|error| io(error.into()))?;
        rustix::fs::fcntl_setfl(fd, flags | rustix::fs::OFlags::NONBLOCK)
            .map_err(|error| io(error.into()))?;
    }
    let mut open = vec![true; channels.len()];
    let mut status = None;
    let mut bytes = [0; WORK_BLOCK];
    let wait = Timespec {
        tv_sec: 0,
        tv_nsec: 20_000_000,
    };
    while status.is_none() || open.iter().any(|open| *open) {
        control.checkpoint(1)?;
        let polled: Vec<usize> = (0..channels.len()).filter(|index| open[*index]).collect();
        let mut descriptors: Vec<_> = polled
            .iter()
            .map(|index| PollFd::new(&channels[*index].0, PollFlags::IN))
            .collect();
        match rustix::event::poll(&mut descriptors, Some(&wait)) {
            Ok(_) | Err(Errno::INTR) => {}
            Err(error) => return Err(io(error.into())),
        }
        let ready: Vec<usize> = polled
            .iter()
            .zip(&descriptors)
            .filter(|(_, descriptor)| !descriptor.revents().is_empty())
            .map(|(index, _)| *index)
            .collect();
        drop(descriptors);
        for index in ready {
            // One bounded read per channel gives cancellation and other channels a turn.
            match rustix::io::read(channels[index].0, &mut bytes) {
                Ok(0) => open[index] = false,
                Ok(count) => sink.write(channels[index].1, &bytes[..count], control)?,
                Err(Errno::AGAIN | Errno::INTR) => {}
                Err(error) => return Err(io(error.into())),
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
    let mut command = oer_os_process::command(path);
    command
        .env_clear()
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    oer_os_process::child_setup::without_core_dumps(&mut command);
    command
}
