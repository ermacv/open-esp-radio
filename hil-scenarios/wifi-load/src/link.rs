//! What the link ran within a window, seen from both ends: the access
//! point's view of the station (the LAB router's `station-link` operation,
//! read before a run and after its drain) and the image's own counts
//! ([`oer_services_load::Link`], over the image's window). Directions are
//! the image's: RX is what the access point sent it.

use std::time::Duration;

use oer_hil_harness::{Cx, RouterOperation};
use oer_services_load::{Link, Rates};
use serde_json::{Value, json};

/// One reading of the access point's view of the station; why there is
/// none when the router could not give it.
pub type Reading = Result<Value, String>;

/// The access point's view of station `mac` now.
pub fn read(cx: &Cx<'_>, mac: &str) -> Reading {
    cx.router(RouterOperation::StationLink {
        mac: mac.to_owned(),
    })
    .map_err(|failure| format!("{failure:?}"))
}

/// The value of `view` at `path`; null when it has none.
fn at<'a>(view: &'a Value, path: &[&str]) -> &'a Value {
    const NULL: &Value = &Value::Null;
    path.iter()
        .try_fold(view, |value, key| value.get(key))
        .unwrap_or(NULL)
}

/// A counter of `view` at `path`.
fn counter(view: &Value, path: &[&str]) -> Option<u64> {
    at(view, path).as_u64()
}

/// The difference of the counter at `path` between `start` and `end`;
/// null when either lacks it or it went back (a new association).
fn delta(start: &Value, end: &Value, path: &[&str]) -> Value {
    match (counter(start, path), counter(end, path)) {
        (Some(start), Some(end)) => end.checked_sub(start).map_or(Value::Null, Value::from),
        _ => Value::Null,
    }
}

/// The access point's view between readings `start` and `end`, `span`
/// apart: its counters' differences, the rates of the last frames each
/// way, its channel width and the in-use channel's survey; why not, when a
/// reading is missing.
pub fn router_window(start: &Reading, end: &Reading, span: Duration) -> Value {
    let (start_view, end_view) = match (start, end) {
        (Ok(start), Ok(end)) => (start, end),
        (Err(error), _) | (_, Err(error)) => return json!({ "unavailable": error }),
    };
    let d = |path: &[&str]| delta(start_view, end_view, path);
    let busy = d(&["survey", "busy_ms"]).as_u64();
    let active = d(&["survey", "active_ms"]).as_u64();
    json!({
        "span_ms": u64::try_from(span.as_millis()).unwrap_or(u64::MAX),
        "channel": at(end_view, &["channel"]),
        "width_mhz": at(end_view, &["width_mhz"]),
        "to_device": {
            "last_bitrate": at(end_view, &["tx_bitrate"]),
            "packets": d(&["tx_packets"]),
            "bytes": d(&["tx_bytes"]),
            "retries": d(&["tx_retries"]),
            "failed": d(&["tx_failed"]),
            "airtime_us": d(&["tx_duration_us"]),
            "tid0_aqm_drops": d(&["tid0_aqm_drops"]),
        },
        "from_device": {
            "last_bitrate": at(end_view, &["rx_bitrate"]),
            "packets": d(&["rx_packets"]),
            "bytes": d(&["rx_bytes"]),
            "airtime_us": d(&["rx_duration_us"]),
            "dropped": d(&["rx_drop_misc"]),
            "signal_avg_dbm": at(end_view, &["signal_avg_dbm"]),
        },
        "survey": {
            "active_ms": active,
            "busy_ms": busy,
            "receive_ms": d(&["survey", "receive_ms"]),
            "bss_receive_ms": d(&["survey", "bss_receive_ms"]),
            "transmit_ms": d(&["survey", "transmit_ms"]),
            "busy_percent": busy.zip(active).filter(|(_, active)| *active != 0)
                .map(|(busy, active)| round(100.0 * busy as f64 / active as f64)),
            "noise_dbm": at(end_view, &["survey", "noise_dbm"]),
        },
    })
}

/// Every nonzero count of `rates` with its name, most first.
fn named(rates: &Rates) -> Vec<(String, u32)> {
    let mut named = Vec::new();
    for (width, guard, counts) in [
        (20, "LGI", &rates.ht20_long_gi),
        (20, "SGI", &rates.ht20_short_gi),
        (40, "LGI", &rates.ht40_long_gi),
        (40, "SGI", &rates.ht40_short_gi),
    ] {
        for (mcs, count) in counts.iter().enumerate() {
            named.push((format!("HT{width} {guard} MCS{mcs}"), *count));
        }
    }
    named.extend([
        ("HT MCS8-31".to_owned(), rates.ht_other),
        ("non-HT".to_owned(), rates.non_ht),
        ("HE".to_owned(), rates.he),
        ("unknown".to_owned(), rates.unknown),
    ]);
    named.retain(|(_, count)| *count != 0);
    named.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    named
}

fn total(rates: &Rates) -> u64 {
    [
        &rates.ht20_long_gi,
        &rates.ht20_short_gi,
        &rates.ht40_long_gi,
        &rates.ht40_short_gi,
    ]
    .into_iter()
    .flatten()
    .chain([&rates.ht_other, &rates.non_ht, &rates.he, &rates.unknown])
    .map(|count| u64::from(*count))
    .sum()
}

/// `rates`' histogram as the report names it: the total, each nonzero
/// rate's count and share, and the shares at 40 MHz, with the short guard
/// interval and at MCS 7.
fn histogram(rates: &Rates) -> Value {
    let all = total(rates);
    let share = |part: u64| (all != 0).then(|| percent(part, all));
    let sum = |counts: &[&[u32; 8]]| -> u64 {
        counts
            .iter()
            .flat_map(|counts| counts.iter())
            .map(|count| u64::from(*count))
            .sum()
    };
    let mcs7 = [
        &rates.ht20_long_gi,
        &rates.ht20_short_gi,
        &rates.ht40_long_gi,
        &rates.ht40_short_gi,
    ]
    .iter()
    .filter_map(|counts| counts.get(7))
    .map(|count| u64::from(*count))
    .sum();
    json!({
        "total": all,
        "rates": named(rates)
            .into_iter()
            .map(|(rate, count)| json!({
                "rate": rate,
                "count": count,
                "percent": share(u64::from(count)),
            }))
            .collect::<Vec<_>>(),
        "ht40_percent": share(sum(&[&rates.ht40_long_gi, &rates.ht40_short_gi])),
        "short_gi_percent": share(sum(&[&rates.ht20_short_gi, &rates.ht40_short_gi])),
        "mcs7_percent": share(mcs7),
    })
}

/// The image's counts of the link within its window: received data MPDUs
/// by rate and A-MPDU containment, and the single data transmissions by
/// their last rate, with their transmissions, retries and ends. The
/// metadata holds no A-MPDU boundary and the image counts no transmitted
/// aggregate: both A-MPDU sizes are null.
pub fn device(link: &Link) -> Value {
    let rx = &link.rx;
    let tx = &link.tx;
    let in_ampdu = u64::from(rx.in_ampdu);
    let contained = in_ampdu + u64::from(rx.single);
    // An ended MPDU with a rate reported its transmissions.
    let sent = total(&tx.mpdus).saturating_sub(u64::from(tx.mpdus.unknown));
    let retries = u64::from(tx.transmissions).saturating_sub(sent);
    json!({
        "rx": {
            "mpdus": histogram(&rx.mpdus),
            "in_ampdu": rx.in_ampdu,
            "single": rx.single,
            "containment_unknown": rx.containment_unknown,
            "in_ampdu_percent": (contained != 0).then(|| percent(in_ampdu, contained)),
            "ampdus": Value::Null,
            "mpdus_per_ampdu": Value::Null,
        },
        "tx": {
            "mpdus": histogram(&tx.mpdus),
            "transmissions": tx.transmissions,
            "retries": retries,
            "transmissions_per_mpdu": (sent != 0)
                .then(|| round(f64::from(tx.transmissions) / sent as f64)),
            "delivered": tx.delivered,
            "ack_timeouts": tx.ack_timeouts,
            "other_ends": tx.other_ends,
            "unsent": tx.unsent,
            "ampdu": Value::Null,
        },
    })
}

/// One line a reader takes in at a glance: the access point's last rates
/// each way and its width, the image's commonest rate each way with its
/// share, the A-MPDU share of what it received and its transmissions per
/// MPDU.
pub fn summary(router: &Value, device: &Value) -> String {
    let text = |value: &Value| value.as_str().unwrap_or("-").to_owned();
    let router = match router.get("unavailable") {
        Some(reason) => format!("AP view unavailable ({})", text(reason)),
        None => format!(
            "AP {} MHz, AP→device {} (retries {}), device→AP {}",
            at(router, &["width_mhz"]),
            text(at(router, &["to_device", "last_bitrate", "text"])),
            at(router, &["to_device", "retries"]),
            text(at(router, &["from_device", "last_bitrate", "text"])),
        ),
    };
    let top = |side: &str| {
        let mpdus = at(device, &[side, "mpdus"]);
        match at(mpdus, &["rates"]).get(0) {
            Some(first) => format!(
                "{} MPDUs, {} {}%",
                at(mpdus, &["total"]),
                text(at(first, &["rate"])),
                at(first, &["percent"])
            ),
            None => "no MPDU".to_owned(),
        }
    };
    format!(
        "{router}; device RX {} (in A-MPDU {}%), device TX {} ({} transmissions/MPDU)",
        top("rx"),
        at(device, &["rx", "in_ampdu_percent"]),
        top("tx"),
        at(device, &["tx", "transmissions_per_mpdu"]),
    )
}

fn round(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn percent(part: u64, whole: u64) -> f64 {
    round(100.0 * part as f64 / whole.max(1) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_services_load::{RxLink, TxLink};

    const SPAN: Duration = Duration::from_millis(6200);

    /// The router's view as the LAB service answers it.
    fn view(packets: u64, retries: u64, busy: u64) -> Value {
        json!({
            "rx_bytes": 1000, "rx_packets": 10, "tx_bytes": packets * 1500,
            "tx_packets": packets, "tx_retries": retries, "tx_failed": 0,
            "rx_drop_misc": null, "rx_duration_us": 100, "tx_duration_us": 2000,
            "signal_avg_dbm": -40,
            "rx_bitrate": { "text": "135.0 MBit/s MCS 7 40MHz", "mcs": 7 },
            "tx_bitrate": { "text": "150.0 MBit/s MCS 7 40MHz short GI", "mcs": 7 },
            "channel": 13, "frequency_mhz": 2472, "width_mhz": 40,
            "survey": { "frequency_mhz": 2472, "noise_dbm": -91, "active_ms": 1000 + busy * 4,
                        "busy_ms": busy, "receive_ms": null, "bss_receive_ms": null,
                        "transmit_ms": 5 },
            "tid0_aqm_drops": null,
        })
    }

    #[test]
    fn the_access_point_s_window_takes_counter_differences_and_the_end_s_rates() {
        let window = router_window(&Ok(view(100, 10, 0)), &Ok(view(4100, 210, 250)), SPAN);
        assert_eq!(window["span_ms"], 6200);
        assert_eq!(window["width_mhz"], 40);
        assert_eq!(window["to_device"]["packets"], 4000);
        assert_eq!(window["to_device"]["retries"], 200);
        assert_eq!(window["to_device"]["bytes"], 6_000_000);
        assert_eq!(
            window["to_device"]["last_bitrate"]["text"],
            "150.0 MBit/s MCS 7 40MHz short GI"
        );
        assert_eq!(window["from_device"]["packets"], 0);
        assert!(window["from_device"]["dropped"].is_null(), "not reported");
        assert!(window["to_device"]["tid0_aqm_drops"].is_null());
        assert_eq!(window["survey"]["busy_ms"], 250);
        assert_eq!(window["survey"]["busy_percent"], 25.0);
        assert!(window["survey"]["receive_ms"].is_null());
        let gone = router_window(&Ok(view(100, 10, 0)), &Ok(view(5, 0, 0)), SPAN);
        assert!(gone["to_device"]["packets"].is_null(), "a new association");
        let missing = router_window(
            &Err("the lab router: no station".to_owned()),
            &Ok(view(1, 0, 0)),
            SPAN,
        );
        assert_eq!(missing["unavailable"], "the lab router: no station");
    }

    #[test]
    fn the_device_s_histograms_name_each_rate_and_its_shares() {
        let mut link = Link {
            rx: RxLink {
                in_ampdu: 900,
                single: 100,
                ..RxLink::default()
            },
            tx: TxLink {
                transmissions: 1100,
                delivered: 995,
                ack_timeouts: 5,
                unsent: 2,
                ..TxLink::default()
            },
        };
        link.rx.mpdus.ht40_short_gi[7] = 900;
        link.rx.mpdus.ht40_long_gi[6] = 60;
        link.rx.mpdus.non_ht = 40;
        link.tx.mpdus.ht20_long_gi[7] = 1000;
        link.tx.mpdus.unknown = 2;
        let value = device(&link);
        let rx = &value["rx"];
        assert_eq!(rx["mpdus"]["total"], 1000);
        assert_eq!(rx["mpdus"]["rates"][0]["rate"], "HT40 SGI MCS7");
        assert_eq!(rx["mpdus"]["rates"][0]["percent"], 90.0);
        assert_eq!(rx["mpdus"]["rates"][1]["rate"], "HT40 LGI MCS6");
        assert_eq!(rx["mpdus"]["rates"][2]["rate"], "non-HT");
        assert_eq!(rx["mpdus"]["ht40_percent"], 96.0);
        assert_eq!(rx["mpdus"]["short_gi_percent"], 90.0);
        assert_eq!(rx["mpdus"]["mcs7_percent"], 90.0);
        assert_eq!(rx["in_ampdu_percent"], 90.0);
        assert!(rx["mpdus_per_ampdu"].is_null());
        let tx = &value["tx"];
        assert_eq!(tx["mpdus"]["total"], 1002);
        assert_eq!(tx["retries"], 100, "1100 transmissions of 1000 sent MPDUs");
        assert_eq!(tx["transmissions_per_mpdu"], 1.1);
        assert_eq!(tx["mpdus"]["ht40_percent"], 0.0);
        let line = summary(
            &router_window(&Ok(view(100, 10, 0)), &Ok(view(200, 20, 0)), SPAN),
            &value,
        );
        assert!(line.contains("AP 40 MHz"), "{line}");
        assert!(line.contains("150.0 MBit/s MCS 7 40MHz short GI"), "{line}");
        assert!(line.contains("HT40 SGI MCS7 90"), "{line}");
        assert!(line.contains("HT20 LGI MCS7"), "{line}");
    }

    #[test]
    fn an_empty_window_has_no_shares() {
        let value = device(&Link::default());
        assert_eq!(value["rx"]["mpdus"]["total"], 0);
        assert!(value["rx"]["mpdus"]["mcs7_percent"].is_null());
        assert!(value["rx"]["in_ampdu_percent"].is_null());
        assert!(value["tx"]["transmissions_per_mpdu"].is_null());
        assert!(summary(&json!({"unavailable": "x"}), &value).contains("no MPDU"));
    }
}
