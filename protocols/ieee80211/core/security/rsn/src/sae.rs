//! Simultaneous Authentication of Equals (SAE) over group 19 (NIST P-256).
//!
//! WPA3-Personal authenticates both peers with a password and derives the PMK
//! from an elliptic-curve Diffie-Hellman exchange bound to it. Each peer sends
//! an SAE Commit (a scalar and an element) and an SAE Confirm (an HMAC over
//! both commits). The password element (PWE) comes either from the looping
//! hunting-and-pecking derivation or from the hash-to-element (H2E)
//! derivation through a password token (PT).
//!
//! The station follows the vendor supplicant: it offers only group 19, and it
//! uses H2E when the access point advertises it in its RSNXE, hunting and
//! pecking otherwise (`sae_pwe_h2e` defaults to both).
//!
//! SOURCE(esp32s31): ESP-IDF `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
//! `components/wpa_supplicant/src/common/sae.c` and `dragonfly.c`,
//! `components/wpa_supplicant/esp_supplicant/src/esp_wpa3.c`; the pinned
//! `libnet80211.a[ieee80211_ioctl.o]::map_wifi_config_sae_pwe_to_supp`;
//! IEEE Std 802.11-2020 12.4 and Annex J.10.

use core::ops::{Mul, Neg};
use hmac::Mac;
use oer_protocols_ieee802_wire::Eui48;
use oer_protocols_ieee80211_wire::sae::{
    SAE_CONFIRM_LEN, SAE_ELEMENT_LEN, SAE_PRIME_LEN, SaeBodyError, SaeCommitValues, SaeConfirm,
};
use oer_protocols_ieee80211_wire::security::{Pmkid, rsn::RSN_PMKID_LEN};
use p256::{
    AffinePoint, FieldBytes, FieldElement, ProjectivePoint, Scalar,
    elliptic_curve::{
        Field, PrimeField,
        bigint::{NonZero, U256, U384},
        group::Group,
        ops::Reduce,
        point::{AffineCoordinates, DecompressPoint},
        subtle::{Choice, ConditionallySelectable, ConstantTimeEq, CtOption},
    },
};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::kdf::kdf_sha256;

/// Octets of the KCK and of the PMK of group 19.
pub const SAE_KEY_LEN: usize = 32;

/// The minimum hunting-and-pecking iterations of an ECC group.
const MINIMUM_PWE_ITERATIONS: u8 = 40;
/// The vendor gives up hunting and pecking after this many iterations.
const MAXIMUM_PWE_ITERATIONS: u8 = 200;
/// SSWU parameter Z of group 19.
const SSWU_Z: u64 = 10;
/// Octets of pwd-value in H2E: olen(p) + ceil(olen(p)/2).
const H2E_PWD_VALUE_LEN: usize = {
    if !SAE_PRIME_LEN.is_multiple_of(2) {
        panic!("group 19 field length must be even");
    }
    SAE_PRIME_LEN + SAE_PRIME_LEN / 2
};

const P256_MODULUS: U256 =
    U256::from_be_hex("ffffffff00000001000000000000000000000000ffffffffffffffffffffffff");
const P256_ORDER: U256 =
    U256::from_be_hex("ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551");
// b represented in four base-2^64 limbs, evaluated entirely at compile time.
const CURVE_B: FieldElement = {
    let radix = FieldElement::from_u64(u64::MAX).add(&FieldElement::ONE);
    FieldElement::from_u64(0x5ac635d8aa3a93e7)
        .multiply(&radix)
        .add(&FieldElement::from_u64(0xb3ebbd55769886bc))
        .multiply(&radix)
        .add(&FieldElement::from_u64(0x651d06b0cc53b0f6))
        .multiply(&radix)
        .add(&FieldElement::from_u64(0x3bce3c3e27d2604b))
};
const WIDE_MODULUS: NonZero<U384> = {
    let value = U384::from_be_hex(
        "00000000000000000000000000000000ffffffff00000001000000000000000000000000ffffffffffffffffffffffff",
    );
    if value.bits_vartime() == 0 {
        panic!("the P-256 modulus must be nonzero");
    }
    NonZero::<U384>::const_new(value).0
};
const ORDER_MINUS_ONE: NonZero<U256> = {
    let value = P256_ORDER.wrapping_sub(&U256::ONE);
    if value.bits_vartime() == 0 {
        panic!("the P-256 order minus one must be nonzero");
    }
    NonZero::<U256>::const_new(value).0
};

/// Why an SAE computation or received message was rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaeError {
    /// An internal field or curve conversion failed its mathematical invariant.
    Crypto,
    /// Hunting and pecking found no password element.
    NoPasswordElement,
    /// A random value outside 1 < value < r, or a commit scalar of 0 or 1.
    UnsuitableRandom,
    /// A commit message of another group.
    UnsupportedGroup(u16),
    /// A commit or confirm body of the wrong length.
    Malformed,
    /// A peer scalar outside 1 < scalar < r.
    InvalidScalar,
    /// A peer element not on the curve.
    InvalidElement,
    /// The peer reflected this station's own commit.
    Reflection,
    /// A commit element or shared secret is the point at infinity.
    InfiniteSecret,
    /// The peer's confirm does not verify.
    ConfirmMismatch,
}

impl From<SaeBodyError> for SaeError {
    fn from(error: SaeBodyError) -> Self {
        match error {
            SaeBodyError::Malformed => Self::Malformed,
            SaeBodyError::UnsupportedGroup(group) => Self::UnsupportedGroup(group),
        }
    }
}

/// The longest SAE password: the vendor station profile's password field
/// holds 63 octets.
pub const SAE_PASSWORD_MAX_LEN: usize = 63;

/// One SAE password, retained for the password element every attempt
/// derives and cleared on drop. It cannot be formatted.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct SaePassword {
    bytes: heapless::Vec<u8, SAE_PASSWORD_MAX_LEN, u8>,
}

impl SaePassword {
    /// A password of 1 to 63 octets.
    pub fn new(password: &[u8]) -> Option<Self> {
        if password.is_empty() || password.len() > SAE_PASSWORD_MAX_LEN {
            return None;
        }
        Some(Self {
            bytes: heapless::Vec::from_slice(password).ok()?,
        })
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.bytes.as_slice()
    }
}

impl core::fmt::Debug for SaePassword {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("SaePassword(<redacted>)")
    }
}

/// A password element: the generator both peers mask their commits with.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct SaePasswordElement(ProjectivePoint);

/// The H2E password token of one SSID, password and optional identifier.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct SaePasswordToken(ProjectivePoint);

fn hmac_sha256(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut mac = crate::mac::sha256(key);
    for part in parts {
        mac.update(part);
    }
    mac.finalize().into_bytes().into()
}

/// The larger address first, then the smaller.
fn max_min(first: Eui48, second: Eui48) -> [u8; 12] {
    const {
        if 12 != 2 * Eui48::LEN {
            panic!("SAE address context geometry");
        }
    }
    let (high, low) = if first > second {
        (first, second)
    } else {
        (second, first)
    };
    let mut addresses = [0; 12];
    for (target, source) in addresses
        .iter_mut()
        .zip(high.octets().iter().chain(low.as_octets()))
    {
        *target = *source;
    }
    addresses
}

fn curve_b() -> FieldElement {
    CURVE_B
}

/// x^3 - 3x + b.
fn curve_rhs(x: &FieldElement) -> FieldElement {
    let three = FieldElement::from_u64(3);
    x.square()
        .multiply(x)
        .sub(&three.multiply(x))
        .add(&curve_b())
}

/// Validate coordinates without SEC1's tag parser or any secret-dependent branch.
fn point(x: &FieldElement, y: &FieldElement) -> CtOption<ProjectivePoint> {
    AffinePoint::decompress(&x.to_bytes(), y.is_odd()).and_then(|point| {
        CtOption::new(
            ProjectivePoint::from(point),
            y.square().ct_eq(&curve_rhs(x)),
        )
    })
}

impl SaePasswordElement {
    /// Hunting and pecking: hash the password with a counter until the value
    /// is the x coordinate of a curve point, always running at least 40
    /// iterations. The last bit of the successful seed selects y.
    pub fn hunting_and_pecking(
        password: &[u8],
        local: Eui48,
        peer: Eui48,
    ) -> Result<Self, SaeError> {
        let key = max_min(local, peer);
        let prime = crate::crypto::octets_of_u256(P256_MODULUS);
        let mut found = Choice::from(0);
        let mut x_bytes = [0_u8; SAE_PRIME_LEN];
        let mut seed_odd = Choice::from(0);
        for counter in 1..=MAXIMUM_PWE_ITERATIONS {
            let mut seed = hmac_sha256(&key, &[password, &[counter]]);
            let mut value = [0_u8; SAE_PRIME_LEN];
            kdf_sha256(&seed, b"SAE Hunting and Pecking", &prime, &mut value);
            let candidate = FieldElement::from_bytes(&FieldBytes::from(value));
            let on_curve = candidate.is_some()
                & candidate
                    .map(|x| curve_rhs(&x).sqrt().is_some())
                    .unwrap_or(Choice::from(0));
            let take = on_curve & !found;
            for (stored, byte) in x_bytes.iter_mut().zip(value) {
                stored.conditional_assign(&byte, take);
            }
            let [.., last_seed] = seed;
            seed_odd.conditional_assign(&Choice::from(last_seed & 1), take);
            found |= on_curve;
            seed.zeroize();
            value.zeroize();
            if counter >= MINIMUM_PWE_ITERATIONS && bool::from(found) {
                break;
            }
        }
        if !bool::from(found) {
            return Err(SaeError::NoPasswordElement);
        }
        let x = FieldElement::from_bytes(&FieldBytes::from(x_bytes));
        x_bytes.zeroize();
        let point = x.and_then(|x| {
            curve_rhs(&x).sqrt().and_then(|y| {
                let y = FieldElement::conditional_select(&y.neg(), &y, y.is_odd().ct_eq(&seed_odd));
                point(&x, &y)
            })
        });
        Option::<ProjectivePoint>::from(point)
            .map(Self)
            .ok_or(SaeError::Crypto)
    }
}

/// u = value mod p for a 48-octet H2E value.
fn reduce_wide(value: &[u8; H2E_PWD_VALUE_LEN]) -> CtOption<FieldElement> {
    let reduced =
        crate::crypto::octets_of_u384(crate::crypto::u384_of_octets(*value).rem(&WIDE_MODULUS));
    let mut low = [0; SAE_PRIME_LEN];
    for (target, source) in low.iter_mut().rev().zip(reduced.iter().rev()) {
        *target = *source;
    }
    FieldElement::from_bytes(&FieldBytes::from(low))
}

/// Simplified SWU. Invalid intermediate values retain their validity bit;
/// both candidate coordinates are always evaluated and selected in constant time.
fn sswu(u: &FieldElement) -> CtOption<ProjectivePoint> {
    let z = FieldElement::ZERO.sub(&FieldElement::from_u64(SSWU_Z));
    let a = FieldElement::ZERO.sub(&FieldElement::from_u64(3));
    let b = curve_b();
    let t1 = z.multiply(&u.square());
    let m = t1.add(&t1.square());
    // SSWU defines inv0(0) = 0; x1a is selected for this case below.
    let t = m.invert().unwrap_or(FieldElement::ZERO);
    z.multiply(&a).invert().and_then(|za_inv| {
        a.invert().and_then(|a_inv| {
            let x1a = b.multiply(&za_inv);
            let x1b = FieldElement::ZERO
                .sub(&b)
                .multiply(&a_inv)
                .multiply(&FieldElement::ONE.add(&t));
            let x1 = FieldElement::conditional_select(&x1b, &x1a, m.is_zero());
            let gx1 = curve_rhs(&x1);
            let x2 = t1.multiply(&x1);
            let gx2 = curve_rhs(&x2);
            let gx1_square = gx1.sqrt().is_some();
            let v = FieldElement::conditional_select(&gx2, &gx1, gx1_square);
            let x = FieldElement::conditional_select(&x2, &x1, gx1_square);
            v.sqrt().and_then(|y| {
                let y =
                    FieldElement::conditional_select(&y.neg(), &y, u.is_odd().ct_eq(&y.is_odd()));
                point(&x, &y)
            })
        })
    })
}

impl SaePasswordToken {
    /// PT = SSWU(u1) + SSWU(u2) with u1, u2 expanded from
    /// HKDF-Extract(ssid, password [|| identifier]).
    pub fn derive(
        ssid: &[u8],
        password: &[u8],
        identifier: Option<&[u8]>,
    ) -> Result<Self, SaeError> {
        Self::derive_with_map(ssid, password, identifier, sswu)
    }

    fn derive_with_map(
        ssid: &[u8],
        password: &[u8],
        identifier: Option<&[u8]>,
        mut map: impl FnMut(&FieldElement) -> CtOption<ProjectivePoint>,
    ) -> Result<Self, SaeError> {
        let mut seed = hmac_sha256(ssid, &[password, identifier.unwrap_or(&[])]);
        let mut element = |label: &[u8]| {
            let mut value = [0_u8; H2E_PWD_VALUE_LEN];
            hkdf_expand(&seed, label, &mut value);
            let u = reduce_wide(&value);
            value.zeroize();
            u.and_then(|u| map(&u))
        };
        let first = element(b"SAE Hash to Element u1 P1");
        let second = element(b"SAE Hash to Element u2 P2");
        let token = first.and_then(|first| second.map(|second| core::ops::Add::add(first, second)));
        seed.zeroize();
        Option::<ProjectivePoint>::from(token)
            .map(Self)
            .ok_or(SaeError::Crypto)
    }

    /// PWE = val * PT, val = H(0^32, MAX(addresses) || MIN(addresses))
    /// mod (r - 1) + 1.
    pub fn password_element(&self, local: Eui48, peer: Eui48) -> SaePasswordElement {
        let hash = hmac_sha256(&[0; 32], &[&max_min(local, peer)]);

        let value = crate::crypto::u256_of_octets(hash)
            .rem(&ORDER_MINUS_ONE)
            .wrapping_add(&U256::ONE);
        SaePasswordElement(self.0.mul(<Scalar as Reduce<U256>>::reduce(value)))
    }
}

/// HKDF-Expand with SHA-256.
fn hkdf_expand<const N: usize>(prk: &[u8; 32], info: &[u8], output: &mut [u8; N]) {
    const {
        if N > 255 * 32 {
            panic!("HKDF output must fit 255 blocks");
        }
    }
    let mut previous = [0_u8; 32];

    let mut remaining = output.iter_mut().peekable();
    for counter in 1_u8..=u8::MAX {
        if remaining.peek().is_none() {
            break;
        }
        let previous_input: &[u8] = if counter == 1 {
            &[]
        } else {
            previous.as_slice()
        };
        let block = hmac_sha256(prk, &[previous_input, info, &[counter]]);
        for source in &block {
            if let Some(target) = remaining.next() {
                *target = *source;
            }
        }
        previous = block;
    }
    previous.zeroize();
}

fn scalar_from(bytes: &[u8; SAE_PRIME_LEN]) -> Option<Scalar> {
    Option::from(Scalar::from_repr(FieldBytes::from(*bytes)))
}

fn element_bytes(element: &AffinePoint) -> Result<[u8; SAE_ELEMENT_LEN], SaeError> {
    let x = FieldElement::from_bytes(&element.x());
    let coordinates = x.and_then(|x| {
        curve_rhs(&x).sqrt().map(|y| {
            let y = FieldElement::conditional_select(
                &y.neg(),
                &y,
                element.y_is_odd().ct_eq(&y.is_odd()),
            );
            let mut bytes = [0; SAE_ELEMENT_LEN];
            for (target, source) in bytes
                .iter_mut()
                .zip(x.to_bytes().iter().chain(y.to_bytes().iter()))
            {
                *target = *source;
            }
            bytes
        })
    });
    if bool::from(element.is_identity()) {
        return Err(SaeError::InfiniteSecret);
    }
    Option::<[u8; SAE_ELEMENT_LEN]>::from(coordinates).ok_or(SaeError::Crypto)
}

fn peer_element(element: &[u8; SAE_ELEMENT_LEN]) -> Option<AffinePoint> {
    let (x, y) = oer_protocols_ieee80211_wire::sae::element_coordinates(element)?;
    let x = FieldElement::from_bytes(&FieldBytes::from(*x));
    let y = FieldElement::from_bytes(&FieldBytes::from(*y));
    Option::<ProjectivePoint>::from(x.and_then(|x| y.and_then(|y| point(&x, &y))))
        .map(|point| point.to_affine())
}

/// This station's commit and the secrets behind it.
#[derive(ZeroizeOnDrop)]
pub struct SaeCommit {
    #[zeroize(skip)]
    pwe: SaePasswordElement,
    rand: Scalar,
    own_scalar: Scalar,
    #[zeroize(skip)]
    own: SaeCommitValues,
}

/// The keys an accepted commit exchange derives.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct SaeKeys {
    kck: [u8; SAE_KEY_LEN],
    pmk: [u8; SAE_KEY_LEN],
    pmkid: [u8; RSN_PMKID_LEN],
    #[zeroize(skip)]
    own: SaeCommitValues,
    #[zeroize(skip)]
    peer: SaeCommitValues,
}

impl SaeCommit {
    /// Commit with `rand` and `mask`, each drawn uniformly from [2, r - 1]
    /// by the caller; the scalar (rand + mask) mod r must exceed 1.
    pub fn new(
        pwe: &SaePasswordElement,
        rand: [u8; SAE_PRIME_LEN],
        mask: [u8; SAE_PRIME_LEN],
    ) -> Result<Self, SaeError> {
        let (Some(rand_scalar), Some(mask_scalar)) = (scalar_from(&rand), scalar_from(&mask))
        else {
            return Err(SaeError::UnsuitableRandom);
        };
        if bool::from(rand_scalar.is_zero() | rand_scalar.ct_eq(&Scalar::ONE))
            || bool::from(mask_scalar.is_zero() | mask_scalar.ct_eq(&Scalar::ONE))
        {
            return Err(SaeError::UnsuitableRandom);
        }
        let scalar = core::ops::Add::add(rand_scalar, mask_scalar);
        if bool::from(scalar.is_zero() | scalar.ct_eq(&Scalar::ONE)) {
            return Err(SaeError::UnsuitableRandom);
        }
        let element = pwe.0.mul(mask_scalar).neg().to_affine();
        Ok(Self {
            pwe: SaePasswordElement(pwe.0),
            rand: rand_scalar,
            own_scalar: scalar,
            own: SaeCommitValues {
                scalar: scalar.to_bytes().into(),
                element: element_bytes(&element)?,
            },
        })
    }

    pub const fn values(&self) -> SaeCommitValues {
        self.own
    }

    /// Process the peer's commit and derive KCK, PMK and PMKID with the
    /// keyseed salt of hunting and pecking or H2E without rejected groups:
    /// zeros.
    pub fn process(&self, peer: SaeCommitValues) -> Result<SaeKeys, SaeError> {
        let peer_scalar = scalar_from(&peer.scalar).ok_or(SaeError::InvalidScalar)?;
        if bool::from(peer_scalar.is_zero() | peer_scalar.ct_eq(&Scalar::ONE)) {
            return Err(SaeError::InvalidScalar);
        }
        let peer_element = peer_element(&peer.element).ok_or(SaeError::InvalidElement)?;
        if peer == self.own {
            return Err(SaeError::Reflection);
        }
        let rand = self.rand;
        let shared = core::ops::Add::add(
            self.pwe.0.mul(peer_scalar),
            ProjectivePoint::from(peer_element),
        )
        .mul(rand);
        if bool::from(shared.is_identity()) {
            return Err(SaeError::InfiniteSecret);
        }
        let mut k: [u8; SAE_PRIME_LEN] = shared.to_affine().x().into();
        let mut keyseed = hmac_sha256(&[0; 32], &[&k]);
        k.zeroize();
        let own_scalar = self.own_scalar;
        let context: [u8; SAE_PRIME_LEN] = core::ops::Add::add(own_scalar, peer_scalar)
            .to_bytes()
            .into();
        const {
            if !(2 * SAE_KEY_LEN == 64 && RSN_PMKID_LEN <= SAE_PRIME_LEN) {
                panic!("SAE derived key geometry");
            }
        }
        let mut keys = [0_u8; 64];
        kdf_sha256(&keyseed, b"SAE KCK and PMK", &context, &mut keys);
        keyseed.zeroize();
        let mut derived = SaeKeys {
            kck: [0; SAE_KEY_LEN],
            pmk: [0; SAE_KEY_LEN],
            pmkid: [0; RSN_PMKID_LEN],
            own: self.own,
            peer,
        };
        for (target, source) in derived.kck.iter_mut().chain(&mut derived.pmk).zip(&keys) {
            *target = *source;
        }
        for (target, source) in derived.pmkid.iter_mut().zip(&context) {
            *target = *source;
        }
        keys.zeroize();
        Ok(derived)
    }
}

impl SaeKeys {
    pub const fn kck(&self) -> &[u8; SAE_KEY_LEN] {
        &self.kck
    }
    pub const fn pmk(&self) -> &[u8; SAE_KEY_LEN] {
        &self.pmk
    }
    pub const fn pmkid(&self) -> Pmkid {
        Pmkid::from_octets(self.pmkid)
    }

    fn confirm(
        &self,
        send_confirm: u16,
        first: &SaeCommitValues,
        second: &SaeCommitValues,
    ) -> [u8; 32] {
        hmac_sha256(
            &self.kck,
            &[
                &oer_protocols_ieee80211_wire::sae::SaeField::of(send_confirm).to_bytes(),
                &first.scalar,
                &first.element,
                &second.scalar,
                &second.element,
            ],
        )
    }

    /// This station's confirm body.
    pub fn own_confirm(&self, send_confirm: u16) -> [u8; SAE_CONFIRM_LEN] {
        SaeConfirm {
            send_confirm,
            confirm: &self.confirm(send_confirm, &self.own, &self.peer),
        }
        .encode()
    }

    /// Verify the peer's confirm body and return its send-confirm.
    pub fn verify_peer_confirm(&self, body: &[u8]) -> Result<u16, SaeError> {
        let SaeConfirm {
            send_confirm,
            confirm,
        } = SaeConfirm::parse(body)?;
        let expected = self.confirm(send_confirm, &self.peer, &self.own);
        if bool::from(expected.ct_eq(confirm)) {
            Ok(send_confirm)
        } else {
            Err(SaeError::ConfirmMismatch)
        }
    }
}

#[cfg(test)]
mod tests;

impl core::fmt::Debug for SaePasswordElement {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("SaePasswordElement(<redacted>)")
    }
}

impl core::fmt::Debug for SaePasswordToken {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("SaePasswordToken(<redacted>)")
    }
}

impl core::fmt::Debug for SaeKeys {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("SaeKeys(<redacted>)")
    }
}
