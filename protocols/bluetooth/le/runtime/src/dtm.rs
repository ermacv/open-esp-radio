//! Capacity and terminal waits around the durable synchronous controller.

use oer_bluetooth_le_controller::dtm::{
    Admission, Fault, RequestRefusal, Service, Submission, SubmitError, Submitted,
};
use oer_bluetooth_le_port::{LeCapabilities, LePhase};
use oer_radio_port::PoolPath;

/// Wait for capacity, then perform one uninterrupted synchronous admission.
/// Cancellation while waiting retains the command, seat and lease in `service`.
pub async fn submit<'p, P: PoolPath, C: LeCapabilities, F>(
    service: &mut Service<'p, P, C>,
    phase: &impl LePhase<'p, P, (), Caps = C, Fault = F>,
) -> Result<Submitted, SubmitError<F>> {
    let capacity = match service.submission()? {
        Submission::Done(result) => return Ok(result),
        Submission::Capacity(seat) => phase.take(seat).await,
    };
    let request = match service.request(capacity) {
        Ok(request) => request,
        Err(RequestRefusal { capacity, error }) => {
            phase.release(capacity);
            return Err(error);
        }
    };
    let Admission { release, result } = service.submitted(phase.submit(request));
    if let Some(capacity) = release {
        phase.release(capacity);
    }
    result
}

/// Wait for the admitted command's terminal. Dropping this wait detaches only
/// the wait; the durable command and mandatory terminal obligation survive.
pub async fn complete<'p, P: PoolPath, C: LeCapabilities, F>(
    service: &mut Service<'p, P, C>,
    phase: &mut impl LePhase<'p, P, (), Caps = C, Fault = F>,
) -> Result<(), Fault> {
    service.completion()?;
    service.complete_with(phase.next_event().await)
}

#[cfg(test)]
mod tests;
