//! Ordinary TX ownership retained across ESP32-S31 station protocol epochs.
//!
//! A station starts with one control TX owner, lends it to the connected
//! runner, and must recover the same descriptor resources before scan or join
//! can run again. This owner retains the construction policy while the
//! descriptor itself is absent, so platform/HIL code never reconstructs a
//! driver object from duplicated constants. Construction of a concrete async
//! TX implementation belongs to its runtime adapter.

use oer_esp32s31_ieee80211::tx::ControlTxConfig;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaTxEpochError {
    OwnerUnavailable,
    OwnerAlreadyPresent,
}

/// One station-wide ordinary descriptor owner and its immutable policy.
pub struct StaTxEpoch<C> {
    control: Option<C>,
    config: ControlTxConfig,
}

impl<C> StaTxEpoch<C> {
    pub const fn from_control(control: C, config: ControlTxConfig) -> Self {
        Self {
            control: Some(control),
            config,
        }
    }

    pub const fn config(&self) -> ControlTxConfig {
        self.config
    }

    pub fn control(&self) -> Result<&C, StaTxEpochError> {
        self.control
            .as_ref()
            .ok_or(StaTxEpochError::OwnerUnavailable)
    }

    pub fn control_mut(&mut self) -> Result<&mut C, StaTxEpochError> {
        self.control
            .as_mut()
            .ok_or(StaTxEpochError::OwnerUnavailable)
    }

    pub fn take_control(&mut self) -> Result<C, StaTxEpochError> {
        self.control.take().ok_or(StaTxEpochError::OwnerUnavailable)
    }

    /// Restore a returned protocol owner without overwriting a live phase.
    pub fn restore_control(&mut self, control: C) -> Result<(), (StaTxEpochError, C)> {
        if self.control.is_some() {
            return Err((StaTxEpochError::OwnerAlreadyPresent, control));
        }
        self.control = Some(control);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
