//! The explicit host-requested reset served by `RequestReset`. The staged
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
    let acknowledged = match oer_hil_agent::lifecycle::acknowledge_reset(&CONSOLE, request).await {
        Ok(acknowledged) => acknowledged,
        Err(fault) => crate::system::receiver_limit(fault),
    };
    reset_fault::<oer_esp32s31_hal::platform::Port>(
        FaultReason::HilRequested,
        acknowledged.observations(),
    );
}
