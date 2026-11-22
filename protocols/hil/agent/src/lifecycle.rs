//! Generic lifecycle service. The image owns the terminal reset action.
use crate::console::{Console, Dropped, Immediate, progress::ReceiverLimit};
use oer_protocols_hil_protocol::{RequestIdentity, lifecycle::ResetAccepted};

crate::requests! {
    /// Mounted only by images that can perform a requested reset.
    pub enum Request (sessions = false) {
        Reset(oer_protocols_hil_protocol::lifecycle::RequestReset),
    }
}

/// Proof that the reset acknowledgement reached the console's endpoint.
/// Constructed only after the completion barrier succeeds.
///
/// ```compile_fail
/// use oer_protocols_hil_agent::lifecycle::AcknowledgedReset;
/// use oer_protocols_hil_protocol::RequestIdentity;
/// let acknowledged = AcknowledgedReset {
///     request: RequestIdentity { boot_id: 1, session_id: 0, request_id: 1 },
///     place: 1,
/// };
/// ```
#[derive(Debug)]
pub struct AcknowledgedReset {
    request: RequestIdentity,
    /// The acknowledgement's place among the console's entries.
    place: u32,
}

impl AcknowledgedReset {
    /// Retained correlation: request, the acknowledgement's place among the
    /// console's entries, session low/high.
    pub fn observations(self) -> [u32; 4] {
        [
            self.request.request_id,
            self.place,
            self.request.session_id as u32,
            (self.request.session_id >> 32) as u32,
        ]
    }
}

/// Why the reset acknowledgement is not known written: the image keeps the
/// observations and performs no reset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Unacknowledged {
    /// The acknowledgement could not be built: nothing was queued.
    Dropped,
    /// The completion barrier's fan-out is exhausted.
    Barrier(ReceiverLimit),
}

/// Publish the reset acknowledgement and wait for its write. A barrier
/// failure returns its observations to the image without authorizing reset.
pub async fn acknowledge_reset<const N: usize, const M: usize, P: Immediate>(
    console: &Console<N, M, P>,
    request: RequestIdentity,
) -> Result<AcknowledgedReset, Unacknowledged> {
    let entered = console
        .publish_reliably(request.session_id, request.request_id, &ResetAccepted)
        .await
        .map_err(|Dropped| Unacknowledged::Dropped)?;
    console
        .written(entered)
        .await
        .map_err(Unacknowledged::Barrier)?;
    Ok(AcknowledgedReset {
        request,
        place: entered.place(),
    })
}
