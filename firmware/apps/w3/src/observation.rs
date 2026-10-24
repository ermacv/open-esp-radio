//! W3 counters at the Ethernet boundary, before/after the native IP stack.
use crate::{
    echo::EchoSeries,
    icmp,
    report::{Lease, Report},
};

pub struct Observations {
    pub station: EchoSeries,
    pub peer: EchoSeries,
    pub lease: Option<Lease>,
    pub malformed: u32,
    pub queue_drops: u32,
    pub bridge: crate::bridge::Counters,
    pub cadence_ok: bool,
    identifier: u16,
    last_request_us: Option<u64>,
    scratch: [u8; 1500],
}
impl Observations {
    pub const fn new(identifier: u16) -> Self {
        Self {
            station: EchoSeries::new(),
            peer: EchoSeries::new(),
            lease: None,
            malformed: 0,
            queue_drops: 0,
            bridge: crate::bridge::Counters::new(),
            cadence_ok: true,
            identifier,
            last_request_us: None,
            scratch: [0; 1500],
        }
    }
    /// `outgoing` means accepted by the service's Ethernet TX queue.
    /// Every parse failure is counted; it supplies no echo observation.
    pub fn observe(&mut self, ether_type: u16, payload: &[u8], outgoing: bool, at_us: u64) {
        if ether_type != 0x0800 {
            return;
        }
        let Some(bytes) = self.scratch.get_mut(..payload.len()) else {
            self.malformed = self.malformed.saturating_add(1);
            return;
        };
        bytes.copy_from_slice(payload);
        let echo = match icmp::parse(bytes) {
            Ok(Some(echo)) => echo,
            Ok(None) => return,
            Err(_) => {
                self.malformed = self.malformed.saturating_add(1);
                return;
            }
        };
        let Some(lease) = self.lease else {
            return;
        };
        let station_to_router = echo.source == lease.address && echo.destination == lease.router;
        let router_to_station = echo.source == lease.router && echo.destination == lease.address;
        if station_to_router && outgoing && !echo.reply && echo.identifier == self.identifier {
            if self.station.request(echo.identifier, echo.sequence) {
                if let Some(previous) = self.last_request_us {
                    self.cadence_ok &= at_us
                        .checked_sub(previous)
                        .is_some_and(|gap| (750_000..=1_250_000).contains(&gap));
                }
                self.last_request_us = Some(at_us);
            }
        } else if router_to_station && !outgoing && echo.reply && echo.identifier == self.identifier
        {
            self.station.reply(echo.identifier, echo.sequence);
        } else if router_to_station && !outgoing && !echo.reply {
            self.peer.request(echo.identifier, echo.sequence);
        } else if station_to_router && outgoing && echo.reply {
            self.peer.reply(echo.identifier, echo.sequence);
        }
    }
    pub const fn identifier(&self) -> u16 {
        self.identifier
    }
    pub fn complete(&self) -> bool {
        self.station.complete() && self.peer.complete()
    }
    pub fn report(&self, report: &mut Report) {
        report.lease = self.lease;
        report.malformed_ip = self.malformed;
        report.queue_drops = self.queue_drops;
        report.bridge = self.bridge;
        report.sent = self.station.requests();
        report.received = self.station.replies();
        report.peer_requests = self.peer.requests();
        report.peer_replies = self.peer.replies();
        report.cadence_ok = self.cadence_ok;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::net::Ipv4Addr;
    use oer_time::{Clock, Duration};
    use oer_time_virtual::VirtualClock;
    use owned_embassy_net::wire::{Icmpv4Message, Icmpv4Packet};
    fn reply(from: Ipv4Addr, to: Ipv4Addr, id: u16, seq: u16) -> [u8; icmp::REQUEST_LEN] {
        let mut bytes = icmp::request(from, to, id, seq);
        let mut echo = Icmpv4Packet::new_unchecked(&mut bytes[20..]);
        echo.set_msg_type(Icmpv4Message::EchoReply);
        echo.fill_checksum();
        bytes
    }
    #[test]
    fn both_directions_count_only_unique_matching_pairs_and_malformed_input_is_counted() {
        let clock = VirtualClock::<1>::new();
        let station = Ipv4Addr::new(10, 231, 0, 8);
        let router = Ipv4Addr::new(10, 231, 0, 1);
        let mut observations = Observations::new(7);
        observations.lease = Some(Lease {
            address: station,
            router,
            prefix: 24,
        });
        for sequence in 1..=100 {
            let at = clock.now().as_micros();
            let own = icmp::request(station, router, 7, sequence);
            let answer = reply(router, station, 7, sequence);
            observations.observe(0x800, &answer, false, at); // before request
            observations.observe(0x800, &own, true, at);
            observations.observe(0x800, &answer, false, at);
            observations.observe(0x800, &answer, false, at); // duplicate
            let peer = icmp::request(router, station, 9, sequence);
            let answer = reply(station, router, 9, sequence);
            observations.observe(0x800, &peer, true, at); // wrong direction
            observations.observe(0x800, &peer, false, at);
            observations.observe(0x800, &answer, true, at);
            clock.advance(Duration::from_secs(1)).expect("virtual time");
        }
        assert!(observations.complete());
        assert!(observations.cadence_ok);
        assert_eq!(observations.station.replies(), 100);
        assert_eq!(observations.peer.replies(), 100);
        observations.observe(0x800, &[], false, clock.now().as_micros());
        assert_eq!(observations.malformed, 1);
        observations.queue_drops = 2;
        let mut report = Report::new(1, [0; 6], [0; 6]);
        observations.report(&mut report);
        assert_eq!(report.malformed_ip, 1);
        assert_eq!(report.queue_drops, 2);
    }
    #[test]
    fn cadence_boundaries_are_inclusive_and_missing_lease_never_counts() {
        let clock = VirtualClock::<1>::new();
        let station = Ipv4Addr::new(10, 231, 0, 8);
        let router = Ipv4Addr::new(10, 231, 0, 1);
        let mut observations = Observations::new(7);
        let packet = icmp::request(station, router, 7, 0);
        observations.observe(0x800, &packet, true, clock.now().as_micros());
        assert_eq!(observations.station.requests(), 0);
        observations.lease = Some(Lease {
            address: station,
            router,
            prefix: 24,
        });
        for (sequence, gap) in [(1, 0), (2, 750_000), (3, 1_250_000)] {
            clock
                .advance(Duration::from_micros(gap))
                .expect("virtual time");
            observations.observe(
                0x800,
                &icmp::request(station, router, 7, sequence),
                true,
                clock.now().as_micros(),
            );
            assert!(observations.cadence_ok);
        }
        clock
            .advance(Duration::from_micros(1_250_001))
            .expect("virtual time");
        observations.observe(
            0x800,
            &icmp::request(station, router, 7, 4),
            true,
            clock.now().as_micros(),
        );
        assert!(!observations.cadence_ok);
    }
}
