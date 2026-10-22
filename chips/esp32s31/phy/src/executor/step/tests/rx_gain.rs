use super::*;
use crate::rx::{gain::*, gain_calibration::PhyRxGainDcParameters};

fn publisher() -> PhyRxGainInitTransition {
    PhyRxGainInitTransition::new(PhyRxGainInitParameters {
        dc_calibrated: true,
        tables_initialized: false,
        dc: PhyRxGainDcParameters {
            crystal_selector: 0,
            pbus_rx_path_value: 0xbf,
            rx_saturation_detected: false,
        },
        memory: crate::calibration::baseband::PhyRxGainMemoryParameters {
            parameter_002: 0xbf,
            wifi_index_dc: [[0x101, 0x102]; 8],
            wifi_fine_dc: [[0x103, 0x104]; 5],
            shared_index_dc: [[0x105, 0x106]; 11],
            wifi_auxiliary: 0,
        },
    })
}

#[test]
fn complete_entries_cross_real_poll_boundaries_before_the_root_tail() {
    for measured in [false, true] {
        complete_publisher(measured);
    }
}

fn complete_publisher(measured: bool) {
    let clock = VirtualClock::<1>::new();
    let mut io = Io::default();
    let mut transition = publisher();
    let mut scratch = crate::executor::step::rx_gain::Scratch::default();
    let mut execution = crate::tracking::observation::RxGainExecution::default();
    let mut timing = EngineTiming::default();
    let (waker, _) = context();
    let mut cx = Context::from_waker(&waker);
    while transition.phase().is_some() {
        let mut job = Job::rx_gain(
            &mut transition,
            &mut scratch,
            &mut execution,
            clock.now(),
            measured,
        )
        .expect("clock fits");
        let atoms = io.atoms.clone();
        let mut future = core::pin::pin!(drive_calibration::<TestDelay, _, _>(
            &mut job,
            &mut io,
            &clock,
            measured.then_some(&mut timing)
        ));
        loop {
            let before = atoms.get();
            let result = future.as_mut().poll(&mut cx);
            assert!(
                atoms.get() <= before + 1,
                "at most one admitted atom per poll"
            );
            if let Poll::Ready(result) = result {
                result.expect("publisher succeeds");
                break;
            }
            clock.advance(Duration::from_micros(1)).expect("clock fits");
        }
    }
    let Some(Ok(outcome)) = transition.terminal() else {
        panic!("both banks and tail must complete");
    };
    assert!(outcome.generated_tables);
    if measured {
        assert!(timing.polls > 0);
        assert!(timing.ready_edges > 0);
    }
    assert_eq!(
        outcome.dc, None,
        "the pre-calibrated path does not measure again"
    );
    assert_eq!(
        io.power_gain_entries.len(),
        usize::from(outcome.wifi_last_index) + usize::from(outcome.shared_last_index) + 2
    );
}

#[test]
fn dropped_publication_does_not_finish_or_run_another_entry() {
    let clock = VirtualClock::<1>::new();
    let mut io = Io::default();
    let mut transition = publisher();
    let mut scratch = crate::executor::step::rx_gain::Scratch::default();
    let mut execution = crate::tracking::observation::RxGainExecution::default();
    let (waker, _) = context();
    let mut cx = Context::from_waker(&waker);
    {
        let mut job = Job::rx_gain(
            &mut transition,
            &mut scratch,
            &mut execution,
            clock.now(),
            false,
        )
        .expect("clock fits");
        let mut future = core::pin::pin!(drive::<TestDelay, _, _>(
            &mut job, &mut io, &clock, None, None, None
        ));
        assert!(future.as_mut().poll(&mut cx).is_pending());
    }
    assert!(transition.terminal().is_none());
    assert!(io.power_gain_entries.is_empty());
    assert_eq!(io.atoms.get(), 1);
}
