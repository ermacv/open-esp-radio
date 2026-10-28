//! Bounded, memory-only SSH observers, including a readiness barrier for
//! tcpdump. Dropping an observer stops its owned SSH process group.

use std::{
    io::{BufRead, BufReader, Read},
    sync::mpsc,
    thread::JoinHandle,
    time::Duration,
};

use oer_os_process::owned::Child;

use crate::{
    mac::Mac,
    router::quote,
    ssh::{CommandObservation, Remote, Result, Ssh, Stderr, read_stderr},
};

pub struct Capture {
    child: Child,
    output: Option<JoinHandle<Result<String>>>,
    diagnostics: Option<JoinHandle<(bool, Stderr)>>,
    count: u16,
    saved: Option<std::fs::File>,
    status_file: Option<std::fs::File>,
    stop: Option<(Ssh, String)>,
    command: Option<CommandObservation>,
}

fn bounded(reader: impl Read, limit: usize) -> Result<String> {
    let mut bytes = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "cannot read router observation")?;
    if bytes.len() > limit {
        return Err("router observation exceeded memory bound".to_owned());
    }
    String::from_utf8(bytes).map_err(|_| "router observation is not UTF-8".to_owned())
}

impl Capture {
    pub fn start(
        ssh: &Ssh,
        interface: &str,
        address: std::net::Ipv4Addr,
        count: u16,
    ) -> Result<Self> {
        Self::begin(ssh, interface, address, count, None, None, None)
    }

    pub(crate) fn start_load(
        ssh: &Ssh,
        interface: &str,
        address: std::net::Ipv4Addr,
        directory: &str,
        saved: std::fs::File,
        status_file: std::fs::File,
    ) -> Result<Self> {
        Self::begin(
            ssh,
            interface,
            address,
            10000,
            Some(directory),
            Some(saved),
            Some(status_file),
        )
    }

    fn begin(
        ssh: &Ssh,
        interface: &str,
        address: std::net::Ipv4Addr,
        count: u16,
        directory: Option<&str>,
        saved: Option<std::fs::File>,
        status_file: Option<std::fs::File>,
    ) -> Result<Self> {
        if !matches!(
            (count, directory.is_some()),
            (10 | 100, false) | (10000, true)
        ) {
            return Err("invalid station echo count".to_owned());
        }
        if !super::usable_address(address) {
            return Err("unusable capture address".to_owned());
        }
        let script = format!(
            "exec timeout {} tcpdump -n -tt -l -c {} -i {} {}",
            u32::from(count) + 25,
            u32::from(count) * 4,
            quote(interface)?,
            quote(&format!("icmp and host {address}"))?
        );
        let (script, stop) = if let Some(directory) = directory {
            let pid = quote(&format!("{directory}/capture.pid"))?;
            let script = format!(
                "printf '%s\\n' \"$$\" > {pid}; exec tcpdump -n -tt -l -i {} {}",
                quote(interface)?,
                quote(&load_filter(address))?
            );
            (
                script,
                Some((ssh.parallel(), format!("kill -INT \"$(cat {pid})\""))),
            )
        } else {
            (script, None)
        };
        let limit = if stop.is_some() {
            4 * 1024 * 1024
        } else {
            128 * 1024
        };
        let mut child = ssh.start(&script)?;
        let stdout = child.take_stdout().ok_or("capture has no stdout")?;
        let stderr = child.take_stderr().ok_or("capture has no stderr")?;
        let (ready, readiness) = mpsc::sync_channel(1);
        let output = std::thread::spawn(move || bounded(stdout, limit));
        let diagnostics = std::thread::spawn(move || {
            let mut listening = false;
            let mut dropped = false;
            let diagnostics = read_stderr(stderr, |line| {
                let line = line.trim_end();
                if line.starts_with("listening on ") {
                    listening = true;
                    let _ = ready.try_send(());
                } else if line.ends_with(" packets dropped by kernel") {
                    dropped |= !line.starts_with("0 ");
                } else if !line.starts_with("tcpdump: verbose output suppressed")
                    && !line.ends_with(" packets captured")
                    && !line.ends_with(" packets received by filter")
                    && !line.trim().is_empty()
                {
                    dropped = true;
                }
            });
            (
                listening && !dropped && diagnostics.error.is_none(),
                diagnostics,
            )
        });
        let capture = Self {
            child,
            output: Some(output),
            diagnostics: Some(diagnostics),
            count,
            saved,
            status_file,
            stop,
            command: Some(ssh.observe(&script)),
        };
        if readiness.recv_timeout(Duration::from_secs(10)).is_err() {
            // Finish the owned process and retain its actual stderr even when
            // tcpdump never announced readiness.
            let _ = capture.finish();
            return Err("router capture did not become ready".into());
        }
        Ok(capture)
    }

    pub fn finish(mut self) -> Result<(Vec<super::Echo>, bool)> {
        // Let tcpdump deliver its final buffered packet batch and statistics.
        // Killing SSH as soon as ping reports completion can truncate seq 100.
        // Incomplete/failed captures remain failures after this bounded drain.
        let load = self.stop.is_some();
        let stopped = self
            .stop
            .take()
            .map(|(mut ssh, script)| ssh.run(&script))
            .transpose();
        let waited = self.child.wait_timeout(Some(Duration::from_secs(10)));
        let status = self.child.try_wait().ok().flatten();
        let (complete, diagnostics) = self
            .diagnostics
            .take()
            .ok_or("missing capture status reader")?
            .join()
            .map_err(|_| "capture status reader panicked")?;
        let error = diagnostics
            .error
            .or_else(|| waited.as_ref().err().map(ToString::to_string));
        if let Some(command) = self.command.take() {
            command.finish(status, &diagnostics.text, error.as_deref());
        }
        let text = self
            .output
            .take()
            .ok_or("missing capture reader")?
            .join()
            .map_err(|_| "capture reader panicked")??;
        if let Some(status_file) = &mut self.status_file {
            save_status(
                status_file,
                waited.as_ref().is_ok_and(|s| s.success()),
                complete,
            )?;
        }
        if load {
            let saved = self.saved.as_mut().ok_or("missing AP load capture sink")?;
            let echoes = decode_load(&text, saved);
            stopped?;
            return Ok((
                echoes?,
                waited.is_ok_and(|status| status.success()) && complete,
            ));
        }
        stopped?;
        completed_observation(
            &text,
            waited.is_ok_and(|status| status.success()),
            complete,
            self.count,
        )
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        if let Some((mut ssh, script)) = self.stop.take() {
            let _ = ssh.run(&script);
        }

        let _ = self.child.kill();
        if let Some(command) = self.command.take() {
            let diagnostics = self
                .diagnostics
                .take()
                .and_then(|reader| reader.join().ok());
            let (stderr, error) = diagnostics.map_or_else(
                || {
                    (
                        String::new(),
                        Some("capture stderr reader unavailable".to_owned()),
                    )
                },
                |(_, diagnostics)| (diagnostics.text, diagnostics.error),
            );
            command.finish(
                self.child.try_wait().ok().flatten(),
                &stderr,
                error
                    .as_deref()
                    .or(Some("capture stopped before completion")),
            );
        }
        if let Some(output) = self.output.take() {
            if let Ok(Ok(text)) = output.join() {
                if let Some(saved) = &mut self.saved {
                    use std::io::Write;
                    let _ = saved.write_all(text.as_bytes());
                }
            }
        }
    }
}

fn save_status(saved: &mut impl std::io::Write, exited: bool, healthy: bool) -> Result<()> {
    writeln!(
        saved,
        "exited={}\nhealthy={}",
        u8::from(exited),
        u8::from(healthy)
    )
    .map_err(|_| "cannot save AP capture status".into())
}

fn decode_load(text: &str, saved: &mut impl std::io::Write) -> Result<Vec<super::Echo>> {
    saved
        .write_all(text.as_bytes())
        .map_err(|_| "cannot save AP load capture")?;
    super::load::parse_capture(text)
}

fn load_filter(address: std::net::Ipv4Addr) -> String {
    format!("icmp and host {address} and (icmp[0] = 0 or icmp[0] = 8)")
}

fn completed_observation(
    text: &str,
    exited: bool,
    healthy: bool,
    count: u16,
) -> Result<(Vec<super::Echo>, bool)> {
    let echoes = super::parse_echoes(text)?;
    let complete = exited && healthy && echoes.len() == usize::from(count) * 4;
    Ok((echoes, complete))
}

/// AP-STA-DISCONNECTED events after subscription only. Historical log records
/// are excluded, including a previous run of the same target MAC: the one
/// history line `logread -l 1` replays arrives once the follow subscription
/// exists, and the observer turns it into its readiness marker.
pub struct Departure {
    child: Child,
    output: Option<JoinHandle<Result<String>>>,
    diagnostics: Option<JoinHandle<Stderr>>,
    command: Option<CommandObservation>,
}

/// What a departure observer saw between its readiness and its finish.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DepartureOutcome {
    /// Exactly one departure of the target.
    Unique,
    /// No departure of the target.
    Absent,
    /// More than one departure of the target.
    Duplicate,
    /// The observer exited before it was finished; its count is not final.
    Ended,
}

impl std::fmt::Display for DepartureOutcome {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Unique => "one departure event",
            Self::Absent => "no departure event",
            Self::Duplicate => "more than one departure event",
            Self::Ended => "departure observer exited before it was finished",
        })
    }
}

const READY: &str = "ready";
const DEPARTED: &str = "departure";
/// The observer output kept in a failed observation's record.
const RECORDED_OUTPUT: usize = 512;

/// The outcome of an observer whose standard output is `text` and which had
/// (`ended`) or had not exited by itself when it was finished. The output is
/// the readiness marker, then one line per departure.
pub fn departure_outcome(text: &str, ended: bool) -> Result<DepartureOutcome> {
    let mut lines = text.lines();
    if lines.next() != Some(READY) {
        return Err("departure observer output has no readiness marker".to_owned());
    }
    let mut departures = 0_usize;
    for line in lines {
        if line != DEPARTED {
            return Err("invalid departure observation".to_owned());
        }
        departures += 1;
    }
    Ok(match (ended, departures) {
        (true, _) => DepartureOutcome::Ended,
        (false, 0) => DepartureOutcome::Absent,
        (false, 1) => DepartureOutcome::Unique,
        (false, _) => DepartureOutcome::Duplicate,
    })
}

/// Read the observer's output within the memory bound, signalling `ready`
/// when its first line is the readiness marker.
fn departure_output(reader: impl Read, ready: mpsc::SyncSender<()>) -> Result<String> {
    let mut reader = BufReader::new(reader.take(128 * 1024 + 1));
    let mut bytes = Vec::new();
    loop {
        let start = bytes.len();
        let read = reader
            .read_until(b'\n', &mut bytes)
            .map_err(|_| "cannot read router observation")?;
        if read == 0 {
            break;
        }
        if start == 0 && bytes.trim_ascii_end() == READY.as_bytes() {
            let _ = ready.try_send(());
        }
    }
    if bytes.len() > 128 * 1024 {
        return Err("router observation exceeded memory bound".to_owned());
    }
    String::from_utf8(bytes).map_err(|_| "router observation is not UTF-8".to_owned())
}

impl Departure {
    /// Subscribe to the router log and return once the subscription is
    /// ready (at most 10 s), so an event the caller causes next is seen.
    pub fn start(ssh: &Ssh, target: Mac) -> Result<Self> {
        let pipeline = format!(
            "logread -f -l 1 | awk -v target={} 'NR==1 {{print \"{READY}\"; fflush(); next}} /AP-STA-DISCONNECTED/ && index(tolower($0),target) {{print \"{DEPARTED}\"; fflush()}}'",
            quote(&super::mac_text(target))?
        );
        let script = format!("exec timeout 240 sh -c {}", quote(&pipeline)?);
        let mut child = ssh.start(&script)?;
        let stdout = child
            .take_stdout()
            .ok_or("departure observer has no stdout")?;
        let stderr = child
            .take_stderr()
            .ok_or("departure observer has no stderr")?;
        let (ready, readiness) = mpsc::sync_channel(1);
        let diagnostics = std::thread::spawn(move || read_stderr(stderr, |_| {}));
        let output = std::thread::spawn(move || departure_output(stdout, ready));
        let mut departure = Self {
            child,
            output: Some(output),
            diagnostics: Some(diagnostics),
            command: Some(ssh.observe(&script)),
        };
        if readiness.recv_timeout(Duration::from_secs(10)).is_err() {
            let reason = "departure observer did not become ready";
            return Err(match departure.close(Some(reason)) {
                Err(error) => error,
                Ok(_) => reason.to_owned(),
            });
        }
        Ok(departure)
    }

    /// Stop the observer and classify what it saw. Anything but
    /// [`DepartureOutcome::Unique`] is recorded with the observer output.
    pub fn finish(mut self) -> Result<DepartureOutcome> {
        self.close(None)
    }

    /// Stop the observer, record it with its output unless it saw one
    /// departure, and return its outcome. `stopped` names why the observer
    /// stops before its caller finished it.
    fn close(&mut self, stopped: Option<&str>) -> Result<DepartureOutcome> {
        let ended = self
            .child
            .try_wait()
            .map(|status| status.is_some())
            .map_err(|_| "cannot read departure observer status".to_owned());
        let _ = self.child.kill();
        let status = self.child.try_wait().ok().flatten();
        let diagnostics = self
            .diagnostics
            .take()
            .and_then(|reader| reader.join().ok())
            .unwrap_or_else(|| Stderr {
                text: String::new(),
                error: Some("departure stderr reader unavailable".to_owned()),
            });
        let text = self
            .output
            .take()
            .ok_or_else(|| "departure reader unavailable".to_owned())
            .and_then(|reader| {
                reader
                    .join()
                    .map_err(|_| "departure reader panicked".to_owned())
            })
            .and_then(|text| text);
        let outcome = match (stopped, &diagnostics.error, &text) {
            (Some(stopped), _, _) => Err(stopped.to_owned()),
            (None, Some(error), _) => Err(format!("departure observer stderr failed: {error}")),
            (None, None, Err(error)) => Err(error.clone()),
            (None, None, Ok(text)) => ended.and_then(|ended| departure_outcome(text, ended)),
        };
        if let Some(command) = self.command.take() {
            let error = match &outcome {
                Ok(DepartureOutcome::Unique) => None,
                Ok(outcome) => Some(outcome.to_string()),
                Err(error) => Some(error.clone()),
            }
            .map(|error| {
                let output = text.as_deref().unwrap_or_default();
                let kept = output
                    .char_indices()
                    .nth(RECORDED_OUTPUT)
                    .map_or(output, |(end, _)| &output[..end]);
                format!("{error}; observer output {kept:?}")
            });
            command.finish(status, &diagnostics.text, error.as_deref());
        }
        outcome
    }
}

impl Drop for Departure {
    fn drop(&mut self) {
        if self.command.is_some() {
            let _ = self.close(Some("departure observer stopped before completion"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn departure_outcome_counts_only_events_after_readiness() {
        assert_eq!(
            departure_outcome("ready\ndeparture\n", false),
            Ok(DepartureOutcome::Unique)
        );
        assert_eq!(
            departure_outcome("ready\n", false),
            Ok(DepartureOutcome::Absent)
        );
        assert_eq!(
            departure_outcome("ready\ndeparture\ndeparture\n", false),
            Ok(DepartureOutcome::Duplicate)
        );
        assert_eq!(
            departure_outcome("ready\ndeparture\n", true),
            Ok(DepartureOutcome::Ended)
        );
        assert_eq!(
            departure_outcome("ready\n", true),
            Ok(DepartureOutcome::Ended)
        );
    }

    #[test]
    fn departure_without_readiness_or_with_foreign_lines_is_an_error() {
        assert!(departure_outcome("", false).is_err());
        assert!(departure_outcome("departure\n", false).is_err());
        assert!(departure_outcome("ready\ndeparture\nother\n", false).is_err());
        assert!(departure_outcome("ready\ndeparture\nother\n", true).is_err());
    }

    #[test]
    fn departure_readiness_is_signalled_by_the_first_line_only() {
        let (ready, readiness) = mpsc::sync_channel(1);
        let text = departure_output(&b"ready\ndeparture\n"[..], ready).unwrap();
        assert_eq!(text, "ready\ndeparture\n");
        assert!(readiness.try_recv().is_ok());
        let (ready, readiness) = mpsc::sync_channel(1);
        departure_output(&b"departure\nready\n"[..], ready).unwrap();
        assert!(readiness.try_recv().is_err());
        let (ready, _) = mpsc::sync_channel(1);
        let oversized = vec![b'x'; 128 * 1024 + 1];
        assert!(departure_output(&oversized[..], ready).is_err());
    }
    #[test]
    fn saved_capture_status_retains_health_independently_of_decoding() {
        let mut saved = Vec::new();
        save_status(&mut saved, true, false).unwrap();
        assert_eq!(saved, b"exited=1\nhealthy=0\n");
    }

    #[test]
    fn tcpdump_stop_separator_is_accepted_only_in_the_load_stream() {
        let row = "1.000000 IP 192.0.2.1 > 192.0.2.8: ICMP echo request, id 7, seq 1, length 64\n";
        let text = format!("\n{row}\n\n");
        let mut saved = Vec::new();
        assert_eq!(decode_load(&text, &mut saved).unwrap().len(), 1);
        assert_eq!(saved, text.as_bytes());
        assert!(super::super::parse_echoes(&text).is_err());
        assert!(decode_load(&format!("{text}garbage\n"), &mut Vec::new()).is_err());
    }

    #[test]
    fn a_failed_load_decode_still_preserves_the_original_observation() {
        let text = "1.000000 IP 192.0.2.1 > 192.0.2.8: ICMP host unreachable\n";
        let mut saved = Vec::new();
        assert!(decode_load(text, &mut saved).is_err());
        assert_eq!(saved, text.as_bytes());
        let text = "1.000000 IP 192.0.2.1 > 192.0.2.8: ICMP echo request, id 7, seq 1, length 64\n";
        let mut saved = Vec::new();
        assert_eq!(decode_load(text, &mut saved).unwrap().len(), 1);
        assert_eq!(saved, text.as_bytes());
    }

    #[test]
    fn load_filter_selects_echoes_without_treating_icmp_errors_as_echo_rows() {
        assert_eq!(
            load_filter("192.0.2.8".parse().unwrap()),
            "icmp and host 192.0.2.8 and (icmp[0] = 0 or icmp[0] = 8)"
        );
    }

    #[test]
    fn load_capture_bounds_do_not_relax_the_standard_console_observation() {
        let text = "1.000000 IP 192.0.2.1 > 192.0.2.8: ICMP echo request, id 7, seq 1, length 64\n"
            .repeat(401);
        assert!(super::super::parse_echoes(&text).is_err());
        assert_eq!(super::super::load::parse_capture(&text).unwrap().len(), 401);
        assert!(bounded(&b"oversized"[..], 2).is_err());
    }

    #[test]
    fn natural_completion_retains_the_last_echo_pair_and_requires_all_400_rows() {
        let text = (1..=100).flat_map(|sequence| {
            [false, true].into_iter().flat_map(move |reverse| {
                [false, true].map(move |reply| {
                    let (source, destination) = if reverse ^ reply { ("192.0.2.1", "192.0.2.8") }
                        else { ("192.0.2.8", "192.0.2.1") };
                    format!("{}.{:06} IP {source} > {destination}: ICMP echo {}, id {}, seq {sequence}, length 64\n",
                        1792590000_u64 + sequence, u32::from(reply) * 1000,
                        if reply { "reply" } else { "request" }, if reverse { 26 } else { 25 })
                })
            })
        }).collect::<String>();
        let (rows, complete) = completed_observation(&text, true, true, 100).unwrap();
        assert!(complete);
        assert_eq!(rows.len(), 400);
        let station = "192.0.2.8".parse().unwrap();
        let router = "192.0.2.1".parse().unwrap();
        assert!(crate::w3::echo_gate(&rows, station, router));
        assert!(crate::w3::echo_gate(&rows, router, station));
        let truncated = text.lines().take(398).collect::<Vec<_>>().join("\n");
        assert!(
            !completed_observation(&truncated, true, true, 100)
                .unwrap()
                .1
        );
        assert!(!completed_observation(&text, false, true, 100).unwrap().1);
        assert!(!completed_observation(&text, true, false, 100).unwrap().1);
        assert!(completed_observation(&format!("{text}truncated"), true, true, 100).is_err());
    }
}
