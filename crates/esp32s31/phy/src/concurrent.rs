//! One registered PHY domain shared by concurrently running protocol clients.
//!
//! [`ConcurrentPhy`] is the PHY layer's attachment to the HAL shared radio
//! arbiter (`SharedRadio<ConcurrentPhy>`). Every operation takes the arbiter's
//! lease, so access to the domain is serialized by the same mechanism that
//! serializes the shared registers, and PHY work always runs on the lease's
//! shared-PHY borrow.
//!
//! The domain is registered once. Wi-Fi, Bluetooth and IEEE 802.15.4 enter and
//! leave it as clients of the one client set. Scheduled tracking and a first
//! acquisition that needs tracking leave the domain *pending* until the
//! tracking transaction runs. A failed or cancelled tracking transaction
//! poisons the domain until reset.
//!
//! Pending tracking starts as ESP-IDF's does: `phy_param_track_tot` runs
//! from a periodic timer under the PHY lock alone, without pausing any
//! protocol, and its RF-sensitive regions carry the grant-protect request.
//! Here the arbiter lease is that lock and the tracking graph emits the grant
//! brackets; a protocol operation that loses RF to it ends with its ordinary
//! outcome.
//!
//! The domain also owns its modem clock modules, as ESP-IDF's `esp_phy_enable`
//! and `esp_phy_disable` do: `PHY` is held while RF is open, and
//! `PHY_CALIBRATION` only while registration or RF wake runs. After the last
//! client leaves, closing RF releases `PHY`; waking RF takes it again.

use oer_esp32s31_hal::shared_radio::{ModemClockError, RadioClient, SharedRadioLease};

use crate::{
    PhyState, RegisteredPhyState,
    domain::PhyDomain,
    state::client::{
        PhyClientAcquireError, PhyClientReleaseError, PhyClientSnapshot, PhyPendingTrack,
        PhyTrackTimeError,
    },
};

/// State of the shared PHY domain kept under the arbiter.
#[derive(Default)]
pub struct ConcurrentPhy {
    slot: Slot,
}

#[derive(Default)]
pub(crate) enum Slot {
    /// No registration yet.
    #[default]
    Empty,
    /// Registered with RF open, with every active client settled.
    Registered(PhyDomain),
    /// Registered with RF closed and no client.
    RfClosed(PhyDomain),
    /// A tracking request must run before clients continue.
    Pending {
        registered: RegisteredPhyState,
        pending: PhyPendingTrack,
    },
    /// Tracking hardware work failed or was cancelled.
    Poisoned,
}

/// Why a concurrent PHY operation was rejected before any register access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConcurrentPhyError {
    /// The domain is not registered.
    NotRegistered,
    /// The domain is already registered.
    AlreadyRegistered,
    /// Tracking is pending; it must run before this operation.
    TrackingPending,
    /// No tracking is pending.
    NoTrackingPending,
    /// Tracking, RF close or RF wake failed earlier; the domain requires
    /// reset.
    Poisoned,
    /// RF is closed; it must be woken before clients enter.
    RfClosed,
    /// RF is open; only a closed domain can be woken.
    RfOpen,
    /// The shared-PHY borrow belongs to another registration; no register
    /// access ran.
    EpochMismatch,
    /// Clients are still active; RF closes only after the last one left.
    ClientsActive,
    /// The domain's modem clock module could not change.
    Clock(ModemClockError),
    /// The client set rejected the acquisition.
    Acquire(PhyClientAcquireError),
    /// The client set rejected the release.
    Release(PhyClientReleaseError),
    /// The tracking clock was rejected.
    Time(PhyTrackTimeError),
    /// The client this operation serves is not in the domain's client set.
    ClientAbsent(RadioClient),
}

/// Result of a client acquisition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConcurrentAcquire {
    /// The client runs; no tracking was due.
    Settled,
    /// The client is recorded, but tracking must run before it transmits.
    TrackingDue,
}

impl ConcurrentPhy {
    /// An empty slot for a fresh arbiter.
    pub const fn new() -> Self {
        Self { slot: Slot::Empty }
    }

    /// The active client set, when the domain is registered and settled.
    pub fn client_snapshot(&self) -> Option<PhyClientSnapshot> {
        match &self.slot {
            Slot::Registered(domain) => Some(domain.client_snapshot()),
            Slot::RfClosed(domain) => Some(domain.client_snapshot()),
            Slot::Pending { pending, .. } => Some(pending.snapshot()),
            Slot::Empty | Slot::Poisoned => None,
        }
    }

    /// The registered calibration state, when every active client is
    /// settled.
    ///
    /// # Errors
    ///
    /// The domain is not registered, has RF closed, awaits tracking or is
    /// poisoned.
    pub fn phy_state(&self) -> Result<&PhyState, ConcurrentPhyError> {
        self.settled().map(PhyDomain::phy_state)
    }

    /// Capture the registered domain's current calibration state as a cache
    /// for a later cold registration: the final-state handoff. RF may be
    /// open or closed; the state stays registered in both.
    ///
    /// # Errors
    ///
    /// The domain is not registered, awaits tracking or is poisoned.
    pub fn calibration_cache(
        &self,
        identity: crate::PhyCalibrationIdentity,
    ) -> Result<crate::PhyCalibrationCache, ConcurrentPhyError> {
        let domain = match &self.slot {
            Slot::Registered(domain) | Slot::RfClosed(domain) => domain,
            Slot::Empty => return Err(ConcurrentPhyError::NotRegistered),
            Slot::Pending { .. } => return Err(ConcurrentPhyError::TrackingPending),
            Slot::Poisoned => return Err(ConcurrentPhyError::Poisoned),
        };
        Ok(domain.phy_state().calibration_cache(identity))
    }

    /// Whether the domain is registered with RF closed.
    pub const fn rf_closed(&self) -> bool {
        matches!(self.slot, Slot::RfClosed(_))
    }

    /// Whether tracking must run before clients continue.
    pub const fn tracking_pending(&self) -> bool {
        matches!(self.slot, Slot::Pending { .. })
    }

    fn settled_domain(&mut self) -> Result<PhyDomain, ConcurrentPhyError> {
        match core::mem::take(&mut self.slot) {
            Slot::Registered(domain) => Ok(domain),
            Slot::Empty => Err(ConcurrentPhyError::NotRegistered),
            closed @ Slot::RfClosed(_) => {
                self.slot = closed;
                Err(ConcurrentPhyError::RfClosed)
            }
            pending @ Slot::Pending { .. } => {
                self.slot = pending;
                Err(ConcurrentPhyError::TrackingPending)
            }
            Slot::Poisoned => {
                self.slot = Slot::Poisoned;
                Err(ConcurrentPhyError::Poisoned)
            }
        }
    }

    /// Set the vendor's temperature-tracking debug policy of the registered
    /// domain: bit 0 of `first` forces the RFPLL-cap threshold and bit 1 the
    /// calibration threshold to `second`, as ESP-IDF's debug bytes do. A
    /// zero threshold makes every due tracking tick run that branch. For
    /// diagnostic images only.
    ///
    /// # Errors
    ///
    /// The domain is not registered and settled.
    #[cfg(all(feature = "tracking-diagnostics", target_arch = "riscv32"))]
    pub fn set_temperature_tracking_debug(
        &mut self,
        first: u8,
        second: u8,
    ) -> Result<(), ConcurrentPhyError> {
        self.settled_mut()?
            .set_temperature_tracking_debug(first, second);
        Ok(())
    }

    /// Borrow the registered domain when every active client is settled.
    pub(crate) fn settled(&self) -> Result<&PhyDomain, ConcurrentPhyError> {
        match &self.slot {
            Slot::Registered(domain) => Ok(domain),
            Slot::Empty => Err(ConcurrentPhyError::NotRegistered),
            Slot::RfClosed(_) => Err(ConcurrentPhyError::RfClosed),
            Slot::Pending { .. } => Err(ConcurrentPhyError::TrackingPending),
            Slot::Poisoned => Err(ConcurrentPhyError::Poisoned),
        }
    }

    /// Mutably borrow the registered domain when every active client is
    /// settled.
    pub(crate) fn settled_mut(&mut self) -> Result<&mut PhyDomain, ConcurrentPhyError> {
        match &mut self.slot {
            Slot::Registered(domain) => Ok(domain),
            Slot::Empty => Err(ConcurrentPhyError::NotRegistered),
            Slot::RfClosed(_) => Err(ConcurrentPhyError::RfClosed),
            Slot::Pending { .. } => Err(ConcurrentPhyError::TrackingPending),
            Slot::Poisoned => Err(ConcurrentPhyError::Poisoned),
        }
    }

    pub(crate) fn slot_mut(&mut self) -> &mut Slot {
        &mut self.slot
    }

    /// Take the RF-open domain with no active client, for RF close.
    pub(crate) fn idle_domain(&mut self) -> Result<PhyDomain, ConcurrentPhyError> {
        let domain = self.settled_domain()?;
        if domain.client_snapshot().is_empty() {
            Ok(domain)
        } else {
            self.slot = Slot::Registered(domain);
            Err(ConcurrentPhyError::ClientsActive)
        }
    }

    /// Take the RF-closed domain, for RF wake.
    pub(crate) fn closed_domain(&mut self) -> Result<PhyDomain, ConcurrentPhyError> {
        match core::mem::take(&mut self.slot) {
            Slot::RfClosed(domain) => Ok(domain),
            Slot::Empty => Err(ConcurrentPhyError::NotRegistered),
            Slot::Poisoned => {
                self.slot = Slot::Poisoned;
                Err(ConcurrentPhyError::Poisoned)
            }
            other => {
                self.slot = other;
                Err(ConcurrentPhyError::RfOpen)
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn registered_for_test(domain: PhyDomain) -> Self {
        Self {
            slot: Slot::Registered(domain),
        }
    }

    #[cfg(test)]
    pub(crate) fn rf_closed_for_test(domain: PhyDomain) -> Self {
        Self {
            slot: Slot::RfClosed(domain),
        }
    }
}

/// Proof that the shared PHY domain is ready for a client to enter it:
/// registered, RF open and settled, and its registration describes the
/// lease. It borrows that lease exclusively, so nothing can close RF or start
/// tracking before the client's join consumes it; the join gives the lease
/// back. Whoever prepared the domain (the shared radio's owner, or another
/// client before), a client enters only a domain that is ready.
///
/// It cannot be copied for a second client:
///
/// ```compile_fail,E0599
/// use oer_esp32s31_phy::concurrent::PhyReady;
/// fn clone<'l, 'r>(ready: &PhyReady<'l, 'r>) -> PhyReady<'l, 'r> {
///     ready.clone()
/// }
/// ```
///
/// And a client cannot make one from a lease without the check:
///
/// ```compile_fail,E0451
/// use oer_esp32s31_hal::shared_radio::SharedRadioLease;
/// use oer_esp32s31_phy::concurrent::{ConcurrentPhy, PhyReady};
/// fn forge<'l, 'r>(lease: &'l mut SharedRadioLease<'r, ConcurrentPhy>) -> PhyReady<'l, 'r> {
///     PhyReady { lease }
/// }
/// ```
#[must_use = "a ready PHY is consumed by the client's join"]
pub struct PhyReady<'l, 'r> {
    lease: &'l mut SharedRadioLease<'r, ConcurrentPhy>,
}

impl core::fmt::Debug for PhyReady<'_, '_> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("PhyReady")
    }
}

impl<'l, 'r> PhyReady<'l, 'r> {
    #[cfg(test)]
    pub(crate) fn for_test(lease: &'l mut SharedRadioLease<'r, ConcurrentPhy>) -> Self {
        Self { lease }
    }

    /// The lease, for the join that consumes the proof.
    pub(crate) fn into_lease(self) -> &'l mut SharedRadioLease<'r, ConcurrentPhy> {
        self.lease
    }
}

/// A client entered the shared domain; the lease its [`PhyReady`] borrowed
/// comes back. [`ConcurrentAcquire::TrackingDue`] means the domain must run
/// its tracking before the client uses RF.
#[must_use = "the membership must be kept and left"]
pub struct Joined<'l, 'r, M> {
    pub membership: M,
    pub acquired: ConcurrentAcquire,
    pub lease: &'l mut SharedRadioLease<'r, ConcurrentPhy>,
}

/// The domain refused the client; nothing changed and the lease comes back.
pub struct JoinRefused<'l, 'r, E> {
    pub error: E,
    pub lease: &'l mut SharedRadioLease<'r, ConcurrentPhy>,
}

impl<E: core::fmt::Debug> core::fmt::Debug for JoinRefused<'_, '_, E> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("JoinRefused")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Prove the shared domain ready for a client under `lease`.
///
/// # Errors
///
/// The domain is not registered, RF is closed, tracking is pending, the
/// domain is poisoned, or its registration no longer describes the lease.
pub fn phy_ready<'l, 'r>(
    lease: &'l mut SharedRadioLease<'r, ConcurrentPhy>,
) -> Result<PhyReady<'l, 'r>, ConcurrentPhyError> {
    let epoch = lease.registration_epoch();
    let domain = lease.attachment().settled()?;
    if !domain.clients.describes_epoch(epoch) {
        return Err(ConcurrentPhyError::EpochMismatch);
    }
    Ok(PhyReady { lease })
}

/// Enter `client` into the shared domain `ready` proves ready.
///
/// # Errors
///
/// The client set rejects the client (already acquired, clock error). The
/// domain is unchanged.
pub(crate) fn acquire_client(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    client: RadioClient,
    clock: &impl oer_time::Clock,
) -> Result<ConcurrentAcquire, ConcurrentPhyError> {
    let phy = lease.attachment_mut();
    let PhyDomain {
        registered,
        clients,
    } = phy.settled_domain()?;
    match clients.acquire(client, clock) {
        Ok(outcome) => match outcome.into_owner() {
            Ok(clients) => {
                phy.slot = Slot::Registered(PhyDomain::new(registered, clients));
                Ok(ConcurrentAcquire::Settled)
            }
            Err(pending) => {
                phy.slot = Slot::Pending {
                    registered,
                    pending,
                };
                Ok(ConcurrentAcquire::TrackingDue)
            }
        },
        Err(failure) => {
            let error = failure.error();
            phy.slot = Slot::Registered(PhyDomain::new(registered, failure.into_owner()));
            Err(ConcurrentPhyError::Acquire(error))
        }
    }
}

/// Remove `client` from the shared domain. Returns whether it was the last.
///
/// # Errors
///
/// The domain is not registered and settled, or the client was not acquired.
/// The domain is unchanged.
pub fn release_client(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    client: RadioClient,
) -> Result<bool, ConcurrentPhyError> {
    let phy = lease.attachment_mut();
    let PhyDomain {
        registered,
        clients,
    } = phy.settled_domain()?;
    match clients.release(client) {
        Ok(outcome) => {
            let is_last = outcome.is_last();
            let clients = outcome.into_owner();
            phy.slot = Slot::Registered(PhyDomain::new(registered, clients));
            Ok(is_last)
        }
        Err(failure) => {
            let error = failure.error();
            phy.slot = Slot::Registered(PhyDomain::new(registered, failure.into_owner()));
            Err(ConcurrentPhyError::Release(error))
        }
    }
}

/// Run one periodic tracking evaluation. Returns whether tracking is now due.
///
/// # Errors
///
/// The domain is not registered and settled, or the clock was rejected; the
/// domain is unchanged.
pub fn evaluate_periodic_tracking(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    clock: &impl oer_time::Clock,
) -> Result<bool, ConcurrentPhyError> {
    let phy = lease.attachment_mut();
    let PhyDomain {
        registered,
        clients,
    } = phy.settled_domain()?;
    match clients.evaluate_periodic_tracking(clock) {
        Ok(evaluation) => match evaluation.into_owner() {
            Ok(clients) => {
                phy.slot = Slot::Registered(PhyDomain::new(registered, clients));
                Ok(false)
            }
            Err(pending) => {
                phy.slot = Slot::Pending {
                    registered,
                    pending,
                };
                Ok(true)
            }
        },
        Err(failure) => {
            let error = failure.error();
            phy.slot = Slot::Registered(PhyDomain::new(registered, failure.into_owner()));
            Err(ConcurrentPhyError::Time(error))
        }
    }
}

/// Check, without register access, whether tracking is pending: it then
/// starts at once, with the protocols running.
///
/// # Errors
///
/// No tracking is pending, or the domain is not registered, RF is closed or
/// the domain is poisoned.
pub fn admit_maintenance(
    lease: &SharedRadioLease<'_, ConcurrentPhy>,
) -> Result<(), ConcurrentPhyError> {
    match &lease.attachment().slot {
        Slot::Pending { .. } => Ok(()),
        Slot::Poisoned => Err(ConcurrentPhyError::Poisoned),
        Slot::RfClosed(_) => Err(ConcurrentPhyError::RfClosed),
        Slot::Empty => Err(ConcurrentPhyError::NotRegistered),
        Slot::Registered(_) => Err(ConcurrentPhyError::NoTrackingPending),
    }
}

#[cfg(test)]
mod tests;
