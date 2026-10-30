//! The station's security policy: which advertised BSS security one request
//! admits and the RSN selection its association request carries.

use oer_ieee80211_wire::{
    scan::ScanRecord,
    security::{
        AssociationAkm, LinkProtection, ManagementProtection, SaePwe, SppCapability, SppError,
        rsn::{
            RSN_AKM_PSK, RSN_AKM_PSK_SHA256, RSN_AKM_SAE, RSN_CIPHER_BIP_CMAC_128, RSN_CIPHER_CCMP,
            RsnCapabilities, RsnElement, RsnSyntaxError, ieee_suite, rsnx_capabilities,
        },
    },
    station::SelectedRsn,
};

/// What one station request admits.
///
/// A personal request never downgrades: WPA2-Personal admits PSK or, as the
/// vendor's WPA3-enabled station does, upgrades to SAE whenever the access
/// point offers it; WPA3-Personal admits SAE only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaSecurityPolicy {
    Open,
    Wpa2Personal,
    /// PSK credentials only: never select SAE, including transition BSSes.
    Wpa2Psk,
    Wpa3Personal,
}

impl StaSecurityPolicy {
    /// The data protection of the link.
    pub const fn link_protection(self) -> LinkProtection {
        match self {
            Self::Open => LinkProtection::Open,
            Self::Wpa2Personal | Self::Wpa2Psk | Self::Wpa3Personal => LinkProtection::Ccmp,
        }
    }

    /// The station's management frame protection. A personal station is
    /// always capable, as the vendor supplicant's configuration sets MFPC,
    /// and WPA3-Personal requires it.
    pub const fn management_protection(self) -> ManagementProtection {
        match self {
            Self::Open => ManagementProtection::Disabled,
            Self::Wpa2Personal | Self::Wpa2Psk => ManagementProtection::Capable,
            Self::Wpa3Personal => ManagementProtection::Required,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaSecurityError {
    SecurityModeMismatch,
    MissingRsn,
    MalformedRsn,
    UnsupportedVersion,
    UnsupportedGroupCipher,
    UnsupportedPairwiseCipher,
    UnsupportedAkm,
    /// The access point protects management frames with a group management
    /// cipher other than BIP-CMAC-128.
    UnsupportedGroupManagementCipher,
    /// The peer's SPP requirement or capability encoding is incompatible.
    Spp(SppError),
}

/// Whether `record` matches one exact requested security mode.
///
/// Mixed WPA/WPA2 advertisements are intentionally rejected. Open means
/// that the Privacy capability is clear and neither RSN nor legacy WPA
/// was observed; WPA2 means a Privacy-marked, RSN-only BSS. Full WPA2
/// suite validation remains at association encoding.
pub fn matches_security(record: &ScanRecord, mode: LinkProtection) -> bool {
    match mode {
        LinkProtection::Open => {
            !record.information_elements_truncated
                && !record.privacy
                && !record.rsn
                && record.rsn_ie_count == 0
                && !record.legacy_wpa
                && record.rsn_ie_bytes().is_empty()
        }
        LinkProtection::Ccmp => {
            !record.information_elements_truncated
                && record.privacy
                && record.rsn
                && record.rsn_ie_count == 1
                && !record.legacy_wpa
                && !record.rsn_ie_bytes().is_empty()
        }
    }
}

/// Select the RSN element (and RSNXE) of a personal association.
///
/// As the vendor's `ieee80211_parse_rsn` does, the station uses SAE whenever
/// the access point offers it with management frame protection, then prefers
/// PSK-SHA256 over PSK; [`StaSecurityPolicy::Wpa3Personal`] admits SAE only.
/// The station protects management frames whenever the access point is
/// capable of it: the vendor supplicant then sets MFPC in its own element
/// (the configuration's `capable` is always set), and MFPR as well under SAE.
/// Under SAE it adds an RSNXE announcing hash to element, as the vendor's
/// default `sae_pwe_h2e` (both methods) makes `wpa_gen_rsnxe` do. Only the
/// default group management cipher, BIP-CMAC-128, is implemented, so the
/// element omits the Group Management Cipher Suite as the vendor supplicant
/// does for the default.
///
/// SOURCE(esp32s31): complete pinned `libnet80211.a[ieee80211_input.o]::
/// ieee80211_parse_rsn` (AKM switch table `CSWTCH.76`, PMF flag
/// `g_ic+0x210`); ESP-IDF `wpa_gen_wpa_ie_rsn` and `wpa_gen_rsnxe`.
fn select_personal_rsn(
    access_point: &ScanRecord,
    policy: StaSecurityPolicy,
) -> Result<SelectedRsn, StaSecurityError> {
    if !matches_security(access_point, LinkProtection::Ccmp) {
        return Err(StaSecurityError::SecurityModeMismatch);
    }
    let rsn = access_point.rsn_ie_bytes();
    if rsn.is_empty() {
        return Err(StaSecurityError::MissingRsn);
    }
    let rsn = RsnElement::parse(rsn).map_err(|error| match error {
        RsnSyntaxError::Malformed => StaSecurityError::MalformedRsn,
        RsnSyntaxError::UnsupportedVersion => StaSecurityError::UnsupportedVersion,
    })?;

    if rsn.group_data_cipher() != ieee_suite(RSN_CIPHER_CCMP) {
        return Err(StaSecurityError::UnsupportedGroupCipher);
    }
    if !rsn.pairwise_ciphers().contains(ieee_suite(RSN_CIPHER_CCMP)) {
        return Err(StaSecurityError::UnsupportedPairwiseCipher);
    }
    // An advertised PMKID list is ignored.
    let capabilities = rsn.capabilities().unwrap_or(0);
    let spp = SppCapability::Capable
        .negotiate(SppCapability::from_rsn(capabilities).map_err(StaSecurityError::Spp)?)
        .map_err(StaSecurityError::Spp)?;
    let management_protection = policy.management_protection().capable()
        && RsnCapabilities::from_bytes(capabilities.to_le_bytes()).mfpc();
    if !management_protection
        && (rsn.group_management_cipher().is_some()
            || RsnCapabilities::from_bytes(capabilities.to_le_bytes()).mfpr())
    {
        return Err(StaSecurityError::MalformedRsn);
    }
    if management_protection
        && rsn
            .group_management_cipher()
            .is_some_and(|cipher| cipher != ieee_suite(RSN_CIPHER_BIP_CMAC_128))
    {
        return Err(StaSecurityError::UnsupportedGroupManagementCipher);
    }
    let sae = management_protection && rsn.akm_suites().contains(ieee_suite(RSN_AKM_SAE));
    let akm = match policy {
        _ if sae && policy != StaSecurityPolicy::Wpa2Psk => AssociationAkm::Sae(
            if rsnx_capabilities(access_point.rsnxe_bytes())
                .is_some_and(|capabilities| capabilities.sae_h2e())
            {
                SaePwe::HashToElement
            } else {
                SaePwe::HuntingAndPecking
            },
        ),
        StaSecurityPolicy::Wpa3Personal | StaSecurityPolicy::Open => {
            return Err(StaSecurityError::UnsupportedAkm);
        }
        StaSecurityPolicy::Wpa2Personal | StaSecurityPolicy::Wpa2Psk
            if rsn.akm_suites().contains(ieee_suite(RSN_AKM_PSK_SHA256)) =>
        {
            AssociationAkm::PskSha256
        }
        StaSecurityPolicy::Wpa2Personal | StaSecurityPolicy::Wpa2Psk
            if rsn.akm_suites().contains(ieee_suite(RSN_AKM_PSK)) =>
        {
            AssociationAkm::Psk
        }
        StaSecurityPolicy::Wpa2Personal | StaSecurityPolicy::Wpa2Psk => {
            return Err(StaSecurityError::UnsupportedAkm);
        }
    };
    // Advertise support and retain the negotiated decision for key installation.
    // SOURCE[HIL_VENDOR_HE20_NDPA_CBF_2026_07_24]: frame 7624 carries RSN
    // Capabilities 0x0400 in the successful HE association request.
    Ok(personal_rsn(akm, spp, management_protection))
}

/// The RSN element of a personal association negotiating `akm`, and under
/// SAE an RSNXE announcing hash to element.
///
/// Its RSN Capabilities advertise SPP A-MSDU capability, MFPC when
/// `management_protection` holds and MFPR under SAE; protected management
/// frames take BIP-CMAC-128.
pub fn personal_rsn(
    akm: AssociationAkm,
    spp: oer_ieee80211_wire::security::Spp,
    management_protection: bool,
) -> SelectedRsn {
    use oer_ieee80211_wire::security::rsn::{
        RSN_CAPABILITY_MFPC, RSN_CAPABILITY_MFPR, RsnExtendedCapabilities,
    };
    use oer_ieee80211_wire::security::{GroupManagementCipher, RsnAssociation, SppCapability};
    let sae = matches!(akm, AssociationAkm::Sae(_));
    let mut capabilities = SppCapability::Capable.rsn_bits();
    if management_protection {
        capabilities |= RSN_CAPABILITY_MFPC;
    }
    if sae {
        capabilities |= RSN_CAPABILITY_MFPR;
    }
    SelectedRsn::rsn(
        RsnAssociation {
            akm,
            spp,
            management: management_protection.then_some(GroupManagementCipher::BipCmac128),
            pmkid: None,
        },
        capabilities,
        sae.then(|| RsnExtendedCapabilities::from_bytes([0]).with_sae_h2e(true)),
    )
}

/// Select the association security elements for one station request.
///
/// Open never accepts a Privacy/RSN/WPA advertisement. A personal request
/// never accepts an open or mixed WPA/WPA2 advertisement, and then validates
/// the complete retained RSN suites before returning source-owned elements.
pub fn select_association_rsn(
    access_point: &ScanRecord,
    policy: StaSecurityPolicy,
) -> Result<SelectedRsn, StaSecurityError> {
    match policy {
        StaSecurityPolicy::Open if matches_security(access_point, LinkProtection::Open) => {
            Ok(SelectedRsn::OPEN)
        }
        StaSecurityPolicy::Open => Err(StaSecurityError::SecurityModeMismatch),
        StaSecurityPolicy::Wpa2Personal
        | StaSecurityPolicy::Wpa2Psk
        | StaSecurityPolicy::Wpa3Personal => select_personal_rsn(access_point, policy),
    }
}

#[cfg(test)]
mod tests;
