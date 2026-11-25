//! RSN element wire syntax (IEEE 802.11-2020 9.4.2.24).
//!
//! [`RsnElement::parse`] validates the complete version-1 field sequence and
//! returns borrowed views of it. It applies no suite or capability policy:
//! station candidate selection and the RSN authenticator/supplicant apply
//! their own acceptance rules to the same parsed element.
//! Counted suite and PMKID lists retain complete byte spans after parsing.
//! Their private construction proves an exact multiple of the entry width;
//! iteration reads fixed arrays and cannot omit a partial entry.
//!
//! Received bytes reach this module; nothing here panics on them.
use crate::element::ElementId;
use crate::security::Pmkid;

/// The only RSN element version defined by IEEE 802.11.
pub const RSN_VERSION: u16 = 1;
/// OUI of the IEEE 802.11 cipher and AKM suite selectors.
pub const RSN_OUI: [u8; 3] = [0x00, 0x0f, 0xac];
/// Length of one PMKID.
pub const RSN_PMKID_LEN: usize = 16;

const SUITE_LEN: usize = 4;

macro_rules! suite_selector {
    (
        $(#[$meta:meta])*
        $name:ident {
            $($(#[$value_meta:meta])* $value:ident = $number:literal),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        ///
        /// Octets are no selector until they are named one:
        ///
        /// ```compile_fail,E0308
        #[doc = concat!("use oer_protocols_ieee80211_wire::security::rsn::", stringify!($name), ";")]
        #[doc = concat!("let suite: ", stringify!($name), " = [0x00, 0x0f, 0xac, 4];")]
        /// ```
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        #[repr(transparent)]
        pub struct $name([u8; SUITE_LEN]);

        impl $name {
            $($(#[$value_meta])* pub const $value: Self = Self::ieee($number);)+

            /// The selector of `suite_type` under the IEEE 802.11 OUI.
            const fn ieee(suite_type: u8) -> Self {
                let [o0, o1, o2] = RSN_OUI;
                Self([o0, o1, o2, suite_type])
            }

            /// The selector of `octets`: an OUI and its suite type.
            pub const fn from_octets(octets: [u8; SUITE_LEN]) -> Self {
                Self(octets)
            }

            /// Its octets.
            pub const fn octets(self) -> [u8; SUITE_LEN] {
                self.0
            }
        }
    };
}

suite_selector! {
    /// A cipher suite selector: any octets, some of them named.
    ///
    /// SOURCE: IEEE Std 802.11-2020, 9.4.2.24.2 (Cipher suites).
    CipherSuite {
        /// CCMP-128.
        CCMP_128 = 4,
        /// BIP-CMAC-128, the default group management cipher.
        BIP_CMAC_128 = 6,
    }
}

suite_selector! {
    /// An AKM suite selector: any octets, some of them named.
    ///
    /// SOURCE: IEEE Std 802.11-2020, 9.4.2.24.3 (AKM suites).
    AkmSuite {
        /// PSK with SHA-1 key derivation.
        PSK = 2,
        /// PSK with SHA-256 key derivation.
        PSK_SHA256 = 6,
        /// SAE.
        SAE = 8,
    }
}

/// Authentication and key management suite named by an RSN element.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Akm {
    /// `00-0F-AC:2`, PSK with PRF-SHA1 key expansion and HMAC-SHA1-128 MIC
    /// (key descriptor version 2).
    Psk,
    /// `00-0F-AC:6`, PSK with the SHA-256 key derivation function and
    /// AES-128-CMAC MIC (key descriptor version 3), as protected management
    /// frames select.
    PskSha256,
    /// `00-0F-AC:8`, SAE: the PMK comes from the SAE exchange; the SHA-256
    /// key derivation function and AES-128-CMAC MIC under the AKM-defined
    /// key descriptor version 0.
    Sae,
}

impl Akm {
    /// The suite named by an RSN element AKM selector, if supported.
    pub const fn from_suite_selector(selector: AkmSuite) -> Option<Self> {
        match selector {
            AkmSuite::PSK => Some(Self::Psk),
            AkmSuite::PSK_SHA256 => Some(Self::PskSha256),
            AkmSuite::SAE => Some(Self::Sae),
            _ => None,
        }
    }

    pub const fn suite_selector(self) -> AkmSuite {
        match self {
            Self::Psk => AkmSuite::PSK,
            Self::PskSha256 => AkmSuite::PSK_SHA256,
            Self::Sae => AkmSuite::SAE,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnSyntaxError {
    /// The header, a counted list or an optional field is incomplete, or
    /// bytes remain after the last defined field.
    Malformed,
    /// The element is well framed but not version 1, so its body layout is
    /// unknown.
    UnsupportedVersion,
}

/// A counted list of four-byte suite selectors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RsnSuiteList<'a, S> {
    suites: CountedArrays<'a, SUITE_LEN>,
    kind: core::marker::PhantomData<S>,
}

impl<'a, S> RsnSuiteList<'a, S> {
    pub const fn len(&self) -> usize {
        self.suites.bytes.len() / SUITE_LEN
    }

    pub const fn is_empty(&self) -> bool {
        self.suites.bytes.is_empty()
    }
}

macro_rules! suite_list {
    ($name:ident) => {
        impl<'a> RsnSuiteList<'a, $name> {
            pub fn iter(&self) -> impl Iterator<Item = $name> + 'a {
                self.suites.iter().map($name::from_octets)
            }

            pub fn contains(&self, suite: $name) -> bool {
                self.iter().any(|candidate| candidate == suite)
            }
        }
    };
}

suite_list!(CipherSuite);
suite_list!(AkmSuite);

/// A syntactically complete version-1 RSN element.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RsnElement<'a> {
    group_data_cipher: CipherSuite,
    pairwise_ciphers: RsnSuiteList<'a, CipherSuite>,
    akm_suites: RsnSuiteList<'a, AkmSuite>,
    capabilities: Option<RsnCapabilities>,
    pmkids: Option<CountedArrays<'a, RSN_PMKID_LEN>>,
    group_management_cipher: Option<CipherSuite>,
}

impl<'a> RsnElement<'a> {
    /// Parse one complete element, including its ID and length octets.
    ///
    /// The fields after the AKM list are optional only as a trailing
    /// sequence; every field that is started must be complete, and no bytes
    /// may follow the Group Management Cipher Suite.
    pub fn parse(element: &'a [u8]) -> Result<Self, RsnSyntaxError> {
        let [id, length, body @ ..] = element else {
            return Err(RsnSyntaxError::Malformed);
        };
        if ElementId::new(*id) != ElementId::RSN {
            return Err(RsnSyntaxError::Malformed);
        }
        if usize::from(*length) != body.len() {
            return Err(RsnSyntaxError::Malformed);
        }

        let mut reader = RsnReader(oer_portable_wire::Reader::new(body));
        if reader.u16()? != RSN_VERSION {
            return Err(RsnSyntaxError::UnsupportedVersion);
        }
        let group_data_cipher = reader.suite()?;
        let pairwise_ciphers = reader.suite_list()?;
        let akm_suites = reader.suite_list()?;
        let capabilities = reader.optional(RsnReader::capabilities)?;
        let pmkids = reader.optional(|reader| reader.counted_arrays::<RSN_PMKID_LEN>())?;
        let group_management_cipher = reader.optional(RsnReader::suite)?;
        if !reader.0.is_empty() {
            return Err(RsnSyntaxError::Malformed);
        }

        Ok(Self {
            group_data_cipher,
            pairwise_ciphers,
            akm_suites,
            capabilities,
            pmkids,
            group_management_cipher,
        })
    }

    pub const fn group_data_cipher(&self) -> CipherSuite {
        self.group_data_cipher
    }

    pub const fn pairwise_ciphers(&self) -> RsnSuiteList<'a, CipherSuite> {
        self.pairwise_ciphers
    }

    pub const fn akm_suites(&self) -> RsnSuiteList<'a, AkmSuite> {
        self.akm_suites
    }

    /// RSN Capabilities, or `None` when the element ends after the AKM list.
    pub const fn capabilities(&self) -> Option<RsnCapabilities> {
        self.capabilities
    }

    /// Number of PMKIDs, or `None` when the PMKID Count field is absent.
    pub const fn pmkid_count(&self) -> Option<usize> {
        match self.pmkids {
            // Parsing took exactly the count carried by the element.
            Some(pmkids) => Some(pmkids.bytes.len() / RSN_PMKID_LEN),
            None => None,
        }
    }

    /// The listed PMKIDs, in order.
    pub fn pmkids(&self) -> impl Iterator<Item = Pmkid> + use<'a> {
        self.pmkids
            .unwrap_or(CountedArrays { bytes: &[] })
            .iter()
            .map(Pmkid::from_octets)
    }

    pub const fn group_management_cipher(&self) -> Option<CipherSuite> {
        self.group_management_cipher
    }
}

/// A complete counted span. Only RsnReader::counted_arrays constructs it,
/// after admitting exactly count * N bytes. The representation retains one
/// borrowed slice; no count, allocation or partial entry is stored.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CountedArrays<'a, const N: usize> {
    bytes: &'a [u8],
}

impl<'a, const N: usize> CountedArrays<'a, N> {
    fn iter(self) -> impl Iterator<Item = [u8; N]> + use<'a, N> {
        let mut bytes = self.bytes;
        core::iter::from_fn(move || {
            // Construction admitted complete entries and N > 0. Thus None
            // means the list is exhausted, never that an entry was dropped.
            let (entry, rest) = bytes.split_first_chunk::<N>()?;
            bytes = rest;
            Some(*entry)
        })
    }
}

struct RsnReader<'a>(oer_portable_wire::Reader<'a>);

impl<'a> RsnReader<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], RsnSyntaxError> {
        self.0.take(length).map_err(|_| RsnSyntaxError::Malformed)
    }
    fn u16(&mut self) -> Result<u16, RsnSyntaxError> {
        self.0.u16_le().map_err(|_| RsnSyntaxError::Malformed)
    }
    fn capabilities(&mut self) -> Result<RsnCapabilities, RsnSyntaxError> {
        self.0
            .read()
            .map(RsnCapabilities::from_bytes)
            .map_err(|_| RsnSyntaxError::Malformed)
    }
    fn suite(&mut self) -> Result<CipherSuite, RsnSyntaxError> {
        self.0
            .read()
            .map(CipherSuite::from_octets)
            .map_err(|_| RsnSyntaxError::Malformed)
    }

    fn counted_arrays<const N: usize>(&mut self) -> Result<CountedArrays<'a, N>, RsnSyntaxError> {
        const {
            if N == 0 {
                panic!("a counted entry must be nonempty");
            }
        }
        let count = usize::from(self.u16()?);
        let length = count.checked_mul(N).ok_or(RsnSyntaxError::Malformed)?;
        Ok(CountedArrays {
            bytes: self.take(length)?,
        })
    }

    fn suite_list<S>(&mut self) -> Result<RsnSuiteList<'a, S>, RsnSyntaxError> {
        Ok(RsnSuiteList {
            suites: self.counted_arrays::<SUITE_LEN>()?,
            kind: core::marker::PhantomData,
        })
    }

    fn optional<T>(
        &mut self,
        read: impl FnOnce(&mut Self) -> Result<T, RsnSyntaxError>,
    ) -> Result<Option<T>, RsnSyntaxError> {
        if self.0.is_empty() {
            Ok(None)
        } else {
            read(self).map(Some)
        }
    }
}

/// Length of an RSN element naming one group data cipher, one pairwise
/// cipher, one AKM suite and the RSN Capabilities, without the optional
/// PMKID list and group management cipher.
pub const RSN_SINGLE_SUITE_ELEMENT_LEN: usize = 22;
/// [`RSN_SINGLE_SUITE_ELEMENT_LEN`] followed by an empty PMKID list and a
/// group management cipher suite.
pub const RSN_GROUP_MANAGEMENT_ELEMENT_LEN: usize = 28;
/// Length of an RSNXE with one Extended RSN Capabilities octet.
pub const RSNXE_ONE_OCTET_LEN: usize = 3;

/// The RSN element of one group data cipher, one pairwise cipher, one AKM
/// and `capabilities`.
///
/// SOURCE: IEEE Std 802.11-2020, 9.4.2.24.1 (RSNE format: Element ID,
/// Length, Version, Group Data Cipher Suite, Pairwise Cipher Suite Count and
/// List, AKM Suite Count and List, RSN Capabilities).
pub const fn rsn_single_suite_element(
    group_data_cipher: CipherSuite,
    pairwise_cipher: CipherSuite,
    akm: AkmSuite,
    capabilities: RsnCapabilities,
) -> [u8; RSN_SINGLE_SUITE_ELEMENT_LEN] {
    const BODY_LEN: u8 = crate::encoding::octet::<{ RSN_SINGLE_SUITE_ELEMENT_LEN - 2 }>();
    let [v0, v1] = RsnField::of(RSN_VERSION).to_bytes();
    // One suite in each list.
    let [n0, n1] = RsnField::of(1).to_bytes();
    let [c0, c1] = capabilities.to_bytes();
    let [g0, g1, g2, g3] = group_data_cipher.octets();
    let [p0, p1, p2, p3] = pairwise_cipher.octets();
    let [a0, a1, a2, a3] = akm.octets();
    [
        ElementId::RSN.value(),
        BODY_LEN,
        v0,
        v1,
        g0,
        g1,
        g2,
        g3,
        n0,
        n1,
        p0,
        p1,
        p2,
        p3,
        n0,
        n1,
        a0,
        a1,
        a2,
        a3,
        c0,
        c1,
    ]
}

/// [`rsn_single_suite_element`] followed by an empty PMKID list and the
/// group management cipher suite.
///
/// SOURCE: IEEE Std 802.11-2020, 9.4.2.24.1 (PMKID Count, PMKID List and
/// Group Management Cipher Suite follow RSN Capabilities in this order).
pub const fn rsn_group_management_element(
    group_data_cipher: CipherSuite,
    pairwise_cipher: CipherSuite,
    akm: AkmSuite,
    capabilities: RsnCapabilities,
    group_management_cipher: CipherSuite,
) -> [u8; RSN_GROUP_MANAGEMENT_ELEMENT_LEN] {
    const BODY_LEN: u8 = crate::encoding::octet::<{ RSN_GROUP_MANAGEMENT_ELEMENT_LEN - 2 }>();
    let [
        id,
        _,
        v0,
        v1,
        g0,
        g1,
        g2,
        g3,
        p0,
        p1,
        p2,
        p3,
        p4,
        p5,
        a0,
        a1,
        a2,
        a3,
        a4,
        a5,
        c0,
        c1,
    ] = rsn_single_suite_element(group_data_cipher, pairwise_cipher, akm, capabilities);
    let [m0, m1, m2, m3] = group_management_cipher.octets();
    // No PMKID.
    let [k0, k1] = RsnField::of(0).to_bytes();
    [
        id, BODY_LEN, v0, v1, g0, g1, g2, g3, p0, p1, p2, p3, p4, p5, a0, a1, a2, a3, a4, a5, c0,
        c1, k0, k1, m0, m1, m2, m3,
    ]
}

/// An RSNXE carrying one Extended RSN Capabilities octet whose Field Length
/// subfield is zero.
///
/// SOURCE: IEEE Std 802.11-2020, 9.4.2.241 (RSN Extension element: the
/// low four bits of the first octet are the field length minus one).
pub const fn rsnx_element(capabilities: RsnExtendedCapabilities) -> [u8; RSNXE_ONE_OCTET_LEN] {
    const BODY_LEN: u8 = crate::encoding::octet::<{ RSNXE_ONE_OCTET_LEN - 2 }>();
    let [octet] = capabilities.to_bytes();
    [ElementId::RSNX.value(), BODY_LEN, octet]
}

/// The Extended RSN Capabilities of an RSNXE, or `None` for an element
/// shorter than its first capability octet.
pub fn rsnx_capabilities(element: &[u8]) -> Option<RsnExtendedCapabilities> {
    let mut reader = oer_portable_wire::Reader::new(element);
    let [_, _, octet] = *reader.array::<RSNXE_ONE_OCTET_LEN>().ok()?;
    Some(RsnExtendedCapabilities::from_bytes([octet]))
}

oer_portable_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.4.2.24 (RSN Capabilities field).
    pub struct RsnCapabilities[2] (little) {
        pub mfpr, with_mfpr: bool = 6..7;
        pub mfpc, with_mfpc: bool = 7..8;
        pub spp_capable, with_spp_capable: bool = 10..11;
        pub spp_required, with_spp_required: bool = 11..12;
    }
}
oer_portable_wire::bitfield! {
    /// A two-octet little-endian field of an RSNE: the Version, a suite
    /// count or the PMKID Count.
    /// SOURCE: IEEE Std 802.11-2020, 9.4.2.24.1 (RSNE: general, Figure
    /// 9-288).
    pub struct RsnField[2] (little) {
        pub value, with_value: {u16} = 0..16;
    }
}

impl RsnCapabilities {
    /// Every subfield clear.
    pub const NONE: Self = Self::from_bytes([0; 2]);
}

impl RsnField {
    /// The field holding `value`.
    pub const fn of(value: u16) -> Self {
        Self::from_bytes([0; 2]).with_value(value)
    }
}
oer_portable_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.4.2.241 (RSN Extension element).
    pub struct RsnExtendedCapabilities[1] (little) {
        pub sae_h2e, with_sae_h2e: bool = 5..6;
    }
}

#[cfg(test)]
mod tests;
