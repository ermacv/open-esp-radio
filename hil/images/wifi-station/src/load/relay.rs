//! What the load server on hart 1 needs from the boot hart: the PC
//! sampler, which samples the boot hart, the boot hart's cycle,
//! instruction and cache counters with the library's counts read beside
//! them, the receive path's stall snapshot, which the station's radio
//! device on the boot hart takes, and the console for a page of samples.
//!
//! The server sends one [`Command`] at a time over [`RELAY`] and waits for
//! its [`Reply`] where it has one; the relay on the application executor
//! serves them in order.

use core::convert::Infallible;

#[cfg(feature = "trace")]
use oer::observe::{self, Sampler};
#[cfg(feature = "trace")]
use oer::time::{Instant, Timer};
use oer::{ApplicationEnd, Channel, GroupEnd};
#[cfg(feature = "trace")]
use oer_hil_services_load::STALL_WAIT_MS;
#[cfg(feature = "trace")]
use oer_hil_services_load::{LoadError, SamplesCollected};
use oer_hil_services_load::{RxSnapshot, StallMac};

use crate::measure::Hart0;
#[cfg(feature = "trace")]
use oer_protocols_hil_protocol::RequestIdentity;

use super::report::Snapshot;

/// What the load server asks the boot hart.
pub(crate) enum Command {
    /// Whether a sampled run can start.
    #[cfg(feature = "trace")]
    Available,
    /// Forget the last session's samples and sample every `period_us`.
    #[cfg(feature = "trace")]
    Start { period_us: u32 },
    /// Ask the running session to stop at its next tick: no reply.
    #[cfg(feature = "trace")]
    RequestStop,
    /// Stop the session and take it back.
    #[cfg(feature = "trace")]
    Stop,
    /// The boot hart's counters and the library's counts now; with
    /// `start`, the cache's counters cleared and running first.
    Counters { start: bool },
    /// Answer `RequestIdentity` with the last session's samples from the
    /// one numbered `from`: no reply.
    #[cfg(feature = "trace")]
    Page(RequestIdentity, u16),
    /// The boot hart's idle account, read there.
    Idle,
    /// Ask the radio device for a stall snapshot and wait for it and its
    /// facts after the turn's receive pull, at most [`STALL_WAIT_MS`].
    Stall,
}

/// The boot hart's reply to a command that has one.
#[allow(
    clippy::large_enum_variant,
    reason = "a reply lives in place in its static queue until its one reader takes it; the image has no allocator"
)]
pub(crate) enum Reply {
    #[cfg(feature = "trace")]
    Sampler(Result<(), LoadError>),
    /// One end of a window, without the network's frames, which hart 1
    /// counts.
    Counters(Snapshot),
    /// The radio device's stall snapshot, `None` when it took none in
    /// time, and the MAC's banks it read.
    Stall(Option<RxSnapshot>, StallMac),
    /// The boot hart's idle account.
    Idle(oer::cpu::IdleReading),
}

/// The relay's channel: replies to hart 1, commands back.
pub(crate) static RELAY: Channel<Reply, Command, 1> = Channel::new();

/// The relay's end, on the boot hart.
pub(crate) type Relay = ApplicationEnd<Reply, Command, 1>;
/// The load server's end, on hart 1.
pub(crate) type Client = GroupEnd<Reply, Command, 1>;

/// Serve the load server's commands with the boot hart's `hart0` until the
/// chip resets.
#[cfg(feature = "trace")]
pub(crate) async fn serve(mut relay: Relay, mut hart0: Hart0) -> Infallible {
    loop {
        let reply = match relay.receive().await {
            Command::Available => Reply::Sampler(available(hart0.observe.sampler())),
            Command::Start { period_us } => Reply::Sampler(
                hart0
                    .observe
                    .sampler()
                    .start(period_us)
                    .map_err(|_| LoadError::NoSampler),
            ),
            Command::RequestStop => {
                hart0.observe.sampler().request_stop();
                continue;
            }
            Command::Stop => Reply::Sampler(
                hart0
                    .observe
                    .sampler()
                    .stop()
                    .await
                    .map_err(|_| LoadError::NoSampler),
            ),
            Command::Counters { start } => {
                // The counters first: the ADDBA reading may yield.
                let (hart, counters) = (hart0.cpu.counters(start), observe::counters());
                let addba = observe::addba_requests().await;
                Reply::Counters(super::report::snapshot(hart, counters, &addba))
            }
            Command::Page(identity, from) => {
                oer_hil_agent::answer(identity, &page(hart0.observe.sampler(), from));
                continue;
            }
            Command::Stall => {
                let (snapshot, mac) = stall(&mut hart0).await;
                Reply::Stall(snapshot, mac)
            }
            Command::Idle => Reply::Idle(hart0.cpu.idle()),
        };
        relay.send(reply).await;
    }
}

/// Serve the load server's commands with the boot hart's `hart0` until the
/// chip resets: the measuring build has no sampler and takes no stall
/// snapshot, and its counters are the boot hart's own.
#[cfg(not(feature = "trace"))]
pub(crate) async fn serve(mut relay: Relay, mut hart0: Hart0) -> Infallible {
    loop {
        let reply = match relay.receive().await {
            Command::Counters { start } => {
                Reply::Counters(super::report::cpu_snapshot(hart0.cpu.counters(start)))
            }
            Command::Stall => Reply::Stall(None, StallMac::default()),
            Command::Idle => Reply::Idle(hart0.cpu.idle()),
        };
        relay.send(reply).await;
    }
}

#[cfg(feature = "trace")]
/// How often the relay looks for the device's snapshot while it waits.
const STALL_POLL_US: u64 = 500;

/// Ask the radio device for a stall snapshot and take it once the device
/// recorded its facts after the turn's receive pull, or as it stands when
/// [`STALL_WAIT_MS`] passed; each time is read against this hart's clock
/// beside the device's.
#[cfg(feature = "trace")]
async fn stall(hart0: &mut Hart0) -> (Option<RxSnapshot>, StallMac) {
    let ticket = hart0.observe.request_rx_snapshot();
    let deadline = now_us().saturating_add(u64::from(STALL_WAIT_MS).saturating_mul(1000));
    let mut taken = None;
    loop {
        if let Some(reading) = hart0.observe.rx_snapshot(ticket) {
            let now = now_us();
            let served = reading.resume.is_some_and(|resume| resume.serviced);
            taken = Some(super::report::stall(&reading, now));
            if served {
                break;
            }
        }
        let now = now_us();
        if now >= deadline {
            break;
        }
        Timer::at(Instant::from_micros(now.saturating_add(STALL_POLL_US))).await;
    }
    match taken {
        Some((snapshot, mac)) => (Some(snapshot), mac),
        None => (None, StallMac::default()),
    }
}

#[cfg(feature = "trace")]
/// The image's monotonic microseconds: the library's clock.
fn now_us() -> u64 {
    Instant::now().as_micros()
}

#[cfg(feature = "trace")]
fn available(sampler: &Sampler) -> Result<(), LoadError> {
    if sampler.available() {
        Ok(())
    } else {
        Err(LoadError::NoSampler)
    }
}

#[cfg(feature = "trace")]
/// The last session's page from the sample numbered `from`.
fn page(sampler: &Sampler, from: u16) -> SamplesCollected {
    let page = sampler.session().and_then(|session| {
        sampler.samples(|buffer| {
            super::report::samples(buffer, from, session.ticks, session.stray, session.cut)
        })
    });
    page.map_or(
        SamplesCollected::Refused(LoadError::NoSampler),
        SamplesCollected::Page,
    )
}

/// Ask the boot hart `command` and take its reply.
async fn call(client: &mut Client, command: Command) -> Reply {
    client.send(command).await;
    client.receive().await
}

#[cfg(feature = "trace")]
/// Whether a sampled run can start.
///
/// # Errors
/// [`LoadError::NoSampler`] when it cannot.
pub(crate) async fn sampler_available(client: &mut Client) -> Result<(), LoadError> {
    sampler(call(client, Command::Available).await)
}

#[cfg(feature = "trace")]
/// Start sampling the boot hart every `period_us`.
///
/// # Errors
/// [`LoadError::NoSampler`] when the sampler cannot start.
pub(crate) async fn start_sampler(client: &mut Client, period_us: u32) -> Result<(), LoadError> {
    sampler(call(client, Command::Start { period_us }).await)
}

#[cfg(feature = "trace")]
/// Ask the running session to stop at its next tick.
pub(crate) async fn request_stop(client: &mut Client) {
    client.send(Command::RequestStop).await;
}

#[cfg(feature = "trace")]
/// Stop the running session and take it back.
///
/// # Errors
/// [`LoadError::NoSampler`] when the sampler's handler kept it.
pub(crate) async fn stop_sampler(client: &mut Client) -> Result<(), LoadError> {
    sampler(call(client, Command::Stop).await)
}

/// The boot hart's counters and the library's counts, read there now;
/// with `start`, the cache's counters cleared first.
pub(crate) async fn counters(client: &mut Client, start: bool) -> Option<Snapshot> {
    match call(client, Command::Counters { start }).await {
        Reply::Counters(snapshot) => Some(snapshot),
        #[cfg(feature = "trace")]
        Reply::Sampler(_) => None,
        Reply::Stall(..) | Reply::Idle(_) => None,
    }
}

/// The radio device's stall snapshot, asked now, and the MAC's banks it
/// read; `None` when the relay answered otherwise, which it does not.
pub(crate) async fn stall_snapshot(client: &mut Client) -> Option<(Option<RxSnapshot>, StallMac)> {
    match call(client, Command::Stall).await {
        Reply::Stall(snapshot, mac) => Some((snapshot, mac)),
        #[cfg(feature = "trace")]
        Reply::Sampler(_) => None,
        Reply::Counters(_) | Reply::Idle(_) => None,
    }
}

/// The boot hart's idle account, read there now; `None` when the relay
/// answered otherwise, which it does not.
pub(crate) async fn idle(client: &mut Client) -> Option<oer::cpu::IdleReading> {
    match call(client, Command::Idle).await {
        Reply::Idle(reading) => Some(reading),
        #[cfg(feature = "trace")]
        Reply::Sampler(_) => None,
        Reply::Counters(_) | Reply::Stall(..) => None,
    }
}

#[cfg(feature = "trace")]
/// Have the boot hart answer `identity` with a page of samples.
pub(crate) async fn page_of(client: &mut Client, identity: RequestIdentity, from: u16) {
    client.send(Command::Page(identity, from)).await;
}

#[cfg(feature = "trace")]
/// A sampler call's result; the relay answers each call in order, so a
/// reply of another kind does not arise and refuses the call.
fn sampler(reply: Reply) -> Result<(), LoadError> {
    match reply {
        Reply::Sampler(result) => result,
        Reply::Counters(_) | Reply::Stall(..) | Reply::Idle(_) => Err(LoadError::NoSampler),
    }
}
