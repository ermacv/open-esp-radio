//! What poisoned the PHY domain: the first failure the actor saw, the
//! operation it saw it in and the failing step.
//!
//! The actor keeps the first [`Poison`] and answers every later request with
//! it, so a request refused by an earlier failure names that failure, not
//! itself. The causes use only types the host builds; the backend lowers the
//! PHY's target errors into [`PhyFault`].

use oer_esp32s31_hal::domain::{CommonRadioPowerError, ModemClockError, RadioClient};
use oer_esp32s31_phy::concurrent::ConcurrentPhyError;

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
    /// PHY tracking started and failed.
    Tracking(PhyFault),
    /// A channel transaction started and failed.
    Channel(PhyFault),
    /// RF wake or close started and failed.
    Rf(PhyFault),
    /// The PHY registration started and failed.
    Registration(PhyFault),
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
    /// An action could not be bound to the hardware, or a child failed and
    /// its parent reports it as an unexpected binding.
    UnexpectedBinding,
    /// The registration no longer describes the borrowed PHY partition.
    RegistrationEpochMismatch,
    /// A deadline did not fit the clock.
    DeadlineOverflow,
    /// The transaction's own sequence broke: a wrong completion, its edge
    /// limit or a missing terminal owner.
    Sequence,
    /// The recovered radio sequence reported its own failure (a calibration
    /// result out of its range).
    Radio,
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
