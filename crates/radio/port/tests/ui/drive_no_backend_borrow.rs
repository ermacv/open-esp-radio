#![no_std]
use core::future::{Future, pending, ready};
use oer_radio_port::{
    Call, ClockError, Detached, DriverExit, EnableResult, EventsLost, InstallParts, Mint, Poisoned,
    PortBackend, PortPhase, PortResult, Retire, Retired,
};
use oer_time::RadioInstant;
pub struct Backend;
pub struct Phase;
impl PortPhase for Phase {
    type Event = ();
    type Fault = ();
    fn next_event(&mut self) -> impl Future<Output = Result<Result<(), EventsLost>, oer_radio_port::EventEnd<()>>> {
        pending()
    }
}
impl PortBackend for Backend {
    type Pools = ();
    type Event<'id> = ();
    type Terminal<'id> = ();
    type Domain = ();
    type Fault = ();
    type Refusal = ();
    type Setup<'a, 'id> = () where Self: 'a, 'id: 'a;
    type Phase<'a, 'id>
        = Phase
    where
        'id: 'a;
    type Held<'id> = ();
    type Local<'id> = ();
    type Resources<'id> = ();
    type Driver = ();
    type State = ();
    type Uninstall = ();
    type Uninstalled = ();
    fn enable<'a, 'id>(&'a mut self, _: Call,
        _: &'a Self::Held<'id>,
        _: &'a mut Self::Local<'id>,) -> impl Future<Output = EnableResult<(), ()>>
    where
        'id: 'a,
    {
        ready(Ok(Ok(())))
    }
    fn disable<'a, 'id>(
        &'a mut self,
        _: Call,
        _: &'a Self::Held<'id>,
        _: &'a mut Self::Local<'id>,
        _: &'a mut dyn FnMut(()) -> core::ops::ControlFlow<()>,
    ) -> impl Future<Output = Result<(), Poisoned<()>>> + 'a
    where
        'id: 'a,
    {
        ready(Ok(()))
    }
    fn now(&self, _: Call) -> impl Future<Output = PortResult<RadioInstant<()>, ClockError, ()>> {
        pending()
    }
    fn resources<'id>(&mut self, mint: Mint<'id, ()>) -> InstallParts<'id, Self> {
        mint.into_pools();
        InstallParts {
            held: (),
            local: (),
            resources: (),
        }
    }
    fn setup<'a, 'id>(&'a mut self, _: Call, _: &'a Self::Held<'id>, _: &'a mut Self::Local<'id>) -> Self::Setup<'a, 'id>
    where
        'id: 'a,
    {}
    fn phase<'a, 'id>(
        &'a mut self,
        _: Call,
        _: &'a Self::Held<'id>,
        _: &'a mut Self::Local<'id>,
    ) -> Self::Phase<'a, 'id>
    where
        'id: 'a,
    {
        Phase
    }
    #[cfg(ui_reject)]
    fn drive<'p, 'id>(
        &mut self, // ui-error: E0185
        _: &'p Self::Held<'id>,
        retire: Retire<'p>,
    ) -> impl Future<Output = DriverExit<(), ()>> + use<'p, 'id> {
        pending()
    }
    #[cfg(not(ui_reject))]
    fn drive<'p, 'id>(
        driver: oer_radio_port::DriverLoan<'p, Self::Driver>,
        _: &'p Self::Held<'id>,
        retire: Retire<'p>,
    ) -> impl Future<Output = DriverExit<(), ()>> + use<'p, 'id> {
        async move {
            DriverExit::Retired {
                state: (),
                proof: retire.requested(driver).await,
            }
        }
    }
    fn retire_bound(&self, _: Call) -> impl Future<Output = ()> + use<> {
        pending()
    }
    fn detach<'id>(&mut self, _: oer_radio_port::Detach<'_>, _: &Self::Held<'id>) {}
    fn uninstall<'id>(
        self,
        _: Self::Held<'id>,
        _: Self::Local<'id>,
        _: Retired<'id, (Self::Driver, ())>,
        _: Detached<'id>,
        _: (),
    ) {
    }
    fn abandon(&mut self, call: oer_radio_port::Quarantine) -> oer_radio_port::Abandon {
        oer_radio_port::Abandon::Stopped(call.stopped())
    }
}
