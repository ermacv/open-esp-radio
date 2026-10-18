extern crate std;

use super::*;
use oer_radio_port::{Mint, Slot};
use oer_time::RadioInstant;

/// The test port's clock domain.
enum Port {}

/// A backend in one cell: its admitted work by slot and client, its
/// queued events, and what its next enable does.
#[derive(Default)]
struct Model {
    running: core::cell::RefCell<std::vec::Vec<(u32, u8)>>,
    events: core::cell::RefCell<std::collections::VecDeque<u32>>,
    next_slot: core::cell::Cell<u32>,
    refuse_enable: core::cell::Cell<bool>,
    poisoned: core::cell::Cell<bool>,
    /// `disable` answers `Pending` once before it ends.
    slow_disable: core::cell::Cell<bool>,
    lifecycle: core::cell::RefCell<std::vec::Vec<&'static str>>,
    /// The install's held part as the last split or disable left it.
    held: core::cell::Cell<(u32, u32)>,
    /// How often `run_port` polled the driver.
    driver_polls: core::cell::Cell<u32>,
    /// Work posted to the driver (an interrupt status), and how much of it
    /// the driver served.
    posted: core::cell::Cell<u32>,
    served: core::cell::Cell<u32>,
    /// The driver ignores the retire request.
    stubborn_driver: core::cell::Cell<bool>,
    /// The backend's retire bound has passed.
    bound_passed: core::cell::Cell<bool>,
}

/// The model's capabilities marker.
struct ModelCaps;

/// The model's backend; only `run_port` wraps it.
struct Backend<'m>(&'m Model);

struct Events<'m>(&'m Model);
struct Control(());
struct Client<'m> {
    model: &'m Model,
    interface: u8,
}
struct Setup(());

/// One admitted operation's token: only the model mints it.
struct Token(u32);

impl RadioWork for Token {
    type Id = u32;

    fn id(&self) -> u32 {
        self.0
    }
}

impl Client<'_> {
    fn admit(&self) -> Work<'_, Self> {
        let slot = self.model.next_slot.get();
        self.model.next_slot.set(slot + 1);
        self.model.running.borrow_mut().push((slot, self.interface));
        Work::new(self, Token(slot))
    }
}

/// End one admitted operation by its slot; its terminal is queued.
fn end(model: &Model, slot: u32) -> Result<(), CancelError> {
    let mut running = model.running.borrow_mut();
    let index = running
        .iter()
        .position(|(running, _)| *running == slot)
        .ok_or(CancelError::NotRunning)?;
    running.remove(index);
    model.events.borrow_mut().push_back(slot);
    Ok(())
}

impl<'m> PortBackend for Backend<'m> {
    type Event = u32;
    type Domain = Port;
    type Fault = ();
    type Refusal = &'static str;
    type Setup<'a, 'id>
        = Setup
    where
        Self: 'a;
    type Events<'a, 'id>
        = Events<'m>
    where
        Self: 'a,
        'id: 'a;
    type Control<'a, 'id>
        = Control
    where
        Self: 'a,
        'id: 'a;
    type Clients<'a, 'id>
        = [Client<'m>; 2]
    where
        Self: 'a,
        'id: 'a;
    type Resources<'id> = [Slot<'id, ModelCaps>; 2];
    /// The phases lent and the disables run over the install's held part.
    type Held<'id> = core::cell::Cell<(u32, u32)>;
    /// The parts reach the state through the model, so the driver owns
    /// none.
    type State = ();
    type Uninstall = ();
    type Uninstalled = &'m Model;

    fn enable(&mut self) -> impl Future<Output = EnableResult<&'static str, ()>> + '_ {
        let model = self.0;
        core::future::ready(if model.poisoned.get() {
            Err(Poisoned { cause: () })
        } else if model.refuse_enable.get() {
            Ok(Err("the radio refused the channel"))
        } else {
            model.lifecycle.borrow_mut().push("enable");
            Ok(Ok(()))
        })
    }

    fn disable<'a, 'id>(
        &'a mut self,
        held: &'a Self::Held<'id>,
        sink: &'a mut dyn FnMut(u32),
    ) -> impl Future<Output = Result<(), Poisoned<()>>> + 'a
    where
        'id: 'a,
    {
        let model = self.0;
        async move {
            let (phases, disables) = held.get();
            held.set((phases, disables + 1));
            model.held.set(held.get());
            if model.slow_disable.get() {
                let mut waited = false;
                core::future::poll_fn(|_| {
                    if waited {
                        core::task::Poll::Ready(())
                    } else {
                        waited = true;
                        core::task::Poll::Pending
                    }
                })
                .await;
            }
            if model.poisoned.get() {
                Err(Poisoned { cause: () })
            } else {
                model.lifecycle.borrow_mut().push("disable");
                // Disable ends every admitted work; its terminal goes to the
                // sink before the port is disabled.
                let slots: std::vec::Vec<u32> = model
                    .running
                    .borrow()
                    .iter()
                    .map(|(slot, _)| *slot)
                    .collect();
                for slot in slots {
                    let _ = end(model, slot);
                }
                while let Some(event) = model.events.borrow_mut().pop_front() {
                    sink(event);
                }
                Ok(())
            }
        }
    }

    fn now(&self) -> impl Future<Output = PortResult<RadioInstant<Port>, ClockError, ()>> + '_ {
        core::future::ready(Ok(Err(ClockError::Unavailable)))
    }

    fn resources<'id>(
        &mut self,
        mut mint: Mint<'id>,
    ) -> (core::cell::Cell<(u32, u32)>, [Slot<'id, ModelCaps>; 2]) {
        (core::cell::Cell::new((0, 0)), mint.slots())
    }

    fn setup<'a, 'id>(&'a mut self) -> Lent<'a, Self::Setup<'a, 'id>> {
        Lent::new(Setup(()))
    }

    fn split<'a, 'id>(
        &'a mut self,
        held: &'a Self::Held<'id>,
    ) -> PhaseParts<'a, Self::Events<'a, 'id>, Self::Control<'a, 'id>, Self::Clients<'a, 'id>>
    where
        'id: 'a,
    {
        let model = self.0;
        let (phases, disables) = held.get();
        held.set((phases + 1, disables));
        model.held.set(held.get());
        let client = |interface| Client { model, interface };
        PhaseParts {
            events: Lent::new(Events(model)),
            control: Lent::new(Control(())),
            clients: Lent::new([client(0), client(1)]),
        }
    }

    fn drive<'p, 'id>(
        &mut self,
        _: &'p Self::Held<'id>,
        retire: Retire<'p>,
    ) -> impl Future<Output = ()> + use<'p, 'id, 'm> {
        let model = self.0;
        // It serves what was posted at every poll, and retires at the first
        // poll after the request unless it is stubborn.
        core::future::poll_fn(move |_| {
            model.driver_polls.set(model.driver_polls.get() + 1);
            model.served.set(model.posted.get());
            if retire.is_requested() && !model.stubborn_driver.get() {
                model.lifecycle.borrow_mut().push("retire");
                core::task::Poll::Ready(())
            } else {
                core::task::Poll::Pending
            }
        })
    }

    fn retire_bound(&self) -> impl Future<Output = ()> + '_ {
        let model = self.0;
        core::future::poll_fn(move |_| {
            if model.bound_passed.get() {
                core::task::Poll::Ready(())
            } else {
                core::task::Poll::Pending
            }
        })
    }

    fn detach<'id>(&mut self, _: &Self::Held<'id>) {
        self.0.lifecycle.borrow_mut().push("detach");
    }

    fn uninstall<'id>(self, _: Self::Held<'id>, (): (), (): ()) -> &'m Model {
        self.0.lifecycle.borrow_mut().push("uninstall");
        self.0
    }

    fn abandon(&mut self) {
        self.0.lifecycle.borrow_mut().push("abandon");
    }
}

impl RadioPortEvents for Events<'_> {
    type Event = u32;
    type Fault = ();

    fn next_event(&mut self) -> impl Future<Output = PortResult<u32, EventsLost, ()>> + '_ {
        core::future::ready(match self.0.events.borrow_mut().pop_front() {
            Some(event) => Ok(Ok(event)),
            None => Ok(Err(EventsLost)),
        })
    }
}

impl RadioPortClient for Client<'_> {
    type Token = Token;
    type Domain = Port;
    type Fault = ();

    fn now(&self) -> impl Future<Output = PortResult<RadioInstant<Port>, ClockError, ()>> + '_ {
        core::future::ready(Ok(Err(ClockError::Unavailable)))
    }

    fn end(&self, token: Token) -> impl Future<Output = PortResult<(), CancelError, ()>> + '_ {
        core::future::ready(Ok(end(self.model, token.0)))
    }
}

/// The value of a future that is ready at its first poll.
fn now_ready<F: Future>(future: F) -> F::Output {
    let mut future = core::pin::pin!(future);
    match future.as_mut().poll(&mut core::task::Context::from_waker(
        core::task::Waker::noop(),
    )) {
        core::task::Poll::Ready(value) => value,
        core::task::Poll::Pending => panic!("the model answers at once"),
    }
}

/// A body that runs one phase with work left in flight, then disables and
/// uninstalls, recording the leftover terminals.
struct OnePhase<'t> {
    leftovers: &'t core::cell::RefCell<std::vec::Vec<u32>>,
    cancelled: &'t core::cell::Cell<Option<u32>>,
}

impl<'m> PortBody<Backend<'m>> for OnePhase<'_> {
    async fn run<'id>(
        self,
        mut owner: Owner<'_, 'id, Backend<'m>, Disabled>,
        _: [Slot<'id, ModelCaps>; 2],
    ) -> Ended<'id, Backend<'m>> {
        let _ = owner.setup();
        let mut enabled = match owner.enable().await {
            Ok(enabled) => enabled,
            Err(failed) => return failed.fail_stop(),
        };
        {
            let PhaseParts {
                mut events,
                clients,
                ..
            } = enabled.split();
            let [station, access_point] = &*clients;
            let joined = station.admit();
            let joined_id = joined.id();
            let _served = access_point.admit();
            // A client ends its own work by its token, once.
            assert_eq!(joined.cancel().await, Ok(Ok(())));
            assert_eq!(events.next_event().await, Ok(Ok(joined_id)));
            self.cancelled.set(Some(joined_id));
        }
        // The access point's work is left in flight: `disable` ends it and
        // hands its terminal to the sink.
        let leftovers = self.leftovers;
        match enabled
            .disable(&mut |event| leftovers.borrow_mut().push(event))
            .await
        {
            Ok(disabled) => disabled.uninstall(()),
            Err(poisoned) => poisoned.fail_stop(),
        }
    }
}

#[test]
fn a_port_runs_a_phase_between_its_lifecycle_commands_and_ends_through_its_uninstall() {
    let model = Model::default();
    let leftovers = core::cell::RefCell::default();
    let cancelled = core::cell::Cell::default();
    let ended = now_ready(run_port(
        Backend(&model),
        OnePhase {
            leftovers: &leftovers,
            cancelled: &cancelled,
        },
    ));
    assert_eq!(cancelled.get(), Some(0));
    assert_eq!(*leftovers.borrow(), [1]);
    assert!(core::ptr::eq(ended.unwrap(), &model));
    assert_eq!(
        *model.lifecycle.borrow(),
        ["enable", "disable", "retire", "detach", "uninstall"]
    );
}

/// A body that runs two phases of one install.
struct TwoPhases;

impl<'m> PortBody<Backend<'m>> for TwoPhases {
    async fn run<'id>(
        self,
        mut owner: Owner<'_, 'id, Backend<'m>, Disabled>,
        _: [Slot<'id, ModelCaps>; 2],
    ) -> Ended<'id, Backend<'m>> {
        for _ in 0..2 {
            let Ok(mut enabled) = owner.enable().await else {
                panic!("the model enables");
            };
            let _ = enabled.split();
            owner = match enabled.disable(&mut |_| {}).await {
                Ok(disabled) => disabled,
                Err(poisoned) => return poisoned.fail_stop(),
            };
        }
        owner.uninstall(())
    }
}

#[test]
fn the_backends_held_part_of_the_install_reaches_every_phase_and_disable() {
    let model = Model::default();
    assert!(now_ready(run_port(Backend(&model), TwoPhases)).is_some());
    // One held part travelled through both transitions of both phases.
    assert_eq!(model.held.get(), (2, 2));
}

/// A body that enables once and reports the refusal it met.
struct EnableOnce<'t> {
    refusal: &'t core::cell::Cell<Option<&'static str>>,
}

impl<'m> PortBody<Backend<'m>> for EnableOnce<'_> {
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, Backend<'m>, Disabled>,
        _: [Slot<'id, ModelCaps>; 2],
    ) -> Ended<'id, Backend<'m>> {
        match owner.enable().await {
            Ok(enabled) => match enabled.disable(&mut |_| {}).await {
                Ok(disabled) => disabled.uninstall(()),
                Err(poisoned) => poisoned.fail_stop(),
            },
            // The radio refused: the owner is disabled and uninstalls.
            Err(Failed::Refused(owner, refusal)) => {
                self.refusal.set(Some(refusal));
                owner.uninstall(())
            }
            Err(Failed::Poisoned(poisoned)) => poisoned.fail_stop(),
        }
    }
}

#[test]
fn a_refused_enable_returns_the_disabled_owner_with_the_worlds_cause() {
    let model = Model::default();
    model.refuse_enable.set(true);
    let refusal = core::cell::Cell::default();
    let ended = now_ready(run_port(Backend(&model), EnableOnce { refusal: &refusal }));
    assert_eq!(refusal.get(), Some("the radio refused the channel"));
    assert!(ended.is_some());
}

#[test]
fn a_poisoned_backend_ends_through_a_fail_stop_without_its_uninstall() {
    let model = Model::default();
    model.poisoned.set(true);
    let refusal = core::cell::Cell::default();
    let ended = now_ready(run_port(Backend(&model), EnableOnce { refusal: &refusal }));
    assert!(ended.is_none());
    assert_eq!(*model.lifecycle.borrow(), ["abandon"]);
}

/// A body whose `enable` future is dropped before it runs: the owner inside
/// it is dropped too.
struct DropTheEnable;

impl<'m> PortBody<Backend<'m>> for DropTheEnable {
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, Backend<'m>, Disabled>,
        _: [Slot<'id, ModelCaps>; 2],
    ) -> Ended<'id, Backend<'m>> {
        drop(owner.enable());
        panic!("the dropped owner's safety net asserts first")
    }
}

#[test]
fn a_dropped_owner_abandons_its_backend_and_asserts_in_debug() {
    let model = Model::default();
    let dropped = std::panic::catch_unwind(core::panic::AssertUnwindSafe(|| {
        now_ready(run_port(Backend(&model), DropTheEnable))
    }));
    assert!(dropped.is_err());
    // The safety net stopped the backend before it asserted.
    assert_eq!(*model.lifecycle.borrow(), ["abandon"]);
}

/// A body that drops its `disable` future after its first poll, while the
/// backend still waits: the owner inside it is dropped mid-wait.
struct DropTheDisable;

impl<'m> PortBody<Backend<'m>> for DropTheDisable {
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, Backend<'m>, Disabled>,
        _: [Slot<'id, ModelCaps>; 2],
    ) -> Ended<'id, Backend<'m>> {
        let Ok(enabled) = owner.enable().await else {
            panic!("the model enables");
        };
        {
            let mut sink = |_| {};
            let mut disabling = core::pin::pin!(enabled.disable(&mut sink));
            let first = disabling
                .as_mut()
                .poll(&mut core::task::Context::from_waker(
                    core::task::Waker::noop(),
                ));
            assert!(first.is_pending());
        }
        panic!("the dropped owner's safety net asserts first")
    }
}

#[test]
fn an_owner_dropped_while_its_disable_waits_abandons_its_backend() {
    let model = Model::default();
    model.slow_disable.set(true);
    let dropped = std::panic::catch_unwind(core::panic::AssertUnwindSafe(|| {
        now_ready(run_port(Backend(&model), DropTheDisable))
    }));
    assert!(dropped.is_err());
    // The owner held the backend across the wait: its safety net stopped it.
    assert_eq!(*model.lifecycle.borrow(), ["enable", "abandon"]);
}

/// A body whose backend is poisoned between `enable` and `disable`: the
/// disable meets the poison, and the poisoned owner uninstalls.
struct PoisonBetween<'t>(&'t Model);

impl<'m> PortBody<Backend<'m>> for PoisonBetween<'_> {
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, Backend<'m>, Disabled>,
        _: [Slot<'id, ModelCaps>; 2],
    ) -> Ended<'id, Backend<'m>> {
        let Ok(enabled) = owner.enable().await else {
            panic!("the model enables");
        };
        // The world poisons the backend while the port runs.
        self.0.poisoned.set(true);
        let Err(poisoned) = enabled.disable(&mut |_| {}).await else {
            panic!("a poisoned model fails its disable");
        };
        poisoned.cause();
        poisoned.uninstall(())
    }
}

#[test]
fn a_poisoned_disable_leaves_through_the_poisoned_owners_uninstall() {
    let model = Model::default();
    let ended = now_ready(run_port(Backend(&model), PoisonBetween(&model)));
    assert!(core::ptr::eq(ended.unwrap(), &model));
    assert_eq!(
        *model.lifecycle.borrow(),
        ["enable", "retire", "detach", "uninstall"]
    );
}

/// A body that fail-stops after the radio refused its `Enable`.
struct FailStopRefused;

impl<'m> PortBody<Backend<'m>> for FailStopRefused {
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, Backend<'m>, Disabled>,
        _: [Slot<'id, ModelCaps>; 2],
    ) -> Ended<'id, Backend<'m>> {
        match owner.enable().await {
            Ok(_) => panic!("the model refuses"),
            Err(failed) => failed.fail_stop(),
        }
    }
}

#[test]
fn a_refused_enable_can_fail_stop_without_an_uninstall() {
    let model = Model::default();
    model.refuse_enable.set(true);
    let ended = now_ready(run_port(Backend(&model), FailStopRefused));
    assert!(ended.is_none());
    assert_eq!(*model.lifecycle.borrow(), ["abandon"]);
}

/// Poll `future` once with a waker that does nothing.
fn poll_once<F: Future>(future: core::pin::Pin<&mut F>) -> core::task::Poll<F::Output> {
    future.poll(&mut core::task::Context::from_waker(
        core::task::Waker::noop(),
    ))
}

/// A future of the world that the test makes ready: it stands for a
/// foreign future (another task's reply, a permit) the body awaits.
fn foreign(ready: &core::cell::Cell<bool>) -> impl Future<Output = ()> + '_ {
    core::future::poll_fn(|_| {
        if ready.get() {
            core::task::Poll::Ready(())
        } else {
            core::task::Poll::Pending
        }
    })
}

/// A body that awaits a foreign future while disabled, then uninstalls.
struct Stalled<'t>(&'t core::cell::Cell<bool>);

impl<'m> PortBody<Backend<'m>> for Stalled<'_> {
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, Backend<'m>, Disabled>,
        _: [Slot<'id, ModelCaps>; 2],
    ) -> Ended<'id, Backend<'m>> {
        foreign(self.0).await;
        owner.uninstall(())
    }
}

#[test]
fn a_body_awaiting_a_foreign_future_does_not_stall_the_driver() {
    let model = Model::default();
    let released = core::cell::Cell::new(false);
    let mut port = core::pin::pin!(run_port(Backend(&model), Stalled(&released)));
    assert!(poll_once(port.as_mut()).is_pending());
    // The interrupt posts work while the body waits on the world: the
    // driver serves it at the task's next wake.
    model.posted.set(1);
    assert!(poll_once(port.as_mut()).is_pending());
    assert_eq!(model.served.get(), 1);
    model.posted.set(2);
    assert!(poll_once(port.as_mut()).is_pending());
    assert_eq!(model.served.get(), 2);
    released.set(true);
    let core::task::Poll::Ready(ended) = poll_once(port.as_mut()) else {
        panic!("the released body uninstalls");
    };
    assert!(core::ptr::eq(ended.unwrap(), &model));
}

/// A body that uninstalls the disabled port at once.
struct UninstallAtOnce;

impl<'m> PortBody<Backend<'m>> for UninstallAtOnce {
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, Backend<'m>, Disabled>,
        _: [Slot<'id, ModelCaps>; 2],
    ) -> Ended<'id, Backend<'m>> {
        owner.uninstall(())
    }
}

#[test]
fn an_uninstall_retires_the_driver_then_detaches_then_takes_the_backend_apart() {
    let model = Model::default();
    let ended = now_ready(run_port(Backend(&model), UninstallAtOnce));
    assert!(core::ptr::eq(ended.unwrap(), &model));
    assert_eq!(*model.lifecycle.borrow(), ["retire", "detach", "uninstall"]);
}

#[test]
fn a_driver_that_misses_its_retire_bound_fail_stops_the_port() {
    let model = Model::default();
    model.stubborn_driver.set(true);
    let mut port = core::pin::pin!(run_port(Backend(&model), UninstallAtOnce));
    // Within the bound, `run_port` waits for the driver.
    assert!(poll_once(port.as_mut()).is_pending());
    assert!(model.lifecycle.borrow().is_empty());
    model.bound_passed.set(true);
    let core::task::Poll::Ready(ended) = poll_once(port.as_mut()) else {
        panic!("a passed bound ends the port");
    };
    // No detach and no uninstall: the hardware stays installed.
    assert!(ended.is_none());
    assert_eq!(*model.lifecycle.borrow(), ["abandon"]);
}

/// A body that fail-stops after a wait, then yields once more before it
/// returns its `Ended`.
struct FailStopThenYield<'t>(&'t core::cell::Cell<bool>);

impl<'m> PortBody<Backend<'m>> for FailStopThenYield<'_> {
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, Backend<'m>, Disabled>,
        _: [Slot<'id, ModelCaps>; 2],
    ) -> Ended<'id, Backend<'m>> {
        foreign(self.0).await;
        let ended = owner.fail_stop();
        let mut yielded = false;
        core::future::poll_fn(|_| {
            if yielded {
                core::task::Poll::Ready(())
            } else {
                yielded = true;
                core::task::Poll::Pending
            }
        })
        .await;
        ended
    }
}

#[test]
fn a_fail_stop_halts_the_driver_at_its_next_poll() {
    let model = Model::default();
    let fail = core::cell::Cell::new(false);
    let mut port = core::pin::pin!(run_port(Backend(&model), FailStopThenYield(&fail)));
    assert!(poll_once(port.as_mut()).is_pending());
    let before = model.driver_polls.get();
    assert_eq!(before, 1);
    fail.set(true);
    // The body abandons the backend and yields: the driver is not polled.
    assert!(poll_once(port.as_mut()).is_pending());
    assert_eq!(*model.lifecycle.borrow(), ["abandon"]);
    assert_eq!(model.driver_polls.get(), before);
    let core::task::Poll::Ready(ended) = poll_once(port.as_mut()) else {
        panic!("the body returns its fail-stop");
    };
    assert!(ended.is_none());
    assert_eq!(model.driver_polls.get(), before);
}

/// A body that drops its owner, survives the safety net's debug assertion,
/// and waits on.
struct DropTheOwnerAndWait;

impl<'m> PortBody<Backend<'m>> for DropTheOwnerAndWait {
    async fn run<'id>(
        self,
        owner: Owner<'_, 'id, Backend<'m>, Disabled>,
        _: [Slot<'id, ModelCaps>; 2],
    ) -> Ended<'id, Backend<'m>> {
        let dropped = std::panic::catch_unwind(core::panic::AssertUnwindSafe(|| drop(owner)));
        assert_eq!(dropped.is_err(), cfg!(debug_assertions));
        foreign(&core::cell::Cell::new(false)).await;
        unreachable!("the test drops the port first")
    }
}

#[test]
fn a_dropped_owner_halts_the_driver_at_its_next_poll() {
    let model = Model::default();
    let mut port = core::pin::pin!(run_port(Backend(&model), DropTheOwnerAndWait));
    assert!(poll_once(port.as_mut()).is_pending());
    assert!(poll_once(port.as_mut()).is_pending());
    assert_eq!(*model.lifecycle.borrow(), ["abandon"]);
    assert_eq!(model.driver_polls.get(), 0);
}

#[test]
fn a_port_dropped_while_its_driver_retires_abandons_its_backend() {
    let model = Model::default();
    model.stubborn_driver.set(true);
    let dropped = std::panic::catch_unwind(core::panic::AssertUnwindSafe(|| {
        let mut port = core::pin::pin!(run_port(Backend(&model), UninstallAtOnce));
        // The body asked for the uninstall; `run_port` waits for the driver.
        assert!(poll_once(port.as_mut()).is_pending());
        assert!(model.lifecycle.borrow().is_empty());
    }));
    // The safety net stopped the backend before it asserted in debug.
    assert_eq!(dropped.is_err(), cfg!(debug_assertions));
    assert_eq!(*model.lifecycle.borrow(), ["abandon"]);
}
