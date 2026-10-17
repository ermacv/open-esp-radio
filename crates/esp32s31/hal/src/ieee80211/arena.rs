//! Reclaimable stable placement for the running Wi-Fi register owner.
//!
//! The arena is the explicit serialization owner for its shared handles.
//! `RefCell` dynamic borrows serialize synchronous MMIO transactions, make the
//! arena non-`Sync`, and prevent handles from authorizing cross-thread MMIO.
//! Consumers can obtain only narrow HAL operations; no generic register
//! callback or PAC owner escapes this module.

use core::{
    cell::{RefCell, RefMut},
    sync::atomic::{AtomicU8, Ordering},
};
use oer_esp32s31_pac::WifiRadioRegisters;

use crate::{owner::RadioRuntimeOwner, shared_radio::SharedRadioLease};

const EMPTY: u8 = 0;
const PUBLISHED: u8 = 1;
const RESET_REQUIRED: u8 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RadioOwnerArenaState {
    Empty,
    Published,
    ResetRequired,
}

impl RadioOwnerArenaState {
    const fn decode(value: u8) -> Self {
        match value {
            EMPTY => Self::Empty,
            PUBLISHED => Self::Published,
            _ => Self::ResetRequired,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RadioOwnerArenaError {
    AlreadyPublished,
    ResetRequired,
    Borrowed,
    MissingOwner,
}

/// Failed publication retaining the exact register owner.
pub struct RadioOwnerPublishFailure {
    pub error: RadioOwnerArenaError,
    pub owner: RadioRuntimeOwner,
}

/// Exact empty-arena capability returned beside a reclaimed runtime owner.
///
/// Keeping this non-cloneable value in role-local stopped resources preserves
/// which stable arena may host the next task epoch. Higher layers therefore do
/// not need to recover an initialized arena from a global `StaticCell`.
pub struct RadioOwnerRepublish<'arena> {
    arena: &'arena RadioOwnerArena,
}

impl<'arena> RadioOwnerRepublish<'arena> {
    /// Publish a returned runtime owner into the exact arena reclaimed with it.
    pub fn try_publish(
        self,
        owner: RadioRuntimeOwner,
    ) -> Result<PublishedRadioOwner<'arena>, RadioOwnerRepublishFailure<'arena>> {
        match self.arena.publish(owner) {
            Ok(published) => Ok(published),
            Err(failure) => Err(RadioOwnerRepublishFailure {
                error: failure.error,
                owner: failure.owner,
                republish: self,
            }),
        }
    }
}

/// Failed exact-arena republication retaining both movable capabilities.
pub struct RadioOwnerRepublishFailure<'arena> {
    pub error: RadioOwnerArenaError,
    pub owner: RadioRuntimeOwner,
    pub republish: RadioOwnerRepublish<'arena>,
}

/// Runtime owner and the exact empty-arena capability reclaimed from one epoch.
pub struct ReclaimedRadioOwner<'arena> {
    owner: RadioRuntimeOwner,
    republish: RadioOwnerRepublish<'arena>,
}

impl<'arena> ReclaimedRadioOwner<'arena> {
    pub fn into_parts(self) -> (RadioRuntimeOwner, RadioOwnerRepublish<'arena>) {
        (self.owner, self.republish)
    }

    /// Discard the empty-arena binding when the caller intentionally does not
    /// need another task-stable publication.
    pub fn into_owner(self) -> RadioRuntimeOwner {
        self.owner
    }

    pub fn try_republish(
        self,
    ) -> Result<PublishedRadioOwner<'arena>, RadioOwnerRepublishFailure<'arena>> {
        self.republish.try_publish(self.owner)
    }
}

/// Stable storage used while executor tasks require a `'static` register
/// address.
///
/// The arena is role-neutral. Publishing transfers the unique
/// [`RadioRuntimeOwner`] value into it and returns the only movable lease.
/// Consuming that lease after every task has stopped returns the original
/// value. Dropping a live lease poisons the arena instead of making the
/// hardware owner silently reusable.
pub struct RadioOwnerArena {
    registers: RefCell<Option<RadioRuntimeOwner>>,
    state: AtomicU8,
}

impl RadioOwnerArena {
    pub const fn new() -> Self {
        Self {
            registers: RefCell::new(None),
            state: AtomicU8::new(EMPTY),
        }
    }

    pub fn state(&self) -> RadioOwnerArenaState {
        RadioOwnerArenaState::decode(self.state.load(Ordering::Acquire))
    }

    /// Borrow the published owner only through the narrow channel capability,
    /// with the shared PHY borrowed from the arbiter lease. The dynamic borrow
    /// is the serialization guard and remains held across the complete
    /// asynchronous channel transaction.
    pub fn try_channel_hal<'arena, P, T>(
        &'arena self,
        platform: &'arena mut P,
        lease: &'arena mut SharedRadioLease<'_, T>,
    ) -> Result<crate::ieee80211::channel::RadioChannelHal<'arena, P>, RadioOwnerArenaError> {
        let mac = self.try_published_mac()?;
        let (phy, restore) = lease.phy_parts_mut();
        Ok(crate::ieee80211::channel::RadioChannelHal::from_published(
            platform, mac, phy, restore,
        ))
    }

    /// [`Self::try_channel_hal`] together with the arbiter's attachment, for
    /// a PHY channel operation that updates the shared domain's state.
    pub fn try_channel_hal_with_attachment<'arena, P, T>(
        &'arena self,
        platform: &'arena mut P,
        lease: &'arena mut SharedRadioLease<'_, T>,
    ) -> Result<
        (
            crate::ieee80211::channel::RadioChannelHal<'arena, P>,
            &'arena mut T,
        ),
        RadioOwnerArenaError,
    > {
        let mac = self.try_published_mac()?;
        let (phy, restore, attachment) = lease.phy_parts_with_attachment();
        Ok((
            crate::ieee80211::channel::RadioChannelHal::from_published(platform, mac, phy, restore),
            attachment,
        ))
    }

    /// Borrow the published MAC registers for one serialized transaction.
    fn try_published_mac(&self) -> Result<RefMut<'_, WifiRadioRegisters>, RadioOwnerArenaError> {
        match self.state() {
            RadioOwnerArenaState::Empty => {
                return Err(RadioOwnerArenaError::MissingOwner);
            }
            RadioOwnerArenaState::ResetRequired => {
                return Err(RadioOwnerArenaError::ResetRequired);
            }
            RadioOwnerArenaState::Published => {}
        }
        let slot = self
            .registers
            .try_borrow_mut()
            .map_err(|_| RadioOwnerArenaError::Borrowed)?;
        let owner = RefMut::filter_map(slot, Option::as_mut)
            .map_err(|_| RadioOwnerArenaError::MissingOwner)?;
        Ok(RefMut::map(owner, RadioRuntimeOwner::pac_mut))
    }

    /// Borrow the published owner only through the closed Wi-Fi MAC
    /// capability. The returned guard is the complete synchronous
    /// serialization interval; it exposes no PAC owner or generic callback.
    pub fn try_wifi_mac_hal(
        &self,
    ) -> Result<crate::ieee80211::mac::WifiMacHal<'_>, RadioOwnerArenaError> {
        match self.state() {
            RadioOwnerArenaState::Empty => {
                return Err(RadioOwnerArenaError::MissingOwner);
            }
            RadioOwnerArenaState::ResetRequired => {
                return Err(RadioOwnerArenaError::ResetRequired);
            }
            RadioOwnerArenaState::Published => {}
        }
        let slot = self
            .registers
            .try_borrow_mut()
            .map_err(|_| RadioOwnerArenaError::Borrowed)?;
        let owner = RefMut::filter_map(slot, Option::as_mut)
            .map_err(|_| RadioOwnerArenaError::MissingOwner)?;
        let registers = RefMut::map(owner, RadioRuntimeOwner::pac_mut);
        Ok(crate::ieee80211::mac::WifiMacHal::from_published(registers))
    }

    /// Borrow the published owner only through the closed station wake
    /// capability. The returned guard is the complete synchronous
    /// serialization interval.
    pub fn try_station_wake_hal(
        &self,
    ) -> Result<crate::ieee80211::station_wake::StationWakeHal<'_>, RadioOwnerArenaError> {
        match self.state() {
            RadioOwnerArenaState::Empty => {
                return Err(RadioOwnerArenaError::MissingOwner);
            }
            RadioOwnerArenaState::ResetRequired => {
                return Err(RadioOwnerArenaError::ResetRequired);
            }
            RadioOwnerArenaState::Published => {}
        }
        let slot = self
            .registers
            .try_borrow_mut()
            .map_err(|_| RadioOwnerArenaError::Borrowed)?;
        let owner = RefMut::filter_map(slot, Option::as_mut)
            .map_err(|_| RadioOwnerArenaError::MissingOwner)?;
        let (registers, state) =
            RefMut::map_split(owner, RadioRuntimeOwner::station_wake_parts_mut);
        Ok(crate::ieee80211::station_wake::StationWakeHal::from_published(registers, state))
    }

    /// Run one fallible, bounded observation without creating a copyable raw
    /// register capability.
    ///
    /// This is intended for value-only diagnostics at the integration
    /// boundary. It reports an inactive or synchronously borrowed arena
    /// instead of panicking, and the closure cannot retain the borrow across
    /// an async suspension.
    fn try_with_ref<T>(
        &self,
        transaction: impl FnOnce(&WifiRadioRegisters) -> T,
    ) -> Result<T, RadioOwnerArenaError> {
        match self.state() {
            RadioOwnerArenaState::Empty => {
                return Err(RadioOwnerArenaError::MissingOwner);
            }
            RadioOwnerArenaState::ResetRequired => {
                return Err(RadioOwnerArenaError::ResetRequired);
            }
            RadioOwnerArenaState::Published => {}
        }
        let slot = self
            .registers
            .try_borrow()
            .map_err(|_| RadioOwnerArenaError::Borrowed)?;
        let owner = slot.as_ref().ok_or(RadioOwnerArenaError::MissingOwner)?;
        Ok(transaction(owner.pac()))
    }

    /// Run one synchronous mutation only while the published lifecycle is
    /// live. Keeping this helper private prevents callers from turning the
    /// arena back into an unrestricted PAC callback API.
    fn try_with_mut<T>(
        &self,
        transaction: impl FnOnce(&mut WifiRadioRegisters) -> T,
    ) -> Result<T, RadioOwnerArenaError> {
        match self.state() {
            RadioOwnerArenaState::Empty => {
                return Err(RadioOwnerArenaError::MissingOwner);
            }
            RadioOwnerArenaState::ResetRequired => {
                return Err(RadioOwnerArenaError::ResetRequired);
            }
            RadioOwnerArenaState::Published => {}
        }
        let mut slot = self
            .registers
            .try_borrow_mut()
            .map_err(|_| RadioOwnerArenaError::Borrowed)?;
        let owner = slot.as_mut().ok_or(RadioOwnerArenaError::MissingOwner)?;
        Ok(transaction(owner.pac_mut()))
    }

    /// Read the reviewed station receive-policy projection without exposing a
    /// PAC owner or a generic closure at the integration boundary.
    pub fn try_station_receive_policy_snapshot(
        &self,
    ) -> Result<crate::ieee80211::mac::MacStaReceivePolicySnapshot, RadioOwnerArenaError> {
        self.try_with_ref(|registers| registers.sta_receive_policy_snapshot())
    }

    /// Read the reviewed MAC receive-statistics projection without exposing a
    /// PAC owner or a generic closure at the integration boundary.
    pub fn try_receive_statistics_snapshot(
        &self,
    ) -> Result<crate::ieee80211::mac::MacRxStatisticsSnapshot, RadioOwnerArenaError> {
        self.try_with_ref(|registers| registers.rx_statistics_snapshot())
    }

    /// Read the reviewed MAC RX walker projection without exposing a PAC
    /// owner or retaining the serialization guard at the caller.
    pub fn try_receive_dma_snapshot(
        &self,
    ) -> Result<crate::ieee80211::mac::MacRxDmaSnapshot, RadioOwnerArenaError> {
        self.try_with_ref(|registers| registers.mac_rx_dma_snapshot())
    }

    /// Apply the reviewed station-link receive policy as one serialized HAL
    /// transaction.
    pub fn try_configure_station_receive_policy(
        &self,
        bssid: [u8; 6],
    ) -> Result<(), RadioOwnerArenaError> {
        self.try_with_mut(|registers| {
            crate::ieee80211::mac::WifiMacHal::from_mac(registers)
                .configure_station_receive_policy(bssid);
        })
    }

    /// Apply normal STA filtering/auto-ACK restoration followed by the exact
    /// management-without-BSSID-check policy used by ESP-NOW normal RX.
    pub fn try_configure_station_esp_now_receive_policy(
        &self,
        bssid: [u8; 6],
    ) -> Result<(), RadioOwnerArenaError> {
        self.try_with_mut(|registers| {
            let mut hal = crate::ieee80211::mac::WifiMacHal::from_mac(registers);
            hal.configure_station_receive_policy(bssid);
            hal.configure_station_policy_six(bssid, crate::types::MacStaPolicyMode::Mode2);
        })
    }

    /// Read the current hardware noise floor as a bounded value observation.
    pub fn try_noise_floor_dbm(&self) -> Result<i8, RadioOwnerArenaError> {
        self.try_with_ref(|registers| registers.read_noise_floor_dbm())
    }

    /// Install one semantic station CCMP key under the runtime owner.
    pub fn try_install_station_ccmp_entry(
        &self,
        index: u8,
        identity: crate::types::MacCcmpKeyIdentity,
        temporal_key: &[u8; 16],
    ) -> Result<crate::ieee80211::mac::MacKeyInstallOutcome, RadioOwnerArenaError> {
        self.try_with_mut(|registers| {
            crate::ieee80211::mac::WifiMacHal::from_mac(registers).install_station_ccmp_entry(
                index,
                identity,
                temporal_key,
            )
        })
    }

    /// Install one semantic access-point CCMP key under runtime serialization
    /// owner. This is deliberately distinct from the station transaction:
    /// the vendor leaf enables the crypto engine for the selected MAC
    /// interface, so substituting interface zero corrupts simultaneous
    /// STA+AP transmit encryption even when the key-table entry itself is
    /// otherwise valid.
    pub fn try_install_access_point_ccmp_entry(
        &self,
        index: u8,
        identity: crate::types::MacCcmpKeyIdentity,
        temporal_key: &[u8; 16],
    ) -> Result<crate::ieee80211::mac::MacKeyInstallOutcome, RadioOwnerArenaError> {
        self.try_with_mut(|registers| {
            crate::ieee80211::mac::WifiMacHal::from_mac(registers).install_access_point_ccmp_entry(
                index,
                identity,
                temporal_key,
            )
        })
    }

    /// Clear one CCMP table entry under the runtime serialization owner.
    pub fn try_clear_ccmp_entry(&self, index: u8) -> Result<(), RadioOwnerArenaError> {
        self.try_with_mut(|registers| {
            crate::ieee80211::mac::WifiMacHal::from_mac(registers).clear_ccmp_entry(index);
        })
    }

    /// Observe one CCMP validity bit without exposing register authority.
    pub fn try_ccmp_entry_is_valid(&self, index: u8) -> Result<Option<bool>, RadioOwnerArenaError> {
        self.try_with_mut(|registers| {
            crate::ieee80211::mac::WifiMacHal::from_mac(registers).ccmp_entry_is_valid(index)
        })
    }

    /// Move one register owner into stable storage for a finite task epoch.
    pub fn publish(
        &self,
        owner: RadioRuntimeOwner,
    ) -> Result<PublishedRadioOwner<'_>, RadioOwnerPublishFailure> {
        let state = self.state();
        if state != RadioOwnerArenaState::Empty {
            return Err(RadioOwnerPublishFailure {
                error: match state {
                    RadioOwnerArenaState::Published => RadioOwnerArenaError::AlreadyPublished,
                    RadioOwnerArenaState::ResetRequired => RadioOwnerArenaError::ResetRequired,
                    RadioOwnerArenaState::Empty => unreachable!(),
                },
                owner,
            });
        }
        let mut slot = match self.registers.try_borrow_mut() {
            Ok(slot) => slot,
            Err(_) => {
                return Err(RadioOwnerPublishFailure {
                    error: RadioOwnerArenaError::Borrowed,
                    owner,
                });
            }
        };
        if slot.is_some() {
            self.state.store(RESET_REQUIRED, Ordering::Release);
            return Err(RadioOwnerPublishFailure {
                error: RadioOwnerArenaError::ResetRequired,
                owner,
            });
        }
        *slot = Some(owner);
        self.state.store(PUBLISHED, Ordering::Release);
        drop(slot);
        Ok(PublishedRadioOwner {
            arena: self,
            reclaim_required: true,
        })
    }
}

impl Default for RadioOwnerArena {
    fn default() -> Self {
        Self::new()
    }
}

/// Unique movable lease for one published register owner.
pub struct PublishedRadioOwner<'arena> {
    arena: &'arena RadioOwnerArena,
    reclaim_required: bool,
}

impl<'arena> PublishedRadioOwner<'arena> {
    /// Copyable bounded-transaction handle for child actors in the same
    /// finite role epoch. The root lease must not be reclaimed until every
    /// actor using this handle has acknowledged shutdown.
    pub const fn access(&self) -> RadioAccess<'arena> {
        RadioAccess { arena: self.arena }
    }

    /// Replace the station TSF in one serialized MAC transaction. Only the
    /// unique lease writes it, never a copyable [`RadioAccess`] or the
    /// arena: the station TSF has one writer above the HAL.
    pub fn set_station_tsf(&mut self, value: u64) -> Result<(), RadioOwnerArenaError> {
        self.arena.try_wifi_mac_hal()?.set_station_tsf(value);
        Ok(())
    }

    /// Return the exact PAC owner only while no synchronous register
    /// transaction is borrowed.
    pub fn try_reclaim(self) -> Result<RadioRuntimeOwner, (Self, RadioOwnerArenaError)> {
        self.try_reclaim_with_republish()
            .map(ReclaimedRadioOwner::into_owner)
    }

    /// Return the PAC owner together with the exact empty-arena capability.
    ///
    /// This is the owner-preserving boundary for a later role/task epoch. The
    /// ordinary [`try_reclaim`](Self::try_reclaim) remains useful when the
    /// caller deliberately tears down stable publication permanently.
    pub fn try_reclaim_with_republish(
        mut self,
    ) -> Result<ReclaimedRadioOwner<'arena>, (Self, RadioOwnerArenaError)> {
        let mut slot = match self.arena.registers.try_borrow_mut() {
            Ok(slot) => slot,
            Err(_) => return Err((self, RadioOwnerArenaError::Borrowed)),
        };
        let Some(owner) = slot.take() else {
            self.arena.state.store(RESET_REQUIRED, Ordering::Release);
            return Err((self, RadioOwnerArenaError::MissingOwner));
        };
        self.arena.state.store(EMPTY, Ordering::Release);
        self.reclaim_required = false;
        drop(slot);
        Ok(ReclaimedRadioOwner {
            owner,
            republish: RadioOwnerRepublish { arena: self.arena },
        })
    }
}

/// Non-owning transaction handle derived from one published lease.
#[derive(Clone, Copy)]
pub struct RadioAccess<'arena> {
    arena: &'arena RadioOwnerArena,
}

impl<'arena> RadioAccess<'arena> {
    /// Start one serialized channel transaction with the shared PHY borrowed
    /// from the arbiter lease.
    pub fn try_channel_hal<'access, P, T>(
        &'access self,
        platform: &'access mut P,
        lease: &'access mut SharedRadioLease<'_, T>,
    ) -> Result<crate::ieee80211::channel::RadioChannelHal<'access, P>, RadioOwnerArenaError> {
        self.arena.try_channel_hal(platform, lease)
    }

    /// [`Self::try_channel_hal`] together with the arbiter's attachment.
    pub fn try_channel_hal_with_attachment<'access, P, T>(
        &'access self,
        platform: &'access mut P,
        lease: &'access mut SharedRadioLease<'_, T>,
    ) -> Result<
        (
            crate::ieee80211::channel::RadioChannelHal<'access, P>,
            &'access mut T,
        ),
        RadioOwnerArenaError,
    > {
        self.arena.try_channel_hal_with_attachment(platform, lease)
    }

    /// Start one serialized Wi-Fi MAC transaction without exposing the
    /// published PAC owner.
    pub fn try_wifi_mac_hal(
        &self,
    ) -> Result<crate::ieee80211::mac::WifiMacHal<'arena>, RadioOwnerArenaError> {
        self.arena.try_wifi_mac_hal()
    }

    /// Start one serialized station wake transaction without exposing the
    /// published PAC owner.
    pub fn try_station_wake_hal(
        &self,
    ) -> Result<crate::ieee80211::station_wake::StationWakeHal<'arena>, RadioOwnerArenaError> {
        self.arena.try_station_wake_hal()
    }

    /// Prepare the finite connected-STA interrupt state while the runtime
    /// register owner remains serialized inside the arena.
    pub fn try_prepare_connected_sta_without_power_save(
        &self,
        setup: &mut crate::owner::MacInterruptSetup,
    ) -> Result<crate::owner::ConnectedStaInterruptPrepared, RadioOwnerArenaError> {
        self.arena
            .try_with_mut(|registers| setup.prepare_connected_sta_with_pac(registers))
    }

    /// Apply connected-STA interrupt policy through the live ISR capability
    /// while the runtime register owner remains serialized in the arena.
    pub fn try_prepare_active_connected_sta_without_power_save(
        &self,
        interrupt: &mut crate::owner::MacInterruptRegisters,
    ) -> Result<crate::owner::ConnectedStaInterruptPrepared, RadioOwnerArenaError> {
        self.arena
            .try_with_mut(|registers| interrupt.prepare_connected_sta_with_pac(registers))
    }

    /// Read the associated-STA policy as a value-only HAL observation.
    pub fn try_station_receive_policy_snapshot(
        &self,
    ) -> Result<crate::ieee80211::mac::MacStaReceivePolicySnapshot, RadioOwnerArenaError> {
        self.arena.try_station_receive_policy_snapshot()
    }

    /// Read receive counters as a value-only HAL observation.
    pub fn try_receive_statistics_snapshot(
        &self,
    ) -> Result<crate::ieee80211::mac::MacRxStatisticsSnapshot, RadioOwnerArenaError> {
        self.arena.try_receive_statistics_snapshot()
    }

    /// Read the RX DMA walker as a value-only HAL observation.
    pub fn try_receive_dma_snapshot(
        &self,
    ) -> Result<crate::ieee80211::mac::MacRxDmaSnapshot, RadioOwnerArenaError> {
        self.arena.try_receive_dma_snapshot()
    }

    /// Apply the reviewed associated-STA receive policy.
    pub fn try_configure_station_receive_policy(
        &self,
        bssid: [u8; 6],
    ) -> Result<(), RadioOwnerArenaError> {
        self.arena.try_configure_station_receive_policy(bssid)
    }

    /// Apply serialized normal STA plus ESP-NOW management admission policy.
    pub fn try_configure_station_esp_now_receive_policy(
        &self,
        bssid: [u8; 6],
    ) -> Result<(), RadioOwnerArenaError> {
        self.arena
            .try_configure_station_esp_now_receive_policy(bssid)
    }

    /// Read the hardware noise floor without exposing the PAC owner.
    pub fn try_noise_floor_dbm(&self) -> Result<i8, RadioOwnerArenaError> {
        self.arena.try_noise_floor_dbm()
    }

    /// Install one semantic station CCMP key without exposing the table owner.
    pub fn try_install_station_ccmp_entry(
        &self,
        index: u8,
        identity: crate::types::MacCcmpKeyIdentity,
        temporal_key: &[u8; 16],
    ) -> Result<crate::ieee80211::mac::MacKeyInstallOutcome, RadioOwnerArenaError> {
        self.arena
            .try_install_station_ccmp_entry(index, identity, temporal_key)
    }

    /// Install one semantic access-point CCMP key without exposing the table
    /// owner or erasing the role-specific crypto-interface transaction.
    pub fn try_install_access_point_ccmp_entry(
        &self,
        index: u8,
        identity: crate::types::MacCcmpKeyIdentity,
        temporal_key: &[u8; 16],
    ) -> Result<crate::ieee80211::mac::MacKeyInstallOutcome, RadioOwnerArenaError> {
        self.arena
            .try_install_access_point_ccmp_entry(index, identity, temporal_key)
    }

    /// Clear one CCMP table entry without exposing the key-table owner.
    pub fn try_clear_ccmp_entry(&self, index: u8) -> Result<(), RadioOwnerArenaError> {
        self.arena.try_clear_ccmp_entry(index)
    }

    /// Observe one CCMP validity bit without exposing the key-table owner.
    pub fn try_ccmp_entry_is_valid(&self, index: u8) -> Result<Option<bool>, RadioOwnerArenaError> {
        self.arena.try_ccmp_entry_is_valid(index)
    }
}

impl Drop for PublishedRadioOwner<'_> {
    fn drop(&mut self) {
        if self.reclaim_required {
            self.arena.state.store(RESET_REQUIRED, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests;
