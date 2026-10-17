//! Transactional Wi-Fi PHY-I2C temperature tracking for ESP32-S31.
//!
//! The complete pinned `phy_tx_i2c_track` body partitions the signed current
//! temperature into four ranges. A range change performs two low-nibble
//! masked writes and only then commits the retained range. The linked vendor
//! body is 330 bytes at `0x1000_0140`; no vendor table or ABI image is needed
//! by this source-owned replacement.

use crate::analog::i2c::{MaskedI2cWriteBinding, MaskedI2cWriteBindingError, analog_registers};

/// Semantic replacement for the vendor range byte at `phy_param + 0x4d`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyWifiI2cTrackingBand {
    Nominal,
    Cold,
    Elevated,
    Hot,
}

impl PhyWifiI2cTrackingBand {
    /// Exact signed-temperature partition recovered from the complete body.
    pub const fn for_temperature(temperature: i16) -> Self {
        if temperature < -19 {
            Self::Cold
        } else if temperature <= 54 {
            Self::Nominal
        } else if temperature <= 94 {
            Self::Elevated
        } else {
            Self::Hot
        }
    }

    const fn values(self) -> (u8, u8) {
        match self {
            Self::Nominal => (10, 13),
            Self::Cold => (10, 15),
            Self::Elevated => (8, 15),
            Self::Hot => (6, 15),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyWifiI2cTrackingParameters {
    pub current_temperature: i16,
    pub previous_band: PhyWifiI2cTrackingBand,
}

impl PhyWifiI2cTrackingParameters {
    pub const fn target_band(self) -> PhyWifiI2cTrackingBand {
        PhyWifiI2cTrackingBand::for_temperature(self.current_temperature)
    }

    /// A changed band requires both writes before committing the new band.
    pub fn update_required(self) -> bool {
        self.previous_band != self.target_band()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhyWifiI2cTrackingOutcome {
    pub band: PhyWifiI2cTrackingBand,
    pub changed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyWifiI2cTrackingAction {
    MaskedWrite(oer_radio_analog::Action<crate::analog::i2c::PhyI2cAddress, ()>),
    Complete(PhyWifiI2cTrackingOutcome),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyWifiI2cTrackingCompletion {
    MaskedWrite(oer_radio_analog::Completion<crate::analog::i2c::PhyI2cAddress>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhyWifiI2cTrackingTransitionError {
    WrongCompletion,
    AlreadyComplete,
}

/// Unique finite owner of both masked writes for one selected range.
#[must_use = "the Wi-Fi tracking transition owns an in-flight PHY-I2C update"]
#[derive(Debug, Eq, PartialEq)]
pub struct PhyWifiI2cTrackingTransition {
    target_band: PhyWifiI2cTrackingBand,
    changed: bool,
    write_index: u8,
    write: Option<oer_radio_analog::FieldWriteTransition<crate::analog::i2c::PhyI2cAddress>>,
}

impl PhyWifiI2cTrackingTransition {
    pub fn new(parameters: PhyWifiI2cTrackingParameters) -> Self {
        let target_band = parameters.target_band();
        let changed = parameters.update_required();
        Self {
            target_band,
            changed,
            write_index: 0,
            write: changed.then(|| Self::write(target_band, 0)),
        }
    }

    fn write(
        band: PhyWifiI2cTrackingBand,
        index: u8,
    ) -> oer_radio_analog::FieldWriteTransition<crate::analog::i2c::PhyI2cAddress> {
        let (first, second) = band.values();
        let (field, value) = if index == 0 {
            (analog_registers::WIFI_TX_TEMPERATURE_TRACKING_0, first)
        } else {
            (analog_registers::WIFI_TX_TEMPERATURE_TRACKING_1, second)
        };
        crate::analog::i2c::field_write(field, value)
    }

    pub const fn action(&self) -> PhyWifiI2cTrackingAction {
        match self.write {
            Some(write) => PhyWifiI2cTrackingAction::MaskedWrite(write.action()),
            None => PhyWifiI2cTrackingAction::Complete(PhyWifiI2cTrackingOutcome {
                band: self.target_band,
                changed: self.changed,
            }),
        }
    }

    pub fn advance(
        &mut self,
        completion: PhyWifiI2cTrackingCompletion,
    ) -> Result<(), PhyWifiI2cTrackingTransitionError> {
        let Some(mut write) = self.write else {
            return Err(PhyWifiI2cTrackingTransitionError::AlreadyComplete);
        };
        let PhyWifiI2cTrackingCompletion::MaskedWrite(completion) = completion;
        write.advance(completion).map_err(|error| match error {
            oer_radio_analog::TransitionError::WrongCompletion => {
                PhyWifiI2cTrackingTransitionError::WrongCompletion
            }
            oer_radio_analog::TransitionError::AlreadyComplete => {
                PhyWifiI2cTrackingTransitionError::AlreadyComplete
            }
        })?;

        if write.action() == oer_radio_analog::Action::Complete(()) {
            self.write_index += 1;
            self.write = (self.write_index < 2).then(|| Self::write(self.target_band, 1));
        } else {
            self.write = Some(write);
        }
        Ok(())
    }

    /// Lower one explicit masked read/write edge to the existing target
    /// PHY-I2C transaction. Terminal outcomes cannot be lowered.
    pub fn lower_external(&self) -> Result<MaskedI2cWriteBinding, MaskedI2cWriteBindingError> {
        match self.action() {
            PhyWifiI2cTrackingAction::MaskedWrite(action) => MaskedI2cWriteBinding::new(action),
            PhyWifiI2cTrackingAction::Complete(_) => {
                Err(MaskedI2cWriteBindingError::UnsupportedAction)
            }
        }
    }
}

#[cfg(test)]
mod tests;
