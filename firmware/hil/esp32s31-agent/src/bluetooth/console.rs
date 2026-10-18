//! The HIL console of the Bluetooth images: the runtime's console, with the
//! image's profile serving the requests beyond the base module and the
//! common ones: interrupt stacks and the event trace.

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use oer_hil_agent::base::{Either, Requests};
use oer_hil_protocol::base::{RejectReason, Rejected};
use oer_hil_protocol::system::{GetInterruptStacks, InterruptStacks};
#[cfg(trace_journal)]
use oer_hil_protocol::telemetry::{
    ControlTrace, GetTraceEntries, GetTraceSnapshot, TraceEntriesPage, TraceSnapshot, TraceState,
};
use oer_hil_protocol::{Message, RequestIdentity};

use crate::transport::CONSOLE;

oer_hil_agent::requests! {
    /// What every Bluetooth image serves beyond the base module.
    pub(super) enum Common (sessions = false) {
        InterruptStacks(GetInterruptStacks),
        #[cfg(trace_journal)]
        ControlTrace(ControlTrace),
        #[cfg(trace_journal)]
        GetTraceEntries(GetTraceEntries),
        #[cfg(trace_journal)]
        GetTraceSnapshot(GetTraceSnapshot),
    }
}

/// Requests the console handed over, served in order.
pub(super) type Queue<R> =
    Channel<CriticalSectionRawMutex, (RequestIdentity, Either<Common, R>), 4>;

/// What one image adds to the console.
pub(super) trait Profile {
    /// The image's own requests; none belongs to a session.
    type Request: Requests + 'static;

    /// The image's request queue.
    fn queue(&self) -> &'static Queue<Self::Request>;

    fn maximum_payload_bytes(&self) -> u16;

    /// Serve one request and publish its reply.
    async fn serve(&self, request: RequestIdentity, body: Self::Request);
}

/// Publish `reply` to `request`: its response, or the reason it was refused.
pub(super) async fn respond<M: Message>(request: RequestIdentity, reply: Result<M, RejectReason>) {
    match reply {
        Ok(response) => {
            CONSOLE
                .publish_reliably(request.session_id, request.request_id, &response)
                .await
        }
        Err(reason) => {
            CONSOLE
                .publish_reliably(request.session_id, request.request_id, &Rejected(reason))
                .await
        }
    };
}

/// Serve the host over `usb`, handing the image's requests to its queue;
/// [`serve_requests`] serves them, in a task of its own.
pub(super) async fn serve_console<P: Profile>(
    usb: crate::transport::Usb,
    boot: u64,
    profile: &P,
) -> ! {
    let requests = profile.queue();
    crate::transport::serve(
        usb,
        boot,
        profile.maximum_payload_bytes(),
        |request, body: Either<Common, P::Request>| {
            if requests.try_send((request, body)).is_err() {
                CONSOLE.publish(
                    request.session_id,
                    request.request_id,
                    &Rejected(RejectReason::Busy),
                );
            }
        },
    )
    .await
}

/// Serve the requests the console queued for `profile`, in order.
pub(super) async fn serve_requests<P: Profile>(profile: &P) -> ! {
    let requests = profile.queue();
    loop {
        match requests.receive().await {
            // Only CPU0 runs in the Bluetooth images.
            (request, Either::First(Common::InterruptStacks(GetInterruptStacks))) => {
                respond(
                    request,
                    Ok(InterruptStacks {
                        cpu0: crate::stack_evidence::current_irq_snapshot(),
                        cpu1: None,
                    }),
                )
                .await;
            }
            // The event trace of an image that keeps one, read and
            // controlled as every image does.
            #[cfg(trace_journal)]
            (request, Either::First(Common::ControlTrace(ControlTrace(control)))) => {
                respond(request, Ok(TraceState(crate::trace::control(control)))).await;
            }
            #[cfg(trace_journal)]
            (request, Either::First(Common::GetTraceEntries(GetTraceEntries { first }))) => {
                respond(request, Ok(TraceEntriesPage(crate::trace::entries(first)))).await;
            }
            #[cfg(trace_journal)]
            (
                request,
                Either::First(Common::GetTraceSnapshot(GetTraceSnapshot { slot, offset })),
            ) => {
                respond(
                    request,
                    Ok(TraceSnapshot(crate::trace::snapshot(slot, offset))),
                )
                .await;
            }
            (request, Either::Second(body)) => profile.serve(request, body).await,
        }
    }
}
