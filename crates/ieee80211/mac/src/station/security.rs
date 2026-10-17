//! Station association admission and the selected RSN elements.

use super::*;

use crate::security::{
    AssociationAkm, AssociationSecurity, GroupManagementCipher, Pmkid, RsnAssociation, SaePwe,
    SppCapability, SppError,
};

use crate::security::rsn::{
    RSN_AKM_PSK, RSN_AKM_PSK_SHA256, RSN_AKM_SAE, RSN_CAPABILITY_MFPC, RSN_CAPABILITY_MFPR,
    RSN_CIPHER_BIP_CMAC_128, RSN_CIPHER_CCMP, RsnElement, RsnSyntaxError, ieee_suite,
};

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

/// The association request's RSN element and RSNXE, and the security they
/// negotiate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SelectedRsn {
    length: u8,
    bytes: [u8; SELECTED_SECURITY_IES_CAPACITY],
    security: AssociationSecurity,
}

impl SelectedRsn {
    const OPEN: Self = Self {
        length: 0,
        bytes: [0; SELECTED_SECURITY_IES_CAPACITY],
        security: AssociationSecurity::Open,
    };

    /// The security elements of the association request.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.length)]
    }

    /// The security the association negotiates.
    pub const fn security(&self) -> AssociationSecurity {
        self.security
    }

    /// The same selection naming `pmkid`: an SAE association that resumes a
    /// cached PMKSA lists its PMKID after the RSN capabilities, as the
    /// vendor's `wpa_gen_wpa_ie_rsn` does for the current PMKSA. `None` for
    /// a selection that is not SAE: only SAE associations are cached.
    pub fn with_pmkid(self, pmkid: Pmkid) -> Option<Self> {
        let AssociationSecurity::Rsn(mut association) = self.security else {
            return None;
        };
        if !matches!(association.akm, AssociationAkm::Sae(_)) {
            return None;
        }
        association.pmkid = Some(pmkid);
        let mut resumed = self;
        resumed.security = AssociationSecurity::Rsn(association);
        let rsnxe = &self.bytes[SELECTED_RSN_IE_LEN..usize::from(self.length)];
        resumed.bytes[1] += PMKID_LIST_LEN as u8;
        resumed.bytes[SELECTED_RSN_IE_LEN..SELECTED_RSN_IE_LEN + 2]
            .copy_from_slice(&1_u16.to_le_bytes());
        resumed.bytes[SELECTED_RSN_IE_LEN + 2..SELECTED_RSN_IE_LEN + PMKID_LIST_LEN]
            .copy_from_slice(&pmkid.0);
        let rsnxe_start = SELECTED_RSN_IE_LEN + PMKID_LIST_LEN;
        resumed.bytes[rsnxe_start..rsnxe_start + rsnxe.len()].copy_from_slice(rsnxe);
        resumed.length = (rsnxe_start + rsnxe.len()) as u8;
        Some(resumed)
    }
}

/// RSN Extension element identifier.
const RSNXE_ELEMENT_ID: u8 = 244;
/// RSNXE capability bit of SAE hash to element.
const RSNXE_SAE_H2E: u8 = 1 << 5;

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
    if !access_point.matches_security(LinkProtection::Ccmp) {
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
    let management_protection =
        policy.management_protection().capable() && capabilities & RSN_CAPABILITY_MFPC != 0;
    if !management_protection
        && (rsn.group_management_cipher().is_some() || capabilities & RSN_CAPABILITY_MFPR != 0)
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
        _ if sae => AssociationAkm::Sae(
            if access_point
                .rsnxe_bytes()
                .get(2)
                .is_some_and(|capabilities| capabilities & RSNXE_SAE_H2E != 0)
            {
                SaePwe::HashToElement
            } else {
                SaePwe::HuntingAndPecking
            },
        ),
        StaSecurityPolicy::Wpa3Personal | StaSecurityPolicy::Open => {
            return Err(StaSecurityError::UnsupportedAkm);
        }
        StaSecurityPolicy::Wpa2Personal
            if rsn.akm_suites().contains(ieee_suite(RSN_AKM_PSK_SHA256)) =>
        {
            AssociationAkm::PskSha256
        }
        StaSecurityPolicy::Wpa2Personal if rsn.akm_suites().contains(ieee_suite(RSN_AKM_PSK)) => {
            AssociationAkm::Psk
        }
        StaSecurityPolicy::Wpa2Personal => return Err(StaSecurityError::UnsupportedAkm),
    };
    let akm_type = akm.akm().suite_selector()[3];

    // Advertise support and retain the negotiated decision for key installation.
    // SOURCE[HIL_VENDOR_HE20_NDPA_CBF_2026_07_24]: frame 7624 carries RSN
    // Capabilities 0x0400 in the successful HE association request.
    let mut station_capabilities = SppCapability::Capable.rsn_bits();
    if management_protection {
        station_capabilities |= RSN_CAPABILITY_MFPC;
    }
    if matches!(akm, AssociationAkm::Sae(_)) {
        station_capabilities |= RSN_CAPABILITY_MFPR;
    }
    let mut selected = SelectedRsn::OPEN;
    selected.security = AssociationSecurity::Rsn(RsnAssociation {
        akm,
        spp,
        management: management_protection.then_some(GroupManagementCipher::BipCmac128),
        pmkid: None,
    });
    selected.bytes[..SELECTED_RSN_IE_LEN].copy_from_slice(&[
        48,
        20,
        1,
        0,
        0x00,
        0x0f,
        0xac,
        RSN_CIPHER_CCMP,
        1,
        0,
        0x00,
        0x0f,
        0xac,
        RSN_CIPHER_CCMP,
        1,
        0,
        0x00,
        0x0f,
        0xac,
        akm_type,
        station_capabilities as u8,
        (station_capabilities >> 8) as u8,
    ]);
    selected.length = SELECTED_RSN_IE_LEN as u8;
    if matches!(akm, AssociationAkm::Sae(_)) {
        selected.bytes[SELECTED_RSN_IE_LEN..SELECTED_RSN_IE_LEN + 3].copy_from_slice(&[
            RSNXE_ELEMENT_ID,
            1,
            RSNXE_SAE_H2E,
        ]);
        selected.length = SELECTED_RSN_IE_LEN as u8 + 3;
    }
    Ok(selected)
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
        StaSecurityPolicy::Open if access_point.matches_security(LinkProtection::Open) => {
            Ok(SelectedRsn::OPEN)
        }
        StaSecurityPolicy::Open => Err(StaSecurityError::SecurityModeMismatch),
        StaSecurityPolicy::Wpa2Personal | StaSecurityPolicy::Wpa3Personal => {
            select_personal_rsn(access_point, policy)
        }
    }
}
