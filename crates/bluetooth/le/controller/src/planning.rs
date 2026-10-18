//! Context retained when a required radio continuation lies outside the
//! radio epoch.

use oer_bluetooth_radio::{LeInstant, RadioTiming};

/// The logical owner whose continuation failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanningRole {
    Advertising,
    Scanning,
    Peripheral,
    DirectTest,
}

/// The operation being prepared, before any new request is published.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanningOperation {
    Event,
    FutureReservation,
    ConnectionIndication,
}

/// The failing calculation within that operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanningCalculation {
    Admission,
    ChannelSpacing,
    EventDuration,
    LatestAnchor,
    Reservation,
    Recurrence,
    Expiry,
    IntervalEnd,
    MinimumWindow,
    WindowClipping,
    FirstAnchor,
    WindowGeometry,
    Widening,
    SlotAlignment,
    /// The earliest recurring DTM transmitter point reachable now.
    ReachableAnchor,
    /// The next point on the DTM transmitter's interval grid.
    NextAnchor,
}

/// A required continuation of the current schedule lies outside the radio
/// epoch: no later sample can bring it back. The service must cease
/// admission and settle the retained Controller and radio owners through
/// the port's `disable`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EpochExhausted {
    /// The role whose continuation left the epoch.
    pub role: PlanningRole,
    /// The operation being prepared.
    pub operation: PlanningOperation,
    /// The calculation whose result left the epoch.
    pub calculation: PlanningCalculation,
}

impl EpochExhausted {
    pub(crate) const fn at(
        role: PlanningRole,
        operation: PlanningOperation,
        calculation: PlanningCalculation,
    ) -> Self {
        Self {
            role,
            operation,
            calculation,
        }
    }
}

pub(crate) fn earliest(
    now: LeInstant,
    timing: RadioTiming,
    role: PlanningRole,
) -> Result<LeInstant, EpochExhausted> {
    now.checked_add(timing.preparation_lead)
        .and_then(|at| at.checked_add(timing.admission_guard))
        .and_then(|at| at.checked_add(crate::PLANNING_SLACK))
        .ok_or(EpochExhausted::at(
            role,
            PlanningOperation::Event,
            PlanningCalculation::Admission,
        ))
}
