use super::*;
use crate::analog::{
    dcode::{PhyDcodeAction, PhyDcodeExternalBinding, PhyDcodeParameters, PhyDcodeTransition},
    rfpll::RfpllFrequencyAction,
};

#[test]
fn cold_and_runtime_keep_channel_capture_order_and_checkpoint_every_transaction() {
    let mut histories = Vec::new();
    for runtime in [false, true] {
        let clock = VirtualClock::<1>::new();
        let mut io = Io::default();
        let atoms = io.atoms.clone();
        let (waker, _) = context();
        let mut cx = Context::from_waker(&waker);
        let mut transition = PhyDcodeTransition::new(PhyDcodeParameters {
            crystal_selector: 0,
        });
        let mut timing = EngineTiming::default();
        let mut captures = Vec::new();
        for _ in 0..100 {
            let action = transition.action();
            if let PhyDcodeAction::Complete(outcome) = action {
                assert_eq!(outcome.codes.len(), 8);
                break;
            }
            if let PhyDcodeAction::ReadMasked { field } = action {
                captures.push(field);
            }
            let result = {
                let binding = PhyDcodeExternalBinding::lower(action).expect("nonterminal action");
                let mut future =
                    core::pin::pin!(crate::executor::step::dcode::complete::<TestDelay, _, _>(
                        binding,
                        &mut io,
                        &clock,
                        runtime,
                        Some(&mut timing),
                    ));
                let before = atoms.get();
                assert!(
                    future.as_mut().poll(&mut cx).is_pending(),
                    "every edge yields even with immediately ready hardware"
                );
                assert!(atoms.get() > before);
                let mut polls = 0;
                loop {
                    polls += 1;
                    assert!(polls < 20);
                    clock.advance(Duration::from_micros(1)).expect("time fits");
                    if let Poll::Ready(result) = future.as_mut().poll(&mut cx) {
                        break result;
                    }
                }
            };
            transition
                .advance(result.expect("hardware completed"))
                .expect("matching completion");
        }
        assert!(matches!(transition.action(), PhyDcodeAction::Complete(_)));
        let visits: Vec<_> = io
            .frequency_actions
            .iter()
            .filter_map(|action| match action {
                RfpllFrequencyAction::StartChannelSwitch {
                    frequency_index, ..
                } => Some(*frequency_index),
                _ => None,
            })
            .collect();
        // Frequency-table indices are MHz offsets from 2400, for channels 1/5/10/14.
        assert_eq!(visits, [12, 32, 57, 84]);
        assert_eq!(
            captures,
            [
                analog_registers::RFPLL_INTERNAL_DCODE_0,
                analog_registers::RFPLL_INTERNAL_DCODE_1
            ]
            .repeat(4)
        );
        assert!(!timing.invalid && timing.pending > 0);
        histories.push((io.frequency_actions, io.actions));
    }
    assert_eq!(
        histories[0], histories[1],
        "cold and runtime execute the same commands"
    );
}

#[test]
fn failure_and_drop_cannot_advance_the_dcode_model_or_publish_captures() {
    for fail in [false, true] {
        let clock = VirtualClock::<1>::new();
        let mut io = Io {
            frequency_failure: fail,
            ..Io::default()
        };
        let transition = PhyDcodeTransition::new(PhyDcodeParameters {
            crystal_selector: 0,
        });
        let before = transition;
        let (waker, _) = context();
        let mut cx = Context::from_waker(&waker);
        {
            let mut future =
                core::pin::pin!(crate::executor::step::dcode::complete::<TestDelay, _, _>(
                    PhyDcodeExternalBinding::lower(transition.action()).expect("first action"),
                    &mut io,
                    &clock,
                    true,
                    None,
                ));
            let result = future.as_mut().poll(&mut cx);
            if fail {
                assert_eq!(result, Poll::Ready(Err(Failure::Binding)));
            } else {
                assert!(result.is_pending());
            }
        }
        assert_eq!(transition, before);
        assert!(
            io.actions.is_empty(),
            "no CKGEN writes or D-code reads after failure/drop"
        );
    }
}
