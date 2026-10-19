//! Value-only admission contract for a real completed DMA unit.

use super::Discard;
use oer_esp32s31_ieee80211_mac::rx::pool::RxStageTransactionError;

/// Explicit bulk/critical capacity policy, independent of vendor pool sizes.
///
/// Validation never reduces a requested reserve. Both domains must retain an
/// ordinary credit, so a one-slot pool or queue requires explicit zero reserve.
///
/// ```ignore-riscv32
/// use oer_esp32s31_ieee80211_driver::rx::transaction::CreditPolicy;
/// assert_eq!(CreditPolicy::new(1).validate(2, 2).unwrap().critical_reserved_credits(), 1);
/// assert!(CreditPolicy::new(1).validate(32, 1).is_err());
/// assert_eq!(CreditPolicy::new(0).validate(32, 1).unwrap().critical_reserved_credits(), 0);
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CreditPolicy {
    reserved: usize,
}

impl CreditPolicy {
    pub const fn new(critical_reserved_credits: usize) -> Self {
        Self {
            reserved: critical_reserved_credits,
        }
    }

    pub const fn validate(
        self,
        pool_slots: usize,
        queue_depth: usize,
    ) -> Result<ValidatedCredits, RxStageTransactionError> {
        if self.reserved >= pool_slots || self.reserved >= queue_depth {
            return Err(RxStageTransactionError::InvalidCreditReserve {
                reserved: self.reserved,
                pool_slots,
                queue_depth,
            });
        }
        Ok(ValidatedCredits {
            reserved: self.reserved,
        })
    }
}

/// Immutable effective reserve after validation against both credit domains.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ValidatedCredits {
    reserved: usize,
}

impl ValidatedCredits {
    pub const fn critical_reserved_credits(self) -> usize {
        self.reserved
    }
}

/// Value-only description of one real completed DMA unit before staging.
///
/// The policy never receives a descriptor pointer, payload view or ring
/// capability. It can narrow the admitted payload length, but ownership and
/// descriptor reclaim remain exclusively inside the physical transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompletedUnit {
    pub head_index: usize,
    pub descriptor_count: usize,
    pub payload_length: usize,
}

/// Fact-only traffic class visible at the DMA/staging admission boundary.
///
/// Only an IEEE 802.11 frame-control value copied from the completed unit is
/// interpreted. No association, authorization or hardware meaning is
/// inferred here. Protected data is the sole bulk class; management, control
/// and unprotected data (including pre-key EAPOL) remain critical.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IngressClass {
    BulkProtectedData,
    Critical,
    Unclassified,
}

impl IngressClass {
    /// The class of a unit whose frame control, if it was readable, is
    /// `frame_control`: protected data is bulk, every other frame critical.
    pub const fn of(frame_control: Option<u16>) -> Self {
        match frame_control {
            Some(value) if value & 0x000c == 0x0008 && value & 0x4000 != 0 => {
                Self::BulkProtectedData
            }
            Some(_) => Self::Critical,
            None => Self::Unclassified,
        }
    }
}

/// Fact-only logical route for overload accounting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IngressRoute {
    Standalone,
    Station,
    AccessPoint,
    Foreign,
    Ambiguous,
    Malformed,
}

/// Value-only preview used before staging ownership is transferred.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Preview {
    pub unit: CompletedUnit,
    pub frame_control: Option<u16>,
    pub class: IngressClass,
    pub route: IngressRoute,
}

/// Policy decision when ordinary staging credits are unavailable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Unavailable {
    /// Keep ordinary bulk data at the completed ring head until staging frees.
    PreserveForCapacity,
    /// Preserve the final staging credit for control/management/EAPOL input.
    PreserveForCriticalAdmission,
    /// Drop the upper copy but return the completed descriptor immediately.
    DiscardAndRecycle,
}

/// A completed ingress transaction observed after its ownership edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Observation {
    /// The unit was discarded and retained until the frozen-LAST reclaim.
    DiscardRetained {
        unit: CompletedUnit,
        reason: Discard,
    },
    /// The original DMA buffer was published into the bounded upper queue.
    Staged(CompletedUnit),
    /// A bulk unit could not acquire an upper-layer credit and followed the
    /// reviewed vendor discard/append path.
    OverloadDiscardedAndRecycled(Preview),
    /// A bulk unit remained at the completed ring head until upper capacity
    /// becomes available.
    BulkAdmissionBlocked(Preview),
    /// A critical unit consumed the reserved final staging credit.
    CriticalReserveAdmitted(Preview),
    /// No reserved credit remained for a critical unit. Descriptor ownership
    /// was deliberately not transferred, so a later capacity wake retries it.
    CriticalAdmissionBlocked(Preview),
}

/// Admission policy at the completed-DMA-unit/staging boundary.
///
/// This hook declares the credit reserve, selects overload disposition, may
/// lower the maximum staged payload, and observes completed ownership edges.
/// It cannot mutate descriptor metadata,
/// reclaim buffers, fabricate frames or retain hardware ownership.
pub trait Admission {
    fn maximum_payload_length(&self, _unit: CompletedUnit, physical_capacity: usize) -> usize {
        physical_capacity
    }

    fn observe(&self, _observation: Observation) {}

    /// Explicit staging/queue reserve, validated before any transaction work.
    /// The default reserves one credit for all pool/queue sizes of at least two.
    fn credit_policy(&self) -> CreditPolicy {
        CreditPolicy::new(1)
    }

    /// Decide whether a unit may be discarded when only the critical reserve
    /// remains. The default is deliberately conservative for unknown input.
    fn unavailable_disposition(&self, preview: Preview) -> Unavailable {
        match preview.class {
            IngressClass::BulkProtectedData => Unavailable::DiscardAndRecycle,
            IngressClass::Critical | IngressClass::Unclassified => {
                Unavailable::PreserveForCriticalAdmission
            }
        }
    }
}

impl<T: Admission + ?Sized> Admission for &T {
    fn maximum_payload_length(&self, unit: CompletedUnit, physical_capacity: usize) -> usize {
        T::maximum_payload_length(*self, unit, physical_capacity)
    }

    fn observe(&self, observation: Observation) {
        T::observe(*self, observation);
    }

    fn credit_policy(&self) -> CreditPolicy {
        T::credit_policy(*self)
    }

    fn unavailable_disposition(&self, preview: Preview) -> Unavailable {
        T::unavailable_disposition(*self, preview)
    }
}

/// Zero-sized production policy admitting the complete physical stage slot
/// and reserving one credit for critical traffic. Both credit domains require
/// capacity of at least two; validation never silently disables the reserve.
#[derive(Clone, Copy, Debug, Default)]
pub struct AdmitAll;

impl Admission for AdmitAll {}

/// Explicit zero-reserve profile for a single-credit pool or queue.
///
/// Admits the full payload, but cannot protect critical traffic from bulk
/// saturation. This is never selected implicitly by the physical transaction.
#[derive(Clone, Copy, Debug, Default)]
pub struct AdmitUnreserved;

impl Admission for AdmitUnreserved {
    fn credit_policy(&self) -> CreditPolicy {
        CreditPolicy::new(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_protected_data_is_bulk() {
        // Data, Protected Frame set.
        assert_eq!(
            IngressClass::of(Some(0x4008)),
            IngressClass::BulkProtectedData
        );
        // QoS data, protected.
        assert_eq!(
            IngressClass::of(Some(0x4088)),
            IngressClass::BulkProtectedData
        );
        // Unprotected data (EAPOL before the keys), management, control.
        for frame_control in [0x0008, 0x0088, 0x0080, 0x00d0, 0x0094] {
            assert_eq!(
                IngressClass::of(Some(frame_control)),
                IngressClass::Critical
            );
        }
        // A protected management frame stays critical.
        assert_eq!(IngressClass::of(Some(0x40d0)), IngressClass::Critical);
        assert_eq!(IngressClass::of(None), IngressClass::Unclassified);
    }

    #[test]
    fn reserve_is_one_for_every_supported_multicredit_capacity() {
        for slots in 2..=32 {
            for depth in 2..=32 {
                let validated = AdmitAll.credit_policy().validate(slots, depth).unwrap();
                assert_eq!(validated.critical_reserved_credits(), 1);
            }
        }
    }

    #[test]
    fn single_credit_requires_explicit_zero_and_empty_domains_are_rejected() {
        for (slots, depth) in [(1, 1), (1, 32), (32, 1)] {
            assert!(AdmitAll.credit_policy().validate(slots, depth).is_err());
            assert_eq!(
                AdmitUnreserved
                    .credit_policy()
                    .validate(slots, depth)
                    .unwrap()
                    .critical_reserved_credits(),
                0
            );
        }
        for (slots, depth) in [(0, 0), (0, 32), (32, 0)] {
            assert!(
                AdmitUnreserved
                    .credit_policy()
                    .validate(slots, depth)
                    .is_err()
            );
        }
        assert!(CreditPolicy::new(usize::MAX).validate(32, 32).is_err());
    }
}
