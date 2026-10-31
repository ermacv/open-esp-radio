use super::*;

pub(super) mod withdraw;

pub(super) async fn yield_once() {
    let mut yielded = false;
    core::future::poll_fn(|cx| {
        if core::mem::replace(&mut yielded, true) {
            Poll::Ready(())
        } else {
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    })
    .await
}

struct Barrier {
    configure: bool,
    observed: Rc<RefCell<Observed>>,
}
impl<Id: radio::SlotIdentity> PortBody<Id, Backend<Id>> for Barrier {
    type Output = ();
    async fn run<'p, 'id>(
        self,
        owner: radio::Owner<'p, 'id, Id, Backend<Id>, radio::Disabled>,
        mut resources: Res<'id, Id>,
    ) -> radio::Ended<Id, Backend<Id>, ()>
    where
        Backend<Id>: 'id,
        Id: 'id,
        'id: 'p,
    {
        let mut owner = match owner.enable().await {
            Ok(owner) => owner,
            Err(_) => panic!("enable"),
        };
        {
            let mut phase = owner.phase();
            let [head, tail, last, ..] = resources.buffers;
            let [head_ticket, tail_ticket, last_ticket, ..] = resources.tickets;
            let expected = [
                head_ticket.index(),
                tail_ticket.index(),
                last_ticket.index(),
            ];
            self.observed.borrow_mut().hold_completion = true;
            submit_mpdu(&mut phase, head, head_ticket, None, None);
            for _ in 0..4 {
                yield_once().await;
            }
            assert_eq!(self.observed.borrow().publications, 1);
            submit_mpdu(&mut phase, tail, tail_ticket, None, None);
            submit_mpdu(&mut phase, last, last_ticket, None, None);
            if self.configure {
                phase.station().configure(
                    resources
                        .station_commands
                        .configure
                        .with_context(Tag(314, None)),
                    StationReceive::Off,
                );
            } else {
                phase.close_gate(resources.gate);
            }
            for _ in 0..4 {
                yield_once().await;
            }
            {
                let mut event = pin!(phase.next_event());
                assert!(
                    core::future::poll_fn(|cx| Poll::Ready(event.as_mut().poll(cx)))
                        .await
                        .is_pending(),
                    "neither a queued tail nor its barrier may overtake the published head"
                );
            }
            self.observed.borrow_mut().hold_completion = false;
            for (position, ticket) in expected.into_iter().enumerate() {
                let completed = match phase.next_event().await {
                    Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::TxCompleted(
                        TxCompleted::Single(completed),
                    )))) => completed,
                    _ => panic!("TX terminal precedes the barrier confirmation"),
                };
                let returned = completed.into_parts(&mut resources.terminal_access);
                assert_eq!(returned.ticket.index(), ticket, "original per-AC order");
                let (buffer, _) = returned.payload.into_parts();
                if position == 0 {
                    assert!(returned.outcome.is_delivered());
                    assert!(matches!(buffer, AnyBuffer::Sent(_)));
                } else {
                    assert_eq!(
                        returned.outcome,
                        TxOutcome::Withdrawn(if self.configure {
                            WithdrawCause::Disabled
                        } else {
                            WithdrawCause::GateClosed
                        })
                    );
                    assert!(matches!(buffer, AnyBuffer::Fresh(_)));
                }
            }
            match phase.next_event().await {
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::Configured(
                    Configured::Station(permission),
                )))) if self.configure => assert_eq!(permission.into_parts().1, 314),
                Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::GateClosed(gate))))
                    if !self.configure =>
                {
                    let _gate = phase.open_gate(gate);
                }
                _ => panic!("the barrier follows all earlier TX terminals"),
            }
            assert_eq!(self.observed.borrow().publications, 1);
        }
        let owner = match owner
            .disable(None, &mut |_| core::ops::ControlFlow::Continue(()))
            .await
        {
            Ok(owner) => owner,
            Err(_) => panic!("disable"),
        };
        radio::Ended::uninstall(owner, (), ())
    }
}

#[test]
fn close_gate_returns_published_head_then_queued_tails_then_confirmation() {
    check(false);
}

#[test]
fn configure_returns_published_head_then_queued_tails_then_confirmation() {
    check(true);
}

fn check(configure: bool) {
    let (backend, observed, _) = installed(Vec::new());
    assert!(matches!(
        block_on(run_body(
            backend,
            Barrier {
                configure,
                observed
            }
        )),
        Report::Uninstalled { .. }
    ));
}
