//! ICMP latency and loss qualification for an already connected target,
//! measured through the one ICMP method, [`oer_hil_net_traffic::icmp`].

use oer_hil_net_traffic::{
    await_network_ready,
    icmp::{self, Summary},
};
use oer_hil_workload::context::Context;
use std::{net::Ipv4Addr, path::Path, time::Duration};

use crate::Result;
use crate::link::WifiCapture as _;
use oer_hil_link::SerialCapture;
use oer_hil_run_bundle_format::run::Comparison;
use oer_hil_run_bundle_format::run::Measurement;
use oer_hil_run_bundle_format::run::MeasurementUnit;

const DEFAULT_COUNT: u16 = 100;
const DEFAULT_INTERVAL: Duration = Duration::from_millis(20);
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(1);
const DEFAULT_PAYLOAD_BYTES: usize = 56;
const NETWORK_READY_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Config {
    pub device: Ipv4Addr,
    pub count: u16,
    pub interval: Duration,
    pub timeout: Duration,
    pub payload_bytes: usize,
    pub maximum_lost: u16,
    pub maximum_p95: Option<Duration>,
}

pub fn run(
    options: Config,
    output: &Path,
    context: &Context<'_>,
    require_no_beacon_loss: bool,
) -> Result<()> {
    run_observed(
        options,
        output,
        context,
        require_no_beacon_loss,
        |_| Ok(()),
        |_| Ok(()),
    )
}

/// [`run`] with two hooks on the live capture: `before` once the network is
/// ready, before the first echo; `after` once the last echo is measured.
/// Their observations of the device frame the ICMP session.
pub(crate) fn run_observed<T>(
    options: Config,
    output: &Path,
    context: &Context<'_>,
    require_no_beacon_loss: bool,
    before: impl FnOnce(&SerialCapture) -> Result<()>,
    after: impl FnOnce(&SerialCapture) -> Result<T>,
) -> Result<T> {
    let mut options = options.validate()?;
    let capture = context.capture(output)?;
    options.device = match await_network_ready(&capture, context.target(), NETWORK_READY_TIMEOUT) {
        Ok(address) => address,
        Err(error) => {
            return capture.finish_with(Err(error));
        }
    };
    if let Err(error) = before(&capture) {
        return capture.finish_with(Err(error));
    }
    let summary = match icmp::measure(&icmp::Ping {
        device: options.device,
        count: options.count,
        interval: options.interval,
        timeout: options.timeout,
        payload_bytes: options.payload_bytes,
        interface: None,
    }) {
        Ok(summary) => summary,
        Err(error) => {
            return capture.finish_with(Err(error));
        }
    };
    let observed = match after(&capture) {
        Ok(observed) => observed,
        Err(error) => {
            return capture.finish_with(Err(error));
        }
    };
    context.measurements.record(measurements(options, &summary));
    let beacon_loss = require_no_beacon_loss.then(|| capture.require_no_beacon_loss());
    capture.finish()?;
    if let Some(result) = beacon_loss {
        result?;
    }
    let acceptance_failure = if options.count - summary.received > options.maximum_lost {
        Some(format!(
            "ICMP lost {} replies at sequences {:?}, above the configured maximum {}",
            options.count - summary.received,
            summary.lost_sequences,
            options.maximum_lost,
        ))
    } else if options.maximum_p95.is_some_and(|maximum| {
        summary.p95_us > u64::try_from(maximum.as_micros()).unwrap_or(u64::MAX)
    }) {
        Some(format!(
            "ICMP p95 {} us exceeds the configured {} us",
            summary.p95_us,
            options.maximum_p95.expect("checked above").as_micros(),
        ))
    } else {
        None
    };
    context.results.observe("icmp", &summary);
    if acceptance_failure.is_none() {
        eprintln!(
            "OPENRADIOHOST result=PASS mode=icmp transmitted={} received={} loss_percent={:.3} \
             readiness_attempts={} min_us={} avg_us={} p50_us={} p95_us={} p99_us={} max_us={}",
            summary.transmitted,
            summary.received,
            summary.loss_percent(),
            summary.readiness_attempts,
            summary.minimum_us,
            summary.average_us,
            summary.p50_us,
            summary.p95_us,
            summary.p99_us,
            summary.maximum_us,
        );
    }
    match acceptance_failure {
        Some(failure) => Err(failure.into()),
        None => Ok(observed),
    }
}

fn measurements(options: Config, summary: &Summary) -> Vec<Measurement> {
    let lost = u64::from(summary.transmitted - summary.received);
    let minimum_received = u64::from(options.count - options.maximum_lost);
    let loss_basis_points = lost
        .saturating_mul(10_000)
        .checked_div(u64::from(summary.transmitted))
        .unwrap_or(0);
    let mut measurements = vec![
        Measurement::observed(
            "icmp.requests.transmitted",
            u64::from(summary.transmitted),
            MeasurementUnit::Count,
        ),
        Measurement::observed(
            "icmp.replies.received",
            u64::from(summary.received),
            MeasurementUnit::Count,
        )
        .evaluated(Comparison::AtLeast, minimum_received),
        Measurement::observed("icmp.replies.lost", lost, MeasurementUnit::Count)
            .evaluated(Comparison::AtMost, u64::from(options.maximum_lost)),
        Measurement::observed("icmp.loss", loss_basis_points, MeasurementUnit::BasisPoints),
        Measurement::observed(
            "icmp.readiness.attempts",
            u64::from(summary.readiness_attempts),
            MeasurementUnit::Count,
        ),
        Measurement::observed(
            "icmp.rtt.minimum",
            summary.minimum_us,
            MeasurementUnit::Microseconds,
        ),
        Measurement::observed(
            "icmp.rtt.average",
            summary.average_us,
            MeasurementUnit::Microseconds,
        ),
        Measurement::observed(
            "icmp.rtt.p50",
            summary.p50_us,
            MeasurementUnit::Microseconds,
        ),
        Measurement::observed(
            "icmp.rtt.p95",
            summary.p95_us,
            MeasurementUnit::Microseconds,
        ),
        Measurement::observed(
            "icmp.rtt.p99",
            summary.p99_us,
            MeasurementUnit::Microseconds,
        ),
        Measurement::observed(
            "icmp.rtt.maximum",
            summary.maximum_us,
            MeasurementUnit::Microseconds,
        ),
    ];
    if let Some(maximum) = options.maximum_p95 {
        let maximum = u64::try_from(maximum.as_micros()).unwrap_or(u64::MAX);
        let p95 = measurements
            .iter_mut()
            .find(|measurement| measurement.name == "icmp.rtt.p95")
            .expect("p95 measurement is present");
        *p95 = p95.clone().evaluated(Comparison::AtMost, maximum);
    }
    measurements
}

impl Default for Config {
    fn default() -> Self {
        Self {
            device: Ipv4Addr::UNSPECIFIED,
            count: DEFAULT_COUNT,
            interval: DEFAULT_INTERVAL,
            timeout: DEFAULT_TIMEOUT,
            payload_bytes: DEFAULT_PAYLOAD_BYTES,
            maximum_lost: 0,
            maximum_p95: None,
        }
    }
}
impl Config {
    pub(crate) fn validate(self) -> Result<Self> {
        if self.count == 0 {
            return Err("ICMP count must be nonzero".into());
        }
        if self.maximum_lost >= self.count {
            return Err("ICMP maximum lost replies must be below the request count".into());
        }
        if self.interval.is_zero() || self.timeout.is_zero() {
            return Err("ICMP interval and timeout must be nonzero".into());
        }
        if self.payload_bytes > icmp::MAX_PAYLOAD_BYTES {
            return Err(
                format!("ICMP payload must be 0..={} bytes", icmp::MAX_PAYLOAD_BYTES).into(),
            );
        }

        Ok(self)
    }
}

#[cfg(test)]
mod tests;
