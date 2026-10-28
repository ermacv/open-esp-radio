//! Executable fixtures for the portable suite, using registered model backends.
#![allow(
    clippy::expect_used,
    clippy::disallowed_methods,
    clippy::disallowed_macros,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "fixture constants and single-use completed futures are internal invariants"
)]
use super::*;
use alloc::rc::Rc;
use core::{
    future::{Future, poll_fn},
    num::NonZeroU64,
    ops::ControlFlow,
    pin::pin,
    task::{Context, Poll, Waker},
};
use oer_bluetooth_le_port::{conformance::*, *};
use oer_radio_port::*;
use oer_time::Duration;

/// Reference fixture. Every case creates a fresh environment and local slots;
/// all deadlines and packet arrivals are driven by that environment's owner.
pub struct ModelFixture {
    power: TxPower,
}
impl ModelFixture {
    /// Use the fixture's explicit, regulatory-capped DTM TX profile.
    pub const fn new(power: TxPower) -> Self {
        Self { power }
    }
}
impl Fixture for ModelFixture {
    fn capabilities(&self) -> LeRadioCapabilities {
        ModelCaps::CAPS
    }
    async fn run(&mut self, case: Case) -> Result<Trace, Violation> {
        if matches!(case, Case::Dtm | Case::Dtm2M | Case::Loss) {
            return pair(case, self.power).await;
        }
        let env = Rc::new(Environment::new(
            const { NonZeroU64::new(1_000).expect("nonzero profile period") },
        ));
        with_local_slot(async |claim| {
            let backend = Backend::<_, Tag>::new(env.clone());
            let radio = backend.radio.clone();
            let mut slot = claim.place();
            let end = LocalExecution
                .scope(
                    slot.register(backend).map_err(|_| Violation::Lifecycle)?,
                    async |f| {
                        run_port(
                            f,
                            Body {
                                case,
                                env: env.clone(),
                                radio: radio.clone(),
                            },
                        )
                        .await
                    },
                )
                .await;
            let (mut trace, joined) = match end {
                PortEnd::Uninstalled { joined, output } => (output?, Some(joined)),
                PortEnd::FailStopped { output, .. } => (output?, None),
                PortEnd::Abandoned { .. } => return Err(Violation::Lifecycle),
            };
            if let Some(joined) = joined {
                trace.joins = trace.joins.saturating_add(1);
                if case == Case::Reinstall {
                    let previous = trace.original.ok_or(Violation::Owners)?;
                    let end = LocalExecution
                        .scope(
                            slot.reinstall(joined, |v| {
                                Backend::resume(v).expect("fixture epoch has room")
                            })
                            .map_err(|_| Violation::Lifecycle)?,
                            async |f| {
                                run_port(
                                    f,
                                    Body {
                                        case,
                                        env: env.clone(),
                                        radio: radio.clone(),
                                    },
                                )
                                .await
                            },
                        )
                        .await;
                    match end {
                        PortEnd::Uninstalled { output, .. } => {
                            let next = output?;
                            if next.original.ok_or(Violation::Owners)?.epoch
                                != previous.epoch.checked_add(1).ok_or(Violation::Owners)?
                            {
                                return Err(Violation::Owners);
                            }
                            trace.reinstalled = next.original;
                            trace.events[1] = next.events[0];
                            trace.submitted[1] = next.submitted[0];
                            trace.joins = trace.joins.saturating_add(1);
                        }
                        _ => return Err(Violation::Lifecycle),
                    }
                }
            }
            slot.end_model_world();
            if env.active_radios() != 0 {
                return Err(Violation::Lifecycle);
            }
            Ok(trace)
        })
        .await
    }
}
// Every yield permits an independent P2 poll; this never advances wall time.
async fn yield_once() {
    let mut yielded = false;
    poll_fn(|cx| {
        if core::mem::replace(&mut yielded, true) {
            Poll::Ready(())
        } else {
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    })
    .await
}
fn ready<F: Future>(future: core::pin::Pin<&mut F>) -> Result<F::Output, Violation> {
    match future.poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(value) => Ok(value),
        Poll::Pending => Err(Violation::Fifo),
    }
}
fn pending<F: Future>(future: core::pin::Pin<&mut F>) -> Result<(), Violation> {
    if future
        .poll(&mut Context::from_waker(Waker::noop()))
        .is_pending()
    {
        Ok(())
    } else {
        Err(Violation::Fifo)
    }
}
fn receive() -> Result<Supported<ModelCaps, Receive>, Violation> {
    Supported::new(Receive {
        channel: TestChannel::new(19).map_err(|_| Violation::Terminal)?,
        phy: TestPhy::Le1M,
    })
    .map_err(|_| Violation::Terminal)
}
struct Body {
    case: Case,
    env: Rc<Environment>,
    radio: Rc<super::air::Radio>,
}
impl<Id: SlotIdentity> PortBody<Id, Backend<Id, Tag>> for Body {
    type Output = Result<Trace, Violation>;
    async fn run<'a, 'p>(
        self,
        owner: Owner<'a, 'p, Id, Backend<Id, Tag>, Disabled>,
        resources: Result<Resources<'p, Id>, Fault>,
    ) -> Ended<Id, Backend<Id, Tag>, Self::Output>
    where
        Backend<Id, Tag>: 'p,
        Id: 'p,
        'p: 'a,
    {
        let resources = match resources {
            Ok(resources) => resources,
            Err(_) => return Ended::fail_stop(owner.fail_stop(), Err(Violation::Lifecycle)),
        };
        let mut owner = match owner.enable().await {
            Ok(o) => o,
            Err(f) => return Ended::fail_stop(f.fail_stop(), Err(Violation::Lifecycle)),
        };
        let mut trace = Trace {
            original: Some(Identity::session(&resources.session, 71)),
            ..Trace::default()
        };
        let shared = {
            let phase = owner.phase();
            phase.view.shared
        };
        let result = exchange(
            self.case,
            &mut owner.phase(),
            resources,
            &mut trace,
            &self.env,
            self.radio,
        )
        .await;
        if let Err(v) = result {
            return Ended::fail_stop(owner.fail_stop(), Err(v));
        }
        if matches!(self.case, Case::Indeterminate | Case::DrainedFault) {
            return Ended::fail_stop(owner.fail_stop(), Ok(trace));
        }
        let rejected = self.case == Case::RejectSink;
        let owner = owner
            .disable(&mut |terminal| {
                trace.events[0] = Some(Observation::capture(&terminal));
                if rejected {
                    ControlFlow::Break(terminal)
                } else {
                    trace.disabled = trace.disabled.saturating_add(1);
                    ControlFlow::Continue(())
                }
            })
            .await;
        if rejected {
            let retained = shared.events.with(|q| {
                if let Some(event) = q.front() {
                    let (slot, epoch) = match &event.session {
                        super::queues::SessionOwner::Idle(s) => (s.index(), s.epoch()),
                        super::queues::SessionOwner::Running(s) => (s.index(), s.epoch()),
                        _ => return Err(Violation::Owners),
                    };
                    Ok(Some(Identity {
                        slot,
                        epoch,
                        context: event.context.owner,
                    }))
                } else {
                    Ok(None)
                }
            });
            trace.retained = match retained {
                Ok(retained) => retained,
                Err(v) => {
                    return match owner {
                        Ok(o) => Ended::fail_stop(o.fail_stop(), Err(v)),
                        Err(o) => Ended::fail_stop(o.fail_stop(), Err(v)),
                    };
                }
            };
            trace.end = shared
                .end
                .get()
                .map(|end| matches!(end, EventEnd::Drained { .. }));
            return match owner {
                Ok(o) => Ended::fail_stop(o.fail_stop(), Err(Violation::Lifecycle)),
                Err(o) => Ended::fail_stop(o.fail_stop(), Ok(trace)),
            };
        }
        match owner {
            Ok(owner) => Ended::uninstall(owner, (), Ok(trace)),
            Err(owner) => Ended::fail_stop(owner.fail_stop(), Err(Violation::Lifecycle)),
        }
    }
}
async fn exchange<'p, Id: SlotIdentity + 'p>(
    case: Case,
    phase: &mut ModelPhase<'_, 'p, Id, Tag>,
    resources: Resources<'p, Id>,
    trace: &mut Trace,
    env: &Environment,
    radio: Rc<super::air::Radio>,
) -> Result<(), Violation> {
    let Resources {
        seats: [mut s0, mut s1, mut s2],
        session,
    } = resources;
    if phase.capabilities() != ModelCaps::CAPS {
        return Err(Violation::Terminal);
    }
    if case == Case::Capacity {
        let indices = [s1.index(), s2.index(), s0.index()];
        let a = phase.take(&mut s0).await;
        let b = phase.take(&mut s1).await;
        {
            let mut cancelled = pin!(phase.take(&mut s0));
            pending(cancelled.as_mut())?;
        }
        let mut first = pin!(phase.take(&mut s1));
        let mut second = pin!(phase.take(&mut s2));
        let mut last = pin!(phase.take(&mut s0));
        pending(first.as_mut())?;
        pending(second.as_mut())?;
        pending(last.as_mut())?;
        phase.release(a);
        phase.release(b);
        pending(second.as_mut())?;
        let a = ready(first.as_mut())?;
        trace.fifo[0] = Some(indices[0]);
        let b = ready(second.as_mut())?;
        trace.fifo[1] = Some(indices[1]);
        pending(last.as_mut())?;
        phase.release(a);
        let a = ready(last.as_mut())?;
        trace.fifo[2] = Some(indices[2]);
        phase.release(a);
        phase.release(b);
        return Ok(());
    }
    let capacity = phase.take(&mut s0).await;
    let request = Request {
        id: LeOperationId(if case == Case::Refusal {
            0xFFFF_FF00
        } else {
            17
        }),
        capacity,
        command: Command::DirectTest(DtmCommand::Receive(session, receive()?)),
        context: PortContext::new(Tag { owner: 71 }),
    };
    if case == Case::Refusal {
        let index = request.capacity.index();
        let request = match phase.submit(request) {
            Ok(Err(refusal)) if refusal.reason == Refusal::ReservedId => refusal.request,
            _ => return Err(Violation::Terminal),
        };
        let Command::DirectTest(command) = request.command;
        let (session, config) = match command {
            DtmCommand::Receive(s, c) => (s, c),
            _ => return Err(Violation::Owners),
        };
        trace.retained = Some(Identity::session(&session, request.context.view()));
        if request.capacity.index() != index || config.get() != receive()?.get() {
            return Err(Violation::Owners);
        }
        phase.release(request.capacity);
        return Ok(());
    }
    let stimulus = match case {
        Case::Aborted => Some(Stimulus::Withdrawn),
        Case::AirDenied => Some(Stimulus::AirDenied),
        Case::Indeterminate => Some(Stimulus::Indeterminate),
        Case::DrainedFault => Some(Stimulus::DrainedFault),
        Case::StaleEpoch => Some(Stimulus::StaleEpoch),
        _ => None,
    };
    if let Some(input) = stimulus {
        env.inject(&radio, input);
    }
    let before_admission = env.now();
    trace.submitted[0] = Some(
        phase
            .submit(request)
            .map_err(|_| Violation::Terminal)?
            .map_err(|_| Violation::Terminal)?,
    );
    if matches!(case, Case::DisableQueued | Case::RejectSink) {
        return Ok(());
    }
    if matches!(
        case,
        Case::DisableExecuted | Case::DropAfterAdmission | Case::Aborted
    ) {
        if case == Case::DropAfterAdmission {
            env.advance(Duration::from_micros(1_000))
                .map_err(|_| Violation::Terminal)?;
            {
                let mut wait = pin!(phase.next_event());
                pending(wait.as_mut())?;
            }
            if phase.view.shared.body.is_some() {
                return Err(Violation::Lifecycle);
            }
        }
        yield_once().await;
        if phase.view.shared.events.with(|events| events.len()) != 1 {
            return Err(Violation::Terminal);
        }
        if matches!(case, Case::DisableExecuted | Case::Aborted) {
            return Ok(());
        }
    }
    let Event::Terminal(terminal) = phase
        .next_event()
        .await
        .map_err(|_| Violation::Terminal)?
        .map_err(|_| Violation::Terminal)?;
    if terminal.admitted_at != before_admission {
        return Err(Violation::Terminal);
    }
    trace.events[0] = Some(Observation::capture(&terminal));
    if case == Case::Indeterminate {
        trace.retained = Some(phase.view.shared.quarantine.with(|retained| {
            let (command, _) = retained.first().ok_or(Violation::Owners)?;
            Ok(Identity {
                slot: command.index(),
                epoch: command.epoch(),
                context: terminal.context.view(),
            })
        })?);
    }
    if matches!(case, Case::Indeterminate | Case::DrainedFault) {
        let end = phase.next_event().await.err().ok_or(Violation::Lifecycle)?;
        trace.end = Some(matches!(end, EventEnd::Drained { .. }));
        // P2 cleanup cannot rewrite a final end already exposed to the body.
        phase.view.shared.poison(Fault::Injected);
        // The final event end is stable on repeated dequeue, with its cause.
        if phase.next_event().await.err() != Some(end) {
            return Err(Violation::Lifecycle);
        }
    }
    Ok(())
}

struct Pair {
    env: Rc<Environment>,
    power: TxPower,
    receiving: core::cell::Cell<bool>,
    ended: core::cell::Cell<bool>,
    rx: core::cell::RefCell<Option<Waker>>,
    tx: core::cell::RefCell<Option<Waker>>,
}
impl Pair {
    async fn wait(&self, receive: bool) {
        poll_fn(|cx| {
            let (ready, waker) = if receive {
                (&self.receiving, &self.tx)
            } else {
                (&self.ended, &self.rx)
            };
            if ready.get() {
                Poll::Ready(())
            } else {
                *waker.borrow_mut() = Some(cx.waker().clone());
                Poll::Pending
            }
        })
        .await
    }
    fn notify(&self, receive: bool) {
        let (ready, waker) = if receive {
            (&self.receiving, &self.tx)
        } else {
            (&self.ended, &self.rx)
        };
        ready.set(true);
        if let Some(waker) = waker.borrow_mut().take() {
            waker.wake();
        }
    }
}
struct AirBody {
    pair: Rc<Pair>,
    transmit: bool,
}
impl<Id: SlotIdentity> PortBody<Id, Backend<Id, Tag>> for AirBody {
    type Output = Result<Trace, Violation>;
    async fn run<'a, 'p>(
        self,
        owner: Owner<'a, 'p, Id, Backend<Id, Tag>, Disabled>,
        resources: Result<Resources<'p, Id>, Fault>,
    ) -> Ended<Id, Backend<Id, Tag>, Self::Output>
    where
        Backend<Id, Tag>: 'p,
        Id: 'p,
        'p: 'a,
    {
        let resources = match resources {
            Ok(resources) => resources,
            Err(_) => return Ended::fail_stop(owner.fail_stop(), Err(Violation::Lifecycle)),
        };
        let mut owner = match owner.enable().await {
            Ok(owner) => owner,
            Err(f) => return Ended::fail_stop(f.fail_stop(), Err(Violation::Lifecycle)),
        };
        let result = air_exchange(&mut owner.phase(), resources, &self.pair, self.transmit).await;
        if result.is_err() {
            return Ended::fail_stop(owner.fail_stop(), result);
        }
        match owner.disable(&mut |_| ControlFlow::Continue(())).await {
            Ok(owner) => Ended::uninstall(owner, (), result),
            Err(owner) => Ended::fail_stop(owner.fail_stop(), Err(Violation::Lifecycle)),
        }
    }
}
async fn air_exchange<'p, Id: SlotIdentity + 'p>(
    phase: &mut ModelPhase<'_, 'p, Id, Tag>,
    resources: Resources<'p, Id>,
    pair: &Pair,
    tx: bool,
) -> Result<Trace, Violation> {
    let Resources {
        seats: [mut seat, ..],
        session,
    } = resources;
    let mut trace = Trace {
        original: Some(Identity::session(&session, 71)),
        ..Trace::default()
    };
    if tx {
        pair.wait(true).await;
    }
    let command = if tx {
        DtmCommand::Transmit(
            session,
            Supported::new(Transmit {
                channel: TestChannel::new(19).map_err(|_| Violation::Terminal)?,
                phy: TestPhy::Le1M,
                payload: TestPayloadType::new(0).map_err(|_| Violation::Terminal)?,
                length: 37,
                power: pair.power,
            })
            .map_err(|_| Violation::Terminal)?,
        )
    } else {
        DtmCommand::Receive(session, receive()?)
    };
    let (receipt, terminal) = transact(
        phase,
        &mut seat,
        LeOperationId(1),
        command,
        PortContext::new(Tag { owner: 71 }),
    )
    .await?;
    trace.submitted[0] = Some(receipt);
    trace.events[0] = Some(Observation::capture(&terminal));
    let Completion::DirectTest {
        session: ReturnedSession::Running(running),
        ..
    } = terminal.completion
    else {
        return Err(Violation::Terminal);
    };
    if tx {
        pair.env
            .advance(Duration::from_micros(10_000))
            .map_err(|_| Violation::Terminal)?;
    } else {
        pair.notify(true);
        pair.wait(false).await;
    }
    let (receipt, terminal) = transact(
        phase,
        &mut seat,
        LeOperationId(2),
        DtmCommand::End(running),
        PortContext::new(Tag { owner: 71 }),
    )
    .await?;
    trace.submitted[1] = Some(receipt);
    trace.events[1] = Some(Observation::capture(&terminal));
    let Completion::DirectTest {
        session: ReturnedSession::Idle(session),
        count,
    } = terminal.completion
    else {
        return Err(Violation::Terminal);
    };
    if tx {
        if count != Some(PacketCount::default()) {
            return Err(Violation::Terminal);
        }
        pair.env
            .advance(Duration::from_micros(10_000))
            .map_err(|_| Violation::Terminal)?;
        pair.notify(false);
    } else {
        let (_, terminal) = transact(
            phase,
            &mut seat,
            LeOperationId(3),
            DtmCommand::Receive(session, receive()?),
            PortContext::new(Tag { owner: 71 }),
        )
        .await?;
        let Completion::DirectTest {
            session: ReturnedSession::Running(running),
            ..
        } = terminal.completion
        else {
            return Err(Violation::Terminal);
        };
        pair.env
            .advance(Duration::from_micros(10_000))
            .map_err(|_| Violation::Terminal)?;
        let (_, terminal) = transact(
            phase,
            &mut seat,
            LeOperationId(4),
            DtmCommand::End(running),
            PortContext::new(Tag { owner: 71 }),
        )
        .await?;
        let Completion::DirectTest { count, .. } = terminal.completion;
        trace.quiet = count;
    }
    Ok(trace)
}
async fn join<A: Future, B: Future>(a: A, b: B) -> (A::Output, B::Output) {
    let mut a = pin!(a);
    let mut b = pin!(b);
    let mut x = None;
    let mut y = None;
    poll_fn(|cx| {
        if x.is_none()
            && let Poll::Ready(value) = a.as_mut().poll(cx)
        {
            x = Some(value);
        }
        if y.is_none()
            && let Poll::Ready(value) = b.as_mut().poll(cx)
        {
            y = Some(value);
        }
        if x.is_some() && y.is_some() {
            Poll::Ready((
                x.take().expect("ready first world"),
                y.take().expect("ready second world"),
            ))
        } else {
            Poll::Pending
        }
    })
    .await
}
async fn pair(case: Case, power: TxPower) -> Result<Trace, Violation> {
    let env = Rc::new(Environment::new(
        const { NonZeroU64::new(1_000).expect("nonzero profile period") },
    ));
    if case == Case::Loss {
        env.set_loss(Loss::EveryNth(
            const { core::num::NonZeroU32::new(2).expect("nonzero loss period") },
        ));
    }
    let pair = Rc::new(Pair {
        env: env.clone(),
        power,
        receiving: core::cell::Cell::new(false),
        ended: core::cell::Cell::new(false),
        rx: core::cell::RefCell::new(None),
        tx: core::cell::RefCell::new(None),
    });
    let run = async |tx: bool| {
        with_local_slot(async |claim| {
            let backend = Backend::<_, Tag>::new(env.clone());
            let mut slot = claim.place();
            let end = LocalExecution
                .scope(
                    slot.register(backend).map_err(|_| Violation::Lifecycle)?,
                    async |f| {
                        run_port(
                            f,
                            AirBody {
                                pair: pair.clone(),
                                transmit: tx,
                            },
                        )
                        .await
                    },
                )
                .await;
            let output = match end {
                PortEnd::Uninstalled { output, .. } => output,
                _ => Err(Violation::Lifecycle),
            };
            slot.end_model_world();
            output
        })
        .await
    };
    let (rx, tx) = join(run(false), run(true)).await;
    let mut rx = rx?;
    let tx = tx?;
    if !matches!(
        tx.events[0].ok_or(Violation::Terminal)?.result,
        EventResult::Executed { .. }
    ) {
        return Err(Violation::Terminal);
    }
    if env.active_radios() != 0 {
        return Err(Violation::Lifecycle);
    }
    rx.joins = 2;
    Ok(rx)
}
