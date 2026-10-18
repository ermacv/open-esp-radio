#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! The lifecycle of the IEEE 802.15.4 radio port: the one authority over an installed
//! port, as types. The IEEE 802.15.4 block owns it; the vocabulary it shares with
//! the other radio blocks (the brand and [`Mint`], slots, permits and
//! seats, [`Poisoned`], [`PortResult`], [`EventsLost`], the clock
//! relation) stays in `oer-radio-port`.
//!
//! Every rule a caller controls is a type here, so breaking it does not
//! compile ([ownership and refusals]); only what the world controls (the
//! air, time, the hardware, a peer, the arbiter) stays a typed outcome.
//!
//! # Shape
//!
//! A port lives inside one body ([`PortBody`]) that [`run_port`] runs over a
//! backend ([`PortBackend`]), joined with the backend's driver
//! ([below](#the-driver)); no caller ever holds the backend. The body
//! receives an [`Owner`] in the [`Disabled`] state and must return
//! [`Ended`], which only the uninstalls and the `fail_stop`s construct, so
//! a body that drops or forgets its owner does not compile. The owner's
//! state is its type (`'p` is its borrow of the install `run_port` keeps,
//! `'id` the install's brand):
//!
//! ```text
//! Owner<'p, 'id, B, Disabled> ── enable(self).await ──▶ Ok(Owner<'p, 'id, B, Enabled>)
//!        ▲                                    Err(Failed::Refused(Owner<'p, 'id, B, Disabled>, refusal))
//!        │                                    Err(Failed::Poisoned(PoisonedOwner<'p, 'id, B>))
//!        └── Ok ── disable(self, sink).await ── Owner<'p, 'id, B, Enabled>
//!                  Err(PoisonedOwner<'p, 'id, B>)
//! uninstall / fail_stop ──▶ Ended<'id, B>
//! ```
//!
//! - [`Owner<'p, 'id, B, Disabled>`](Owner) lends the protocol's disabled-only
//!   settings ([`Owner::setup`]: a permanent channel move, interface
//!   configuration, port-lifetime resources such as keys and agreements,
//!   whose tokens the owner keeps and lends into each phase), and is the
//!   only state that asks for the uninstall.
//! - [`Owner<'p, 'id, B, Enabled>`](Owner) lends one running phase at a time
//!   ([`Owner::split`]): the phase's one event consumer, its control and
//!   its interface-scoped clients, each a [`Lent`] of the owner's borrow.
//! - The transitions take the owner by value and are bounded by contract:
//!   each ends within its backend's bound, or as poisoned. An unbounded wait
//!   (the radio arbiter's lease) is a separate, cancel-safe step before the
//!   transition, which takes its result. `disable` hands every leftover
//!   terminal of a phase's work, with what it returns, to its `sink`, and
//!   ends only when every permit is back.
//! - [`PoisonedOwner`] is the one exit of a poisoned backend: its cause,
//!   its uninstall or its fail-stop.
//! - An uninstall only asks: the body returns its [`Ended`], and
//!   [`run_port`] takes the backend apart ([below](#the-driver)).
//!
//! A command never meets another command, a phase never meets a lifecycle
//! command, a disabled-only setting never meets an enabled port and an
//! enabled port is never uninstalled: there is no lifecycle refusal and no
//! "port enabled", "port disabled" or "port running" refusal. A refused
//! `Enable` (the radio refused it) is the world's outcome and returns the
//! disabled owner.
//!
//! # What types guarantee
//!
//! | Guarantee | Enforced by |
//! | --- | --- |
//! | A port ends through an uninstall or a fail-stop | the body returns [`Ended`], which only they construct |
//! | A caller holds an owner only inside a body | [`Owner`] has no public constructor |
//! | A disabled port lends no phase; an enabled one no disabled-only setting and no uninstall | the state parameter |
//! | One lifecycle command at a time; no phase or setting while one runs | the transitions take the owner by value |
//! | A phase's parts end before any owner call or transition | [`Lent`], invariant in the owner's borrow |
//! | One running phase, one event consumer | `split(&mut self)` |
//! | A caller reaches the backend only through the owner and its parts | the owner's private field |
//! | Admitted work names only its own phase's work | [`Work`] borrows its client |
//! | Admitted work ends only through the client that admitted it | [`Work`] holds that client and its token, which nothing takes out |
//! | A part or a token is not taken out of its loan | `Lent` and `Work` give nothing away; see below |
//!
//! Each line has a compile-fail test below, pinned to its error code, with a
//! compiling twin.
//!
//! # What each backend proves
//!
//! The language cannot express three rules, so every backend proves them of
//! its own types:
//!
//! - its phase parts and every token it mints (admitted work, permits,
//!   seats, leases, installed keys and agreements, validated values) are not
//!   `Clone`, `Copy` or `Default` and have no public constructor, so a
//!   mutable reference cannot exchange a part out of its loan
//!   (compile-fail tests);
//! - everything that can act on the port borrows the phase part it came
//!   from (compile-fail tests); received data and returned bodies act on
//!   nothing and may outlive it;
//! - everything the hardware touches (descriptors, DMA buffers, key tables)
//!   is `'static` and owned by the port, so no abandoned owner is ever a
//!   memory-safety issue.
//!
//! # Port-lifetime and phase-scoped tokens
//!
//! A resource that outlives a phase (an installed key, an agreement, a
//! configured interface) has its token kept by the owner and lent into each
//! phase. Admitted work is phase-scoped: [`Work`] borrows its client. The
//! pools of the permits work holds (a transmit slot, a terminal-event cell)
//! live in the backend's held part of the install, which outlives every
//! phase: a submission moves its permit into the backend, which gives it
//! back to its pool when the work ends and before it hands the terminal
//! out, so no queue overflows, no terminal is lost and no caller holds the
//! permit of submitted work; only unsolicited reception may lose events.
//! A taker waits for a permit with its own seat in the pool's line, which
//! the body's resources hand it with its client and which it keeps across
//! takes, so the line holds a place for every taker and serves them in
//! arrival order. A `'static` sans-IO core holds no
//! phase-scoped token: the phase keeps a boundary table from the core's own
//! identity to the token, which carries out the core's decisions.
//!
//! # Drop
//!
//! Rust's types are affine: an owner can be dropped and `mem::forget` is
//! safe. A body cannot do either and still end, and it runs as an executor
//! task that is not cancelled; inside it, the phase's parts run under
//! `join` over disjoint borrows. An owner is dropped only by a panic (on the
//! target, a reset) or by a body raced outside its task. Its `Drop` is a
//! safety net, as [`PlatformClockGuard`]'s: it calls
//! [`PortBackend::abandon`] (close transmission, mask interrupts, refuse any
//! new install until a reset) and asserts in debug builds; it is never the
//! shutdown.
//!
//! # Several executor tasks
//!
//! A borrow does not cross executor tasks. The port's parts stay inside its
//! body's task; another task (a network stack) talks to the body as to an
//! actor, by owned requests and replies. A handle shared between tasks over
//! an internal mutex is not a form of this contract.
//!
//! # The driver
//!
//! [`run_port`] joins the body with the backend's driver
//! ([`PortBackend::drive`]), its runner, in the body's task, and polls both
//! at every wake until the body ends. So the driver's progress never
//! depends on what the body awaits (a foreign future, a permit the driver
//! itself frees), and a body takes on no liveness duty. The driver runs for
//! the body's whole life, enabled or disabled, and reaches the install only
//! through the backend's held part ([`PortBackend::Held`]) in one of two
//! forms:
//!
//! - **Messages:** the driver owns the backend's state by value (its radio
//!   role and hardware), and the parts reach it through a request channel,
//!   an outcome queue and wakes in `Held`; it returns that state
//!   ([`PortBackend::State`]) when it retires.
//! - **Direct:** the state is in `Held`, which the parts reach
//!   synchronously, and the driver serves it there and returns `()`.
//!
//! The uninstall is `run_port`'s, because a driver still pending holds the
//! state and borrows `Held`. [`Owner::uninstall`] and
//! [`PoisonedOwner::uninstall`] only record the request in the [`Ended`]
//! they return. Once the body returned it, `run_port`:
//!
//! 1. asks the driver to retire ([`Retire`]): the driver finishes its
//!    current step and returns its state within the bound the backend
//!    states ([`PortBackend::retire_bound`]); a driver that misses its
//!    bound is a fail-stop, and `run_port` abandons the backend and
//!    returns `None`;
//! 2. detaches the backend's interrupt entries synchronously
//!    ([`PortBackend::detach`]);
//! 3. calls [`PortBackend::uninstall`] with the held part and the driver's
//!    state by value, which returns the hardware owners by value.
//!
//! A fail-stop and a dropped owner abandon the backend and halt the driver
//! synchronously: `run_port` does not poll it again. The driver is
//! cancel-safe ([`PortBackend::drive`]).
//!
//! The host tests of this module check what types cannot: a body awaiting
//! a never-ready foreign future does not stall the driver; an uninstall
//! retires, then detaches, then uninstalls; a driver that misses its bound
//! fail-stops the port; a fail-stop and a dropped owner halt the driver at
//! its next poll.
//!
//! # Inside the backend
//!
//! One owner reaches the backend's state of the install at a time: the
//! body's task, where the joined driver and the body's parts take turns
//! through `Held`, or the backend's own interrupt context, as the interrupt
//! rule of [ownership and refusals] decides. Anything beside the body (an
//! interrupt entry, another task) reaches that state only through atomics
//! or signals; no task and no async lock is shared with it.
//!
//! - **The interrupt latches, the body's task owns:** the interrupt reads
//!   and clears its own status and posts it, and the driver serves it.
//! - **The interrupt's context owns:** terminals and receptions never
//!   depend on the driver, which runs only while the body does, so a body
//!   that ends its phase early would otherwise strand them.
//!
//! The rules of the body's task
//! ([owner decisions on ](https://github.com/the predecessor repository/issues/581#issuecomment-)):
//!
//! - **Shared state through `with`.** Per-install state the parts and the
//!   driver share through `Held` is reached by a synchronous closure
//!   (`with`), so no borrow spans an await, and a `with` closure calls no
//!   port method: a nested borrow is a same-task reentry.
//! - **A retune stays a bounded step of `enable`**, not of the driver:
//!   `enable` takes what it needs out of `Held` and puts it back, and a
//!   driver pass meanwhile defers the work that needs it.
//! - **An interrupt-owned backend's body runs on its routes' core**, and
//!   the route binding's type states that core, so the body starts only
//!   through a spawner of that core's executor and a synchronous detach
//!   masks a handler that cannot be mid-execution. A backend whose
//!   interrupt only posts atomics needs no such binding.
//!
//! One exception is allowed: IEEE 802.15.4's interim critical section
//! between its interrupt and the phase, a short interrupt mask without an
//! await, until [#668](https://github.com/the predecessor repository/issues/668)
//! moves its engine into the interrupt's context and removes it.
//!
//! None of this reaches a caller as a refusal.
//!
//! # Compile-fail tests
//!
//! A body cannot end without an uninstall or a fail-stop:
//!
//! ```compile_fail,E0451
//! # use oer_i_port::{Disabled, Ended, Owner, PortBackend, PortBody};
//! struct Forget;
//! impl<B: PortBackend> PortBody<B> for Forget {
//!     async fn run<'id>(self, owner: Owner<'_, 'id, B, Disabled>, _: B::Resources<'id>) -> Ended<'id, B> {
//!         drop(owner);
//!         Ended { uninstall: None }
//!     }
//! }
//! ```
//!
//! Its compiling twin ends through the owner:
//!
//! ```
//! # use oer_i_port::{Disabled, Ended, Owner, PortBackend, PortBody};
//! struct FailStop;
//! impl<B: PortBackend> PortBody<B> for FailStop {
//!     async fn run<'id>(self, owner: Owner<'_, 'id, B, Disabled>, _: B::Resources<'id>) -> Ended<'id, B> {
//!         owner.fail_stop()
//!     }
//! }
//! ```
//!
//! No caller makes an owner outside a body:
//!
//! ```compile_fail,E0624
//! # use oer_i_port::{Disabled, Owner, PortBackend};
//! fn forge<'p, 'id, B: PortBackend>(backend: B) -> Owner<'p, 'id, B, Disabled> {
//!     Owner::new(backend)
//! }
//! ```
//!
//! A disabled port lends no phase:
//!
//! ```compile_fail,E0599
//! # use oer_i_port::{Disabled, Owner, PortBackend};
//! fn phase_while_disabled<'id, B: PortBackend>(owner: &mut Owner<'_, 'id, B, Disabled>) {
//!     let _ = owner.split();
//! }
//! ```
//!
//! An enabled port takes no disabled-only setting and no uninstall:
//!
//! ```compile_fail,E0599
//! # use oer_i_port::{Enabled, Owner, PortBackend};
//! fn setup_while_enabled<'id, B: PortBackend>(owner: &mut Owner<'_, 'id, B, Enabled>) {
//!     let _ = owner.setup();
//! }
//! ```
//!
//! ```compile_fail,E0599
//! # use oer_i_port::{Enabled, Owner, PortBackend};
//! fn uninstall_while_enabled<'id, B: PortBackend>(owner: Owner<'_, 'id, B, Enabled>, control: B::Uninstall) {
//!     let _ = owner.uninstall(control);
//! }
//! ```
//!
//! An owner is gone once its command starts, so a second command cannot
//! start beside it:
//!
//! ```compile_fail,E0382
//! # use oer_i_port::{Disabled, Owner, PortBackend};
//! async fn two_commands<'id, B: PortBackend>(owner: Owner<'_, 'id, B, Disabled>) {
//!     let enabling = owner.enable();
//!     let _ = owner.enable();
//!     let _ = enabling.await;
//! }
//! ```
//!
//! A phase's part does not outlive the phase:
//!
//! ```compile_fail,E0505
//! # use oer_i_port::{Enabled, Owner, PortBackend};
//! async fn keep_the_control<'id, B: PortBackend>(mut owner: Owner<'_, 'id, B, Enabled>) {
//!     let control = owner.split().control;
//!     let _ = owner.disable(&mut |_| {}).await;
//!     drop(control);
//! }
//! ```
//!
//! Nor does a second phase run beside the first:
//!
//! ```compile_fail,E0499
//! # use oer_i_port::{Enabled, Owner, PortBackend};
//! fn two_phases<'id, B: PortBackend>(owner: &mut Owner<'_, 'id, B, Enabled>) {
//!     let first = owner.split();
//!     let second = owner.split();
//!     drop((first, second));
//! }
//! ```
//!
//! A caller does not reach the backend past the owner:
//!
//! ```compile_fail,E0616
//! # use oer_i_port::{Enabled, Owner, PortBackend};
//! fn reach_the_backend<'id, B: PortBackend>(owner: &mut Owner<'_, 'id, B, Enabled>) {
//!     let _ = &mut owner.backend;
//! }
//! ```
//!
//! Nor takes a part out of its loan:
//!
//! ```compile_fail,E0616
//! # use oer_i_port::{Enabled, Owner, PortBackend};
//! fn take_the_part<'id, B: PortBackend>(owner: &mut Owner<'_, 'id, B, Enabled>) {
//!     let _ = owner.split().control.part;
//! }
//! ```
//!
//! Nor does admitted work outlive the client that admitted it, so no token
//! names the work of a later phase:
//!
//! ```compile_fail,E0515
//! # use oer_i_port::{RadioPortClient, Work};
//! fn keep_the_work<C: RadioPortClient>(
//!     admit: impl for<'c> Fn(&'c C) -> Work<'c, C>,
//!     client: C,
//! ) -> Work<'static, C> {
//!     admit(&client)
//! }
//! ```
//!
//! Nor does a caller take the token out of a work, so a work ends only
//! through the client that admitted it:
//!
//! ```compile_fail,E0616
//! # use oer_i_port::{RadioPortClient, Work};
//! fn take_the_token<C: RadioPortClient>(work: Work<'_, C>) {
//!     let _ = work.token;
//! }
//! ```
//!
//! Its compiling twin reads only the work's routing key:
//!
//! ```
//! # use oer_i_port::{RadioPortClient, Work};
//! fn read_the_key<C: RadioPortClient>(work: Work<'_, C>) {
//!     let _ = work.id();
//! }
//! ```
//!
//! A driver's future borrows the install, never the backend, and does not
//! outlive the install's borrow:
//!
//! ```compile_fail,E0700
//! # use core::future::Future;
//! # use oer_i_port::{PortBackend, Retire};
//! fn keep_the_driver<'p, 'id, B: PortBackend + 'static>(
//!     backend: &mut B,
//!     held: &'p B::Held<'id>,
//!     retire: Retire<'p>,
//! ) -> impl Future<Output = B::State> + 'static {
//!     backend.drive(held, retire)
//! }
//! ```
//!
//! Its compiling twin keeps the driver for the install's borrow only, and
//! reaches the backend beside it:
//!
//! ```
//! # use core::future::Future;
//! # use oer_i_port::{PortBackend, Retire};
//! fn keep_the_driver<'p, 'id, B: PortBackend + 'static>(
//!     backend: &mut B,
//!     held: &'p B::Held<'id>,
//!     retire: Retire<'p>,
//! ) -> impl Future<Output = B::State> + use<'p, 'id, B> {
//!     let driver = backend.drive(held, retire);
//!     let _ = backend.now();
//!     driver
//! }
//! ```
//!
//! Only `run_port` asks a driver to retire:
//!
//! ```compile_fail,E0451
//! # use core::cell::Cell;
//! # use oer_i_port::Retire;
//! fn forge(requested: &Cell<bool>) -> Retire<'_> {
//!     Retire { requested }
//! }
//! ```
//!
//! Its compiling twin reads the request it was given:
//!
//! ```
//! # use oer_i_port::Retire;
//! fn read(retire: &Retire<'_>) -> bool {
//!     retire.is_requested()
//! }
//! ```
//!
//! The compiling twin of the lifecycle tests runs a phase between the
//! commands and keeps the work only as long as its client:
//!
//! ```
//! # use oer_i_port::{Disabled, Ended, Owner, PortBackend, PortBody, RadioPortClient, Work};
//! struct OnePhase;
//! impl<B: PortBackend> PortBody<B> for OnePhase {
//!     async fn run<'id>(self, mut owner: Owner<'_, 'id, B, Disabled>, _: B::Resources<'id>) -> Ended<'id, B> {
//!         let _ = owner.setup();
//!         let mut enabled = match owner.enable().await {
//!             Ok(enabled) => enabled,
//!             Err(failed) => return failed.fail_stop(),
//!         };
//!         {
//!             let phase = enabled.split();
//!             drop(phase);
//!         }
//!         match enabled.disable(&mut |_| {}).await {
//!             Ok(disabled) => disabled.fail_stop(),
//!             Err(poisoned) => poisoned.fail_stop(),
//!         }
//!     }
//! }
//! fn keep_the_work<'c, C: RadioPortClient>(
//!     admit: impl Fn(&'c C) -> Work<'c, C>,
//!     client: &'c C,
//! ) -> Work<'c, C> {
//!     admit(client)
//! }

//! ```
//!
//! The compiling twins of the remaining tests differ only in the rule each
//! breaks: an owner comes from `run_port`, a disabled owner uninstalls, the
//! clock is reached through the owner, and a part is borrowed through its
//! loan:
//!
//! ```
//! # use oer_i_port::{Disabled, Enabled, Ended, Owner, PortBackend, PortBody, run_port};
//! async fn own<B: PortBackend>(backend: B, body: impl PortBody<B>) -> Option<B::Uninstalled> {
//!     run_port(backend, body).await
//! }
//! fn uninstall_while_disabled<'id, B: PortBackend>(owner: Owner<'_, 'id, B, Disabled>, control: B::Uninstall) {
//!     let _ = owner.uninstall(control);
//! }
//! fn reach_through_the_owner<'id, B: PortBackend>(owner: &mut Owner<'_, 'id, B, Enabled>) {
//!     let _ = owner.now();
//! }
//! fn borrow_the_part<'id, B: PortBackend>(owner: &mut Owner<'_, 'id, B, Enabled>) {
//!     let _ = &mut *owner.split().control;
//! }
//! ```
//!
//! [ownership and refusals]: https://github.com/the predecessor repository/blob/main/docs/architecture.md#ownership-and-refusals
//! [`PlatformClockGuard`]: https://github.com/the predecessor repository/blob/main/crates/hardware/esp32s31/hal/src/power/clock/executor.rs

#[cfg(any(test, all(feature = "test-support", not(target_os = "none"))))]
extern crate std;

use core::{
    cell::Cell,
    fmt::Debug,
    future::{Future, poll_fn},
    marker::PhantomData,
    ops::{Deref, DerefMut},
    pin::pin,
    task::Poll,
};

use core::ops::RangeInclusive;

use oer_time::RadioInstant;

use oer_radio_port::{EventsLost, Mint, Poisoned, PortResult, branded};

/// The brand of one install: invariant, so no install's brand converts to
/// another's.
type Brand<'id> = PhantomData<fn(&'id ()) -> &'id ()>;

/// The state of an owner whose port admits no work and receives nothing.
pub enum Disabled {}

/// The state of an owner whose port runs.
pub enum Enabled {}

/// The result of a backend's `Enable`: the radio's refusal inside, a
/// poisoned backend outside.
pub type EnableResult<R, F> = Result<Result<(), R>, Poisoned<F>>;

/// What a backend implements; callers never hold it. [`run_port`] wraps it
/// in the body's owner, joins its driver with the body, and the owner and
/// `run_port` call each method only in the state its documentation names.
pub trait PortBackend: Sized {
    /// One owned event.
    type Event;
    /// The clock domain of the port's radio instants.
    type Domain;
    /// The cause a poisoned backend reports.
    type Fault: Copy + Debug;
    /// Why the radio refused `Enable` (a refused retune): the world's
    /// outcome, after which the port is disabled.
    type Refusal;
    /// The protocol's disabled-only settings, lent by [`Owner::setup`].
    type Setup<'a, 'id>
    where
        Self: 'a;
    /// The phase's one event consumer.
    type Events<'a, 'id>: RadioPortEvents<Event = Self::Event, Fault = Self::Fault>
    where
        Self: 'a,
        'id: 'a;
    /// The phase control: the radio-wide operations that only gate or tune
    /// admission while clients run, as guards on its sub-resources; each
    /// guard proves only its own state (a closed transmit gate proves this
    /// port publishes nothing, `TxGated`, not that the air is quiet).
    type Control<'a, 'id>
    where
        Self: 'a,
        'id: 'a;
    /// The protocol's client handles, one per configured interface.
    type Clients<'a, 'id>
    where
        Self: 'a,
        'id: 'a;
    /// The port-lifetime resources the body receives once with its owner:
    /// slot pools, permit pools and the seats of their lines, minted
    /// through [`Mint`].
    type Resources<'id>;
    /// The port-lifetime part of the install the backend keeps, shared by
    /// the body's parts and the driver: [`run_port`] keeps it for the
    /// port's life, lends it into each phase, disable and driver step, and
    /// hands it to the uninstall by value. It may carry the brand (the
    /// pools of the permits its work holds, which it gives back before it
    /// hands a terminal event out). Whatever the driver serves for the
    /// parts lives here: the state itself, reached synchronously, or the
    /// request channel, outcome queue and wakes to a driver that owns the
    /// state by value. `()` when it keeps none.
    type Held<'id>;
    /// What the driver owns by value and returns when it retires: the
    /// backend's state of a driver that owns it (the parts reach it by
    /// messages through `Held`), or `()` when the parts reach the state
    /// directly through `Held`.
    type State;
    /// What the uninstall takes beside the backend (the composition's
    /// control).
    type Uninstall;
    /// The hardware owners the uninstall returns.
    type Uninstalled;

    /// Enable the disabled port, within the backend's bound.
    fn enable(&mut self) -> impl Future<Output = EnableResult<Self::Refusal, Self::Fault>> + '_;

    /// Disable the enabled port of the install `'id` within the backend's
    /// bound: end every admitted work, hand its terminal event with what it
    /// returns to `sink`, and return when every permit is back in `held`.
    fn disable<'a, 'id>(
        &'a mut self,
        held: &'a Self::Held<'id>,
        sink: &'a mut dyn FnMut(Self::Event),
    ) -> impl Future<Output = Result<(), Poisoned<Self::Fault>>> + 'a
    where
        'id: 'a;

    /// The radio clock.
    fn now(
        &self,
    ) -> impl Future<Output = PortResult<RadioInstant<Self::Domain>, ClockError, Self::Fault>> + '_;

    /// The port-lifetime part the backend keeps and the resources the body
    /// receives of the install `'id`, minted with `mint`; [`run_port`] calls
    /// it once.
    fn resources<'id>(&mut self, mint: Mint<'id>) -> (Self::Held<'id>, Self::Resources<'id>);

    /// Lend the disabled-only settings of the install `'id`. An
    /// implementation names the associated types with both lifetimes
    /// (`Self::Setup<'a, 'id>`), as the trait does, so its `'id` stays a
    /// parameter of the method.
    fn setup<'a, 'id>(&'a mut self) -> Lent<'a, Self::Setup<'a, 'id>>;

    /// Lend one running phase of the enabled port of the install `'id`,
    /// over the part of the install it keeps.
    #[allow(clippy::type_complexity, reason = "the phase's three parts")]
    fn split<'a, 'id>(
        &'a mut self,
        held: &'a Self::Held<'id>,
    ) -> PhaseParts<'a, Self::Events<'a, 'id>, Self::Control<'a, 'id>, Self::Clients<'a, 'id>>
    where
        'id: 'a;

    /// The driver of the install `'id`: the backend's runner, which
    /// [`run_port`] calls once, after [`resources`](Self::resources), and
    /// joins with the body in its task for the body's whole life, enabled
    /// or disabled. The call may move what the driver owns out of the
    /// backend; the future it returns borrows only `held` and `retire`,
    /// never the backend, which the owner lends to the body meanwhile.
    ///
    /// The driver serves what the backend runs in software (an interrupt
    /// status posted to it, a deadline, a backoff, a periodic refresh). It
    /// runs until `retire` is requested, then finishes its current step
    /// and returns its state within [`retire_bound`](Self::retire_bound).
    /// It may return earlier, when it has nothing more to serve;
    /// [`run_port`] keeps the state for the uninstall.
    ///
    /// # Cancel safety
    ///
    /// The driver must be cancel-safe: [`run_port`] drops it at any await
    /// point, at a fail-stop, at a missed retire bound and with its own
    /// future, and stops polling it once a fail-stop or a dropped owner
    /// abandoned the backend. A step that needs a resource across an await
    /// takes it out of `held` and puts it back when it completes, or from a
    /// guard when it drops; meanwhile the driver defers the work that needs
    /// it and the status stays latched.
    fn drive<'p, 'id>(
        &mut self,
        held: &'p Self::Held<'id>,
        retire: Retire<'p>,
    ) -> impl Future<Output = Self::State> + use<'p, 'id, Self>;

    /// The bound within which a driver asked to retire returns its state:
    /// a future that is ready once the bound has passed, measured on the
    /// backend's own timer. [`run_port`] awaits it beside the retiring
    /// driver; if it is ready first, the driver missed its bound and the
    /// port ends as a fail-stop.
    fn retire_bound(&self) -> impl Future<Output = ()> + '_;

    /// Detach the backend's interrupt entries of the install `'id`
    /// synchronously, so none reaches the install's state again.
    /// [`run_port`] calls it once, after the driver retired and before
    /// [`uninstall`](Self::uninstall).
    fn detach<'id>(&mut self, held: &Self::Held<'id>);

    /// Take the disabled or poisoned backend apart with the composition's
    /// `control`, its held part and its retired driver's state, each by
    /// value, and return the hardware owners by value. [`run_port`] calls
    /// it once, after [`detach`](Self::detach).
    fn uninstall<'id>(
        self,
        held: Self::Held<'id>,
        state: Self::State,
        control: Self::Uninstall,
    ) -> Self::Uninstalled;

    /// Stop the backend at once, synchronously: close transmission, mask its
    /// interrupts and refuse any new install until a reset. A fail-stop, a
    /// dropped owner and a driver that missed its retire bound call it;
    /// [`run_port`] polls the driver no more, and the hardware stays
    /// installed and unreachable.
    fn abandon(&mut self);
}

/// The one lifetime of a port, from its install to its [`Ended`]; see the
/// [module](self) documentation.
pub trait PortBody<B: PortBackend> {
    /// Run the port: the owner arrives disabled and leaves through an
    /// uninstall or a fail-stop.
    ///
    /// The body is generic over the install's brand `'id`: it names no
    /// concrete one and cannot prove one `'static`, so nothing branded by
    /// `'id` (the owner, its resources, the tokens its clients mint)
    /// leaves the body or reaches another install. `'p` is the owner's
    /// borrow of the install [`run_port`] keeps.
    fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, B, Disabled>,
        resources: B::Resources<'id>,
    ) -> impl Future<Output = Ended<'id, B>>;
}

/// The retire request [`run_port`] makes of a driver
/// ([`PortBackend::drive`]) once the body asked for the uninstall. It is
/// neither `Clone` nor `Copy`, and only [`run_port`] makes one.
pub struct Retire<'p> {
    requested: &'p Cell<bool>,
}

impl Retire<'_> {
    /// Whether [`run_port`] asked the driver to retire.
    pub fn is_requested(&self) -> bool {
        self.requested.get()
    }

    /// Ready once [`run_port`] asked the driver to retire. It registers no
    /// waker: [`run_port`] polls the driver right after it asks.
    pub fn requested(&self) -> impl Future<Output = ()> + '_ {
        poll_fn(|_| {
            if self.requested.get() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
    }
}

/// The install [`run_port`] keeps for the port's life: the backend's held
/// part, which the body's parts and the driver share, and the driver's two
/// signals.
struct Install<'id, B: PortBackend> {
    held: B::Held<'id>,
    /// Set by a fail-stop or a dropped owner: the driver is polled no more.
    halted: Cell<bool>,
    /// Set by `run_port` once the body asked for the uninstall.
    retiring: Cell<bool>,
}

/// Run `body` over a just-installed, disabled `backend`. A backend's install
/// calls it, in the executor task that runs the port, and receives the
/// hardware owners an uninstall returned, or `None` after a fail-stop.
///
/// It joins the body with the backend's driver ([`PortBackend::drive`]),
/// polling both at every wake until the body ends, so the driver's
/// progress never depends on what the body awaits. After an uninstall it
/// asks the driver to retire within [`PortBackend::retire_bound`] (a
/// missed bound is a fail-stop), then calls [`PortBackend::detach`] and
/// [`PortBackend::uninstall`].
pub async fn run_port<B: PortBackend>(
    backend: B,
    body: impl PortBody<B>,
) -> Option<B::Uninstalled> {
    branded(async move |mint| install(backend, body, mint).await).await
}

/// [`run_port`] over the install `'id`, whose `mint` [`branded`] made.
async fn install<'id, B: PortBackend>(
    mut backend: B,
    body: impl PortBody<B>,
    mint: Mint<'id>,
) -> Option<B::Uninstalled> {
    let (held, resources) = backend.resources(mint);
    let install = Install {
        held,
        halted: Cell::new(false),
        retiring: Cell::new(false),
    };
    let (mut backend, state, control) = {
        let mut driver = pin!(backend.drive(
            &install.held,
            Retire {
                requested: &install.retiring,
            },
        ));
        let mut body = pin!(body.run(Owner::new(backend, &install), resources));
        let mut state = None;
        let mut ended = poll_fn(|cx| {
            if let Poll::Ready(ended) = body.as_mut().poll(cx) {
                return Poll::Ready(ended);
            }
            if state.is_none()
                && !install.halted.get()
                && let Poll::Ready(retired) = driver.as_mut().poll(cx)
            {
                state = Some(retired);
            }
            Poll::Pending
        })
        .await;
        // A fail-stop already abandoned the backend and halted the driver.
        let (backend, control) = ended.uninstall.take()?;
        // Until `detach`, a cancelled `run_port` abandons the backend, as a
        // dropped owner does.
        let mut retiring = Retiring {
            backend: Some(backend),
            install: &install,
        };
        let state = match state {
            Some(state) => state,
            None => {
                install.retiring.set(true);
                let Some(state) = retired(retiring.backend.as_ref().expect(HELD), driver).await
                else {
                    halt(&install, retiring.take());
                    return None;
                };
                state
            }
        };
        (retiring.take(), state, control)
    };
    backend.detach(&install.held);
    Some(backend.uninstall(install.held, state, control))
}

/// The backend between the body's uninstall and its `detach`, while the
/// driver retires: dropped with a cancelled `run_port`, it halts the driver
/// and abandons the backend, as a dropped owner does.
struct Retiring<'p, 'id, B: PortBackend> {
    /// `None` only once `run_port` took it.
    backend: Option<B>,
    install: &'p Install<'id, B>,
}

impl<B: PortBackend> Retiring<'_, '_, B> {
    fn take(&mut self) -> B {
        self.backend.take().expect(HELD)
    }
}

impl<B: PortBackend> Drop for Retiring<'_, '_, B> {
    fn drop(&mut self) {
        if self.backend.is_some() {
            self.install.halted.set(true);
        }
        abandoned(&mut self.backend);
    }
}

/// The state of a driver asked to retire, or `None` once the backend's
/// retire bound passed first.
async fn retired<B: PortBackend>(
    backend: &B,
    mut driver: core::pin::Pin<&mut impl Future<Output = B::State>>,
) -> Option<B::State> {
    let mut bound = pin!(backend.retire_bound());
    poll_fn(|cx| {
        if let Poll::Ready(state) = driver.as_mut().poll(cx) {
            return Poll::Ready(Some(state));
        }
        if bound.as_mut().poll(cx).is_ready() {
            return Poll::Ready(None);
        }
        Poll::Pending
    })
    .await
}

/// How a port's body ended: the uninstall it asked for, which [`run_port`]
/// carries out, or a fail-stop, which leaves the hardware installed until a
/// reset. Only the uninstalls and the `fail_stop`s construct it.
#[must_use = "an ended port returns its hardware owners"]
pub struct Ended<'id, B: PortBackend> {
    /// The backend and the composition's control of an uninstall; `None`
    /// after a fail-stop, or once `run_port` took them.
    uninstall: Option<(B, B::Uninstall)>,
    brand: Brand<'id>,
}

impl<B: PortBackend> Ended<'_, B> {
    const fn uninstall(backend: B, control: B::Uninstall) -> Self {
        Self {
            uninstall: Some((backend, control)),
            brand: PhantomData,
        }
    }

    const fn fail_stop() -> Self {
        Self {
            uninstall: None,
            brand: PhantomData,
        }
    }
}

impl<B: PortBackend> Drop for Ended<'_, B> {
    fn drop(&mut self) {
        // A body returns its `Ended` and `run_port` takes the backend out, so
        // only a panic or a cancelled `run_port` drops one that still holds
        // its backend; the driver is dropped with it.
        let mut backend = self.uninstall.take().map(|(backend, _)| backend);
        abandoned(&mut backend);
    }
}

/// The one authority over an installed port, in state `S`; see the
/// [module](self) documentation. `'p` is its borrow of the install
/// [`run_port`] keeps; `'id` is the install's brand.
#[must_use = "an owner leaves only through a transition, an uninstall or a fail-stop"]
pub struct Owner<'p, 'id, B: PortBackend, S> {
    /// `None` only once a consuming method took it.
    backend: Option<B>,
    /// The install: the part the backend keeps, and the driver's halt.
    install: &'p Install<'id, B>,
    state: PhantomData<S>,
}

impl<'p, 'id, B: PortBackend, S> Owner<'p, 'id, B, S> {
    const fn new(backend: B, install: &'p Install<'id, B>) -> Self {
        Self {
            backend: Some(backend),
            install,
            state: PhantomData,
        }
    }

    fn backend(&mut self) -> &mut B {
        self.backend.as_mut().expect(HELD)
    }

    /// The backend and the part of the install it keeps, borrowed apart.
    fn parts(&mut self) -> (&mut B, &B::Held<'id>) {
        (self.backend.as_mut().expect(HELD), &self.install.held)
    }

    fn take(mut self) -> B {
        self.backend.take().expect(HELD)
    }

    /// The radio clock.
    pub fn now(
        &self,
    ) -> impl Future<Output = PortResult<RadioInstant<B::Domain>, ClockError, B::Fault>> + '_ {
        self.backend.as_ref().expect(HELD).now()
    }

    /// Stop the backend and its driver at once and end the port without its
    /// uninstall: the hardware stays installed and unreachable until a
    /// reset.
    pub fn fail_stop(self) -> Ended<'id, B> {
        let install = self.install;
        halt(install, self.take());
        Ended::fail_stop()
    }
}

impl<'p, 'id, B: PortBackend> Owner<'p, 'id, B, Disabled> {
    /// Lend the protocol's disabled-only settings.
    pub fn setup(&mut self) -> Lent<'_, B::Setup<'_, 'id>> {
        self.backend().setup()
    }

    /// Enable the port, within the backend's bound.
    ///
    /// # Errors
    ///
    /// The radio's refusal, with the disabled owner, or a poisoned backend.
    pub async fn enable(mut self) -> Result<Owner<'p, 'id, B, Enabled>, Failed<'p, 'id, B>> {
        // The owner holds the backend across the wait, so a dropped future
        // drops the owner and its safety net runs.
        let enabled = self.backend().enable().await;
        let install = self.install;
        let backend = self.take();
        match enabled {
            Ok(Ok(())) => Ok(Owner::new(backend, install)),
            Ok(Err(refusal)) => Err(Failed::Refused(Owner::new(backend, install), refusal)),
            Err(poisoned) => Err(Failed::Poisoned(PoisonedOwner::new(
                backend, install, poisoned,
            ))),
        }
    }

    /// Ask for the uninstall with the composition's `control`: the body
    /// returns the [`Ended`], and [`run_port`] retires the driver, detaches
    /// the interrupt entries and takes the backend apart.
    pub fn uninstall(self, control: B::Uninstall) -> Ended<'id, B> {
        Ended::uninstall(self.take(), control)
    }
}

impl<'p, 'id, B: PortBackend> Owner<'p, 'id, B, Enabled> {
    /// Lend one running phase: its event consumer, its control and its
    /// clients, each a [`Lent`] of this borrow.
    #[allow(clippy::type_complexity, reason = "the phase's three parts")]
    pub fn split(
        &mut self,
    ) -> PhaseParts<'_, B::Events<'_, 'id>, B::Control<'_, 'id>, B::Clients<'_, 'id>> {
        let (backend, held) = self.parts();
        backend.split(held)
    }

    /// Disable the port within the backend's bound: every leftover terminal
    /// of a phase's work, with what it returns, goes to `sink`, and the port
    /// is disabled once every permit is back.
    ///
    /// # Errors
    ///
    /// A poisoned backend.
    pub async fn disable(
        mut self,
        sink: &mut dyn FnMut(B::Event),
    ) -> Result<Owner<'p, 'id, B, Disabled>, PoisonedOwner<'p, 'id, B>> {
        // The owner holds the backend across the wait, as `enable`'s does.
        let disabled = {
            let (backend, held) = self.parts();
            backend.disable(held, sink).await
        };
        let install = self.install;
        let backend = self.take();
        match disabled {
            Ok(()) => Ok(Owner::new(backend, install)),
            Err(poisoned) => Err(PoisonedOwner::new(backend, install, poisoned)),
        }
    }
}

impl<B: PortBackend, S> Drop for Owner<'_, '_, B, S> {
    fn drop(&mut self) {
        if self.backend.is_some() {
            self.install.halted.set(true);
        }
        abandoned(&mut self.backend);
    }
}

/// Why `Enable` did not leave an enabled owner.
pub enum Failed<'p, 'id, B: PortBackend> {
    /// The radio refused; the port is disabled.
    Refused(Owner<'p, 'id, B, Disabled>, B::Refusal),
    /// The backend is poisoned.
    Poisoned(PoisonedOwner<'p, 'id, B>),
}

impl<'id, B: PortBackend> Failed<'_, 'id, B> {
    /// Stop the backend and its driver at once and end the port.
    pub fn fail_stop(self) -> Ended<'id, B> {
        match self {
            Self::Refused(owner, _) => owner.fail_stop(),
            Self::Poisoned(poisoned) => poisoned.fail_stop(),
        }
    }
}

/// The one exit of a poisoned backend: its state is unknown, and it leaves
/// through its uninstall or a fail-stop.
#[must_use = "a poisoned backend leaves only through its uninstall or a fail-stop"]
pub struct PoisonedOwner<'p, 'id, B: PortBackend> {
    /// `None` only once a consuming method took it.
    backend: Option<B>,
    install: &'p Install<'id, B>,
    cause: B::Fault,
}

impl<'p, 'id, B: PortBackend> PoisonedOwner<'p, 'id, B> {
    const fn new(backend: B, install: &'p Install<'id, B>, poisoned: Poisoned<B::Fault>) -> Self {
        Self {
            backend: Some(backend),
            install,
            cause: poisoned.cause,
        }
    }

    fn take(mut self) -> B {
        self.backend.take().expect(HELD)
    }

    /// Why the backend's state is unknown.
    pub const fn cause(&self) -> B::Fault {
        self.cause
    }

    /// Ask for the uninstall with the composition's `control`, as
    /// [`Owner::uninstall`] does.
    pub fn uninstall(self, control: B::Uninstall) -> Ended<'id, B> {
        Ended::uninstall(self.take(), control)
    }

    /// Stop the backend and its driver at once and end the port without
    /// its uninstall.
    pub fn fail_stop(self) -> Ended<'id, B> {
        let install = self.install;
        halt(install, self.take());
        Ended::fail_stop()
    }
}

impl<B: PortBackend> Drop for PoisonedOwner<'_, '_, B> {
    fn drop(&mut self) {
        if self.backend.is_some() {
            self.install.halted.set(true);
        }
        abandoned(&mut self.backend);
    }
}

const HELD: &str = "an owner holds its backend until a consuming method";

/// A fail-stop: halt the driver, then abandon the backend.
fn halt<B: PortBackend>(install: &Install<'_, B>, mut backend: B) {
    install.halted.set(true);
    backend.abandon();
}

/// The safety net of a dropped owner: abandon the backend it still holds,
/// and assert in debug builds, as `PlatformClockGuard` does. The owner
/// halted the driver first.
fn abandoned<B: PortBackend>(backend: &mut Option<B>) {
    let Some(mut backend) = backend.take() else {
        return;
    };
    backend.abandon();
    // A host test unwinding a panic drops its owners; asserting then would
    // abort the test process and hide the first panic.
    #[cfg(any(test, all(feature = "test-support", not(target_os = "none"))))]
    if std::thread::panicking() {
        return;
    }
    debug_assert!(
        false,
        "a port owner was dropped without an uninstall or a fail-stop"
    );
}

/// The parts of one running phase ([`Owner::split`]), each lent for the
/// phase.
pub struct PhaseParts<'a, E, C, K> {
    /// The phase's one event consumer.
    pub events: Lent<'a, E>,
    /// The phase control, with one holder.
    pub control: Lent<'a, C>,
    /// The protocol's client handles, one per configured interface.
    pub clients: Lent<'a, K>,
}

/// One part lent for the borrow `'a` of the port's owner. It reaches its
/// part by shared or mutable reference and gives the part away by no
/// method; `'a` is invariant, so no loan outlasts the borrow that made it.
/// A part's type has no value outside its backend, so a mutable reference
/// cannot exchange it out ([module](self) documentation).
pub struct Lent<'a, T> {
    part: T,
    loan: PhantomData<fn(&'a ()) -> &'a ()>,
}

impl<T> Lent<'_, T> {
    /// Lend `part` for the borrow of the owner that the backend's `split` or
    /// `setup` received.
    pub const fn new(part: T) -> Self {
        Self {
            part,
            loan: PhantomData,
        }
    }
}

impl<T> Deref for Lent<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.part
    }
}

impl<T> DerefMut for Lent<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.part
    }
}

/// The one consumer of a running phase's events: a router that hands each
/// terminal event to the holder of its work ([`Work::id`]).
pub trait RadioPortEvents {
    /// One owned event.
    type Event;
    /// The cause a poisoned backend reports.
    type Fault: Copy + Debug;

    /// The next event. Taking it only dequeues it; dropping the future
    /// loses no event. `Ok(Err(EventsLost))` takes the place of the first
    /// dropped event, which only unsolicited reception may lose. A poisoned
    /// backend returns [`Poisoned`] after every event it produced before
    /// the fault, and again at every later call.
    fn next_event(
        &mut self,
    ) -> impl Future<Output = PortResult<Self::Event, EventsLost, Self::Fault>> + '_;
}

/// One admitted operation: what a client's submission returns. It holds
/// the backend's token, which is neither `Clone` nor `Copy` and only the
/// backend mints, and the client that admitted it, so it names exactly one
/// operation, only its holder ends it, and it ends through the client that
/// admitted it ([`Self::cancel`]). It borrows that client (`'c`), so no
/// work outlives its phase and none can name the work of a later one.
#[must_use = "dropping a work forgoes ending it; its terminal still arrives"]
pub struct Work<'c, C: RadioPortClient> {
    token: C::Token,
    client: &'c C,
}

impl<'c, C: RadioPortClient> Work<'c, C> {
    /// Admitted work of `client`, which the backend's submission minted
    /// `token` for.
    pub const fn new(client: &'c C, token: C::Token) -> Self {
        Self { token, client }
    }

    /// The key a router matches the operation's terminal event by.
    pub fn id(&self) -> <C::Token as RadioWork>::Id {
        self.token.id()
    }

    /// End the operation through the client that admitted it. Its terminal
    /// event is the operation's own, which may be its natural end; a
    /// refusal as [`CancelError::NotRunning`] says the air ended it first,
    /// the world's outcome. Dropping the future may leave the work running
    /// or ended; its terminal arrives either way.
    pub fn cancel(self) -> impl Future<Output = PortResult<(), CancelError, C::Fault>> + 'c {
        self.client.end(self.token)
    }
}

/// A backend's token of one admitted operation ([`Work`]).
pub trait RadioWork {
    /// The key a router matches the operation's terminal event by. It
    /// grants nothing: ending the work takes the [`Work`] itself.
    type Id: Copy + Eq;

    /// The routing key of this operation.
    fn id(&self) -> Self::Id;
}

/// What every client handle of a running phase does beside its protocol's
/// own submissions, settings and permits: read the radio clock and end its
/// own work.
///
/// A client exists only for an interface the disabled port configured, and
/// its protocol's calls name only that interface. Its submissions return a
/// [`Work`] that borrows it and ends through it.
pub trait RadioPortClient {
    /// The backend's token of one admitted operation of this client.
    type Token: RadioWork;
    /// The clock domain of the port's radio instants.
    type Domain;
    /// The cause a poisoned backend reports.
    type Fault: Copy + Debug;

    /// The radio clock: the epoch of the port's scheduled work and receive
    /// timestamps, at the resolution its [`ClockInfo`](oer_radio_port::ClockInfo)
    /// states.
    fn now(
        &self,
    ) -> impl Future<Output = PortResult<RadioInstant<Self::Domain>, ClockError, Self::Fault>> + '_;

    /// The backend's side of [`Work::cancel`]: end the operation `token`
    /// names, which this client admitted. Only a [`Work`] holds a token and
    /// nothing takes it out but `cancel`, which calls the client that
    /// admitted it, so a caller reaches this only through that `cancel`.
    fn end(
        &self,
        token: Self::Token,
    ) -> impl Future<Output = PortResult<(), CancelError, Self::Fault>> + '_;
}

/// Why the radio clock could not be read; nothing changed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ClockError {
    /// The backend could not take a reading now (a latch failed, the clock
    /// belongs to a replaced radio start); a later call may succeed.
    Unavailable,
    /// The radio epoch has no later instant: the clock cannot advance, and
    /// no later call succeeds. The port still settles the work it admitted.
    EpochExhausted,
}

/// Why a cancellation was refused; nothing changed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CancelError {
    /// No admitted work has this identity: it already ended and its terminal
    /// event was reported, or it never ran.
    NotRunning,
}

/// A lifecycle command; each admitted one ends with one [`LifecycleEvent`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LifecycleCommand {
    /// Start admitting work and receiving; ends with
    /// [`LifecycleEvent::Enabled`] or [`LifecycleEvent::Failed`].
    Enable,
    /// Stop admitting work, end the work in flight and stop receiving; ends
    /// with [`LifecycleEvent::Disabled`] after the terminal event of every
    /// admitted operation.
    Disable,
    /// Stop admitting work and let the work in flight complete; ends with
    /// [`LifecycleEvent::Quiesced`] after the last terminal event.
    /// [`Self::Enable`] resumes admission.
    Quiesce,
}

/// The terminal event of a lifecycle command.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LifecycleEvent {
    /// [`LifecycleCommand::Enable`] completed.
    Enabled,
    /// [`LifecycleCommand::Disable`] completed.
    Disabled,
    /// [`LifecycleCommand::Quiesce`] completed.
    Quiesced,
    /// An admitted command failed and left the port in the state it was in
    /// before the command. A failure that leaves the backend's state
    /// unknown is not reported here: [`Poisoned`] follows instead.
    Failed {
        /// The command that failed.
        command: LifecycleCommand,
    },
}

/// Why a lifecycle command was refused; nothing changed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LifecycleError {
    /// The port is already in the requested state.
    AlreadyInState,
    /// The command does not apply in the port's current state, or the
    /// backend does not implement it.
    InvalidState,
    /// Another lifecycle command has not ended yet.
    Busy,
}

/// A caller-chosen correlation identity of one port's submissions.
///
/// Each port keeps its own identity type (a Wi-Fi `TxId`, an
/// IEEE 802.15.4 `RequestId`, a Bluetooth LE `EventId`), so the identity of
/// one port cannot complete another's work; all of them are 32-bit values
/// with the same [`BACKEND_RESERVED`] range, which callers never choose and
/// a backend may use for work it submits on its own.
pub trait Correlation: Copy + Eq {
    /// The identity with this raw value.
    fn from_raw(raw: u32) -> Self;

    /// The raw value.
    fn raw(self) -> u32;

    /// Whether the identity lies in the backend's range.
    fn is_backend_reserved(self) -> bool {
        BACKEND_RESERVED.contains(&self.raw())
    }
}

/// Raw identities a caller never chooses: a backend may name its own work
/// with them, and a caller's allocator skips them.
pub const BACKEND_RESERVED: RangeInclusive<u32> = 0xFFFF_FF00..=u32::MAX;

/// A caller's allocator of correlation identities: consecutive values that
/// wrap before [`BACKEND_RESERVED`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CorrelationIds {
    next: u32,
}

impl Default for CorrelationIds {
    fn default() -> Self {
        Self::new()
    }
}

impl CorrelationIds {
    /// Identities from zero.
    pub const fn new() -> Self {
        Self::starting_at(0)
    }

    /// Identities from `first`; a reserved `first` starts at zero.
    pub const fn starting_at(first: u32) -> Self {
        Self {
            next: if first >= *BACKEND_RESERVED.start() {
                0
            } else {
                first
            },
        }
    }

    /// The next identity.
    #[allow(
        clippy::should_implement_trait,
        reason = "the allocator never ends and yields any identity type"
    )]
    pub fn next<I: Correlation>(&mut self) -> I {
        let id = self.next;
        self.next = match id.checked_add(1) {
            Some(next) if next < *BACKEND_RESERVED.start() => next,
            _ => 0,
        };
        I::from_raw(id)
    }
}

#[cfg(test)]
mod tests;
