use super::*;
use crate::analog::{
    dcode::{PhyDcodeAction, PhyDcodeExternalBinding, PhyDcodeParameters, PhyDcodeTransition},
    rfpll::RfpllFrequencyAction,
};

#[test]
fn cold_and_runtime_keep_channel_capture_order_and_complete_transaction_batches() {
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
        if runtime {
            let mut lock = None;
            let mut job = Job::dcode(&mut transition, &mut lock, clock.now()).expect("clock fits");
            let mut future = core::pin::pin!(drive::<TestDelay, _, _>(
                &mut job,
                &mut io,
                &clock,
                Some(&mut timing),
                None,
                None,
            ));
            let mut polls = 0;
            loop {
                polls += 1;
                assert!(polls < 200);
                let before = atoms.get();
                let result = future.as_mut().poll(&mut cx);
                assert!(
                    atoms.get() - before <= usize::from(BATCH_COMMANDS) * 4,
                    "a bounded batch of complete RMW transactions per poll"
                );
                if let Poll::Ready(result) = result {
                    result.expect("D-code completed");
                    break;
                }
                clock.advance(Duration::from_micros(1)).expect("time fits");
            }
        } else {
            for _ in 0..100 {
                let action = transition.action();
                if let PhyDcodeAction::Complete(outcome) = action {
                    assert_eq!(outcome.codes.len(), 8);
                    break;
                }
                let result = {
                    let binding =
                        PhyDcodeExternalBinding::lower(action).expect("nonterminal action");
                    let mut future =
                        core::pin::pin!(cold_complete(binding, &mut io, &clock, &mut timing));
                    let before = atoms.get();
                    let first = future.as_mut().poll(&mut cx);
                    assert!(atoms.get() > before);
                    if let Poll::Ready(result) = first {
                        result
                    } else {
                        let mut polls = 0;
                        loop {
                            polls += 1;
                            assert!(polls < 20);
                            clock.advance(Duration::from_micros(1)).expect("time fits");
                            if let Poll::Ready(result) = future.as_mut().poll(&mut cx) {
                                break result;
                            }
                        }
                    }
                };
                transition
                    .advance(result.expect("hardware completed"))
                    .expect("matching completion");
            }
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
        let captures: Vec<_> = io
            .actions
            .iter()
            .filter_map(|action| match action {
                PhyColdI2cAction::StartRead { address }
                    if *address == fields::RFPLL_INTERNAL_DCODE_0.address()
                        || *address == fields::RFPLL_INTERNAL_DCODE_1.address() =>
                {
                    Some(*address)
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            captures,
            [
                fields::RFPLL_INTERNAL_DCODE_0.address(),
                fields::RFPLL_INTERNAL_DCODE_1.address()
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
fn runtime_failure_and_drop_leave_a_partial_model_without_terminal_authority() {
    for fail in [false, true] {
        let clock = VirtualClock::<1>::new();
        let mut io = Io {
            frequency_failure: fail,
            ..Io::default()
        };
        let mut transition = PhyDcodeTransition::new(PhyDcodeParameters {
            crystal_selector: 0,
        });
        let mut lock = None;
        let (waker, _) = context();
        let mut cx = Context::from_waker(&waker);
        {
            let mut job = Job::dcode(&mut transition, &mut lock, clock.now()).expect("clock fits");
            let mut future = core::pin::pin!(drive::<TestDelay, _, _>(
                &mut job, &mut io, &clock, None, None, None
            ));
            let result = future.as_mut().poll(&mut cx);
            if fail {
                assert_eq!(result, Poll::Ready(Err(Failure::Binding)));
            } else {
                assert!(result.is_pending());
            }
        }
        assert!(!matches!(transition.action(), PhyDcodeAction::Complete(_)));
        assert!(io.actions.is_empty(), "no capture after failure/drop");
    }
}

// The preexisting cold lowering uses counted I2C and direct MMIO/settles.
async fn cold_complete(
    binding: PhyDcodeExternalBinding,
    io: &mut Io,
    clock: &VirtualClock<1>,
    timing: &mut EngineTiming,
) -> Result<crate::analog::dcode::PhyDcodeCompletion, Failure> {
    use crate::analog::{
        dcode::PhyDcodeCompletion as Completion, rfpll::RfpllFrequencyExternalBinding as Frequency,
    };
    match binding {
        PhyDcodeExternalBinding::I2c(mut binding) => {
            let mut job = Job::counted_i2c(binding.transaction_mut(), clock.now())?;
            drive::<TestDelay, _, _>(&mut job, io, clock, Some(timing), None, None).await?;
            binding.into_completion().map_err(|_| Failure::Binding)
        }
        PhyDcodeExternalBinding::Rfpll(Frequency::I2c(mut binding)) => {
            let mut job = Job::counted_i2c(binding.transaction_mut(), clock.now())?;
            drive::<TestDelay, _, _>(&mut job, io, clock, Some(timing), None, None).await?;
            binding
                .into_completion()
                .map(Completion::Rfpll)
                .map_err(|_| Failure::Binding)
        }
        PhyDcodeExternalBinding::Rfpll(binding) => {
            super::super::io::Io::Test(io, core::marker::PhantomData)
                .frequency::<TestDelay, _>(clock, binding)
                .map(Completion::Rfpll)
        }
    }
}
