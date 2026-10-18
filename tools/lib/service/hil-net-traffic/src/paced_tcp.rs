//! Bounded, paced host-to-target TCP stream producer for HIL qualification.

use std::{
    hint,
    io::{Read as _, Write as _},
    net::{Ipv4Addr, Shutdown, SocketAddrV4, TcpStream},
    thread,
    time::{Duration, Instant},
};

use oer_hil_protocol::{network::fill_stream_pattern, network::stream_pattern_matches};

use crate::Result;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const WRITE_TIMEOUT: Duration = Duration::from_secs(3);
const READ_POLL_INTERVAL: Duration = Duration::from_millis(250);
const FINAL_SPIN: Duration = Duration::from_micros(30);
const MAX_CATCH_UP_INTERVALS: u32 = 4;

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub address: Ipv4Addr,
    pub port: u16,
    pub rate_bps: u64,
    pub duration: Duration,
    pub chunk_bytes: usize,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct HostTransmission {
    pub bytes: u64,
    pub writes: u64,
    pub elapsed: Duration,
    pub maximum_lateness: Duration,
    pub maximum_catch_up_writes: u32,
    pub deadline_resets: u64,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct HostReception {
    pub bytes: u64,
    pub reads: u64,
    pub elapsed: Duration,
    pub pattern_errors: u64,
    pub eof: bool,
}

impl HostReception {
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

pub fn send(config: Config) -> Result<HostTransmission> {
    send_stream(connect(config)?, config)
}

pub fn receive(config: Config) -> Result<HostReception> {
    receive_stream(connect(config)?, config)
}

pub fn exchange(config: Config) -> Result<(HostTransmission, HostReception)> {
    let stream = connect(config)?;
    let transmit = stream.try_clone()?;
    let sender =
        thread::spawn(move || send_stream(transmit, config).map_err(|error| error.to_string()));
    let reception = receive_stream(stream, config)?;
    let transmission = sender.join().map_err(|_| "TCP sender thread panicked")??;
    Ok((transmission, reception))
}

fn connect(config: Config) -> Result<TcpStream> {
    let address = SocketAddrV4::new(config.address, config.port);
    let stream = TcpStream::connect_timeout(&address.into(), CONNECT_TIMEOUT)?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(WRITE_TIMEOUT))?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    Ok(stream)
}

fn send_stream(mut stream: TcpStream, config: Config) -> Result<HostTransmission> {
    stream.set_nodelay(true)?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let mut chunk = vec![0; config.chunk_bytes];
    let interval = chunk_interval(config.chunk_bytes, config.rate_bps)?;
    let maximum_catch_up = interval.saturating_mul(MAX_CATCH_UP_INTERVALS);
    let started = Instant::now();
    let deadline = started + config.duration;
    let mut next = started;
    let mut bytes = 0_u64;
    let mut writes = 0_u64;
    let mut maximum_lateness = Duration::ZERO;
    let mut maximum_catch_up_writes = 1_u32;
    let mut deadline_resets = 0_u64;

    while Instant::now() < deadline {
        oer_process::check_cancelled()?;
        wait_until(next)?;
        let now = Instant::now();
        let lateness = now.saturating_duration_since(next);
        maximum_lateness = maximum_lateness.max(lateness);
        if lateness > maximum_catch_up {
            next = now;
            deadline_resets = deadline_resets.saturating_add(1);
        } else {
            let catch_up = u32::try_from(lateness.as_nanos() / interval.as_nanos())
                .unwrap_or(u32::MAX)
                .saturating_add(1);
            maximum_catch_up_writes = maximum_catch_up_writes.max(catch_up);
        }

        fill_stream_pattern(&mut chunk, bytes);
        stream.write_all(&chunk)?;
        bytes = bytes.saturating_add(chunk.len() as u64);
        writes = writes.saturating_add(1);
        next += interval;
    }

    let elapsed = started.elapsed();
    stream.shutdown(Shutdown::Write)?;
    Ok(HostTransmission {
        bytes,
        writes,
        elapsed,
        maximum_lateness,
        maximum_catch_up_writes,
        deadline_resets,
    })
}

fn receive_stream(mut stream: TcpStream, config: Config) -> Result<HostReception> {
    // A quiet interval is not EOF. The target owns a bounded payload+FIN
    // drain after the offered-load interval, so poll the blocking host socket
    // until that single absolute deadline instead of treating one arbitrary
    // three-second gap as terminal delivery.
    stream.set_read_timeout(Some(READ_POLL_INTERVAL))?;
    let mut chunk = vec![0; config.chunk_bytes];
    let started = Instant::now();
    let deadline = started + config.duration + WRITE_TIMEOUT;
    let mut bytes = 0_u64;
    let mut reads = 0_u64;
    let mut pattern_errors = 0_u64;
    let eof = loop {
        oer_process::check_cancelled()?;
        match stream.read(&mut chunk) {
            Ok(0) => break true,
            Ok(length) => {
                if !stream_pattern_matches(&chunk[..length], bytes) {
                    pattern_errors = pattern_errors.saturating_add(1);
                }
                bytes = bytes.saturating_add(length as u64);
                reads = reads.saturating_add(1);
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if Instant::now() >= deadline {
                    break false;
                }
            }
            Err(error) => return Err(error.into()),
        }
    };
    Ok(HostReception {
        bytes,
        reads,
        elapsed: started.elapsed(),
        pattern_errors,
        eof,
    })
}

fn chunk_interval(chunk_bytes: usize, rate_bps: u64) -> Result<Duration> {
    Ok(Duration::from_nanos(
        u64::try_from((chunk_bytes as u128 * 8 * 1_000_000_000) / rate_bps as u128)?.max(1),
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
