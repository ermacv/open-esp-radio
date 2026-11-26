//! Zeroizing RSN master/transient keys, PSK derivation and commitments.

use crate::AkmKeys;
use hmac::{Hmac, Mac};
use oer_portable_crypto::Aes128Key;
use oer_protocols_ieee802_wire::Eui48;
use oer_protocols_ieee80211_contracts_security::Ccmp128Key;
use sha1::Sha1;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::Akm;

mod pbkdf2;
pub use pbkdf2::PmkDerivation;

/// The octet string of a 256-bit number, most significant octet first.
///
/// SOURCE: RFC 8017, 4.1 (I2OSP), as IEEE Std 802.11-2020, 12.4.4.2.1
/// converts a field element to octets.
pub(crate) fn octets_of_u256(value: p256::U256) -> [u8; 32] {
    p256::elliptic_curve::bigint::Encoding::to_be_bytes(&value)
}

/// The 256-bit number of an octet string, most significant octet first.
///
/// SOURCE: RFC 8017, 4.2 (OS2IP).
pub(crate) fn u256_of_octets(octets: [u8; 32]) -> p256::U256 {
    <p256::U256 as p256::elliptic_curve::bigint::Encoding>::from_be_bytes(octets)
}

/// The octet string of a 384-bit number, most significant octet first.
///
/// SOURCE: RFC 8017, 4.1 (I2OSP).
pub(crate) fn octets_of_u384(value: p256::elliptic_curve::bigint::U384) -> [u8; 48] {
    p256::elliptic_curve::bigint::Encoding::to_be_bytes(&value)
}

/// The 384-bit number of an octet string, most significant octet first.
///
/// SOURCE: RFC 8017, 4.2 (OS2IP).
pub(crate) fn u384_of_octets(octets: [u8; 48]) -> p256::elliptic_curve::bigint::U384 {
    <p256::elliptic_curve::bigint::U384 as p256::elliptic_curve::bigint::Encoding>::from_be_bytes(
        octets,
    )
}

oer_portable_wire::bitfield! {
    /// The binding's length of the association security IEs: a 64-bit
    /// big-endian count of octets, as this crate's binding defines it.
    struct BindingLength[8] (big) {
        value, with_value: {u64} = 0..64;
    }
}

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
    pub authenticator_address: Eui48,
    pub supplicant_address: Eui48,
    pub authenticator_nonce: oer_protocols_ieee80211_wire::eapol::Nonce,
    pub supplicant_nonce: oer_protocols_ieee80211_wire::eapol::Nonce,
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

    /// Copy borrowed material straight into its zeroizing owner.
    pub(crate) const fn copy_from(bytes: &[u8; 32]) -> Self {
        Self(*bytes)
    }

    /// The key, for the suite's PRF expanding it.
    pub(crate) const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Derive the pairwise transient key with the association's suite.
    pub fn derive_ptk(&self, akm: Akm, context: PtkContext) -> Ptk {
        const {
            if !(RSN_PTK_LEN == core::mem::size_of::<[[u8; 16]; 3]>()
                && RSN_PTK_LEN == RSN_KCK_LEN + RSN_KEK_LEN + 16
                && core::mem::size_of::<[u8; 76]>()
                    == 2 * (Eui48::LEN + core::mem::size_of::<[u8; 32]>()))
            {
                panic!("PTK expansion geometry");
            }
        }
        let mut canonical = [0; 76];
        let (first_address, second_address) = ordered(
            context.authenticator_address.as_octets(),
            context.supplicant_address.as_octets(),
        );

        let (first_nonce, second_nonce) = ordered(
            context.authenticator_nonce.octets(),
            context.supplicant_nonce.octets(),
        );
        for (target, source) in canonical.iter_mut().zip(
            first_address
                .iter()
                .chain(second_address)
                .chain(first_nonce)
                .chain(second_nonce),
        ) {
            *target = *source;
        }
        // Split the expansion into the KCK, KEK and TK owners, then erase
        // the temporary expansion and canonical context before returning.
        let mut expanded = [[0; 16]; 3];
        akm.expand_ptk(self, &canonical, &mut expanded);
        let [kck, kek, temporal_key] = &expanded;
        let ptk = Ptk {
            akm,
            kck: Kck(Aes128Key::copy_from(kck)),
            kek: Kek(Aes128Key::copy_from(kek)),
            temporal_key: Ccmp128Key::new(temporal_key),
        };
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
        // `usize` is at most 64 bits on every target: the widening is lossless.
        let length = association_security_ies.len() as u64;
        mac.update(
            &BindingLength::from_bytes([0; 8])
                .with_value(length)
                .to_bytes(),
        );
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
    kck: Kck,
    kek: Kek,
    temporal_key: Ccmp128Key,
}

/// The EAPOL-Key confirmation key (KCK, IEEE Std 802.11-2020, 12.7.1.3):
/// the key of the EAPOL-Key MIC alone, an HMAC or AES-CMAC key by its
/// suite.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct Kck(Aes128Key);

impl Kck {
    /// A KCK of the given octets, for a test.
    #[cfg(test)]
    pub(crate) const fn copy_from(bytes: &[u8; RSN_KCK_LEN]) -> Self {
        Self(Aes128Key::copy_from(bytes))
    }

    /// Lend the key's octets, for an HMAC suite and for a test.
    pub const fn as_bytes(&self) -> &[u8; RSN_KCK_LEN] {
        self.0.as_bytes()
    }

    /// Lend the key to an AES-128 engine, for an AES-CMAC suite.
    pub(crate) const fn aes_key(&self) -> &Aes128Key {
        &self.0
    }

    /// A second zeroizing owner of the same key.
    const fn second_owner(&self) -> Self {
        Self(Aes128Key::copy_from(self.0.as_bytes()))
    }
}

/// The EAPOL-Key encryption key (KEK, IEEE Std 802.11-2020, 12.7.1.3): the
/// key that wraps and unwraps the Key Data alone.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct Kek(Aes128Key);

impl Kek {
    /// Lend the key's octets, for a test's key wrap.
    pub const fn as_bytes(&self) -> &[u8; RSN_KEK_LEN] {
        self.0.as_bytes()
    }

    /// Lend the key to an AES-128 engine, for a key wrap or unwrap.
    pub(crate) const fn aes_key(&self) -> &Aes128Key {
        &self.0
    }

    /// A second zeroizing owner of the same key.
    const fn second_owner(&self) -> Self {
        Self(Aes128Key::copy_from(self.0.as_bytes()))
    }
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
    kck: Kck,
}

impl Ptk {
    pub const fn akm(&self) -> Akm {
        self.akm
    }

    pub const fn kck(&self) -> &Kck {
        &self.kck
    }
    pub const fn kek(&self) -> &Kek {
        &self.kek
    }
    /// The temporal key, lent for software CCMP.
    pub const fn temporal_key(&self) -> &Ccmp128Key {
        &self.temporal_key
    }

    /// A second owner of the temporal key, for its installation.
    pub const fn install_key(&self) -> Ccmp128Key {
        Ccmp128Key::new(self.temporal_key.as_bytes())
    }

    /// The connected state's KCK and KEK (the KEK retained only for an
    /// authenticated Group Message 1), without the installed temporal key.
    pub(crate) fn into_connected_keys(mut self) -> (RsnKeyConfirmationKey, Kek) {
        let keys = (
            RsnKeyConfirmationKey {
                akm: self.akm,
                kck: self.kck.second_owner(),
            },
            self.kek.second_owner(),
        );
        // Do not retain a second software copy of the installed temporal key
        // until this value happens to leave scope.
        self.zeroize();
        keys
    }
}

impl RsnKeyConfirmationKey {
    pub(crate) const fn akm(&self) -> Akm {
        self.akm
    }

    pub(crate) const fn kck(&self) -> &Kck {
        &self.kck
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
