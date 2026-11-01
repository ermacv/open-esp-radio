//! A stop's deadline in the model: no disabled owner and no join after it.

use super::*;
use oer_radio_port::DisableFailed;
use oer_time::Duration;
use oer_time_virtual::VirtualClock;

/// How far into the stop the deadline lies.
const DEADLINE: Duration = Duration::from_millis(1);

/// Enable, configure the station so that a terminal waits for the drain,
/// and disable by a deadline; `past` moves the clock beyond it from the
/// disable's sink. `disabled` records whether the disable issued an owner.
struct StopBy {
    clock: Rc<VirtualClock>,
    past: bool,
    disabled: Rc<Cell<Option<bool>>>,
}
impl<Id: SlotIdentity> PortBody<Id, ModelBackend<Id>> for StopBy {
    type Output = ();
    async fn run<'a, 'p>(
        self,
        owner: Owner<'a, 'p, Id, ModelBackend<Id>, Disabled>,
        resources: ModelResources<'p, ModelBackend<Id>>,
    ) -> Ended<Id, ModelBackend<Id>, ()>
    where
        ModelBackend<Id>: 'p,
        Id: 'p,
        'p: 'a,
    {
        let mut owner = owner.enable().await.ok().unwrap();
        owner
            .phase()
            .station()
            .configure(resources.station_commands.configure, StationReceive::Off);
        let deadline = self.clock.now().saturating_add(DEADLINE);
        let mut offered = 0;
        let disabled = owner
            .disable(Some(deadline), &mut |_| {
                offered += 1;
                if self.past {
                    self.clock.advance(DEADLINE).unwrap();
                }
                core::ops::ControlFlow::Continue(())
            })
            .await;
        assert_eq!(offered, 1, "the configure terminal reaches the sink");
        match disabled {
            Ok(disabled) => {
                self.disabled.set(Some(true));
                Ended::uninstall(disabled, (), ())
            }
            Err(DisableFailed::Expired(request)) => {
                self.disabled.set(Some(false));
                Ended::fail_stop(request, ())
            }
            Err(DisableFailed::Poisoned(_)) => panic!("the model has no fault here"),
        }
    }
}

#[test]
fn a_disable_whose_sink_runs_past_the_deadline_issues_no_disabled_owner() {
    for past in [false, true] {
        let clock = Rc::new(VirtualClock::new());
        let (backend, air) = backend_on(clock.clone());
        let disabled = Rc::new(Cell::new(None));
        let report = block_on(run_model(
            backend,
            StopBy {
                clock,
                past,
                disabled: disabled.clone(),
            },
        ));
        assert_eq!(disabled.get(), Some(!past));
        assert_eq!(air.disable_calls(), 1);
        if past {
            assert!(matches!(report, Report::Unconfirmed(())));
        } else {
            assert!(matches!(report, Report::Joined(())));
        }
    }
}

#[test]
fn a_join_answer_after_the_stop_deadline_issues_no_joined() {
    // `late` holds P2's answer past the deadline and releases it with the
    // clock's advance, so both are ready at the next poll; otherwise it is
    // released in time.
    for late in [false, true] {
        let clock = Rc::new(VirtualClock::new());
        let (backend, air) = backend_on(clock.clone());
        let disabled = Rc::new(Cell::new(None));
        air.hold_retirement(true);
        let mut running = Box::pin(run_model(
            backend,
            StopBy {
                clock: clock.clone(),
                past: false,
                disabled: disabled.clone(),
            },
        ));
        let mut cx = core::task::Context::from_waker(Waker::noop());
        for _ in 0..8 {
            assert!(running.as_mut().poll(&mut cx).is_pending(), "P2 is held");
        }
        assert_eq!(disabled.get(), Some(true), "the disable ended in time");
        if late {
            clock.advance(DEADLINE).unwrap();
        }
        air.hold_retirement(false);
        air.clock_advanced();
        let report = (0..8)
            .find_map(|_| match running.as_mut().poll(&mut cx) {
                Poll::Ready(report) => Some(report),
                Poll::Pending => None,
            })
            .expect("the stop ends once P2 answers or the deadline passes");
        if late {
            assert!(matches!(report, Report::Unconfirmed(())));
        } else {
            assert!(matches!(report, Report::Joined(())));
        }
        assert_eq!(air.disable_calls(), 1);
    }
}
