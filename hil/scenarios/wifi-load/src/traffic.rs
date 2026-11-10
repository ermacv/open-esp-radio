//! The host's end of the load: a paced UDP sender, a receiver that counts
//! the image's load and the echo replies of the RTT probes, and the counts
//! both ends keep of a sequence of datagrams.

use std::{
    net::{SocketAddrV4, UdpSocket},
    num::NonZeroU32,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use oer_hil_services_load::header::{self, Header};

/// How long the sender sleeps at most between two batches.
const BATCH: Duration = Duration::from_millis(1);

/// The host's tally: 2^19 sequences, far more than the longest window
/// sends (60 s at 65 Mbit/s of 1472-byte datagrams is about 330 000).
pub type Tally = oer_hil_services_load::tally::Tally<8192>;
const _: () = if Tally::CAPACITY < 60 * 65_000_000 / (1472 * 8) {
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
        // A send never holds the sender past its deadline for long.
        socket.set_write_timeout(Some(Duration::from_millis(10)))?;
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
                            use oer_hil_services_load::tally::Arrival;
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

/// How fast a sender offers its load, one direction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Rate {
    /// No pacing: datagrams go as fast as the socket takes them, until the
    /// window ends. A burst size does not apply.
    Unpaced,
    /// This many bits per second of UDP payload on average.
    Paced(NonZeroU32),
}

impl Rate {
    /// A paced rate of `bps` bits per second; `None` for 0 (which is no
    /// rate: [`Rate::Unpaced`] names the unpaced load) and for a rate a
    /// `u32` of bit/s does not hold, as the image's wire rate does.
    pub fn paced(bps: u64) -> Option<Self> {
        u32::try_from(bps)
            .ok()
            .and_then(NonZeroU32::new)
            .map(Self::Paced)
    }

    /// The rate the image's [`Transmit`](oer_hil_services_load::Transmit)
    /// carries: bits per second, where 0 asks the image for no pacing.
    pub fn device_bps(self) -> u32 {
        match self {
            Self::Unpaced => 0,
            Self::Paced(bps) => bps.get(),
        }
    }

    /// Megabits per second; `None` unpaced, which offers no fixed rate.
    pub fn mbps(self) -> Option<f64> {
        match self {
            Self::Unpaced => None,
            Self::Paced(bps) => Some(f64::from(bps.get()) / 1e6),
        }
    }
}

/// What the sender needs of its host: time, a pause, and the wire. The
/// real host is an [`Endpoint`]; a test supplies controlled time and a
/// scripted wire.
pub trait Host {
    /// Monotonic time since the host's start.
    fn now(&self) -> Duration;
    /// Wait for `duration`.
    fn pause(&self, duration: Duration);
    /// Hand one datagram to the wire.
    fn transmit(&self, datagram: &[u8], peer: SocketAddrV4) -> std::io::Result<()>;
}

impl Host for Endpoint {
    fn now(&self) -> Duration {
        self.started.elapsed()
    }

    fn pause(&self, duration: Duration) {
        std::thread::sleep(duration);
    }

    fn transmit(&self, datagram: &[u8], peer: SocketAddrV4) -> std::io::Result<()> {
        self.socket.send_to(datagram, peer).map(|_| ())
    }
}

/// The datagrams due `elapsed` into a run of one each `interval`, `burst`
/// of them together: the bursts begun by then, whole.
fn due(elapsed: Duration, interval: Duration, burst: u16) -> u64 {
    let burst = u64::from(burst.max(1));
    let begun = (elapsed.as_secs_f64() / (interval.as_secs_f64() * burst as f64)) as u64 + 1;
    begun.saturating_mul(burst)
}

/// One datagram of the run: sequence `*sequence` through the wire, counted
/// as sent or refused. The sequence moves on either way, so a refused
/// datagram shows as a gap the receiver counts as lost.
fn send_one(
    host: &impl Host,
    peer: SocketAddrV4,
    run: u32,
    buffer: &mut [u8],
    sequence: &mut u64,
    sent: &mut Sent,
) {
    let header = Header {
        run,
        sequence: *sequence,
        sent_us: u64::try_from(host.now().as_micros()).unwrap_or(u64::MAX),
        echo_request: false,
        echo_reply: false,
    };
    if header.write(buffer).is_some() && host.transmit(buffer, peer).is_ok() {
        sent.datagrams += 1;
        sent.bytes += buffer.len() as u64;
    } else {
        sent.failed += 1;
    }
    *sequence += 1;
}

/// Send run `run`'s load to `peer` from `host`: datagrams of `payload`
/// bytes at `rate` for `duration`. Paced, `burst` of them go together, in
/// batches; unpaced, one after the other as fast as the wire takes them,
/// and `burst` does not apply. A refused send is counted, not retried, and
/// the next datagram follows at once. Nothing is sent once `duration`
/// passed, also within a batch that catches up.
pub fn send(
    host: &impl Host,
    peer: SocketAddrV4,
    run: u32,
    payload: usize,
    rate: Rate,
    burst: u16,
    duration: Duration,
) -> Sent {
    // The filler stays in place; each datagram rewrites only its header.
    let mut buffer = vec![header::FILLER; payload.max(header::LENGTH)];
    let started = host.now();
    let elapsed = || host.now().saturating_sub(started);
    let mut sent = Sent::default();
    let mut sequence = 0_u64;
    match rate {
        Rate::Unpaced => {
            while elapsed() < duration {
                send_one(host, peer, run, &mut buffer, &mut sequence, &mut sent);
            }
        }
        Rate::Paced(bps) => {
            let interval = Duration::from_secs_f64(payload as f64 * 8.0 / f64::from(bps.get()));
            while elapsed() < duration {
                // Every datagram due by now goes in this batch.
                let due = due(elapsed(), interval, burst);
                while sequence < due && elapsed() < duration {
                    send_one(host, peer, run, &mut buffer, &mut sequence, &mut sent);
                }
                host.pause(BATCH.min(interval));
            }
        }
    }
    sent.elapsed = elapsed();
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

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, net::Ipv4Addr};

    use oer_portable_time::{Clock, Instant};
    use oer_portable_time_virtual::{START, SkipClock};

    use super::*;

    /// A host on controlled time: each wire send costs `cost` and fails
    /// when `refuse` says so for its (zero-based) call; nothing is real.
    struct Scripted {
        clock: SkipClock,
        cost: Duration,
        refuse: fn(u64) -> bool,
        calls: RefCell<u64>,
        stamps: RefCell<Vec<Duration>>,
        pauses: RefCell<u64>,
    }

    impl Scripted {
        fn new(cost: Duration, refuse: fn(u64) -> bool) -> Self {
            Self {
                clock: SkipClock::new(),
                cost,
                refuse,
                calls: RefCell::new(0),
                stamps: RefCell::new(Vec::new()),
                pauses: RefCell::new(0),
            }
        }

        fn advance(&self, by: Duration) {
            let by = oer_portable_time::Duration::from_micros(by.as_micros() as u64);
            self.clock
                .advance_to(self.clock.now().checked_add(by).unwrap());
        }
    }

    impl Host for Scripted {
        fn now(&self) -> Duration {
            let since: Instant = self.clock.now();
            Duration::from_micros(since.saturating_duration_since(START).as_micros())
        }

        fn pause(&self, duration: Duration) {
            *self.pauses.borrow_mut() += 1;
            self.advance(duration);
        }

        fn transmit(&self, _: &[u8], _: SocketAddrV4) -> std::io::Result<()> {
            self.stamps.borrow_mut().push(self.now());
            let call = {
                let mut calls = self.calls.borrow_mut();
                *calls += 1;
                *calls - 1
            };
            self.advance(self.cost);
            if (self.refuse)(call) {
                Err(std::io::ErrorKind::WouldBlock.into())
            } else {
                Ok(())
            }
        }
    }

    const PEER: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(10, 231, 0, 2), 5001);
    const WINDOW: Duration = Duration::from_secs(1);

    fn run(host: &Scripted, rate: Rate, burst: u16) -> Sent {
        send(host, PEER, 7, 1472, rate, burst, WINDOW)
    }

    #[test]
    fn a_rate_is_paced_only_when_positive_and_a_u32() {
        assert_eq!(Rate::paced(0), None);
        assert_eq!(Rate::paced(u64::from(u32::MAX) + 1), None);
        let rate = Rate::paced(10_000_000).unwrap();
        assert_eq!(rate.device_bps(), 10_000_000);
        assert_eq!(rate.mbps(), Some(10.0));
        assert_eq!(Rate::Unpaced.device_bps(), 0);
        assert_eq!(Rate::Unpaced.mbps(), None);
    }

    #[test]
    fn an_unpaced_sender_sends_as_fast_as_the_wire_takes_until_the_deadline() {
        // 100 µs per datagram: 10 000 fit the 1 s window; the pacing of
        // any rate would send far fewer, and a stuck interval just one.
        let host = Scripted::new(Duration::from_micros(100), |_| false);
        let sent = run(&host, Rate::Unpaced, 1);
        assert_eq!(sent.datagrams, 10_000);
        assert_eq!(sent.failed, 0);
        assert_eq!(sent.bytes, 10_000 * 1472);
        assert_eq!(sent.elapsed, WINDOW);
        assert_eq!(*host.pauses.borrow(), 0, "unpaced never sleeps");
        let stamps = host.stamps.borrow();
        assert!(stamps.iter().all(|at| *at < WINDOW), "none past the window");
    }

    #[test]
    fn an_unpaced_sender_ignores_the_burst() {
        let host = Scripted::new(Duration::from_micros(100), |_| false);
        let one = run(&host, Rate::Unpaced, 1);
        let host = Scripted::new(Duration::from_micros(100), |_| false);
        let many = run(&host, Rate::Unpaced, 32);
        assert_eq!(one.datagrams, many.datagrams);
    }

    #[test]
    fn a_slow_wire_ends_an_unpaced_run_by_its_deadline() {
        // A send that takes 400 ms (the socket's write timeout stands for
        // far less) still leaves the run at the window: the third send
        // starts at 800 ms, the fourth would start past it.
        let host = Scripted::new(Duration::from_millis(400), |_| false);
        let sent = run(&host, Rate::Unpaced, 1);
        assert_eq!(sent.datagrams, 3);
        assert_eq!(sent.elapsed, Duration::from_millis(1200));
    }

    #[test]
    fn refused_sends_of_an_unpaced_run_are_counted_and_the_run_ends() {
        // Every third send is refused; every one costs time, so the run
        // ends at the window with all counted and none lost to the count.
        let host = Scripted::new(Duration::from_micros(100), |call| call % 3 == 2);
        let sent = run(&host, Rate::Unpaced, 1);
        assert_eq!(sent.datagrams + sent.failed, 10_000);
        assert_eq!(sent.failed, 3333);
        assert_eq!(sent.bytes, sent.datagrams * 1472);
        // All refused: still counted, still ends.
        let host = Scripted::new(Duration::from_micros(100), |_| true);
        let sent = run(&host, Rate::Unpaced, 1);
        assert_eq!(sent.datagrams, 0);
        assert_eq!(sent.failed, 10_000);
    }

    #[test]
    fn a_paced_sender_keeps_its_rate() {
        // 11.776 Mbit/s of 1472-byte datagrams: one each millisecond.
        let rate = Rate::paced(11_776_000).unwrap();
        let host = Scripted::new(Duration::ZERO, |_| false);
        let sent = run(&host, rate, 1);
        assert_eq!(sent.datagrams, 1000);
        assert!(*host.pauses.borrow() > 0, "paced sleeps between batches");
        // Bursts of 8 keep the average, 8 together each 8 ms.
        let host = Scripted::new(Duration::ZERO, |_| false);
        let sent = run(&host, rate, 8);
        assert_eq!(sent.datagrams, 1000);
        let stamps = host.stamps.borrow();
        assert_eq!(stamps.first(), stamps.get(7), "a burst falls due whole");
        assert!(stamps.iter().all(|at| *at < WINDOW));
    }

    #[test]
    fn a_paced_sender_counts_refused_sends_and_ends_by_its_deadline() {
        let rate = Rate::paced(11_776_000).unwrap();
        let host = Scripted::new(Duration::from_micros(50), |call| call % 2 == 0);
        let sent = run(&host, rate, 1);
        assert_eq!(sent.datagrams + sent.failed, 1000);
        assert_eq!(sent.failed, 500);
        assert!(sent.elapsed >= WINDOW);
        assert!(sent.elapsed < WINDOW + Duration::from_millis(2));
    }

    #[test]
    fn a_burst_falls_due_whole_when_it_begins() {
        let interval = Duration::from_millis(10);
        assert_eq!(due(Duration::ZERO, interval, 1), 1);
        assert_eq!(due(Duration::from_millis(25), interval, 1), 3);
        assert_eq!(due(Duration::ZERO, interval, 4), 4);
        assert_eq!(due(Duration::from_millis(39), interval, 4), 4);
        assert_eq!(due(Duration::from_millis(40), interval, 4), 8);
    }
}
