//! Bluetooth as a client of the shared PHY domain.
//!
//! The HAL chain holds the Bluetooth controller partition; the registered PHY
//! domain lives in the radio domain as [`ConcurrentPhy`]. The Controller enable
//! runs `esp_phy_enable(PHY_MODEM_BT)` and `esp_btbb_enable` after the
//! controller HAL is initialized and before the BLE base stack is enabled.
//! [`join_bluetooth`] performs those steps on the task owner, which proves the
//! Bluetooth module clocks, and issues the affine [`BluetoothPhyMembership`];
//! [`leave_bluetooth`] consumes it again.
//!
//! Unlike IEEE 802.15.4, Bluetooth keeps the transmit-on delay that the
//! first BTBB initialization writes.

use oer_esp32s31_hal::power::PlatformClockBackend;

use core::fmt;

use oer_esp32s31_hal::{
    bluetooth::TaskOwner,
    domain::{BtbbError, RadioClient, RadioDomain},
};

use crate::concurrent::{
    BtbbClientAcquireError, ConcurrentPhy, ConcurrentPhyError, JoinRefused, Joined, PhyClientReady,
    acquire_btbb_client, release_client,
};

/// Bluetooth holds a client bit in the shared PHY domain and a reference on
/// the shared BTBB baseband.
///
/// ```compile_fail
/// use oer_esp32s31_phy::bluetooth_client::BluetoothPhyMembership;
/// fn requires_clone<T: Clone>() {}
/// requires_clone::<BluetoothPhyMembership>();
/// ```
#[must_use = "the Bluetooth PHY membership must be left through leave_bluetooth"]
pub struct BluetoothPhyMembership {
    key: crate::state::client::PhyClientKey,
}

impl fmt::Debug for BluetoothPhyMembership {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BluetoothPhyMembership")
    }
}

/// Exclusive access to a settled domain for this Bluetooth membership.
/// Pending initial tracking has no RF-ready transition.
#[must_use]
pub struct BluetoothRfReady<'r, B: PlatformClockBackend> {
    radio: &'r mut RadioDomain<B, ConcurrentPhy>,
    _membership: &'r BluetoothPhyMembership,
}

impl<'r, B: PlatformClockBackend> BluetoothRfReady<'r, B> {
    pub fn into_radio(self) -> &'r mut RadioDomain<B, ConcurrentPhy> {
        self.radio
    }
}

/// Check the exact registration, active client and BTBB reference before a
/// BLE PHY write. The exclusive borrow prevents tracking or RF close racing it.
pub fn bluetooth_rf_ready<'r, B: PlatformClockBackend>(
    radio: &'r mut RadioDomain<B, ConcurrentPhy>,
    membership: &'r BluetoothPhyMembership,
) -> Result<BluetoothRfReady<'r, B>, ConcurrentPhyError> {
    let domain = radio.attachment().settled()?;
    if !domain.clients.describes_epoch(radio.registration_epoch())
        || domain.clients.permission_key() != membership.key
    {
        return Err(ConcurrentPhyError::EpochMismatch);
    }
    if !domain.clients.snapshot().contains(RadioClient::Bluetooth)
        || !radio.holds_btbb(RadioClient::Bluetooth)
    {
        return Err(ConcurrentPhyError::ClientAbsent(RadioClient::Bluetooth));
    }
    Ok(BluetoothRfReady {
        radio,
        _membership: membership,
    })
}

/// Why Bluetooth could not join or leave the shared PHY and BTBB.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothPhyClientError {
    /// The PHY domain rejected the operation.
    Phy(ConcurrentPhyError),
    /// The BTBB reference is not in the state the operation requires.
    Btbb(BtbbError),
}

/// Enter Bluetooth into the shared PHY domain and the shared BTBB baseband.
///
/// This is `esp_phy_enable(PHY_MODEM_BT)` followed by `esp_btbb_enable`. The
/// returned [`ConcurrentAcquire::TrackingDue`](crate::concurrent::ConcurrentAcquire::TrackingDue) means the domain must run its
/// tracking before the Controller may use RF.
///
/// # Errors
///
/// Every check runs before the client set or BTBB changes: Bluetooth already
/// holds BTBB, or the client set rejects the client. The radio comes back
/// either way.
pub fn join_bluetooth<'l, B: PlatformClockBackend>(
    ready: PhyClientReady<'l, B, BluetoothPhyMembership>,
    task: &TaskOwner,
    clock: &impl oer_time::Clock,
) -> Result<Joined<'l, B, BluetoothPhyMembership>, JoinRefused<'l, B, BluetoothPhyClientError>> {
    let radio = ready.into_radio();
    let acquired = match acquire_btbb_client(radio, RadioClient::Bluetooth, clock, |radio, gain| {
        task.acquire_btbb(radio, gain)
    }) {
        Ok(acquired) => acquired,
        Err(error) => {
            let error = match error {
                BtbbClientAcquireError::Phy(error) => BluetoothPhyClientError::Phy(error),
                BtbbClientAcquireError::Btbb(error) => BluetoothPhyClientError::Btbb(error),
            };
            return Err(JoinRefused { error, radio });
        }
    };
    let key = radio.attachment().permission_key();
    Ok(Joined {
        membership: BluetoothPhyMembership { key },
        acquired,
        radio,
    })
}

/// Failed leave retaining the membership.
#[must_use = "a failed leave still holds the Bluetooth PHY membership"]
pub struct BluetoothPhyLeaveFailure {
    membership: BluetoothPhyMembership,
    error: BluetoothPhyClientError,
}

impl BluetoothPhyLeaveFailure {
    pub const fn error(&self) -> BluetoothPhyClientError {
        self.error
    }

    /// Recover the membership for a retry.
    pub fn into_membership(self) -> BluetoothPhyMembership {
        self.membership
    }
}

impl fmt::Debug for BluetoothPhyLeaveFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BluetoothPhyLeaveFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

/// Leave the shared PHY domain and drop the BTBB reference. Returns whether
/// Bluetooth was the last PHY client.
///
/// This is `esp_btbb_disable` and `esp_phy_disable(PHY_MODEM_BT)`; neither
/// performs register access here.
///
/// # Errors
///
/// The domain rejects the release (tracking pending, poisoned) or the BTBB
/// reference is missing; nothing changed and the membership is returned.
pub fn leave_bluetooth<B: PlatformClockBackend>(
    radio: &mut RadioDomain<B, ConcurrentPhy>,
    task: &TaskOwner,
    membership: BluetoothPhyMembership,
) -> Result<bool, BluetoothPhyLeaveFailure> {
    if !radio.holds_btbb(RadioClient::Bluetooth) {
        return Err(BluetoothPhyLeaveFailure {
            membership,
            error: BluetoothPhyClientError::Btbb(BtbbError::NotAcquired),
        });
    }
    let key = membership.key;
    let last = match release_client(radio, RadioClient::Bluetooth, key) {
        Ok(last) => last,
        Err(error) => {
            return Err(BluetoothPhyLeaveFailure {
                membership,
                error: BluetoothPhyClientError::Phy(error),
            });
        }
    };
    let BluetoothPhyMembership { key: _ } = membership;
    // The reference was checked above, so the release cannot be rejected.
    let _ = task.release_btbb(radio);
    Ok(last)
}

#[cfg(test)]
impl BluetoothPhyMembership {
    fn for_test() -> Self {
        Self {
            key: crate::state::client::model_permission_key_for_test(),
        }
    }
}

#[cfg(test)]
mod tests;
