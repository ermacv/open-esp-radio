//! The power elements an HE Association Request carries on one channel:
//! the HE Power Capability and the HE UL-MU power capability, from the PHY's
//! production target powers under the channel's transmit ceiling.

use oer_chips_esp32s31_hal::types::MAC_MINIMUM_TX_POWER_DBM;
use oer_chips_esp32s31_radio_phy::PhyConfig;
use oer_protocols_ieee80211_contracts_port::RegulatedChannel;
use oer_protocols_ieee80211_wire::{
    channel::{ChannelWidth, Ghz2_4Channel},
    station::{
        HeUlMuPowerCapability, HeUlMuPowerCapabilityError, StaPowerCapability,
        StaPowerCapabilityError,
    },
};
use oer_protocols_radio_regulatory::{PowerLimits, RegulatoryDomain};

/// The two power elements of an HE Association Request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HePowerElements {
    pub power_capability: StaPowerCapability,
    pub ul_mu_power: HeUlMuPowerCapability,
}

/// Why a channel gives no HE power elements.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HePowerError {
    /// The channel is no 2.4 GHz channel the domain permits.
    Channel,
    /// The HE-MCS 0 power lies below the MAC's minimum.
    Capability(StaPowerCapabilityError),
    /// A rate of HE-MCS 1 to 9 has more power than HE-MCS 0.
    UlMu(HeUlMuPowerCapabilityError),
}

/// The power elements of an HE join on the scanned primary channel
/// `primary` of `domain`, under `limits`: each HE-MCS's production target
/// power bounded by the channel's transmit ceiling, the ceiling every
/// transmission of the port takes ([`crate::tx::CappedTxPowerProfile`]),
/// and the MAC's minimum.
///
/// The channel is the scanned 20 MHz primary because HE is offered at
/// 20 MHz only ([`crate::facts::ESP32S31_FORMAT_WIDTHS`]): an HE join is on
/// the channel it was scanned on. Whoever offers HE at 40 MHz takes the
/// ceiling of the join's 40 MHz channel here.
pub fn he_power(
    domain: &RegulatoryDomain,
    primary: u8,
    limits: PowerLimits,
) -> Result<HePowerElements, HePowerError> {
    let channel =
        Ghz2_4Channel::new(primary, ChannelWidth::Mhz20).map_err(|_| HePowerError::Channel)?;
    let ceiling = domain
        .permit(RegulatedChannel::from(channel))
        .map_err(|_| HePowerError::Channel)?
        .tx_ceiling(limits)
        .dbm();
    let targets = PhyConfig::production().tx_target_power_profile();
    elements(MAC_MINIMUM_TX_POWER_DBM, |rate| {
        targets.pair(rate).primary.min(ceiling)
    })
}

/// The power elements from `minimum_dbm` and `rate_power`, the power each
/// MAC rate code of 16 to 25 (HE-MCS 0 to 9) transmits at: the Power
/// Capability (the minimum and the HE-MCS 0 power) and the HE UL-MU power
/// capability (HE-MCS 1 to 9 against 0).
///
/// SOURCE(esp32s31): complete pinned `libnet80211.a[ieee80211_he.o]::
/// ieee80211_add_power_cap` (`hal_get_tx_min_pwr`, `hal_get_tx_pwr(16, 1)`)
/// and `ieee80211_add_ulmu_pwrcap` (`phy_get_max_pwr` of rates 16..=25).
fn elements(
    minimum_dbm: i8,
    rate_power: impl Fn(u8) -> i8,
) -> Result<HePowerElements, HePowerError> {
    // The MAC rate codes of HE-MCS 0 to 9.
    let rates: [i8; 10] = [16, 17, 18, 19, 20, 21, 22, 23, 24, 25].map(rate_power);
    let [mcs0, ..] = rates;
    Ok(HePowerElements {
        power_capability: StaPowerCapability::new(minimum_dbm, mcs0)
            .map_err(HePowerError::Capability)?,
        ul_mu_power: HeUlMuPowerCapability::from_rate_power_indices(rates)
            .map_err(HePowerError::UlMu)?,
    })
}

#[cfg(test)]
mod tests;
