//! ICMP workload execution and assessment, through the one ICMP method
//! ([`oer_hil_net_traffic::icmp`]).

use std::{net::Ipv4Addr, time::Duration};

use oer_hil_net_traffic::icmp::{Ping, measure};

use crate::Result;
use crate::scenario::access_point::AccessPointIcmp;
use crate::workload::ieee80211::access_point::report::TrafficReport;

/// The laptop client's Wi-Fi interface: the echoes leave through it rather
/// than the laptop's wired route.
const LAPTOP_WIFI_INTERFACE: &str = "wlan0";

pub(super) fn qualify_icmp(
    target: Ipv4Addr,
    bind_wifi_interface: bool,
    icmp: &AccessPointIcmp,
) -> Result<TrafficReport> {
    let AccessPointIcmp {
        count,
        interval_ms,
        timeout_ms,
        payload_bytes,
        maximum_lost: allowed_lost,
        maximum_p95_ms,
    } = *icmp;
    let summary = measure(&Ping {
        device: target,
        count,
        interval: Duration::from_millis(u64::from(interval_ms)),
        timeout: Duration::from_millis(u64::from(timeout_ms)),
        payload_bytes: usize::from(payload_bytes),
        interface: bind_wifi_interface.then(|| LAPTOP_WIFI_INTERFACE.to_owned()),
    })?;
    let lost = summary.lost();
    if lost > allowed_lost {
        return Err(format!(
            "AP ICMP lost {lost}/{count} packets (allowed {allowed_lost}) at sequences {:?}",
            summary.lost_sequences
        )
        .into());
    }
    if let Some(maximum_ms) = maximum_p95_ms
        && summary.p95_us > u64::from(maximum_ms) * 1_000
    {
        return Err(format!(
            "AP ICMP p50={} us p95={} us p99={} us; p95 exceeds {maximum_ms} ms",
            summary.p50_us, summary.p95_us, summary.p99_us
        )
        .into());
    }
    Ok(TrafficReport::Icmp {
        transmitted: count,
        received: summary.received,
        lost,
        p50_micros: summary.p50_us,
        p95_micros: summary.p95_us,
        p99_micros: summary.p99_us,
    })
}
