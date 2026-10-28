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
#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]

/// Element ID of the RSN element.
pub const RSN_ELEMENT_ID: u8 = 48;
/// The only RSN element version defined by IEEE 802.11.
pub const RSN_VERSION: u16 = 1;
/// OUI of the IEEE 802.11 cipher and AKM suite selectors.
pub const RSN_OUI: [u8; 3] = [0x00, 0x0f, 0xac];
/// Cipher suite type of CCMP-128.
pub const RSN_CIPHER_CCMP: u8 = 4;
/// AKM suite type of PSK with SHA-1 key derivation.
pub const RSN_AKM_PSK: u8 = 2;
/// `00-0F-AC:6`: PSK with SHA-256 key derivation.
pub const RSN_AKM_PSK_SHA256: u8 = 6;
/// `00-0F-AC:8`: SAE.
pub const RSN_AKM_SAE: u8 = 8;
/// `00-0F-AC:6`: BIP-CMAC-128, the default group management cipher.
pub const RSN_CIPHER_BIP_CMAC_128: u8 = 6;
/// RSN Capabilities: management frame protection required.
pub const RSN_CAPABILITY_MFPR: u16 = u16::from_le_bytes(
    RsnCapabilities::from_bytes([0; 2])
        .with_mfpr(true)
        .to_bytes(),
);
/// RSN Capabilities: management frame protection capable.
pub const RSN_CAPABILITY_MFPC: u16 = u16::from_le_bytes(
    RsnCapabilities::from_bytes([0; 2])
        .with_mfpc(true)
        .to_bytes(),
);
/// RSN Capabilities: signaling-and-payload-protected A-MSDU capable.
pub const RSN_CAPABILITY_SPP_AMSDU_CAPABLE: u16 = u16::from_le_bytes(
    RsnCapabilities::from_bytes([0; 2])
        .with_spp_capable(true)
        .to_bytes(),
);
/// SPP A-MSDU Required; valid only together with SPP A-MSDU Capable.
pub const RSN_CAPABILITY_SPP_AMSDU_REQUIRED: u16 = u16::from_le_bytes(
    RsnCapabilities::from_bytes([0; 2])
        .with_spp_required(true)
        .to_bytes(),
);
/// Length of one PMKID.
pub const RSN_PMKID_LEN: usize = 16;

const SUITE_LEN: usize = 4;

/// The suite selector for `suite_type` under the IEEE 802.11 OUI.
pub const fn ieee_suite(suite_type: u8) -> [u8; 4] {
    [RSN_OUI[0], RSN_OUI[1], RSN_OUI[2], suite_type]
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
    pub const fn from_suite_selector(selector: [u8; 4]) -> Option<Self> {
        match selector {
            [0x00, 0x0f, 0xac, RSN_AKM_PSK] => Some(Self::Psk),
            [0x00, 0x0f, 0xac, RSN_AKM_PSK_SHA256] => Some(Self::PskSha256),
            [0x00, 0x0f, 0xac, RSN_AKM_SAE] => Some(Self::Sae),
            _ => None,
        }
    }

    pub const fn suite_selector(self) -> [u8; 4] {
        ieee_suite(match self {
            Self::Psk => RSN_AKM_PSK,
            Self::PskSha256 => RSN_AKM_PSK_SHA256,
            Self::Sae => RSN_AKM_SAE,
        })
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
pub struct RsnSuiteList<'a> {
    suites: CountedArrays<'a, SUITE_LEN>,
}

impl<'a> RsnSuiteList<'a> {
    pub const fn len(&self) -> usize {
        self.suites.bytes.len() / SUITE_LEN
    }

    pub const fn is_empty(&self) -> bool {
        self.suites.bytes.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = [u8; 4]> + 'a {
        self.suites.iter()
    }

    pub fn contains(&self, suite: [u8; 4]) -> bool {
        self.iter().any(|candidate| candidate == suite)
    }
}

/// A syntactically complete version-1 RSN element.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RsnElement<'a> {
    group_data_cipher: [u8; 4],
    pairwise_ciphers: RsnSuiteList<'a>,
    akm_suites: RsnSuiteList<'a>,
    capabilities: Option<u16>,
    pmkids: Option<CountedArrays<'a, RSN_PMKID_LEN>>,
    group_management_cipher: Option<[u8; 4]>,
}

impl<'a> RsnElement<'a> {
    /// Parse one complete element, including its ID and length octets.
    ///
    /// The fields after the AKM list are optional only as a trailing
    /// sequence; every field that is started must be complete, and no bytes
    /// may follow the Group Management Cipher Suite.
    pub fn parse(element: &'a [u8]) -> Result<Self, RsnSyntaxError> {
        let [RSN_ELEMENT_ID, length, body @ ..] = element else {
            return Err(RsnSyntaxError::Malformed);
        };
        if usize::from(*length) != body.len() {
            return Err(RsnSyntaxError::Malformed);
        }

        let mut reader = RsnReader(oer_wire::Reader::new(body));
        if reader.u16()? != RSN_VERSION {
            return Err(RsnSyntaxError::UnsupportedVersion);
        }
        let group_data_cipher = reader.suite()?;
        let pairwise_ciphers = reader.suite_list()?;
        let akm_suites = reader.suite_list()?;
        let capabilities = reader.optional(RsnReader::u16)?;
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

    pub const fn group_data_cipher(&self) -> [u8; 4] {
        self.group_data_cipher
    }

    pub const fn pairwise_ciphers(&self) -> RsnSuiteList<'a> {
        self.pairwise_ciphers
    }

    pub const fn akm_suites(&self) -> RsnSuiteList<'a> {
        self.akm_suites
    }

    /// RSN Capabilities, or `None` when the element ends after the AKM list.
    pub const fn capabilities(&self) -> Option<u16> {
        self.capabilities
    }

    /// Number of PMKIDs, or `None` when the PMKID Count field is absent.
    pub const fn pmkid_count(&self) -> Option<u16> {
        match self.pmkids {
            // Parsing took exactly the u16 count carried by the element.
            Some(pmkids) => Some((pmkids.bytes.len() / RSN_PMKID_LEN) as u16),
            None => None,
        }
    }

    /// The listed PMKIDs, in order.
    pub fn pmkids(&self) -> impl Iterator<Item = [u8; RSN_PMKID_LEN]> + use<'a> {
        self.pmkids.unwrap_or(CountedArrays { bytes: &[] }).iter()
    }

    pub const fn group_management_cipher(&self) -> Option<[u8; 4]> {
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

struct RsnReader<'a>(oer_wire::Reader<'a>);

impl<'a> RsnReader<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], RsnSyntaxError> {
        self.0.take(length).map_err(|_| RsnSyntaxError::Malformed)
    }
    fn u16(&mut self) -> Result<u16, RsnSyntaxError> {
        self.0.u16_le().map_err(|_| RsnSyntaxError::Malformed)
    }
    fn suite(&mut self) -> Result<[u8; 4], RsnSyntaxError> {
        self.0.read().map_err(|_| RsnSyntaxError::Malformed)
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

    fn suite_list(&mut self) -> Result<RsnSuiteList<'a>, RsnSyntaxError> {
        Ok(RsnSuiteList {
            suites: self.counted_arrays::<SUITE_LEN>()?,
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

/// Element ID of the RSN Extension element (RSNXE).
///
/// SOURCE: IEEE Std 802.11-2020, 9.4.2.241 and Table 9-92 (Element IDs).
pub const RSNXE_ELEMENT_ID: u8 = 244;

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
/// and `capabilities`, each suite under the IEEE 802.11 OUI.
///
/// SOURCE: IEEE Std 802.11-2020, 9.4.2.24.1 (RSNE format: Element ID,
/// Length, Version, Group Data Cipher Suite, Pairwise Cipher Suite Count and
/// List, AKM Suite Count and List, RSN Capabilities).
pub const fn rsn_single_suite_element(
    group_data_cipher: u8,
    pairwise_cipher: u8,
    akm: u8,
    capabilities: u16,
) -> [u8; RSN_SINGLE_SUITE_ELEMENT_LEN] {
    const BODY_LEN: u8 = (RSN_SINGLE_SUITE_ELEMENT_LEN - 2) as u8;
    let [v0, v1] = RSN_VERSION.to_le_bytes();
    let [o0, o1, o2] = RSN_OUI;
    let [n0, n1] = 1_u16.to_le_bytes();
    let [c0, c1] = capabilities.to_le_bytes();
    [
        RSN_ELEMENT_ID,
        BODY_LEN,
        v0,
        v1,
        o0,
        o1,
        o2,
        group_data_cipher,
        n0,
        n1,
        o0,
        o1,
        o2,
        pairwise_cipher,
        n0,
        n1,
        o0,
        o1,
        o2,
        akm,
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
    group_data_cipher: u8,
    pairwise_cipher: u8,
    akm: u8,
    capabilities: u16,
    group_management_cipher: u8,
) -> [u8; RSN_GROUP_MANAGEMENT_ELEMENT_LEN] {
    const BODY_LEN: u8 = (RSN_GROUP_MANAGEMENT_ELEMENT_LEN - 2) as u8;
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
    let [o0, o1, o2] = RSN_OUI;
    let [k0, k1] = 0_u16.to_le_bytes();
    [
        id,
        BODY_LEN,
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
        k0,
        k1,
        o0,
        o1,
        o2,
        group_management_cipher,
    ]
}

/// An RSNXE carrying one Extended RSN Capabilities octet whose Field Length
/// subfield is zero.
///
/// SOURCE: IEEE Std 802.11-2020, 9.4.2.241 (RSN Extension element: the
/// low four bits of the first octet are the field length minus one).
pub const fn rsnx_element(capabilities: RsnExtendedCapabilities) -> [u8; RSNXE_ONE_OCTET_LEN] {
    const BODY_LEN: u8 = (RSNXE_ONE_OCTET_LEN - 2) as u8;
    let [octet] = capabilities.to_bytes();
    [RSNXE_ELEMENT_ID, BODY_LEN, octet]
}

/// The Extended RSN Capabilities of an RSNXE, or `None` for an element
/// shorter than its first capability octet.
pub fn rsnx_capabilities(element: &[u8]) -> Option<RsnExtendedCapabilities> {
    let mut reader = oer_wire::Reader::new(element);
    let [_, _, octet] = *reader.array::<RSNXE_ONE_OCTET_LEN>().ok()?;
    Some(RsnExtendedCapabilities::from_bytes([octet]))
}

oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.4.2.24 (RSN Capabilities field).
    pub struct RsnCapabilities[2] (little) {
        pub mfpr, with_mfpr: bool = 6..7;
        pub mfpc, with_mfpc: bool = 7..8;
        pub spp_capable, with_spp_capable: bool = 10..11;
        pub spp_required, with_spp_required: bool = 11..12;
    }
}
oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.4.2.241 (RSN Extension element).
    pub struct RsnExtendedCapabilities[1] (little) {
        pub sae_h2e, with_sae_h2e: bool = 5..6;
    }
}

#[cfg(test)]
mod tests;
