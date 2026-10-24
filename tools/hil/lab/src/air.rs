//! Passive laptop capture. Raw pcap bytes exist only in bounded host memory;
//! tshark reads stdin and only decoded evidence leaves this module.

use std::{
    io::{Read, Write},
    process::Stdio,
    sync::mpsc,
    thread::JoinHandle,
    time::Duration,
};

use oer_os_process::owned::Child;

use crate::{Result, config::LabConfig, lock::FixtureLock};
pub mod station;

/// The maximum raw capture, including headers. Exhaustion fails the run.
const MAX_PCAP: usize = 32 * 1024 * 1024;

/// One tshark-decoded management or ACK frame. Addresses and SSIDs stay in
/// memory; callers print only masked identities and aggregate evidence.
#[derive(Clone, PartialEq)]
pub struct Frame {
    pub at_us: u64,
    pub frequency_mhz: u16,
    pub rate_mbps: Option<f32>,
    pub subtype: u8,
    pub source: Option<[u8; 6]>,
    pub receiver: [u8; 6],
    pub bssid: Option<[u8; 6]>,
    pub sequence: Option<u16>,
    pub retry: bool,
    pub ssid: Vec<u8>,
}

/// Owns the stand's laptop radio and its passive capture until finish/drop.
/// The managed interface is restored before the fixture lease is released.
pub struct AirCapture {
    child: Option<Child>,
    reader: Option<JoinHandle<Result<Vec<u8>>>>,
    stats: Option<JoinHandle<Result<u32>>>,
    monitor: bool,
    _lease: Option<FixtureLock>,
}

fn helper(mode: &str, input: &[u8]) -> Result<()> {
    let mut command = oer_os_process::command("sudo");
    command
        .args(["-n", crate::NETWORK_HELPER, mode])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = Child::spawn(&mut command)?;
    child
        .stdin
        .take()
        .ok_or("radio helper has no stdin")?
        .write_all(input)?;
    if !child.wait_timeout(Some(Duration::from_secs(15)))?.success() {
        return Err("laptop radio helper failed".into());
    }
    Ok(())
}

impl AirCapture {
    /// Create a standalone monitor on the laptop AX211 after obtaining the
    /// normal stand, physical-radio and software leases. No frame is injected.
    pub fn start(lab: &LabConfig, channel: u8) -> Result<Self> {
        if !(1..=13).contains(&channel) {
            return Err("capture channel must be 1..=13".into());
        }
        let lease = FixtureLock::acquire_without_device(
            lab,
            oer_hil_model::catalog::requirements::Requirements {
                laptop_air_monitor: true,
                ..Default::default()
            },
        )?;
        Self::start_inner(channel, Some(lease))
    }

    /// Capture inside a HIL workload whose plan already holds the laptop
    /// physical-radio, fixture-software and air leases for the whole capture.
    /// Taking another lease here would deadlock on the enclosing run's radio.
    pub fn start_in_run(channel: u8) -> Result<Self> {
        if !(1..=13).contains(&channel) {
            return Err("capture channel must be 1..=13".into());
        }
        Self::start_inner(channel, None)
    }

    fn start_inner(channel: u8, lease: Option<FixtureLock>) -> Result<Self> {
        let mut capture = Self {
            child: None,
            reader: None,
            stats: None,
            monitor: true,
            _lease: lease,
        };
        helper(
            "observer",
            format!("{} HT20\n", 2407 + u16::from(channel) * 5).as_bytes(),
        )?;
        let mut command = oer_os_process::command("dumpcap");
        // `-w -` is essential: no temporary/ring capture file is created.
        command
            .args(["-i", "mon0", "-P", "-w", "-", "-q"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = Child::spawn(&mut command)?;
        let mut stats = child.take_stderr().ok_or("dumpcap has no status stream")?;
        capture.stats = Some(std::thread::spawn(move || {
            let mut text = String::new();
            stats.by_ref().take(8193).read_to_string(&mut text)?;
            if text.len() > 8192 {
                return Err("capture status exceeded memory bound".into());
            }
            capture_drops(&text)
        }));
        let mut stream = child.take_stdout().ok_or("dumpcap has no stdout")?;
        let (ready, started) = mpsc::sync_channel(1);
        capture.child = Some(child);
        capture.reader = Some(std::thread::spawn(move || {
            let mut header = [0; 24];
            stream.read_exact(&mut header)?;
            if ![0xa1b2c3d4, 0xd4c3b2a1, 0xa1b23c4d, 0x4d3cb2a1]
                .contains(&u32::from_le_bytes(header[..4].try_into()?))
            {
                return Err("dumpcap did not produce pcap".into());
            }
            let _ = ready.send(());
            let mut bytes = header.to_vec();
            stream
                .take((MAX_PCAP - 24 + 1) as u64)
                .read_to_end(&mut bytes)?;
            if bytes.len() > MAX_PCAP {
                return Err("air capture exceeded memory bound".into());
            }
            Ok(bytes)
        }));
        started
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| "dumpcap did not become ready")?;
        Ok(capture)
    }

    /// Stop capture, decode in memory and restore the managed laptop radio.
    pub fn finish(mut self) -> Result<Vec<Frame>> {
        self.stop()?;
        let pcap = self
            .reader
            .take()
            .ok_or("capture reader is missing")?
            .join()
            .map_err(|_| "capture reader panicked")??;
        let drops = self.finish_stats()?;
        if drops != 0 {
            return Err("air capture dropped packets".into());
        }
        decode(&pcap)
    }

    /// Project station metadata in memory; raw pcap and key data never leave
    /// the capture owner. Caller persists only the returned public metadata.
    pub fn finish_station(
        mut self,
        target: [u8; 6],
        ap: [u8; 6],
    ) -> Result<(Vec<station::StationFrame>, u32, station::ProjectionStats)> {
        self.stop()?;
        let pcap = self
            .reader
            .take()
            .ok_or("capture reader is missing")?
            .join()
            .map_err(|_| "capture reader panicked")??;
        let drops = self.finish_stats()?;
        let text = decode_fields(
            &pcap,
            "wlan.fc.type == 0 || wlan.fc.type == 2",
            &station::FIELDS,
        )?;
        Ok((
            station::parse(&text, target, ap)?,
            drops,
            station::projection_stats(&text, ap)?,
        ))
    }

    fn finish_stats(&mut self) -> Result<u32> {
        self.stats
            .take()
            .ok_or("capture status reader is missing")?
            .join()
            .map_err(|_| "capture status reader panicked")?
    }

    fn stop(&mut self) -> Result<()> {
        if let Some(mut child) = self.child.take() {
            child.kill()?;
        }
        if self.monitor {
            helper("managed", b"")?;
            self.monitor = false;
        }
        Ok(())
    }
}

impl Drop for AirCapture {
    fn drop(&mut self) {
        if let Err(error) = oer_os_process::cleanup(|| self.stop()) {
            eprintln!("air: restore failed: {error}");
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        if let Some(stats) = self.stats.take() {
            let _ = stats.join();
        }
    }
}

/// Decode a bounded in-memory pcap. tshark never receives a file path.
pub fn decode(pcap: &[u8]) -> Result<Vec<Frame>> {
    let text = decode_fields(
        pcap,
        "wlan.fc.type_subtype == 4 || wlan.fc.type_subtype == 5 || wlan.fc.type_subtype == 29",
        &[
            "frame.time_epoch",
            "radiotap.channel.freq",
            "radiotap.datarate",
            "wlan.fc.type_subtype",
            "wlan.sa",
            "wlan.ra",
            "wlan.bssid",
            "wlan.seq",
            "wlan.fc.retry",
            "wlan.ssid",
        ],
    )?;
    parse(&text)
}

fn decode_fields(pcap: &[u8], filter: &str, fields: &[&str]) -> Result<String> {
    if pcap.len() > MAX_PCAP {
        return Err("air capture exceeded memory bound".into());
    }
    let mut command = oer_os_process::command("tshark");
    command.args([
        "-n",
        "-r",
        "-",
        "-Y",
        filter,
        "-T",
        "fields",
        "-E",
        "occurrence=f",
    ]);
    for field in fields {
        command.args(["-e", field]);
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = Child::spawn(&mut command)?;
    let mut input = child.stdin.take().ok_or("tshark has no stdin")?;
    let (written, output) = std::thread::scope(|scope| {
        let writer = scope.spawn(move || input.write_all(pcap));
        let output = child.wait_with_output_timeout(Some(Duration::from_secs(20)));
        (writer.join(), output)
    });
    written.map_err(|_| "tshark writer panicked")??;
    let output = output?;
    if !output.status.success() {
        return Err("tshark could not decode the air capture".into());
    }
    String::from_utf8(output.stdout).map_err(|_| "invalid tshark output encoding".into())
}

fn capture_drops(text: &str) -> Result<u32> {
    let mut summaries = text
        .lines()
        .filter(|line| line.starts_with("Packets received/dropped on interface "));
    let summary = summaries
        .next()
        .ok_or("capture has no packet-drop statistics")?;
    if summaries.next().is_some() {
        return Err("capture has repeated packet-drop statistics".into());
    }
    let counts = summary
        .split_once("': ")
        .and_then(|(_, tail)| tail.split_whitespace().next())
        .and_then(|counts| counts.split_once('/'))
        .ok_or("invalid capture statistics")?;
    let _: u32 = counts
        .0
        .parse()
        .map_err(|_| "invalid capture received count")?;
    counts
        .1
        .parse()
        .map_err(|_| "invalid capture dropped count".into())
}

fn mac(value: &str) -> Result<[u8; 6]> {
    value
        .split(':')
        .map(|part| u8::from_str_radix(part, 16))
        .collect::<std::result::Result<Vec<_>, _>>()?
        .try_into()
        .map_err(|_| "invalid decoded MAC".into())
}

fn optional<T>(s: &str, parse: impl FnOnce(&str) -> Result<T>) -> Result<Option<T>> {
    if s.is_empty() {
        Ok(None)
    } else {
        parse(s).map(Some)
    }
}

fn parse(text: &str) -> Result<Vec<Frame>> {
    text.lines()
        .map(|line| {
            let fields: [&str; 10] = line
                .split('\t')
                .collect::<Vec<_>>()
                .try_into()
                .map_err(|_| "invalid tshark field count")?;
            let [
                time,
                frequency,
                rate,
                subtype,
                source,
                receiver,
                bssid,
                sequence,
                retry,
                ssid,
            ] = fields;
            let (seconds, fraction) = time.split_once('.').ok_or("invalid capture timestamp")?;
            if fraction.len() < 6 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
                return Err("invalid capture timestamp precision".into());
            }
            let at_us = seconds
                .parse::<u64>()?
                .checked_mul(1_000_000)
                .and_then(|s| s.checked_add(fraction[..6].parse().ok()?))
                .ok_or("capture timestamp overflow")?;
            let ssid = if ssid == "<MISSING>" {
                String::new()
            } else {
                ssid.replace(':', "")
            };
            if !ssid.is_ascii() || ssid.len() % 2 != 0 {
                return Err("invalid decoded SSID".into());
            }
            Ok(Frame {
                at_us,
                frequency_mhz: frequency.parse()?,
                rate_mbps: optional(rate, |s| Ok(s.parse()?))?,
                subtype: u8::from_str_radix(subtype.strip_prefix("0x").unwrap_or(subtype), 16)?,
                source: optional(source, mac)?,
                receiver: mac(receiver)?,
                bssid: optional(bssid, mac)?,
                sequence: optional(sequence, |s| Ok(s.parse()?))?,
                retry: match retry {
                    "0" | "False" => false,
                    "1" | "True" => true,
                    _ => return Err("invalid retry bit".into()),
                },
                ssid: hex::decode(&ssid).map_err(|_| "invalid digit found in string")?,
            })
        })
        .collect()
}

/// ACKs have no transmitter address. Require an immediately preceding
/// unicast probe to this AP, on the same channel, followed within 1 ms (probe airtime plus SIFS) by
/// an ACK addressed to the target. An unrelated ACK cannot satisfy this.
pub fn acknowledged_probes(frames: &[Frame], target: [u8; 6], ap: [u8; 6]) -> usize {
    frames
        .windows(2)
        .filter(|pair| {
            let [probe, ack] = pair else {
                return false;
            };
            probe.subtype == 4
                && probe.source == Some(target)
                && probe.receiver == ap
                && probe.bssid == Some(ap)
                && ack.subtype == 29
                && ack.receiver == target
                && ack.frequency_mhz == probe.frequency_mhz
                && ack
                    .at_us
                    .checked_sub(probe.at_us)
                    .is_some_and(|us| us <= 1000)
        })
        .count()
}

#[cfg(test)]
mod tests;
