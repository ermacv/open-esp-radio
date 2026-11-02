//! Finite retained-wake analog-I2C host sequencing.
use oer_espressif_analog::model::Host;

/// One finite failure while publishing current-vendor retained-wake analog
/// initialization through both PHY-I²C hosts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyI2cInitializationStageTwoError {
    Transition(oer_espressif_analog::TransitionError),
    BusyAtStart { host: Host },
    CompletionTimeout { host: Host, pair: u8 },
}

pub trait PhyI2cParallelAccess {
    type Pair: Copy;
    fn select_parallel_host_map(&mut self);
    fn restore_radio_host_map(&mut self);
    fn is_busy(&self, host: Host) -> bool;
    fn start_pair(&mut self, pair: Self::Pair);
}

pub async fn configure_initialization_stage_two<P: PhyI2cParallelAccess>(
    access: &mut P,
    pair: impl Fn(usize) -> Option<P::Pair>,
    maximum_observations: u32,
    mut wait: impl AsyncFnMut(),
) -> Result<(), PhyI2cInitializationStageTwoError> {
    use oer_espressif_analog::{ParallelAction, ParallelHost, ParallelTransition};
    let host = |host| match host {
        ParallelHost::First => Host::Host0,
        ParallelHost::Second => Host::Host1,
    };
    let mut transition = ParallelTransition::new(pair);
    let mut result = Ok(());
    loop {
        match transition.action() {
            ParallelAction::SelectParallelMap => {
                access.select_parallel_host_map();
                // Both hosts must be idle before the first pair starts.
                if let Some(busy) = [Host::Host0, Host::Host1]
                    .into_iter()
                    .find(|host| access.is_busy(*host))
                {
                    result = Err(PhyI2cInitializationStageTwoError::BusyAtStart { host: busy });
                    break;
                }
            }
            ParallelAction::StartPair(pair) => access.start_pair(pair),
            ParallelAction::AwaitIdle(parallel_host) => {
                let awaited = host(parallel_host);
                let mut observations = 0;
                while access.is_busy(awaited) {
                    if observations == maximum_observations {
                        result = Err(PhyI2cInitializationStageTwoError::CompletionTimeout {
                            host: awaited,
                            pair: transition.pair_index().unwrap_or(0) as u8,
                        });
                        break;
                    }
                    observations = observations.saturating_add(1);
                    wait().await;
                }
                if result.is_err() {
                    break;
                }
            }
            ParallelAction::RestoreMap | ParallelAction::Complete => break,
        }
        if let Err(error) = transition.advance() {
            result = Err(PhyI2cInitializationStageTwoError::Transition(error));
            break;
        }
    }

    // The vendor leaf restores the normal 0x3fa0 host field after the
    // parallel sequence. OER also restores it on finite failure so diagnostic
    // access remains well-defined; the outer RF-wake epoch still fails closed.
    access.restore_radio_host_map();
    result
}

/// Two host commands, published host zero first.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyI2cParallelWrite<A> {
    pub first: (A, u8),
    pub second: (A, u8),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, vec::Vec};
    fn run_parallel_stage(
        access: &mut impl PhyI2cParallelAccess<Pair = usize>,
        _inputs: (),
        budget: u32,
    ) -> Result<(), PhyI2cInitializationStageTwoError> {
        use core::{
            future::Future,
            task::{Context, Poll, Waker},
        };
        let mut future = core::pin::pin!(configure_initialization_stage_two(
            access,
            |index| (index < 22).then_some(index),
            budget,
            async || {}
        ));
        match future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(result) => result,
            Poll::Pending => panic!("an immediately ready test wait does not suspend"),
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum ParallelEvent {
        SelectParallelMap,
        StartPair,
        RestoreRadioMap,
    }

    #[derive(Default)]
    struct FakeParallelI2c {
        events: Vec<ParallelEvent>,
        initially_busy_host: Option<Host>,
        busy_after_start_host: Option<Host>,
        started: Cell<bool>,
    }

    impl PhyI2cParallelAccess for FakeParallelI2c {
        type Pair = usize;
        fn select_parallel_host_map(&mut self) {
            self.events.push(ParallelEvent::SelectParallelMap);
        }

        fn restore_radio_host_map(&mut self) {
            self.events.push(ParallelEvent::RestoreRadioMap);
        }

        fn is_busy(&self, host: Host) -> bool {
            if self.started.get() {
                self.busy_after_start_host == Some(host)
            } else {
                self.initially_busy_host == Some(host)
            }
        }

        fn start_pair(&mut self, _pair: Self::Pair) {
            self.started.set(true);
            self.events.push(ParallelEvent::StartPair);
        }
    }

    #[test]
    fn retained_wake_i2c_stage_two_pairs_both_hosts_and_restores_the_radio_map() {
        let mut access = FakeParallelI2c::default();
        run_parallel_stage(&mut access, (), 10).unwrap();

        assert_eq!(
            access.events.first(),
            Some(&ParallelEvent::SelectParallelMap)
        );
        assert_eq!(access.events.last(), Some(&ParallelEvent::RestoreRadioMap));
        let pairs = access
            .events
            .iter()
            .filter(|event| **event == ParallelEvent::StartPair)
            .count();
        assert_eq!(pairs, 22);
    }

    #[test]
    fn retained_wake_i2c_stage_two_restores_the_radio_map_on_busy_entry() {
        let mut access = FakeParallelI2c {
            initially_busy_host: Some(Host::Host1),
            ..FakeParallelI2c::default()
        };
        assert_eq!(
            run_parallel_stage(&mut access, (), 10,),
            Err(PhyI2cInitializationStageTwoError::BusyAtStart { host: Host::Host1 })
        );
        assert_eq!(
            access.events,
            [
                ParallelEvent::SelectParallelMap,
                ParallelEvent::RestoreRadioMap,
            ]
        );
    }

    #[test]
    fn retained_wake_i2c_stage_two_bounds_completion_and_restores_the_radio_map() {
        let mut access = FakeParallelI2c {
            busy_after_start_host: Some(Host::Host0),
            ..FakeParallelI2c::default()
        };
        assert_eq!(
            run_parallel_stage(&mut access, (), 2,),
            Err(PhyI2cInitializationStageTwoError::CompletionTimeout {
                host: Host::Host0,
                pair: 0,
            })
        );
        assert_eq!(access.events.last(), Some(&ParallelEvent::RestoreRadioMap));
    }

    #[test]
    fn a_busy_parallel_host_suspends_before_the_next_observation() {
        use core::{
            future::{Future, pending},
            task::{Context, Poll, Waker},
        };
        let mut access = FakeParallelI2c {
            busy_after_start_host: Some(Host::Host0),
            ..FakeParallelI2c::default()
        };
        let mut future = core::pin::pin!(configure_initialization_stage_two(
            &mut access,
            |index| (index < 22).then_some(index),
            2,
            async || pending::<()>().await
        ));
        assert_eq!(
            future
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Pending
        );
    }

    #[test]
    fn each_busy_read_waits_for_virtual_time_before_retry_and_timeout() {
        use core::{
            future::Future,
            task::{Context, Poll, Waker},
        };
        use oer_time::{Clock, Duration, Timer};
        let clock = oer_time_virtual::VirtualClock::<1>::new();
        let mut access = FakeParallelI2c {
            busy_after_start_host: Some(Host::Host1),
            ..FakeParallelI2c::default()
        };
        {
            let mut future = core::pin::pin!(configure_initialization_stage_two(
                &mut access,
                |index| (index < 1).then_some(index),
                2,
                async || {
                    clock.wait_for(Duration::from_micros(1)).await.unwrap();
                },
            ));
            let mut cx = Context::from_waker(Waker::noop());
            assert_eq!(future.as_mut().poll(&mut cx), Poll::Pending);
            assert_eq!(future.as_mut().poll(&mut cx), Poll::Pending);
            clock.advance(Duration::from_micros(1)).unwrap();
            assert_eq!(future.as_mut().poll(&mut cx), Poll::Pending);
            clock.advance(Duration::from_micros(1)).unwrap();
            assert_eq!(
                future.as_mut().poll(&mut cx),
                Poll::Ready(Err(PhyI2cInitializationStageTwoError::CompletionTimeout {
                    host: Host::Host1,
                    pair: 0
                }))
            );
        }
        assert_eq!(access.events.last(), Some(&ParallelEvent::RestoreRadioMap));
        assert_eq!(
            clock.now(),
            oer_time_virtual::START
                .checked_add(Duration::from_micros(2))
                .unwrap()
        );
    }
    #[test]
    fn cancellation_does_not_restore_or_complete_an_in_flight_pair() {
        use core::{
            future::Future,
            task::{Context, Poll, Waker},
        };
        use oer_time::{Duration, Timer};
        let clock = oer_time_virtual::VirtualClock::<1>::new();
        let mut access = FakeParallelI2c {
            busy_after_start_host: Some(Host::Host0),
            ..FakeParallelI2c::default()
        };
        {
            let mut future = core::pin::pin!(configure_initialization_stage_two(
                &mut access,
                |index| (index < 1).then_some(index),
                2,
                async || {
                    clock.wait_for(Duration::from_micros(1)).await.unwrap();
                },
            ));
            assert_eq!(
                future
                    .as_mut()
                    .poll(&mut Context::from_waker(Waker::noop())),
                Poll::Pending
            );
        }
        assert_eq!(
            access.events,
            [ParallelEvent::SelectParallelMap, ParallelEvent::StartPair]
        );
    }
}
