#![forbid(unsafe_code)]

use core::{
    future::Future,
    pin::pin,
    sync::atomic::{AtomicU32, Ordering},
    task::{Context, Poll, Waker},
};

struct Probe {
    ready: AtomicU32,
    calls: AtomicU32,
}
impl Probe {
    const fn new() -> Self {
        Self {
            ready: AtomicU32::new(0),
            calls: AtomicU32::new(0),
        }
    }
    fn step(&self, step: u32) {
        self.calls.fetch_or(step, Ordering::Relaxed);
    }
    fn poll(&self, bit: u32) -> Poll<()> {
        if self.ready.load(Ordering::Relaxed) & bit == 0 {
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }
}

macro_rules! port {
    ($module:ident, $port:ident) => {
        mod $module {
            use super::*;
            use $port::*;
            struct Backend<'a>(&'a Probe);
            struct View<'p, 'a, Id: SlotIdentity> {
                probe: &'a Probe,
                loan: BodyLoan<'p, Id, Backend<'a>>,
            }
            struct Events;
            impl PortPhase for Events {
                type Event = u32;
                type Fault = u32;
                async fn next_event(&mut self) -> Result<Result<u32, EventsLost>, EventEnd<u32>> {
                    core::future::pending().await
                }
            }
            impl<'r, Id: SlotIdentity> SlotOwner<Id> for Backend<'r> {
                type Pools = ();
                type Vacant = ();
                type Queues = Option<&'r Probe>;
                const VACANT_QUEUES: Self::Queues = None;
                type Frontend<'s>
                    = &'r Probe
                where
                    Self: 's,
                    Id: 's;
                type Invocation<'s>
                    = &'r Probe
                where
                    Self: 's,
                    Id: 's;
                fn prepare_queues(
                    &mut self,
                    queues: &mut Self::Queues,
                    mint: Mint<'static, (), Id>,
                ) {
                    mint.into_pools();
                    *queues = Some(self.0);
                }
                fn split<'s>(
                    queues: &'s mut Self::Queues,
                    _: SlotWakes<'s, Id>,
                ) -> (&'r Probe, &'r Probe)
                where
                    Self: 's,
                    Id: 's,
                {
                    let probe = queues.expect("prepared probe");
                    (probe, probe)
                }
                async fn run<'s>(
                    &mut self,
                    probe: &mut Self::Invocation<'s>,
                    retire: SlotRetire<'_, Id, Self>,
                ) -> SlotStopped<Id, Self>
                where
                    Self: 's,
                    Id: 's,
                {
                    {
                        let mut request = pin!(retire.wait());
                        core::future::poll_fn(|cx| {
                            probe.step(4);
                            request.as_mut().poll(cx)
                        })
                        .await;
                    }
                    let proof = retire.requested().await;
                    probe.step(8);
                    proof
                }
                fn abandon<'s>(
                    &mut self,
                    _: Option<&mut Self::Invocation<'s>>,
                    request: Quarantine,
                ) -> Abandon
                where
                    Self: 's,
                    Id: 's,
                {
                    self.0.step(32);
                    Abandon::Stopped(request.stopped())
                }
                fn finish<'s>(&mut self, _: &mut Self::Invocation<'s>)
                where
                    Self: 's,
                    Id: 's,
                {
                    self.0.step(16);
                }
            }
            impl<'r, Id: SlotIdentity> SlotBodyOwner<Id> for Backend<'r> {
                type Body<'p>
                    = View<'p, 'r, Id>
                where
                    Self: 'p,
                    Id: 'p;
                fn lend<'p, 's>(
                    frontend: &'p mut Self::Frontend<'s>,
                    loan: BodyLoan<'p, Id, Self>,
                ) -> Self::Body<'p>
                where
                    Self: 's,
                    Id: 's,
                    's: 'p,
                {
                    View {
                        probe: frontend,
                        loan,
                    }
                }
            }
            impl<'r, Id: SlotIdentity> PortBackend<Id> for Backend<'r> {
                type Event<'p>
                    = u32
                where
                    Self: 'p,
                    Id: 'p;
                type Terminal<'p>
                    = u32
                where
                    Self: 'p,
                    Id: 'p;
                type Domain = ();
                type Fault = u32;
                type Refusal = ();
                type View<'p>
                    = View<'p, 'r, Id>
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
                    = PhaseParts<Events, (), ()>
                where
                    Self: 'p,
                    Id: 'p,
                    'p: 'a;
                type Resources<'p>
                    = ()
                where
                    Self: 'p,
                    Id: 'p;
                type Uninstall = ();
                fn body_parts<'p>(view: Self::Body<'p>) -> InstallParts<'p, Id, Self>
                where
                    Self: 'p,
                    Id: 'p,
                {
                    InstallParts {
                        view,
                        resources: (),
                    }
                }
                async fn enable<'a, 'p>(
                    _: Call,
                    view: &'a mut Self::View<'p>,
                ) -> EnableResult<(), u32>
                where
                    Self: 'p,
                    Id: 'p,
                    'p: 'a,
                {
                    view.probe.step(1);
                    core::future::poll_fn(|_| view.probe.poll(1)).await;
                    Ok(Ok(()))
                }
                async fn disable<'a, 'p, S>(
                    _: Call,
                    view: &'a mut Self::View<'p>,
                    sink: &'a mut S,
                ) -> Result<(), Poisoned<u32>>
                where
                    Self: 'p,
                    Id: 'p,
                    'p: 'a,
                    S: FnMut(u32) -> core::ops::ControlFlow<u32>,
                {
                    view.probe.step(2);
                    core::future::poll_fn(|_| view.probe.poll(2)).await;
                    match sink(view.probe.calls.load(Ordering::Relaxed)) {
                        core::ops::ControlFlow::Continue(()) => Ok(()),
                        core::ops::ControlFlow::Break(terminal) => {
                            view.probe.calls.store(terminal, Ordering::Relaxed);
                            Err(Poisoned { cause: 1 })
                        }
                    }
                }
                async fn now<'a, 'p>(
                    _: Call,
                    _: &'a Self::View<'p>,
                ) -> PortResult<oer_time::RadioInstant<()>, ClockError, u32>
                where
                    Self: 'p,
                    Id: 'p,
                    'p: 'a,
                {
                    Ok(Err(ClockError::Unavailable))
                }
                fn setup<'a, 'p>(_: Call, view: &'a mut Self::View<'p>)
                where
                    Self: 'p,
                    Id: 'p,
                    'p: 'a,
                {
                    let _ = &view.loan;
                }
                fn phase<'a, 'p>(_: Call, _: &'a mut Self::View<'p>) -> Self::Phase<'a, 'p>
                where
                    Self: 'p,
                    Id: 'p,
                    'p: 'a,
                {
                    PhaseParts {
                        events: Events,
                        control: (),
                        clients: (),
                    }
                }
                fn request_uninstall<'s>(
                    _: &mut Self::Frontend<'s>,
                    _: (),
                ) -> Result<(), Poisoned<u32>>
                where
                    Self: 's,
                    Id: 's,
                {
                    Ok(())
                }
                fn shutdown_bound<'s>(
                    frontend: &Self::Frontend<'s>,
                ) -> impl Future<Output = ()> + use<'r, 's, Id>
                where
                    Self: 's,
                    Id: 's,
                {
                    let probe = *frontend;
                    core::future::poll_fn(move |_| probe.poll(4))
                }
            }
            struct Body;
            impl<'a, Id: SlotIdentity> PortBody<Id, Backend<'a>> for Body {
                type Output = ();
                async fn run<'p, 'id>(
                    self,
                    owner: Owner<'p, 'id, Id, Backend<'a>, Disabled>,
                    _: (),
                ) -> Ended<Id, Backend<'a>, ()>
                where
                    Backend<'a>: 'id,
                    Id: 'id,
                    'id: 'p,
                {
                    let owner = match owner.enable().await {
                        Ok(owner) => owner,
                        Err(failed) => return Ended::fail_stop(failed.fail_stop(), ()),
                    };
                    match owner
                        .disable(&mut |event| {
                            core::hint::black_box(event);
                            core::ops::ControlFlow::Continue(())
                        })
                        .await
                    {
                        Ok(owner) => Ended::uninstall(owner, (), ()),
                        Err(failed) => Ended::fail_stop(failed.fail_stop(), ()),
                    }
                }
            }
            #[inline(never)]
            pub fn run(probe: &Probe) -> usize {
                let future = with_local_slot(async |claim| {
                    let mut slot = claim.place();
                    let registered = slot
                        .register(Backend(probe))
                        .ok()
                        .expect("fresh cost probe");
                    let end = LocalExecution
                        .scope(registered, |frontend| run_port(frontend, Body))
                        .await;
                    core::hint::black_box(end);
                });
                let bytes = core::mem::size_of_val(&future);
                let mut future = pin!(future);
                let mut cx = Context::from_waker(Waker::noop());
                let _ = core::hint::black_box(future.as_mut().poll(&mut cx));
                bytes
            }
        }
    };
}

port!(bluetooth, oer_radio_port);
port!(ieee802154, oer_radio_port);

static BLE: Probe = Probe::new();
static IEEE: Probe = Probe::new();

fn main() {
    println!(
        "BLE future: {} bytes; IEEE 802.15.4 future: {} bytes",
        bluetooth::run(&BLE),
        ieee802154::run(&IEEE)
    );
}
