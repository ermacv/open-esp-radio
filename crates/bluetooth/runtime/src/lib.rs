#![no_std]
#![forbid(unsafe_code)]

//! Executor-independent service loop of the Bluetooth LE Controller.
//!
//! [`serve`] moves packets and radio work between three owners: the
//! Controller end of an in-process HCI transport, the sans-IO
//! [`LeController`] core and one [`LeRadioBackend`], which the protocol
//! package `oer-bluetooth-radio` declares. It runs as the body of the
//! backend's port ([`PortBody`]): [`run_port`] hands it the port's owner
//! disabled, and it returns the owner's [`Ended`].
//!
//! The body enables the port, serves one phase and then disables and
//! uninstalls it. While it serves, it publishes the core's queued packets,
//! takes the next Host command when the core is ready for one, submits the
//! core's radio requests one at a time through the phase's client and feeds
//! every radio outcome back. A refused request is retried after
//! [`REFUSED_RETRY_DELAY`] or the next outcome or command. The core plans
//! only what the backend's capabilities ([`LeRadioBackend::Caps`]) serve,
//! so no request is refused as outside them. Whenever the core's
//! [`RadioActivity`] changes, the body reports it through the phase's
//! control before doing anything else.
//!
//! The body owns no memory of its own beyond one command buffer and spawns
//! nothing; [`run_port`] joins it with the backend's driver in the task the
//! caller polls it on. It stops serving when the caller's `stop`
//! completes, when the transport closes or fails, when the radio's clock
//! cannot be read, or when the radio epoch is exhausted ([`ServeExit`]).
//! The backend reserves every event's end and a connection event's data
//! when it admits the event, so an [`EventsLost`] stands for advertising
//! and scan reports only and the body goes on: the core's roles still
//! account every event by its end. Host ACL data enters the core while it
//! has room for a packet; until then commands pass queued data.
//!
//! The radio epoch is exhausted when a fresh sample lies past its end
//! ([`ClockError::EpochExhausted`], [`RequestError::EpochExhausted`]) or a
//! required continuation of the core's schedule lies outside it
//! ([`EpochExhausted`]): no later sample brings either back, so the body
//! submits nothing more, retries nothing and stops serving.
//!
//! The owner's `disable`, bounded by the backend, then hands every event
//! the phase left to the core: each admitted event ends exactly once, with
//! its real completion when it ran. The caller then receives the
//! backend's uninstalled hardware and a core that accounted every event,
//! ready for the Host epoch's retirement. A backend poisoned while serving
//! ([`Served::Poisoned`]) or while disabling ([`Served::DisableFailed`]) is
//! fail-stopped instead: no hardware comes back, and the caller keeps its
//! other owners as they are.

#[cfg(test)]
extern crate std;

use core::pin::pin;

use embassy_futures::select::{Either, Either4, select, select4};
use embassy_sync::blocking_mutex::raw::RawMutex;
use oer_bluetooth_controller::{EpochExhausted, LeController, RadioWork};
use oer_bluetooth_hci::HostToControllerFrame;
use oer_bluetooth_hci_transport::{HciChannelError, InProcessHciControllerTransport};
use oer_bluetooth_port::{
    Disabled, Ended, Failed, Owner, PhaseParts, PortBackend, PortBody, RadioPortEvents, run_port,
};
use oer_bluetooth_radio::{
    ClockError, EventsLost, LeRadioBackend, Poisoned, RadioActivity, RequestError,
};
use oer_time::{Duration, Instant, Timer};

/// Delay before asking the core again after the radio refused a request.
pub const REFUSED_RETRY_DELAY: Duration = Duration::from_millis(1);

/// Why [`serve`] stopped serving.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServeExit {
    /// The caller's stop request completed.
    Stopped,
    /// The transport closed.
    Closed,
    /// The transport refused a packet.
    Transport(HciChannelError),
    /// The radio clock could not be read now.
    Clock(ClockError),
    /// The radio epoch is exhausted.
    EpochExhausted(Exhaustion),
}

/// Where [`serve`] found the radio epoch exhausted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Exhaustion {
    /// A fresh sample of the radio clock lies past the epoch's end.
    Clock,
    /// A required continuation of the core's schedule lies outside the epoch.
    Planning(EpochExhausted),
}

/// How [`serve`] ended; `F` is the backend's poison cause.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Served<F> {
    /// The service stopped serving for `exit`, the port's `disable` ended
    /// every admitted event, and the backend is uninstalled.
    Stopped(ServeExit),
    /// The service stopped serving for `exit`, and the port's `disable`
    /// found or left the backend poisoned: admitted events may be
    /// unaccounted, and the backend is fail-stopped.
    DisableFailed {
        /// Why the service stopped serving.
        exit: ServeExit,
        /// Why the backend is poisoned.
        cause: F,
    },
    /// The backend was poisoned before the service stopped serving; it is
    /// fail-stopped.
    Poisoned(F),
    /// The service stopped serving for this exit and disabled and
    /// uninstalled the port, but the backend's driver missed its retire
    /// bound: [`run_port`] fail-stopped the backend.
    Unretired(ServeExit),
}

/// Serve the Host through `transport` with `core` over `backend` until
/// `stop` completes or the transport, the radio or the radio epoch ends the
/// service, then disable the port and uninstall the backend with
/// `uninstall`. Retries after a refusal wait on `timer`.
///
/// Returns how the service ended, with the backend's uninstalled hardware,
/// or `None` after a fail-stop.
pub async fn serve<
    B,
    M,
    const H2C: usize,
    const C2H: usize,
    const PACKET: usize,
    const OUTPUT: usize,
    const CONNECTIONS: usize,
>(
    backend: B,
    uninstall: B::Uninstall,
    transport: &InProcessHciControllerTransport<'_, M, H2C, C2H, PACKET>,
    core: &mut LeController<'_, B::Caps, OUTPUT, CONNECTIONS>,
    timer: &impl Timer,
    stop: impl Future<Output = ()>,
) -> (Served<B::Fault>, Option<B::Uninstalled>)
where
    B: LeRadioBackend,
    M: RawMutex,
{
    let mut served = None;
    let uninstalled = run_port(
        backend,
        Serve {
            uninstall,
            transport,
            core,
            timer,
            stop,
            served: &mut served,
        },
    )
    .await;
    let served = match (served.expect("the body records how it ended"), &uninstalled) {
        // The body uninstalled the port, but its driver did not retire in
        // time.
        (Served::Stopped(exit), None) => Served::Unretired(exit),
        (served, _) => served,
    };
    (served, uninstalled)
}

/// The service as the port's body.
struct Serve<
    's,
    't,
    'c,
    B: PortBackend,
    M: RawMutex,
    T,
    S,
    const H2C: usize,
    const C2H: usize,
    const PACKET: usize,
    const OUTPUT: usize,
    const CONNECTIONS: usize,
> where
    B: LeRadioBackend,
{
    uninstall: B::Uninstall,
    transport: &'s InProcessHciControllerTransport<'t, M, H2C, C2H, PACKET>,
    core: &'s mut LeController<'c, B::Caps, OUTPUT, CONNECTIONS>,
    timer: &'s T,
    stop: S,
    served: &'s mut Option<Served<B::Fault>>,
}

impl<
    B,
    M,
    T,
    S,
    const H2C: usize,
    const C2H: usize,
    const PACKET: usize,
    const OUTPUT: usize,
    const CONNECTIONS: usize,
> PortBody<B> for Serve<'_, '_, '_, B, M, T, S, H2C, C2H, PACKET, OUTPUT, CONNECTIONS>
where
    B: LeRadioBackend,
    M: RawMutex,
    T: Timer,
    S: Future<Output = ()>,
{
    async fn run<'p, 'id>(
        self,
        owner: Owner<'p, 'id, B, Disabled>,
        // The port-lifetime resources carry nothing the body uses yet.
        _: B::Resources<'id>,
    ) -> Ended<'id, B> {
        let Self {
            uninstall,
            transport,
            core,
            timer,
            stop,
            served,
        } = self;
        let mut owner = match owner.enable().await {
            Ok(enabled) => enabled,
            Err(Failed::Refused(_, refusal)) => match refusal {},
            Err(Failed::Poisoned(poisoned)) => {
                *served = Some(Served::Poisoned(poisoned.cause()));
                return poisoned.fail_stop();
            }
        };
        let exit = {
            let PhaseParts {
                mut events,
                control,
                mut clients,
            } = owner.split();
            serve_until::<B, _, _, H2C, C2H, PACKET, OUTPUT, CONNECTIONS>(
                transport,
                core,
                &mut *events,
                &control,
                &mut clients,
                timer,
                stop,
            )
            .await
        };
        let exit = match exit {
            Ok(exit) => exit,
            Err(poisoned) => {
                *served = Some(Served::Poisoned(poisoned.cause));
                return owner.fail_stop();
            }
        };
        match owner
            .disable(&mut |event| core.outcome(B::view(&event)))
            .await
        {
            Ok(disabled) => {
                *served = Some(Served::Stopped(exit));
                disabled.uninstall(uninstall)
            }
            Err(poisoned) => {
                *served = Some(Served::DisableFailed {
                    exit,
                    cause: poisoned.cause(),
                });
                poisoned.fail_stop()
            }
        }
    }
}

/// Serve one phase until the service must stop: why, or the backend's
/// poison.
#[allow(
    clippy::too_many_arguments,
    reason = "the phase's three parts beside the service's owners"
)]
async fn serve_until<
    'a,
    'id,
    B,
    M,
    T,
    const H2C: usize,
    const C2H: usize,
    const PACKET: usize,
    const OUTPUT: usize,
    const CONNECTIONS: usize,
>(
    transport: &InProcessHciControllerTransport<'_, M, H2C, C2H, PACKET>,
    core: &mut LeController<'_, B::Caps, OUTPUT, CONNECTIONS>,
    events: &mut B::Events<'a, 'id>,
    control: &B::Control<'a, 'id>,
    clients: &mut B::Clients<'a, 'id>,
    timer: &T,
    stop: impl Future<Output = ()>,
) -> Result<ServeExit, Poisoned<B::Fault>>
where
    B: LeRadioBackend + 'a,
    M: RawMutex,
    T: Timer,
{
    let mut stop = pin!(stop);
    let mut buffer = [0; PACKET];
    let mut retry_at: Option<Instant> = None;
    let mut reported = RadioActivity::IDLE;
    loop {
        // Report a change of the active roles first.
        let activity = core.activity();
        if activity != reported {
            B::activity(control, activity)?;
            reported = activity;
        }

        // Publish what the core queued.
        while let Some(packet) = core.front() {
            match transport.try_publish(packet.kind(), packet.as_bytes()) {
                Ok(()) => core.pop(),
                Err(HciChannelError::Full) => break,
                Err(HciChannelError::Closed) => return Ok(ServeExit::Closed),
                Err(error) => return Ok(ServeExit::Transport(error)),
            }
        }

        // Submit the next radio request.
        if core.wants_radio() && retry_at.is_none_or(|at| timer.now() >= at) {
            retry_at = None;
            let now = match B::client_now(&mut *clients).await {
                Ok(Ok(now)) => now,
                Ok(Err(ClockError::EpochExhausted)) => {
                    return Ok(ServeExit::EpochExhausted(Exhaustion::Clock));
                }
                Ok(Err(error)) => return Ok(ServeExit::Clock(error)),
                Err(poisoned) => return Err(poisoned),
            };
            let work = match core.next_request(now) {
                Ok(work) => work,
                Err(exhausted) => {
                    return Ok(ServeExit::EpochExhausted(Exhaustion::Planning(exhausted)));
                }
            };
            match work {
                Some(RadioWork::Submit(request)) => {
                    let result = match B::submit(&mut *clients, request).await {
                        Ok(result) => result,
                        Err(poisoned) => return Err(poisoned),
                    };
                    core.request_done(result);
                    match result {
                        Ok(()) => {}
                        Err(RequestError::EpochExhausted) => {
                            return Ok(ServeExit::EpochExhausted(Exhaustion::Clock));
                        }
                        Err(_) => retry_at = Some(retry_after_refusal(timer)),
                    }
                    continue;
                }
                Some(RadioWork::Cancel(id)) => {
                    let result = match B::cancel(&mut *clients, id).await {
                        Ok(result) => result,
                        Err(poisoned) => return Err(poisoned),
                    };
                    core.cancel_done(result);
                    continue;
                }
                None => {}
            }
            // Nothing could be placed now; ask again shortly.
            retry_at = Some(retry_after_refusal(timer));
        }

        // Take the next command, or ACL data while the connection takes it.
        if core.is_command_ready() {
            let admission = core.acl_admission();
            match transport.try_receive_admitted(&mut buffer, |handle| admission.admits(handle)) {
                Ok(HostToControllerFrame::Command(command)) => {
                    core.command(command)
                        .expect("the core is ready for a command");
                    continue;
                }
                Ok(HostToControllerFrame::Acl(packet)) => {
                    core.acl(packet);
                    continue;
                }
                // LE carries no synchronous or isochronous data here.
                Ok(_) => continue,
                Err(HciChannelError::Empty) => {}
                Err(HciChannelError::Closed) => return Ok(ServeExit::Closed),
                Err(error) => return Ok(ServeExit::Transport(error)),
            }
        }

        // Wait for any of them to make progress.
        let command_ready = core.is_command_ready();
        let admission = core.acl_admission();
        let publishing = core.front().is_some();
        let retry = retry_at.filter(|_| core.wants_radio());
        let event = select4(
            events.next_event(),
            async {
                if command_ready {
                    transport
                        .wait_receive_admitted(|handle| admission.admits(handle))
                        .await;
                } else {
                    core::future::pending::<()>().await;
                }
            },
            async {
                if publishing {
                    transport.wait_publish_ready().await;
                } else {
                    core::future::pending::<()>().await;
                }
            },
            select(stop.as_mut(), async {
                match retry {
                    Some(at) => timer.wait_until(at).await,
                    None => core::future::pending::<()>().await,
                }
            }),
        )
        .await;
        match event {
            Either4::First(Ok(Ok(event))) => {
                retry_at = None;
                core.outcome(B::view(&event));
            }
            // Advertising and scan reports are gone; every event's end and
            // the connection's data still follow.
            Either4::First(Ok(Err(EventsLost))) => {}
            Either4::First(Err(poisoned)) => return Err(poisoned),
            Either4::Second(()) | Either4::Third(()) => {}
            Either4::Fourth(Either::First(())) => return Ok(ServeExit::Stopped),
            Either4::Fourth(Either::Second(())) => retry_at = None,
        }
    }
}

/// When to ask the core again after a refusal; the end of time when the
/// delay would leave the timer's range.
fn retry_after_refusal(timer: &impl Timer) -> Instant {
    timer
        .deadline_after(REFUSED_RETRY_DELAY)
        .unwrap_or(Instant::from_micros(u64::MAX))
}

#[cfg(test)]
mod tests;
