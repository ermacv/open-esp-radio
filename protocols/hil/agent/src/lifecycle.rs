//! Generic lifecycle service. The image owns the terminal reset action.
use crate::console::{Dropped, Producer};
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

/// Publish the reset acknowledgement through `producer` and wait for its
/// write.
///
/// # Errors
/// [`Dropped`] when the acknowledgement could not be built: nothing was
/// queued, and the image performs no reset.
pub async fn acknowledge_reset<const DEPTH: usize>(
    producer: &mut Producer<'_, DEPTH>,
    request: RequestIdentity,
) -> Result<AcknowledgedReset, Dropped> {
    let entered = producer
        .publish_reliably(request.session_id, request.request_id, &ResetAccepted)
        .await?;
    producer.written(entered).await;
    Ok(AcknowledgedReset {
        request,
        place: entered.place(),
    })
}
