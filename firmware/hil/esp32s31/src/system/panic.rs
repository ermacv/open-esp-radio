//! The explicit host-requested reset served by `InjectPanic`. The staged
//! runtime retains `HilRequested` and reports it on the next boot. This is
//! a requested reset, not a hardware fault or a call to the panic hook.
#[cfg(target_arch = "riscv32")]
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
#[cfg(target_arch = "riscv32")]
use oer_espressif_staged_runtime::panic::{FaultReason, reset_fault};
use oer_hil_protocol::RequestIdentity;
#[cfg(target_arch = "riscv32")]
use oer_hil_protocol::base::{RejectReason, Rejected};
#[cfg(target_arch = "riscv32")]
use oer_hil_protocol::system::PanicInjected;

#[cfg(target_arch = "riscv32")]
use crate::transport::CONSOLE;

/// The reset request the console handed over.
#[cfg(target_arch = "riscv32")]
static RESETS: Channel<CriticalSectionRawMutex, RequestIdentity, 1> = Channel::new();

/// Hand the request to [`reset_on_request`], or refuse it while one waits.
#[cfg(target_arch = "riscv32")]
pub(crate) fn queue(request: RequestIdentity) {
    if RESETS.try_send(request).is_err() {
        CONSOLE.publish(
            request.session_id,
            request.request_id,
            &Rejected(RejectReason::Busy),
        );
    }
}

/// Acknowledge the request before recording its identity and resetting.
#[cfg(target_arch = "riscv32")]
pub(crate) async fn reset_on_request() {
    let request = RESETS.receive().await;
    let sequence = CONSOLE
        .publish_reliably(request.session_id, request.request_id, &PanicInjected)
        .await;
    // The host must see the acknowledgement before the reset.
    if let Err(fault) = CONSOLE.written(sequence).await {
        crate::system::receiver_limit(fault);
    }
    reset_fault::<oer_esp32s31_hal::platform::Port>(
        FaultReason::HilRequested,
        observations(request, sequence),
    );
}

/// The retained report correlates the request and its written acknowledgement.
fn observations(request: RequestIdentity, sequence: u32) -> [u32; 4] {
    [
        request.request_id,
        sequence,
        request.session_id as u32,
        (request.session_id >> 32) as u32,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_request_keeps_the_acknowledgement_and_both_session_words() {
        let request = RequestIdentity {
            boot_id: 42,
            session_id: 0x89ab_cdef_0123_4567,
            request_id: u32::MAX,
        };
        assert_eq!(
            observations(request, 0x7654_3210),
            [u32::MAX, 0x7654_3210, 0x0123_4567, 0x89ab_cdef],
        );
    }
}
