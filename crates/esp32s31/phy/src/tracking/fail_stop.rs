//! Terminal policy for the shared PHY epoch, independent of the stop mechanism.
//!
//! This decision is not itself proof that RF stopped. Its platform consumer must
//! establish TX/RX and DMA/IRQ quiescence and prevent every protocol from
//! reacquiring the epoch. An active DTM test is abandoned, never paused or
//! completed by synthesizing a Host Test End. If local quiescence cannot be
//! proven, the platform must escalate to a full system reset. Host progress
//! must never be a prerequisite for that stop.

/// The shared PHY epoch may no longer authorize RF work by any protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "establish proven shared RF quiescence or reset the platform"]
pub enum SharedPhyFailStop {
    MaintenanceHardDeadlineExceeded,
    /// Maintenance or restoration became ambiguous; ordinary recovery is forbidden.
    MaintenanceFailed,
    /// RF close, wake or initialization failed without a completed safe cleanup.
    LifecycleFailed,
}

impl SharedPhyFailStop {
    /// Apply the common policy to a retained close/wake/initialization failure.
    /// The input must come from its physical owner, not from a generic error
    /// code or the presence of pending work. `None` grants no resume authority.
    pub const fn from_ambiguous_lifecycle(hardware_ambiguous: bool) -> Option<Self> {
        if hardware_ambiguous {
            Some(Self::LifecycleFailed)
        } else {
            None
        }
    }
}

/// Retained physical frontier of a rejected or failed maintenance operation.
/// These facts do not select a watchdog peripheral or release any owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaintenanceFailureStage {
    /// No PHY execution began; original quiesced hardware remains retained.
    Admission,
    /// Executed PHY work did not return a valid owner.
    Execution,
    /// PHY work settled, but the required restoration did not complete.
    Restoration,
}

impl MaintenanceFailureStage {
    /// Only execution/restoration failure invalidates this maintenance epoch.
    pub const fn shared_phy_failure(self) -> Option<SharedPhyFailStop> {
        match self {
            Self::Admission => None,
            Self::Execution | Self::Restoration => Some(SharedPhyFailStop::MaintenanceFailed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SharedPhyFailStop;

    #[test]
    fn admission_and_incomplete_restoration_have_distinct_dispositions() {
        use super::MaintenanceFailureStage;
        assert_eq!(
            MaintenanceFailureStage::Admission.shared_phy_failure(),
            None
        );
        for stage in [
            MaintenanceFailureStage::Execution,
            MaintenanceFailureStage::Restoration,
        ] {
            assert_eq!(
                stage.shared_phy_failure(),
                Some(SharedPhyFailStop::MaintenanceFailed)
            );
        }
    }

    #[test]
    fn only_ambiguous_lifecycle_requires_shared_phy_escalation() {
        assert_eq!(SharedPhyFailStop::from_ambiguous_lifecycle(false), None);
        assert_eq!(
            SharedPhyFailStop::from_ambiguous_lifecycle(true),
            Some(SharedPhyFailStop::LifecycleFailed)
        );
    }
}
