use std::cell::Cell;

use super::*;

/// Virtual time: a wait ends at once at its deadline, and a send may
/// advance the time by what it takes.
struct VirtualClock<'a>(&'a Cell<Instant>);

impl PacingClock for VirtualClock<'_> {
    fn now(&self) -> Instant {
        self.0.get()
    }

    fn wait_until(&mut self, deadline: Instant) -> Result<()> {
        self.0.set(self.0.get().max(deadline));
        Ok(())
    }
}

/// The slots, from the start, of the payload datagrams `config` sends on
/// virtual time, each send taking `send_takes`.
fn slots(config: Config, send_takes: Duration) -> (Vec<Duration>, HostTransmission) {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let start = Instant::now();
    let time = Cell::new(start);
    let mut sent = Vec::new();
    let transmission = send_with(config, &socket, &mut VirtualClock(&time), |packet| {
        // Terminal markers are four octets.
        if packet.len() != 4 {
            sent.push(time.get() - start);
        }
        time.set(time.get() + send_takes);
        Ok(packet.len())
    })
    .unwrap();
    (sent, transmission)
}

#[test]
fn packet_interval_matches_requested_payload_rate() {
    assert_eq!(
        packet_interval(1_200, 80_000_000).unwrap(),
        Duration::from_micros(120)
    );
    assert_eq!(
        packet_interval(1_200, 10_000_000).unwrap(),
        Duration::from_micros(960)
    );
}

#[test]
fn terminal_marker_is_redundant_and_bounded() {
    let receiver = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).unwrap();
    receiver
        .set_read_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    let sender = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0)).unwrap();
    sender.connect(receiver.local_addr().unwrap()).unwrap();

    send_terminal_markers(&sender).unwrap();

    let mut marker = [0_u8; 4];
    for _ in 0..TERMINAL_MARKERS {
        let (length, source) = receiver.recv_from(&mut marker).unwrap();
        assert_eq!(length, marker.len());
        assert_eq!(source, sender.local_addr().unwrap());
        assert_eq!(i32::from_be_bytes(marker), -1);
    }
}

#[test]
fn failed_sender_retains_admitted_bytes_and_original_io_cause() {
    let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let time = Cell::new(Instant::now());
    let mut calls = 0;
    let error = send_with(
        Config {
            address: Ipv4Addr::LOCALHOST,
            port: 4323,
            rate_bps: 1_000_000,
            duration: Duration::from_secs(1),
            payload: 100,
        },
        &socket,
        &mut VirtualClock(&time),
        |packet| {
            calls += 1;
            if calls == 3 {
                Err(std::io::ErrorKind::ConnectionRefused.into())
            } else {
                Ok(packet.len())
            }
        },
    )
    .unwrap_err();
    let failure = error.downcast_ref::<SendFailure>().unwrap();
    assert_eq!(failure.progress.datagrams, 2);
    assert_eq!(failure.progress.bytes, 200);
    // The I/O cause stays the source: the runner classifies it as an
    // infrastructure failure.
    let source = std::error::Error::source(failure).expect("an I/O cause");
    assert!(source.is::<std::io::Error>());
}

#[test]
fn only_slots_that_start_inside_the_duration_carry_a_datagram() {
    let config = |duration_ms| Config {
        address: Ipv4Addr::LOCALHOST,
        port: 4324,
        // 1 200 octets at 2 Mb/s: one every 4.8 ms, as the AP scenarios.
        rate_bps: 2_000_000,
        duration: Duration::from_millis(duration_ms),
        payload: 1_200,
    };
    let ms = |tenths: u64| Duration::from_micros(tenths * 100);
    // 20 ms: the slots at 0, 4.8, 9.6, 14.4 and 19.2 ms. The next, at
    // 24 ms, starts after the duration and carries nothing.
    let (sent, transmission) = slots(config(20), Duration::ZERO);
    assert_eq!(sent, [ms(0), ms(48), ms(96), ms(144), ms(192)]);
    assert_eq!(transmission.datagrams, 5);
    assert_eq!(transmission.elapsed, ms(192));
    // A slot that starts exactly at the end is outside it.
    let (sent, _) = slots(config(24), Duration::ZERO);
    assert_eq!(sent.len(), 5);
    // 10 s: 2 084 slots, the last at 9 998.4 ms; never the 2 085th at
    // 10 003.2 ms that the receiver's 10 s window cannot hold.
    let (sent, transmission) = slots(config(10_000), Duration::ZERO);
    assert_eq!(transmission.datagrams, 2_084);
    assert_eq!(sent.last(), Some(&ms(99_984)));
}

#[test]
fn a_late_send_catches_up_within_the_duration() {
    // Each send takes 7 ms of a 4.8 ms interval: the sender falls behind
    // and sends each slot at once when the last send ends. The slots that
    // start inside the 20 ms carry the datagrams, however late they leave;
    // the receiver counts them up to the terminal markers.
    let (sent, transmission) = slots(
        Config {
            address: Ipv4Addr::LOCALHOST,
            port: 4325,
            rate_bps: 2_000_000,
            duration: Duration::from_millis(20),
            payload: 1_200,
        },
        Duration::from_millis(7),
    );
    let ms = Duration::from_millis;
    assert_eq!(sent, [ms(0), ms(7), ms(14), ms(21), ms(28)]);
    assert_eq!(transmission.datagrams, 5);
    assert_eq!(transmission.maximum_lateness, Duration::from_micros(8_800));
    assert_eq!(transmission.deadline_resets, 0);
}
