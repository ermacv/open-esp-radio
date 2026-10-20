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

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use oer_hil_link::{
        Received,
        test_support::{self, Input},
    };
    use oer_hil_protocol::{Envelope, FrameDecoder, Message, WireKind, base::*};
    use std::sync::{Arc, Mutex, mpsc};

    fn reply<M: Message>(input: &Input, request: &Received, sequence: &mut u32, body: M) {
        input
            .send(Ok(test_support::frame(Envelope::new(
                7,
                *sequence,
                request.session_id,
                request.request_id,
                body,
            ))))
            .unwrap();
        *sequence += 1;
    }
    fn target(
        input: Input,
        commands: mpsc::Receiver<Vec<u8>>,
        bad_page: bool,
        bad_stop: bool,
    ) -> Vec<&'static str> {
        let keys = oer_hil_protocol::feature_keys::image_keys(&|name| name == "diagnostic-phy");
        let keys = ImageKeySet::new(&keys);
        input
            .send(Ok(test_support::frame(Envelope::new(
                7,
                0,
                0,
                0,
                keys.hello(0),
            ))))
            .unwrap();
        let mut sequence = 1;
        let mut calls = Vec::new();
        while let Ok(bytes) = commands.recv() {
            let mut decoded = None;
            FrameDecoder::new().feed(WireKind::Command, &bytes, |frame| {
                decoded = Some(Received::from_frame(&frame.unwrap()).unwrap());
            });
            let request = decoded.unwrap();
            if let Some(get) = request.decode::<GetImageKeys>() {
                reply(&input, &request, &mut sequence, keys.page(get.first));
            } else if request.is::<GetHello>() {
                reply(&input, &request, &mut sequence, keys.hello(0));
            } else if let Some(config) = request.decode::<PrepareCapture>() {
                calls.push("prepare");
                reply(
                    &input,
                    &request,
                    &mut sequence,
                    CaptureReady {
                        epoch: 1,
                        lifecycle: CaptureLifecycle::Ieee802154,
                        full_calibration_performed: true,
                        path: CalibrationPath::FullForCache,
                        projection_schema: 1,
                        calibration_words: 92,
                        registers: 1,
                        analog_bytes: 1,
                        channel: config.channel,
                        radio: config.radio,
                    },
                );
            } else if let Some(start) = request.decode::<StartTrackingHold>() {
                calls.push("start");
                reply(
                    &input,
                    &request,
                    &mut sequence,
                    HoldStarted {
                        id: 1,
                        epoch: start.epoch,
                        ticks: start.ticks,
                        channel: 15,
                        radio: CaptureRadio::Ieee802154 { power_dbm: 21 },
                        force: [3, 0],
                        period_micros: 1_000_000,
                    },
                );
            } else if let Some(get) = request.decode::<GetTrackingHold>() {
                calls.push("get");
                let mut sample = super::tests::sample();
                sample.tick = get.first + 1;
                reply(
                    &input,
                    &request,
                    &mut sequence,
                    TrackingHold {
                        id: if bad_page { 2 } else { get.id },
                        first: get.first,
                        available: 2,
                        terminal: true,
                        invalid: false,
                        sample: Some(sample),
                    },
                );
            } else if let Some(stop) = request.decode::<StopTrackingHold>() {
                calls.push("hold-stop");
                if bad_stop {
                    reply(
                        &input,
                        &request,
                        &mut sequence,
                        Rejected(RejectReason::Internal),
                    );
                } else {
                    reply(
                        &input,
                        &request,
                        &mut sequence,
                        HoldStopped {
                            id: stop.id,
                            samples: 2,
                            complete: true,
                            debug_restored: true,
                        },
                    );
                }
            } else if request.is::<GetLinkHealth>() {
                reply(
                    &input,
                    &request,
                    &mut sequence,
                    LinkHealth {
                        rx_frames: 1,
                        rx_cobs_errors: 0,
                        rx_checksum_errors: 0,
                        rx_decode_errors: 0,
                        rx_overflows: 0,
                        tx_frames: 1,
                        tx_dropped: 0,
                        text_dropped: 0,
                        text_truncated: 0,
                    },
                );
            } else if let Some(stop) = request.decode::<StopCapture>() {
                calls.push("capture-stop");
                reply(
                    &input,
                    &request,
                    &mut sequence,
                    CaptureStopped {
                        epoch: stop.epoch,
                        rf_closed: true,
                    },
                );
            }
        }
        calls
    }
    #[test]
    fn stale_pages_and_failed_hold_cleanup_still_attempt_rf_closure() {
        for (bad_page, bad_stop) in [(false, false), (true, false), (false, true)] {
            let output = tempfile::tempdir().unwrap();
            let lab = oer_hil_lab::config::LabConfig::for_test();
            let target_thread = Arc::new(Mutex::new(None));
            let store = Arc::clone(&target_thread);
            let context = Context::new(&lab, output.path()).with_opener(move |path| {
                let (capture, input, commands) = test_support::capture_with_commands_at(path);
                *store.lock().unwrap() = Some(std::thread::spawn(move || {
                    target(input, commands, bad_page, bad_stop)
                }));
                Ok(capture)
            });
            let result = run(2, output.path(), &context);
            let calls = target_thread
                .lock()
                .unwrap()
                .take()
                .unwrap()
                .join()
                .unwrap();
            assert_eq!(
                result.is_ok(),
                !bad_page && !bad_stop,
                "{result:?}; {calls:?}"
            );
            assert!(calls.ends_with(&["hold-stop", "capture-stop"]));
            assert_eq!(
                context.results.snapshot().claim.is_some(),
                !bad_page && !bad_stop
            );
        }
    }
}
