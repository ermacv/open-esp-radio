//! Version 1 console block consumed by `cargo fw w3 check`.

use core::fmt::{self, Write};
use core::net::Ipv4Addr;

use crate::echo::{ECHO_COUNT, INTERVAL_MS};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Lease {
    pub address: Ipv4Addr,
    pub router: Ipv4Addr,
    pub prefix: u8,
}

/// Stores masked identities only. The report cannot retain an SSID or full
/// address, even when it is formatted on an error path.
#[derive(Clone, Copy)]
pub struct Report {
    pub channel: u8,
    pub target: [u8; 2],
    pub stand: [u8; 2],
    pub authenticated: bool,
    pub associated: bool,
    pub lease: Option<Lease>,
    pub malformed_ip: u32,
    pub queue_drops: u32,
    pub service_rx_queue_full: u32,
    pub service_rx_rejected: u32,
    pub service_rx_reasons: u16,
    pub port_events_lost: u32,
    pub cadence_ok: bool,
    pub sent: u32,
    pub received: u32,
    pub peer_requests: u32,
    pub peer_replies: u32,
    pub tx_timeout: u32,
    pub queue_collision: u32,
    pub irq_free: usize,
    pub deauth_sent: bool,
}

impl Report {
    pub const fn new(channel: u8, target: [u8; 6], stand: [u8; 6]) -> Self {
        Self {
            channel,
            target: [target[4], target[5]],
            stand: [stand[4], stand[5]],
            authenticated: false,
            associated: false,
            lease: None,
            malformed_ip: 0,
            queue_drops: 0,
            service_rx_queue_full: 0,
            service_rx_rejected: 0,
            service_rx_reasons: 0,
            port_events_lost: 0,
            cadence_ok: true,
            sent: 0,
            received: 0,
            peer_requests: 0,
            peer_replies: 0,
            tx_timeout: 0,
            queue_collision: 0,
            irq_free: 0,
            deauth_sent: false,
        }
    }

    pub fn gates(&self) -> [bool; 5] {
        let count = u32::from(ECHO_COUNT);
        [
            self.authenticated && self.associated,
            self.lease.is_some(),
            self.sent == count && self.received == count && self.cadence_ok,
            self.peer_requests == count && self.peer_replies == count,
            self.tx_timeout == 0 && self.queue_collision == 0 && self.deauth_sent,
        ]
    }

    /// All twelve body rows are emitted on success and on failure.
    pub fn print(&self, out: &mut impl Write) -> fmt::Result {
        if self.service_rx_queue_full != 0
            || self.service_rx_rejected != 0
            || self.port_events_lost != 0
        {
            writeln!(
                out,
                "w3.rx queue_full={} rejected={} reasons={} port_events_lost={}\r",
                self.service_rx_queue_full,
                self.service_rx_rejected,
                self.service_rx_reasons,
                self.port_events_lost
            )?;
        }
        if self.malformed_ip != 0 || self.queue_drops != 0 {
            writeln!(
                out,
                "w3.diag malformed_ip={} queue_drops={}\r",
                self.malformed_ip, self.queue_drops
            )?;
        }
        let gates = self.gates();
        let lease = self.lease.unwrap_or(Lease {
            address: Ipv4Addr::UNSPECIFIED,
            router: Ipv4Addr::UNSPECIFIED,
            prefix: 0,
        });
        writeln!(out, "OER-W3-BEGIN v=1\r")?;
        writeln!(
            out,
            "w3.config channel={} target=xx:xx:xx:xx:{:02X}:{:02X} stand=xx:xx:xx:xx:{:02X}:{:02X}\r",
            self.channel, self.target[0], self.target[1], self.stand[0], self.stand[1]
        )?;
        writeln!(
            out,
            "w3.association authenticated={} associated={}\r",
            u8::from(self.authenticated),
            u8::from(self.associated)
        )?;
        writeln!(
            out,
            "w3.lease acquired={} address={} router={} prefix={}\r",
            u8::from(self.lease.is_some()),
            lease.address,
            lease.router,
            lease.prefix
        )?;
        writeln!(
            out,
            "w3.ping sent={} received={} interval_ms={INTERVAL_MS}\r",
            self.sent, self.received
        )?;
        writeln!(
            out,
            "w3.peer_ping requests={} replies={}\r",
            self.peer_requests, self.peer_replies
        )?;
        writeln!(
            out,
            "w3.stats tx_timeout={} queue_collision={} irq_free={}\r",
            self.tx_timeout, self.queue_collision, self.irq_free
        )?;
        writeln!(
            out,
            "w3.departure deauth_sent={}\r",
            u8::from(self.deauth_sent)
        )?;
        for (id, pass) in ['a', 'b', 'c', 'd', 'f'].into_iter().zip(gates) {
            writeln!(out, "w3.gate id={id} pass={}\r", u8::from(pass))?;
        }
        writeln!(out, "OER-W3-END lines=12\r")?;
        writeln!(
            out,
            "OER-RESULT: {} w3\r",
            if gates.into_iter().all(|gate| gate) {
                "pass"
            } else {
                "fail"
            }
        )
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::{string::String, vec::Vec};

    fn passed() -> Report {
        let mut report = Report::new(6, [2, 0, 0, 0, 0, 8], [2, 0, 0, 0, 0, 1]);
        report.authenticated = true;
        report.associated = true;
        report.lease = Some(Lease {
            address: Ipv4Addr::new(10, 231, 0, 8),
            router: Ipv4Addr::new(10, 231, 0, 1),
            prefix: 24,
        });
        report.sent = 100;
        report.received = 100;
        report.peer_requests = 100;
        report.peer_replies = 100;
        report.deauth_sent = true;
        report
    }

    #[test]
    fn failures_still_emit_a_complete_masked_block_and_zero_lease() {
        let report = Report::new(6, [0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff], [2, 1, 2, 3, 4, 5]);
        let mut text = String::new();
        report.print(&mut text).expect("String accepts writes");
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 15);
        assert_eq!(lines[0], "OER-W3-BEGIN v=1");
        assert_eq!(
            lines[1],
            "w3.config channel=6 target=xx:xx:xx:xx:EE:FF stand=xx:xx:xx:xx:04:05"
        );
        assert_eq!(
            lines[3],
            "w3.lease acquired=0 address=0.0.0.0 router=0.0.0.0 prefix=0"
        );
        assert_eq!(lines[13], "OER-W3-END lines=12");
        assert_eq!(lines[14], "OER-RESULT: fail w3");
    }

    #[test]
    fn every_echo_and_final_deauthentication_are_required() {
        let mut report = passed();
        assert_eq!(report.gates(), [true; 5]);
        report.received = 99;
        assert!(!report.gates()[2]);
        report.received = 100;
        report.peer_replies = 99;
        assert!(!report.gates()[3]);
        report.peer_replies = 100;
        report.tx_timeout = 1;
        assert!(!report.gates()[4]);
        report.tx_timeout = 0;
        report.queue_collision = 1;
        assert!(!report.gates()[4]);
        report.queue_collision = 0;
        report.deauth_sent = false;
        assert!(!report.gates()[4]);
        report.deauth_sent = true;
        let mut text = String::new();
        report.print(&mut text).expect("String accepts writes");
        assert!(text.ends_with("OER-RESULT: pass w3\r\n"));
    }

    #[test]
    fn service_rx_diagnostics_precede_the_unchanged_console_contract() {
        let mut report = passed();
        report.service_rx_queue_full = 1;
        report.service_rx_rejected = 2;
        report.service_rx_reasons = 4;
        report.port_events_lost = 3;
        let mut text = String::new();
        report.print(&mut text).expect("String accepts writes");
        assert!(text.starts_with(
            "w3.rx queue_full=1 rejected=2 reasons=4 port_events_lost=3\r\nOER-W3-BEGIN v=1\r\n"
        ));
        assert!(text.ends_with("OER-W3-END lines=12\r\nOER-RESULT: pass w3\r\n"));
    }
}
