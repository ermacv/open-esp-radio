//! Manually selected 154 idle hold; never stands in for associated Wi-Fi traffic.
use crate::Result;
use oer_hil_protocol::phy::*;
use oer_hil_scenario::workload::context::Context;
use std::{
    path::Path,
    time::{Duration, Instant},
};

pub(crate) fn run(ticks: u8, output: &Path, context: &Context<'_>) -> Result<()> {
    context.with_capture(&output.join("hold"), |capture| {
        capture.discover(Duration::from_secs(15))?;
        let ready = capture.request(
            0,
            PrepareCapture {
                channel: 15,
                radio: CaptureRadio::Ieee802154 { power_dbm: 21 },
            },
            Duration::from_secs(120),
        )?;
        context.results.observe("capture-ready", &ready);
        let mut id = None;
        let measured: Result<()> = (|| {
            if ready.epoch == 0
                || !ready.full_calibration_performed
                || ready.lifecycle != CaptureLifecycle::Ieee802154
                || ready.channel != 15
                || ready.radio != (CaptureRadio::Ieee802154 { power_dbm: 21 })
            {
                return Err("INCOMPLETE: requested RX capture was not established".into());
            }
            let started = capture.request(
                0,
                StartTrackingHold {
                    epoch: ready.epoch,
                    ticks,
                },
                Duration::from_secs(5),
            )?;
            id = Some(started.id);
            context.results.observe("hold-started", &started);
            if started.id == 0
                || started.epoch != ready.epoch
                || started.ticks != ticks
                || started.channel != 15
                || started.radio != ready.radio
                || started.force != [3, 0]
                || started.period_micros != 1_000_000
            {
                return Err("hold did not establish the requested 154 idle forced policy".into());
            }
            let deadline = Instant::now() + Duration::from_secs(u64::from(ticks) * 3 + 10);
            let mut first = 0;
            let mut samples = Vec::new();
            loop {
                oer_os_process::check_cancelled()?;
                let page = capture.request(
                    0,
                    GetTrackingHold {
                        id: started.id,
                        first,
                    },
                    Duration::from_secs(5),
                )?;
                context
                    .results
                    .observe(format!("hold-page-{}-{}", first, page.available), &page);
                if page.id != started.id
                    || page.first != first
                    || page.available > ticks
                    || page.available < first
                    || page.invalid
                {
                    return Err("INCOMPLETE: invalid hold page/session".into());
                }
                if let Some(sample) = page.sample {
                    if first >= page.available {
                        return Err("INCOMPLETE: unpublished hold sample".into());
                    }
                    validate_sample(&sample, first + 1)?;
                    samples.push(sample);
                    first += 1;
                } else if page.available > first {
                    return Err("INCOMPLETE: missing published hold sample".into());
                }
                if page.terminal && first == page.available {
                    context.results.observe("hold-engine-polls", &page.engine);
                    validate_engine(&page.engine, ticks)?;
                    if first != ticks {
                        return Err("INCOMPLETE: hold terminated before all ticks".into());
                    }
                    break;
                }
                if Instant::now() >= deadline {
                    return Err("INCOMPLETE: hold sample deadline".into());
                }
                if first == page.available {
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            context.results.observe("hold-samples", &samples);
            let mut maximum = [0u64; 5];
            for sample in samples {
                for interval in sample.intervals {
                    let index = interval.operation as usize;
                    maximum[index] =
                        maximum[index].max(interval.finished_micros - interval.started_micros);
                }
            }
            context.results.observe("hold-maximum-micros", &maximum);
            Ok(())
        })();
        // Attempt both cleanup operations even when a sample or hold stop failed.
        let hold_stopped = id
            .map(|id| capture.request(0, StopTrackingHold { id }, Duration::from_secs(5)))
            .transpose();
        if let Ok(Some(stopped)) = &hold_stopped {
            context.results.observe("hold-stopped", stopped);
        }
        let closed = capture.request(
            0,
            StopCapture { epoch: ready.epoch },
            Duration::from_secs(30),
        );
        if let Ok(stopped) = &closed {
            context.results.observe("capture-stopped", stopped);
        }
        measured?;
        let stopped = hold_stopped?.ok_or("INCOMPLETE: hold did not start")?;
        if !stopped.complete
            || !stopped.debug_restored
            || stopped.samples != ticks
            || Some(stopped.id) != id
        {
            return Err("INCOMPLETE: hold cleanup/policy restoration".into());
        }
        let closed = closed?;
        if closed.epoch != ready.epoch || !closed.rf_closed {
            return Err("INCOMPLETE: final RF closure".into());
        }
        context.results.claim(
            "forced 154 idle tracking intervals captured",
            &["RF exclusion", "Wi-Fi hold", "guaranteed maximum"],
        );
        Ok(())
    })
}

fn validate_engine(engine: &HoldPolls, ticks: u8) -> Result<()> {
    if engine.invalid
        || engine.polls <= engine.pending
        || engine.pending == 0
        || engine.resumes == 0
        || engine.ready_edges < u16::from(ticks)
        || engine.total_micros < engine.maximum_micros
        || engine.lateness_micros < engine.maximum_lateness_micros
    {
        return Err("INCOMPLETE: missing or invalid engine poll observations".into());
    }
    if engine.maximum_micros > 50 {
        return Err("P1 engine poll exceeded 50 microseconds".into());
    }
    Ok(())
}

fn validate_sample(sample: &HoldSample, tick: u8) -> Result<()> {
    if sample.invalid || sample.tick != tick || sample.committed != 7 {
        return Err("INCOMPLETE: missing committed forced branch".into());
    }
    let roots: Vec<_> = sample
        .intervals
        .iter()
        .filter(|i| i.operation == HoldOperation::Tracking)
        .collect();
    if roots.len() != 1 {
        return Err("INCOMPLETE: missing/duplicate tracking boundary".into());
    }
    let root = roots[0];
    let mut counts = [0u8; 5];
    for interval in &sample.intervals {
        counts[interval.operation as usize] += 1;
        if interval.failed
            || interval.finished_micros < interval.started_micros
            || interval.started_micros < root.started_micros
            || interval.finished_micros > root.finished_micros
        {
            return Err("INCOMPLETE: invalid hold boundary".into());
        }
    }
    if counts[..4] != [1, 1, 1, 1] {
        return Err("INCOMPLETE: missing/duplicate forced hold bracket".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    pub(super) fn sample() -> HoldSample {
        HoldSample {
            tick: 1,
            committed: 7,
            invalid: false,
            intervals: heapless::Vec::from_slice(&[
                HoldInterval {
                    operation: HoldOperation::Rfpll,
                    started_micros: 1,
                    finished_micros: 2,
                    failed: false,
                },
                HoldInterval {
                    operation: HoldOperation::Common,
                    started_micros: 2,
                    finished_micros: 5,
                    failed: false,
                },
                HoldInterval {
                    operation: HoldOperation::Transmit,
                    started_micros: 5,
                    finished_micros: 8,
                    failed: false,
                },
                HoldInterval {
                    operation: HoldOperation::Tracking,
                    started_micros: 0,
                    finished_micros: 9,
                    failed: false,
                },
            ])
            .unwrap(),
        }
    }
    #[test]
    fn incomplete_failed_stale_and_unbounded_samples_do_not_become_maxima() {
        assert!(validate_sample(&sample(), 1).is_ok());
        assert!(validate_sample(&sample(), 2).is_err());
        let mut s = sample();
        s.committed = 3;
        assert!(validate_sample(&s, 1).is_err());
        let mut s = sample();
        s.intervals.pop();
        assert!(validate_sample(&s, 1).is_err());
        let mut s = sample();
        s.intervals[0].failed = true;
        assert!(validate_sample(&s, 1).is_err());
        let mut s = sample();
        s.intervals[0].finished_micros = 10;
        assert!(validate_sample(&s, 1).is_err());
        let mut s = sample();
        s.invalid = true;
        assert!(validate_sample(&s, 1).is_err());
    }
}
