//! Bluetooth as a client of the shared PHY domain.
//!
//! The HAL chain holds the Bluetooth controller partition; the registered PHY
//! domain lives under the arbiter as [`ConcurrentPhy`]. The Controller enable
//! runs `esp_phy_enable(PHY_MODEM_BT)` and `esp_btbb_enable` after the
//! controller HAL is initialized and before the BLE base stack is enabled.
//! [`join_bluetooth`] performs those steps on the task owner, which proves the
//! Bluetooth module clocks, and issues the affine [`BluetoothPhyMembership`];
//! [`leave_bluetooth`] consumes it again.
//!
//! Unlike IEEE 802.15.4, Bluetooth keeps the transmit-on delay that the
//! first BTBB initialization writes.

use core::fmt;

use oer_esp32s31_hal::{
    bluetooth::TaskOwner,
    shared_radio::{BtbbError, RadioClient, SharedRadioLease},
};

use crate::concurrent::{
    ConcurrentPhy, ConcurrentPhyError, JoinRefused, Joined, PhyReady, acquire_client,
    release_client,
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
    _private: (),
}

impl fmt::Debug for BluetoothPhyMembership {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BluetoothPhyMembership")
    }
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
/// holds BTBB, or the client set rejects the client. The lease comes back
/// either way.
pub fn join_bluetooth<'l, 'r>(
    ready: PhyReady<'l, 'r>,
    task: &TaskOwner,
    clock: &impl oer_time::Clock,
) -> Result<Joined<'l, 'r, BluetoothPhyMembership>, JoinRefused<'l, 'r, BluetoothPhyClientError>> {
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
                error: BluetoothPhyClientError::Phy(error),
                lease,
            });
        }
    };
    if lease.holds_btbb(RadioClient::Bluetooth) {
        return Err(JoinRefused {
            error: BluetoothPhyClientError::Btbb(BtbbError::AlreadyAcquired),
            lease,
        });
    }
    let acquired = match acquire_client(lease, RadioClient::Bluetooth, clock) {
        Ok(acquired) => acquired,
        Err(error) => {
            return Err(JoinRefused {
                error: BluetoothPhyClientError::Phy(error),
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
        // that same registered state. `task` proves the Bluetooth module
        // clocks.
        unsafe { task.acquire_btbb(lease, gain_parameter) }
    };
    if let Err(error) = btbb {
        // Both rejections were checked above; undo the client bit when the
        // domain still permits it.
        let _ = release_client(lease, RadioClient::Bluetooth);
        return Err(JoinRefused {
            error: BluetoothPhyClientError::Btbb(error),
            lease,
        });
    }
    Ok(Joined {
        membership: BluetoothPhyMembership { _private: () },
        acquired,
        lease,
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
pub fn leave_bluetooth(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    task: &TaskOwner,
    membership: BluetoothPhyMembership,
) -> Result<bool, BluetoothPhyLeaveFailure> {
    if !lease.holds_btbb(RadioClient::Bluetooth) {
        return Err(BluetoothPhyLeaveFailure {
            membership,
            error: BluetoothPhyClientError::Btbb(BtbbError::NotAcquired),
        });
    }
    let last = match release_client(lease, RadioClient::Bluetooth) {
        Ok(last) => last,
        Err(error) => {
            return Err(BluetoothPhyLeaveFailure {
                membership,
                error: BluetoothPhyClientError::Phy(error),
            });
        }
    };
    let BluetoothPhyMembership { _private: () } = membership;
    // The reference was checked above, so the release cannot be rejected.
    let _ = task.release_btbb(lease);
    Ok(last)
}

#[cfg(test)]
impl BluetoothPhyMembership {
    fn for_test() -> Self {
        Self { _private: () }
    }
}

#[cfg(test)]
mod tests;
