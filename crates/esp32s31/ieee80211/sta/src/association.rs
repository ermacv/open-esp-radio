//! ESP32-S31 STA Association policy derived from scan and PHY calibration.

use oer_esp32s31_ieee80211::tx::WifiTxPowerProfile;

use {
    oer_ieee80211_mac::scan::ScanRecord, oer_ieee80211_mac::station::HeUlMuPowerCapability,
    oer_ieee80211_mac::station::HeUlMuPowerCapabilityError,
    oer_ieee80211_mac::station::StaPowerCapability,
    oer_ieee80211_mac::station::StaPowerCapabilityError,
    oer_ieee80211_mac::station::association::PhyMode,
    oer_ieee80211_mac::station::association::Preference,
};

use oer_ieee80211_sta::request::StationListenInterval;

/// Recovered minimum power advertised by the ESP32-S31 HE STA.
///
/// Complete vendor `hal_he_init` installs this value through
/// `hal_set_tx_min_pwr`; the maximum remains derived from calibrated rate 16.
pub const ESP32S31_STA_MINIMUM_TX_POWER_DBM: i8 = -11;

/// Listen interval used by the qualified ESP32-S31 infrastructure STA path.
pub const ESP32S31_STA_LISTEN_INTERVAL: u16 = StationListenInterval::DEFAULT.get();

/// Complete association inputs derived from scan policy and calibration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaAssociationProfile {
    pub phy: PhyMode,
    pub power_capability: Option<StaPowerCapability>,
    pub he_ul_mu_power: Option<HeUlMuPowerCapability>,
    pub rate_16_through_25: Option<[i8; 10]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaAssociationProfileError {
    PowerCapability(StaPowerCapabilityError),
    HeUlMuPower(HeUlMuPowerCapabilityError),
    /// The access point has no HT Capabilities. The S31 connected station
    /// does not yet serve a legacy association, so the attempt fails here
    /// rather than after the join.
    LegacyUnsupported,
}

/// Select an Association PHY and derive every HE power field from calibration.
pub fn esp32s31_sta_association_profile<P: WifiTxPowerProfile>(
    access_point: &ScanRecord,
    preference: Preference,
    power: &P,
) -> Result<StaAssociationProfile, StaAssociationProfileError> {
    let phy = crate::profile::select_association(access_point, preference).phy;
    if phy == PhyMode::Legacy {
        return Err(StaAssociationProfileError::LegacyUnsupported);
    }
    if phy != PhyMode::He20 {
        return Ok(StaAssociationProfile {
            phy,
            power_capability: None,
            he_ul_mu_power: None,
            rate_16_through_25: None,
        });
    }

    let rate_power = core::array::from_fn(|offset| power.power_pair(16 + offset as u8).primary);
    let power_capability =
        StaPowerCapability::new(ESP32S31_STA_MINIMUM_TX_POWER_DBM, rate_power[0])
            .map_err(StaAssociationProfileError::PowerCapability)?;
    let he_ul_mu_power = HeUlMuPowerCapability::from_rate_power_indices(rate_power)
        .map_err(StaAssociationProfileError::HeUlMuPower)?;
    Ok(StaAssociationProfile {
        phy,
        power_capability: Some(power_capability),
        he_ul_mu_power: Some(he_ul_mu_power),
        rate_16_through_25: Some(rate_power),
    })
}

#[cfg(test)]
mod tests;
