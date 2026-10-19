//! Current RFPLL thermal gate and frequency-control transaction.
//!
//! Execution requires the caller's exclusive physical admission; this child
//! owns no vendor busy byte or grant API. Due temperature alone is not RF access.
use super::{Correction, Error};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Request {
    pub current_temperature: i16,
    pub reference_temperature: i16,
    pub current_channel: u16,
    pub threshold_override: Option<u8>,
}

impl Request {
    /// Threshold in PHY sensor units, without a Celsius conversion claim.
    /// An explicit zero admits work even when temperature is unchanged.
    pub const fn threshold(self) -> u8 {
        match self.threshold_override {
            Some(value) => value,
            None => 15,
        }
    }

    pub const fn is_due(self) -> bool {
        (self.current_temperature as i32 - self.reference_temperature as i32).unsigned_abs()
            >= self.threshold() as u32
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Outcome {
    pub reference_temperature: i16,
    /// A completed procedure may have a zero delta and no accepted samples.
    /// Presence means it ran and restored control, not that RFPLL locked.
    pub correction: Option<super::Outcome>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    /// Program (`true`) or withdraw (`false`) the PHY grant-protect request
    /// around the correction, as `phy_rfpll_cap_track_new` does.
    SetGrantProtect {
        enabled: bool,
    },
    SelectSoftwareControl,
    Settle,
    ObserveBoundary,
    Correct(super::Action),
    RestoreHardwareControl,
    Complete(Outcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Completion {
    GrantProtectSet { enabled: bool },
    SoftwareControlSelected,
    Settled,
    BoundaryObserved,
    Correction(super::Completion),
    HardwareControlRestored,
}

#[derive(Debug, Eq, PartialEq)]
enum Step {
    AcquireGrant,
    Select,
    Settle,
    Observe,
    Correct(Correction),
    Restore(super::Outcome),
    ReleaseGrant(super::Outcome),
    Complete(Outcome),
}

/// Retains the correction until the restoration completion is consumed.
#[derive(Debug, Eq, PartialEq)]
pub struct Transition {
    request: Request,
    step: Step,
}

impl Transition {
    pub const fn new(request: Request) -> Self {
        let step = if request.is_due() {
            Step::AcquireGrant
        } else {
            Step::Complete(Outcome {
                reference_temperature: request.reference_temperature,
                correction: None,
            })
        };
        Self { request, step }
    }

    pub const fn request(&self) -> Request {
        self.request
    }

    #[inline(always)]
    pub const fn action(&self) -> Action {
        match &self.step {
            Step::AcquireGrant => Action::SetGrantProtect { enabled: true },
            Step::Select => Action::SelectSoftwareControl,
            Step::Settle => Action::Settle,
            Step::Observe => Action::ObserveBoundary,
            Step::Correct(child) => Action::Correct(child.action()),
            Step::Restore(_) => Action::RestoreHardwareControl,
            Step::ReleaseGrant(_) => Action::SetGrantProtect { enabled: false },
            Step::Complete(outcome) => Action::Complete(*outcome),
        }
    }

    #[inline(always)]
    pub fn advance(&mut self, completion: Completion) -> Result<(), Error> {
        match (&mut self.step, completion) {
            (Step::AcquireGrant, Completion::GrantProtectSet { enabled: true }) => {
                self.step = Step::Select
            }
            (Step::Select, Completion::SoftwareControlSelected) => self.step = Step::Settle,
            (Step::Settle, Completion::Settled) => self.step = Step::Observe,
            (Step::Observe, Completion::BoundaryObserved) => {
                self.step = Step::Correct(Correction::new(self.request.current_channel))
            }
            (Step::Correct(child), Completion::Correction(completion)) => {
                child.advance(completion)?;
                if let super::Action::Complete(outcome) = child.action() {
                    self.step = Step::Restore(outcome);
                }
            }
            (Step::Restore(correction), Completion::HardwareControlRestored) => {
                self.step = Step::ReleaseGrant(*correction);
            }
            (Step::ReleaseGrant(correction), Completion::GrantProtectSet { enabled: false }) => {
                self.step = Step::Complete(Outcome {
                    reference_temperature: self.request.current_temperature,
                    correction: Some(*correction),
                });
            }
            (Step::Complete(_), _) => return Err(Error::AlreadyComplete),
            _ => return Err(Error::WrongCompletion),
        }
        Ok(())
    }
}
