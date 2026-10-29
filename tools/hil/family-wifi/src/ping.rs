//! Informational peer latency, independent of the station's acceptance gates.
use oer_hil_runs::format::run::{Measurement, MeasurementUnit};
use oer_hil_scenario::workload::{measurements::Recorder, results::Results};
use oer_lab_router::station::Ping;

#[derive(Clone, Copy)]
pub(super) enum Window {
    GroupKeyChange,
    AfterReconnect,
}
impl Window {
    fn name(self) -> &'static str {
        match self {
            Self::GroupKeyChange => "group-key-change",
            Self::AfterReconnect => "after-reconnect",
        }
    }
}

pub(super) fn record(results: &Results, measurements: &Recorder, window: Window, ping: &Ping) {
    let prefix = format!("wifi-station.ping.{}", window.name());
    results.observe(&prefix, ping);
    if let Some(rtt) = &ping.rtt {
        measurements.record(
            [
                ("min", Some(rtt.min_us)),
                ("avg", Some(rtt.avg_us)),
                ("max", Some(rtt.max_us)),
                ("mdev", rtt.mdev_us),
            ]
            .into_iter()
            .filter_map(|(statistic, value)| {
                value.map(|value| {
                    Measurement::observed(
                        format!("{prefix}.rtt-us.{statistic}"),
                        value,
                        MeasurementUnit::Microseconds,
                    )
                })
            }),
        );
    }
}

#[cfg(test)]
mod tests;
