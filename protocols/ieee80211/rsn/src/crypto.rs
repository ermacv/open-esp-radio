//! Zeroizing RSN master/transient keys, PSK derivation and commitments.

use crate::AkmKeys;
use hmac::{Hmac, Mac};
use sha1::Sha1;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::Akm;

mod pbkdf2;
pub use pbkdf2::PmkDerivation;

pub const PSK_PASSPHRASE_MIN_LEN: usize = 8;
pub const PSK_PASSPHRASE_MAX_LEN: usize = 63;
pub const PSK_SSID_MAX_LEN: usize = 32;
pub const PSK_PBKDF2_ITERATIONS: u32 = 4096;
pub const RSN_PTK_LEN: usize = 48;
pub const RSN_KCK_LEN: usize = 16;
pub const RSN_KEK_LEN: usize = 16;

const RSN_ASSOCIATION_SECURITY_BINDING_LABEL: &[u8] = b"open-esp-radio AP association security IEs";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PskDerivationError {
    InvalidPassphraseLength,
    InvalidSsidLength,
}

/// Canonical addresses and nonces used by the WPA2 PRF-384.
#[derive(Clone, Copy)]
pub struct PtkContext {
    pub authenticator_address: [u8; 6],
    pub supplicant_address: [u8; 6],
    pub authenticator_nonce: [u8; 32],
    pub supplicant_nonce: [u8; 32],
}

/// Pairwise master key. The bytes cannot be formatted and are cleared on drop.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct Pmk([u8; 32]);

/// PMK-authenticated commitment to the exact Association RSN IE plus RSNXE.
///
/// An authenticator stores this bounded value per peer instead of retaining a
/// second copy of the complete management-frame elements. It can later bind
/// Message 2 Key Data to the exact Association bytes without exposing the PMK
/// or expanding the fixed AP peer table by the IE capacity for every client.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct AssociationSecurityBinding([u8; 16]);

impl AssociationSecurityBinding {
    pub fn matches(&self, pmk: &Pmk, association_security_ies: &[u8]) -> bool {
        let mac = pmk.association_security_binding_mac(association_security_ies);
        mac.verify_truncated_left(&self.0).is_ok()
    }
}

impl Pmk {
    /// Import an already-derived 256-bit PSK.
    ///
    /// This avoids retaining a passphrase in applications which provision raw
    /// key material. The owned bytes receive the same zeroize-on-drop policy
    /// as a locally derived key.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// A second owner of the same key, for a PMKSA cache that outlives the
    /// association which derived it.
    pub fn duplicate(&self) -> Self {
        Self(self.0)
    }

    /// Derive the pairwise transient key with the association's suite.
    pub fn derive_ptk(&self, akm: Akm, context: PtkContext) -> Ptk {
        const {
            if !(RSN_PTK_LEN == 3 * 16
                && RSN_PTK_LEN == RSN_KCK_LEN + RSN_KEK_LEN + 16
                && core::mem::size_of::<[u8; 76]>()
                    == 2 * (core::mem::size_of::<[u8; 6]>() + core::mem::size_of::<[u8; 32]>()))
            {
                panic!("PTK expansion geometry");
            }
        }
        let mut canonical = [0; 76];
        let (first_address, second_address) =
            ordered(&context.authenticator_address, &context.supplicant_address);

        let (first_nonce, second_nonce) =
            ordered(&context.authenticator_nonce, &context.supplicant_nonce);
        for (target, source) in canonical.iter_mut().zip(
            first_address
                .iter()
                .chain(second_address)
                .chain(first_nonce)
                .chain(second_nonce),
        ) {
            *target = *source;
        }
        // Split the expansion into typed KCK, KEK and TK blocks, then erase
        // the temporary expansion and canonical context before returning.
        let mut ptk = Ptk {
            akm,
            bytes: [[0; 16]; 3],
        };
        let mut expanded = [0; RSN_PTK_LEN];
        akm.expand_ptk(&self.0, &canonical, &mut expanded);
        for (target, source) in ptk.bytes.as_flattened_mut().iter_mut().zip(&expanded) {
            *target = *source;
        }
        expanded.zeroize();
        canonical.zeroize();
        ptk
    }

    pub fn bind_association_security_ies(
        &self,
        association_security_ies: &[u8],
    ) -> AssociationSecurityBinding {
        let mut digest = self
            .association_security_binding_mac(association_security_ies)
            .finalize()
            .into_bytes();
        let mut binding = [0; 16];
        for (target, source) in binding.iter_mut().zip(digest.iter()) {
            *target = *source;
        }
        digest.zeroize();
        AssociationSecurityBinding(binding)
    }

    fn association_security_binding_mac(&self, association_security_ies: &[u8]) -> Hmac<Sha1> {
        let mut mac = crate::mac::sha1(&self.0);
        mac.update(RSN_ASSOCIATION_SECURITY_BINDING_LABEL);
        mac.update(&(association_security_ies.len() as u64).to_be_bytes());
        mac.update(association_security_ies);
        mac
    }
}

/// Pairwise transient key material (KCK | KEK | TK) of one suite, cleared on
/// drop. The suite that derived it also selects its EAPOL-Key MIC.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct Ptk {
    #[zeroize(skip)]
    akm: Akm,
    bytes: [[u8; 16]; 3],
}

/// Connected-state EAPOL authentication authority derived from one PTK.
///
/// The temporal key is published to hardware before this value is created.
/// Keeping the KCK in its own zeroizing owner lets the connected supplicant
/// authenticate retransmitted Message 3 without retaining a software copy of
/// the installed CCMP key.
#[derive(Zeroize, ZeroizeOnDrop)]
pub(crate) struct RsnKeyConfirmationKey {
    #[zeroize(skip)]
    akm: Akm,
    bytes: [u8; RSN_KCK_LEN],
}

/// Connected-state key-data unwrap authority derived from one PTK.
///
/// This is retained only for an authenticated Group Message 1. It is separate
/// from the completed-Message-3 context so that path owns exactly its KCK and
/// protocol binding, never the installed temporal key.
#[derive(Zeroize, ZeroizeOnDrop)]
pub(crate) struct RsnKeyEncryptionKey([u8; RSN_KEK_LEN]);

impl Ptk {
    pub const fn akm(&self) -> Akm {
        self.akm
    }

    pub fn kck(&self) -> &[u8; RSN_KCK_LEN] {
        let [kck, _, _] = &self.bytes;
        kck
    }
    pub fn kek(&self) -> &[u8; RSN_KEK_LEN] {
        let [_, kek, _] = &self.bytes;
        kek
    }
    pub fn temporal_key(&self) -> &[u8; 16] {
        let [_, _, tk] = &self.bytes;
        tk
    }

    pub(crate) fn into_connected_keys(mut self) -> (RsnKeyConfirmationKey, RsnKeyEncryptionKey) {
        let kck = *self.kck();
        let kek = *self.kek();
        // Do not retain a second software copy of the installed temporal key
        // until this value happens to leave scope.
        let akm = self.akm;
        self.zeroize();
        (
            RsnKeyConfirmationKey { akm, bytes: kck },
            RsnKeyEncryptionKey(kek),
        )
    }
}

impl RsnKeyConfirmationKey {
    pub(crate) const fn akm(&self) -> Akm {
        self.akm
    }

    pub(crate) const fn as_bytes(&self) -> &[u8; RSN_KCK_LEN] {
        &self.bytes
    }
}

impl RsnKeyEncryptionKey {
    /// A zeroizing copy of a KEK handed to a key-data unwrap request.
    pub(crate) const fn copy_from(kek: &[u8; RSN_KEK_LEN]) -> Self {
        Self(*kek)
    }

    pub(crate) const fn as_bytes(&self) -> &[u8; RSN_KEK_LEN] {
        &self.0
    }
}

fn ordered<'a, const N: usize>(
    left: &'a [u8; N],
    right: &'a [u8; N],
) -> (&'a [u8; N], &'a [u8; N]) {
    if left <= right {
        (left, right)
    } else {
        (right, left)
    }
}

#[cfg(test)]
mod tests;
