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
use p256::{
    AffinePoint, FieldBytes, FieldElement, ProjectivePoint, Scalar,
    elliptic_curve::{
        Field, PrimeField,
        bigint::{Encoding, NonZero, U256, U384},
        group::Group,
        ops::Reduce,
        point::{AffineCoordinates, DecompressPoint},
        subtle::{Choice, ConditionallySelectable, ConstantTimeEq, CtOption},
    },
};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::kdf::kdf_sha256;
use oer_time::{Duration, Instant};

/// The only SAE group the station offers: NIST P-256.
pub const SAE_GROUP_P256: u16 = 19;
/// Octets of one P-256 field element or scalar.
pub const SAE_PRIME_LEN: usize = 32;
/// An SAE Commit body of group 19 without an anti-clogging token or
/// optional elements: group, scalar and element.
pub const SAE_COMMIT_LEN: usize = 2 + SAE_PRIME_LEN + 2 * SAE_PRIME_LEN;
/// An SAE Confirm body: send-confirm and the confirm HMAC.
pub const SAE_CONFIRM_LEN: usize = 2 + 32;
/// Octets of the KCK and of the PMK of group 19.
pub const SAE_KEY_LEN: usize = 32;
pub const SAE_PMKID_LEN: usize = 16;

const EXTENSION_ELEMENT_ID: u8 = 255;
/// Element ID Extension of the Rejected Groups element.
const REJECTED_GROUPS_ELEMENT: u8 = 92;
/// Element ID Extension of the Anti-Clogging Token Container element.
const ANTI_CLOGGING_TOKEN_ELEMENT: u8 = 93;
/// Element ID Extension of the Password Identifier element.
const PASSWORD_IDENTIFIER_ELEMENT: u8 = 33;
/// Octets of an anti-clogging token: its index and an HMAC-SHA256 tail.
pub const SAE_ANTI_CLOGGING_TOKEN_LEN: usize = 32;

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
fn max_min(first: [u8; 6], second: [u8; 6]) -> [u8; 12] {
    let (high, low) = if first > second {
        (first, second)
    } else {
        (second, first)
    };
    let mut addresses = [0; 12];
    for (target, source) in addresses.iter_mut().zip(high.iter().chain(&low)) {
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
        local: [u8; 6],
        peer: [u8; 6],
    ) -> Result<Self, SaeError> {
        let key = max_min(local, peer);
        let prime = P256_MODULUS.to_be_bytes();
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
    let reduced = U384::from_be_slice(value).rem(&WIDE_MODULUS).to_be_bytes();
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
    pub fn password_element(&self, local: [u8; 6], peer: [u8; 6]) -> SaePasswordElement {
        let hash = hmac_sha256(&[0; 32], &[&max_min(local, peer)]);

        let value = U256::from_be_slice(&hash)
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

fn element_bytes(element: &AffinePoint) -> Result<[u8; 2 * SAE_PRIME_LEN], SaeError> {
    let x = FieldElement::from_bytes(&element.x());
    let coordinates = x.and_then(|x| {
        curve_rhs(&x).sqrt().map(|y| {
            let y = FieldElement::conditional_select(
                &y.neg(),
                &y,
                element.y_is_odd().ct_eq(&y.is_odd()),
            );
            let mut bytes = [0; 2 * SAE_PRIME_LEN];
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
    Option::<[u8; 2 * SAE_PRIME_LEN]>::from(coordinates).ok_or(SaeError::Crypto)
}

/// One peer's commit: its scalar and element.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SaeCommitValues {
    pub scalar: [u8; SAE_PRIME_LEN],
    pub element: [u8; 2 * SAE_PRIME_LEN],
}

#[deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
impl SaeCommitValues {
    /// Parse the access point's group-19 commit body. Hunting and pecking
    /// admits nothing after the element. H2E admits a Rejected Groups
    /// element that does not reject group 19, as the vendor's
    /// `wpa3_check_sae_rejected_groups` does; a Password Identifier, which
    /// this station never sends, is refused.
    pub fn parse(body: &[u8], h2e: bool) -> Result<Self, SaeError> {
        let Some((&[group_low, group_high], rest)) = body.split_first_chunk::<2>() else {
            return Err(SaeError::Malformed);
        };
        let group = u16::from_le_bytes([group_low, group_high]);
        if group != SAE_GROUP_P256 {
            return Err(SaeError::UnsupportedGroup(group));
        }
        let Some((&scalar, rest)) = rest.split_first_chunk::<SAE_PRIME_LEN>() else {
            return Err(SaeError::Malformed);
        };
        let Some((&element, mut rest)) = rest.split_first_chunk::<{ 2 * SAE_PRIME_LEN }>() else {
            return Err(SaeError::Malformed);
        };
        while !rest.is_empty() {
            if !h2e {
                return Err(SaeError::Malformed);
            }
            let [EXTENSION_ELEMENT_ID, length, extension, ..] = *rest else {
                return Err(SaeError::Malformed);
            };
            let Some((whole, tail)) = rest.split_at_checked(
                usize::from(length)
                    .checked_add(2)
                    .ok_or(SaeError::Malformed)?,
            ) else {
                return Err(SaeError::Malformed);
            };
            let [_, _, _, value @ ..] = whole else {
                return Err(SaeError::Malformed);
            };
            match extension {
                REJECTED_GROUPS_ELEMENT
                    if value.len().is_multiple_of(2) && !contains_offered_group(value) => {}
                _ => return Err(SaeError::Malformed),
            }
            rest = tail;
        }
        Ok(Self { scalar, element })
    }

    /// This station's commit body: group, the anti-clogging token of hunting
    /// and pecking, scalar and element, then the token container of H2E, as
    /// the vendor's `sae_write_commit` orders them. Returns the length.
    pub fn encode(
        &self,
        token: Option<&[u8]>,
        h2e: bool,
        output: &mut [u8],
    ) -> Result<usize, SaeError> {
        let token = token.unwrap_or(&[]);
        let container_length = if h2e && !token.is_empty() {
            Some(
                u8::try_from(token.len())
                    .ok()
                    .and_then(|length| length.checked_add(1))
                    .ok_or(SaeError::Malformed)?,
            )
        } else {
            None
        };
        let length = SAE_COMMIT_LEN
            .checked_add(token.len())
            .and_then(|length| length.checked_add(if container_length.is_some() { 3 } else { 0 }))
            .ok_or(SaeError::Malformed)?;
        let mut output = output.get_mut(..length).ok_or(SaeError::Malformed)?;
        put(&mut output, &SAE_GROUP_P256.to_le_bytes())?;
        if !h2e {
            put(&mut output, token)?;
        }
        put(&mut output, &self.scalar)?;
        put(&mut output, &self.element)?;
        if let Some(container_length) = container_length {
            put(
                &mut output,
                &[
                    EXTENSION_ELEMENT_ID,
                    container_length,
                    ANTI_CLOGGING_TOKEN_ELEMENT,
                ],
            )?;
            put(&mut output, token)?;
        }
        Ok(length)
    }
}

/// Write `bytes` at the front of `output` and advance past them.
fn put(output: &mut &mut [u8], bytes: &[u8]) -> Result<(), SaeError> {
    let (front, rest) = core::mem::take(output)
        .split_at_mut_checked(bytes.len())
        .ok_or(SaeError::Malformed)?;
    for (target, source) in front.iter_mut().zip(bytes) {
        *target = *source;
    }
    *output = rest;
    Ok(())
}

/// Why an access point refuses a station's commit before any computation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaeCommitRefusal {
    /// A group other than 19: status 77.
    UnsupportedGroup,
    /// A truncated body, a scalar outside 1 < scalar < r, an element off the
    /// curve or a malformed trailing element: status 1.
    Unspecified,
}

/// A station's commit as the access point receives it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SaeReceivedCommit<'a> {
    pub values: SaeCommitValues,
    /// The anti-clogging token the station echoed.
    pub token: Option<&'a [u8]>,
    /// The station named a password identifier.
    pub password_identifier: bool,
    /// The Rejected Groups payload of an H2E commit: little-endian groups.
    pub rejected_groups: Option<&'a [u8]>,
}

#[deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
impl<'a> SaeReceivedCommit<'a> {
    /// Parse a station's group-19 commit as the vendor's `sae_parse_commit`
    /// does: under hunting and pecking, octets between the group and the
    /// scalar and element are the anti-clogging token when at least 32;
    /// under H2E, a Password Identifier, a Rejected Groups element and an
    /// Anti-Clogging Token Container may follow the element. The scalar must
    /// satisfy 1 < scalar < r and the element must be a curve point.
    pub fn parse(body: &'a [u8], h2e: bool) -> Result<Self, SaeCommitRefusal> {
        let [group_low, group_high, ref rest @ ..] = *body else {
            return Err(SaeCommitRefusal::Unspecified);
        };
        if u16::from_le_bytes([group_low, group_high]) != SAE_GROUP_P256 {
            return Err(SaeCommitRefusal::UnsupportedGroup);
        }
        let values_len = 3 * SAE_PRIME_LEN;
        let mut token = None;
        let mut rest = rest;
        if !h2e
            && let Some(extra) = rest.len().checked_sub(values_len)
            && extra > 0
            && extra >= SAE_ANTI_CLOGGING_TOKEN_LEN
            && let Some((echoed, tail)) = rest.split_at_checked(extra)
        {
            token = Some(echoed);
            rest = tail;
        }
        let Some((&scalar, rest)) = rest.split_first_chunk::<SAE_PRIME_LEN>() else {
            return Err(SaeCommitRefusal::Unspecified);
        };
        let Some((&element, mut rest)) = rest.split_first_chunk::<{ 2 * SAE_PRIME_LEN }>() else {
            return Err(SaeCommitRefusal::Unspecified);
        };
        let values = SaeCommitValues { scalar, element };
        let scalar = scalar_from(&values.scalar).ok_or(SaeCommitRefusal::Unspecified)?;
        if bool::from(scalar.is_zero() | scalar.ct_eq(&Scalar::ONE)) {
            return Err(SaeCommitRefusal::Unspecified);
        }
        if peer_element(&values.element).is_none() {
            return Err(SaeCommitRefusal::Unspecified);
        }
        let mut commit = Self {
            values,
            token,
            password_identifier: false,
            rejected_groups: None,
        };
        if let Some((_, tail)) = extension_element(rest, PASSWORD_IDENTIFIER_ELEMENT, 1) {
            commit.password_identifier = true;
            rest = tail;
        }
        if h2e {
            if let Some((value, tail)) = extension_element(rest, REJECTED_GROUPS_ELEMENT, 2) {
                if !value.len().is_multiple_of(2) {
                    return Err(SaeCommitRefusal::Unspecified);
                }
                commit.rejected_groups = Some(value);
                rest = tail;
            }
            if let Some((value, _)) = extension_element(rest, ANTI_CLOGGING_TOKEN_ELEMENT, 1) {
                commit.token = Some(value);
            }
        }
        Ok(commit)
    }

    /// Whether the station rejected group 19, the only group this access
    /// point offers.
    pub fn rejects_offered_group(&self) -> bool {
        self.rejected_groups.is_some_and(contains_offered_group)
    }
}

/// Seconds an access point keeps one comeback-token key.
const COMEBACK_KEY_LIFETIME: Duration = Duration::from_secs(60);

/// The anti-clogging tokens an access point issues and checks.
///
/// A token is a big-endian index followed by the tail of
/// HMAC-SHA256(key, station address || index); a table indexed by the first
/// octet of HMAC-SHA256(key, station address) holds each station's pending
/// index, and a checked token invalidates it. The key is redrawn when a token
/// is issued more than 60 seconds after the last draw. The vendor passes its
/// index counter by value, so its counter never advances and every newly
/// pending index is 1.
///
/// SOURCE: ESP-IDF `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
/// `components/wpa_supplicant/src/ap/comeback_token.c`
/// (`auth_build_token_req`, `check_comeback_token`, `comeback_token_hash`).
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct SaeComebackTokens {
    key: [u8; 32],
    #[zeroize(skip)]
    key_drawn: Option<Instant>,
    pending: [u16; 256],
}

#[deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
impl SaeComebackTokens {
    pub const fn new() -> Self {
        Self {
            key: [0; 32],
            key_drawn: None,
            pending: [0; 256],
        }
    }

    /// The token requested from `station`, drawing a fresh key with
    /// `draw_key` when the current one is missing or older than 60 seconds.
    pub fn issue(
        &mut self,
        station: [u8; 6],
        now: Instant,
        draw_key: impl FnOnce() -> [u8; 32],
    ) -> [u8; SAE_ANTI_CLOGGING_TOKEN_LEN] {
        let expired = self
            .key_drawn
            .is_none_or(|drawn| now.saturating_duration_since(drawn) > COMEBACK_KEY_LIFETIME);
        if expired {
            self.key = draw_key();
            self.key_drawn = Some(now);
            self.pending = [0; 256];
        }
        let (slot, pending) = self.pending_index(station);
        let pending = u16::conditional_select(&pending, &1, pending.ct_eq(&0));
        self.set_pending(slot, pending);
        let index = pending.to_be_bytes();
        let mut token = hmac_sha256(&self.key, &[&station, &index]);
        for (target, source) in token.iter_mut().zip(index) {
            *target = source;
        }
        token
    }

    /// `station`'s pending index, in the slot of the first octet of
    /// HMAC-SHA256(key, station address).
    fn pending_index(&self, station: [u8; 6]) -> (u8, u16) {
        let [first, ..] = hmac_sha256(&self.key, &[&station]);
        let pending = self
            .pending
            .iter()
            .zip(0_u8..=u8::MAX)
            .fold(0, |value, (pending, slot)| {
                u16::conditional_select(&value, pending, slot.ct_eq(&first))
            });
        (first, pending)
    }

    fn set_pending(&mut self, selected: u8, value: u16) {
        for (pending, slot) in self.pending.iter_mut().zip(0_u8..=u8::MAX) {
            pending.conditional_assign(&value, slot.ct_eq(&selected));
        }
    }

    /// Whether `token` is the one pending for `station`; a valid token is
    /// consumed.
    pub fn check(&mut self, station: [u8; 6], token: &[u8]) -> bool {
        let Some((index_bytes, tail)) = token.split_first_chunk::<2>() else {
            return false;
        };
        if token.len() != SAE_ANTI_CLOGGING_TOKEN_LEN {
            return false;
        }
        let mac = hmac_sha256(&self.key, &[&station, index_bytes]);
        let [_, _, mac_tail @ ..] = &mac;
        let (slot, pending) = self.pending_index(station);
        if pending == 0 || pending.to_be_bytes() != *index_bytes {
            return false;
        }
        if !bool::from(mac_tail.ct_eq(tail)) {
            return false;
        }
        self.set_pending(slot, 0);
        true
    }
}

impl Default for SaeComebackTokens {
    fn default() -> Self {
        Self::new()
    }
}

/// The extension element `extension` at the start of `bytes`, whose length
/// field is at least `minimum_length`, and the bytes after it; `None` when
/// another element or nothing starts there.
#[deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
fn extension_element(bytes: &[u8], extension: u8, minimum_length: u8) -> Option<(&[u8], &[u8])> {
    let [EXTENSION_ELEMENT_ID, length, id, ..] = *bytes else {
        return None;
    };
    if id != extension || length < minimum_length {
        return None;
    }
    let (element, tail) = bytes.split_at_checked(usize::from(length).checked_add(2)?)?;
    let [_, _, _, value @ ..] = element else {
        return None;
    };
    Some((value, tail))
}

#[deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
fn peer_element(element: &[u8; 2 * SAE_PRIME_LEN]) -> Option<AffinePoint> {
    let (x, y) = element.split_first_chunk::<SAE_PRIME_LEN>()?;
    let y: &[u8; SAE_PRIME_LEN] = y.try_into().ok()?;
    let x = FieldElement::from_bytes(&FieldBytes::from(*x));
    let y = FieldElement::from_bytes(&FieldBytes::from(*y));
    Option::<ProjectivePoint>::from(x.and_then(|x| y.and_then(|y| point(&x, &y))))
        .map(|point| point.to_affine())
}

/// The anti-clogging token of a commit refused with status 76: after the
/// group under hunting and pecking, inside its container element under H2E.
#[deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
pub fn anti_clogging_token(body: &[u8], h2e: bool) -> Result<&[u8], SaeError> {
    let rest = body.get(2..).ok_or(SaeError::Malformed)?;
    if !h2e {
        return Ok(rest);
    }
    let [
        EXTENSION_ELEMENT_ID,
        length,
        ANTI_CLOGGING_TOKEN_ELEMENT,
        ..,
    ] = *rest
    else {
        return Err(SaeError::Malformed);
    };
    if length == 0 {
        return Err(SaeError::Malformed);
    }
    let Some((element, _)) = rest.split_at_checked(
        usize::from(length)
            .checked_add(2)
            .ok_or(SaeError::Malformed)?,
    ) else {
        return Err(SaeError::Malformed);
    };
    let [_, _, _, token @ ..] = element else {
        return Err(SaeError::Malformed);
    };
    Ok(token)
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
    pmkid: [u8; SAE_PMKID_LEN],
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
    #[deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )]
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
        let mut keys = [0_u8; 2 * SAE_KEY_LEN];
        kdf_sha256(&keyseed, b"SAE KCK and PMK", &context, &mut keys);
        keyseed.zeroize();
        let mut derived = SaeKeys {
            kck: [0; SAE_KEY_LEN],
            pmk: [0; SAE_KEY_LEN],
            pmkid: [0; SAE_PMKID_LEN],
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
    pub const fn pmkid(&self) -> &[u8; SAE_PMKID_LEN] {
        &self.pmkid
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
                &send_confirm.to_le_bytes(),
                &first.scalar,
                &first.element,
                &second.scalar,
                &second.element,
            ],
        )
    }

    /// This station's confirm body.
    pub fn own_confirm(&self, send_confirm: u16) -> [u8; SAE_CONFIRM_LEN] {
        let mut body = [0; SAE_CONFIRM_LEN];
        for (target, source) in body.iter_mut().zip(
            send_confirm
                .to_le_bytes()
                .iter()
                .chain(self.confirm(send_confirm, &self.own, &self.peer).iter()),
        ) {
            *target = *source;
        }
        body
    }

    /// Verify the peer's confirm body and return its send-confirm.
    #[deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )]
    pub fn verify_peer_confirm(&self, body: &[u8]) -> Result<u16, SaeError> {
        if body.len() != SAE_CONFIRM_LEN {
            return Err(SaeError::Malformed);
        }
        let Some((&[low, high], confirm)) = body.split_first_chunk::<2>() else {
            return Err(SaeError::Malformed);
        };
        let send_confirm = u16::from_le_bytes([low, high]);
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

impl core::fmt::Debug for SaeComebackTokens {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("SaeComebackTokens(<redacted>)")
    }
}

fn contains_offered_group(mut groups: &[u8]) -> bool {
    while let Some((group, rest)) = groups.split_first_chunk::<2>() {
        if *group == SAE_GROUP_P256.to_le_bytes() {
            return true;
        }
        groups = rest;
    }
    false
}
