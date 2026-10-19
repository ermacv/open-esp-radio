//! The intentional panic the console serves on `InjectPanic`: the HIL panic
//! hook records the post-mortem fault, and the next boot reports it.
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use oer_hil_protocol::RequestIdentity;
use oer_hil_protocol::base::{RejectReason, Rejected};
use oer_hil_protocol::system::PanicInjected;

use super::console::CONSOLE;

/// The panic request the console handed over.
static PANICS: Channel<CriticalSectionRawMutex, RequestIdentity, 1> = Channel::new();

/// Hand the request to [`panic_on_request`], or refuse it while one waits.
pub(crate) fn queue(request: RequestIdentity) {
    if PANICS.try_send(request).is_err() {
        CONSOLE.publish(
            request.session_id,
            request.request_id,
            &Rejected(RejectReason::Busy),
        );
    }
}

/// Acknowledges the request, then panics in thread context: the location
/// is this file, which the host checks in the next boot's evidence.
pub(crate) async fn panic_on_request() -> ! {
    let request = PANICS.receive().await;
    let sequence = CONSOLE
        .publish_reliably(request.session_id, request.request_id, &PanicInjected)
        .await;
    // The host must see the acknowledgement before the reset.
    CONSOLE.written(sequence).await;
    panic!("intentional HIL panic");
}
