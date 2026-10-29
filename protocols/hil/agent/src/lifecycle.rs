//! Generic lifecycle service. The image owns the terminal reset action.
use crate::console::{Console, Immediate, progress::ReceiverLimit};
use oer_hil_protocol::{RequestIdentity, lifecycle::ResetAccepted};

crate::requests! {
    /// Mounted only by images that can perform a requested reset.
    pub enum Request (sessions = false) {
        Reset(oer_hil_protocol::lifecycle::RequestReset),
    }
}

/// Proof that the reset acknowledgement reached the console's endpoint.
/// Constructed only after the completion barrier succeeds.
///
/// ```compile_fail
/// use oer_hil_agent::lifecycle::AcknowledgedReset;
/// use oer_hil_protocol::RequestIdentity;
/// let acknowledged = AcknowledgedReset {
///     request: RequestIdentity { boot_id: 1, session_id: 0, request_id: 1 },
///     sequence: 1,
/// };
/// ```
#[derive(Debug)]
pub struct AcknowledgedReset {
    request: RequestIdentity,
    sequence: u32,
}

impl AcknowledgedReset {
    /// Retained correlation: request, acknowledgement, session low/high.
    pub fn observations(self) -> [u32; 4] {
        [
            self.request.request_id,
            self.sequence,
            self.request.session_id as u32,
            (self.request.session_id >> 32) as u32,
        ]
    }
}

/// Publish the reset acknowledgement and wait for its write. A barrier
/// failure returns its observations to the image without authorizing reset.
pub async fn acknowledge_reset<const N: usize, const M: usize, P: Immediate>(
    console: &Console<N, M, P>,
    request: RequestIdentity,
) -> Result<AcknowledgedReset, ReceiverLimit> {
    let sequence = console
        .publish_reliably(request.session_id, request.request_id, &ResetAccepted)
        .await;
    console.written(sequence).await?;
    Ok(AcknowledgedReset { request, sequence })
}
