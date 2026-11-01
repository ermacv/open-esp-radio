//! What the load server on hart 1 needs from the boot hart: the PC
//! sampler, which samples the boot hart, the boot hart's cycle,
//! instruction and cache counters with the library's counts read beside
//! them, and the console for a page of samples.
//!
//! The server sends one [`Command`] at a time over [`RELAY`] and waits for
//! its [`Reply`] where it has one; the relay on the application executor
//! serves them in order.

use core::convert::Infallible;

use oer_espressif::observe::{self, Hart0, Sampler};
use oer_espressif::{ApplicationEnd, Channel, SecondEnd};
use oer_hil_protocol::RequestIdentity;
use oer_services_load::{LoadError, SamplesCollected};

use super::report::Snapshot;

/// What the load server asks the boot hart.
pub(crate) enum Command {
    /// Whether a sampled run can start.
    Available,
    /// Forget the last session's samples and sample every `period_us`.
    Start { period_us: u32 },
    /// Ask the running session to stop at its next tick: no reply.
    RequestStop,
    /// Stop the session and take it back.
    Stop,
    /// The boot hart's counters and the library's counts now; with
    /// `start`, the cache's counters cleared and running first.
    Counters { start: bool },
    /// Answer `RequestIdentity` with the last session's samples from the
    /// one numbered `from`: no reply.
    Page(RequestIdentity, u16),
}

/// The boot hart's reply to a command that has one.
#[allow(
    clippy::large_enum_variant,
    reason = "a reply lives in place in its static queue until its one reader takes it; the image has no allocator"
)]
pub(crate) enum Reply {
    Sampler(Result<(), LoadError>),
    /// One end of a window, without the IP stack's frames, which hart 1
    /// counts.
    Counters(Snapshot),
}

/// The relay's channel: replies to hart 1, commands back.
pub(crate) static RELAY: Channel<Reply, Command, 1> = Channel::new();

/// The relay's end, on the boot hart.
pub(crate) type Relay = ApplicationEnd<Reply, Command, 1>;
/// The load server's end, on hart 1.
pub(crate) type Client = SecondEnd<Reply, Command, 1>;

/// Serve the load server's commands with the boot hart's `hart0` until the
/// chip resets.
pub(crate) async fn serve(mut relay: Relay, mut hart0: Hart0) -> Infallible {
    loop {
        let reply = match relay.receive().await {
            Command::Available => Reply::Sampler(available(hart0.sampler())),
            Command::Start { period_us } => Reply::Sampler(
                hart0
                    .sampler()
                    .start(period_us)
                    .map_err(|_| LoadError::NoSampler),
            ),
            Command::RequestStop => {
                hart0.sampler().request_stop();
                continue;
            }
            Command::Stop => Reply::Sampler(
                hart0
                    .sampler()
                    .stop()
                    .await
                    .map_err(|_| LoadError::NoSampler),
            ),
            Command::Counters { start } => Reply::Counters(super::report::snapshot(
                hart0.counters(start),
                observe::counters(),
            )),
            Command::Page(identity, from) => {
                oer_hil_agent_espressif::answer(identity, &page(hart0.sampler(), from));
                continue;
            }
        };
        relay.send(reply).await;
    }
}

fn available(sampler: &Sampler) -> Result<(), LoadError> {
    if sampler.available() {
        Ok(())
    } else {
        Err(LoadError::NoSampler)
    }
}

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

/// Whether a sampled run can start.
///
/// # Errors
/// [`LoadError::NoSampler`] when it cannot.
pub(crate) async fn sampler_available(client: &mut Client) -> Result<(), LoadError> {
    sampler(call(client, Command::Available).await)
}

/// Start sampling the boot hart every `period_us`.
///
/// # Errors
/// [`LoadError::NoSampler`] when the sampler cannot start.
pub(crate) async fn start_sampler(client: &mut Client, period_us: u32) -> Result<(), LoadError> {
    sampler(call(client, Command::Start { period_us }).await)
}

/// Ask the running session to stop at its next tick.
pub(crate) async fn request_stop(client: &mut Client) {
    client.send(Command::RequestStop).await;
}

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
        Reply::Sampler(_) => None,
    }
}

/// Have the boot hart answer `identity` with a page of samples.
pub(crate) async fn page_of(client: &mut Client, identity: RequestIdentity, from: u16) {
    client.send(Command::Page(identity, from)).await;
}

/// A sampler call's result; the relay answers each call in order, so a
/// reply of another kind does not arise and refuses the call.
fn sampler(reply: Reply) -> Result<(), LoadError> {
    match reply {
        Reply::Sampler(result) => result,
        Reply::Counters(_) => Err(LoadError::NoSampler),
    }
}
