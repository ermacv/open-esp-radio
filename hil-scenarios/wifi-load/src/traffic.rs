//! The host's end of the load: a paced UDP sender, a receiver that counts
//! the image's load and the echo replies of the RTT probes, and the counts
//! both ends keep of a sequence of datagrams.

use std::{
    net::{SocketAddrV4, UdpSocket},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use oer_services_load::header::{self, Header};

/// How long the sender sleeps at most between two batches.
const BATCH: Duration = Duration::from_millis(1);

/// The host's tally: 2^19 sequences, far more than the longest window
/// sends (60 s at 65 Mbit/s of 1400-byte datagrams is about 350 000).
pub type Tally = oer_services_load::tally::Tally<8192>;
const _: () = if Tally::CAPACITY < 60 * 65_000_000 / (1400 * 8) {
    panic!("the host tally covers the longest window");
};

/// The echo replies of the RTT probes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Echoes {
    pub sent: u64,
    /// Round trips in microseconds, in arrival order.
    pub rtt_us: Vec<u64>,
}

/// What the receiver counted.
#[derive(Clone, Debug, Default)]
pub struct Received {
    pub load: Tally,
    /// When the first and the last distinct load datagram arrived.
    pub first: Option<Instant>,
    pub last: Option<Instant>,
    pub echoes: Echoes,
    /// Datagrams of other runs or without a header.
    pub foreign: u64,
}

/// One run's host socket and the receiver reading it.
pub struct Endpoint {
    pub socket: Arc<UdpSocket>,
    stop: Arc<AtomicBool>,
    received: Arc<Mutex<Received>>,
    reader: Option<std::thread::JoinHandle<()>>,
    started: Instant,
}

impl Endpoint {
    /// Bind `local` and count what arrives for `run`.
    pub fn open(local: SocketAddrV4, run: u32) -> std::io::Result<Self> {
        let socket = Arc::new(UdpSocket::bind(local)?);
        socket.set_read_timeout(Some(Duration::from_millis(50)))?;
        let stop = Arc::new(AtomicBool::new(false));
        let received = Arc::new(Mutex::new(Received::default()));
        let started = Instant::now();
        let reader = {
            let (socket, stop, received) = (socket.clone(), stop.clone(), received.clone());
            std::thread::spawn(move || {
                let mut buffer = vec![0; 2048];
                while !stop.load(Ordering::Relaxed) {
                    let Ok((length, _)) = socket.recv_from(&mut buffer) else {
                        continue;
                    };
                    let at = Instant::now();
                    let payload = buffer.get(..length).unwrap_or_default();
                    let mut received = received.lock().unwrap_or_else(PoisonError::into_inner);
                    match Header::read(payload) {
                        Some(header) if header.run == run && header.echo_reply => {
                            let now = micros(started, at);
                            received
                                .echoes
                                .rtt_us
                                .push(now.saturating_sub(header.sent_us));
                        }
                        Some(header) if header.run == run && !header.echo_request => {
                            use oer_services_load::tally::Arrival;
                            let arrival = received.load.count(header.sequence, length as u64);
                            if matches!(arrival, Arrival::New | Arrival::Late) {
                                received.first.get_or_insert(at);
                                received.last = Some(at);
                            }
                        }
                        _ => received.foreign += 1,
                    }
                }
            })
        };
        Ok(Self {
            socket,
            stop,
            received,
            reader: Some(reader),
            started,
        })
    }

    /// The host's microseconds of this endpoint, the probes' timestamps.
    pub fn now_us(&self) -> u64 {
        micros(self.started, Instant::now())
    }

    /// Stop reading and return what arrived.
    pub fn close(mut self) -> Received {
        self.stop_reading();
        std::mem::take(&mut *self.received.lock().unwrap_or_else(PoisonError::into_inner))
    }

    fn stop_reading(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

/// An endpoint left on an error stops its reader too: no thread keeps the
/// socket.
impl Drop for Endpoint {
    fn drop(&mut self) {
        self.stop_reading();
    }
}

fn micros(since: Instant, at: Instant) -> u64 {
    u64::try_from(at.saturating_duration_since(since).as_micros()).unwrap_or(u64::MAX)
}

/// What the sender sent.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sent {
    pub datagrams: u64,
    pub bytes: u64,
    /// Sends the host's socket refused.
    pub failed: u64,
    pub elapsed: Duration,
}

/// Send run `run`'s load to `peer` from `endpoint`: datagrams of `payload`
/// bytes at `rate_bps` of UDP payload for `duration`, in batches.
pub fn send(
    endpoint: &Endpoint,
    peer: SocketAddrV4,
    run: u32,
    payload: usize,
    rate_bps: u64,
    duration: Duration,
) -> Sent {
    let mut buffer = vec![0x5a; payload.max(header::LENGTH)];
    let interval = Duration::from_secs_f64(payload as f64 * 8.0 / rate_bps.max(1) as f64);
    let started = Instant::now();
    let mut sent = Sent::default();
    let mut sequence = 0_u64;
    while started.elapsed() < duration {
        // Every datagram due by now goes in this batch.
        let due = (started.elapsed().as_secs_f64() / interval.as_secs_f64()) as u64 + 1;
        while sequence < due {
            let header = Header {
                run,
                sequence,
                sent_us: endpoint.now_us(),
                echo_request: false,
                echo_reply: false,
            };
            if header.write(&mut buffer).is_some() && endpoint.socket.send_to(&buffer, peer).is_ok()
            {
                sent.datagrams += 1;
                sent.bytes += buffer.len() as u64;
            } else {
                sent.failed += 1;
            }
            sequence += 1;
        }
        std::thread::sleep(BATCH.min(interval));
    }
    sent.elapsed = started.elapsed();
    sent
}

/// Send RTT probes of run `run` to `peer` every `period` until `stop`; the
/// number sent. Their sequence numbers are their own: the image echoes a
/// probe without counting it as load.
pub fn probe(
    endpoint: &Endpoint,
    peer: SocketAddrV4,
    run: u32,
    period: Duration,
    stop: &AtomicBool,
) -> u64 {
    let mut buffer = [0; header::LENGTH];
    let mut sequence = 0;
    while !stop.load(Ordering::Relaxed) {
        let header = Header {
            run,
            sequence,
            sent_us: endpoint.now_us(),
            echo_request: true,
            echo_reply: false,
        };
        if header.write(&mut buffer).is_some() {
            let _ = endpoint.socket.send_to(&buffer, peer);
        }
        sequence += 1;
        std::thread::sleep(period);
    }
    sequence
}
