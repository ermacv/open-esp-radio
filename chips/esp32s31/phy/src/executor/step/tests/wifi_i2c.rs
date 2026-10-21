use super::*;
use crate::tracking::i2c::{
    PhyWifiI2cTrackingAction as Action, PhyWifiI2cTrackingBand as Band,
    PhyWifiI2cTrackingParameters, PhyWifiI2cTrackingTransition,
};

fn changed() -> PhyWifiI2cTrackingTransition {
    PhyWifiI2cTrackingTransition::new(PhyWifiI2cTrackingParameters {
        current_temperature: 100,
        previous_band: Band::Nominal,
    })
}

#[test]
fn both_masked_writes_keep_order_and_unrelated_bits_across_real_pending_edges() {
    let clock = VirtualClock::<1>::new();
    let mut io = Io {
        byte: 0xa5,
        ..Io::default()
    };
    let atoms = io.atoms.clone();
    let mut transition = changed();
    let mut timing = EngineTiming::default();
    let (waker, _) = context();
    let mut cx = Context::from_waker(&waker);
    let outcome = {
        let mut future = core::pin::pin!(super::super::wifi_i2c::track::<TestDelay, _, _>(
            &mut transition,
            &mut io,
            &clock,
            Some(&mut timing)
        ));
        let mut polls = 0;
        loop {
            polls += 1;
            assert!(polls < 100);
            let before = atoms.get();
            let result = future.as_mut().poll(&mut cx);
            assert!(atoms.get() - before <= 1, "one bus edge per caller poll");
            if let Poll::Ready(result) = result {
                break result.expect("tracking completes");
            }
            clock
                .advance(Duration::from_micros(1))
                .expect("virtual time fits");
        }
    };
    assert_eq!(outcome.band, Band::Hot);
    assert!(outcome.changed);
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
    let first = analog_registers::WIFI_TX_TEMPERATURE_TRACKING_0;
    let second = analog_registers::WIFI_TX_TEMPERATURE_TRACKING_1;
    assert_eq!(
        commands,
        std::vec![
            PhyColdI2cAction::StartRead {
                address: first.address()
            },
            PhyColdI2cAction::StartWrite {
                address: first.address(),
                value: first.replace(0xa5, 6)
            },
            PhyColdI2cAction::StartRead {
                address: second.address()
            },
            PhyColdI2cAction::StartWrite {
                address: second.address(),
                value: second.replace(first.replace(0xa5, 6), 15)
            },
        ]
    );
    assert_eq!(transition.action(), Action::Complete(outcome));
}

#[test]
fn unchanged_band_finishes_without_hardware_or_a_yield() {
    let clock = VirtualClock::<1>::new();
    let mut io = Io::default();
    let mut transition = PhyWifiI2cTrackingTransition::new(PhyWifiI2cTrackingParameters {
        current_temperature: 25,
        previous_band: Band::Nominal,
    });
    let (waker, _) = context();
    let mut cx = Context::from_waker(&waker);
    {
        let mut future = core::pin::pin!(super::super::wifi_i2c::track::<TestDelay, _, _>(
            &mut transition,
            &mut io,
            &clock,
            None
        ));
        let Poll::Ready(Ok(outcome)) = future.as_mut().poll(&mut cx) else {
            panic!("unchanged band is immediately complete")
        };
        assert!(!outcome.changed);
    }
    assert!(io.actions.is_empty());
}

#[test]
fn busy_readiness_exhaustion_cannot_complete_either_band_write() {
    let clock = VirtualClock::<1>::new();
    let mut io = Io::default();
    io.busy.set(true);
    let mut transition = changed();
    let (waker, _) = context();
    let mut cx = Context::from_waker(&waker);
    {
        let mut future = core::pin::pin!(super::super::wifi_i2c::track::<TestDelay, _, _>(
            &mut transition,
            &mut io,
            &clock,
            None
        ));
        let mut polls = 0;
        loop {
            polls += 1;
            assert!(polls < usize::from(crate::HARDWARE_EDGE_LIMIT) * 4);
            if let Poll::Ready(result) = future.as_mut().poll(&mut cx) {
                assert_eq!(result, Err(Failure::TimedOut));
                break;
            }
            clock
                .advance(Duration::from_micros(1))
                .expect("virtual time fits");
        }
    }
    assert!(matches!(
        transition.action(),
        Action::MaskedWrite(oer_espressif_analog::Action::Read { .. })
    ));
    assert!(
        !io.actions
            .iter()
            .any(|a| matches!(a, PhyColdI2cAction::StartWrite { .. }))
    );
}
