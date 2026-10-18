//! A coexistence leave that could not withdraw a request timer.
//!
//! The participant set and the per-client states
//! ([`oer_esp32s31_hal::shared_radio::CoexAbsent`],
//! [`oer_esp32s31_hal::shared_radio::Coexisting`]) belong to the arbiter;
//! the radio system applies the coexistence policy to them.

/// Leaving coexistence could not withdraw a request timer. The participant
/// still takes part and the core keeps the timer as uncertain; the proof
/// stays with the caller, so the client cannot leave power, and a retry
/// withdraws the requests again.
#[derive(Debug)]
pub struct CoexLeaveFailure<Proof, Error> {
    /// The proof that still takes part in coexistence.
    pub proof: Proof,
    /// Why the requests could not be withdrawn.
    pub error: Error,
}
