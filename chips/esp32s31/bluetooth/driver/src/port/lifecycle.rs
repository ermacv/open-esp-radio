//! Supervision over the existing radio port contract, without hardware in P1.
use super::*;

impl<Id: SlotIdentity, D: Device, X> SlotOwner<Id> for Backend<Id, D, X> {
    type Pools = (
        Permits<CommandCapacity, COMMANDS, WAITERS>,
        Slots<DtmSessionKind, 1>,
    );
    type Queues = Queues<Id, D, X>;
    const VACANT_QUEUES: Self::Queues = Queues {
        shared: Shared::new(),
        commands: Queue::new(),
        events: Queue::new(),
        failed_request: OnceValue::new(),
        failed_terminal: OnceValue::new(),
        resources: None,
        credits: None,
        outstanding: Cell::new(None),
        timer: None,
        clock: None,
        epoch: 0,
        bound: Duration::ZERO,
    };
    type Vacant = Vacant<D>;
    type Frontend<'s>
        = Frontend<'s, Id, D, X>
    where
        Self: 's,
        Id: 's;
    type Invocation<'s>
        = Invocation<'s, Id, D, X>
    where
        Self: 's,
        Id: 's;
    fn prepare_queues(&mut self, q: &mut Self::Queues, mint: Mint<'static, Self::Pools, Id>) {
        let (capacity, [session]) = mint.into_pools();
        let [credit] = capacity.supply.into_permits();
        let d = self
            .device
            .as_mut()
            .expect("initial device retained until retirement");
        q.shared = Shared::new();
        q.resources = Some((capacity.seats, OwnedSession::new(session, d.epoch())));
        q.credits = Some(Pool::new(credit));
        q.outstanding.set(None);
        q.timer = Some(d.timer());
        q.clock = Some(d.clock());
        q.epoch = d.epoch();
        q.bound = d.shutdown_bound();
    }
    fn split<'s>(
        q: &'s mut Self::Queues,
        wakes: SlotWakes<'s, Id>,
    ) -> (Self::Frontend<'s>, Self::Invocation<'s>)
    where
        Self: 's,
        Id: 's,
    {
        let (commands, driver_commands) = q.commands.split();
        let (driver_events, events) = q.events.split();
        (
            Frontend {
                shared: &q.shared,
                commands,
                events,
                failed_request: &q.failed_request,
                failed_terminal: &q.failed_terminal,
                resources: &mut q.resources,
                credits: q.credits.as_ref().expect("prepared command credit"),
                outstanding: &q.outstanding,
                wakes,
                timer: q.timer.as_ref().expect("prepared timer").clone(),
                clock: q.clock.as_mut().expect("prepared clock endpoint"),
                epoch: q.epoch,
                bound: q.bound,
            },
            Invocation {
                shared: &q.shared,
                commands: driver_commands,
                events: driver_events,
                wakes,
            },
        )
    }
    async fn run<'s>(
        &mut self,
        q: &mut Self::Invocation<'s>,
        retire: SlotRetire<'_, Id, Self>,
    ) -> SlotStopped<Id, Self>
    where
        Self: 's,
        Id: 's,
    {
        loop {
            if q.shared.fault.get().is_some() {
                self.device.as_mut().expect("live device").quarantine();
                // The queued mandatory terminal can still be consumed by P1.
                // A fault closes publication immediately, but does not cancel
                // its body before that terminal or the stream end is observed.
                retire.wait().await;
                retire.request_quarantine();
                core::future::pending::<()>().await;
            }
            if q.shared.enabled.load(Ordering::Acquire) && !q.shared.ready.load(Ordering::Acquire) {
                if let Err(cause) = self.device.as_mut().expect("live device").enable().await {
                    q.shared.poison(cause);
                } else {
                    q.shared.ready.store(true, Ordering::Release);
                }
                q.wakes.wake_frontend();
                continue;
            }
            if self.current.is_none() {
                self.current = q.commands.try_receive().ok();
            }
            if self.current.is_some() {
                let outcome = self.serve(q.shared.draining.load(Ordering::Acquire)).await;
                if let Some(terminal) = self.terminal.take() {
                    if let Err(e) = q.events.try_send(terminal) {
                        self.terminal = Some(e.0);
                        q.shared.poison(Fault::Accounting);
                    }
                    q.wakes.wake_frontend();
                }
                if let Err(cause) = outcome {
                    q.shared.poison(cause);
                    q.wakes.wake_frontend();
                }
                continue;
            }
            if q.shared.draining.load(Ordering::Acquire)
                && !q.shared.drained.load(Ordering::Acquire)
            {
                let d = self.device.as_mut().expect("live device");
                if d.is_running() && d.end().await.is_err() {
                    q.shared.poison(Fault::Hardware);
                    continue;
                }
                q.shared.enabled.store(false, Ordering::Release);
                q.shared.drained.store(true, Ordering::Release);
                q.wakes.wake_frontend();
            }
            if retire.is_requested() {
                if !q.shared.drained.load(Ordering::Acquire)
                    || self.current.is_some()
                    || self.terminal.is_some()
                    || !q.events.is_empty()
                {
                    q.shared.poison(Fault::Accounting);
                    continue;
                }
                if self
                    .device
                    .as_mut()
                    .expect("live device")
                    .shutdown()
                    .await
                    .is_err()
                {
                    q.shared.poison(Fault::Hardware);
                    continue;
                }
                return retire.requested().await;
            }
            if self.device.as_ref().expect("live device").is_running() {
                if let Err(cause) = self.device.as_mut().expect("live device").progress().await {
                    q.shared.poison(cause);
                    q.wakes.wake_frontend();
                }
            } else {
                poll_fn(|cx| {
                    // Bind before checking queue/retire to close the lost-wake gap.
                    let _ = q.shared.notify.poll_wait(cx);
                    if retire.is_requested()
                        || (q.shared.enabled.load(Ordering::Acquire)
                            && !q.shared.ready.load(Ordering::Acquire))
                        || (q.shared.draining.load(Ordering::Acquire)
                            && !q.shared.drained.load(Ordering::Acquire))
                        || !q.commands.is_empty()
                        || q.shared.fault.get().is_some()
                    {
                        return Poll::Ready(());
                    }
                    let wait = retire.wait();
                    let mut wait = core::pin::pin!(wait);
                    wait.as_mut().poll(cx)
                })
                .await;
            }
        }
    }
    fn abandon<'s>(
        &mut self,
        q: Option<&mut Self::Invocation<'s>>,
        authority: Quarantine,
    ) -> Abandon
    where
        Self: 's,
        Id: 's,
    {
        if let Some(q) = q {
            q.shared.poison(Fault::Abandoned);
            q.wakes.wake_frontend();
        }
        if let Some(d) = self.device.as_mut() {
            d.quarantine();
        }
        // P1 cancellation never supplies synchronous DMA/ISR exclusion proof.
        Abandon::Unconfirmed(authority)
    }
    fn finish<'s>(&mut self, _: &mut Self::Invocation<'s>) -> Self::Vacant
    where
        Self: 's,
        Id: 's,
    {
        Vacant {
            device: self
                .device
                .take()
                .expect("successful shutdown retains returned device"),
        }
    }
}
impl<Id: SlotIdentity, D: Device, X> SlotBodyOwner<Id> for Backend<Id, D, X> {
    type Body<'p>
        = InstallParts<'p, Id, Self>
    where
        Self: 'p,
        Id: 'p;
    fn lend<'p, 's>(f: &'p mut Self::Frontend<'s>, loan: BodyLoan<'p, Id, Self>) -> Self::Body<'p>
    where
        Self: 's,
        Id: 's,
        's: 'p,
    {
        let (seats, session) = f.resources.take().expect("one initial body resource set");
        let resources = Resources {
            seats: seats.map(|s| loan.lend_seat(s)),
            session: session.lend(&loan),
        };
        InstallParts {
            resources,
            view: View {
                shared: f.shared,
                commands: f.commands.borrow_local(),
                events: f.events.borrow_local(),
                failed_request: f.failed_request,
                failed_terminal: f.failed_terminal,
                credits: f.credits,
                outstanding: f.outstanding,
                wakes: f.wakes,
                timer: f.timer.clone(),
                clock: RefCell::new(f.clock),
                epoch: f.epoch,
                loan,
            },
        }
    }
}
impl<Id: SlotIdentity, D: Device, X> PortBackend<Id> for Backend<Id, D, X> {
    type Domain = LeRadio;
    type Fault = Fault;
    type Refusal = Refusal;
    type Event<'p>
        = Event<'p, Path<Id>, X>
    where
        Self: 'p,
        Id: 'p;
    type Terminal<'p>
        = Terminal<'p, Path<Id>, X>
    where
        Self: 'p,
        Id: 'p;
    type View<'p>
        = View<'p, Id, D, X>
    where
        Self: 'p,
        Id: 'p;
    type Resources<'p>
        = Resources<'p, Id>
    where
        Self: 'p,
        Id: 'p;
    type Setup<'a, 'p>
        = ()
    where
        Self: 'p,
        Id: 'p,
        'p: 'a;
    type Phase<'a, 'p>
        = Phase<'a, 'p, Id, D, X>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a;
    type Uninstall = ();
    fn body_parts<'p>(body: Self::Body<'p>) -> InstallParts<'p, Id, Self>
    where
        Self: 'p,
        Id: 'p,
    {
        body
    }
    async fn enable<'a, 'p>(_: Call, v: &'a mut Self::View<'p>) -> EnableResult<Refusal, Fault>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
    {
        if let Some(cause) = v.fault() {
            return Err(Poisoned { cause });
        }
        if v.shared.draining.load(Ordering::Acquire) {
            return Ok(Err(Refusal::Unavailable));
        }
        v.shared.enabled.store(true, Ordering::Release);
        v.shared.drained.store(false, Ordering::Release);
        v.shared.notify.notify();
        poll_fn(|cx| {
            v.wakes.register_frontend(cx);
            if let Some(cause) = v.fault() {
                Poll::Ready(Err(Poisoned { cause }))
            } else if v.shared.ready.load(Ordering::Acquire) {
                Poll::Ready(Ok(Ok(())))
            } else {
                Poll::Pending
            }
        })
        .await
    }
    async fn disable<'a, 'p, S>(
        _: Call,
        v: &'a mut Self::View<'p>,
        sink: &'a mut S,
    ) -> Result<(), Poisoned<Fault>>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
        S: FnMut(Self::Terminal<'p>) -> ControlFlow<Self::Terminal<'p>>,
    {
        v.shared.draining.store(true, Ordering::Release);
        v.shared.notify.notify();
        poll_fn(|cx| {
            v.wakes.register_frontend(cx);
            while let Ok(mut event) = v.events.try_receive() {
                let id = event.admitted.id;
                if v.outstanding.get() != Some(id) {
                    assert!(
                        v.failed_terminal.set(event).is_ok(),
                        "one failed terminal reservation"
                    );
                    v.shared.poison(Fault::Accounting);
                    return Poll::Ready(Err(Poisoned {
                        cause: Fault::Accounting,
                    }));
                }
                let credit = event.credit.take();
                if let ControlFlow::Break(terminal) = sink(event.lend(&v.loan)) {
                    let mut event = OwnedTerminal::retain(terminal, &v.loan);
                    event.credit = credit;
                    assert!(
                        v.failed_terminal.set(event).is_ok(),
                        "sole terminal cannot be rejected twice"
                    );
                    v.shared.poison(Fault::RejectedTerminal);
                    return Poll::Ready(Err(Poisoned {
                        cause: Fault::RejectedTerminal,
                    }));
                }
                v.outstanding.set(None);
                if let Some(credit) = credit {
                    v.credits.release(credit);
                }
            }
            if let Some(cause) = v.fault() {
                return Poll::Ready(Err(Poisoned { cause }));
            }
            if v.shared.drained.load(Ordering::Acquire) {
                Poll::Ready(Ok(()))
            } else {
                Poll::Pending
            }
        })
        .await
    }
    async fn now<'a, 'p>(
        _: Call,
        v: &'a Self::View<'p>,
    ) -> PortResult<oer_time::RadioInstant<LeRadio>, ClockError, Fault>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
    {
        if let Some(cause) = v.fault() {
            return Err(Poisoned { cause });
        }
        if !v.shared.ready.load(Ordering::Acquire) {
            return Ok(Err(ClockError::Unavailable));
        }
        let Ok(mut clock) = v.clock.try_borrow_mut() else {
            return Ok(Err(ClockError::Unavailable));
        };
        clock.now().await
    }
    fn setup<'a, 'p>(_: Call, _: &'a mut Self::View<'p>)
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
    {
    }
    fn phase<'a, 'p>(_: Call, v: &'a mut Self::View<'p>) -> Self::Phase<'a, 'p>
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
    {
        Phase { view: v }
    }
    fn request_uninstall<'s>(_: &mut Self::Frontend<'s>, (): ()) -> Result<(), Poisoned<Fault>>
    where
        Self: 's,
        Id: 's,
    {
        Ok(())
    }
    fn shutdown_bound<'s>(f: &Self::Frontend<'s>) -> impl Future<Output = ()> + use<'s, Id, D, X>
    where
        Self: 's,
        Id: 's,
    {
        let timer = f.timer.clone();
        let bound = f.bound;
        async move {
            if let Some(deadline) = timer.now().checked_add(bound) {
                timer.wait_until(deadline).await;
            }
        }
    }
}
