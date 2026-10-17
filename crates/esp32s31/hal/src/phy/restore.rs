//! One restore authority shared by mutually exclusive PHY calibrations.
//!
//! TX-DC PWDET, TX-IQ tone control, RX-DCO control and Bluetooth TX-power
//! control each capture register fields, replace them for a calibration and
//! restore them afterwards. The restricted PAC performs only the capture,
//! replacement and restore transactions. This slot decides which calibration
//! owns the single restore obligation and rejects an interloper before any
//! register access. A route cannot release the neutral radio root while the
//! slot is occupied.

use super::registration::{PhyRegistration, PhyRegistrationEpoch};
use oer_esp32s31_pac::{RxDcoControlField, TxDcPwdetFields, TxIqToneControlFields};

/// Preparing TX-DC PWDET was rejected before any register access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxDcPwdetPrepareError {
    /// Another calibration still owns the one pending restore operation.
    RestorePending,
}

/// Restoring TX-DC PWDET was rejected before any register access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxDcPwdetRestoreError {
    /// No successful prepare operation owns saved fields.
    RestoreNotPending,
}

/// A lifecycle operation would overwrite TX-DC fields awaiting restore.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxDcPwdetLifecycleError {
    RestorePending,
}

/// Preparing a TX-IQ tone-control restore was rejected before register access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxIqToneControlPrepareError {
    /// Another calibration still owns the pending restore operation.
    RestorePending,
}

/// Restoring TX-IQ tone control was rejected before register access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxIqToneControlRestoreError {
    /// No successful prepare operation owns saved field state.
    RestoreNotPending,
}

/// Preparing an RX-DCO control restore was rejected before register access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxDcoControlPrepareError {
    /// A different calibration owns the shared restore slot.
    RestorePending,
    /// Both reviewed RX-DCO nesting levels already own saved fields.
    RestoreStackFull,
}

/// Restoring RX-DCO control was rejected before register access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxDcoControlRestoreError {
    /// No RX-DCO control field is awaiting restoration.
    RestoreNotPending,
}

/// Preparing the Bluetooth TX-power analog-control restore was rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothTxPowerControlPrepareError {
    /// Another calibration still owns the shared restore slot.
    RestorePending,
}

/// Using the Bluetooth TX-power analog-control restore was rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothTxPowerControlRestoreError {
    /// No prepared Bluetooth TX-power analog-control restore is pending.
    RestoreNotPending,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
enum Restore {
    #[default]
    Empty,
    TxDcPwdet(TxDcPwdetFields),
    TxIqToneControl(TxIqToneControlFields),
    RxDcoControlOne(RxDcoControlField),
    RxDcoControlTwo(RxDcoControlField, RxDcoControlField),
    BluetoothTxPowerControl {
        low: u8,
        high: u8,
    },
}

/// HAL software state beside the shared PHY partition for one route: the
/// single calibration restore obligation and the PHY registration epoch.
#[derive(Debug)]
pub struct PhyRouteState {
    restore: Restore,
    registration: PhyRegistration,
}

impl PhyRouteState {
    /// Enter a route with the root's registration counter and no restore.
    pub(crate) const fn new(registration: PhyRegistration) -> Self {
        Self {
            restore: Restore::Empty,
            registration,
        }
    }

    /// Fresh state for an isolated validation image or a host test.
    #[cfg(any(test, feature = "validation-probes"))]
    pub(crate) const fn for_validation() -> Self {
        Self::new(PhyRegistration::new())
    }

    /// Leave the route: retire the current registration and return the
    /// counter to the neutral root. The caller has checked that no restore
    /// obligation remains.
    pub(crate) fn into_registration(self) -> PhyRegistration {
        debug_assert_eq!(self.restore, Restore::Empty);
        self.registration.retired()
    }

    /// The registration that currently describes the route's PHY, if any.
    pub(crate) const fn registration_epoch(&self) -> Option<PhyRegistrationEpoch> {
        self.registration.current()
    }

    /// Begin a registration and retire every earlier epoch.
    pub(crate) fn begin_registration_epoch(&mut self) -> PhyRegistrationEpoch {
        self.registration.begin()
    }

    pub(crate) const fn txdc_pending(&self) -> bool {
        matches!(self.restore, Restore::TxDcPwdet(_))
    }

    pub(crate) const fn txiq_pending(&self) -> bool {
        matches!(self.restore, Restore::TxIqToneControl(_))
    }

    pub(crate) const fn rx_dco_pending(&self) -> bool {
        matches!(
            self.restore,
            Restore::RxDcoControlOne(_) | Restore::RxDcoControlTwo(..)
        )
    }

    pub(crate) const fn bluetooth_tx_power_control_pending(&self) -> bool {
        matches!(self.restore, Restore::BluetoothTxPowerControl { .. })
    }

    /// Capture, record and then replace the TX-DC PWDET fields.
    ///
    /// The slot is filled before either temporary field is published. A
    /// second caller is rejected without touching MMIO, so it cannot steal
    /// the first caller's restore authority.
    #[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
    pub(crate) fn prepare_txdc_with<Port: ?Sized>(
        &mut self,
        port: &mut Port,
        capture: impl FnOnce(&Port) -> TxDcPwdetFields,
        apply: impl FnOnce(&mut Port),
    ) -> Result<(), TxDcPwdetPrepareError> {
        if self.restore != Restore::Empty {
            return Err(TxDcPwdetPrepareError::RestorePending);
        }
        self.restore = Restore::TxDcPwdet(capture(port));
        apply(port);
        Ok(())
    }

    /// Restore the saved TX-DC fields and only then release the slot.
    #[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
    pub(crate) fn restore_txdc_with(
        &mut self,
        restore: impl FnOnce(TxDcPwdetFields),
    ) -> Result<(), TxDcPwdetRestoreError> {
        let Restore::TxDcPwdet(fields) = self.restore else {
            return Err(TxDcPwdetRestoreError::RestoreNotPending);
        };
        restore(fields);
        self.restore = Restore::Empty;
        Ok(())
    }

    pub(crate) fn prepare_txiq_with(
        &mut self,
        capture: impl FnOnce() -> TxIqToneControlFields,
    ) -> Result<(), TxIqToneControlPrepareError> {
        if self.restore != Restore::Empty {
            return Err(TxIqToneControlPrepareError::RestorePending);
        }
        self.restore = Restore::TxIqToneControl(capture());
        Ok(())
    }

    pub(crate) fn restore_txiq_with(
        &mut self,
        restore: impl FnOnce(TxIqToneControlFields),
    ) -> Result<(), TxIqToneControlRestoreError> {
        let Restore::TxIqToneControl(fields) = self.restore else {
            return Err(TxIqToneControlRestoreError::RestoreNotPending);
        };
        restore(fields);
        self.restore = Restore::Empty;
        Ok(())
    }

    /// Push one RX-DCO control capture onto the two-entry LIFO.
    ///
    /// The crystal-duty operation masks this field around a nested RX-DCO
    /// calibration which independently performs the same sequence.
    #[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
    pub(crate) fn prepare_rx_dco_with(
        &mut self,
        capture: impl FnOnce() -> RxDcoControlField,
    ) -> Result<(), RxDcoControlPrepareError> {
        self.restore = match self.restore {
            Restore::Empty => Restore::RxDcoControlOne(capture()),
            Restore::RxDcoControlOne(first) => Restore::RxDcoControlTwo(first, capture()),
            Restore::RxDcoControlTwo(..) => return Err(RxDcoControlPrepareError::RestoreStackFull),
            _ => return Err(RxDcoControlPrepareError::RestorePending),
        };
        Ok(())
    }

    /// Restore the most recent RX-DCO control capture.
    #[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
    pub(crate) fn restore_rx_dco_with(
        &mut self,
        restore: impl FnOnce(RxDcoControlField),
    ) -> Result<(), RxDcoControlRestoreError> {
        let (field, next) = match self.restore {
            Restore::RxDcoControlOne(first) => (first, Restore::Empty),
            Restore::RxDcoControlTwo(first, second) => (second, Restore::RxDcoControlOne(first)),
            _ => return Err(RxDcoControlRestoreError::RestoreNotPending),
        };
        restore(field);
        self.restore = next;
        Ok(())
    }

    pub(crate) fn prepare_bluetooth_tx_power_control(
        &mut self,
    ) -> Result<(), BluetoothTxPowerControlPrepareError> {
        if self.restore != Restore::Empty {
            return Err(BluetoothTxPowerControlPrepareError::RestorePending);
        }
        self.restore = Restore::BluetoothTxPowerControl { low: 0, high: 0 };
        Ok(())
    }

    pub(crate) fn capture_bluetooth_tx_power_control_low(
        &mut self,
        value: u8,
    ) -> Result<(), BluetoothTxPowerControlRestoreError> {
        let Restore::BluetoothTxPowerControl { low, .. } = &mut self.restore else {
            return Err(BluetoothTxPowerControlRestoreError::RestoreNotPending);
        };
        *low = value;
        Ok(())
    }

    pub(crate) fn capture_bluetooth_tx_power_control_high(
        &mut self,
        value: u8,
    ) -> Result<(), BluetoothTxPowerControlRestoreError> {
        let Restore::BluetoothTxPowerControl { high, .. } = &mut self.restore else {
            return Err(BluetoothTxPowerControlRestoreError::RestoreNotPending);
        };
        *high = value;
        Ok(())
    }

    pub(crate) fn bluetooth_tx_power_control_values(
        &self,
    ) -> Result<(u8, u8), BluetoothTxPowerControlRestoreError> {
        let Restore::BluetoothTxPowerControl { low, high } = self.restore else {
            return Err(BluetoothTxPowerControlRestoreError::RestoreNotPending);
        };
        Ok((low, high))
    }

    pub(crate) fn finish_bluetooth_tx_power_control_restore(
        &mut self,
    ) -> Result<(), BluetoothTxPowerControlRestoreError> {
        if !self.bluetooth_tx_power_control_pending() {
            return Err(BluetoothTxPowerControlRestoreError::RestoreNotPending);
        }
        self.restore = Restore::Empty;
        Ok(())
    }

    /// Occupy the slot with one pending TX-DC restore for route tests.
    #[cfg(test)]
    pub(crate) fn occupy_txdc_for_test(&mut self) {
        self.restore = Restore::TxDcPwdet(TxDcPwdetFields::default());
    }

    #[cfg(test)]
    pub(crate) fn occupy_txiq_for_test(&mut self) {
        self.restore = Restore::TxIqToneControl(TxIqToneControlFields::default());
    }

    #[cfg(test)]
    pub(crate) fn occupy_rx_dco_for_test(&mut self) {
        self.restore = Restore::RxDcoControlOne(RxDcoControlField::default());
    }

    #[cfg(test)]
    pub(crate) fn occupy_bluetooth_tx_power_control_for_test(&mut self) {
        self.restore = Restore::BluetoothTxPowerControl { low: 0, high: 0 };
    }
}

#[cfg(test)]
mod tests;
