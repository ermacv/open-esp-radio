//! Delivery evidence retained even when no burst reaches qualification criteria.
use super::Burst;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub(super) struct DeliveryProgress {
    pub target_socket_accepted_datagrams: u64,
    pub host_received_datagrams: u64,
    pub host_maximum_interarrival_us: Option<u64>,
    pub stack_driver_tx_accepted: Option<u64>,
    pub stack_driver_rx_delivered: Option<u64>,
    pub rx_network_enqueued: Option<u64>,
    pub rx_network_dropped: Option<u64>,
    pub rx_network_pool_exhausted: Option<u64>,
}

impl DeliveryProgress {
    pub fn new(target_socket_accepted_datagrams: u64, bursts: &[Burst], log: &str) -> Self {
        Self {
            target_socket_accepted_datagrams,
            host_received_datagrams: bursts.iter().map(|burst| burst.datagrams).sum(),
            host_maximum_interarrival_us: bursts
                .iter()
                .filter(|burst| burst.datagrams > 1)
                .map(|burst| burst.maximum_interarrival_us)
                .max(),
            stack_driver_tx_accepted: counter(log, "hil-net:", "tx_accepted"),
            stack_driver_rx_delivered: counter(log, "hil-net:", "rx_delivered"),
            rx_network_enqueued: counter(log, "hil-tx-ingress:", "enqueued"),
            rx_network_dropped: counter(log, "hil-tx-ingress:", "dropped"),
            rx_network_pool_exhausted: counter(log, "hil-tx-ingress:", "pool_exhausted"),
        }
    }

    pub fn no_delivery_message(&self) -> String {
        format!(
            "UDP delivery never started: host_received_datagrams=0 target_socket_accepted_datagrams={}; \
             stack_driver_tx_accepted={:?} stack_driver_rx_delivered={:?} \
             rx_network_pool_exhausted={:?}; see delivery-progress.json and uart.log; \
             socket acceptance does not prove radio transmission",
            self.target_socket_accepted_datagrams,
            self.stack_driver_tx_accepted,
            self.stack_driver_rx_delivered,
            self.rx_network_pool_exhausted,
        )
    }
}

fn counter(log: &str, record: &str, name: &str) -> Option<u64> {
    log.lines()
        .filter_map(|line| line.split_once(record).map(|(_, fields)| fields))
        .flat_map(str::split_whitespace)
        .filter_map(|field| field.split_once('='))
        .find_map(|(key, value)| (key == name).then(|| value.parse().ok()).flatten())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_delivery_retains_socket_admission_and_the_rx_failure_boundary() {
        let evidence = DeliveryProgress::new(
            96,
            &[],
            "hil-net: tx_ready=90 tx_accepted=6\nhil-net: rx_empty=90 rx_delivered=0\n\
             hil-tx-ingress: publications=9 enqueued=0 dropped=9 pool_exhausted=9\n",
        );
        assert_eq!(evidence.host_received_datagrams, 0);
        assert_eq!(evidence.target_socket_accepted_datagrams, 96);
        assert_eq!(evidence.stack_driver_tx_accepted, Some(6));
        assert_eq!(evidence.stack_driver_rx_delivered, Some(0));
        assert_eq!(evidence.rx_network_pool_exhausted, Some(9));
        assert!(
            evidence
                .no_delivery_message()
                .contains("pool_exhausted=Some(9)")
        );
    }

    #[test]
    fn absent_diagnostics_remain_unknown_and_partial_delivery_is_retained() {
        let evidence = DeliveryProgress::new(
            96,
            &[Burst {
                datagrams: 12,
                maximum_interarrival_us: 50_000,
                ..Burst::default()
            }],
            "unrelated: rx_delivered=42\nhil-net: tx_accepted=invalid\n",
        );
        assert_eq!(evidence.host_received_datagrams, 12);
        assert_eq!(evidence.host_maximum_interarrival_us, Some(50_000));
        assert_eq!(evidence.stack_driver_rx_delivered, None);
        assert_eq!(evidence.stack_driver_tx_accepted, None);
        assert_eq!(evidence.rx_network_pool_exhausted, None);
    }
}
