//! Association RSN element validation and suite selection.
//!
//! An authenticator validates a station's Association element with it, and a
//! supplicant derives the suite of its own element. Peer queues, callbacks and
//! chip node pointers are deliberately outside this crate.

use oer_ieee80211_mac::security::rsn::{
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

/// The management frame protection an RSN element advertises.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ManagementFrameProtection {
    /// MFPC: the party protects management frames when its peer does.
    pub capable: bool,
    /// MFPR: the party associates only with protected management frames.
    pub required: bool,
}

impl ManagementFrameProtection {
    /// Protection is in effect when both parties are capable.
    pub const fn negotiated(self, peer: Self) -> bool {
        self.capable && peer.capable
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedRsnElement {
    owned: OwnedRsnIe,
    akm: Akm,
    capabilities: u16,
    pmkid_count: u16,
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
    pub const fn pmkid_count(&self) -> u16 {
        self.pmkid_count
    }

    /// Whether the element's AKM list names `akm`.
    pub fn lists_akm(&self, akm: Akm) -> bool {
        RsnElement::parse(self.owned.as_bytes())
            .expect("a validated element parses again")
            .akm_suites()
            .iter()
            .any(|selector| Akm::from_suite_selector(selector) == Some(akm))
    }

    /// The PMKIDs the element lists, in order.
    pub fn pmkids(&self) -> impl Iterator<Item = [u8; RSN_PMKID_LEN]> + '_ {
        RsnElement::parse(self.owned.as_bytes())
            .expect("a validated element parses again")
            .pmkids()
    }

    pub const fn management_frame_protection(&self) -> ManagementFrameProtection {
        ManagementFrameProtection {
            capable: self.capabilities & RSN_CAPABILITY_MFPC != 0,
            required: self.capabilities & RSN_CAPABILITY_MFPR != 0,
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
