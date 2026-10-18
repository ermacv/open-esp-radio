//! Capture readiness and stop are process events, independent of traffic duration.

use crate::Result;
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Command, Output, Stdio},
    sync::mpsc,
    thread,
    time::Duration,
};

pub struct Capture {
    child: Option<oer_process::owned::Child>,
    reader: Option<thread::JoinHandle<Result<Vec<u8>>>>,
}

enum Event {
    Ready,
    Closed,
    Cancelled,
}

impl Capture {
    pub fn start(command: &mut Command, ready_prefix: String, lifetime: Duration) -> Result<Self> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .env("LC_ALL", "C");
        let mut child = oer_process::owned::Child::spawn(command)?.with_timeout(lifetime);
        let pipe = child.take_stderr().ok_or("capture stderr unavailable")?;
        let (sender, receiver) = mpsc::channel();
        let cancelled = sender.clone();
        let _notification = oer_process::notify_on_cancel(move || {
            let _ = cancelled.send(Event::Cancelled);
        });
        let reader = thread::spawn(move || {
            let result = read_diagnostics(pipe, &ready_prefix, &sender);
            let _ = sender.send(Event::Closed);
            result
        });
        let owner = Self {
            child: Some(child),
            reader: Some(reader),
        };
        oer_process::check_cancelled()?;
        match receiver.recv_timeout(Duration::from_secs(15)) {
            Ok(Event::Ready) => Ok(owner),
            Ok(Event::Cancelled) => Err(oer_process::Cancelled.into()),
            _ => {
                let diagnostics = owner.abort();
                Err(super::Error::new(format!("capture did not acknowledge an opened capture handle before the readiness deadline: {}", diagnostics.trim())).into())
            }
        }
    }

    fn abort(mut self) -> String {
        drop(self.child.take());
        match self.reader.take().expect("capture owns reader").join() {
            Ok(Ok(bytes)) => String::from_utf8_lossy(&bytes).into_owned(),
            Ok(Err(error)) => error.to_string(),
            Err(_) => "capture reader panicked".into(),
        }
    }

    pub fn finish(mut self) -> Result<Output> {
        let mut child = self.child.take().expect("capture owns child");
        let stop = child
            .stdin
            .take()
            .ok_or("capture control stream unavailable")
            .and_then(|mut stdin| {
                stdin
                    .write_all(b"stop\n")
                    .map_err(|_| "capture exited before Stop")
            });
        let status = child.wait_timeout(Some(Duration::from_secs(10)));
        // Child termination closes stderr before joining its reader, even on timeout.
        drop(child);
        let stderr = self
            .reader
            .take()
            .expect("capture owns reader")
            .join()
            .map_err(|_| "capture reader panicked")??;
        let status = status?;
        if let Err(error) = stop {
            return Err(super::Error::new(format!(
                "{error} ({status}): {}",
                String::from_utf8_lossy(&stderr).trim()
            ))
            .into());
        }
        if !status.success() {
            return Err(super::Error::new(format!(
                "capture process failed while completing Stop ({status}): {}",
                String::from_utf8_lossy(&stderr).trim()
            ))
            .into());
        }
        Ok(Output {
            status,
            stdout: Vec::new(),
            stderr,
        })
    }
}

fn read_diagnostics(
    pipe: impl Read,
    ready_prefix: &str,
    sender: &mpsc::Sender<Event>,
) -> Result<Vec<u8>> {
    const LIMIT: usize = 1024 * 1024;
    // Bound the read itself: even a single unterminated line cannot
    // allocate beyond the diagnostic budget.
    let mut pipe = BufReader::new(pipe).take((LIMIT + 1) as u64);
    let mut output = Vec::new();
    loop {
        let mut line = Vec::new();
        if pipe.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        if output.len() + line.len() > LIMIT {
            return Err("capture diagnostic stream exceeded 1 MiB".into());
        }
        if String::from_utf8_lossy(&line)
            .trim_start()
            .starts_with(ready_prefix)
        {
            let _ = sender.send(Event::Ready);
        }
        output.extend_from_slice(&line);
    }
    Ok(output)
}

impl Drop for Capture {
    fn drop(&mut self) {
        drop(self.child.take());
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// The dumpcap command line of one capture.
fn dumpcap_program(interface: &str, filter: Option<&str>, snapshot: u16, path: &str) -> String {
    // The kernel's capture ring must absorb bursts at the links' ceiling
    // rates: the default 2 MiB dropped frames at 90 Mbit/s.
    let mut program = format!(
        "dumpcap -q -i {} -B {CAPTURE_BUFFER_MIB} -s {snapshot} -a filesize:131072 -w {}",
        quote(interface),
        quote(path)
    );
    if let Some(filter) = filter {
        program.push_str(&format!(" -f {}", quote(filter)));
    }
    program
}

/// The kernel capture buffer of every stand capture, in MiB.
const CAPTURE_BUFFER_MIB: u32 = 64;

pub fn dumpcap(
    interface: &str,
    filter: Option<&str>,
    snapshot: u16,
    output: &std::path::Path,
    duration: Duration,
) -> Result<Capture> {
    let path = output.to_str().ok_or("capture path is not UTF-8")?;
    let program = dumpcap_program(interface, filter, snapshot, path);
    let lifetime = duration.saturating_add(Duration::from_secs(120));
    let mut command = oer_process::command("timeout");
    command.args([
        "-s",
        "TERM",
        &lifetime.as_secs().to_string(),
        "sh",
        "-c",
        &controlled(&program, ":"),
    ]);
    // dumpcap emits File only after opening its capture handles and output.
    Capture::start(&mut command, format!("File: {path}"), lifetime)
}

/// The shell owns only the capture child it created. EOF/cancellation also
/// interrupts and reaps that child; a watchdog expiration is a test failure.
pub fn controlled(program: &str, cleanup: &str) -> String {
    format!(
        r#"
{program} & capture=$!
cleanup_capture() {{
    status=$?
    trap - EXIT HUP INT TERM
    if test -n "$capture"; then kill -TERM "$capture" 2>/dev/null || true; wait "$capture" 2>/dev/null || true; fi
    {cleanup}
    exit "$status"
}}
trap cleanup_capture EXIT
trap 'exit 129' HUP; trap 'exit 130' INT; trap 'exit 143' TERM
IFS= read -r instruction || exit 1
test "$instruction" = stop || exit 1
kill -TERM "$capture" || exit 1
status=0; wait "$capture" || status=$?
capture=
exit "$status"
"#
    )
}

#[cfg(test)]
mod tests;
