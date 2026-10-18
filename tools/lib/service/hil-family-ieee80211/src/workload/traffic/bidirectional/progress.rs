//! Preserve both delivery directions before terminal and throughput checks.
use super::{Burst, HostTransmission, TransportEvidence};

pub(super) fn snapshot(
    host: HostTransmission,
    bursts: &[Burst],
    target: Option<TransportEvidence>,
) -> serde_json::Value {
    serde_json::json!({
        "schema": 1,
        "host_offer": {
            "bytes": host.bytes,
            "datagrams": host.datagrams,
            "elapsed_micros": host.elapsed.as_micros(),
            "rate_bps": host.throughput_bps(),
            "maximum_lateness_us": host.maximum_lateness_us(),
            "maximum_catch_up_datagrams": host.maximum_catch_up_datagrams,
            "deadline_resets": host.deadline_resets,
        },
        "host_received_datagrams": bursts.iter().map(|burst| burst.datagrams).sum::<u64>(),
        "host_rx_bursts": bursts,
        "target_transport": target,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{net::Ipv4Addr, time::Duration};

    fn host() -> HostTransmission {
        HostTransmission {
            source: Ipv4Addr::LOCALHOST,
            bytes: 16_000,
            datagrams: 100,
            elapsed: Duration::from_secs(16),
            maximum_lateness: Duration::from_micros(25),
            maximum_catch_up_datagrams: 2,
            deadline_resets: 0,
        }
    }

    #[test]
    fn absent_host_delivery_is_distinct_from_target_socket_acceptance() {
        let evidence = snapshot(
            host(),
            &[],
            Some(TransportEvidence {
                rx_maximum_silence_micros: None,
                rx_bytes: 0,
                tx_bytes: 15_360,
                rx_units: 0,
                tx_units: 96,
                rx_late_bytes: 0,
                rx_late_units: 0,
                elapsed_micros: 16_000_000,
                transport_errors: 0,
            }),
        );
        assert_eq!(evidence["host_received_datagrams"], 0);
        assert_eq!(evidence["target_transport"]["tx_units"], 96);
        assert_eq!(evidence["host_offer"]["datagrams"], 100);
        assert_eq!(evidence["host_rx_bursts"], serde_json::json!([]));
    }

    #[test]
    fn early_rx_termination_and_unqualified_host_bursts_retain_both_directions() {
        let burst = Burst {
            datagrams: 12,
            bytes: 1_920,
            elapsed_us: 100_000,
            started_at_zero: false,
            lowest_sequence: 8,
            ..Burst::default()
        };
        let evidence = snapshot(
            host(),
            &[burst],
            Some(TransportEvidence {
                rx_maximum_silence_micros: None,
                rx_units: 3,
                rx_bytes: 480,
                tx_units: 20,
                tx_bytes: 3_200,
                rx_late_bytes: 0,
                rx_late_units: 0,
                elapsed_micros: 16_000_000,
                transport_errors: 0,
            }),
        );
        assert_eq!(evidence["host_received_datagrams"], 12);
        assert_eq!(evidence["host_rx_bursts"][0]["lowest_sequence"], 8);
        assert_eq!(evidence["host_rx_bursts"][0]["started_at_zero"], false);
        assert_eq!(evidence["target_transport"]["rx_units"], 3);
        assert_eq!(evidence["target_transport"]["tx_units"], 20);
    }

    #[test]
    fn missing_terminal_evidence_retains_host_delivery_without_inventing_target_counts() {
        let evidence = snapshot(
            host(),
            &[Burst {
                datagrams: 12,
                ..Burst::default()
            }],
            None,
        );
        assert_eq!(evidence["host_offer"]["datagrams"], 100);
        assert_eq!(evidence["host_received_datagrams"], 12);
        assert!(evidence["target_transport"].is_null());
    }
}
