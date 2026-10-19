//! Controller-time latch ownership over the PAC's single transactions.
//!
//! Publication performs one fresh-read RMW. Every later step observes the
//! request bit exactly once and returns immediately; only the step which sees
//! the self-clear edge reads the first latched-time word. The HAL stores the
//! in-flight state beside the unique task owner, so cancelling a higher async
//! worker cannot accidentally authorize a second request.

use oer_esp32s31_pac::{BluetoothControllerLatchedTime, BluetoothTaskRegisters};

/// A controller-time transaction has not yet been drained by software.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothControllerTimeLatchBeginError {
    /// The previous request must be completed before another is published.
    AlreadyInFlight,
}

/// No controller-time request is available for one event step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothControllerTimeLatchStepError {
    /// A request must be published before its self-clear edge is observed.
    NotInFlight,
}

/// One controller-time latch request for the always-awake timer path.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BluetoothControllerTimeLatchRequest;

impl BluetoothControllerTimeLatchRequest {
    /// Construct one request without touching MMIO.
    pub const fn new() -> Self {
        Self
    }
}

/// Result of exactly one live controller-time event step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "a waiting latch must be revisited after another event"]
pub enum BluetoothControllerTimeLatchStep {
    /// Hardware still owns the request; the caller must yield.
    Waiting,
    /// Hardware cleared the request and the first latched word was read once.
    Ready(BluetoothControllerLatchedTime),
}

/// Sticky software ownership retained by the unique Bluetooth task owner.
///
/// An async operation may disappear while hardware still owns a request. Drop
/// therefore performs no implicit MMIO and does not reset this state. The
/// durable upper task owner can continue with `step_controller_time_latch`; a
/// second begin fails closed until the original request reaches `Ready`.
pub(crate) struct ControllerTimeLatch {
    in_flight: bool,
}

impl ControllerTimeLatch {
    pub(crate) const fn new() -> Self {
        Self { in_flight: false }
    }

    #[cfg(test)]
    pub(crate) fn begin_for_test(&mut self) {
        self.in_flight = true;
    }

    fn begin(&mut self) -> Result<(), BluetoothControllerTimeLatchBeginError> {
        if self.in_flight {
            return Err(BluetoothControllerTimeLatchBeginError::AlreadyInFlight);
        }
        self.in_flight = true;
        Ok(())
    }

    fn require_in_flight(&self) -> Result<(), BluetoothControllerTimeLatchStepError> {
        if self.in_flight {
            Ok(())
        } else {
            Err(BluetoothControllerTimeLatchStepError::NotInFlight)
        }
    }

    fn complete(&mut self) {
        debug_assert!(self.in_flight);
        self.in_flight = false;
    }

    pub(crate) const fn in_flight(&self) -> bool {
        self.in_flight
    }
}

trait BluetoothControllerTimeLatchControl {
    fn publish_latch_request(&mut self, request: BluetoothControllerTimeLatchRequest);
    fn order_after_publication(&mut self);
    fn latch_request_pending(&mut self) -> bool;
    fn order_after_clear_observation(&mut self);
    fn read_latched_time_0(&mut self) -> u32;
}

fn execute_latch_publication(
    ownership: &mut ControllerTimeLatch,
    control: &mut impl BluetoothControllerTimeLatchControl,
    request: BluetoothControllerTimeLatchRequest,
) -> Result<(), BluetoothControllerTimeLatchBeginError> {
    ownership.begin()?;
    control.publish_latch_request(request);
    control.order_after_publication();
    Ok(())
}

fn execute_latch_step(
    ownership: &mut ControllerTimeLatch,
    control: &mut impl BluetoothControllerTimeLatchControl,
) -> Result<BluetoothControllerTimeLatchStep, BluetoothControllerTimeLatchStepError> {
    ownership.require_in_flight()?;
    if control.latch_request_pending() {
        Ok(BluetoothControllerTimeLatchStep::Waiting)
    } else {
        control.order_after_clear_observation();
        let step = BluetoothControllerTimeLatchStep::Ready(
            BluetoothControllerLatchedTime::from_bits(control.read_latched_time_0()),
        );
        ownership.complete();
        Ok(step)
    }
}

impl BluetoothControllerTimeLatchControl for BluetoothTaskRegisters {
    fn publish_latch_request(&mut self, _request: BluetoothControllerTimeLatchRequest) {
        // The PAC publication includes its trailing device fence.
        self.publish_controller_time_latch_request();
    }

    fn order_after_publication(&mut self) {}

    fn latch_request_pending(&mut self) -> bool {
        self.controller_time_latch_request_pending()
    }

    fn order_after_clear_observation(&mut self) {}

    fn read_latched_time_0(&mut self) -> u32 {
        // The PAC read includes the fence ordering it after the clear edge.
        self.read_controller_latched_time().bits()
    }
}

/// Publish one controller-time latch request owned by `latch`.
pub(crate) fn begin(
    latch: &mut ControllerTimeLatch,
    registers: &mut BluetoothTaskRegisters,
) -> Result<(), BluetoothControllerTimeLatchBeginError> {
    execute_latch_publication(latch, registers, BluetoothControllerTimeLatchRequest::new())
}

/// Perform at most one observation of the in-flight latch request.
pub(crate) fn step(
    latch: &mut ControllerTimeLatch,
    registers: &mut BluetoothTaskRegisters,
) -> Result<BluetoothControllerTimeLatchStep, BluetoothControllerTimeLatchStepError> {
    execute_latch_step(latch, registers)
}

#[cfg(test)]
mod tests;
