use crate::{CoexClient, CoexClockHardware, CoexError, CoexEventId, CoexPti, CoexTimerIndex};

/// Finite hardware transactions, not RF admission.
///
/// An error may follow a partial hardware write. The core must retain a
/// cleanup obligation until `disable` succeeds. Successful `disable` reports
/// completion of the backend's withdrawal transaction, not an observed RF
/// grant revocation or whole-radio quiescence.
pub trait CoexTimerHardware {
    /// The radio arbiter's current priority of `event`.
    fn pti(&mut self, event: CoexEventId) -> CoexPti;
    fn configure_request(
        &mut self,
        index: CoexTimerIndex,
        client: CoexClient,
        pti: CoexPti,
    ) -> Result<(), CoexError>;
    fn set_primary_target(
        &mut self,
        index: CoexTimerIndex,
        tick_image: u32,
    ) -> Result<(), CoexError>;
    fn set_secondary_target(
        &mut self,
        index: CoexTimerIndex,
        tick_image: u32,
    ) -> Result<(), CoexError>;
    fn enable(&mut self, index: CoexTimerIndex) -> Result<(), CoexError>;
    fn disable(&mut self, index: CoexTimerIndex) -> Result<(), CoexError>;
    fn force(&mut self, index: CoexTimerIndex) -> Result<(), CoexError>;
    fn unforce(&mut self, index: CoexTimerIndex) -> Result<(), CoexError>;
}

/// Program one hardware timer exactly like `coex_hw_timer_set`.
///
/// Enabling the timer is deliberately separate because the vendor core first
/// completes all four fresh-read RMW operations and only then publishes the
/// timer through `coex_hw_timer_enable`.
pub fn program_timer<H: CoexTimerHardware, C: CoexClockHardware>(
    hardware: &mut H,
    clock: &mut C,
    index: CoexTimerIndex,
    client: CoexClient,
    pti: CoexPti,
    latency: u32,
    duration: u32,
) -> Result<(), CoexError> {
    hardware.configure_request(index, client, pti)?;
    let primary = clock.sample()?.tick_image(duration)?;
    hardware.set_primary_target(index, primary)?;
    let secondary = clock.sample()?.tick_image(latency)?;
    hardware.set_secondary_target(index, secondary)
}
