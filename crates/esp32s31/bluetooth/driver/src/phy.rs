//! Bluetooth membership in the shared PHY domain and BTBB baseband.
//!
//! The Controller enable runs `esp_phy_enable(PHY_MODEM_BT)` and
//! `esp_btbb_enable` after the controller HAL, scheduler and low-power hardware
//! are initialized and before the BLE base stack is enabled. The shared radio's
//! owner must register or wake the shared domain first; this module only enters
//! and leaves it. The BTBB gain byte is projected by the PHY client from the
//! registration that describes the lease.

use oer_esp32s31_hal::shared_radio::SharedRadioLease;
use oer_esp32s31_phy::{
    bluetooth_client::{
        BluetoothPhyClientError, BluetoothPhyLeaveFailure, BluetoothPhyMembership, join_bluetooth,
        leave_bluetooth,
    },
    concurrent::{ConcurrentAcquire, ConcurrentPhy, PhyReady},
};

use crate::{low_power::ControllerLowPowerHardwareInitialized, resources::TaskResources};

/// Powered Controller that holds a client bit in the shared PHY domain and a
/// reference on the shared BTBB baseband.
///
/// The BLE PHY engine is initialized from this state. Its membership stays
/// with the Controller until `leave_phy` at shutdown.
///
/// The engine, and with it the installed Controller, starts only from a
/// joined Controller:
///
/// ```no_run
/// # #![cfg_attr(target_arch = "riscv32", no_std)]
/// # #![cfg_attr(target_arch = "riscv32", no_main)]
/// # #[cfg(target_arch = "riscv32")]
/// # #[panic_handler]
/// # fn panic(_: &core::panic::PanicInfo) -> ! {
/// #     loop {}
/// # }
/// use oer_esp32s31_bluetooth::phy::ControllerPhyJoined;
/// let _engine = ControllerPhyJoined::<'static, 4>::initialize_ble_phy_engine::<()>;
/// ```
///
/// ```compile_fail,E0599
/// # #![cfg_attr(target_arch = "riscv32", no_std)]
/// # #![cfg_attr(target_arch = "riscv32", no_main)]
/// # #[cfg(target_arch = "riscv32")]
/// # #[panic_handler]
/// # fn panic(_: &core::panic::PanicInfo) -> ! {
/// #     loop {}
/// # }
/// use oer_esp32s31_bluetooth::low_power::ControllerLowPowerHardwareInitialized;
/// let _engine =
///     ControllerLowPowerHardwareInitialized::<'static, 4>::initialize_ble_phy_engine::<()>;
/// ```
///
/// And the membership leaves only with the Controller's retirement, never
/// from the joined owner:
///
/// ```no_run
/// # #![cfg_attr(target_arch = "riscv32", no_std)]
/// # #![cfg_attr(target_arch = "riscv32", no_main)]
/// # #[cfg(target_arch = "riscv32")]
/// # #[panic_handler]
/// # fn panic(_: &core::panic::PanicInfo) -> ! {
/// #     loop {}
/// # }
/// use oer_esp32s31_bluetooth::phy::ControllerPhyJoined;
/// fn keep(joined: ControllerPhyJoined<'static, 4>) -> ControllerPhyJoined<'static, 4> {
///     joined
/// }
/// ```
///
/// ```compile_fail,E0616
/// # #![cfg_attr(target_arch = "riscv32", no_std)]
/// # #![cfg_attr(target_arch = "riscv32", no_main)]
/// # #[cfg(target_arch = "riscv32")]
/// # #[panic_handler]
/// # fn panic(_: &core::panic::PanicInfo) -> ! {
/// #     loop {}
/// # }
/// use oer_esp32s31_bluetooth::phy::ControllerPhyJoined;
/// use oer_esp32s31_phy::bluetooth_client::BluetoothPhyMembership;
/// fn leave_early(joined: ControllerPhyJoined<'static, 4>) -> BluetoothPhyMembership {
///     joined.membership
/// }
/// ```
#[must_use = "the joined Controller retains its PHY membership"]
pub struct ControllerPhyJoined<'cells, const MODEM_TIMER_CAPACITY: usize> {
    pub(crate) controller: ControllerLowPowerHardwareInitialized<'cells, MODEM_TIMER_CAPACITY>,
    pub(crate) membership: BluetoothPhyMembership,
}

/// Rejected join retaining the powered Controller.
///
/// The Controller has run its HAL, scheduler and low-power initialization,
/// which no verified transition reverses before the shutdown of a running
/// epoch; the retained owner is fail-stop.
#[must_use = "a failed PHY join retains the powered Controller"]
pub struct ControllerPhyJoinFailure<'cells, const MODEM_TIMER_CAPACITY: usize> {
    _controller: ControllerLowPowerHardwareInitialized<'cells, MODEM_TIMER_CAPACITY>,
    error: BluetoothPhyClientError,
}

impl<const MODEM_TIMER_CAPACITY: usize> ControllerPhyJoinFailure<'_, MODEM_TIMER_CAPACITY> {
    /// The rejection; the client set and BTBB are unchanged.
    pub const fn error(&self) -> BluetoothPhyClientError {
        self.error
    }
}

impl<'cells, const MODEM_TIMER_CAPACITY: usize>
    ControllerLowPowerHardwareInitialized<'cells, MODEM_TIMER_CAPACITY>
{
    /// Enter the shared PHY domain and BTBB as the Bluetooth client, with
    /// the proof that the shared radio's owner prepared the domain.
    ///
    /// [`ConcurrentAcquire::TrackingDue`] means the domain must run its
    /// tracking before the Controller uses RF.
    ///
    /// # Errors
    ///
    /// The proof names another registration, the domain is not settled for
    /// the lease, Bluetooth already holds BTBB, or the client set rejected
    /// the client.
    #[allow(
        clippy::result_large_err,
        reason = "the failure retains the complete allocation-free Controller epoch"
    )]
    pub fn join_phy(
        self,
        ready: PhyReady<'_, '_>,
        clock: &impl oer_time::Clock,
    ) -> Result<
        (
            ControllerPhyJoined<'cells, MODEM_TIMER_CAPACITY>,
            ConcurrentAcquire,
        ),
        ControllerPhyJoinFailure<'cells, MODEM_TIMER_CAPACITY>,
    > {
        match join_bluetooth(ready, self.task().hal(), clock) {
            Ok(joined) => Ok((
                ControllerPhyJoined {
                    controller: self,
                    membership: joined.membership,
                },
                joined.acquired,
            )),
            Err(refused) => Err(ControllerPhyJoinFailure {
                _controller: self,
                error: refused.error,
            }),
        }
    }
}

/// Leave the shared PHY domain and drop the BTBB reference of a retired
/// Controller. Returns whether Bluetooth was the last PHY client, in which
/// case the shared radio's owner closes RF.
///
/// # Errors
///
/// The domain rejected the release; nothing changed and the membership is
/// returned.
pub(crate) fn leave_phy(
    lease: &mut SharedRadioLease<'_, ConcurrentPhy>,
    task: &TaskResources,
    membership: BluetoothPhyMembership,
) -> Result<bool, BluetoothPhyLeaveFailure> {
    leave_bluetooth(lease, task.hal(), membership)
}
