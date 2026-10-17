//! IEEE 802.15.4 transmit-power resolution.
//!
//! Ported from ESP-IDF `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
//! `components/ieee802154/driver/esp_ieee802154_pib.c` (Copyright Espressif
//! Systems (Shanghai) Co Ltd, Apache-2.0) and modified by Vladislav
//! Ermachenkov; see `NOTICE`.
//!
//! The public vendor PIB supplies the conversion control flow but obtains the
//! actual ordered dBm levels from the BTBB provider `bt_bb_get_tx_pwr_table`.
//! This module ports that control flow, validates a level set and produces an
//! opaque field code without touching MMIO. Each chip's HAL supplies the level
//! set recovered from its vendor library.

use core::marker::PhantomData;

use crate::channel::Ieee802154Channel;

const MAX_PROVIDER_LEVEL_COUNT: usize = u8::MAX as usize;

/// Why an externally supplied transmit-power level set cannot be resolved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154TxPowerLevelsError {
    /// The provider supplied no levels.
    Empty,
    /// The provider supplied more levels than its public eight-bit length can
    /// represent.
    TooLong {
        /// Complete rejected level count.
        length: usize,
        /// Largest accepted level count.
        maximum: usize,
    },
    /// A later level was lower than its predecessor.
    Descending {
        /// Index of the lower, rejected level.
        index: usize,
        /// Level immediately before the rejected value.
        previous_dbm: i8,
        /// Rejected lower level.
        current_dbm: i8,
    },
}

/// One validated external sequence of supported transmit-power levels.
///
/// The sequence must contain between one and 255 entries and be
/// non-decreasing. Equal adjacent values are retained because the public PIB
/// branch order makes their selection deterministic: a request at or below
/// the first level selects index zero, a request at or above the final level
/// selects the final index, and the interior floor scan selects the greatest
/// matching index. Validation proves only that the open conversion can execute
/// without truncation; it does not prove where the values came from or that RF
/// calibration is ready.
/// In particular, this borrow tracks storage lifetime, not a provider or
/// calibration epoch; arbitrary external levels grant no hardware authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154TxPowerLevels<'levels> {
    levels_dbm: &'levels [i8],
}

impl<'levels> Ieee802154TxPowerLevels<'levels> {
    /// Validate one externally supplied level sequence without copying it.
    pub const fn new(levels_dbm: &'levels [i8]) -> Result<Self, Ieee802154TxPowerLevelsError> {
        if levels_dbm.is_empty() {
            return Err(Ieee802154TxPowerLevelsError::Empty);
        }
        if levels_dbm.len() > MAX_PROVIDER_LEVEL_COUNT {
            return Err(Ieee802154TxPowerLevelsError::TooLong {
                length: levels_dbm.len(),
                maximum: MAX_PROVIDER_LEVEL_COUNT,
            });
        }

        let mut index = 1;
        while index < levels_dbm.len() {
            if levels_dbm[index] < levels_dbm[index - 1] {
                return Err(Ieee802154TxPowerLevelsError::Descending {
                    index,
                    previous_dbm: levels_dbm[index - 1],
                    current_dbm: levels_dbm[index],
                });
            }
            index += 1;
        }

        Ok(Self { levels_dbm })
    }

    /// Return the validated external level count.
    pub const fn len(self) -> usize {
        self.levels_dbm.len()
    }

    /// Return the final, highest provider level (`IEEE802154_TXPOWER_VALUE_MAX`).
    pub const fn highest_dbm(self) -> i8 {
        self.levels_dbm[self.levels_dbm.len() - 1]
    }

    /// A validated provider level set is never empty.
    pub const fn is_empty(self) -> bool {
        false
    }

    /// Resolve one request with the public vendor clamp-and-floor scan.
    ///
    /// A request at or below the first provider value selects index zero, and
    /// one at or above the final value selects the final index. Otherwise the
    /// selected field code is the greatest provider index whose dBm value does
    /// not exceed `requested_dbm`. This branch precedence is observable when
    /// boundary values are duplicated. The result retains the channel so
    /// later integration cannot silently separate channel and power selection.
    pub const fn resolve(
        self,
        channel: Ieee802154Channel,
        requested_dbm: i8,
    ) -> Ieee802154ResolvedTxPower<'levels> {
        let last_index = self.levels_dbm.len() - 1;
        let selected_index = if requested_dbm <= self.levels_dbm[0] {
            0
        } else if requested_dbm >= self.levels_dbm[last_index] {
            last_index
        } else {
            let mut index = last_index;
            while index != 0 && self.levels_dbm[index] > requested_dbm {
                index -= 1;
            }
            index
        };

        Ieee802154ResolvedTxPower {
            channel,
            requested_dbm,
            effective_dbm: self.levels_dbm[selected_index],
            field_code: Ieee802154TxPowerCode(selected_index as u8),
            _levels: PhantomData,
        }
    }
}

impl Ieee802154TxPowerLevels<'static> {
    /// The levels as the port publishes them.
    pub const fn portable(self) -> oer_ieee802154::TransmitPowerLevels {
        // Both validations accept a non-empty, non-decreasing sequence.
        match oer_ieee802154::TransmitPowerLevels::new(self.levels_dbm) {
            Ok(levels) => levels,
            Err(_) => unreachable!(),
        }
    }
}

/// Internal field code selected from one validated provider level set.
///
/// The type is public so a chip HAL can write the code; its constructor is
/// not. Keeping it non-`Copy`
/// prevents the provider-relative code from being detached from the
/// channel-bound resolution that owns it; it does not turn arbitrary levels
/// into a provider, calibration epoch, or MMIO capability.
#[derive(Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154TxPowerCode(u8);

impl Ieee802154TxPowerCode {
    /// The selected provider index.
    pub const fn value(&self) -> u8 {
        self.0
    }
}

/// One channel-bound result of the source-only transmit-power scan.
///
/// The lifetime prevents the result from outliving the external level storage
/// from which it was derived. It is not a fabricated provider or calibration
/// epoch; operational code must add that ownership boundary before writing
/// the retained field code.
///
/// ```compile_fail
/// use oer_espressif_ieee802154_engine::{
///     channel::Ieee802154Channel, tx_power::Ieee802154ResolvedTxPower,
/// };
///
/// let channel = Ieee802154Channel::new(20).unwrap();
/// let forged = Ieee802154ResolvedTxPower::from_raw_parts(channel, 0, 0, 7);
/// ```
///
/// ```compile_fail
/// use oer_espressif_ieee802154_engine::{
///     channel::Ieee802154Channel, tx_power::Ieee802154TxPowerLevels,
/// };
///
/// let escaped = {
///     let provider_levels = [-9, 4];
///     let levels = Ieee802154TxPowerLevels::new(&provider_levels).unwrap();
///     let channel = Ieee802154Channel::new(20).unwrap();
///     levels.resolve(channel, 0)
/// };
/// let _ = escaped.selected_provider_index();
/// ```
#[derive(Debug, Eq, PartialEq)]
pub struct Ieee802154ResolvedTxPower<'levels> {
    channel: Ieee802154Channel,
    requested_dbm: i8,
    effective_dbm: i8,
    field_code: Ieee802154TxPowerCode,
    _levels: PhantomData<&'levels [i8]>,
}

impl Ieee802154ResolvedTxPower<'_> {
    /// Return the channel retained by this resolution.
    pub const fn channel(&self) -> Ieee802154Channel {
        self.channel
    }

    /// Return the caller's request before provider-relative clamping.
    pub const fn requested_dbm(&self) -> i8 {
        self.requested_dbm
    }

    /// Return the provider level selected by the public scan.
    pub const fn effective_dbm(&self) -> i8 {
        self.effective_dbm
    }

    /// Return the selected external-provider index for diagnostics.
    ///
    /// This numeric observation grants no register-write authority. A future
    /// HAL writer must consume the complete channel-bound resolution rather
    /// than accept this value or the internal field code separately.
    pub const fn selected_provider_index(&self) -> u8 {
        self.field_code().value()
    }

    /// The provider-relative field code the chip HAL writes.
    pub const fn field_code(&self) -> &Ieee802154TxPowerCode {
        &self.field_code
    }
}

#[cfg(test)]
mod tests;
