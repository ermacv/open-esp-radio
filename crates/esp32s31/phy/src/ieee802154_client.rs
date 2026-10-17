//! IEEE 802.15.4 as a client of the shared PHY domain.
//!
//! The HAL chain holds the IEEE 802.15.4 partition; the registered PHY domain
//! lives under the arbiter as [`ConcurrentPhy`]. Between the module clocks and
//! the MAC reset, ESP-IDF's `esp_ieee802154_enable` runs `esp_phy_enable` and
//! `esp_btbb_enable`, and `ieee802154_mac_init` then overrides the shared
//! transmit-on delay. [`join_ieee802154`] performs those steps on the
//! [`Ieee802154Clocked`] owner and issues the affine
//! [`Ieee802154PhyMembership`]; [`leave_ieee802154`] consumes it again.
//!
//! The vendor puts the radio to sleep without closing RF: on the ESP32-S31
//! `ieee802154_rf_disable` closes RF only together with the REGDMA PHY
//! retention, which the pinned libraries do not provide, so a sleeping
//! client stays a PHY client.
//!
//! [`RegisteredIeee802154Operational`] keeps that membership beside the
//! operational MAC owners, so an interrupt-driven MAC epoch cannot exist apart
//! from a PHY client and a BTBB reference.
//!
//! This module holds the crate's single scoped `unsafe` override: the BTBB
//! gain parameter is projected here from the settled registration that
//! describes the lease, never supplied by a caller.

use core::fmt;

use oer_esp32s31_hal::{
    ieee802154::{
        Ieee802154Clocked, Ieee802154FoundationConfigured, Ieee802154FoundationTransitionFailure,
        Ieee802154Operational,
        mac::{Ieee802154InterruptSetupOwner, Ieee802154TaskOwner},
    },
    shared_radio::{BtbbError, RadioClient, SharedRadioLease},
};

use crate::concurrent::{
    ConcurrentPhy, ConcurrentPhyError, JoinRefused, Joined, PhyReady, acquire_client,
    release_client,
};

/// IEEE 802.15.4 holds a client bit in the shared PHY domain and a reference
/// on the shared BTBB baseband, with its transmit-on delay applied.
///
/// ```compile_fail
/// use oer_esp32s31_phy::ieee802154_client::Ieee802154PhyMembership;
/// fn requires_clone<T: Clone>() {}
/// requires_clone::<Ieee802154PhyMembership>();
/// ```
#[must_use = "the IEEE 802.15.4 PHY membership must be left through leave_ieee802154"]
pub struct Ieee802154PhyMembership {
    _private: (),
}

impl fmt::Debug for Ieee802154PhyMembership {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Ieee802154PhyMembership")
    }
}

/// Why IEEE 802.15.4 could not join or leave the shared PHY and BTBB.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154PhyClientError {
    /// The PHY domain rejected the operation.
    Phy(ConcurrentPhyError),
    /// The BTBB reference is not in the state the operation requires.
    Btbb(BtbbError),
}

/// Enter IEEE 802.15.4 into the shared PHY domain and the shared BTBB
/// baseband.
///
/// This is `esp_phy_enable(PHY_MODEM_IEEE802154)` followed by
/// `esp_btbb_enable`; the pinned `ieee802154_txon_delay_set` writes only the
/// MAC's own delays, so the shared transmit-on delay keeps the value of the
/// BTBB initialization. The
/// returned [`ConcurrentAcquire::TrackingDue`](crate::concurrent::ConcurrentAcquire::TrackingDue) means the domain must run its
/// tracking before the MAC may use RF.
///
/// # Errors
///
/// Every check runs before the client set or BTBB changes: IEEE 802.15.4 already
/// holds BTBB, or the client set rejects the client. The lease comes back
/// either way.
pub fn join_ieee802154<'l, 'r>(
    ready: PhyReady<'l, 'r>,
    clocked: &Ieee802154Clocked,
    clock: &impl oer_time::Clock,
) -> Result<Joined<'l, 'r, Ieee802154PhyMembership>, JoinRefused<'l, 'r, Ieee802154PhyClientError>>
{
    let lease = ready.into_lease();
    // The ready proof checked that the settled registration describes the
    // lease.
    let gain_parameter = match lease.attachment().settled() {
        Ok(domain) => {
            domain
                .registered
                .state()
                .register_init_parameters()
                .parameter_120
        }
        Err(error) => {
            return Err(JoinRefused {
                error: Ieee802154PhyClientError::Phy(error),
                lease,
            });
        }
    };
    if lease.holds_btbb(RadioClient::Ieee802154) {
        return Err(JoinRefused {
            error: Ieee802154PhyClientError::Btbb(BtbbError::AlreadyAcquired),
            lease,
        });
    }
    let acquired = match acquire_client(lease, RadioClient::Ieee802154, clock) {
        Ok(acquired) => acquired,
        Err(error) => {
            return Err(JoinRefused {
                error: Ieee802154PhyClientError::Phy(error),
                lease,
            });
        }
    };

    #[allow(
        unsafe_code,
        reason = "the gain byte is projected from the registration that describes the lease"
    )]
    let btbb = {
        // SAFETY: the settled domain was registered by a completed target
        // run whose epoch is the lease's current registration, and
        // `gain_parameter` is the `phy_param` byte at 0x120 projected from
        // that same registered state. `clocked` proves the IEEE 802.15.4
        // module clocks.
        unsafe { clocked.acquire_btbb(lease, gain_parameter) }
    };
    if let Err(error) = btbb {
        // Both rejections were checked above; undo the client bit when the
        // domain still permits it.
        let _ = release_client(lease, RadioClient::Ieee802154);
        return Err(JoinRefused {
            error: Ieee802154PhyClientError::Btbb(error),
            lease,
        });
    }
    Ok(Joined {
        membership: Ieee802154PhyMembership { _private: () },
        acquired,
        lease,
    })
}

/// Failed leave retaining the membership.
#[must_use = "a failed leave still holds the IEEE 802.15.4 PHY membership"]
pub struct Ieee802154PhyLeaveFailure {
    membership: Ieee802154PhyMembership,
    error: Ieee802154PhyClientError,
}

impl Ieee802154PhyLeaveFailure {
    pub const fn error(&self) -> Ieee802154PhyClientError {
        self.error
    }

    /// Recover the membership for a retry.
    pub fn into_membership(self) -> Ieee802154PhyMembership {
        self.membership
    }
}

impl fmt::Debug for Ieee802154PhyLeaveFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Ieee802154PhyLeaveFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Leave the shared PHY domain and drop the BTBB reference. Returns whether
/// IEEE 802.15.4 was the last PHY client.
///
/// This is `esp_btbb_disable` and `esp_phy_disable(PHY_MODEM_IEEE802154)`;
/// neither performs register access here.
///
/// # Errors
///
/// The domain rejects the release (tracking pending, poisoned) or the BTBB
/// reference is missing; nothing changed and the membership is returned.
pub fn leave_ieee802154(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    clocked: &Ieee802154Clocked,
    membership: Ieee802154PhyMembership,
) -> Result<bool, Ieee802154PhyLeaveFailure> {
    if !lease.holds_btbb(RadioClient::Ieee802154) {
        return Err(Ieee802154PhyLeaveFailure {
            membership,
            error: Ieee802154PhyClientError::Btbb(BtbbError::NotAcquired),
        });
    }
    let last = match release_client(lease, RadioClient::Ieee802154) {
        Ok(last) => last,
        Err(error) => {
            return Err(Ieee802154PhyLeaveFailure {
                membership,
                error: Ieee802154PhyClientError::Phy(error),
            });
        }
    };
    let Ieee802154PhyMembership { _private: () } = membership;
    // The reference was checked above, so the release cannot be rejected.
    let _ = clocked.release_btbb(lease);
    Ok(last)
}

/// The owners of one operational IEEE 802.15.4 MAC epoch together with its
/// PHY membership: the MAC is installed only with the membership a join
/// issued.
///
/// ```
/// use oer_esp32s31_hal::ieee802154::Ieee802154FoundationConfigured;
/// use oer_esp32s31_phy::ieee802154_client::{
///     Ieee802154PhyMembership, RegisteredIeee802154Operational,
/// };
/// fn install(
///     foundation: Ieee802154FoundationConfigured,
///     membership: Ieee802154PhyMembership,
/// ) -> RegisteredIeee802154Operational {
///     RegisteredIeee802154Operational::new(foundation, membership)
/// }
/// ```
///
/// Not without it:
///
/// ```compile_fail,E0061
/// use oer_esp32s31_hal::ieee802154::Ieee802154FoundationConfigured;
/// use oer_esp32s31_phy::ieee802154_client::RegisteredIeee802154Operational;
/// fn install(foundation: Ieee802154FoundationConfigured) -> RegisteredIeee802154Operational {
///     RegisteredIeee802154Operational::new(foundation)
/// }
/// ```
#[must_use = "the operational owners must return to their registered route"]
pub struct RegisteredIeee802154Operational {
    /// Task-side MAC registers.
    pub task: Ieee802154TaskOwner,
    /// Inactive interrupt ownership for the platform CPU route.
    pub interrupts: Ieee802154InterruptSetupOwner,
    /// The PHY membership retained while the MAC is operational.
    pub route: RegisteredIeee802154OperationalRoute,
}

impl RegisteredIeee802154Operational {
    /// Hand the foundation owners to the operational MAC under `membership`.
    /// This performs no MMIO.
    pub fn new(
        foundation: Ieee802154FoundationConfigured,
        membership: Ieee802154PhyMembership,
    ) -> Self {
        let Ieee802154Operational { task, interrupts } = foundation.into_operational();
        Self {
            task,
            interrupts,
            route: RegisteredIeee802154OperationalRoute { membership },
        }
    }
}

/// PHY membership retained while the MAC is operational. The membership
/// comes back only with the foundation owners
/// ([`RegisteredIeee802154OperationalRoute::into_foundation`]), so IEEE 802.15.4
/// leaves the PHY only after its operational MAC was taken apart:
///
/// ```
/// use oer_esp32s31_hal::ieee802154::mac::{Ieee802154InterruptSetupOwner, Ieee802154TaskOwner};
/// use oer_esp32s31_phy::ieee802154_client::RegisteredIeee802154OperationalRoute;
/// fn uninstall(
///     route: RegisteredIeee802154OperationalRoute,
///     task: Ieee802154TaskOwner,
///     interrupts: Ieee802154InterruptSetupOwner,
/// ) {
///     if let Ok((_foundation, _membership)) = route.into_foundation(task, interrupts) {}
/// }
/// ```
///
/// ```compile_fail,E0616
/// use oer_esp32s31_phy::ieee802154_client::{
///     Ieee802154PhyMembership, RegisteredIeee802154OperationalRoute,
/// };
/// fn leave_early(route: RegisteredIeee802154OperationalRoute) -> Ieee802154PhyMembership {
///     route.membership
/// }
/// ```
#[must_use = "the registered operational route must reunite with its owners"]
pub struct RegisteredIeee802154OperationalRoute {
    membership: Ieee802154PhyMembership,
}

impl RegisteredIeee802154OperationalRoute {
    /// Reunite the quiescent operational owners and prove the MAC
    /// foundation again.
    ///
    /// The operational epoch rewrote the MAC PIB, so the return proves the
    /// foundation, not a static policy.
    ///
    /// # Errors
    ///
    /// A foundation field did not read back; the failure keeps the reset
    /// owner and the membership.
    #[allow(
        clippy::result_large_err,
        reason = "the allocation-free failure retains the owner and the membership"
    )]
    pub fn into_foundation(
        self,
        task: Ieee802154TaskOwner,
        interrupts: Ieee802154InterruptSetupOwner,
    ) -> Result<
        (Ieee802154FoundationConfigured, Ieee802154PhyMembership),
        RegisteredIeee802154ReturnFailure,
    > {
        match Ieee802154FoundationConfigured::from_operational(Ieee802154Operational {
            task,
            interrupts,
        }) {
            Ok(foundation) => Ok((foundation, self.membership)),
            Err(failure) => Err(RegisteredIeee802154ReturnFailure {
                failure,
                membership: self.membership,
            }),
        }
    }
}

/// Failed foundation proof on return, retaining the reset owner and the
/// membership.
#[must_use = "a failed return still owns the partition and the PHY membership"]
pub struct RegisteredIeee802154ReturnFailure<M = Ieee802154PhyMembership> {
    failure: Ieee802154FoundationTransitionFailure,
    membership: M,
}

impl<M> RegisteredIeee802154ReturnFailure<M> {
    /// Borrow the foundation readback failure.
    pub const fn failure(&self) -> &Ieee802154FoundationTransitionFailure {
        &self.failure
    }

    /// Recover the reset owner and the membership.
    pub fn into_parts(self) -> (Ieee802154FoundationTransitionFailure, M) {
        (self.failure, self.membership)
    }
}

impl<M> fmt::Debug for RegisteredIeee802154ReturnFailure<M> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RegisteredIeee802154ReturnFailure")
            .field("failure", &self.failure)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
impl Ieee802154PhyMembership {
    fn for_test() -> Self {
        Self { _private: () }
    }
}

#[cfg(test)]
mod tests;
