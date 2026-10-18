//! Direct subprocess arguments and owned command lifetimes, without a shell;
//! the repository checkout a tool acts on, Git, live-process facts and the
//! one advisory file lock ([`lock::FileLock`]).

pub mod context;
pub mod git;
pub use context::{Context, command};
#[cfg(unix)]
mod io_lifetime;
#[cfg(unix)]
mod socket;
#[cfg(unix)]
pub use io_lifetime::IoLifetime;
#[cfg(unix)]
mod guardian;
#[cfg(unix)]
pub mod lock;
pub mod owned;
#[cfg(unix)]
pub mod proc;
mod repository;
pub use repository::{Checkout, built_root, checkout_of};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

mod cancellation;
mod notification;
pub use cancellation::{
    Cancelled, check_cancelled, cleanup, cleanup_deadline, is_cancelled, sleep,
};
pub use notification::{CancellationNotification, notify_on_cancel};
use std::{
    io::Read,
    process::{Command, Output, Stdio},
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};

fn cancellation() -> &'static Arc<AtomicBool> {
    static CANCELLED: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    CANCELLED.get_or_init(|| Arc::new(AtomicBool::new(false)))
}
pub fn cancellation_requested() -> bool {
    cancellation().load(Ordering::Relaxed) && !cancellation::in_cleanup()
}

pub struct SignalGuard {
    #[cfg(unix)]
    handle: signal_hook::iterator::Handle,
    #[cfg(unix)]
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Drop for SignalGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            self.handle.close();
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }
}
pub fn install_signal_handlers() -> Result<SignalGuard> {
    #[cfg(unix)]
    {
        let mut signals = signal_hook::iterator::Signals::new([
            signal_hook::consts::SIGINT,
            signal_hook::consts::SIGTERM,
            // A closed terminal or a killed agent shell: stop the children
            // too instead of leaving their groups running.
            signal_hook::consts::SIGHUP,
        ])?;
        let handle = signals.handle();
        let worker = std::thread::spawn(move || {
            for _ in signals.forever() {
                cancellation().store(true, Ordering::Relaxed);
                notification::notify();
            }
        });
        Ok(SignalGuard {
            handle,
            worker: Some(worker),
        })
    }
    #[cfg(not(unix))]
    Err("host process-group cancellation is unsupported on this host".into())
}

fn output_log() -> &'static std::sync::Mutex<Option<std::fs::File>> {
    static LOG: OnceLock<std::sync::Mutex<Option<std::fs::File>>> = OnceLock::new();
    LOG.get_or_init(|| std::sync::Mutex::new(None))
}

/// Sends the output of every later [`run`] and [`run_with_timeout`] command
/// to `file` instead of the terminal, or back to the terminal with `None`.
/// Captures are unaffected.
pub fn log_output_to(file: Option<std::fs::File>) {
    *output_log()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = file;
}

fn route(command: &mut Command) -> Result<()> {
    if let Some(file) = output_log()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
    {
        command.stdout(file.try_clone()?).stderr(file.try_clone()?);
    }
    Ok(())
}

/// The exit code a wrapper returns for its child's `status`: the child's
/// own code, or `128 + signal` for a child a signal ended (on Unix).
pub fn exit_code(status: std::process::ExitStatus) -> std::process::ExitCode {
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt;
    let code = status.code().unwrap_or_else(|| {
        #[cfg(unix)]
        {
            128 + status.signal().unwrap_or(1)
        }
        #[cfg(not(unix))]
        {
            1
        }
    });
    std::process::ExitCode::from(code as u8)
}

pub fn run(command: &mut Command) -> Result<()> {
    run_with_shutdown_grace(command, std::time::Duration::from_secs(1))
}

/// [`run`], stopping the command's whole process group once `timeout`
/// elapses.
pub fn run_with_timeout(command: &mut Command, timeout: std::time::Duration) -> Result<()> {
    route(command)?;
    let status = owned::Child::spawn(command)?
        .with_timeout(timeout)
        .wait()
        .map_err(|error| {
            if error.is::<owned::DeadlineExceeded>() {
                format!(
                    "{} exceeded its {} s limit",
                    command.get_program().to_string_lossy(),
                    timeout.as_secs()
                )
                .into()
            } else {
                error
            }
        })?;
    if !status.success() {
        return Err(format!(
            "{} failed with {status}",
            command.get_program().to_string_lossy()
        )
        .into());
    }
    Ok(())
}

/// Give a nested supervisor time to clean up the sessions or services it owns.
/// This bounds shutdown only; it does not limit command runtime or resources.
pub fn run_with_shutdown_grace(
    command: &mut Command,
    shutdown_grace: std::time::Duration,
) -> Result<()> {
    route(command)?;
    let mut child = owned::Child::spawn_with_shutdown_grace(command, shutdown_grace)?;
    let status = child.wait()?;
    if !status.success() {
        return Err(format!(
            "{} failed with {status}",
            command.get_program().to_string_lossy()
        )
        .into());
    }
    Ok(())
}
fn drain(
    mut input: impl Read + Send + 'static,
) -> std::thread::JoinHandle<std::io::Result<Vec<u8>>> {
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        input.read_to_end(&mut bytes)?;
        Ok(bytes)
    })
}
fn collect(handle: std::thread::JoinHandle<std::io::Result<Vec<u8>>>) -> Result<Vec<u8>> {
    Ok(handle
        .join()
        .map_err(|_| "subprocess output reader panicked")??)
}
/// Capture noninteractive probes: close stdin and drain both output streams.
pub fn capture(command: &mut Command) -> Result<Output> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = output(command, None)?;
    if !output.status.success() {
        return Err(format!(
            "{} failed with {}:\n{}",
            command.get_program().to_string_lossy(),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    Ok(output)
}

/// Capture a command without interpreting its exit status or printing arguments.
pub fn output(command: &mut Command, timeout: Option<std::time::Duration>) -> Result<Output> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    owned::Child::spawn(command)?.wait_with_output_timeout(timeout)
}

pub(crate) fn collect_output(
    mut child: owned::Child,
    timeout: Option<std::time::Duration>,
) -> Result<Output> {
    drop(child.stdin.take());
    let stdout = child.take_stdout().map(drain);
    let stderr = child.take_stderr().map(drain);
    let status = child.wait_timeout(timeout);
    let stdout = stdout.map(collect).transpose();
    let stderr = stderr.map(collect).transpose();
    let status = status?;
    let stdout = stdout?.unwrap_or_default();
    let stderr = stderr?.unwrap_or_default();
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

/// Bounded, cancellable equivalents of Command's blocking operations.
/// Long builds and captures should use an explicit workload-derived timeout.
pub trait CommandExt {
    fn supervised_output(&mut self) -> Result<Output>;
    fn supervised_status(&mut self) -> Result<std::process::ExitStatus>;
    fn spawn_owned(&mut self) -> Result<owned::Child>;
}

impl CommandExt for Command {
    fn supervised_output(&mut self) -> Result<Output> {
        output(self, Some(std::time::Duration::from_secs(120)))
    }
    fn supervised_status(&mut self) -> Result<std::process::ExitStatus> {
        owned::Child::spawn(self)?.wait_timeout(Some(std::time::Duration::from_secs(120)))
    }
    fn spawn_owned(&mut self) -> Result<owned::Child> {
        owned::Child::spawn(self)
    }
}
