//! Shared bounded-burst UDP source for downlink HIL traffic.

use std::{
    hint,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket},
    time::{Duration, Instant},
};

use crate::Result;

const FINAL_SPIN: Duration = Duration::from_micros(30);
const MAX_CATCH_UP_INTERVALS: u32 = 4;
const TERMINAL_MARKERS: usize = 16;
const TERMINAL_MARKER_SPACING: Duration = Duration::from_millis(1);

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub address: Ipv4Addr,
    pub port: u16,
    pub rate_bps: u64,
    pub duration: Duration,
    pub payload: usize,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct HostTransmission {
    pub source: Ipv4Addr,
    pub bytes: u64,
    pub datagrams: u64,
    pub elapsed: Duration,
    pub maximum_lateness: Duration,
    pub maximum_catch_up_datagrams: u32,
    pub deadline_resets: u64,
}

/// A failed sender still owns all admission evidence accumulated before failure.
#[derive(Debug)]
pub struct SendFailure {
    pub progress: HostTransmission,
    cause: Box<dyn std::error::Error + Send + Sync>,
}

impl std::fmt::Display for SendFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "UDP send failed after {} datagrams/{} bytes: {}",
            self.progress.datagrams, self.progress.bytes, self.cause
        )
    }
}
impl std::error::Error for SendFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&*self.cause)
    }
}

impl HostTransmission {
    pub fn throughput_bps(self) -> u64 {
        self.bytes
            .saturating_mul(8)
            .saturating_mul(1_000_000)
            .checked_div(
                u64::try_from(self.elapsed.as_micros())
                    .unwrap_or(u64::MAX)
                    .max(1),
            )
            .unwrap_or(0)
    }

    pub fn maximum_lateness_us(self) -> u64 {
        u64::try_from(self.maximum_lateness.as_micros()).unwrap_or(u64::MAX)
    }
}

/// Send one paced flow from `source`, the source address of the route the
/// caller validated. The socket binds it, so a route that disappears during
/// the flow fails the send rather than leaving through another interface.
pub fn send(source: Ipv4Addr, config: Config) -> Result<HostTransmission> {
    let socket = UdpSocket::bind(SocketAddrV4::new(source, 0))?;
    socket.connect(SocketAddrV4::new(config.address, config.port))?;
    socket.set_write_timeout(Some(Duration::from_secs(2)))?;
    send_with(config, &socket, &mut SystemClock, |packet| {
        socket.send(packet)
    })
}

/// Send one paced flow from an already-bound socket.
///
/// Multi-client AP qualification shares this socket with the reverse
/// target-to-host flow so both directions retain one exact peer endpoint.
pub fn send_on(socket: &UdpSocket, config: Config) -> Result<HostTransmission> {
    socket.set_write_timeout(Some(Duration::from_secs(2)))?;
    send_with(config, socket, &mut SystemClock, |packet| {
        socket.send_to(packet, SocketAddrV4::new(config.address, config.port))
    })
}

/// Where a paced sender reads the time and waits for its next slot.
trait PacingClock {
    fn now(&self) -> Instant;

    fn wait_until(&mut self, deadline: Instant) -> Result<()>;
}

/// The host's monotonic clock.
struct SystemClock;

impl PacingClock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn wait_until(&mut self, deadline: Instant) -> Result<()> {
        wait_until(deadline)
    }
}

/// Send `config`'s datagrams, one in each slot of the packet interval that
/// starts inside the duration, then the terminal markers.
fn send_with(
    config: Config,
    socket: &UdpSocket,
    clock: &mut impl PacingClock,
    mut send_packet: impl FnMut(&[u8]) -> std::io::Result<usize>,
) -> Result<HostTransmission> {
    let source = match socket.local_addr()? {
        SocketAddr::V4(address) => *address.ip(),
        SocketAddr::V6(_) => return Err("paced UDP sender selected an IPv6 source".into()),
    };
    let mut packet = vec![0x5a; config.payload];
    let interval = packet_interval(config.payload, config.rate_bps)?;
    let maximum_catch_up = interval.saturating_mul(MAX_CATCH_UP_INTERVALS);
    let started = clock.now();
    let deadline = started + config.duration;
    let mut next = started;
    let mut bytes = 0_u64;
    let mut datagrams = 0_u64;
    let mut maximum_lateness = Duration::ZERO;
    let mut maximum_catch_up_datagrams = 1_u32;
    let mut deadline_resets = 0_u64;

    let result: Result<()> = (|| {
        // A datagram goes only in a slot inside the duration: the receiver
        // measures exactly the duration from the first datagram, so one
        // sent after waiting past the end would be outside its window.
        while next < deadline {
            oer_process::check_cancelled()?;
            clock.wait_until(next)?;
            let now = clock.now();
            let lateness = now.saturating_duration_since(next);
            maximum_lateness = maximum_lateness.max(lateness);
            if lateness > maximum_catch_up {
                // Do not repay an arbitrary scheduler pause as a line-rate burst.
                // One datagram is sent now and the next deadline starts one exact
                // packet interval later.
                next = now;
                deadline_resets = deadline_resets.saturating_add(1);
            } else {
                let catch_up = u32::try_from(lateness.as_nanos() / interval.as_nanos())
                    .unwrap_or(u32::MAX)
                    .saturating_add(1);
                maximum_catch_up_datagrams = maximum_catch_up_datagrams.max(catch_up);
            }

            packet[..4].copy_from_slice(&i32::try_from(datagrams & i32::MAX as u64)?.to_be_bytes());
            let length = send_packet(&packet)?;
            if length != packet.len() {
                return Err(format!("short UDP send: {length}/{}", packet.len()).into());
            }
            bytes = bytes.saturating_add(length as u64);
            datagrams = datagrams.saturating_add(1);
            next += interval;
        }

        Ok(())
    })();
    let elapsed = clock.now().saturating_duration_since(started);
    let result = result.and_then(|()| send_terminal_markers_with(clock, &mut send_packet));
    let progress = HostTransmission {
        source,
        bytes,
        datagrams,
        elapsed,
        maximum_lateness,
        maximum_catch_up_datagrams,
        deadline_resets,
    };
    result
        .map(|()| progress)
        .map_err(|cause| Box::new(SendFailure { progress, cause }) as _)
}

fn send_terminal_markers_with(
    clock: &mut impl PacingClock,
    send_packet: &mut impl FnMut(&[u8]) -> std::io::Result<usize>,
) -> Result<()> {
    let marker = (-1_i32).to_be_bytes();
    for index in 0..TERMINAL_MARKERS {
        if index != 0 {
            let spaced = clock.now() + TERMINAL_MARKER_SPACING;
            clock.wait_until(spaced)?;
        }
        let length = send_packet(&marker)?;
        if length != marker.len() {
            return Err(format!("short UDP terminal send: {length}/{}", marker.len()).into());
        }
    }
    Ok(())
}

#[cfg(test)]
fn send_terminal_markers(socket: &UdpSocket) -> Result<()> {
    send_terminal_markers_with(&mut SystemClock, &mut |packet| socket.send(packet))
}

fn packet_interval(payload: usize, rate_bps: u64) -> Result<Duration> {
    Ok(Duration::from_nanos(
        u64::try_from((payload as u128 * 8 * 1_000_000_000) / rate_bps as u128)?.max(1),
    ))
}

#[allow(
    clippy::disallowed_methods,
    reason = "host traffic pacing uses a bounded 30 us final spin outside production radio code"
)]
fn wait_until(deadline: Instant) -> Result<()> {
    loop {
        oer_process::check_cancelled()?;
        let now = Instant::now();
        let Some(remaining) = deadline.checked_duration_since(now) else {
            return Ok(());
        };
        if remaining > FINAL_SPIN {
            oer_process::sleep(remaining - FINAL_SPIN)?;
        } else {
            hint::spin_loop();
        }
    }
}

#[cfg(test)]
mod tests;
