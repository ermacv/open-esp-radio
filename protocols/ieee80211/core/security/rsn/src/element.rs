//! Association RSN element validation and suite selection.
//!
//! An authenticator validates a station's Association element with it, and a
//! supplicant derives the suite of its own element. Peer queues, callbacks and
//! chip node pointers are deliberately outside this crate.
//!
//! Received bytes reach this module; nothing here panics on them.

use oer_protocols_ieee80211_wire::security::ManagementProtection;
use oer_protocols_ieee80211_wire::security::rsn::{
    RSN_CAPABILITY_MFPC, RSN_CAPABILITY_MFPR, RSN_CIPHER_BIP_CMAC_128, RSN_CIPHER_CCMP,
    RSN_PMKID_LEN, RsnElement, RsnSyntaxError, ieee_suite,
};

use crate::{
    Akm,
    frames::{OwnedRsnIe, RsnFrameError},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnElementError {
    Malformed,
    CapacityExceeded,
    UnsupportedVersion,
    UnsupportedGroupCipher,
    UnsupportedPairwiseCipher,
    UnsupportedAkm,
    UnsupportedGroupManagementCipher,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedRsnElement {
    owned: OwnedRsnIe,
    akm: Akm,
    capabilities: u16,
    pmkid_count: usize,
}

impl ValidatedRsnElement {
    /// The first supported suite of the element's AKM list.
    pub const fn akm(&self) -> Akm {
        self.akm
    }

    pub fn owned(&self) -> &OwnedRsnIe {
        &self.owned
    }

    pub const fn capabilities(&self) -> u16 {
        self.capabilities
    }

    /// The number of PMKIDs the element lists: a station resuming a cached
    /// PMKSA names its PMKID.
    pub const fn pmkid_count(&self) -> usize {
        self.pmkid_count
    }

    /// Whether the element's AKM list names `akm`.
    pub fn lists_akm(&self, akm: Akm) -> Result<bool, RsnSyntaxError> {
        Ok(self
            .element()?
            .akm_suites()
            .iter()
            .any(|selector| Akm::from_suite_selector(selector) == Some(akm)))
    }

    /// The element `validate_rsn_element` parsed.
    fn element(&self) -> Result<RsnElement<'_>, RsnSyntaxError> {
        RsnElement::parse(self.owned.as_bytes())
    }

    /// The PMKIDs the element lists, in order.
    pub fn pmkids(&self) -> Result<impl Iterator<Item = [u8; RSN_PMKID_LEN]> + '_, RsnSyntaxError> {
        Ok(self.element()?.pmkids())
    }

    /// The management frame protection the element advertises: its MFPC
    /// and MFPR. Validation refused MFPR without MFPC.
    pub const fn management_frame_protection(&self) -> ManagementProtection {
        if self.capabilities & RSN_CAPABILITY_MFPR != 0 {
            ManagementProtection::Required
        } else if self.capabilities & RSN_CAPABILITY_MFPC != 0 {
            ManagementProtection::Capable
        } else {
            ManagementProtection::Disabled
        }
    }

    pub fn into_owned(self) -> OwnedRsnIe {
        self.owned
    }
}

/// Validate the CCMP subset implemented by the handshake state machines and
/// select the first supported [`Akm`] of the element.
///
/// A syntactically incomplete element is [`RsnElementError::Malformed`]
/// before any suite or capability policy is applied. A listed PMKID is
/// reported, not judged: whether a PMKSA can be resumed is the caller's.
pub fn validate_rsn_element(bytes: &[u8]) -> Result<ValidatedRsnElement, RsnElementError> {
    let owned = OwnedRsnIe::try_copy(bytes).map_err(|error| match error {
        RsnFrameError::CapacityExceeded => RsnElementError::CapacityExceeded,
        _ => RsnElementError::Malformed,
    })?;
    let rsn = RsnElement::parse(bytes).map_err(|error| match error {
        RsnSyntaxError::Malformed => RsnElementError::Malformed,
        RsnSyntaxError::UnsupportedVersion => RsnElementError::UnsupportedVersion,
    })?;

    if rsn.group_data_cipher() != ieee_suite(RSN_CIPHER_CCMP) {
        return Err(RsnElementError::UnsupportedGroupCipher);
    }
    if !rsn.pairwise_ciphers().contains(ieee_suite(RSN_CIPHER_CCMP)) {
        return Err(RsnElementError::UnsupportedPairwiseCipher);
    }
    let capabilities = rsn.capabilities().unwrap_or(0);
    let capable = capabilities & RSN_CAPABILITY_MFPC != 0;
    // SAE requires management frame protection.
    let akm = rsn
        .akm_suites()
        .iter()
        .filter_map(Akm::from_suite_selector)
        .find(|akm| capable || *akm != Akm::Sae)
        .ok_or(RsnElementError::UnsupportedAkm)?;
    // Required protection without the capability is contradictory, and so
    // is a group management cipher of a party that protects nothing.
    if capabilities & RSN_CAPABILITY_MFPR != 0 && !capable {
        return Err(RsnElementError::Malformed);
    }
    match rsn.group_management_cipher() {
        None => {}
        Some(_) if !capable => return Err(RsnElementError::Malformed),
        Some(cipher) if cipher == ieee_suite(RSN_CIPHER_BIP_CMAC_128) => {}
        Some(_) => return Err(RsnElementError::UnsupportedGroupManagementCipher),
    }
    Ok(ValidatedRsnElement {
        owned,
        akm,
        capabilities,
        pmkid_count: rsn.pmkid_count().unwrap_or(0),
    })
}

#[cfg(test)]
mod tests;
