//! RSN element wire syntax (IEEE 802.11-2020 9.4.2.24).
//!
//! [`RsnElement::parse`] validates the complete version-1 field sequence and
//! returns borrowed views of it. It applies no suite or capability policy:
//! station candidate selection and the RSN authenticator/supplicant apply
//! their own acceptance rules to the same parsed element.
//! Counted suite and PMKID lists retain array slices after parsing: each
//! element's width is proved once, so iteration cannot omit a partial entry.

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
pub const RSN_CAPABILITY_MFPR: u16 = 1 << 6;
/// RSN Capabilities: management frame protection capable.
pub const RSN_CAPABILITY_MFPC: u16 = 1 << 7;
/// RSN Capabilities: signaling-and-payload-protected A-MSDU capable.
pub const RSN_CAPABILITY_SPP_AMSDU_CAPABLE: u16 = 1 << 10;
/// SPP A-MSDU Required; valid only together with SPP A-MSDU Capable.
pub const RSN_CAPABILITY_SPP_AMSDU_REQUIRED: u16 = 1 << 11;
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
    suites: &'a [[u8; SUITE_LEN]],
}

impl<'a> RsnSuiteList<'a> {
    pub const fn len(&self) -> usize {
        self.suites.len()
    }

    pub const fn is_empty(&self) -> bool {
        self.suites.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = [u8; 4]> + 'a {
        self.suites.iter().copied()
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
    pmkids: Option<&'a [[u8; RSN_PMKID_LEN]]>,
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

        let mut reader = Reader { bytes: body };
        if reader.u16()? != RSN_VERSION {
            return Err(RsnSyntaxError::UnsupportedVersion);
        }
        let group_data_cipher = reader.suite()?;
        let pairwise_ciphers = reader.suite_list()?;
        let akm_suites = reader.suite_list()?;
        let capabilities = reader.optional(Reader::u16)?;
        let pmkids = reader.optional(|reader| {
            let count = reader.u16()?;
            let length = usize::from(count)
                .checked_mul(RSN_PMKID_LEN)
                .ok_or(RsnSyntaxError::Malformed)?;
            // take accepted exactly count complete PMKIDs; the remainder of
            // this array view is empty by the length calculation above.
            Ok(reader.take(length)?.as_chunks::<RSN_PMKID_LEN>().0)
        })?;
        let group_management_cipher = reader.optional(Reader::suite)?;
        if !reader.bytes.is_empty() {
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
            Some(pmkids) => Some(pmkids.len() as u16),
            None => None,
        }
    }

    /// The listed PMKIDs, in order.
    pub fn pmkids(&self) -> impl Iterator<Item = [u8; RSN_PMKID_LEN]> + use<'a> {
        self.pmkids.unwrap_or(&[]).iter().copied()
    }

    pub const fn group_management_cipher(&self) -> Option<[u8; 4]> {
        self.group_management_cipher
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], RsnSyntaxError> {
        let (taken, rest) = self
            .bytes
            .split_at_checked(length)
            .ok_or(RsnSyntaxError::Malformed)?;
        self.bytes = rest;
        Ok(taken)
    }

    fn array<const N: usize>(&mut self) -> Result<&'a [u8; N], RsnSyntaxError> {
        let (taken, rest) = self
            .bytes
            .split_first_chunk::<N>()
            .ok_or(RsnSyntaxError::Malformed)?;
        self.bytes = rest;
        Ok(taken)
    }

    fn u16(&mut self) -> Result<u16, RsnSyntaxError> {
        Ok(u16::from_le_bytes(*self.array()?))
    }

    fn suite(&mut self) -> Result<[u8; 4], RsnSyntaxError> {
        Ok(*self.array()?)
    }

    fn suite_list(&mut self) -> Result<RsnSuiteList<'a>, RsnSyntaxError> {
        let count = usize::from(self.u16()?);
        let length = count
            .checked_mul(SUITE_LEN)
            .ok_or(RsnSyntaxError::Malformed)?;
        Ok(RsnSuiteList {
            // Exactly count complete selectors; no remainder can be lost.
            suites: self.take(length)?.as_chunks::<SUITE_LEN>().0,
        })
    }

    fn optional<T>(
        &mut self,
        read: impl FnOnce(&mut Self) -> Result<T, RsnSyntaxError>,
    ) -> Result<Option<T>, RsnSyntaxError> {
        if self.bytes.is_empty() {
            Ok(None)
        } else {
            read(self).map(Some)
        }
    }
}

#[cfg(test)]
mod tests;
