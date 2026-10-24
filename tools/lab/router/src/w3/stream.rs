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
    ssh::{Result, Ssh},
};

pub struct Capture {
    child: Child,
    output: Option<JoinHandle<Result<String>>>,
    diagnostics: Option<JoinHandle<Result<bool>>>,
    count: u16,
}

fn bounded(reader: impl Read) -> Result<String> {
    let mut bytes = Vec::new();
    reader
        .take(128 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "cannot read router observation")?;
    if bytes.len() > 128 * 1024 {
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
        if !matches!(count, 10 | 100) {
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
        let mut child = ssh.start(&script)?;
        let stdout = child.take_stdout().ok_or("capture has no stdout")?;
        let stderr = child.take_stderr().ok_or("capture has no stderr")?;
        let (ready, readiness) = mpsc::sync_channel(1);
        let output = std::thread::spawn(move || bounded(stdout));
        let diagnostics = std::thread::spawn(move || {
            let mut listening = false;
            let mut dropped = false;
            for line in BufReader::new(stderr.take(8193)).lines() {
                let line = line.map_err(|_| "cannot read capture status")?;
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
            }
            Ok(listening && !dropped)
        });
        let capture = Self {
            child,
            output: Some(output),
            diagnostics: Some(diagnostics),
            count,
        };
        readiness
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| "router capture did not become ready")?;
        Ok(capture)
    }

    pub fn finish(mut self) -> Result<(Vec<super::Echo>, bool)> {
        // Let tcpdump deliver its final buffered packet batch and statistics.
        // Killing SSH as soon as ping reports completion can truncate seq 100.
        // Incomplete/failed captures remain failures after this bounded drain.
        let exited = self
            .child
            .wait_timeout(Some(Duration::from_secs(10)))
            .is_ok_and(|status| status.success());
        let text = self
            .output
            .take()
            .ok_or("missing capture reader")?
            .join()
            .map_err(|_| "capture reader panicked")??;
        let complete = self
            .diagnostics
            .take()
            .ok_or("missing capture status reader")?
            .join()
            .map_err(|_| "capture status reader panicked")??;
        completed_observation(&text, exited, complete, self.count)
    }
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
/// are excluded, including a previous run of the same target MAC.
pub struct Departure {
    child: Child,
    output: Option<JoinHandle<Result<String>>>,
}

impl Departure {
    pub fn start(ssh: &Ssh, target: Mac) -> Result<Self> {
        let pipeline = format!(
            "logread -f -l 1 | awk -v target={} 'NR>1 && /AP-STA-DISCONNECTED/ && index(tolower($0),target) {{print \"departure\"; fflush()}}'",
            quote(&super::mac_text(target))?
        );
        let script = format!("exec timeout 240 sh -c {}", quote(&pipeline)?);
        let mut child = ssh.start(&script)?;
        let stdout = child
            .take_stdout()
            .ok_or("departure observer has no stdout")?;
        // This observer's diagnostics can contain stand identities. Drain and
        // discard them, never include them in errors or evidence.
        let stderr = child
            .take_stderr()
            .ok_or("departure observer has no stderr")?;
        let output = std::thread::spawn(move || {
            let drain = std::thread::spawn(move || {
                let _ = std::io::copy(&mut stderr.take(8192), &mut std::io::sink());
            });
            let result = bounded(stdout);
            let _ = drain.join();
            result
        });
        Ok(Self {
            child,
            output: Some(output),
        })
    }

    pub fn finish(mut self) -> Result<bool> {
        let ended = self
            .child
            .try_wait()
            .map_err(|_| "cannot read departure observer status")?
            .is_some();
        self.child
            .kill()
            .map_err(|_| "cannot stop departure observer")?;
        let text = self
            .output
            .take()
            .ok_or("missing departure reader")?
            .join()
            .map_err(|_| "departure reader panicked")??;
        if text.lines().any(|line| line != "departure") {
            return Err("invalid departure observation".to_owned());
        }
        Ok(!ended && text.lines().count() == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
