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
                    atoms.get() - before <= 4,
                    "a ready peer permits only one bounded byte atom per poll"
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

#[test]
fn a_ready_rmw_write_is_observed_even_when_publication_crosses_the_deadline() {
    for busy in [false, true] {
        let clock = Rc::new(VirtualClock::<1>::new());
        let started = clock.now();
        let mut io = Io {
            i2c_write_publication: Some((clock.clone(), 21)),
            i2c_busy_after_write: busy,
            ..Io::default()
        };
        let mut transaction = PhyColdI2cTransaction::new(PhyColdI2cRequest::write_field(
            fields::RFPLL_DCODE_CKGEN_RESET,
            1,
        ));
        let result = super::super::rfpll::direct_i2c(
            &mut super::super::io::Io::Test(&mut io, core::marker::PhantomData),
            &*clock,
            &mut transaction,
            None,
            false,
            super::super::rfpll::I2cBoundary::Transaction,
        );
        assert_eq!(
            clock.now().saturating_duration_since(started).as_micros(),
            21,
            "no new settle window after publication"
        );
        assert_eq!(
            io.actions.len(),
            if busy { 5 } else { 4 },
            "read publication/readiness, write publication/readiness and, for a busy write, \
             the one readiness sample after the limit"
        );
        if busy {
            assert_eq!(result, Err(Failure::TimedOut));
            assert!(
                matches!(
                    transaction.action(),
                    PhyColdI2cAction::AwaitWriteCompletionEdge { .. }
                ),
                "pending hardware and cleanup obligation remain retained"
            );
        } else {
            result.expect("ready write completes in the publication attempt");
            assert!(matches!(
                transaction.action(),
                PhyColdI2cAction::Complete(_)
            ));
        }
    }
}

#[test]
fn runtime_rmw_retains_its_write_and_gives_each_byte_the_existing_settle() {
    let clock = Rc::new(VirtualClock::<1>::new());
    let mut io = Io {
        i2c_observation_clock: Some(clock.clone()),
        i2c_observations: std::iter::repeat_n([(10, true), (5, false)], 40)
            .flatten()
            .collect(),
        ..Io::default()
    };
    let mut transition = PhyDcodeTransition::new(PhyDcodeParameters {
        crystal_selector: 0,
    });
    let mut lock = None;
    let (waker, _) = context();
    let mut cx = Context::from_waker(&waker);
    {
        let mut job = Job::dcode(&mut transition, &mut lock, clock.now()).expect("deadline fits");
        let mut driver = core::pin::pin!(drive::<TestDelay, _, _>(
            &mut job, &mut io, &*clock, None, None, None
        ));
        let mut polls = 0;
        loop {
            polls += 1;
            assert!(polls < 200);
            let at = clock.now();
            let result = driver.as_mut().poll(&mut cx);
            assert!(clock.now().saturating_duration_since(at).as_micros() <= 50);
            if let Poll::Ready(result) = result {
                result.expect("two fifteen-us bytes are not a twenty-us RMW");
                break;
            }
            clock
                .advance(Duration::from_micros(100))
                .expect("suspension fits");
        }
    }
    assert!(io.i2c_observations.is_empty());
    assert_eq!(
        io.actions
            .iter()
            .filter(|a| matches!(
                a,
                PhyColdI2cAction::StartRead { .. } | PhyColdI2cAction::StartWrite { .. }
            ))
            .count(),
        40
    );
    assert!(matches!(transition.action(), PhyDcodeAction::Complete(_)));
}

#[test]
fn a_dcode_byte_that_stays_busy_keeps_the_original_twenty_us_timeout() {
    let clock = Rc::new(VirtualClock::<1>::new());
    let mut io = Io {
        i2c_observation_clock: Some(clock.clone()),
        // The last sample follows the passed limit and still finds it busy.
        i2c_observations: [(10, true), (15, true), (0, true)].into(),
        ..Io::default()
    };
    let mut transition = PhyDcodeTransition::new(PhyDcodeParameters {
        crystal_selector: 0,
    });
    let mut lock = None;
    let (waker, _) = context();
    let mut cx = Context::from_waker(&waker);
    let mut job = Job::dcode(&mut transition, &mut lock, clock.now()).expect("deadline fits");
    {
        let mut driver = core::pin::pin!(drive::<TestDelay, _, _>(
            &mut job, &mut io, &*clock, None, None, None
        ));
        let mut polls = 0;
        loop {
            polls += 1;
            assert!(polls < 100);
            if let Poll::Ready(result) = driver.as_mut().poll(&mut cx) {
                assert_eq!(result, Err(Failure::TimedOut));
                break;
            }
        }
    }
    let Work::Dcode {
        i2c: Some(binding), ..
    } = &job.work
    else {
        panic!("published byte is retained");
    };
    assert!(matches!(
        binding.action(),
        PhyColdI2cAction::AwaitReadCompletionEdge { .. }
    ));
    assert!(
        !io.actions
            .iter()
            .any(|a| matches!(a, PhyColdI2cAction::StartWrite { .. })),
        "no write after read timeout"
    );
}
