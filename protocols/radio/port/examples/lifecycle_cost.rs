#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

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
            use oer_radio_port::{EventsLost, Mint, Poisoned, PortResult};
            use $port::*;
            struct Backend<'a>(&'a Probe);
            struct Events;
            impl PortPhase for Events {
                type Event = u32;
                type Fault = u32;
                async fn next_event(
                    &mut self,
                ) -> Result<Result<u32, EventsLost>, oer_radio_port::EventEnd<u32>> {
                    core::future::pending().await
                }
            }
            impl<'r> PortBackend for Backend<'r> {
                type Pools = ();
                type Event<'id> = u32;
                type Terminal<'id> = u32;
                type Driver = &'r Probe;
                type Local<'id> = ();
                type Domain = ();
                type Fault = u32;
                type Refusal = ();
                type Setup<'a, 'id>
                    = ()
                where
                    Self: 'a,
                    'id: 'a;
                type Phase<'a, 'id>
                    = PhaseParts<Events, (), ()>
                where
                    Self: 'a,
                    'id: 'a;
                type Resources<'id> = ();
                type Held<'id> = ();
                type State = ();
                type Uninstall = ();
                type Uninstalled = ();
                async fn enable<'a, 'id>(
                    &'a mut self,
                    _: Call,
                    _: &'a Self::Held<'id>,
                    _: &'a mut Self::Local<'id>,
                ) -> EnableResult<(), u32>
                where
                    'id: 'a,
                {
                    self.0.step(1);
                    core::future::poll_fn(|_| self.0.poll(1)).await;
                    Ok(Ok(()))
                }
                async fn disable<'a, 'id>(
                    &'a mut self,
                    _: Call,
                    _: &'a Self::Held<'id>,
                    _: &'a mut Self::Local<'id>,
                    sink: &'a mut dyn FnMut(u32) -> core::ops::ControlFlow<u32>,
                ) -> Result<(), Poisoned<u32>>
                where
                    'id: 'a,
                {
                    self.0.step(2);
                    core::future::poll_fn(|_| self.0.poll(2)).await;
                    match sink(self.0.calls.load(Ordering::Relaxed)) {
                        core::ops::ControlFlow::Continue(()) => Ok(()),
                        core::ops::ControlFlow::Break(terminal) => {
                            self.0.calls.store(terminal, Ordering::Relaxed);
                            Err(Poisoned { cause: 1 })
                        }
                    }
                }
                async fn now(
                    &self,
                    _: Call,
                ) -> PortResult<oer_time::RadioInstant<()>, ClockError, u32> {
                    Ok(Err(ClockError::Unavailable))
                }
                fn resources<'id>(&mut self, mint: Mint<'id, ()>) -> InstallParts<'id, Self> {
                    mint.into_pools();
                    InstallParts {
                        held: (),
                        local: (),
                        resources: (),
                    }
                }
                fn setup<'a, 'id>(
                    &'a mut self,
                    _: Call,
                    _: &'a Self::Held<'id>,
                    _: &'a mut Self::Local<'id>,
                ) -> Self::Setup<'a, 'id>
                where
                    'id: 'a,
                {
                }
                fn phase<'a, 'id>(
                    &'a mut self,
                    _: Call,
                    _: &'a Self::Held<'id>,
                    _: &'a mut Self::Local<'id>,
                ) -> Self::Phase<'a, 'id>
                where
                    'id: 'a,
                {
                    PhaseParts {
                        events: Events,
                        control: (),
                        clients: (),
                    }
                }
                async fn drive<'p, 'id>(
                    probe: oer_radio_port::DriverLoan<'p, Self::Driver>,
                    _: &'p Self::Held<'id>,
                    retire: Retire<'p>,
                ) -> DriverExit<(), u32> {
                    {
                        let mut request = pin!(retire.wait());
                        core::future::poll_fn(|cx| {
                            probe.step(4);
                            request.as_mut().poll(cx)
                        })
                        .await;
                    }
                    DriverExit::Retired {
                        state: (),
                        proof: retire.requested(probe).await,
                    }
                }

                fn retire_bound(&self, _: Call) -> impl Future<Output = ()> + use<'r> {
                    let probe = self.0;
                    core::future::poll_fn(move |_| probe.poll(4))
                }
                fn detach<'id>(&mut self, _: oer_radio_port::Detach<'_>, _: &Self::Held<'id>) {
                    self.0.step(8);
                }
                fn uninstall<'id>(
                    self,
                    _: Self::Held<'id>,
                    _: Self::Local<'id>,
                    _: Retired<'id, (Self::Driver, ())>,
                    _: Detached<'id>,
                    _: (),
                ) {
                    self.0.step(16);
                }
                fn abandon(&mut self, call: oer_radio_port::Quarantine) -> oer_radio_port::Abandon {
                    self.0.step(32);
                    oer_radio_port::Abandon::Stopped(call.stopped())
                }
            }
            struct Body;
            impl<'a> PortBody<Backend<'a>> for Body {
                type Output = ();

                async fn run<'p, 'id>(
                    self,
                    owner: Owner<'p, 'id, Backend<'a>, Disabled>,
                    _: (),
                ) -> Ended<'id, Backend<'a>, ()> {
                    let owner = match owner.enable().await {
                        Ok(owner) => owner,
                        Err(failed) => {
                            return oer_radio_port::Ended::fail_stopped(failed.fail_stop(), ());
                        }
                    };
                    match owner
                        .disable(&mut |event| {
                            core::hint::black_box(event);
                            core::ops::ControlFlow::Continue(())
                        })
                        .await
                    {
                        Ok(owner) => oer_radio_port::Ended::uninstall(owner, (), ()),
                        Err(failed) => oer_radio_port::Ended::fail_stopped(failed.fail_stop(), ()),
                    }
                }
            }
            #[inline(never)]
            pub fn run(probe: &Probe) -> usize {
                let future = run_port(Installed::new(Backend(probe), probe), Body);
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

// SAFETY: this standalone measurement ELF supplies its sole entry symbol.
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    core::hint::black_box(bluetooth::run(&BLE));
    core::hint::black_box(ieee802154::run(&IEEE));
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(not(target_os = "none"))]
fn main() {
    println!(
        "BLE future: {} bytes; IEEE 802.15.4 future: {} bytes",
        bluetooth::run(&BLE),
        ieee802154::run(&IEEE)
    );
}
