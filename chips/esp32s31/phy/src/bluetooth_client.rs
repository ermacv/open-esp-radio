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

use core::fmt;

use oer_esp32s31_hal::{
    bluetooth::TaskOwner,
    domain::{BtbbError, RadioClient, RadioDomain},
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
/// holds BTBB, or the client set rejects the client. The radio comes back
/// either way.
pub fn join_bluetooth<'l>(
    ready: PhyReady<'l>,
    task: &TaskOwner,
    clock: &impl oer_time::Clock,
) -> Result<Joined<'l, BluetoothPhyMembership>, JoinRefused<'l, BluetoothPhyClientError>> {
    let radio = ready.into_radio();
    // The ready proof checked that the settled registration describes the
    // radio.
    if let Err(error) = radio.attachment().settled() {
        return Err(JoinRefused {
            error: BluetoothPhyClientError::Phy(error),
            radio,
        });
    }
    if radio.holds_btbb(RadioClient::Bluetooth) {
        return Err(JoinRefused {
            error: BluetoothPhyClientError::Btbb(BtbbError::AlreadyAcquired),
            radio,
        });
    }
    let acquired = match acquire_client(radio, RadioClient::Bluetooth, clock) {
        Ok(acquired) => acquired,
        Err(error) => {
            return Err(JoinRefused {
                error: BluetoothPhyClientError::Phy(error),
                radio,
            });
        }
    };

    // The settled registered state publishes its current gain byte right
    // before the acquisition, so BTBB starts with the value the vendor reads
    // from `phy_param[0x120]` at that moment; a tracking or registration in
    // progress leaves the domain unsettled, and no gain is published then.
    let gain = {
        let (phy, publisher) = radio.btbb_publisher();
        phy.settled()
            .map_or(Err(BtbbError::Unregistered), |domain| {
                domain.registered.publish_btbb_gain(publisher)
            })
    };
    let btbb = gain.and_then(|gain| task.acquire_btbb(radio, &gain));
    if let Err(error) = btbb {
        // Both rejections were checked above; undo the client bit when the
        // domain still permits it.
        let _ = release_client(radio, RadioClient::Bluetooth);
        return Err(JoinRefused {
            error: BluetoothPhyClientError::Btbb(error),
            radio,
        });
    }
    Ok(Joined {
        membership: BluetoothPhyMembership { _private: () },
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
pub fn leave_bluetooth(
    radio: &mut RadioDomain<ConcurrentPhy>,
    task: &TaskOwner,
    membership: BluetoothPhyMembership,
) -> Result<bool, BluetoothPhyLeaveFailure> {
    if !radio.holds_btbb(RadioClient::Bluetooth) {
        return Err(BluetoothPhyLeaveFailure {
            membership,
            error: BluetoothPhyClientError::Btbb(BtbbError::NotAcquired),
        });
    }
    let last = match release_client(radio, RadioClient::Bluetooth) {
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
    let _ = task.release_btbb(radio);
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
