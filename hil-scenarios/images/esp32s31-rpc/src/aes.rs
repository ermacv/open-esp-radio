//! The AES service: the console hands each request to [`serve`], which owns
//! the product key-unwrap engine and answers one request at a time.

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use oer_hil_protocol::RequestIdentity;
use oer_hil_protocol::base::{RejectReason, Rejected};
use oer_services_aes::Request;

use crate::transport::CONSOLE;

/// The engine the service owns.
pub(crate) type Engine = oer_services_aes::Engine<oer_esp32s31_compose::crypto::KeyUnwrapEngine>;

/// The request the console handed over.
static REQUESTS: Channel<CriticalSectionRawMutex, (RequestIdentity, Request), 1> = Channel::new();

/// Hand `request` to [`serve`], or refuse it while one waits.
pub(crate) fn queue(identity: RequestIdentity, request: Request) {
    if REQUESTS.try_send((identity, request)).is_err() {
        CONSOLE.publish(
            identity.session_id,
            identity.request_id,
            &Rejected(RejectReason::Busy),
        );
    }
}

/// Answer each queued request through `engine`.
pub(crate) async fn serve(mut engine: Engine) {
    loop {
        let (identity, request) = REQUESTS.receive().await;
        request.serve(&mut engine, &CONSOLE, identity).await;
    }
}
