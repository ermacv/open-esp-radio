//! Where the receive path last moved within a level, by boundary, and what
//! the image's radio owner read of it at the window's end
//! ([`oer_hil_services_load::Stall`]): the DMA boundary (descriptors a
//! service pass observed), the driver boundary (units the radio device
//! handed to the station) and the socket boundary (load datagrams the
//! server received). Every time is printed relative to the image's window:
//! milliseconds from its start, negative before it. A boundary resumed when
//! it moved after the snapshot, which the owner took before its turn's
//! receive service. The socket shows only its first datagram after the
//! request, which may precede the snapshot, so it proves no resumption.
//! "Serviced" says the turn called its receive pull, not that the ring
//! moved.

use oer_hil_services_load::{
    MAC_COUNT_NAMES, Mac, Report, RxSnapshot, Stall, StallBoundary, StallMac,
};
use serde_json::{Value, json};

/// `at_us` relative to the window's start, in milliseconds.
fn relative(report: &Report, at_us: u64) -> f64 {
    let start = report.window_start_us;
    let ms = |micros: u64| micros as f64 / 1000.0;
    if at_us >= start {
        ms(at_us - start)
    } else {
        -ms(start - at_us)
    }
}

fn relative_of(report: &Report, at_us: Option<u64>) -> Value {
    at_us.map_or(Value::Null, |at| json!(round(relative(report, at))))
}

fn round(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

/// How a boundary went from the snapshot to the owner's check after the
/// turn's service: whether it moved, its steps and its latest one's time.
fn resumed(report: &Report, before: &StallBoundary, after: &StallBoundary) -> Value {
    let steps = after.count.wrapping_sub(before.count);
    json!({
        "resumed": steps != 0,
        "steps": steps,
        "last_ms": relative_of(report, after.last_us.filter(|_| steps != 0)),
    })
}

/// The MAC's counts that moved between the periodic sample and the
/// snapshot's own reading of the bank, by name.
fn mac_since_sample(mac: &StallMac) -> Value {
    let (Some(periodic), Some(fresh)) = (mac.periodic, mac.fresh) else {
        return Value::Null;
    };
    let moved = Mac::window(&periodic, &fresh);
    let counts: serde_json::Map<_, _> = MAC_COUNT_NAMES
        .iter()
        .enumerate()
        .map(|(index, (name, _))| ((*name).to_owned(), moved.count(index)))
        .filter(|(_, count)| *count != 0)
        .map(|(name, count)| (name, count.into()))
        .collect();
    Value::Object(counts)
}

fn snapshot(report: &Report, snapshot: &RxSnapshot) -> Value {
    let device = &snapshot.device;
    let hardware = snapshot.hardware.map(|hardware| {
        let (walker, session) = (hardware.walker, hardware.session);
        json!({
            "walker": {
                "enabled": walker.enabled,
                "reload_doorbell": walker.reload_doorbell,
                "next": format!("{:#010x}", walker.next_low),
                "next_upper": walker.next_upper,
                "last": format!("{:#010x}", walker.last_low),
                "base": walker.base.map(|base| format!("{base:#010x}")),
                "buffer_full": walker.buffer_full,
                "runtime_state": walker.runtime_state,
            },
            "ring": hardware.ring.map(|ring| json!({
                "base": format!("{:#010x}", ring.base),
                "recycle_start": ring.recycle_start,
                "accepted_tail": ring.accepted_tail,
                "observed": ring.observed,
                "completed": ring.completed,
                "remaining": ring.remaining,
                "reload_pending": ring.reload_pending,
            })),
            "descriptors": {
                "total": session.descriptors,
                "armed": session.armed,
                "done": session.done,
                "software": session.software,
                "reserved_owner": session.reserved_owner,
                "reserved_done": session.reserved_done,
            },
            "buffers": {
                "released": session.released,
                "detached": session.detached,
                "staged": session.staged,
            },
            "session": {
                "masked": session.masked,
                "frozen": session.frozen,
            },
            "interrupts": {
                "line_request_pending": session.line_request_pending,
                "receive_fact_pending": session.receive_fact_pending,
                "mac_irqs": session.mac_irqs,
                "doorbell_unmasks": session.doorbell_unmasks,
                "ignored_unmasks": session.ignored_unmasks,
                "service_passes": session.service_passes,
            },
        })
    });
    json!({
        "at_ms": round(relative(report, snapshot.at_us)),
        "dma": { "count": snapshot.dma.count },
        "driver": { "count": snapshot.driver.count },
        "device": {
            "rx_pending": device.rx_pending,
            "continuation": device.continuation,
            "cut": device.cut,
            "pausing": device.pausing,
            "paused": device.paused,
            "queued": device.queued,
        },
        "hardware": hardware,
    })
}

/// The level's record.
pub fn window(report: &Report, stall: &Stall, mac: &StallMac) -> Value {
    let window_ms = round(relative(report, report.window_end_us));
    let taken = stall.snapshot.as_ref();
    let last = |boundary: fn(&RxSnapshot) -> &StallBoundary| {
        relative_of(report, taken.and_then(|taken| boundary(taken).last_us))
    };
    let resume = taken.map(|taken| match &taken.resume {
        Some(resume) => json!({
            "checked_ms": round(relative(report, resume.checked_us)),
            "after_receive_pull": resume.serviced,
            "dma": resumed(report, &taken.dma, &resume.dma),
            "driver": resumed(report, &taken.driver, &resume.driver),
            "socket": { "first_after_request_ms": relative_of(report, stall.socket_after_us) },
        }),
        None => Value::String("not recorded within the image's wait".to_owned()),
    });
    json!({
        "window_ms": window_ms,
        "requested_ms": round(relative(report, stall.requested_us)),
        "last_progress_ms": {
            "dma": last(|taken| &taken.dma),
            "driver": last(|taken| &taken.driver),
            "socket": relative_of(report, stall.socket_last_us),
        },
        "snapshot": taken.map_or(
            Value::String("not taken within the image's wait".to_owned()),
            |taken| snapshot(report, taken),
        ),
        "resume": resume,
        "mac_since_periodic_sample": {
            "periodic_ms": relative_of(report, mac.periodic_us),
            "counts": mac_since_sample(mac),
        },
    })
}

/// The level's one line: each boundary's last progress, what the snapshot
/// found and whether reception resumed after it.
pub fn summary(report: &Report, stall: &Stall, mac: &StallMac) -> String {
    let time = |at: Option<u64>| {
        at.map_or("never".to_owned(), |at| {
            format!("{:.1} ms", relative(report, at))
        })
    };
    let window = format!(
        "window {:.1} ms; socket last {}",
        relative(report, report.window_end_us),
        time(stall.socket_last_us)
    );
    let Some(taken) = &stall.snapshot else {
        return format!("{window}; no snapshot within the image's wait (the owner did not turn)");
    };
    let mut line = format!(
        "{window}, driver last {}, DMA last {}; snapshot at {:.1} ms",
        time(taken.driver.last_us),
        time(taken.dma.last_us),
        relative(report, taken.at_us),
    );
    match &taken.hardware {
        Some(hardware) => {
            let (walker, session) = (hardware.walker, hardware.session);
            line.push_str(&format!(
                ": walker {}, NEXT {:#x}, LAST {:#x}; descriptors armed {} done {} software {} \
                 of {}; detached {}, staged {}; rx_pending {}, fact pending {:?}, line request {}",
                if walker.enabled { "on" } else { "off" },
                walker.next_low,
                walker.last_low,
                session.armed,
                session.done,
                session.software,
                session.descriptors,
                session.detached,
                session.staged,
                taken.device.rx_pending,
                session.receive_fact_pending,
                session.line_request_pending,
            ));
        }
        None => line.push_str(": no registers held"),
    }
    if let Some(moved) = mac
        .periodic
        .zip(mac.fresh)
        .map(|(periodic, fresh)| Mac::window(&periodic, &fresh))
    {
        line.push_str(&format!(
            "; MAC since its sample at {}: mpdu +{}, fcs_error +{}",
            time(mac.periodic_us),
            moved.count(0),
            moved.count(1),
        ));
    }
    match &taken.resume {
        Some(resume) => {
            let moved = |before: &StallBoundary, after: &StallBoundary| {
                if after.count != before.count {
                    format!("yes ({})", time(after.last_us))
                } else {
                    "no".to_owned()
                }
            };
            line.push_str(&format!(
                "; resumed{}: DMA {}, driver {}; socket after request {}",
                if resume.serviced {
                    ""
                } else {
                    " (no receive pull yet)"
                },
                moved(&taken.dma, &resume.dma),
                moved(&taken.driver, &resume.driver),
                stall
                    .socket_after_us
                    .map_or("no".to_owned(), |at| format!("yes ({})", time(Some(at)))),
            ));
        }
        None => line.push_str("; resume not recorded within the image's wait"),
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_hil_services_load::StallResume;

    fn report() -> Report {
        Report {
            window_start_us: 1_000_000,
            window_end_us: 6_000_000,
            ..Report::default()
        }
    }

    fn boundary(count: u32, last_us: u64) -> StallBoundary {
        StallBoundary {
            count,
            last_us: Some(last_us),
        }
    }

    fn stalled() -> Stall {
        Stall {
            requested_us: 6_000_500,
            socket_last_us: Some(2_834_000),
            socket_after_us: None,
            snapshot: Some(RxSnapshot {
                at_us: 6_000_600,
                dma: boundary(40, 2_833_500),
                driver: boundary(39, 2_833_900),
                resume: Some(StallResume {
                    checked_us: 6_000_700,
                    dma: boundary(40, 2_833_500),
                    driver: boundary(39, 2_833_900),
                    serviced: true,
                }),
                ..RxSnapshot::default()
            }),
        }
    }

    #[test]
    fn last_progress_is_relative_to_the_window_start() {
        let value = window(&report(), &stalled(), &StallMac::default());
        assert_eq!(value["window_ms"], 5000.0);
        assert_eq!(value["last_progress_ms"]["dma"], 1833.5);
        assert_eq!(value["last_progress_ms"]["driver"], 1833.9);
        assert_eq!(value["last_progress_ms"]["socket"], 1834.0);
        assert_eq!(value["requested_ms"], 5000.5);
        assert_eq!(value["snapshot"]["at_ms"], 5000.6);
        let before = Report {
            window_start_us: 3_000_000,
            ..report()
        };
        assert_eq!(
            window(&before, &stalled(), &StallMac::default())["last_progress_ms"]["dma"],
            -166.5,
            "before the window"
        );
    }

    #[test]
    fn a_boundary_resumed_only_when_it_moved_after_the_snapshot() {
        let mut stall = stalled();
        let value = window(&report(), &stall, &StallMac::default());
        assert_eq!(value["resume"]["dma"]["resumed"], false);
        assert_eq!(value["resume"]["dma"]["last_ms"], Value::Null);
        assert_eq!(
            value["resume"]["socket"]["first_after_request_ms"],
            Value::Null
        );
        assert!(
            summary(&report(), &stall, &StallMac::default())
                .ends_with("resumed: DMA no, driver no; socket after request no")
        );
        if let Some(taken) = stall.snapshot.as_mut() {
            taken.resume = Some(StallResume {
                checked_us: 6_000_700,
                dma: boundary(43, 6_000_650),
                driver: boundary(42, 6_000_650),
                serviced: true,
            });
        }
        stall.socket_after_us = Some(6_001_000);
        let value = window(&report(), &stall, &StallMac::default());
        assert_eq!(value["resume"]["after_receive_pull"], true);
        assert_eq!(value["resume"]["dma"]["resumed"], true);
        assert_eq!(value["resume"]["dma"]["steps"], 3);
        assert_eq!(value["resume"]["driver"]["last_ms"], 5000.65);
        assert_eq!(value["resume"]["socket"]["first_after_request_ms"], 5001.0);
    }

    #[test]
    fn a_missing_snapshot_or_resume_says_so() {
        let untaken = Stall {
            snapshot: None,
            ..stalled()
        };
        let value = window(&report(), &untaken, &StallMac::default());
        assert_eq!(value["snapshot"], "not taken within the image's wait");
        assert_eq!(value["resume"], Value::Null);
        assert_eq!(value["last_progress_ms"]["dma"], Value::Null);
        assert!(summary(&report(), &untaken, &StallMac::default()).contains("no snapshot"));
        let mut unserved = stalled();
        if let Some(taken) = unserved.snapshot.as_mut() {
            taken.resume = None;
        }
        let value = window(&report(), &unserved, &StallMac::default());
        assert_eq!(value["resume"], "not recorded within the image's wait");
    }

    #[test]
    fn the_mac_counts_since_its_sample_are_the_banks_difference() {
        let mut periodic = [0; oer_hil_services_load::MAC_COUNTS];
        periodic[0] = 100;
        let mut fresh = periodic;
        fresh[0] = 1_337;
        let mac = StallMac {
            fresh: Some(Mac::new(fresh, 0)),
            periodic: Some(Mac::new(periodic, 0)),
            periodic_us: Some(2_830_000),
        };
        let value = window(&report(), &stalled(), &mac);
        let since = &value["mac_since_periodic_sample"];
        assert_eq!(since["periodic_ms"], 1830.0);
        assert_eq!(since["counts"], json!({ "mpdu": 1237 }));
        assert!(summary(&report(), &stalled(), &mac).contains("mpdu +1237"));
        let unread = StallMac::default();
        assert_eq!(
            window(&report(), &stalled(), &unread)["mac_since_periodic_sample"]["counts"],
            Value::Null
        );
    }
}
