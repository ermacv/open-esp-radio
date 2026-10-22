use super::*;
use crate::{
    state::{PhyConfig, PhyState},
    tracking::{
        i2c::PhyWifiI2cTrackingBand as Band,
        parameters::{
            PhyParamTrackRequest, PhyParamTrackingCompletion, PhyParamTrackingPolicy,
            PhyParamTrackingTransition, PhyParamTrackingWifiI2cTransition,
        },
    },
};

fn state(temperature: i16) -> PhyState {
    let mut state = PhyState::new(PhyConfig::production());
    state.apply_temperature_outcome(PhyTemperatureOutcome {
        temperature,
        sensor_index: 3,
        next_dac: 4,
    });
    state
}

fn child(state: &mut PhyState) -> PhyParamTrackingWifiI2cTransition<'_> {
    let mut policy = PhyParamTrackingPolicy::for_registered_state(state);
    policy.rfpll_cap_tracking_enabled = false;
    let mut parent =
        PhyParamTrackingTransition::new(PhyParamTrackRequest::new(true, false), policy);
    parent
        .advance(PhyParamTrackingCompletion::EnteredCritical)
        .expect("enter the tracking owner");
    parent.begin_wifi_i2c_tracking(state).expect("T3 selected")
}

#[test]
fn both_masked_writes_keep_order_and_unrelated_bits_across_real_pending_edges() {
    let clock = VirtualClock::<1>::new();
    let mut io = Io {
        byte: 0xa5,
        ..Io::default()
    };
    let atoms = io.atoms.clone();
    let mut state = state(100);
    let (waker, _) = context();
    let mut cx = Context::from_waker(&waker);
    let (result, timing) = {
        let mut future = core::pin::pin!(super::super::wifi_i2c::run_param_wifi_i2c::<
            TestDelay,
            _,
            _,
        >(&clock, child(&mut state), &mut io, true));
        let mut polls = 0;
        loop {
            polls += 1;
            assert!(polls < 100);
            let before = atoms.get();
            let result = future.as_mut().poll(&mut cx);
            assert!(atoms.get() - before <= 1, "one bus edge per caller poll");
            if let Poll::Ready(result) = result {
                break result;
            }
            clock
                .advance(Duration::from_micros(1))
                .expect("virtual time fits");
        }
    };
    assert_eq!(result, Ok(PhyParamTrackingCompletion::WifiI2cTracked));
    assert_eq!(
        state.wifi_i2c_tracking_parameters().previous_band,
        Band::Hot
    );
    assert!(timing.pending > 0 && timing.polls > timing.pending && timing.resumes > 0);
    let commands: Vec<_> = io
        .actions
        .iter()
        .filter(|a| {
            matches!(
                a,
                PhyColdI2cAction::StartRead { .. } | PhyColdI2cAction::StartWrite { .. }
            )
        })
        .copied()
        .collect();
    let first = fields::TXRF_WIFI_TX_TEMPERATURE_TRACKING_0;
    let second = fields::TXRF_WIFI_TX_TEMPERATURE_TRACKING_1;
    assert_eq!(
        commands,
        std::vec![
            PhyColdI2cAction::StartRead {
                address: first.address()
            },
            PhyColdI2cAction::StartWrite {
                address: first.address(),
                value: first.insert(0xa5, 6).unwrap()
            },
            PhyColdI2cAction::StartRead {
                address: second.address()
            },
            PhyColdI2cAction::StartWrite {
                address: second.address(),
                value: second.insert(first.insert(0xa5, 6).unwrap(), 15).unwrap()
            },
        ]
    );
}

#[test]
fn unchanged_band_finishes_without_hardware_or_a_yield() {
    let clock = VirtualClock::<1>::new();
    let mut io = Io::default();
    let mut state = state(25);
    let (waker, _) = context();
    let mut cx = Context::from_waker(&waker);
    {
        let mut future = core::pin::pin!(super::super::wifi_i2c::run_param_wifi_i2c::<
            TestDelay,
            _,
            _,
        >(&clock, child(&mut state), &mut io, false));
        let Poll::Ready((Ok(PhyParamTrackingCompletion::WifiI2cTracked), timing)) =
            future.as_mut().poll(&mut cx)
        else {
            panic!("unchanged band is immediately complete")
        };
        assert_eq!(timing.polls, 0);
    }
    assert!(io.actions.is_empty());
}

#[test]
fn busy_readiness_exhaustion_cannot_complete_either_band_write() {
    let clock = VirtualClock::<1>::new();
    let mut io = Io::default();
    io.busy.set(true);
    let mut state = state(100);
    let (waker, _) = context();
    let mut cx = Context::from_waker(&waker);
    {
        let mut future = core::pin::pin!(super::super::wifi_i2c::run_param_wifi_i2c::<
            TestDelay,
            _,
            _,
        >(&clock, child(&mut state), &mut io, false));
        let mut polls = 0;
        loop {
            polls += 1;
            assert!(polls < usize::from(crate::HARDWARE_EDGE_LIMIT) * 4);
            if let Poll::Ready(result) = future.as_mut().poll(&mut cx) {
                assert_eq!(result.0, Err(Failure::TimedOut));
                break;
            }
            clock
                .advance(Duration::from_micros(1))
                .expect("virtual time fits");
        }
    }
    assert_eq!(
        state.wifi_i2c_tracking_parameters().previous_band,
        Band::Nominal
    );
    assert!(
        !io.actions
            .iter()
            .any(|a| matches!(a, PhyColdI2cAction::StartWrite { .. }))
    );
}
