//! What poisoned the PHY domain: the first failure the actor saw, the
//! operation it saw it in and the failing step.
//!
//! The actor keeps the first [`Poison`] and answers every later request with
//! it, so a request refused by an earlier failure names that failure, not
//! itself. The causes use only types the host builds; the backend lowers the
//! PHY's target errors into [`PhyFault`].

use oer_esp32s31_hal::domain::{CommonRadioPowerError, ModemClockError, RadioClient};
use oer_esp32s31_phy::{PhyRegisterBindingError, PhyRegisterStage, concurrent::ConcurrentPhyError};

/// The first failure that poisoned the PHY domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Poison {
    /// The actor operation in which the failure happened.
    pub origin: PoisonOrigin,
    /// The failing step.
    pub cause: PoisonCause,
}

/// The actor operation in which the domain was poisoned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PoisonOrigin {
    /// The periodic PHY tracking tick between requests.
    TrackingTick,
    /// Preparing the domain for a join: registration or RF wake.
    Prepare,
    /// Closing RF after the last member left.
    CloseRf,
    /// A client's request.
    Request {
        client: RadioClient,
        request: RequestKind,
    },
}

/// A request a client posts to the actor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestKind {
    Join,
    Track,
    Retune,
    Leave,
}

/// The step that poisoned the domain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PoisonCause {
    /// PHY tracking was refused before any register access: its clock was
    /// rejected or the domain was not in a state that tracks. A periodic
    /// tick refused this way would fall due again at once.
    TrackingRefused(ConcurrentPhyError),
    /// PHY tracking started and failed in its own step or a leaf child.
    Tracking(PhyFault),
    /// The calibration-tracking child of PHY tracking failed.
    CalibrationTracking(CalibrationTrackingFault),
    /// A channel transaction started and failed.
    Channel(PhyFault),
    /// RF wake or close started and failed.
    Rf(PhyFault),
    /// The PHY registration started and failed.
    Registration(RegistrationFault),
    /// The PHY domain refused a step that its state should admit, or was
    /// found unsettled after a step that settles it.
    Domain(ConcurrentPhyError),
    /// Common radio power stopped at a read-back checkpoint.
    CommonPower(CommonRadioPowerError),
    /// A modem clock module did not change; the modem clocks are poisoned.
    ModemClock(ModemClockError),
    /// The client's rollback to cold did not complete.
    Rollback,
    /// A client controller's step failed after shared writes.
    Controller,
    /// An owner the actor keeps for a client was missing.
    Custody,
}

/// How a started PHY transaction failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyFault {
    /// A hardware edge did not complete within its bound.
    EdgeTimedOut,
    /// The executor could not provide a required capability (a settle).
    CapabilityUnavailable,
    /// A hardware read-back broke an invariant.
    HardwareInvariant,
    /// The transaction exceeded its operation limit.
    RfOperationLimit,
    /// An action could not be bound to the hardware.
    UnexpectedBinding,
    /// The registration no longer describes the borrowed PHY partition.
    RegistrationEpochMismatch,
    /// A deadline did not fit the clock.
    DeadlineOverflow,
    /// The transaction's own sequence broke: a wrong completion, its edge
    /// limit or a missing terminal owner.
    Sequence,
}

/// How the calibration-tracking child of PHY tracking failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CalibrationTrackingFault {
    /// A step of the child failed.
    Step(PhyFault),
    /// The recovered calibration sequence reported its own failure.
    Radio(CalibrationTrackingFailure),
}

/// Which calibration of the tracking child reported a failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CalibrationTrackingFailure {
    PbusClearTimedOut,
    Dcode,
    RxGain,
    Channel,
    TxDcPwdet,
}

/// How the PHY registration failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistrationFault {
    /// A step of the registration failed.
    Step(PhyFault),
    /// An action of `stage` could not be lowered to its hardware binding.
    Lowering {
        stage: PhyRegisterStage,
        error: PhyRegisterBindingError,
    },
    /// The recovered registration sequence reported its own failure.
    Radio(RegistrationFailure),
}

/// Which part of the registration reported a failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistrationFailure {
    /// The I2C master reset at `index` did not clear within `samples`.
    I2cMasterResetTimedOut {
        index: u8,
        samples: u16,
    },
    Rf,
    Baseband,
    Temperature,
    FinalI2cDeadlineExceeded,
}

impl Poison {
    /// The poison of a client's request.
    pub const fn request(client: RadioClient, request: RequestKind, cause: PoisonCause) -> Self {
        Self {
            origin: PoisonOrigin::Request { client, request },
            cause,
        }
    }
}
