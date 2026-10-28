//! Manually selected idle hold; associated Wi-Fi traffic is a separate workload.
use crate::Result;
use oer_esp32s31_phy_probe::*;
use oer_hil_scenario::workload::context::Context;
use std::{
    path::Path,
    time::{Duration, Instant},
};

fn configuration(
    lifecycle: crate::calibration_capture::boots::Lifecycle,
) -> Result<(u8, CaptureRadio, CaptureLifecycle)> {
    use crate::calibration_capture::boots::Lifecycle;
    match lifecycle {
        Lifecycle::Cold => Ok((1, CaptureRadio::Wifi, CaptureLifecycle::Cold)),
        Lifecycle::Ieee802154 => Ok((
            15,
            CaptureRadio::Ieee802154 { power_dbm: 21 },
            CaptureLifecycle::Ieee802154,
        )),
        _ => Err("hold needs a cold receiving radio".into()),
    }
}

pub(crate) fn run(
    ticks: u8,
    policy: HoldPolicy,
    batches: u8,
    lifecycle: crate::calibration_capture::boots::Lifecycle,
    output: &Path,
    context: &Context<'_>,
) -> Result<()> {
    let (channel, radio, point) = configuration(lifecycle)?;
    context.with_capture(&output.join("hold"), |capture| {
        capture.discover(Duration::from_secs(15))?;
        let ready = super::full(context, capture, PrepareCapture { channel, radio })?;
        context.results.observe("capture-ready", &ready);
        let mut id = None;
        let mut initial_policy = None;
        let measured: Result<()> = (|| {
            if ready.epoch == 0
                || !ready.full_calibration_performed
                || ready.lifecycle != point
                || ready.channel != channel
                || ready.radio != radio
            {
                return Err("INCOMPLETE: requested RX capture was not established".into());
            }
            for batch in 0..batches {
                let started = capture.request(
                    0,
                    StartTrackingHold {
                        epoch: ready.epoch,
                        ticks,
                        policy,
                    },
                    Duration::from_secs(5),
                )?;
                id = Some(started.id);
                context
                    .results
                    .observe(format!("hold-{batch}-started"), &started);
                if started.id == 0
                    || started.epoch != ready.epoch
                    || started.ticks != ticks
                    || started.channel != channel
                    || started.radio != ready.radio
                    || started.policy != policy
                    || (policy == HoldPolicy::Forced && started.force != [3, 0])
                    || initial_policy.is_some_and(|previous| previous != started.force)
                    || started.period_micros != 1_000_000
                {
                    return Err("hold did not establish the requested idle policy".into());
                }
                initial_policy = Some(started.force);
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
                    context.results.observe(
                        format!("hold-{batch}-page-{}-{}", first, page.available),
                        &page,
                    );
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
                        validate_sample(&sample, first + 1, policy)?;
                        samples.push(sample);
                        first += 1;
                    } else if page.available > first {
                        return Err("INCOMPLETE: missing published hold sample".into());
                    }
                    if page.terminal && first == page.available {
                        context
                            .results
                            .observe(format!("hold-{batch}-engine-polls"), &page.engine);
                        validate_engine(&page.engine, ticks, policy)?;
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
                context
                    .results
                    .observe(format!("hold-{batch}-samples"), &samples);
                let mut maximum = [0u64; 5];
                for sample in samples {
                    for interval in sample.intervals {
                        let index = interval.operation as usize;
                        maximum[index] =
                            maximum[index].max(interval.finished_micros - interval.started_micros);
                    }
                }
                context
                    .results
                    .observe(format!("hold-{batch}-maximum-micros"), &maximum);
                let stopped = capture.request(
                    0,
                    StopTrackingHold { id: started.id },
                    Duration::from_secs(5),
                )?;
                context
                    .results
                    .observe(format!("hold-{batch}-stopped"), &stopped);
                validate_stop(&stopped, started.id, ticks)?;
                id = None;
            }
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
        hold_stopped?;
        let closed = closed?;
        if closed.epoch != ready.epoch || !closed.rf_closed {
            return Err("INCOMPLETE: final RF closure".into());
        }
        context.results.claim(
            "quiet idle tracking intervals captured",
            &[
                "RF exclusion",
                "associated Wi-Fi traffic",
                "guaranteed maximum",
            ],
        );
        Ok(())
    })
}

fn validate_engine(engine: &HoldPolls, ticks: u8, policy: HoldPolicy) -> Result<()> {
    if engine.invalid
        || engine.polls <= engine.pending
        || engine.pending == 0
        || engine.resumes == 0
        || (policy == HoldPolicy::Forced && engine.ready_edges < u16::from(ticks))
        || engine.total_micros < engine.maximum_micros
        || engine.lateness_micros < engine.maximum_lateness_micros
    {
        return Err("INCOMPLETE: missing or invalid engine poll observations".into());
    }
    if policy == HoldPolicy::Forced && engine.maximum_micros > oer_cooperate::MAX_POLL_MICROS {
        return Err("engine poll exceeded the 1 ms continuous-poll guard".into());
    }
    Ok(())
}

fn validate_sample(sample: &HoldSample, tick: u8, policy: HoldPolicy) -> Result<()> {
    if sample.invalid
        || sample.tick != tick
        || (sample.committed & !7 != 0 || (policy == HoldPolicy::Forced && sample.committed != 7))
    {
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
    let expected = [
        1,
        sample.committed & 1,
        (sample.committed >> 1) & 1,
        (sample.committed >> 2) & 1,
    ];
    if counts[..4] != expected {
        return Err("INCOMPLETE: missing/duplicate forced hold bracket".into());
    }
    Ok(())
}

fn validate_stop(stopped: &HoldStopped, id: u32, ticks: u8) -> Result<()> {
    if stopped.id != id || stopped.samples != ticks || !stopped.complete || !stopped.debug_restored
    {
        return Err("INCOMPLETE: hold cleanup/policy restoration".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn idle_radio_selection_retains_its_point_and_rejects_restart_aliases() {
        use crate::calibration_capture::boots::Lifecycle;
        assert_eq!(
            configuration(Lifecycle::Cold).unwrap(),
            (1, CaptureRadio::Wifi, CaptureLifecycle::Cold)
        );
        assert_eq!(
            configuration(Lifecycle::Ieee802154).unwrap(),
            (
                15,
                CaptureRadio::Ieee802154 { power_dbm: 21 },
                CaptureLifecycle::Ieee802154
            )
        );
        assert!(configuration(Lifecycle::Restart).is_err());
        assert!(configuration(Lifecycle::Ieee802154Restart).is_err());
    }
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
        assert!(validate_sample(&sample(), 1, HoldPolicy::Forced).is_ok());
        assert!(validate_sample(&sample(), 2, HoldPolicy::Forced).is_err());
        let mut s = sample();
        s.committed = 3;
        assert!(validate_sample(&s, 1, HoldPolicy::Forced).is_err());
        let mut s = sample();
        s.intervals.pop();
        assert!(validate_sample(&s, 1, HoldPolicy::Forced).is_err());
        let mut s = sample();
        s.intervals[0].failed = true;
        assert!(validate_sample(&s, 1, HoldPolicy::Forced).is_err());
        let mut s = sample();
        s.intervals[0].finished_micros = 10;
        assert!(validate_sample(&s, 1, HoldPolicy::Forced).is_err());
        let mut s = sample();
        s.invalid = true;
        assert!(validate_sample(&s, 1, HoldPolicy::Forced).is_err());
    }
}

#[cfg(test)]
mod natural_tests {
    use super::*;
    #[test]
    fn continuous_poll_guard_keeps_ready_peer_response_as_a_separate_proof() {
        let timing = HoldPolls {
            polls: 2,
            pending: 1,
            resumes: 1,
            ready_edges: 1,
            total_micros: 1_001,
            maximum_micros: 1_001,
            ..Default::default()
        };
        assert!(validate_engine(&timing, 1, HoldPolicy::Natural).is_ok());
        for micros in [50, 60, 1_000] {
            assert!(
                validate_engine(
                    &HoldPolls {
                        total_micros: micros,
                        maximum_micros: micros,
                        ..timing
                    },
                    1,
                    HoldPolicy::Forced
                )
                .is_ok()
            );
        }
        assert!(validate_engine(&timing, 1, HoldPolicy::Forced).is_err());
        assert!(
            validate_engine(
                &HoldPolls {
                    invalid: true,
                    ..timing
                },
                1,
                HoldPolicy::Natural
            )
            .is_err()
        );
    }
    #[test]
    fn natural_samples_accept_only_the_observed_branch_subset() {
        for committed in 0..=7 {
            let mut sample = super::tests::sample();
            sample.committed = committed;
            sample
                .intervals
                .retain(|interval| match interval.operation {
                    HoldOperation::Tracking => true,
                    HoldOperation::Rfpll => committed & 1 != 0,
                    HoldOperation::Common => committed & 2 != 0,
                    HoldOperation::Transmit => committed & 4 != 0,
                    HoldOperation::WifiBasebandOff => true,
                });
            assert!(validate_sample(&sample, 1, HoldPolicy::Natural).is_ok());
            assert_eq!(
                validate_sample(&sample, 1, HoldPolicy::Forced).is_ok(),
                committed == 7
            );
            if committed != 7 {
                sample.committed = 7;
                assert!(validate_sample(&sample, 1, HoldPolicy::Natural).is_err());
            }
        }
        let mut sample = super::tests::sample();
        sample.committed = 8;
        assert!(validate_sample(&sample, 1, HoldPolicy::Natural).is_err());
        sample.committed = 0;
        assert!(validate_sample(&sample, 1, HoldPolicy::Natural).is_err());
    }
    #[test]
    fn every_batch_requires_completion_and_policy_restoration() {
        let stopped = HoldStopped {
            id: 2,
            samples: 16,
            complete: true,
            debug_restored: true,
        };
        assert!(validate_stop(&stopped, 2, 16).is_ok());
        assert!(validate_stop(&stopped, 1, 16).is_err());
        assert!(validate_stop(&stopped, 2, 15).is_err());
        assert!(
            validate_stop(
                &HoldStopped {
                    complete: false,
                    ..stopped
                },
                2,
                16
            )
            .is_err()
        );
        assert!(
            validate_stop(
                &HoldStopped {
                    debug_restored: false,
                    ..stopped
                },
                2,
                16
            )
            .is_err()
        );
    }
}
