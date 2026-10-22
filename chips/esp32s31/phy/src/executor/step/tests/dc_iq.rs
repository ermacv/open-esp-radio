use super::*;
use crate::calibration::estimator::{
    PhyDcIqAction, PhyDcIqCompletion, PhyDcIqEstimateRequest, PhyDcIqReadinessBinding,
};

fn request() -> PhyDcIqEstimateRequest {
    PhyDcIqEstimateRequest {
        iteration: 0,
        chain: 1,
        control: 0x800,
        mode: 1,
    }
}
fn binding(samples: u16) -> PhyDcIqReadinessBinding {
    PhyDcIqReadinessBinding::new(PhyDcIqAction::AwaitReadinessEdge {
        request: request(),
        readiness_samples: samples,
        readiness_activity_edges: 0,
    })
    .expect("readiness action")
}

#[test]
fn ready_capture_and_disable_tail_remain_in_one_poll() {
    let clock = VirtualClock::<1>::new();
    let mut io = Io::default();
    let atoms = io.atoms.clone();
    let mut binding = Some(rx_binding(binding(0)));
    let mut completion = None;
    let mut job =
        Job::rx_gain_atom(&mut binding, &mut completion, clock.now()).expect("clock fits");
    let (waker, _) = context();
    let mut cx = Context::from_waker(&waker);
    {
        let mut future = core::pin::pin!(drive::<TestDelay, _, _>(
            &mut job, &mut io, &clock, None, None, None
        ));
        assert!(future.as_mut().poll(&mut cx).is_pending());
        assert_eq!(atoms.get(), 1);
        assert_eq!(future.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
        assert_eq!(atoms.get(), 1);
    }
    assert_eq!(
        io.dc_iq_events,
        [
            "ready",
            "accumulators",
            "measurement-off",
            "settle",
            "start-off"
        ]
    );
    assert!(matches!(
        completion,
        Some(crate::rx::gain::PhyRxGainInitCompletion::Dc(
            crate::rx::gain_calibration::PhyRxGainDcCompletion::Calibration(
                crate::rx::gain_calibration::PhyRxDcCalibrationCompletion::Minimum(
                    crate::rx::dc_offset::PhyRxDcMinimumCompletion::DcIq(
                        PhyDcIqCompletion::CaptureFinished {
                            accumulators: Some(_),
                            ..
                        }
                    )
                )
            )
        ))
    ));
}

#[test]
fn false_ready_waits_without_reading_accumulators_or_disabling_measurement() {
    let clock = VirtualClock::<1>::new();
    let mut io = Io {
        dc_iq_pending: 1,
        ..Io::default()
    };
    let atoms = io.atoms.clone();
    let mut binding = Some(rx_binding(binding(0)));
    let mut completion = None;
    let mut job =
        Job::rx_gain_atom(&mut binding, &mut completion, clock.now()).expect("clock fits");
    let (waker, _) = context();
    let mut cx = Context::from_waker(&waker);
    {
        let mut future = core::pin::pin!(drive::<TestDelay, _, _>(
            &mut job, &mut io, &clock, None, None, None
        ));
        assert!(future.as_mut().poll(&mut cx).is_pending());
        assert!(future.as_mut().poll(&mut cx).is_pending());
        assert_eq!(atoms.get(), 1, "no second observation before the timer");
        clock.advance(Duration::from_micros(1)).expect("time fits");
        assert_eq!(future.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
    }
    assert_eq!(io.dc_iq_events, ["ready"]);
    let Some(crate::rx::gain::PhyRxGainInitCompletion::Dc(
        crate::rx::gain_calibration::PhyRxGainDcCompletion::Calibration(
            crate::rx::gain_calibration::PhyRxDcCalibrationCompletion::Minimum(
                crate::rx::dc_offset::PhyRxDcMinimumCompletion::DcIq(completion),
            ),
        ),
    )) = completion
    else {
        panic!("same estimator must complete");
    };
    assert!(matches!(
        completion,
        PhyDcIqCompletion::ReadinessObserved {
            snapshot: crate::calibration::estimator::PhyDcIqReadinessSnapshot {
                ready: false,
                activity: true
            },
            ..
        }
    ));
}

#[test]
fn observation_limit_cleans_up_without_sampling_ready_or_accumulators() {
    let mut io = Io::default();
    let result = io
        .dc_iq::<TestDelay>(binding(crate::HARDWARE_EDGE_LIMIT))
        .expect("tail settles");
    assert_eq!(io.dc_iq_events, ["measurement-off", "settle", "start-off"]);
    assert_eq!(
        result,
        PhyDcIqCompletion::CaptureFinished {
            request: request(),
            accumulators: None
        }
    );
}

fn rx_binding(binding: PhyDcIqReadinessBinding) -> crate::rx::gain::PhyRxGainInitExternalBinding {
    crate::rx::gain::PhyRxGainInitExternalBinding::Dc(
        crate::rx::gain_calibration::PhyRxGainDcExternalBinding::Calibration(
            crate::rx::gain_calibration::PhyRxDcCalibrationExternalBinding::Minimum(
                crate::rx::dc_offset::PhyRxDcMinimumExternalBinding::DcIq(
                    crate::calibration::estimator::PhyDcIqExternalBinding::Readiness(binding),
                ),
            ),
        ),
    )
}
