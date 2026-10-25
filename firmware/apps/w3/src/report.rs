//! Version 1 console block consumed by `cargo fw w3 check`.

use core::net::Ipv4Addr;

use oer_diagnostics::Note;

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
    pub bridge: crate::bridge::Counters,
    pub probes: crate::probe::ProbeCounters,
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
            bridge: crate::bridge::Counters::new(),
            probes: crate::probe::ProbeCounters {
                socket_errors: 0,
                timeouts: 0,
                last_socket_error: None,
            },
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

    /// All twelve body rows are emitted on success and on failure, as typed
    /// notes: the host renders the console text `cargo fw w3 check` reads.
    pub async fn notes<E>(
        &self,
        mut emit: impl AsyncFnMut(&Note<'static>) -> Result<(), E>,
    ) -> Result<(), E> {
        if self.service_rx_queue_full != 0
            || self.service_rx_rejected != 0
            || self.port_events_lost != 0
        {
            emit(&Note::w3_rx(
                self.service_rx_queue_full,
                self.service_rx_rejected,
                u32::from(self.service_rx_reasons),
                self.port_events_lost,
            ))
            .await?;
        }
        if self.malformed_ip != 0 || self.queue_drops != 0 {
            emit(&Note::w3_diag(self.malformed_ip, self.queue_drops)).await?;
        }
        if self.probes.socket_errors != 0 || self.probes.timeouts != 0 {
            emit(&Note::w3_probes(
                self.probes.socket_errors,
                self.probes.timeouts,
                self.probes.last_socket_error_text(),
            ))
            .await?;
        }
        if self.bridge.turns != 0 {
            emit(&Note::w3_bridge(
                self.bridge.turns,
                self.bridge.rx_frames,
                self.bridge.rx_max,
            ))
            .await?;
        }
        let gates = self.gates();
        let lease = self.lease.unwrap_or(Lease {
            address: Ipv4Addr::UNSPECIFIED,
            router: Ipv4Addr::UNSPECIFIED,
            prefix: 0,
        });
        emit(&Note::w3_begin()).await?;
        emit(&Note::w3_config(
            u32::from(self.channel),
            self.target,
            self.stand,
        ))
        .await?;
        emit(&Note::w3_association(self.authenticated, self.associated)).await?;
        emit(&Note::w3_lease(
            self.lease.is_some(),
            lease.address.octets(),
            lease.router.octets(),
            u32::from(lease.prefix),
        ))
        .await?;
        emit(&Note::w3_ping(
            self.sent,
            self.received,
            u32::from(INTERVAL_MS),
        ))
        .await?;
        emit(&Note::w3_peer_ping(self.peer_requests, self.peer_replies)).await?;
        emit(&Note::w3_stats(
            self.tx_timeout,
            self.queue_collision,
            self.irq_free as u32,
        ))
        .await?;
        emit(&Note::w3_departure(self.deauth_sent)).await?;
        for (id, pass) in ["a", "b", "c", "d", "f"].into_iter().zip(gates) {
            emit(&Note::w3_gate(id, pass)).await?;
        }
        emit(&Note::w3_end()).await?;
        emit(&if gates.into_iter().all(|gate| gate) {
            Note::w3_pass()
        } else {
            Note::w3_fail()
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use core::{
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };
    use std::{string::String, vec::Vec};

    /// The report's notes wait for no one: the future completes in one poll.
    fn ready<T>(future: impl Future<Output = T>) -> T {
        match pin!(future).poll(&mut Context::from_waker(Waker::noop())) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("a report written to a String never waits"),
        }
    }

    /// The console text the host renders from the report's notes.
    fn text(report: &Report) -> String {
        let mut text = String::new();
        ready(report.notes(async |note| {
            oer_diagnostics::render::note(note, &mut text)?;
            text.push_str("\r\n");
            Ok::<(), core::fmt::Error>(())
        }))
        .expect("String accepts writes");
        text
    }

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
        let text = text(&report);
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
        let text = text(&report);
        assert!(text.ends_with("OER-RESULT: pass w3\r\n"));
    }

    #[test]
    fn probe_failures_are_reported_outside_the_unchanged_console_block() {
        let mut report = passed();
        report.sent = 99;
        report.received = 99;
        report.probes.socket_errors = 1;
        report.probes.timeouts = 2;
        report.probes.last_socket_error = Some(owned_embassy_net::raw::SendError::Unaddressable);
        let text = text(&report);
        assert!(text.starts_with("w3.probes socket_errors=1 timeouts=2 last_socket_error=Some(Unaddressable)\r\nOER-W3-BEGIN v=1\r\n"));
        assert!(text.contains("w3.departure deauth_sent=1"));
        assert!(text.ends_with("OER-W3-END lines=12\r\nOER-RESULT: fail w3\r\n"));
    }

    #[test]
    fn service_rx_diagnostics_precede_the_unchanged_console_contract() {
        let mut report = passed();
        report.service_rx_queue_full = 1;
        report.service_rx_rejected = 2;
        report.service_rx_reasons = 4;
        report.port_events_lost = 3;
        let text = text(&report);
        assert!(text.starts_with(
            "w3.rx queue_full=1 rejected=2 reasons=4 port_events_lost=3\r\nOER-W3-BEGIN v=1\r\n"
        ));
        assert!(text.ends_with("OER-W3-END lines=12\r\nOER-RESULT: pass w3\r\n"));
    }
}
