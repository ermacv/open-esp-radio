//! ICMP echo between the host and a target, the one way the runner measures
//! reachability, loss and round-trip time: Linux datagram ICMP sockets
//! ("ping sockets", open to the groups `net.ipv4.ping_group_range` allows),
//! never the `ping` binary and its text output.

use std::{
    net::{Ipv4Addr, SocketAddrV4},
    os::fd::OwnedFd,
    time::{Duration, Instant},
};

use rustix::{
    event::{PollFd, PollFlags, Timespec, poll},
    io::Errno,
    net::{
        AddressFamily, RecvFlags, SendFlags, SocketFlags, SocketType, connect, ipproto, recv, send,
        socket_with,
    },
};
use serde::Serialize;

use crate::Result;

/// The largest echo payload a measurement sends.
pub const MAX_PAYLOAD_BYTES: usize = 1_400;

/// Readiness attempts before a measurement gives up on an unreachable
/// target.
const READINESS_ATTEMPTS: u8 = 3;

/// One echo measurement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ping {
    pub device: Ipv4Addr,
    pub count: u16,
    pub interval: Duration,
    pub timeout: Duration,
    pub payload_bytes: usize,
    /// The host interface the echoes leave through, when the route alone
    /// would pick another.
    pub interface: Option<String>,
}

/// What an echo measurement observed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct Summary {
    pub transmitted: u16,
    pub received: u16,
    /// Echoes sent until the target first answered, before the measured ones.
    pub readiness_attempts: u8,
    pub lost_sequences: Vec<u16>,
    pub minimum_us: u64,
    pub average_us: u64,
    pub p50_us: u64,
    pub p95_us: u64,
    pub p99_us: u64,
    pub maximum_us: u64,
}

impl Summary {
    pub fn lost(&self) -> u16 {
        self.transmitted - self.received
    }

    pub fn loss_percent(&self) -> f64 {
        f64::from(self.lost()) * 100.0 / f64::from(self.transmitted)
    }
}

/// The nearest-rank `percent`ile of ascending, nonempty `sorted` samples:
/// the one percentile of the runner's latency and interval measurements.
pub fn nearest_rank(sorted: &[u64], percent: usize) -> u64 {
    let rank = sorted.len().saturating_mul(percent).div_ceil(100).max(1);
    sorted[rank.min(sorted.len()) - 1]
}

/// Wait until the target answers, then send `ping.count` echoes paced at
/// `ping.interval`, each awaited up to `ping.timeout`.
pub fn measure(ping: &Ping) -> Result<Summary> {
    if ping.count == 0 || ping.interval.is_zero() || ping.timeout.is_zero() {
        return Err("ICMP count, interval and timeout must be nonzero".into());
    }
    if ping.payload_bytes > MAX_PAYLOAD_BYTES {
        return Err(format!("ICMP payload must be 0..={MAX_PAYLOAD_BYTES} bytes").into());
    }
    let socket = IcmpSocket::connect(ping.device, ping.interface.as_deref())?;
    let readiness_attempts = wait_until_reachable(&socket, ping.payload_bytes, ping.timeout)?;
    let mut samples = Vec::with_capacity(usize::from(ping.count));
    let mut lost_sequences = Vec::new();
    let mut next_send = Instant::now();
    for sequence in 0..ping.count {
        let now = Instant::now();
        if now < next_send {
            oer_process::sleep(next_send - now)?;
        }
        let started = Instant::now();
        socket.send_echo(sequence, ping.payload_bytes)?;
        if socket.wait_for_echo(sequence, ping.timeout)? {
            samples.push(micros(started.elapsed()));
        } else {
            lost_sequences.push(sequence);
        }
        next_send = started + ping.interval;
    }
    if samples.is_empty() {
        return Err(format!("{} sent no echo replies", ping.device).into());
    }
    samples.sort_unstable();
    let total = samples
        .iter()
        .map(|&sample| u128::from(sample))
        .sum::<u128>();
    Ok(Summary {
        transmitted: ping.count,
        received: u16::try_from(samples.len()).expect("sample count is bounded by u16"),
        readiness_attempts,
        lost_sequences,
        minimum_us: samples[0],
        average_us: u64::try_from(total / samples.len() as u128).unwrap_or(u64::MAX),
        p50_us: nearest_rank(&samples, 50),
        p95_us: nearest_rank(&samples, 95),
        p99_us: nearest_rank(&samples, 99),
        maximum_us: *samples.last().expect("nonempty samples"),
    })
}

fn micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

/// Three new requests on a newly opened socket; no queued replies are reused.
pub fn fresh_echo(device: Ipv4Addr) -> Result<()> {
    let socket = IcmpSocket::connect(device, None)?;
    require_fresh_echoes(|sequence| {
        socket.send_echo(sequence, 32)?;
        socket.wait_for_echo(sequence, Duration::from_secs(2))
    })
}

fn require_fresh_echoes(mut exchange: impl FnMut(u16) -> Result<bool>) -> Result<()> {
    for sequence in 0..3 {
        if !exchange(sequence)? {
            return Err(format!("fresh ICMP echo {sequence} did not return").into());
        }
    }
    Ok(())
}

fn wait_until_reachable(
    socket: &IcmpSocket,
    payload_bytes: usize,
    timeout: Duration,
) -> Result<u8> {
    for attempt in 1..=READINESS_ATTEMPTS {
        let sequence = u16::MAX - u16::from(attempt);
        socket.send_echo(sequence, payload_bytes)?;
        if socket.wait_for_echo(sequence, timeout)? {
            return Ok(attempt);
        }
    }
    Err(format!(
        "ICMP target did not become reachable after {READINESS_ATTEMPTS} readiness attempts"
    )
    .into())
}

struct IcmpSocket {
    descriptor: OwnedFd,
}

impl IcmpSocket {
    fn connect(device: Ipv4Addr, interface: Option<&str>) -> Result<Self> {
        // Linux ping sockets are datagram ICMP endpoints available to groups
        // allowed by `net.ipv4.ping_group_range`; no raw-socket capability is
        // needed for the ordinary `cargo hil` path.
        let descriptor = socket_with(
            AddressFamily::INET,
            SocketType::DGRAM,
            SocketFlags::CLOEXEC,
            Some(ipproto::ICMP),
        )?;
        if let Some(interface) = interface {
            oer_hil_fixture::linux_socket::bind_to_device(&descriptor, interface)?;
        }
        connect(&descriptor, &SocketAddrV4::new(device, 0))?;
        Ok(Self { descriptor })
    }

    fn send_echo(&self, sequence: u16, payload_bytes: usize) -> Result<()> {
        let mut packet = vec![0_u8; 8 + payload_bytes];
        packet[0] = 8;
        packet[6..8].copy_from_slice(&sequence.to_be_bytes());
        for (index, byte) in packet[8..].iter_mut().enumerate() {
            *byte = (index as u8).wrapping_add(sequence as u8);
        }
        let checksum = checksum(&packet);
        packet[2..4].copy_from_slice(&checksum.to_be_bytes());
        let sent = send(&self.descriptor, &packet, SendFlags::empty())?;
        if sent != packet.len() {
            return Err(format!("short ICMP send: {sent}/{}", packet.len()).into());
        }
        Ok(())
    }

    fn wait_for_echo(&self, sequence: u16, timeout: Duration) -> Result<bool> {
        let deadline = Instant::now() + timeout;
        loop {
            oer_process::check_cancelled()?;
            let now = Instant::now();
            if now >= deadline {
                return Ok(false);
            }
            let timeout = Timespec::try_from((deadline - now).min(Duration::from_millis(20)))?;
            match poll(
                &mut [PollFd::new(&self.descriptor, PollFlags::IN)],
                Some(&timeout),
            ) {
                Ok(0) | Err(Errno::INTR) => continue,
                Ok(_) => {}
                Err(error) => return Err(error.into()),
            }
            let mut packet = [0_u8; 1_500];
            let received = match recv(&self.descriptor, &mut packet[..], RecvFlags::empty()) {
                Ok((received, _)) => received,
                Err(Errno::INTR | Errno::WOULDBLOCK) => continue,
                Err(error) => return Err(error.into()),
            };
            let packet = &packet[..received];
            if packet.len() >= 8
                && packet[0] == 0
                && packet[1] == 0
                && packet[6..8] == sequence.to_be_bytes()
            {
                return Ok(true);
            }
        }
    }
}

fn checksum(bytes: &[u8]) -> u16 {
    let mut sum = 0_u32;
    for chunk in bytes.chunks(2) {
        let word = if chunk.len() == 2 {
            u16::from_be_bytes([chunk[0], chunk[1]])
        } else {
            u16::from_be_bytes([chunk[0], 0])
        };
        sum += u32::from(word);
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_are_nearest_rank() {
        let samples = (1..=100).map(|value| value * 10).collect::<Vec<_>>();
        assert_eq!(nearest_rank(&samples, 50), 500);
        assert_eq!(nearest_rank(&samples, 95), 950);
        assert_eq!(nearest_rank(&samples, 99), 990);
        assert_eq!(nearest_rank(&[7], 0), 7);
        assert_eq!(nearest_rank(&[1, 2, 3], 100), 3);
        assert_eq!(nearest_rank(&[1, 2, 3], 150), 3);
    }

    #[test]
    fn post_maintenance_exchange_requires_every_fresh_reply() {
        for missing in 0..3 {
            let mut calls = Vec::new();
            assert!(
                require_fresh_echoes(|sequence| {
                    calls.push(sequence);
                    Ok(sequence != missing)
                })
                .is_err()
            );
            assert_eq!(calls.last(), Some(&missing));
        }
        assert!(require_fresh_echoes(|_| Err("disconnected".into())).is_err());
        let mut calls = Vec::new();
        require_fresh_echoes(|sequence| {
            calls.push(sequence);
            Ok(true)
        })
        .unwrap();
        assert_eq!(calls, [0, 1, 2]);
    }

    #[test]
    fn the_echo_checksum_is_the_internet_checksum() {
        // An echo request with identifier and sequence zero and no payload.
        assert_eq!(checksum(&[8, 0, 0, 0, 0, 0, 0, 0]), 0xf7ff);
        assert_eq!(checksum(&[8, 0, 0, 0, 0, 0, 0, 1]), 0xf7fe);
        assert_eq!(checksum(&[1, 2, 3]), 0xfbfd);
        let mut packet = [8, 0, 0, 0, 0, 0, 0, 1, 0xab];
        let sum = checksum(&packet);
        packet[2..4].copy_from_slice(&sum.to_be_bytes());
        assert_eq!(checksum(&packet), 0);
    }

    #[test]
    fn invalid_measurements_are_refused_before_any_socket() {
        let ping = Ping {
            device: Ipv4Addr::LOCALHOST,
            count: 0,
            interval: Duration::from_millis(1),
            timeout: Duration::from_millis(1),
            payload_bytes: 8,
            interface: None,
        };
        assert!(measure(&ping).is_err());
        assert!(
            measure(&Ping {
                count: 1,
                payload_bytes: MAX_PAYLOAD_BYTES + 1,
                ..ping
            })
            .is_err()
        );
    }
}
