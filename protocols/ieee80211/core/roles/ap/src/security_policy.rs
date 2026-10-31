//! The security an access point offers: its link and management protection
//! and the RSN elements it advertises.

#[cfg(doc)]
use oer_ieee80211_wire::security::spp::SppScope;
use oer_ieee80211_wire::security::{
    BssSecurityElements, LinkProtection, ManagementProtection, SppCapability,
    rsn::{
        RSN_AKM_PSK, RSN_AKM_SAE, RSN_CAPABILITY_MFPC, RSN_CAPABILITY_MFPR,
        RSN_CIPHER_BIP_CMAC_128, RSN_CIPHER_CCMP, RSN_GROUP_MANAGEMENT_ELEMENT_LEN,
        RSN_SINGLE_SUITE_ELEMENT_LEN, RSNXE_ONE_OCTET_LEN, RsnExtendedCapabilities,
        rsn_group_management_element, rsn_single_suite_element, rsnx_element,
    },
};

/// The security an access point offers.
///
/// WPA2-Personal keeps the reviewed PSK profile without management frame
/// protection. WPA3-Personal follows the vendor's softAP: SAE only, with
/// management frame protection required, BIP-CMAC-128 as the group
/// management cipher of a CCMP BSS, and both SAE password element methods,
/// which it advertises through the H2E bit of its RSNXE. Each variant owns
/// the exact RSN element the beacon, the probe response and EAPOL Message 3
/// carry.
///
/// SOURCE: ESP-IDF `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
/// `components/wpa_supplicant/esp_supplicant/src/esp_hostap.c`
/// (`hostap_init`: `WIFI_AUTH_WPA3_PSK` selects `WPA_KEY_MGMT_SAE`, PMF
/// required, `WPA_CIPHER_AES_128_CMAC` for CCMP) and
/// `src/ap/wpa_auth_ie.c` (`wpa_own_rsn_capab`, `rsne_write_data`,
/// `wpa_write_rsnxe`); `docs/en/api-guides/wifi-security.rst` ("For WPA3
/// SoftAP, PMF Required is mandatory").
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApSecurityPolicy {
    Open,
    Wpa2Personal,
    Wpa3Personal,
}

/// RSN element of the WPA2-Personal access point: CCMP group and pairwise
/// ciphers, PSK, no capabilities.
pub const AP_WPA2_PERSONAL_RSN_ELEMENT: [u8; RSN_SINGLE_SUITE_ELEMENT_LEN] =
    rsn_single_suite_element(RSN_CIPHER_CCMP, RSN_CIPHER_CCMP, RSN_AKM_PSK, 0);

/// RSN element of the WPA3-Personal access point: CCMP group and pairwise
/// ciphers, SAE, MFPC and MFPR, no PMKID and BIP-CMAC-128.
pub const AP_WPA3_PERSONAL_RSN_ELEMENT: [u8; RSN_GROUP_MANAGEMENT_ELEMENT_LEN] =
    rsn_group_management_element(
        RSN_CIPHER_CCMP,
        RSN_CIPHER_CCMP,
        RSN_AKM_SAE,
        RSN_CAPABILITY_MFPC | RSN_CAPABILITY_MFPR,
        RSN_CIPHER_BIP_CMAC_128,
    );

/// RSNXE of an access point offering SAE hash-to-element.
pub const AP_SAE_H2E_RSNX_ELEMENT: [u8; RSNXE_ONE_OCTET_LEN] =
    rsnx_element(RsnExtendedCapabilities::from_bytes([0]).with_sae_h2e(true));

impl ApSecurityPolicy {
    /// The current AP RSN profiles do not advertise SPP. A required peer is
    /// refused; a merely capable peer negotiates ordinary CCMP.
    ///
    /// The backend's [`SppScope`] determines whether a future AP profile
    /// can admit different SPP modes on different associations.
    pub const fn spp_capability(self) -> SppCapability {
        SppCapability::NotCapable
    }

    /// The data protection of the BSS.
    pub const fn link_protection(self) -> LinkProtection {
        match self {
            Self::Open => LinkProtection::Open,
            Self::Wpa2Personal | Self::Wpa3Personal => LinkProtection::Ccmp,
        }
    }

    /// The access point's management frame protection: WPA3-Personal
    /// requires it, WPA2-Personal keeps the reviewed profile without it.
    pub const fn management_protection(self) -> ManagementProtection {
        match self {
            Self::Open | Self::Wpa2Personal => ManagementProtection::Disabled,
            Self::Wpa3Personal => ManagementProtection::Required,
        }
    }

    /// The security elements the beacon, the probe response and EAPOL
    /// Message 3 carry; both are empty for an Open BSS.
    pub const fn advertisement(self) -> BssSecurityElements {
        match self {
            Self::Open => BssSecurityElements::OPEN,
            Self::Wpa2Personal => BssSecurityElements {
                rsne: &AP_WPA2_PERSONAL_RSN_ELEMENT,
                rsnxe: &[],
            },
            Self::Wpa3Personal => BssSecurityElements {
                rsne: &AP_WPA3_PERSONAL_RSN_ELEMENT,
                rsnxe: &AP_SAE_H2E_RSNX_ELEMENT,
            },
        }
    }
}

#[cfg(test)]
mod tests;
